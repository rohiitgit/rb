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
    /// Client URLs: a hole ending the URL glued to literal text (`"/api/items" + qs`) was
    /// dropped as an opaque tail (a query, fragment or extension; evaluator pass 1, P3).
    pub opaque_tail: bool,
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
    /// The part before the path is unknown, so the path is matched by suffix: the template
    /// starts with an unresolved hole (S12), the host has a hole and is not external, or a hole
    /// follows a complete authority directly (`http://orders:3000${p}/x`; amendment 12).
    pub base_unknown: bool,
    pub scheme: Option<String>,
    /// Lower-cased, without the port. `None` for a relative URL or a host with a hole.
    pub host: Option<String>,
    pub port: Option<String>,
    /// An absolute URL whose host has a hole.
    pub host_dynamic: bool,
    /// A host with a hole next to literal host text holding a `.` (`{?}.zendesk.com`,
    /// `api.{?}`): an FQDN that names no service (D6), so the site is external (amendment 12).
    /// The host as written, lower-cased, `{?}` for each hole.
    pub host_external: Option<String>,
    /// The normalised path.
    pub path: String,
    pub flags: Flags,
}

/// Splits and normalises a client URL template. A leading hole is the base; a hole filling a
/// whole segment is `{}`; a hole inside a segment makes the segment `{}`, flagged `partial`.
pub fn client_url(pieces: &[Piece]) -> ClientUrl {
    let mut out = ClientUrl::default();
    let mut pieces: Vec<Piece> = merge(pieces);
    strip_userinfo_hole(&mut pieces);
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
            let authority = authority.rsplit('@').next().unwrap_or(authority);
            if end == rest.len() && pieces.len() > 1 {
                // A hole follows the literal authority. Only a hole inside the host makes the
                // host unknown (evaluator pass 1, P1): `http://localhost:${port}/x` keeps
                // `localhost` with an unknown port, `http://${host}/x` and `http://api.${d}/x`
                // do not keep a host.
                // An `@` later in the authority puts the host after user information that
                // holds the hole (`http://user:${pass}@host/x`): not literal before the hole.
                let end = if userinfo_follows(&pieces[1..]) {
                    AuthorityEnd::InHost
                } else {
                    authority_before_hole(authority)
                };
                match end {
                    AuthorityEnd::Port(host) => {
                        out.host = Some(host.to_ascii_lowercase());
                        // The literal, then the port hole. A further hole before the path's
                        // first `/` may hold a path prefix: the path does not start with a
                        // literal, so it is matched by suffix within the host (amendment 13,
                        // Gate B; evaluator pass 3, R4).
                        pieces.drain(..2);
                        if pieces.first() == Some(&Piece::Hole) {
                            out.base_unknown = true;
                        }
                        skip_authority(&mut pieces);
                    }
                    AuthorityEnd::Complete => {
                        let (host, port) = split_port(authority);
                        out.host = Some(host.to_ascii_lowercase());
                        out.port = port.map(str::to_owned);
                        pieces.remove(0);
                        // The hole glued to the authority may continue the port or hold a path
                        // prefix: an unknown prefix, so the rest is matched by suffix (Q6).
                        while pieces.first() == Some(&Piece::Hole) {
                            pieces.remove(0);
                            out.base_unknown = true;
                        }
                    }
                    AuthorityEnd::InHost => {
                        out.host_dynamic = true;
                        let host = host_with_holes(authority, &pieces[1..]);
                        // A literal part with a dot is an FQDN fragment: external. Otherwise the
                        // host is unknown and the unresolved-base rules apply (Q2).
                        if host.split(HOLE).any(|part| part.contains('.')) {
                            out.host_external = Some(host);
                        } else {
                            out.base_unknown = true;
                        }
                        pieces.remove(0);
                        skip_authority(&mut pieces);
                    }
                }
            } else {
                let (host, port) = split_port(authority);
                out.host = Some(host.to_ascii_lowercase());
                out.port = port.map(str::to_owned);
                pieces[0] = Piece::Lit(rest[end..].to_owned());
            }
        }
    }
    // Segments as lists of pieces; stop at a query or fragment in literal text.
    let mut segs: Vec<Vec<Piece>> = vec![Vec::new()];
    // Whether the URL ends with a hole that no `?` or `#` in literal text cut off.
    let mut ends_in_hole = pieces.last() == Some(&Piece::Hole);
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
                            ends_in_hole = false;
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
    let mut segs: Vec<Vec<Piece>> = segs.into_iter().filter(|s| !s.is_empty()).collect();
    let mut flags = Flags::default();
    // A hole ending the URL glued to literal text (`/api/items${qs}`) is an opaque tail: far
    // more often a query or an extension than a parameter, so it is dropped and flagged
    // (evaluator pass 1, P3).
    if ends_in_hole
        && let Some(seg) = segs.last_mut()
        && matches!(seg.first(), Some(Piece::Lit(_)))
        && seg.last() == Some(&Piece::Hole)
        && seg.iter().skip(1).all(|p| *p == Piece::Hole)
    {
        seg.truncate(1);
        flags.opaque_tail = true;
    }
    let last = segs.len().saturating_sub(1);
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

/// Where the literal authority before a hole stops.
enum AuthorityEnd<'a> {
    /// `host:` — the host is complete and the hole is (part of) the port.
    Port(&'a str),
    /// `host:digits` or a bracketed IPv6 literal (`[::1]`, `[::1]:3000`): the authority is
    /// complete and the hole follows it.
    Complete,
    /// Anything else (`""`, `api.`, `localhost`): the hole may be part of the host.
    InHost,
}

fn authority_before_hole(authority: &str) -> AuthorityEnd<'_> {
    if let Some(host) = authority.strip_suffix(':') {
        let plain = !host.is_empty() && !host.contains(['[', ']', ':']);
        let bracketed = host.starts_with('[') && host.ends_with(']') && host.len() > 2;
        if plain || bracketed {
            return AuthorityEnd::Port(host);
        }
        return AuthorityEnd::InHost;
    }
    if authority.starts_with('[') && authority.ends_with(']') && authority.len() > 2 {
        return AuthorityEnd::Complete;
    }
    match split_port(authority) {
        (host, Some(port)) if !host.is_empty() && !port.is_empty() => AuthorityEnd::Complete,
        _ => AuthorityEnd::InHost,
    }
}

/// How a hole is written in [`ClientUrl::host_external`].
const HOLE: &str = "{?}";

/// The host of an authority that continues past a hole: `authority` (the literal text before
/// the first hole) and the following pieces up to the first `/`, `?` or `#`, holes written
/// [`HOLE`], lower-cased; user information (up to the last `@`) and the port are dropped.
fn host_with_holes(authority: &str, rest: &[Piece]) -> String {
    let mut text = authority.to_owned();
    for p in rest {
        match p {
            Piece::Hole => text.push_str(HOLE),
            Piece::Lit(l) => match l.find(['/', '?', '#']) {
                Some(i) => {
                    text.push_str(&l[..i]);
                    break;
                }
                None => text.push_str(l),
            },
        }
    }
    let host = text.rsplit('@').next().unwrap_or(&text);
    let host = if host.starts_with('[') {
        host.find(']').map_or(host, |i| &host[..=i])
    } else {
        host.split(':').next().unwrap_or(host)
    };
    host.to_ascii_lowercase()
}

/// Whether an `@` appears in the authority's remaining literal text (before the first `/`, `?`
/// or `#` of a later literal).
fn userinfo_follows(pieces: &[Piece]) -> bool {
    for p in pieces {
        if let Piece::Lit(l) = p {
            let end = l.find(['/', '?', '#']);
            if l[..end.unwrap_or(l.len())].contains('@') {
                return true;
            }
            if end.is_some() {
                return false;
            }
        }
    }
    false
}

/// Drops the rest of an authority from the front of `pieces`: holes and literal text up to the
/// first `/`, `?` or `#` of a later literal.
fn skip_authority(pieces: &mut Vec<Piece>) {
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

/// `scheme://user…hole…@host`: drops the userinfo (with its holes) through
/// the last `@` in the authority, keeping `scheme://host` (contract M6b S7).
/// The unknown-ness stays confined to the userinfo: the host after `@`
/// parses (and matches, and earns certainty) exactly as if written
/// literally, and credentials never enter the pieces. A hole with no `@`
/// after it in the authority (a port hole), a hole for the host itself, and
/// a leading base hole are today's rules, untouched: only a hole strictly
/// before the last authority-`@` strips.
fn strip_userinfo_hole(pieces: &mut Vec<Piece>) {
    let Some(Piece::Lit(first)) = pieces.first().cloned() else {
        return;
    };
    // Where the authority starts: after a valid `scheme://`, or `//`.
    // Anything else has no authority to strip.
    let start = first
        .find("://")
        .filter(|&i| {
            first[..i]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
                && i > 0
        })
        .map(|i| i + 3)
        .or_else(|| first.starts_with("//").then_some(2));
    let Some(start) = start else {
        return;
    };
    // The last `@` in the authority (before any `/`, `?` or `#` literal
    // text), and the first hole there: only a hole strictly before the `@`
    // is userinfo.
    let mut at: Option<(usize, usize)> = None;
    let mut hole: Option<usize> = None;
    let mut authority = true;
    for (i, piece) in pieces.iter().enumerate() {
        if !authority {
            break;
        }
        match piece {
            Piece::Hole => {
                hole.get_or_insert(i);
            }
            Piece::Lit(text) => {
                let scan = if i == 0 {
                    &text[start..]
                } else {
                    text.as_str()
                };
                let end = scan.find(['/', '?', '#']).unwrap_or(scan.len());
                for (j, c) in scan[..end].char_indices() {
                    if c == '@' {
                        let base = if i == 0 { start } else { 0 };
                        at = Some((i, base + j));
                    }
                }
                if end < scan.len() {
                    authority = false;
                }
            }
        }
    }
    let (at_piece, at_byte) = match at {
        Some(at) if hole.is_some_and(|h| h < at.0) => at,
        _ => return,
    };
    let mut kept = first[..start].to_owned();
    if let Piece::Lit(text) = &pieces[at_piece] {
        kept.push_str(&text[at_byte + 1..]);
    }
    pieces.splice(..=at_piece, [Piece::Lit(kept)]);
}

/// `host:port` → `(host, Some(port))`; IPv6 literals keep their brackets.
pub(crate) fn split_port(authority: &str) -> (&str, Option<&str>) {
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
/// at least one literal segment) against the endpoint path's tail. The matched tail must hold a
/// literal segment too: a tail of parameters and catch-alls is no evidence that the client calls
/// that endpoint (evaluator pass 1, P2).
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
        let tail = &endpoint[i..];
        if literal_segments(tail) == 0 {
            return None;
        }
        matches(client, tail).map(|wildcard| PathMatch {
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

/// The key of an endpoint whose prefix is unknown (contract M4a amendment 10, fork F3): the D1
/// key of its tail followed by ` ?router=<router>`, the identity of the router or group the
/// route is registered on, so routes with one tail on different routers stay apart. The
/// normaliser drops query strings, so ` ?` never occurs in a normalised path.
pub fn prefix_unknown_key(method: &str, path: &str, service: &str, router: &str) -> String {
    format!("{} ?router={router}", endpoint_key(method, path, service))
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
        assert!(u.host_dynamic && u.host.is_none() && u.base_unknown);
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

    /// Evaluator pass 1, P1: a hole in the port (or anything after the host) keeps a literal
    /// host; only a hole inside the host itself makes it unknown.
    #[test]
    fn a_port_hole_keeps_the_host() {
        let host = |pieces: &[Piece]| {
            let u = client_url(pieces);
            (u.host, u.port, u.host_dynamic, u.path)
        };
        for (first, want) in [
            ("http://localhost:", "localhost"),
            ("https://status.example.com:", "status.example.com"),
            ("http://Users:", "users"),
            ("http://127.0.0.1:", "127.0.0.1"),
            ("http://[::1]:", "[::1]"),
            ("http://admin@localhost:", "localhost"),
            ("//cdn.example.com:", "cdn.example.com"),
        ] {
            assert_eq!(
                host(&[lit(first), Piece::Hole, lit("/orders/1")]),
                (Some(want.to_owned()), None, false, "/orders/1".to_owned()),
                "{first}"
            );
        }
        // Several holes and literal digits in the port are all port.
        assert_eq!(
            host(&[
                lit("http://users:"),
                Piece::Hole,
                lit("0"),
                Piece::Hole,
                lit("/x")
            ]),
            (Some("users".to_owned()), None, false, "/x".to_owned())
        );
        // A literal port and a hole after the authority: the host is known (the hole is an
        // unknown prefix, amendment 12).
        let u = client_url(&[lit("http://users:3000"), Piece::Hole, lit("/x")]);
        assert_eq!(
            (u.host.as_deref(), u.port.as_deref(), u.host_dynamic),
            (Some("users"), Some("3000"), false)
        );
        // A hole in user information: the literal host after `@` is kept
        // (contract M6b S7) — only the userinfo is unknown.
        for (rest, host, port) in [
            ("@users/x", "users", None),
            ("@users:3000/x", "users", Some("3000")),
        ] {
            let u = client_url(&[lit("http://admin:"), Piece::Hole, lit(rest)]);
            assert_eq!(u.host.as_deref(), Some(host), "{rest}");
            assert_eq!(u.port.as_deref(), port, "{rest}");
            assert!(
                !u.host_dynamic && u.host_external.is_none() && !u.base_unknown,
                "{rest}"
            );
            assert_eq!(u.path, "/x", "{rest}");
        }
        // A hole inside the host: unknown.
        for first in ["http://", "http://localhost", "http://users-"] {
            let u = client_url(&[lit(first), Piece::Hole, lit("/x")]);
            assert!(u.host_dynamic && u.host.is_none(), "{first}");
            assert_eq!(u.path, "/x", "{first}");
        }
        // A bracketed IPv6 literal followed directly by a hole keeps its host (evaluator pass
        // 2, Q5, M02).
        let u = client_url(&[lit("http://[::1]"), Piece::Hole, lit("/x")]);
        assert_eq!(
            (
                u.host.as_deref(),
                u.host_dynamic,
                u.host_external.as_deref()
            ),
            (Some("[::1]"), false, None)
        );
    }

    /// Contract M6b S7: `scheme://user…hole…@host` matches within the
    /// literal host — the userinfo (with its holes) is dropped, never
    /// stored, and never poisons the host. Certainty is what the rest earns:
    /// no flag is set by the userinfo. A hole in the host itself, and a hole
    /// with no `@` after it, keep today's rules.
    #[test]
    fn a_hole_in_userinfo_keeps_the_literal_host() {
        let read = |pieces: &[Piece]| {
            let u = client_url(pieces);
            (
                u.host,
                u.port,
                u.host_dynamic,
                u.host_external,
                u.base_unknown,
                u.path,
                u.flags.partial,
            )
        };
        let kept = |host: &str, path: &str| {
            (
                Some(host.to_owned()),
                None,
                false,
                None,
                false,
                path.to_owned(),
                false,
            )
        };
        // The contract's row, plus user text with a colon.
        for pieces in [
            vec![lit("http://u:"), Piece::Hole, lit("@users/x")],
            vec![lit("http://user:secret:"), Piece::Hole, lit("@users/x")],
        ] {
            assert_eq!(read(&pieces), kept("users", "/x"), "{pieces:?}");
        }
        // A port after the host still parses, and a second hole after it is
        // a port hole as before.
        assert_eq!(
            read(&[lit("http://u:"), Piece::Hole, lit("@users:8080/x")]),
            (
                Some("users".to_owned()),
                Some("8080".to_owned()),
                false,
                None,
                false,
                "/x".to_owned(),
                false,
            )
        );
        assert_eq!(
            read(&[
                lit("http://u:"),
                Piece::Hole,
                lit("@users:"),
                Piece::Hole,
                lit("/x")
            ]),
            (
                Some("users".to_owned()),
                None,
                false,
                None,
                false,
                "/x".to_owned(),
                false,
            )
        );
        // A hole for the host itself: unknown, as before.
        assert_eq!(
            read(&[
                lit("http://u:"),
                Piece::Hole,
                lit("@"),
                Piece::Hole,
                lit("/x")
            ]),
            (None, None, true, None, true, "/x".to_owned(), false)
        );
        // Multiple `@`: the last one wins, as with literal authorities.
        assert_eq!(
            read(&[lit("http://a@u:"), Piece::Hole, lit("@users/x")]),
            kept("users", "/x")
        );
        // No `@` after the hole: today's rules (a port hole here).
        assert_eq!(
            read(&[lit("http://users:"), Piece::Hole, lit("/x")]),
            (
                Some("users".to_owned()),
                None,
                false,
                None,
                false,
                "/x".to_owned(),
                false,
            )
        );
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q2): a hole next to literal host text. When
    /// a literal part of the host holds a `.` (an FQDN fragment: `.zendesk.com`, `api.`,
    /// `api.stripe.com`) the host names no service and the site is external; otherwise the host
    /// is unknown and the base is unknown, so the path is matched by suffix only. Never an exact
    /// match across services.
    #[test]
    fn a_hole_next_to_literal_host_text() {
        let read = |pieces: &[Piece]| {
            let u = client_url(pieces);
            (u.host_external, u.base_unknown, u.host, u.path)
        };
        let external = |h: &str, p: &str| (Some(h.to_owned()), false, None, p.to_owned());
        let unknown = |p: &str| (None, true, None, p.to_owned());
        for (pieces, want) in [
            // `https://${tenant}.zendesk.com/api/v2/tickets`, Java `String.format` and Go
            // `Sprintf` give the same pieces.
            (
                vec![
                    lit("https://"),
                    Piece::Hole,
                    lit(".zendesk.com/api/v2/tickets"),
                ],
                external("{?}.zendesk.com", "/api/v2/tickets"),
            ),
            // `https://api.stripe.com${v}/charges`, Go `"https://api.stripe.com" + v + …`.
            (
                vec![lit("https://api.stripe.com"), Piece::Hole, lit("/charges")],
                external("api.stripe.com{?}", "/charges"),
            ),
            (
                vec![lit("http://api."), Piece::Hole, lit("/orders/1")],
                external("api.{?}", "/orders/1"),
            ),
            (
                vec![lit("http://10.0."), Piece::Hole, lit(".5:8080/x")],
                external("10.0.{?}.5", "/x"),
            ),
            // A hole in user information with a dotted host after `@`: the
            // host is literal (contract M6b S7) — the stitcher's host rule,
            // not the hole rule, decides it names no service.
            (
                vec![lit("http://admin:"), Piece::Hole, lit("@api.stripe.com/x")],
                (
                    None,
                    false,
                    Some("api.stripe.com".to_owned()),
                    "/x".to_owned(),
                ),
            ),
            // No dot in the literal host text: unknown.
            (
                vec![lit("http://"), Piece::Hole, lit("/orders/1")],
                unknown("/orders/1"),
            ),
            (
                vec![lit("http://"), Piece::Hole, lit(":8080/x/y")],
                unknown("/x/y"),
            ),
            (
                vec![lit("http://localhost"), Piece::Hole, lit("/x")],
                unknown("/x"),
            ),
            (
                vec![lit("http://orders-"), Piece::Hole, lit("/x")],
                unknown("/x"),
            ),
            // A hole in user information with a port after `@`: the literal
            // host and port are kept (contract M6b S7).
            (
                vec![lit("http://admin:"), Piece::Hole, lit("@users:3000/x")],
                (None, false, Some("users".to_owned()), "/x".to_owned()),
            ),
        ] {
            assert_eq!(read(&pieces), want, "{pieces:?}");
        }
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q6): a hole glued to a complete authority
    /// (`http://orders:3000${p}/orders/1`) keeps the host; the path after the hole is matched by
    /// suffix (the hole may continue the port or hold a path prefix).
    #[test]
    fn a_hole_after_a_literal_port_is_an_unknown_prefix() {
        let u = client_url(&[lit("http://orders:3000"), Piece::Hole, lit("/orders/1")]);
        assert_eq!(
            (
                u.host.as_deref(),
                u.port.as_deref(),
                u.base_unknown,
                u.path.as_str()
            ),
            (Some("orders"), Some("3000"), true, "/orders/1")
        );
        // A hole after a `/` is a path segment as before.
        let u = client_url(&[lit("http://orders:3000/"), Piece::Hole, lit("/x")]);
        assert_eq!((u.base_unknown, u.path.as_str()), (false, "/{}/x"));
    }

    /// Contract M4a amendment 13, Gate B (evaluator pass 3, R4): a port hole is one hole;
    /// another hole before the path's first `/` may hold a path prefix, so the path does not
    /// start with a literal and is matched by suffix within the host.
    #[test]
    fn holes_after_a_port_hole_are_an_unknown_prefix() {
        let u = client_url(&[lit("http://users:"), Piece::Hole, Piece::Hole, lit("/x")]);
        assert_eq!(
            (u.host.as_deref(), u.base_unknown, u.path.as_str()),
            (Some("users"), true, "/x")
        );
        let u = client_url(&[
            lit("http://users:"),
            Piece::Hole,
            Piece::Hole,
            Piece::Hole,
            lit("/a/b"),
        ]);
        assert_eq!((u.base_unknown, u.path.as_str()), (true, "/a/b"));
        // One port hole: the path starts with its literal `/`, as before.
        let u = client_url(&[lit("http://users:"), Piece::Hole, lit("/x")]);
        assert_eq!(
            (u.host.as_deref(), u.base_unknown, u.path.as_str()),
            (Some("users"), false, "/x")
        );
    }

    /// Evaluator pass 1, P3: a hole that ends the URL glued to literal text is an opaque tail
    /// (a query, a fragment, an extension): dropped from the path and flagged. Glued inside a
    /// segment elsewhere it makes that segment `{}`, flagged `partial`; after a `/` it is a
    /// whole-segment parameter as before.
    #[test]
    fn a_glued_trailing_hole_is_an_opaque_tail() {
        let u = client_url(&[lit("/api/items"), Piece::Hole]);
        assert_eq!(u.path, "/api/items");
        assert!(u.flags.opaque_tail && !u.flags.partial);
        let u = client_url(&[lit("http://h/api/items?x="), Piece::Hole]);
        assert_eq!(u.path, "/api/items");
        assert!(
            !u.flags.opaque_tail,
            "a query in literal text is dropped as before"
        );
        let u = client_url(&[lit("/f/file."), Piece::Hole, Piece::Hole]);
        assert_eq!(u.path, "/f/file.");
        assert!(u.flags.opaque_tail);
        let u = client_url(&[lit("/api/item"), Piece::Hole, lit("/y")]);
        assert_eq!(u.path, "/api/{}/y");
        assert!(u.flags.partial && !u.flags.opaque_tail);
        let u = client_url(&[lit("/api/items/"), Piece::Hole]);
        assert_eq!(u.path, "/api/items/{}");
        assert!(!u.flags.partial && !u.flags.opaque_tail);
        let u = client_url(&[lit("/a/b"), Piece::Hole, lit("?q=1")]);
        assert_eq!(u.path, "/a/{}");
        assert!(u.flags.partial && !u.flags.opaque_tail);
        // An unknown base and an opaque tail: `${base}/x${qs}`.
        let u = client_url(&[Piece::Hole, lit("/x"), Piece::Hole]);
        assert_eq!((u.base_unknown, u.path.as_str()), (true, "/x"));
        assert!(u.flags.opaque_tail);
        // A template that is only a hole after a literal host has no opaque tail to drop.
        let u = client_url(&[lit("http://users/"), Piece::Hole]);
        assert_eq!(u.path, "/{}");
        assert!(!u.flags.opaque_tail);
    }

    /// Evaluator pass 1, P2: a suffix match needs a literal segment in the endpoint tail it
    /// matched; a tail of parameters and catch-alls is no evidence.
    #[test]
    fn a_suffix_match_needs_a_literal_in_the_matched_tail() {
        for endpoint in ["/{name}", "/{*}", "/accounts/{name}/{*}", "/a/{x}/{y}"] {
            assert_eq!(
                match_path(&segs("/latest"), true, &segs(endpoint)),
                None,
                "{endpoint}"
            );
        }
        let m = Some(PathMatch {
            suffix: true,
            wildcard: false,
        });
        assert_eq!(match_path(&segs("/latest"), true, &segs("/api/latest")), m);
        assert_eq!(
            match_path(&segs("/rates/latest"), true, &segs("/rates/{x}")),
            m
        );
        // A catch-all endpoint matches by suffix only through a literal before it.
        assert_eq!(
            match_path(&segs("/api/x/y"), true, &segs("/v1/api/{*}")),
            Some(PathMatch {
                suffix: true,
                wildcard: true,
            })
        );
        assert_eq!(match_path(&segs("/x/y"), true, &segs("/v1/api/{*}")), None);
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
        assert_eq!(
            prefix_unknown_key("GET", "/", "svc", "users/routers.go#UsersRegister.router"),
            "HTTP GET / @svc ?router=users/routers.go#UsersRegister.router"
        );
        // Nor does ` ?`: a query string is dropped.
        assert!(!norm_path("/a ?router=x").contains(" ?"));
    }
}
