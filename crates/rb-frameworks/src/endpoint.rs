//! Endpoint normalisation (design §10.1, contract M4a S3) and path matching (D9).
//!
//! One pure, total, idempotent function, [`norm_path`], serves server routes and client URLs
//! alike: parameters become `{}`, catch-alls `{*}` (last segment only), duplicate slashes
//! collapse, the trailing slash goes, a leading slash is added, query and fragment are dropped and
//! spaces are percent-encoded. Client templates ([`client_url`]) map holes to `{}` the same way.

/// The method of a route that accepts every method, and of a client whose method is unknown.
pub const ANY: &str = "ANY";

/// One normalised path segment.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Segment {
    Lit(String),
    /// `{}`: a path parameter (a client hole).
    Param,
    /// `{*}`: zero or more trailing segments (last segment only).
    Wild,
}

/// What normalisation noticed about its input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Flags {
    /// A segment mixes literal text and a parameter or hole (`file.:ext`, `/u/${id}.json`).
    pub partial: bool,
    /// A catch-all before the last segment, read as `{}`.
    pub wildcard_not_last: bool,
}

/// `method` upper-cased; `*` and `ALL` are [`ANY`].
pub fn norm_method(method: &str) -> String {
    let m = method.trim().to_ascii_uppercase();
    if m == "*" || m == "ALL" {
        ANY.to_owned()
    } else {
        m
    }
}

/// The normalised form of a route or URL path (see the module docs). Never fails.
pub fn norm_path(raw: &str) -> String {
    norm_path_flags(raw).0
}

/// [`norm_path`] plus what it noticed.
pub fn norm_path_flags(raw: &str) -> (String, Flags) {
    let mut flags = Flags::default();
    let pieces = split_segments(raw);
    let last = pieces.len().saturating_sub(1);
    let segments: Vec<Segment> = pieces
        .into_iter()
        .enumerate()
        .filter_map(|(i, seg)| {
            let s = classify(&seg, &mut flags)?;
            Some(match s {
                Segment::Wild if i != last => {
                    flags.wildcard_not_last = true;
                    Segment::Param
                }
                s => s,
            })
        })
        .collect();
    (render(&segments), flags)
}

/// The segments of a normalised path (`{}` and `{*}` read back).
pub fn segments(path: &str) -> Vec<Segment> {
    path.split('/')
        .filter(|s| !s.is_empty())
        .map(|s| match s {
            "{}" => Segment::Param,
            "{*}" => Segment::Wild,
            lit => Segment::Lit(lit.to_owned()),
        })
        .collect()
}

pub fn render(segments: &[Segment]) -> String {
    let mut out = String::new();
    for s in segments {
        out.push('/');
        match s {
            Segment::Lit(l) => out.push_str(l),
            Segment::Param => out.push_str("{}"),
            Segment::Wild => out.push_str("{*}"),
        }
    }
    if out.is_empty() {
        out.push('/');
    }
    out
}

/// Splits a raw path into non-empty segments, stopping at a query (`?`) or fragment (`#`)
/// outside braces. A `?` that ends a `:name` segment is Express's optional marker, not a query.
/// Braces protect `/`, `?` and `#` only when every brace is closed.
fn split_segments(raw: &str) -> Vec<String> {
    let protect = balanced(raw, '{', '}').is_some();
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let chars: Vec<char> = raw.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        let top = depth == 0 || !protect;
        match c {
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            '/' if top => {
                segments.push(std::mem::take(&mut current));
                continue;
            }
            '?' if top => {
                let optional = current.starts_with(':')
                    && current.len() > 1
                    && chars.get(i + 1).is_none_or(|n| *n == '/');
                if !optional {
                    break;
                }
            }
            '#' if top => break,
            _ => {}
        }
        current.push(c);
    }
    segments.push(current);
    segments.retain(|s| !s.is_empty());
    segments
}

/// One raw segment as a normalised segment; `None` for Go's `{$}`.
fn classify(seg: &str, flags: &mut Flags) -> Option<Segment> {
    match seg {
        "{$}" => return None,
        "{}" => return Some(Segment::Param),
        "{*}" | "*" | "**" => return Some(Segment::Wild),
        _ => {}
    }
    if let Some(name) = seg.strip_prefix(':') {
        if name.ends_with('*') || name.ends_with('+') {
            return Some(Segment::Wild);
        }
        return Some(Segment::Param);
    }
    if seg.starts_with('*') {
        return Some(Segment::Wild);
    }
    let whole = |open: char, close: char| {
        seg.starts_with(open) && seg.ends_with(close) && balanced(seg, open, close) == Some(1)
    };
    if whole('{', '}') {
        let inner = &seg[1..seg.len() - 1];
        return Some(if inner.starts_with('*') || inner.ends_with("...") {
            Segment::Wild
        } else {
            Segment::Param
        });
    }
    if whole('<', '>') {
        let inner = &seg[1..seg.len() - 1];
        return Some(if inner.starts_with("path:") {
            Segment::Wild
        } else {
            Segment::Param
        });
    }
    // A parameter inside a segment (`file.{ext}`, `a<int:b>`, `file.:ext`).
    let embedded = balanced(seg, '{', '}').is_some_and(|n| n > 0)
        || balanced(seg, '<', '>').is_some_and(|n| n > 0)
        || seg.contains(".:")
        || seg.contains("-:");
    if embedded {
        flags.partial = true;
        return Some(Segment::Param);
    }
    Some(Segment::Lit(seg.replace(' ', "%20")))
}

/// The number of top-level `open … close` groups when every group is closed, else `None`.
fn balanced(s: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    let mut groups = 0;
    for c in s.chars() {
        if c == open {
            if depth == 0 {
                groups += 1;
            }
            depth += 1;
        } else if c == close {
            depth = depth.checked_sub(1)?;
        }
    }
    (depth == 0).then_some(groups)
}

/// A piece of a client URL template after folding: literal text, or a hole nothing resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Lit(String),
    Hole,
}

/// A client URL split and normalised (contract M4a S3).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClientUrl {
    /// The template starts with an unresolved hole: the base is unknown (S12).
    pub base_unknown: bool,
    pub scheme: Option<String>,
    /// Lower-cased, without the port. `None` for a relative URL or a host with a hole.
    pub host: Option<String>,
    pub port: Option<String>,
    /// An absolute URL whose host has a hole.
    pub host_dynamic: bool,
    /// The normalised path.
    pub path: String,
    pub flags: Flags,
}

/// Splits and normalises a client URL template. A leading hole is the base; a hole filling a
/// whole segment is `{}`; a hole inside a segment makes the segment `{}`, flagged `partial`.
pub fn client_url(pieces: &[Piece]) -> ClientUrl {
    let mut out = ClientUrl::default();
    let mut pieces: Vec<Piece> = merge(pieces);
    if pieces.first() == Some(&Piece::Hole) {
        out.base_unknown = true;
        pieces.remove(0);
    }
    // An absolute URL: `scheme://authority/path`, or protocol-relative `//authority/path`.
    if let Some(Piece::Lit(first)) = pieces.first().cloned() {
        let after_scheme = first
            .find("://")
            .filter(|&i| {
                first[..i]
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
                    && i > 0
            })
            .map(|i| (Some(first[..i].to_ascii_lowercase()), i + 3))
            .or_else(|| first.starts_with("//").then_some((None, 2)));
        if let Some((scheme, start)) = after_scheme.filter(|_| !out.base_unknown) {
            out.scheme = scheme;
            let rest = &first[start..];
            let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
            let authority = &rest[..end];
            if end == rest.len() && pieces.len() > 1 {
                // A hole inside the authority (`http://${host}/x`).
                out.host_dynamic = true;
                pieces.remove(0);
                // Skip the rest of the authority, up to the first `/` of a later literal.
                while let Some(p) = pieces.first().cloned() {
                    match p {
                        Piece::Hole => {
                            pieces.remove(0);
                        }
                        Piece::Lit(l) => match l.find(['/', '?', '#']) {
                            Some(i) => {
                                pieces[0] = Piece::Lit(l[i..].to_owned());
                                break;
                            }
                            None => {
                                pieces.remove(0);
                            }
                        },
                    }
                }
            } else {
                let authority = authority.rsplit('@').next().unwrap_or(authority);
                let (host, port) = split_port(authority);
                out.host = Some(host.to_ascii_lowercase());
                out.port = port.map(str::to_owned);
                pieces[0] = Piece::Lit(rest[end..].to_owned());
            }
        }
    }
    // Segments as lists of pieces; stop at a query or fragment in literal text.
    let mut segs: Vec<Vec<Piece>> = vec![Vec::new()];
    'outer: for p in pieces {
        match p {
            Piece::Hole => segs.last_mut().expect("never empty").push(Piece::Hole),
            Piece::Lit(text) => {
                let mut current = String::new();
                for c in text.chars() {
                    match c {
                        '/' => {
                            if !current.is_empty() {
                                segs.last_mut()
                                    .expect("never empty")
                                    .push(Piece::Lit(std::mem::take(&mut current)));
                            }
                            segs.push(Vec::new());
                        }
                        '?' | '#' => {
                            if !current.is_empty() {
                                segs.last_mut()
                                    .expect("never empty")
                                    .push(Piece::Lit(current));
                            }
                            break 'outer;
                        }
                        c => current.push(c),
                    }
                }
                if !current.is_empty() {
                    segs.last_mut()
                        .expect("never empty")
                        .push(Piece::Lit(current));
                }
            }
        }
    }
    let segs: Vec<Vec<Piece>> = segs.into_iter().filter(|s| !s.is_empty()).collect();
    let last = segs.len().saturating_sub(1);
    let mut flags = Flags::default();
    let normalised: Vec<Segment> = segs
        .iter()
        .enumerate()
        .filter_map(|(i, seg)| {
            let s = match seg.as_slice() {
                [Piece::Lit(l)] => classify(l, &mut flags)?,
                [Piece::Hole] => Segment::Param,
                _ => {
                    flags.partial = true;
                    Segment::Param
                }
            };
            Some(match s {
                Segment::Wild if i != last => {
                    flags.wildcard_not_last = true;
                    Segment::Param
                }
                s => s,
            })
        })
        .collect();
    out.path = render(&normalised);
    out.flags = flags;
    out
}

fn merge(pieces: &[Piece]) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for p in pieces {
        match (out.last_mut(), p) {
            (Some(Piece::Lit(a)), Piece::Lit(b)) => a.push_str(b),
            (_, Piece::Lit(b)) if b.is_empty() => {}
            (_, p) => out.push(p.clone()),
        }
    }
    out
}

/// `host:port` → `(host, Some(port))`; IPv6 literals keep their brackets.
fn split_port(authority: &str) -> (&str, Option<&str>) {
    if authority.starts_with('[') {
        return match authority.find("]:") {
            Some(i) => (&authority[..=i], Some(&authority[i + 2..])),
            None => (authority, None),
        };
    }
    match authority.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) => (h, Some(p)),
        _ => (authority, None),
    }
}

/// How a client path matched an endpoint path (D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathMatch {
    /// The client's base was unknown and its known part matched the endpoint path's tail.
    pub suffix: bool,
    /// The endpoint's `{*}` took part.
    pub wildcard: bool,
}

/// Whether `client` matches `endpoint` segment by segment (D9): an endpoint `{}` matches any
/// client segment, a client `{}` only an endpoint `{}`, an endpoint `{*}` (last) zero or more
/// trailing client segments. Returns whether the `{*}` was used.
pub fn matches(client: &[Segment], endpoint: &[Segment]) -> Option<bool> {
    let wild = endpoint.last() == Some(&Segment::Wild);
    let fixed = if wild {
        &endpoint[..endpoint.len() - 1]
    } else {
        endpoint
    };
    if client.len() < fixed.len() || (!wild && client.len() != fixed.len()) {
        return None;
    }
    let ok = fixed.iter().zip(client).all(|(e, c)| match (e, c) {
        (Segment::Param, _) => true,
        (Segment::Lit(a), Segment::Lit(b)) => a == b,
        (Segment::Wild, _) => true,
        _ => false,
    });
    ok.then_some(wild)
}

/// Exact match of a client path, or, when its base is unknown, a match of its known part (with
/// at least one literal segment) against the endpoint path's tail.
pub fn match_path(
    client: &[Segment],
    base_unknown: bool,
    endpoint: &[Segment],
) -> Option<PathMatch> {
    if !base_unknown {
        return matches(client, endpoint).map(|wildcard| PathMatch {
            suffix: false,
            wildcard,
        });
    }
    if !client.iter().any(|s| matches!(s, Segment::Lit(_))) {
        return None;
    }
    (0..=endpoint.len()).find_map(|i| {
        matches(client, &endpoint[i..]).map(|wildcard| PathMatch {
            suffix: true,
            wildcard,
        })
    })
}

/// D9's specificity: the number of literal segments of an endpoint path. Among path matches the
/// highest wins, then an exact method over [`ANY`].
pub fn literal_segments(endpoint: &[Segment]) -> usize {
    endpoint
        .iter()
        .filter(|s| matches!(s, Segment::Lit(_)))
        .count()
}

/// Whether a client method and an endpoint method match (same, or either side [`ANY`]).
pub fn methods_match(client: &str, endpoint: &str) -> bool {
    client == endpoint || client == ANY || endpoint == ANY
}

/// The natural key of an HTTP endpoint (contract M4a D1).
pub fn endpoint_key(method: &str, path: &str, service: &str) -> String {
    format!("HTTP {method} {path} @{service}")
}

/// The display name of an HTTP endpoint.
pub fn endpoint_name(method: &str, path: &str) -> String {
    format!("{method} {path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_and_catch_all_forms() {
        for (raw, want) in [
            ("/users/:id", "/users/{}"),
            ("/users/:id?", "/users/{}"),
            ("/users/:id?/x", "/users/{}/x"),
            ("/users/{id}", "/users/{}"),
            ("/users/{id:\\d+}", "/users/{}"),
            ("/users/{id:[0-9]{2}}", "/users/{}"),
            ("/users/<int:id>", "/users/{}"),
            ("/users/<id>", "/users/{}"),
            ("/files/*", "/files/{*}"),
            ("/files/**", "/files/{*}"),
            ("/files/*rest", "/files/{*}"),
            ("/files/{*rest}", "/files/{*}"),
            ("/files/{rest...}", "/files/{*}"),
            ("/files/<path:p>", "/files/{*}"),
            ("/files/:p*", "/files/{*}"),
            ("/x/{$}", "/x"),
            ("{$}", "/"),
            ("/a/*/b", "/a/{}/b"),
        ] {
            assert_eq!(norm_path(raw), want, "{raw}");
        }
        assert!(norm_path_flags("/a/*/b").1.wildcard_not_last);
        assert!(!norm_path_flags("/a/b/*").1.wildcard_not_last);
        assert!(norm_path_flags("/f/file.:ext").1.partial);
        assert_eq!(norm_path("/f/file.:ext"), "/f/{}");
        assert_eq!(norm_path("/f/{name}.json"), "/f/{}");
    }

    #[test]
    fn slashes_queries_fragments_and_spaces() {
        for (raw, want) in [
            ("", "/"),
            ("/", "/"),
            ("//", "/"),
            ("api//v1///login/", "/api/v1/login"),
            ("/login?next=/x", "/login"),
            ("/login#top", "/login"),
            ("/a b/c", "/a%20b/c"),
            ("/a%20b", "/a%20b"),
            ("/a{b", "/a{b"),
        ] {
            assert_eq!(norm_path(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn methods() {
        assert_eq!(norm_method("post"), "POST");
        assert_eq!(norm_method(" Get "), "GET");
        assert_eq!(norm_method("all"), ANY);
        assert_eq!(norm_method("*"), ANY);
        assert_eq!(norm_method("ANY"), ANY);
    }

    /// Design §12.3: `norm(norm(x)) == norm(x)` over generated inputs. A fixed xorshift
    /// generator draws 20,000 paths from tokens that exercise every rule.
    #[test]
    fn normalisation_is_idempotent() {
        const TOKENS: &[&str] = &[
            "/",
            "//",
            "a",
            "B",
            ":id",
            ":id?",
            "?",
            "#",
            "{",
            "}",
            "{id}",
            "{id:\\d+}",
            "<",
            ">",
            "<int:x>",
            "<path:p>",
            "*",
            "**",
            "{*r}",
            "{r...}",
            "{$}",
            " ",
            "%20",
            ".",
            "-",
            ":",
            "{}",
            "{*}",
            "x.:e",
            "é",
            "?q=1",
        ];
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let n = (next() % 9) as usize;
            let raw: String = (0..n)
                .map(|_| TOKENS[(next() % TOKENS.len() as u64) as usize])
                .collect();
            let once = norm_path(&raw);
            assert_eq!(norm_path(&once), once, "{raw:?}");
            assert!(
                once.starts_with('/') && !once.contains(' '),
                "{raw:?} → {once}"
            );
            assert!(once == "/" || !once.ends_with('/'), "{raw:?} → {once}");
        }
    }

    fn lit(s: &str) -> Piece {
        Piece::Lit(s.to_owned())
    }

    #[test]
    fn client_templates() {
        let u = client_url(&[lit("http://Notification:3000/internal/mfa/email")]);
        assert_eq!(
            (
                u.scheme.as_deref(),
                u.host.as_deref(),
                u.port.as_deref(),
                u.path.as_str()
            ),
            (
                Some("http"),
                Some("notification"),
                Some("3000"),
                "/internal/mfa/email"
            )
        );
        assert!(!u.base_unknown);

        let u = client_url(&[Piece::Hole, lit("/api/v1/login")]);
        assert!(u.base_unknown && u.host.is_none());
        assert_eq!(u.path, "/api/v1/login");

        let u = client_url(&[Piece::Hole, lit("/invoices/"), Piece::Hole, lit("/pay?x=1")]);
        assert_eq!(u.path, "/invoices/{}/pay");
        assert!(!u.flags.partial);

        let u = client_url(&[lit("/u/"), Piece::Hole, lit(".json")]);
        assert_eq!(u.path, "/u/{}");
        assert!(u.flags.partial);

        let u = client_url(&[lit("http://"), Piece::Hole, lit(":8080/x/y")]);
        assert!(u.host_dynamic && u.host.is_none());
        assert_eq!(u.path, "/x/y");

        let u = client_url(&[lit("https://status.example.com")]);
        assert_eq!(
            (u.host.as_deref(), u.path.as_str()),
            (Some("status.example.com"), "/")
        );

        let u = client_url(&[lit("//cdn.example.com/a")]);
        assert_eq!(
            (u.scheme, u.host.as_deref()),
            (None, Some("cdn.example.com"))
        );

        let u = client_url(&[lit("http://[::1]:3000/x")]);
        assert_eq!(
            (u.host.as_deref(), u.port.as_deref()),
            (Some("[::1]"), Some("3000"))
        );

        // URI template variables are parameters (S10).
        assert_eq!(client_url(&[lit("/users/{id}")]).path, "/users/{}");
        // A relative URL with no hole.
        let u = client_url(&[lit("internal/x")]);
        assert_eq!((u.host, u.path.as_str()), (None, "/internal/x"));
        // Idempotent with the server normaliser.
        assert_eq!(norm_path(&u.path), u.path);
    }

    fn segs(p: &str) -> Vec<Segment> {
        segments(&norm_path(p))
    }

    #[test]
    fn d9_matching() {
        // An endpoint parameter matches any client segment.
        assert_eq!(matches(&segs("/u/42"), &segs("/u/{id}")), Some(false));
        assert_eq!(matches(&segs("/u/{}"), &segs("/u/{id}")), Some(false));
        // A client hole matches only an endpoint parameter.
        assert_eq!(matches(&segs("/u/{}"), &segs("/u/me")), None);
        // A wildcard matches zero or more trailing segments.
        assert_eq!(matches(&segs("/f"), &segs("/f/*")), Some(true));
        assert_eq!(matches(&segs("/f/a/b"), &segs("/f/*")), Some(true));
        assert_eq!(matches(&segs("/g/a"), &segs("/f/*")), None);
        // Lengths must agree without a wildcard.
        assert_eq!(matches(&segs("/a/b"), &segs("/a")), None);
        // Suffix: an unknown base and a known tail with a literal.
        let m = match_path(
            &segs("/invoices/{}/pay"),
            true,
            &segs("/billing/invoices/:id/pay"),
        );
        assert_eq!(
            m,
            Some(PathMatch {
                suffix: true,
                wildcard: false
            })
        );
        assert_eq!(match_path(&segs("/{}"), true, &segs("/a/{}")), None);
        assert_eq!(
            match_path(
                &segs("/invoices/{}/pay"),
                false,
                &segs("/billing/invoices/:id/pay")
            ),
            None
        );
        // Specificity and methods.
        assert!(literal_segments(&segs("/u/me")) > literal_segments(&segs("/u/{id}")));
        assert!(
            methods_match("POST", "POST")
                && methods_match("POST", ANY)
                && methods_match(ANY, "GET")
        );
        assert!(!methods_match("POST", "GET"));
    }

    #[test]
    fn keys() {
        assert_eq!(
            endpoint_key("POST", "/api/v1/login", "services/auth"),
            "HTTP POST /api/v1/login @services/auth"
        );
        assert_eq!(endpoint_name("POST", "/api/v1/login"), "POST /api/v1/login");
        // ` @` never occurs inside a normalised path: spaces are encoded.
        assert!(!norm_path("/a @b").contains(" @"));
    }
}
