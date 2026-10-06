//! HTTP stitching (design §10.2, contract M4a S12–S14): every client call site's URL is folded
//! (across files through M3a resolution and the owning service's `application.properties`),
//! matched to the endpoints of other services, and linked by `Symbol | File -CONSUMES-> Endpoint`
//! with provenance `http_stitch`, never `exact`. Every site lands in exactly one coverage bucket.
//!
//! Candidates come from an index bucketed by an endpoint's first segment (exact matches) and last
//! segment (suffix matches), and each is confirmed by the reference matcher
//! ([`rb_frameworks::endpoint::match_path`]), so the index decides nothing D9 does not.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rb_frameworks::clients::{ClientKind, FileClients, UrlValue};
use rb_frameworks::endpoint::{
    ANY, PathMatch, Piece, Segment, client_url, literal_segments, match_path, methods_match,
    segments,
};
use rb_frameworks::fold::{self, Folded, Lookup, Origin};
use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance, Span,
};
use rb_syntax::{FileFacts, Language};
use serde_json::{Value, json};

use crate::resolve::Index;
use crate::routes::EndpointRef;

/// Candidates above which a site gets no edge (contract M4a invariant 3).
pub(crate) const MAX_CANDIDATES: usize = 16;

/// Hosts that mean the caller's own host (D6).
const LOOPBACK: &[&str] = &["localhost", "127.0.0.1", "0.0.0.0", "[::1]"];

/// Where a client site lands (contract M4a S14); every site is in exactly one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Bucket {
    Synthetic,
    Heuristic,
    Ambiguous,
    TooAmbiguous,
    Unmatched,
    /// Candidates only in the caller's own service, a loopback host, or a host naming it.
    OwnService,
    External,
    Dynamic,
}

impl Bucket {
    pub(crate) const ALL: [Bucket; 8] = [
        Bucket::Synthetic,
        Bucket::Heuristic,
        Bucket::Ambiguous,
        Bucket::TooAmbiguous,
        Bucket::Unmatched,
        Bucket::OwnService,
        Bucket::External,
        Bucket::Dynamic,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Bucket::Synthetic => "synthetic",
            Bucket::Heuristic => "heuristic",
            Bucket::Ambiguous => "ambiguous",
            Bucket::TooAmbiguous => "too_ambiguous",
            Bucket::Unmatched => "unmatched",
            Bucket::OwnService => "self",
            Bucket::External => "external",
            Bucket::Dynamic => "dynamic",
        }
    }

    /// The diagnostic of a site that is not stitched.
    fn diagnostic(self) -> Option<&'static str> {
        match self {
            Bucket::TooAmbiguous => Some("http_too_ambiguous"),
            Bucket::Unmatched => Some("http_unmatched"),
            Bucket::OwnService => Some("http_self"),
            Bucket::External => Some("http_external"),
            Bucket::Dynamic => Some("http_url_dynamic"),
            Bucket::Synthetic | Bucket::Heuristic | Bucket::Ambiguous => None,
        }
    }
}

/// One client call site, its URL folded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Site {
    /// The definition making the call, or the `File` for top-level code.
    pub src: NodeKey,
    pub at: Evidence,
    /// The key of the caller's service.
    pub service: String,
    pub client: ClientKind,
    /// Upper case; `None` when not known statically.
    pub method: Option<String>,
    /// `None` when the URL is not a string or a name.
    pub url: Option<Folded>,
}

/// A service as the host rule (D6) sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceRef {
    pub key: String,
    pub name: String,
}

/// One edge a site gives.
#[derive(Debug, Clone, PartialEq)]
struct Link {
    /// Index into the endpoints.
    endpoint: usize,
    certainty: Certainty,
    attrs: Attrs,
}

/// A site's bucket and edges.
#[derive(Debug, Clone, PartialEq)]
struct Outcome {
    bucket: Bucket,
    links: Vec<Link>,
    /// Candidates of a `too_ambiguous` site.
    candidates: usize,
}

impl Outcome {
    fn bucket(bucket: Bucket) -> Self {
        Outcome {
            bucket,
            links: Vec::new(),
            candidates: 0,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Sites: client facts with their URLs folded (S9–S12)

/// What site collection reads.
pub(crate) struct Inputs<'a, 'i> {
    pub files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
    pub index: &'i Index<'a>,
    pub keys: &'a BTreeMap<&'a str, Vec<String>>,
    /// The key of the service owning a path.
    pub service_of: &'i dyn Fn(&str) -> String,
    /// Service key → its `application.properties`.
    pub properties: &'i BTreeMap<String, BTreeMap<String, String>>,
}

/// One file's top-level non-member bindings (`None`) and type members (`Some(type)`) by name.
type Declared<'a> = HashMap<(Option<usize>, &'a str), Vec<usize>>;

/// The indexer's [`Lookup`]: files by number, constants across files through M3a resolution
/// (only a unique declaration folds), properties of the owning service.
struct Folding<'a, 'i> {
    inputs: &'i Inputs<'a, 'i>,
    paths: Vec<&'a str>,
    ids: HashMap<&'a str, usize>,
    /// Per file.
    declared: Vec<Declared<'a>>,
}

impl<'a, 'i> Folding<'a, 'i> {
    fn new(inputs: &'i Inputs<'a, 'i>) -> Self {
        let paths: Vec<&'a str> = inputs.files.keys().copied().collect();
        let ids = paths.iter().enumerate().map(|(i, p)| (*p, i)).collect();
        let declared = inputs
            .files
            .values()
            .map(|(_, facts)| {
                let mut table: Declared<'a> = HashMap::new();
                for (i, b) in facts.bindings.iter().enumerate() {
                    if b.member == b.scope.is_some() {
                        table.entry((b.scope, b.name.as_str())).or_default().push(i);
                    }
                }
                table
            })
            .collect();
        Folding {
            inputs,
            paths,
            ids,
            declared,
        }
    }

    fn value(&self, file: usize, v: &UrlValue) -> Option<Folded> {
        match v {
            UrlValue::Template(t) => Some(fold::fold_template(self, file, t)),
            UrlValue::Name { text, span } => Some(fold::fold_name(self, file, text, *span)),
            UrlValue::Unknown => None,
        }
    }
}

impl Lookup for Folding<'_, '_> {
    fn facts(&self, file: usize) -> &FileFacts {
        self.inputs.files[self.paths[file]].1
    }

    fn language(&self, file: usize) -> Language {
        self.inputs.files[self.paths[file]].0
    }

    fn external(&self, file: usize, name: &str, at: Span) -> Option<(usize, usize)> {
        let found: BTreeSet<(usize, usize)> = self
            .inputs
            .index
            .constant_declarations(self.paths[file], name, at)
            .into_iter()
            .flat_map(|(path, owner, declared)| {
                let id = self.ids[path];
                self.declared[id]
                    .get(&(owner, declared.as_str()))
                    .into_iter()
                    .flatten()
                    .map(move |&b| (id, b))
                    .collect::<Vec<_>>()
            })
            .collect();
        // Several declarations: the constant is not known, so nothing folds.
        match found.len() {
            1 => found.into_iter().next(),
            _ => None,
        }
    }

    fn property(&self, file: usize, key: &str) -> Option<String> {
        let service = (self.inputs.service_of)(self.paths[file]);
        self.inputs.properties.get(&service)?.get(key).cloned()
    }
}

/// Every client call site of the snapshot, in path then source order, its URL folded. An axios
/// call on an imported name counts only when the import resolves to a file binding an axios
/// instance of that name at top level; it then takes that instance's base.
pub(crate) fn sites(inputs: &Inputs<'_, '_>) -> Vec<Site> {
    let folding = Folding::new(inputs);
    let clients: BTreeMap<&str, FileClients> = inputs
        .files
        .iter()
        .map(|(path, (language, facts))| (*path, rb_frameworks::clients::clients(*language, facts)))
        .collect();
    let mut out = Vec::new();
    for (&path, file_clients) in &clients {
        let file = folding.ids[path];
        let facts = inputs.files[path].1;
        let service = (inputs.service_of)(path);
        for call in &file_clients.calls {
            let base = match &call.instance {
                Some(name) => match imported_instance(inputs, &clients, path, facts, name) {
                    Some((target, base)) => base.map(|b| (folding.ids[target], b)),
                    None => continue,
                },
                None => call.base.clone().map(|b| (file, b)),
            };
            let url = folding.value(file, &call.url).map(|url| match &base {
                Some((base_file, b)) => join(folding.value(*base_file, b), url),
                None => url,
            });
            let src = match call.source {
                Some(i) => NodeKey::new(NodeKind::Symbol, inputs.keys[path][i].clone()),
                None => NodeKey::new(NodeKind::File, path),
            };
            out.push(Site {
                src,
                at: Evidence {
                    path: path.to_owned(),
                    span: call.span,
                },
                service: service.clone(),
                client: call.kind,
                method: call.method.clone(),
                url,
            });
        }
    }
    out
}

/// The axios instance an imported name is, with its file: the first resolved target (in path
/// order) binding an instance of the exported name at top level.
fn imported_instance<'a>(
    inputs: &Inputs<'a, '_>,
    clients: &BTreeMap<&'a str, FileClients>,
    path: &str,
    facts: &FileFacts,
    local: &str,
) -> Option<(&'a str, Option<UrlValue>)> {
    let targets = inputs.index.import_targets(path);
    facts
        .imports
        .iter()
        .zip(targets)
        .flat_map(|(import, files)| {
            import
                .names
                .iter()
                .filter(|n| n.local == local && n.name != "default")
                .flat_map(move |n| files.iter().map(move |f| (*f, n.name.as_str())))
        })
        .find_map(|(target, exported)| {
            clients
                .get(target)?
                .axios_instances
                .iter()
                .find(|a| a.top_level && a.name == exported)
                .map(|a| (target, a.base.clone()))
        })
}

/// A URL relative to a base: the base then the URL (with a `/` between unless one side has it),
/// unless the URL is absolute. An unknown base is a hole. The label is the weakest among the
/// parts that resolved.
fn join(base: Option<Folded>, url: Folded) -> Folded {
    let absolute = {
        let u = client_url(&url.pieces);
        u.host.is_some() || u.host_dynamic
    };
    if absolute {
        return url;
    }
    let base = base.unwrap_or(Folded {
        pieces: vec![Piece::Hole],
        origin: Origin::Unresolved,
    });
    let origin = match (base.origin, url.origin) {
        (Origin::Unresolved, o) | (o, Origin::Unresolved) => o,
        (a, b) => a.min(b),
    };
    let ends = matches!(base.pieces.last(), Some(Piece::Lit(l)) if l.ends_with('/'));
    let starts = matches!(url.pieces.first(), Some(Piece::Lit(l)) if l.starts_with('/'));
    let mut pieces = base.pieces;
    if !ends && !starts {
        pieces.push(Piece::Lit("/".to_owned()));
    }
    pieces.extend(url.pieces);
    Folded { pieces, origin }
}

// ---------------------------------------------------------------------------------------------
// Candidates (D9)

/// Endpoints bucketed by first segment (exact matches) and last segment (suffix matches). A
/// client's candidates are the union of the buckets that can match it, each confirmed by
/// [`match_path`].
struct EndpointIndex {
    segments: Vec<Vec<Segment>>,
    /// No segment (`/`).
    root: Vec<usize>,
    first: HashMap<String, Vec<usize>>,
    first_param: Vec<usize>,
    first_wild: Vec<usize>,
    last: HashMap<String, Vec<usize>>,
    last_param: Vec<usize>,
    last_wild: Vec<usize>,
}

impl EndpointIndex {
    fn new(endpoints: &[EndpointRef]) -> Self {
        let mut ix = EndpointIndex {
            segments: endpoints.iter().map(|e| segments(&e.path)).collect(),
            root: Vec::new(),
            first: HashMap::new(),
            first_param: Vec::new(),
            first_wild: Vec::new(),
            last: HashMap::new(),
            last_param: Vec::new(),
            last_wild: Vec::new(),
        };
        for (i, segs) in ix.segments.iter().enumerate() {
            match segs.first() {
                None => ix.root.push(i),
                Some(Segment::Lit(l)) => ix.first.entry(l.clone()).or_default().push(i),
                Some(Segment::Param) => ix.first_param.push(i),
                Some(Segment::Wild) => ix.first_wild.push(i),
            }
            match segs.last() {
                None => {}
                Some(Segment::Lit(l)) => ix.last.entry(l.clone()).or_default().push(i),
                Some(Segment::Param) => ix.last_param.push(i),
                Some(Segment::Wild) => ix.last_wild.push(i),
            }
        }
        ix
    }

    /// The endpoints `client` matches, with how, in endpoint order.
    fn candidates(&self, client: &[Segment], suffix: bool) -> Vec<(usize, PathMatch)> {
        let lists: [&[usize]; 3] = if suffix {
            let by_last = match client.last() {
                Some(Segment::Lit(l)) => self.last.get(l).map_or(&[][..], Vec::as_slice),
                _ => &[],
            };
            [by_last, &self.last_param, &self.last_wild]
        } else {
            match client.first() {
                None => [&self.root, &self.first_wild, &[]],
                Some(Segment::Lit(l)) => [
                    self.first.get(l).map_or(&[][..], Vec::as_slice),
                    &self.first_param,
                    &self.first_wild,
                ],
                Some(_) => [&self.first_param, &self.first_wild, &[]],
            }
        };
        let mut out: Vec<(usize, PathMatch)> = lists
            .into_iter()
            .flatten()
            .filter_map(|&i| match_path(client, suffix, &self.segments[i]).map(|m| (i, m)))
            .collect();
        out.sort_unstable_by_key(|(i, _)| *i);
        out
    }
}

/// How the host restricts candidates (D6).
enum Scope<'s> {
    /// No host, a host with a hole, or one naming several services: every other service.
    Any,
    /// A host naming one service that exposes endpoints: only that service.
    Only(&'s str),
    /// A host naming one service that exposes none (a gateway): every other service, through
    /// it.
    Proxy,
}

/// The stitching context of a snapshot: endpoints, their index, and the services hosts name.
struct Stitcher<'e> {
    endpoints: &'e [EndpointRef],
    index: EndpointIndex,
    /// Host form (a service's name, lower-cased, or the last segment of its key) → service keys.
    hosts: HashMap<String, BTreeSet<&'e str>>,
    exposing: HashSet<&'e str>,
}

impl<'e> Stitcher<'e> {
    fn new(endpoints: &'e [EndpointRef], services: &'e [ServiceRef]) -> Self {
        let mut hosts: HashMap<String, BTreeSet<&str>> = HashMap::new();
        for s in services {
            hosts
                .entry(s.name.to_ascii_lowercase())
                .or_default()
                .insert(&s.key);
            let last = s.key.rsplit('/').next().unwrap_or(&s.key);
            hosts
                .entry(last.to_ascii_lowercase())
                .or_default()
                .insert(&s.key);
        }
        Stitcher {
            endpoints,
            index: EndpointIndex::new(endpoints),
            hosts,
            exposing: endpoints.iter().map(|e| e.service.as_str()).collect(),
        }
    }

    /// The bucket and edges of one site (S13, S14).
    fn site(&self, site: &Site) -> Outcome {
        let Some(folded) = &site.url else {
            return Outcome::bucket(Bucket::Dynamic);
        };
        let url = client_url(&folded.pieces);
        let mut scope = Scope::Any;
        if let Some(host) = &url.host {
            if LOOPBACK.contains(&host.as_str()) {
                return Outcome::bucket(Bucket::OwnService);
            }
            let Some(named) = self.hosts.get(host) else {
                return Outcome::bucket(Bucket::External);
            };
            // A host naming several services restricts nothing.
            if let [service] = named.iter().copied().collect::<Vec<_>>().as_slice() {
                if *service == site.service {
                    return Outcome::bucket(Bucket::OwnService);
                }
                scope = if self.exposing.contains(service) {
                    Scope::Only(service)
                } else {
                    Scope::Proxy
                };
            }
        }
        let client = segments(&url.path);
        let has_hole = folded.pieces.contains(&Piece::Hole);
        if has_hole && !client.iter().any(|s| matches!(s, Segment::Lit(_))) {
            return Outcome::bucket(Bucket::Dynamic);
        }
        let method = site.method.as_deref().unwrap_or(ANY);
        let found: Vec<(usize, PathMatch)> = self
            .index
            .candidates(&client, url.base_unknown)
            .into_iter()
            .filter(|(i, _)| methods_match(method, &self.endpoints[*i].method))
            .collect();
        let (own, others): (Vec<_>, Vec<_>) = found
            .into_iter()
            .partition(|(i, _)| self.endpoints[*i].service == site.service);
        let others: Vec<(usize, PathMatch)> = match scope {
            Scope::Only(s) => others
                .into_iter()
                .filter(|(i, _)| self.endpoints[*i].service == s)
                .collect(),
            Scope::Any | Scope::Proxy => others,
        };
        if others.is_empty() {
            let only_own = !own.is_empty() && !matches!(scope, Scope::Only(_));
            return Outcome::bucket(if only_own {
                Bucket::OwnService
            } else {
                Bucket::Unmatched
            });
        }
        let best = self.most_specific(others, method, client.len());
        let n = best.len();
        if n > MAX_CANDIDATES {
            return Outcome {
                candidates: n,
                ..Outcome::bucket(Bucket::TooAmbiguous)
            };
        }
        let mut attrs = Attrs::new();
        let (i, m) = best[0];
        attrs.insert(
            "match".into(),
            if m.suffix { "suffix" } else { "exact" }.into(),
        );
        attrs.insert("client".into(), site.client.as_str().into());
        attrs.insert("base".into(), folded.origin.as_str().into());
        if let Some(host) = &url.host {
            let key = if matches!(scope, Scope::Proxy) {
                "via_host"
            } else {
                "host"
            };
            attrs.insert(key.into(), host.as_str().into());
        }
        if n > 1 {
            attrs.insert("candidates".into(), n.into());
            return Outcome {
                bucket: Bucket::Ambiguous,
                links: best
                    .iter()
                    .map(|&(i, _)| Link {
                        endpoint: i,
                        certainty: Certainty::Ambiguous,
                        attrs: attrs.clone(),
                    })
                    .collect(),
                candidates: n,
            };
        }
        let e = &self.endpoints[i];
        // S13's `synthetic` row (with F1); anything less is `heuristic`. The endpoint's own path
        // certainty caps both (invariant 4).
        let synthetic = !m.suffix
            && !m.wildcard
            && site.method.is_some()
            && matches!(scope, Scope::Only(_))
            && folded.origin >= Origin::Config
            && !url.flags.partial
            && !url.flags.wildcard_not_last
            && !e.prefix_unknown;
        let certainty = if synthetic {
            Certainty::Synthetic
        } else {
            Certainty::Heuristic
        }
        .min(e.certainty);
        Outcome {
            bucket: if certainty == Certainty::Synthetic {
                Bucket::Synthetic
            } else {
                Bucket::Heuristic
            },
            links: vec![Link {
                endpoint: i,
                certainty,
                attrs,
            }],
            candidates: 1,
        }
    }

    /// D9: the candidates with the most literal segments, then those whose method equals a known
    /// client method over `ANY` ones. A suffix match counts the literals of the endpoint tail the
    /// client's `client_len` segments matched (amendment 9); a tail ending in `{*}` has no fixed
    /// length, so its whole path counts.
    fn most_specific(
        &self,
        found: Vec<(usize, PathMatch)>,
        method: &str,
        client_len: usize,
    ) -> Vec<(usize, PathMatch)> {
        let literals = |i: usize, m: &PathMatch| {
            let segs = &self.index.segments[i];
            if m.suffix && segs.last() != Some(&Segment::Wild) {
                literal_segments(&segs[segs.len().saturating_sub(client_len)..])
            } else {
                literal_segments(segs)
            }
        };
        let top = found
            .iter()
            .map(|(i, m)| literals(*i, m))
            .max()
            .unwrap_or(0);
        let mut best: Vec<(usize, PathMatch)> = found
            .into_iter()
            .filter(|(i, m)| literals(*i, m) == top)
            .collect();
        if method != ANY
            && best
                .iter()
                .any(|(i, _)| self.endpoints[*i].method == method)
        {
            best.retain(|(i, _)| self.endpoints[*i].method == method);
        }
        best
    }
}

/// The text of a folded URL, `{?}` for each hole.
fn render(f: &Folded) -> String {
    f.pieces
        .iter()
        .map(|p| match p {
            Piece::Lit(l) => l.as_str(),
            Piece::Hole => "{?}",
        })
        .collect()
}

/// Stitches every site (S13): adds the `CONSUMES` edges, merged per `(source, endpoint)` with the
/// best site's certainty and attributes plus `sites = n` (invariant 4); returns one diagnostic
/// per site that is not stitched and an `http_coverage` count of the buckets.
pub(crate) fn add_consumes(
    b: &mut GraphBuilder,
    sites: &[Site],
    endpoints: &[EndpointRef],
    services: &[ServiceRef],
) -> anyhow::Result<Vec<Diagnostic>> {
    let stitcher = Stitcher::new(endpoints, services);
    struct Merged {
        certainty: Certainty,
        attrs: Attrs,
        best_at: Evidence,
        sites: usize,
        evidence: BTreeSet<Evidence>,
    }
    let mut merged: BTreeMap<(NodeKey, usize), Merged> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let mut counts: BTreeMap<Bucket, usize> = Bucket::ALL.iter().map(|&k| (k, 0)).collect();
    for site in sites {
        let outcome = stitcher.site(site);
        *counts.entry(outcome.bucket).or_default() += 1;
        if let Some(kind) = outcome.bucket.diagnostic() {
            let mut detail = json!({ "client": site.client.as_str() });
            if let Some(url) = &site.url {
                detail["url"] = render(url).into();
                if let Some(host) = client_url(&url.pieces).host {
                    detail["host"] = host.into();
                }
            }
            if outcome.bucket == Bucket::TooAmbiguous {
                detail["candidates"] = outcome.candidates.into();
            }
            diagnostics.push(Diagnostic {
                kind: kind.to_owned(),
                path: Some(site.at.path.clone()),
                span: Some(site.at.span),
                detail,
            });
        }
        for link in outcome.links {
            let m = merged
                .entry((site.src.clone(), link.endpoint))
                .or_insert_with(|| Merged {
                    certainty: link.certainty,
                    attrs: link.attrs.clone(),
                    best_at: site.at.clone(),
                    sites: 0,
                    evidence: BTreeSet::new(),
                });
            if (link.certainty, std::cmp::Reverse(&site.at))
                > (m.certainty, std::cmp::Reverse(&m.best_at))
            {
                m.certainty = link.certainty;
                m.attrs = link.attrs;
                m.best_at = site.at.clone();
            }
            m.sites += 1;
            m.evidence.insert(site.at.clone());
        }
    }
    for ((src, endpoint), m) in merged {
        let mut attrs = m.attrs;
        attrs.insert("sites".into(), m.sites.into());
        let mut edge = NewEdge::new(
            src,
            EdgeKind::Consumes,
            NodeKey::new(NodeKind::Endpoint, endpoints[endpoint].key.clone()),
            Provenance::HttpStitch,
            m.certainty,
        )
        .attrs(attrs);
        edge.evidence = m.evidence.into_iter().collect();
        b.add_edge(edge)?;
    }
    if !sites.is_empty() {
        let mut detail = serde_json::Map::new();
        detail.insert("sites".into(), sites.len().into());
        for (bucket, n) in counts {
            detail.insert(bucket.as_str().into(), Value::from(n));
        }
        diagnostics.push(Diagnostic {
            kind: "http_coverage".into(),
            path: None,
            span: None,
            detail: Value::Object(detail),
        });
    }
    Ok(diagnostics)
}

#[cfg(test)]
mod tests {
    use rb_frameworks::endpoint::{endpoint_key, norm_path};
    use rb_model::Graph;

    use super::*;

    const SERVICES: &[(&str, &str)] = &[
        ("apps/web", "@acme/web"),
        ("gateway", "edge-gateway"),
        ("services/auth", "auth-service"),
        ("services/billing/cmd/billing", "billing"),
        ("services/notification", "notification"),
    ];

    fn services() -> Vec<ServiceRef> {
        SERVICES
            .iter()
            .map(|(key, name)| ServiceRef {
                key: (*key).to_owned(),
                name: (*name).to_owned(),
            })
            .collect()
    }

    /// An endpoint `METHOD path @service`, exact and with a known prefix unless told otherwise.
    fn ep(method: &str, path: &str, service: &str) -> EndpointRef {
        let path = norm_path(path);
        EndpointRef {
            key: endpoint_key(method, &path, service),
            method: method.to_owned(),
            path,
            service: service.to_owned(),
            prefix_unknown: false,
            certainty: Certainty::Exact,
        }
    }

    /// The login-path endpoints of the fixture plan.
    fn endpoints() -> Vec<EndpointRef> {
        vec![
            ep("POST", "/api/v1/login", "services/auth"),
            ep("GET", "/api/v1/login", "services/auth"),
            ep("POST", "/internal/mfa/email", "services/notification"),
            ep("POST", "/internal/mfa/sms", "services/notification"),
            ep(
                "POST",
                "/internal/mfa/email",
                "services/billing/cmd/billing",
            ),
            ep(
                "POST",
                "/billing/invoices/{id}/pay",
                "services/billing/cmd/billing",
            ),
        ]
    }

    /// A folded URL from text, `{?}` for a hole.
    fn folded(text: &str, origin: Origin) -> Folded {
        let mut pieces = Vec::new();
        for (i, part) in text.split("{?}").enumerate() {
            if i > 0 {
                pieces.push(Piece::Hole);
            }
            if !part.is_empty() {
                pieces.push(Piece::Lit(part.to_owned()));
            }
        }
        Folded { pieces, origin }
    }

    /// A site in `service` (its source named after the line) calling `url` with `method`.
    fn site(service: &str, method: Option<&str>, url: Option<(&str, Origin)>, line: i32) -> Site {
        Site {
            src: NodeKey::new(NodeKind::Symbol, format!("{service}#f{line}")),
            at: Evidence {
                path: format!("{service}/f.ts"),
                span: [line, 0, line, 9],
            },
            service: service.to_owned(),
            client: ClientKind::Fetch,
            method: method.map(str::to_owned),
            url: url.map(|(t, o)| folded(t, o)),
        }
    }

    /// `bucket` then `endpoint key [certainty] attrs` per link.
    fn one(endpoints: &[EndpointRef], s: &Site) -> (Bucket, Vec<String>) {
        let services = services();
        let stitcher = Stitcher::new(endpoints, &services);
        let o = stitcher.site(s);
        let links = o
            .links
            .iter()
            .map(|l| {
                format!(
                    "{} [{}] {}",
                    endpoints[l.endpoint].key,
                    l.certainty.as_str(),
                    Value::Object(l.attrs.clone())
                )
            })
            .collect();
        (o.bucket, links)
    }

    /// The pathological input's bound: an order of magnitude above a measured local run.
    const BOUND_SECS: u64 = 20;

    const WEB: &str = "apps/web";
    const AUTH: &str = "services/auth";
    const NOTIFICATION: &str = "services/notification";
    const BILLING: &str = "services/billing/cmd/billing";
    use Origin::{Config, Constant, EnvDefault, Literal};

    /// S13 `synthetic` row: exact path, known method, a literal/constant/config host naming one
    /// service that exposes endpoints (F1), one candidate, no partial segment, no `{*}`, a known
    /// prefix. Each condition removed gives `heuristic`.
    #[test]
    fn the_synthetic_row_and_each_condition_that_lowers_it() {
        let eps = endpoints();
        let feign = site(
            AUTH,
            Some("POST"),
            Some(("http://notification:3000/internal/mfa/email", Literal)),
            1,
        );
        assert_eq!(
            one(&eps, &feign),
            (
                Bucket::Synthetic,
                vec![
                    "HTTP POST /internal/mfa/email @services/notification [synthetic] {\"base\":\"literal\",\"client\":\"fetch\",\"host\":\"notification\",\"match\":\"exact\"}".to_owned()
                ]
            )
        );
        // A constant and a config value are stated sources too.
        for origin in [Constant, Config] {
            let s = site(
                AUTH,
                Some("POST"),
                Some(("http://notification/internal/mfa/email", origin)),
                1,
            );
            assert_eq!(one(&eps, &s).0, Bucket::Synthetic, "{origin:?}");
        }
        let lowered = |s: Site, eps: &[EndpointRef]| {
            let (bucket, links) = one(eps, &s);
            assert_eq!(bucket, Bucket::Heuristic, "{s:?}");
            assert_eq!(links.len(), 1);
            links[0].clone()
        };
        // An env default is the development value.
        lowered(
            site(
                AUTH,
                Some("POST"),
                Some(("http://notification/internal/mfa/email", EnvDefault)),
                1,
            ),
            &eps,
        );
        // An unknown method.
        lowered(
            site(
                AUTH,
                None,
                Some(("http://notification/internal/mfa/email", Literal)),
                1,
            ),
            &eps,
        );
        // No host: the path alone.
        let web = lowered(
            site(WEB, Some("POST"), Some(("/api/v1/login", Literal)), 1),
            &eps,
        );
        assert!(
            web.contains("\"match\":\"exact\"") && !web.contains("host"),
            "{web}"
        );
        // A partial segment.
        let mut with_param = eps.clone();
        with_param.push(ep("GET", "/u/{id}", NOTIFICATION));
        lowered(
            site(
                AUTH,
                Some("GET"),
                Some(("http://notification/u/{?}.json", Literal)),
                1,
            ),
            &with_param,
        );
        // An endpoint `{*}`.
        let mut wild = eps.clone();
        wild.push(ep("GET", "/static/*", NOTIFICATION));
        lowered(
            site(
                AUTH,
                Some("GET"),
                Some(("http://notification/static/a/b.js", Literal)),
                1,
            ),
            &wild,
        );
        // An endpoint whose prefix is unknown.
        let mut unknown = eps.clone();
        unknown[2].prefix_unknown = true;
        lowered(
            site(
                AUTH,
                Some("POST"),
                Some(("http://notification/internal/mfa/email", Literal)),
                1,
            ),
            &unknown,
        );
        // An endpoint composed across files (its path is heuristic): invariant 4.
        let mut composed = eps.clone();
        composed[2].certainty = Certainty::Heuristic;
        lowered(
            site(
                AUTH,
                Some("POST"),
                Some(("http://notification/internal/mfa/email", Literal)),
                1,
            ),
            &composed,
        );
        // A context path is a stated value: synthetic stays synthetic.
        let mut context = eps.clone();
        context[2].certainty = Certainty::Synthetic;
        assert_eq!(
            one(
                &context,
                &site(
                    AUTH,
                    Some("POST"),
                    Some(("http://notification/internal/mfa/email", Literal)),
                    1
                )
            )
            .0,
            Bucket::Synthetic
        );
    }

    /// S13 `suffix` and D6's proxy: an unknown base matches the endpoint path's tail; a host
    /// naming a service without endpoints is a gateway, path-matched across services.
    #[test]
    fn suffix_matches_and_proxies_are_heuristic() {
        let eps = endpoints();
        let (bucket, links) = one(
            &eps,
            &site(
                WEB,
                Some("POST"),
                Some(("{?}/invoices/{?}/pay", Literal)),
                1,
            ),
        );
        assert_eq!(bucket, Bucket::Heuristic);
        assert_eq!(
            links,
            [
                "HTTP POST /billing/invoices/{}/pay @services/billing/cmd/billing [heuristic] {\"base\":\"literal\",\"client\":\"fetch\",\"match\":\"suffix\"}"
            ]
        );
        let (bucket, links) = one(
            &eps,
            &site(
                WEB,
                Some("POST"),
                Some(("http://gateway/api/v1/login", EnvDefault)),
                1,
            ),
        );
        assert_eq!(bucket, Bucket::Heuristic);
        assert_eq!(
            links,
            [
                "HTTP POST /api/v1/login @services/auth [heuristic] {\"base\":\"env_default\",\"client\":\"fetch\",\"match\":\"exact\",\"via_host\":\"gateway\"}"
            ]
        );
        // Through a proxy even a literal host stays heuristic.
        assert_eq!(
            one(
                &eps,
                &site(
                    WEB,
                    Some("POST"),
                    Some(("http://gateway/api/v1/login", Literal)),
                    1
                )
            )
            .0,
            Bucket::Heuristic
        );
        // A suffix needs a literal segment: `{?}/{?}` is dynamic.
        assert_eq!(
            one(
                &eps,
                &site(WEB, Some("POST"), Some(("{?}/{?}", Literal)), 1)
            )
            .0,
            Bucket::Dynamic
        );
    }

    /// S13 `ambiguous` and `too_ambiguous`: 2–16 candidates give one edge each with their count;
    /// more give none.
    #[test]
    fn ambiguous_and_too_ambiguous() {
        let eps = endpoints();
        let (bucket, links) = one(
            &eps,
            &site(WEB, Some("POST"), Some(("/internal/mfa/email", Literal)), 1),
        );
        assert_eq!(bucket, Bucket::Ambiguous);
        assert_eq!(
            links,
            [
                "HTTP POST /internal/mfa/email @services/notification [ambiguous] {\"base\":\"literal\",\"candidates\":2,\"client\":\"fetch\",\"match\":\"exact\"}",
                "HTTP POST /internal/mfa/email @services/billing/cmd/billing [ambiguous] {\"base\":\"literal\",\"candidates\":2,\"client\":\"fetch\",\"match\":\"exact\"}",
            ]
        );
        let many = |n: usize| -> Vec<EndpointRef> {
            (0..n)
                .map(|i| ep("GET", "/health", &format!("svc{i:02}")))
                .collect()
        };
        let health = site(WEB, Some("GET"), Some(("/health", Literal)), 1);
        assert_eq!(one(&many(16), &health).1.len(), 16);
        assert_eq!(one(&many(16), &health).0, Bucket::Ambiguous);
        assert_eq!(one(&many(17), &health), (Bucket::TooAmbiguous, vec![]));
    }

    /// Invariant 5 and D6: own-service candidates, a loopback host and a host naming the caller
    /// are `self`, never linked.
    #[test]
    fn own_service_and_loopback_are_self() {
        let eps = endpoints();
        // Loopback: billing's same-path route must not be linked.
        let selfcheck = site(
            NOTIFICATION,
            Some("POST"),
            Some(("http://localhost:3000/internal/mfa/email", Literal)),
            1,
        );
        assert_eq!(one(&eps, &selfcheck), (Bucket::OwnService, vec![]));
        for host in ["127.0.0.1", "0.0.0.0", "[::1]"] {
            let url = format!("http://{host}/internal/mfa/email");
            assert_eq!(
                one(
                    &eps,
                    &site(NOTIFICATION, Some("POST"), Some((&url, Literal)), 1)
                )
                .0,
                Bucket::OwnService,
                "{host}"
            );
        }
        // A host naming the caller's own service.
        assert_eq!(
            one(
                &eps,
                &site(
                    AUTH,
                    Some("GET"),
                    Some(("http://auth/api/v1/login", Literal)),
                    1
                )
            )
            .0,
            Bucket::OwnService
        );
        // Candidates only in the own service.
        assert_eq!(
            one(
                &eps,
                &site(AUTH, Some("GET"), Some(("/api/v1/login", Literal)), 1)
            )
            .0,
            Bucket::OwnService
        );
        // Own candidates are never counted beside others': only notification's is linked.
        let (bucket, links) = one(
            &eps,
            &site(
                BILLING,
                Some("POST"),
                Some(("/internal/mfa/email", Literal)),
                1,
            ),
        );
        assert_eq!(bucket, Bucket::Heuristic);
        assert!(links[0].starts_with("HTTP POST /internal/mfa/email @services/notification"));
    }

    /// Invariant 7 and D6: a literal host naming no service is external, never path-matched; a
    /// host restricts candidates to its service, so a same-path decoy elsewhere is not linked.
    #[test]
    fn hosts_restrict_candidates() {
        let eps = endpoints();
        assert_eq!(
            one(
                &eps,
                &site(
                    WEB,
                    Some("GET"),
                    Some(("https://status.example.com/api/v1/login", Literal)),
                    1
                )
            ),
            (Bucket::External, vec![])
        );
        // `notification` restricts to its service: billing's decoy is not a candidate.
        let (bucket, links) = one(
            &eps,
            &site(
                BILLING,
                Some("POST"),
                Some(("http://notification:3000/internal/mfa/email", Literal)),
                1,
            ),
        );
        assert_eq!(bucket, Bucket::Synthetic);
        assert!(links[0].contains("@services/notification "));
        // The host's service lacks the path: unmatched, even though billing has it.
        assert_eq!(
            one(
                &eps,
                &site(
                    AUTH,
                    Some("POST"),
                    Some(("http://notification/billing/invoices/1/pay", Literal)),
                    1
                )
            )
            .0,
            Bucket::Unmatched
        );
        // A name matches (`auth-service`) as well as the key's last segment (`auth`); case
        // does not matter.
        assert_eq!(
            one(
                &eps,
                &site(
                    WEB,
                    Some("POST"),
                    Some(("http://Auth-Service/api/v1/login", Literal)),
                    1
                )
            )
            .0,
            Bucket::Synthetic
        );
        // A host naming several services restricts nothing and is heuristic.
        let mut two = services();
        two.push(ServiceRef {
            key: "other/notification".into(),
            name: "n2".into(),
        });
        let stitcher = Stitcher::new(&eps, &two);
        let o = stitcher.site(&site(
            AUTH,
            Some("POST"),
            Some(("http://notification/internal/mfa/sms", Literal)),
            1,
        ));
        assert_eq!((o.bucket, o.links.len()), (Bucket::Heuristic, 1));
        // A host with a hole restricts nothing either.
        assert_eq!(
            one(
                &eps,
                &site(
                    AUTH,
                    Some("POST"),
                    Some(("http://{?}/internal/mfa/sms", Literal)),
                    1
                )
            )
            .0,
            Bucket::Heuristic
        );
    }

    /// S14 `dynamic` and `unmatched`.
    #[test]
    fn dynamic_and_unmatched() {
        let eps = endpoints();
        assert_eq!(
            one(&eps, &site(WEB, Some("GET"), None, 1)).0,
            Bucket::Dynamic
        );
        assert_eq!(
            one(&eps, &site(WEB, Some("GET"), Some(("{?}", Literal)), 1)).0,
            Bucket::Dynamic
        );
        assert_eq!(
            one(&eps, &site(WEB, Some("GET"), Some(("/x/{?}", Literal)), 1)).0,
            Bucket::Unmatched
        );
        assert_eq!(
            one(
                &eps,
                &site(WEB, Some("GET"), Some(("/nowhere", Literal)), 1)
            )
            .0,
            Bucket::Unmatched
        );
        // A method mismatch: GET never receives a POST.
        assert_eq!(
            one(
                &eps,
                &site(WEB, Some("DELETE"), Some(("/api/v1/login", Literal)), 1)
            )
            .0,
            Bucket::Unmatched
        );
        // A fully literal root is not dynamic.
        let root = vec![ep("GET", "/", AUTH)];
        assert_eq!(
            one(&root, &site(WEB, Some("GET"), Some(("/", Literal)), 1)).0,
            Bucket::Heuristic
        );
    }

    /// D9: an endpoint parameter matches any client segment, a client hole only a parameter; the
    /// most literal segments win, then an exact method over `ANY`; a tie is ambiguous.
    #[test]
    fn d9_specificity() {
        let eps = vec![
            ep("GET", "/u/{id}", AUTH),
            ep("GET", "/u/me", NOTIFICATION),
            ep("ANY", "/v/x", AUTH),
            ep("GET", "/v/x", NOTIFICATION),
            ep("GET", "/w/{id}", AUTH),
            ep("GET", "/w/{id}", NOTIFICATION),
        ];
        let target = |url: &str| {
            let (bucket, links) = one(&eps, &site(WEB, Some("GET"), Some((url, Literal)), 1));
            (
                bucket,
                links
                    .iter()
                    .map(|l| l.split(" [").next().unwrap().to_owned())
                    .collect::<Vec<_>>(),
            )
        };
        assert_eq!(
            target("/u/me"),
            (
                Bucket::Heuristic,
                vec!["HTTP GET /u/me @services/notification".to_owned()]
            )
        );
        assert_eq!(
            target("/u/42"),
            (
                Bucket::Heuristic,
                vec!["HTTP GET /u/{} @services/auth".to_owned()]
            )
        );
        assert_eq!(
            target("/u/{?}"),
            (
                Bucket::Heuristic,
                vec!["HTTP GET /u/{} @services/auth".to_owned()]
            )
        );
        assert_eq!(
            target("/v/x"),
            (
                Bucket::Heuristic,
                vec!["HTTP GET /v/x @services/notification".to_owned()]
            )
        );
        assert_eq!(target("/w/1").0, Bucket::Ambiguous);
        // An unknown method takes both sides of the method rule.
        let (bucket, _) = one(&eps, &site(WEB, None, Some(("/v/x", Literal)), 1));
        assert_eq!(bucket, Bucket::Ambiguous);
    }

    /// D9 for a suffix match (contract M4a amendment 9, measured on
    /// spring-petclinic-microservices): specificity counts the literal segments of the endpoint
    /// tail the client matched, not of the whole endpoint path. Counting the whole path sent
    /// `hostname + "pets/visits"` to `/owners/{}/pets/{}/visits` (3 literals, matched tail
    /// `{}/visits`) instead of `/pets/visits`, and `base + "/owners"` and `base + "vets"` to
    /// `/api/gateway/owners/{}` (its tail `{}` matches any segment).
    #[test]
    fn suffix_specificity_counts_the_matched_tail() {
        let eps = vec![
            ep("GET", "/owners/{ownerId}/pets/{petId}/visits", AUTH),
            ep("GET", "/pets/visits", AUTH),
            ep("GET", "/api/gateway/owners/{ownerId}", NOTIFICATION),
            ep("GET", "/owners", BILLING),
            ep("GET", "/vets", BILLING),
        ];
        let target = |url: &str| {
            let (bucket, links) = one(&eps, &site(WEB, Some("GET"), Some((url, Literal)), 1));
            (
                bucket,
                links
                    .iter()
                    .map(|l| l.split(" [").next().unwrap().to_owned())
                    .collect::<Vec<_>>(),
            )
        };
        for (url, want) in [
            ("{?}pets/visits", "HTTP GET /pets/visits @services/auth"),
            (
                "{?}/owners",
                "HTTP GET /owners @services/billing/cmd/billing",
            ),
            ("{?}vets", "HTTP GET /vets @services/billing/cmd/billing"),
        ] {
            assert_eq!(
                target(url),
                (Bucket::Heuristic, vec![want.to_owned()]),
                "{url}"
            );
        }
        // A client longer than an endpoint's tail still counts only what it matched: both
        // tails `{}/visits` and `pets/visits` are compared on two segments.
        assert_eq!(
            target("{?}/x/pets/visits").0,
            Bucket::Unmatched,
            "no endpoint ends in x/pets/visits"
        );
    }

    /// Invariant 4: several sites of one source and endpoint are one edge with the best site's
    /// certainty and attributes, `sites = n` and every site's evidence; each site is counted in
    /// exactly one bucket, and the buckets sum to the sites.
    #[test]
    fn sites_merge_per_edge_and_buckets_sum_to_the_sites() {
        let eps = endpoints();
        let mut a = site(
            AUTH,
            Some("POST"),
            Some(("http://notification/internal/mfa/email", EnvDefault)),
            1,
        );
        let mut b = site(
            AUTH,
            Some("POST"),
            Some(("http://notification/internal/mfa/email", Literal)),
            2,
        );
        b.src = a.src.clone();
        a.client = ClientKind::Axios;
        let sites = vec![
            a,
            b,
            site(WEB, Some("POST"), Some(("/internal/mfa/email", Literal)), 3),
            site(WEB, Some("GET"), None, 4),
            site(
                WEB,
                Some("GET"),
                Some(("https://status.example.com/api/v1/login", Literal)),
                5,
            ),
            site(
                NOTIFICATION,
                Some("POST"),
                Some(("http://localhost/internal/mfa/email", Literal)),
                6,
            ),
            site(WEB, Some("GET"), Some(("/nowhere", Literal)), 7),
            site(
                WEB,
                Some("POST"),
                Some(("{?}/invoices/{?}/pay", Literal)),
                8,
            ),
        ];
        let mut g = GraphBuilder::new();
        for e in &eps {
            g.add_node(
                NodeKey::new(NodeKind::Endpoint, e.key.clone()),
                &e.key,
                Attrs::new(),
            )
            .unwrap();
        }
        for s in &sites {
            if !g.has_node(&s.src) {
                g.add_node(s.src.clone(), "f", Attrs::new()).unwrap();
            }
            let file = NodeKey::new(NodeKind::File, s.at.path.clone());
            if !g.has_node(&file) {
                g.add_node(file, "f.ts", Attrs::new()).unwrap();
            }
        }
        let diagnostics = add_consumes(&mut g, &sites, &eps, &services()).unwrap();
        let graph: Graph = g.finish().unwrap();
        let consumes: Vec<&rb_model::Edge> = graph
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Consumes)
            .collect();
        let merged = consumes
            .iter()
            .find(|e| e.attrs["sites"] == 2)
            .expect("the two auth sites merge");
        assert_eq!(merged.certainty, Certainty::Synthetic);
        assert_eq!(merged.attrs["base"], "literal");
        assert_eq!(
            merged.attrs["client"], "fetch",
            "the best site's attributes"
        );
        assert_eq!(merged.evidence.len(), 2);
        assert_eq!(merged.provenance, Provenance::HttpStitch);
        assert!(
            consumes
                .iter()
                .all(|e| e.certainty < Certainty::Exact && !e.evidence.is_empty())
        );
        assert_eq!(consumes.len(), 4, "1 merged + 2 ambiguous + 1 suffix");
        let coverage = diagnostics
            .iter()
            .find(|d| d.kind == "http_coverage")
            .unwrap();
        assert_eq!(
            coverage.detail,
            json!({"sites": 8, "synthetic": 1, "heuristic": 2, "ambiguous": 1, "too_ambiguous": 0,
                   "unmatched": 1, "self": 1, "external": 1, "dynamic": 1})
        );
        let total: u64 = Bucket::ALL
            .iter()
            .map(|b| coverage.detail[b.as_str()].as_u64().unwrap())
            .sum();
        assert_eq!(total, sites.len() as u64);
        let kinds: Vec<&str> = diagnostics.iter().map(|d| d.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "http_url_dynamic",
                "http_external",
                "http_self",
                "http_unmatched",
                "http_coverage"
            ]
        );
        let external = &diagnostics[1];
        assert_eq!(external.detail["host"], "status.example.com");
        assert_eq!(
            external.detail["url"],
            "https://status.example.com/api/v1/login"
        );
    }

    /// The index is a filter in front of [`match_path`]: over generated endpoints and clients it
    /// returns exactly what a scan of every endpoint with `match_path` returns.
    #[test]
    fn the_candidate_index_agrees_with_the_reference_matcher() {
        const SEGS: &[&str] = &["a", "b", "c", "{}", "{*}"];
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let mut path = |n: usize, wild: bool| -> String {
            let mut p = String::new();
            for i in 0..n {
                let pool = if wild && i + 1 == n {
                    SEGS.len()
                } else {
                    SEGS.len() - 1
                };
                p.push('/');
                p.push_str(SEGS[(next() % pool as u64) as usize]);
            }
            p
        };
        let eps: Vec<EndpointRef> = (0..300)
            .map(|i| ep("GET", &path(i % 5, true), &format!("s{i}")))
            .collect();
        let index = EndpointIndex::new(&eps);
        for i in 0..2000 {
            let client = segments(&norm_path(&path(i % 5, false)));
            for suffix in [false, true] {
                let want: Vec<usize> = (0..eps.len())
                    .filter(|&e| match_path(&client, suffix, &index.segments[e]).is_some())
                    .collect();
                let got: Vec<usize> = index
                    .candidates(&client, suffix)
                    .into_iter()
                    .map(|(e, _)| e)
                    .collect();
                assert_eq!(got, want, "{client:?} suffix={suffix}");
            }
        }
    }

    /// `n` endpoints in `n` services with distinct literal prefixes and tails, and `n` sites:
    /// half exact (relative), half suffix (unknown base).
    fn wide(n: usize) -> (Vec<EndpointRef>, Vec<Site>, Vec<ServiceRef>) {
        let eps = (0..n)
            .map(|i| {
                ep(
                    "POST",
                    &format!("/p{i}/items/{{id}}/t{i}"),
                    &format!("s{i}"),
                )
            })
            .collect();
        let sites = (0..n)
            .map(|i| {
                let url = if i % 2 == 0 {
                    format!("/p{i}/items/7/t{i}")
                } else {
                    format!("{{?}}/items/{{?}}/t{i}")
                };
                site("web", Some("POST"), Some((&url, Literal)), i as i32)
            })
            .collect();
        let services = (0..n)
            .map(|i| ServiceRef {
                key: format!("s{i}"),
                name: format!("s{i}"),
            })
            .collect();
        (eps, sites, services)
    }

    fn time_stitch(
        eps: &[EndpointRef],
        sites: &[Site],
        services: &[ServiceRef],
    ) -> (std::time::Duration, Vec<Bucket>) {
        let mut buckets = Vec::new();
        let time = (0..5)
            .map(|_| {
                let start = std::time::Instant::now();
                for _ in 0..4 {
                    let stitcher = Stitcher::new(eps, services);
                    buckets = sites.iter().map(|s| stitcher.site(s).bucket).collect();
                }
                start.elapsed()
            })
            .min()
            .unwrap();
        (time, buckets)
    }

    /// Invariant 11: doubling endpoints and sites with distinct prefixes costs at most 2.5 times
    /// as much (best of five timings of four runs each, so a timing is well above scheduling
    /// noise under the parallel test run; a ratio, not a wall-clock bound).
    #[test]
    fn the_stitcher_grows_linearly() {
        let (e1, s1, v1) = wide(1000);
        let (e2, s2, v2) = wide(2000);
        let (small, buckets) = time_stitch(&e1, &s1, &v1);
        assert!(
            buckets.iter().all(|b| *b == Bucket::Heuristic),
            "{buckets:?}"
        );
        let (large, _) = time_stitch(&e2, &s2, &v2);
        let ratio = large.as_secs_f64() / small.as_secs_f64();
        assert!(ratio < 2.5, "{small:?} → {large:?} ({ratio:.2}×)");
    }

    /// S19's pathological input: 2,000 endpoints of 16 parameters each, in 2,000 services,
    /// against 2,000 sites of 16 literal segments: every site is `too_ambiguous`, within a bound
    /// set an order of magnitude above a measured local run.
    #[test]
    fn an_all_parameter_input_is_too_ambiguous_within_a_bound() {
        let n = 2000;
        let all = "/{p}".repeat(16);
        let eps: Vec<EndpointRef> = (0..n).map(|i| ep("GET", &all, &format!("s{i}"))).collect();
        let services: Vec<ServiceRef> = (0..n)
            .map(|i| ServiceRef {
                key: format!("s{i}"),
                name: format!("s{i}"),
            })
            .collect();
        let url = "/x".repeat(16);
        let sites: Vec<Site> = (0..n)
            .map(|i| site("web", Some("GET"), Some((&url, Literal)), i as i32))
            .collect();
        let start = std::time::Instant::now();
        let stitcher = Stitcher::new(&eps, &services);
        let outcomes: Vec<Outcome> = sites.iter().map(|s| stitcher.site(s)).collect();
        let elapsed = start.elapsed();
        assert!(
            outcomes.iter().all(|o| o.bucket == Bucket::TooAmbiguous
                && o.candidates == n
                && o.links.is_empty())
        );
        assert!(
            elapsed < std::time::Duration::from_secs(BOUND_SECS),
            "{elapsed:?}"
        );
    }

    /// Every site of `files` (path, language, source) as `path client METHOD url [origin]`, each
    /// file in the service named by its first path segment, with `properties` per service.
    fn folded_sites(
        files: &[(&str, Language, &str)],
        go_modules: &[(&str, &str)],
        properties: &[(&str, &str, &str)],
    ) -> Vec<String> {
        let facts: Vec<(&str, Language, FileFacts)> = files
            .iter()
            .map(|(p, l, src)| (*p, *l, rb_syntax::extract(*l, src.as_bytes()).unwrap()))
            .collect();
        let by_path: BTreeMap<&str, (Language, &FileFacts)> =
            facts.iter().map(|(p, l, f)| (*p, (*l, f))).collect();
        let keys: BTreeMap<&str, Vec<String>> = by_path
            .iter()
            .map(|(p, (_, f))| (*p, rb_syntax::symbol_keys(p, f)))
            .collect();
        let go: BTreeMap<String, String> = go_modules
            .iter()
            .map(|(d, m)| ((*d).to_owned(), (*m).to_owned()))
            .collect();
        let index = Index::new(&by_path, &go, &keys);
        let mut props: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for (service, k, v) in properties {
            props
                .entry((*service).to_owned())
                .or_default()
                .insert((*k).to_owned(), (*v).to_owned());
        }
        let service_of = |p: &str| p.split('/').next().unwrap_or(p).to_owned();
        let inputs = Inputs {
            files: &by_path,
            index: &index,
            keys: &keys,
            service_of: &service_of,
            properties: &props,
        };
        sites(&inputs)
            .iter()
            .map(|s| {
                let url = s.url.as_ref().map_or("?".to_owned(), |u| {
                    format!("{} [{}]", render(u), u.origin.as_str())
                });
                format!(
                    "{} {} {} {url}",
                    s.at.path,
                    s.client.as_str(),
                    s.method.as_deref().unwrap_or("?")
                )
            })
            .collect()
    }

    /// S12 across files: a TS named import (aliased), a Java `Type.CONST` and static import, a
    /// Go `pkg.Const`, each through M3a resolution; mutable declarations never fold.
    #[test]
    fn constants_fold_across_files() {
        let got = folded_sites(
            &[
                (
                    "web/src/api.ts",
                    Language::TypeScript,
                    "import { BASE as B } from \"./config\";\nexport function f() { return fetch(`${B}/api/v1/login`, { method: \"POST\" }); }\n",
                ),
                (
                    "web/src/config.ts",
                    Language::TypeScript,
                    "import { HOST } from \"./hosts\";\nexport const BASE = HOST + \"/\";\nexport let MUTABLE = \"http://m\";\n",
                ),
                (
                    "web/src/hosts.ts",
                    Language::TypeScript,
                    "export const HOST = \"http://gateway\";\n",
                ),
                (
                    "auth/src/main/java/p/Client.java",
                    Language::Java,
                    "package p;\nimport static p.Paths.ROOT;\nimport org.springframework.web.client.RestTemplate;\nclass Client {\n  RestTemplate rest;\n  void a() { rest.getForObject(Paths.NOTIFY + \"/x\", String.class); }\n  void b() { rest.getForObject(ROOT + \"/y\", String.class); }\n  void c() { rest.getForObject(Paths.MUTABLE + \"/z\", String.class); }\n}\n",
                ),
                (
                    "auth/src/main/java/p/Paths.java",
                    Language::Java,
                    "package p;\nclass Paths {\n  static final String NOTIFY = \"http://notification:3000\";\n  static final String ROOT = NOTIFY + \"/root\";\n  static String MUTABLE = \"http://m\";\n}\n",
                ),
                (
                    "billing/cmd/main.go",
                    Language::Go,
                    "package main\nimport (\n\t\"net/http\"\n\t\"example.com/billing/cfg\"\n)\nfunc notifyPaid() { http.Post(cfg.Notify + \"/internal/mfa/email\", \"application/json\", nil) }\nfunc mutable() { http.Get(cfg.Mutable + \"/x\") }\n",
                ),
                (
                    "billing/cfg/cfg.go",
                    Language::Go,
                    "package cfg\nconst Notify = \"http://notification:3000\"\nvar Mutable = \"http://m\"\n",
                ),
            ],
            &[("billing", "example.com/billing")],
            &[],
        );
        assert_eq!(
            got,
            [
                "auth/src/main/java/p/Client.java rest_template GET http://notification:3000/x [constant]",
                "auth/src/main/java/p/Client.java rest_template GET http://notification:3000/root/y [constant]",
                "auth/src/main/java/p/Client.java rest_template GET {?}/z [literal]",
                "billing/cmd/main.go net/http POST http://notification:3000/internal/mfa/email [constant]",
                "billing/cmd/main.go net/http GET {?}/x [literal]",
                "web/src/api.ts fetch POST http://gateway//api/v1/login [constant]",
            ]
        );
    }

    /// Properties come from the caller's own service only (`@Value`, a Feign `${…}` url); an
    /// imported name is an axios client only when its module binds an axios instance of that name
    /// at top level, whose base it takes; a local variable shadows a package qualifier; an
    /// imported-constant cycle terminates unresolved.
    #[test]
    fn properties_instances_shadowing_and_cycles() {
        let feign = "package p;\nimport org.springframework.cloud.openfeign.FeignClient;\nimport org.springframework.web.bind.annotation.PostMapping;\n@FeignClient(name = \"n\", url = \"${notify.url}\")\ninterface Notify { @PostMapping(\"/internal/mfa/email\") void send(); }\n";
        let got = folded_sites(
            &[
                ("auth/src/N.java", Language::Java, feign),
                ("other/src/N.java", Language::Java, feign),
                (
                    "web/src/http.ts",
                    Language::TypeScript,
                    "import axios from \"axios\";\nexport const api = axios.create({ baseURL: \"http://gateway\" });\nexport const notAxios = { get() {} };\n",
                ),
                (
                    "web/src/use.ts",
                    Language::TypeScript,
                    "import { api, notAxios } from \"./http\";\nimport { other } from \"./missing\";\nexport function f() { api.get(\"/api/v1/me\"); notAxios.get(\"/x\"); other.get(\"/y\"); }\n",
                ),
                (
                    "web/src/cycle.ts",
                    Language::TypeScript,
                    "import { B } from \"./cycle2\";\nexport const A = B;\nfetch(`${A}/x`);\n",
                ),
                (
                    "web/src/cycle2.ts",
                    Language::TypeScript,
                    "import { A } from \"./cycle\";\nexport const B = A;\n",
                ),
                (
                    "billing/main.go",
                    Language::Go,
                    "package main\nimport (\n\t\"net/http\"\n\t\"example.com/billing/cfg\"\n)\nfunc f() {\n\tcfg := local()\n\thttp.Get(cfg.Notify + \"/x\")\n}\n",
                ),
                (
                    "billing/cfg/cfg.go",
                    Language::Go,
                    "package cfg\nconst Notify = \"http://notification\"\n",
                ),
            ],
            &[("billing", "example.com/billing")],
            &[("auth", "notify.url", "http://notification:3000")],
        );
        assert_eq!(
            got,
            [
                "auth/src/N.java feign POST http://notification:3000/internal/mfa/email [config]",
                "billing/main.go net/http GET {?}/x [literal]",
                "other/src/N.java feign POST {?}/internal/mfa/email [literal]",
                "web/src/cycle.ts fetch GET {?}/x [literal]",
                "web/src/use.ts axios GET http://gateway/api/v1/me [literal]",
            ]
        );
    }

    /// Two declarations a name may mean fold to nothing: the constant is not known.
    #[test]
    fn ambiguous_declarations_do_not_fold() {
        let got = folded_sites(
            &[
                (
                    "billing/main.go",
                    Language::Go,
                    "package main\nimport (\n\t\"net/http\"\n\t\"example.com/billing/cfg\"\n)\nfunc f() { http.Get(cfg.Base + \"/x\") }\n",
                ),
                (
                    "billing/cfg/a.go",
                    Language::Go,
                    "package cfg\nconst Base = \"http://a\"\n",
                ),
                (
                    "billing/cfg/b.go",
                    Language::Go,
                    "package cfg\nconst Base = \"http://b\"\n",
                ),
            ],
            &[("billing", "example.com/billing")],
            &[],
        );
        assert_eq!(got, ["billing/main.go net/http GET {?}/x [literal]"]);
    }
}
