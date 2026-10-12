//! Gateway packs (design §6.5, contract M6g S3): nginx `location`+`proxy_pass` routes and
//! Spring Cloud Gateway `Path`+`uri` routes as rewrite rules (prefix → service host). Literals
//! only; regex locations, rewrites and dynamics stay unread and counted. The stitcher applies
//! them; this module never touches the graph.

use crate::work::tick;
use rb_syntax::Span;

/// One gateway route: calls under `prefix` reach `target` (a service host as written).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GatewayRule {
    pub prefix: String,
    pub target: String,
    pub span: Span,
}

/// One gateway file read: its rules and its unread-block count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatewayFile {
    pub rules: Vec<GatewayRule>,
    pub unread: usize,
}

/// A gateway file, by file name: `nginx.conf` or Spring `application*`/`bootstrap*` YAML.
pub fn is_gateway_input(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    name == "nginx.conf"
        || ((name.starts_with("application") || name.starts_with("bootstrap"))
            && (name.ends_with(".yml") || name.ends_with(".yaml")))
}

/// Reads nginx `location` blocks: a literal prefix with exactly one `proxy_pass
/// http(s)://host…` and no `rewrite` yields a rule. `~` regex locations, variable targets,
/// rewrite blocks and malformed openers stay unread and counted. Braces inside `#` comments
/// are ignored; braces inside quoted strings would miscount (no such shape in gateway files).
pub fn nginx_gateway(text: &str) -> GatewayFile {
    let mut out = GatewayFile::default();
    let lines: Vec<&str> = text.split('\n').collect();
    // Depth before each line, on comment-stripped code.
    let code: Vec<String> = lines
        .iter()
        .map(|l| l.split('#').next().unwrap_or("").to_owned())
        .collect();
    let mut depths = vec![0i64];
    for line in &code {
        tick(1);
        let depth = depths.last().copied().unwrap_or(0);
        let next = depth + line.matches('{').count() as i64 - line.matches('}').count() as i64;
        depths.push(next.max(0));
    }
    let mut i = 0;
    while i < lines.len() {
        tick(1);
        let trimmed = code[i].trim();
        let Some(rest) = trimmed
            .strip_prefix("location")
            .map(str::trim)
            .filter(|r| !r.is_empty())
        else {
            i += 1;
            continue;
        };
        // `=` strips to its path; `~` regex locations read as no pattern (unread below);
        // `^~` strips to its path while a bare `^` is malformed (unread below).
        let pattern = match rest.chars().next() {
            Some('=') => rest[1..].trim(),
            Some('~') => "",
            Some('^') => match rest.strip_prefix("^~") {
                Some(after) => after.trim(),
                None => "",
            },
            _ => rest,
        };
        let pattern = pattern.split([' ', '\t', '{']).next().unwrap_or("").trim();
        // The block runs to the first line back at (or above) the opener's depth; a
        // single-line block also contributes its opener's remainder.
        let mut j = i + 1;
        while j < lines.len() && depths[j] > depths[i] {
            j += 1;
        }
        let remainder = lines[i]
            .split_once('{')
            .map(|(_, after)| after.split('}').next().unwrap_or("").to_owned())
            .unwrap_or_default();
        let span = [i as i32, 0, i as i32, lines[i].len() as i32];
        if pattern.is_empty() || !pattern.starts_with('/') {
            out.unread += 1;
        } else {
            let mut body: Vec<String> = code[i + 1..j.min(code.len())].to_vec();
            if !remainder.trim().is_empty() {
                body.insert(0, remainder);
            }
            let passes: Vec<&str> = body
                .iter()
                .filter_map(|l| {
                    let t = l.trim();
                    t.strip_prefix("proxy_pass")
                        .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
                        .map(str::trim)
                })
                .collect();
            let rewritten = body.iter().any(|l| {
                l.trim()
                    .strip_prefix("rewrite")
                    .is_some_and(|r| r.is_empty() || r.starts_with(char::is_whitespace))
            });
            match passes.as_slice() {
                [one] if !rewritten => match proxy_host(one) {
                    Some(target) => out.rules.push(GatewayRule {
                        prefix: pattern.to_owned(),
                        target,
                        span,
                    }),
                    None => out.unread += 1,
                },
                [] if !rewritten => {}
                _ => out.unread += 1,
            }
        }
        i = j.max(i + 1);
    }
    out
}

/// The host of a `proxy_pass` value (`http(s)://host[:port]`), or `None` for variable
/// targets (`$host`), other schemes, and values with a URI path: a path replaces the matched
/// prefix instead of stripping it (evaluator pass 1, P9), so it never reads as a rewrite.
fn proxy_host(value: &str) -> Option<String> {
    let value = value.strip_suffix(';').unwrap_or(value).trim();
    let after = value
        .strip_prefix("http://")
        .or_else(|| value.strip_prefix("https://"))?;
    if after.contains('$') {
        return None;
    }
    let mut parts = after.splitn(2, '/');
    let host = parts.next()?.trim();
    if host.is_empty() {
        return None;
    }
    let host = host.split(':').next()?.trim();
    if host.is_empty() {
        return None;
    }
    match parts.next() {
        None | Some("") => Some(host.to_owned()),
        Some(_) => None,
    }
}

/// Reads Spring Cloud Gateway routes: a `routes:` list (at any indent) whose items state a
/// `uri:` (http(s) or `lb://`) and `predicates:` `Path=` entries. Tabs make the file
/// unreadable; anything else unshaped is skipped per block and counted.
pub fn spring_gateway(text: &str) -> GatewayFile {
    let mut out = GatewayFile::default();
    struct Block {
        uri: Option<String>,
        paths: Vec<String>,
        span: Span,
        /// A pattern already counted unread: the flush does not count the block again.
        spoiled: bool,
    }
    let mut current: Option<(Block, usize)> = None;
    let mut routes_indent: Option<usize> = None;
    let mut in_predicates = false;
    for (row, line) in text.split('\n').enumerate() {
        tick(1);
        if line.contains('\t') {
            out.rules.clear();
            out.unread += 1;
            return out;
        }
        let row = row as i32;
        let indent = line.len() - line.trim_start_matches(' ').len();
        let content = line.trim();
        if content.is_empty() || content.starts_with('#') || content == "---" {
            continue;
        }
        if content == "routes:" && routes_indent.is_none() {
            routes_indent = Some(indent);
            continue;
        }
        let Some(routes_at) = routes_indent else {
            continue;
        };
        if indent <= routes_at {
            // Left the routes section.
            if let Some((block, _)) = current.take() {
                flush_block(&mut out, block.uri, block.paths, block.span, block.spoiled);
            }
            routes_indent = None;
            in_predicates = false;
            // A second `routes:` section restarts (multi-document files use `---`, which
            // continues above; only one section reads to keep shadowing explicit).
            continue;
        }
        if content.starts_with("- ") {
            let starts_block = match &current {
                None => indent > routes_at,
                Some((_, item_at)) => indent <= *item_at,
            };
            if starts_block {
                if let Some((block, _)) = current.take() {
                    flush_block(&mut out, block.uri, block.paths, block.span, block.spoiled);
                }
                current = Some((
                    Block {
                        uri: None,
                        paths: Vec::new(),
                        span: [row, 0, row, line.len() as i32],
                        spoiled: false,
                    },
                    indent,
                ));
                in_predicates = false;
            }
        }
        let Some((block, item_at)) = current.as_mut() else {
            continue;
        };
        if indent <= *item_at
            && !content.starts_with("- ")
            && !(item_at == &indent && content.starts_with("uri:"))
        {
            // Dedented past the item: it ends here (a `- uri:` item line itself is handled
            // below through the stripped field).
            if let Some((block, _)) = current.take() {
                flush_block(&mut out, block.uri, block.paths, block.span, block.spoiled);
            }
            in_predicates = false;
            continue;
        }
        let field = content.strip_prefix("- ").unwrap_or(content);
        if field == "predicates:" {
            in_predicates = true;
        } else if let Some(rest) = field.strip_prefix("uri:") {
            block.uri = gateway_uri(rest.trim());
        } else if in_predicates && let Some(pattern) = field.strip_prefix("Path=") {
            let pattern = pattern.split_whitespace().next().unwrap_or("");
            match trim_gateway_pattern(pattern) {
                Some(prefix) => block.paths.push(prefix),
                None => {
                    out.unread += 1;
                    block.spoiled = true;
                }
            }
        }
    }
    if let Some((block, _)) = current.take() {
        flush_block(&mut out, block.uri, block.paths, block.span, block.spoiled);
    }
    out
}

/// A finished route block: one rule per path when it has a uri, else one unread mark.
fn flush_block(
    out: &mut GatewayFile,
    uri: Option<String>,
    paths: Vec<String>,
    span: Span,
    spoiled: bool,
) {
    match uri {
        Some(uri) if !paths.is_empty() => {
            for path in paths {
                out.rules.push(GatewayRule {
                    prefix: path,
                    target: uri.clone(),
                    span,
                });
            }
        }
        _ if !spoiled => out.unread += 1,
        _ => {}
    }
}

/// A gateway `uri:` as a service host: `http(s)://host…` or `lb://service`. Anything else
/// never reads — including values with a URI path (evaluator pass 1, P9).
fn gateway_uri(value: &str) -> Option<String> {
    let value = value.split_whitespace().next().unwrap_or("");
    if let Some(rest) = value.strip_prefix("lb://") {
        let mut parts = rest.splitn(2, '/');
        let name = parts.next().unwrap_or("").trim();
        if name.is_empty() || name.contains([':', '$']) {
            return None;
        }
        return match parts.next() {
            None | Some("") => Some(name.to_owned()),
            Some(_) => None,
        };
    }
    proxy_host(value)
}

/// A `Path=` pattern as a prefix (`/api/**` → `/api/`). `None` for anything without a leading
/// `/` or with a wildcard outside a trailing `/**` (fail-open: the route stays unread).
fn trim_gateway_pattern(pattern: &str) -> Option<String> {
    if !pattern.starts_with('/') {
        return None;
    }
    let mut out = pattern.to_owned();
    while out.ends_with("/**") {
        out.truncate(out.len() - 3);
    }
    if out.contains('*') {
        return None;
    }
    if out.is_empty() {
        return Some("/".to_owned());
    }
    if !out.ends_with('/') {
        out.push('/');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work;

    /// Contract M6g S3: nginx locations pin prefix + proxy host (single- and multi-line);
    /// regex locations, rewrites, variable targets, path-bearing targets (evaluator pass 1,
    /// P9: a URI path replaces the prefix instead of stripping it) and malformed blocks stay
    /// unread.
    #[test]
    fn nginx_locations_pin_prefix_and_proxy_host() {
        let text = "server {\n  listen 80;\n  location /api/ { proxy_pass http://auth:8080; }\n  location = /health {\n    proxy_pass http://auth:8080/health;\n  }\n  location /slash/ { proxy_pass http://auth:8080/; }\n  location ~ ^/old { proxy_pass http://auth:8080; }\n  location /var/ {\n    proxy_pass $backend;\n  }\n  location /rw/ {\n    rewrite ^/rw/(.*) /$1 break;\n    proxy_pass http://auth:8080;\n  }\n  location /empty/ {\n  }\n}\n";
        let file = nginx_gateway(text);
        assert_eq!(
            file.rules,
            [
                GatewayRule {
                    prefix: "/api/".into(),
                    target: "auth".into(),
                    span: [2, 0, 2, 49],
                },
                GatewayRule {
                    prefix: "/slash/".into(),
                    target: "auth".into(),
                    span: [6, 0, 6, 52],
                },
            ]
        );
        assert_eq!(file.unread, 4);
    }

    /// Contract M6g S3: Spring Gateway routes pin Path prefixes + uri hosts (`lb://` included);
    /// tabs fail the file; unshaped blocks are counted.
    #[test]
    fn spring_gateway_routes_pin_paths_and_uris() {
        let text = "spring:\n  cloud:\n    gateway:\n      routes:\n        - id: auth\n          uri: http://auth:8080\n          predicates:\n            - Path=/api/**\n        - id: billing\n          uri: lb://billing\n          predicates:\n            - Path=/billing/**\n            - Method=GET\n        - id: wild\n          uri: lb://wild\n          predicates:\n            - Path=/w/*\n        - id: broken\n          predicates:\n            - Path=/no-uri/**\n";
        let file = spring_gateway(text);
        // Positions come from the tree... the block lines, never hand-counted: recomputed
        // below from the text.
        let span_of = |needle: &str| {
            let pos = text.find(needle).unwrap();
            let row = text[..pos].matches('\n').count() as i32;
            let end = text[pos..].find('\n').map_or(text.len(), |n| pos + n);
            [row, 0, row, (end - pos) as i32]
        };
        assert_eq!(
            file.rules,
            [
                GatewayRule {
                    prefix: "/api/".into(),
                    target: "auth".into(),
                    span: span_of("        - id: auth"),
                },
                GatewayRule {
                    prefix: "/billing/".into(),
                    target: "billing".into(),
                    span: span_of("        - id: billing"),
                },
            ]
        );
        // A single-`*` tail is not a plain prefix (it would over-match deeper paths), and the
        // uri-less block names nothing: both unread, nothing silently folded.
        assert_eq!(file.unread, 2);
        assert!(spring_gateway("routes:\n\t- id: x\n").rules.is_empty());
        assert!(is_gateway_input("gw/nginx.conf"));
        assert!(is_gateway_input("svc/application-prod.yaml"));
        assert!(!is_gateway_input("svc/application.properties"));
        assert!(!is_gateway_input("svc/other.conf"));
    }

    /// Contract M6g S3, invariants 5–6: reading 4× the routes costs at most 5× the operations.
    #[test]
    fn gateway_reading_grows_linearly() {
        let work = |n: usize| {
            let mut text = String::from("server {\n");
            for i in 0..n {
                text.push_str(&format!(
                    "  location /s{i}/ {{ proxy_pass http://svc{i}:8080; }}\n"
                ));
            }
            text.push_str("}\n");
            work::take();
            let file = nginx_gateway(&text);
            assert_eq!(file.rules.len(), n);
            work::take()
        };
        let (small, large) = (work(50), work(200));
        assert!(large <= small * 5, "{small} -> {large}");
    }
}
