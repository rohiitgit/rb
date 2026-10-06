//! HTTP stitching (design §10.2, contract M4a S12–S14): every client call site's URL is folded
//! (across files through M3a resolution and the owning service's `application.properties`),
//! matched to the endpoints of other services, and linked by `Symbol | File -CONSUMES-> Endpoint`
//! with provenance `http_stitch`, never `exact`. Every site lands in exactly one coverage bucket.
//!
//! Candidates come from an index bucketed by an endpoint's first segment (exact matches) and last
//! segment (suffix matches), and each is confirmed by the reference matcher
//! ([`rb_frameworks::endpoint::match_path`]), so the index decides nothing D9 does not.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rb_frameworks::clients::{AxiosInstance, ClientKind, FileClients, UrlValue};
use rb_frameworks::endpoint::{
    ANY, PathMatch, Piece, Segment, client_url, literal_segments, match_path, methods_match,
    segments,
};
use rb_frameworks::fold::{self, Folded, Lookup, Origin};
use rb_frameworks::work::Counted;
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
    /// A Spring property the URL folded is also set by a configuration file M4a does not read
    /// (a profile, YAML or bootstrap file), so the folded value may not be the deployed one
    /// (evaluator pass 1, P7).
    pub config_overridden: bool,
    /// The base was read from a `@Bean` method taken as the injected instance (amendment 12).
    pub base_from_bean: bool,
    /// Gate A (amendment 13): the call is on an instance whose base comes from its construction,
    /// and nothing can change that base afterwards (`Some(true)`) or something may
    /// (`Some(false)`: the host still restricts, the site is never `synthetic`, and its edge
    /// says `base: unverified`); `None` for a call on no such instance.
    pub base_verified: Option<bool>,
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

/// What Spring configuration M4a does not read may set for one service (evaluator pass 1, P7).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Overrides {
    /// Keys set by profile, YAML or `bootstrap*` files.
    pub keys: BTreeSet<String>,
    /// A Spring Cloud Config client: the server may set any key.
    pub all: bool,
}

impl Overrides {
    pub(crate) fn sets(&self, key: &str) -> bool {
        self.all || self.keys.contains(key)
    }
}

/// What site collection reads.
pub(crate) struct Inputs<'a, 'i> {
    pub files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
    pub index: &'i Index<'a>,
    pub keys: &'a BTreeMap<&'a str, Vec<String>>,
    /// The key of the service owning a path.
    pub service_of: &'i dyn Fn(&str) -> String,
    /// Service key → its `application.properties`.
    pub properties: &'i BTreeMap<String, BTreeMap<String, String>>,
    /// Service key → what unread configuration may set.
    pub overrides: &'i BTreeMap<String, Overrides>,
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
    /// The `(service, key)` of every property asked for since the last [`Folding::take_asked`].
    asked: RefCell<Vec<(String, String)>>,
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
            asked: RefCell::new(Vec::new()),
        }
    }

    /// Whether a property asked for since the last call is one unread configuration may set.
    fn take_asked(&self) -> bool {
        self.asked.take().iter().any(|(service, key)| {
            self.inputs
                .overrides
                .get(service)
                .is_some_and(|o| o.sets(key))
        })
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
        let value = self
            .inputs
            .properties
            .get(&service)
            .and_then(|p| p.get(key))
            .cloned();
        self.asked.borrow_mut().push((service, key.to_owned()));
        value
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
    let tainted = escaped_instances(inputs, &clients);
    let mut out = Vec::new();
    for (&path, file_clients) in &clients {
        let file = folding.ids[path];
        let facts = inputs.files[path].1;
        let service = (inputs.service_of)(path);
        for call in &file_clients.calls {
            folding.take_asked();
            // A base stated at the call (axios request config) replaces the instance's
            // (amendment 12).
            let stated = call.base.clone().map(|b| (file, b));
            // Gate A across files (amendment 13): a top-level instance a file importing it
            // uses otherwise is unverified.
            let unverify = |target: &str, name: &str| {
                tainted.contains(&(target, name)) || tainted.contains(&(target, "*"))
            };
            let mut base_verified = call.base_verified;
            if let (Some(true), Some(name)) = (base_verified, &call.top_instance)
                && unverify(path, name)
            {
                base_verified = Some(false);
            }
            let base = match &call.instance {
                Some(name) => match imported_instance(inputs, &clients, path, facts, name) {
                    Some((target, instance)) => {
                        if instance.base.is_some() {
                            base_verified = Some(
                                instance.verified && !unverify(target, instance.name.as_str()),
                            );
                        }
                        stated.or_else(|| instance.base.clone().map(|b| (folding.ids[target], b)))
                    }
                    None => continue,
                },
                None => stated,
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
                config_overridden: folding.take_asked(),
                base_from_bean: call.base_from_bean,
                base_verified,
            });
        }
    }
    out
}

/// Gate A across files (contract M4a amendment 13): `(file, exported name)` for every name a
/// file imports and uses other than as the receiver of a request call
/// ([`FileClients::escaped_imports`]), resolved through M3a module resolution; `(file, "*")`
/// for a namespace import used so, which may reach any of the module's exports.
fn escaped_instances<'a>(
    inputs: &Inputs<'a, '_>,
    clients: &BTreeMap<&'a str, FileClients>,
) -> HashSet<(&'a str, &'a str)> {
    let mut out = HashSet::new();
    for (&path, fc) in clients {
        if fc.escaped_imports.is_empty() {
            continue;
        }
        let facts = inputs.files[path].1;
        let escaped = |name: &str| {
            fc.escaped_imports
                .binary_search_by(|e| e.as_str().cmp(name))
                .is_ok()
        };
        for (import, files) in facts.imports.iter().zip(inputs.index.import_targets(path)) {
            for &f in files {
                rb_frameworks::work::tick(1);
                for n in import.names.iter().filter(|n| escaped(&n.local)) {
                    out.insert((f, n.name.as_str()));
                }
                if import.alias.as_deref().is_some_and(escaped) {
                    out.insert((f, "*"));
                }
            }
        }
    }
    out
}

/// The axios instance an imported name is, with its file: the first resolved target (in path
/// order) binding an instance of the exported name at top level.
fn imported_instance<'a, 'c>(
    inputs: &Inputs<'a, '_>,
    clients: &'c BTreeMap<&'a str, FileClients>,
    path: &str,
    facts: &FileFacts,
    local: &str,
) -> Option<(&'a str, &'c AxiosInstance)> {
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
                .map(|a| (target, a))
        })
}

/// A URL relative to a base: the base then the URL (with a `/` between unless one side has it),
/// unless the URL is absolute. An unknown base is a hole. A URL that starts with an unknown
/// part is not joined to any base (amendment 13, Gate B): that part may be absolute (axios and
/// Spring then ignore the base) or a prefix, so it is the unknown base, and the path is matched
/// by suffix only. The label is the weakest among the parts that resolved.
fn join(base: Option<Folded>, url: Folded) -> Folded {
    let absolute = {
        let u = client_url(&url.pieces);
        u.host.is_some() || u.host_dynamic
    };
    if absolute || url.pieces.first() == Some(&Piece::Hole) {
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
            .inspect(|_| rb_frameworks::work::tick(1))
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
    /// Counted when scanned, so a scan per site is measured (amendment 13; R6).
    endpoints: Counted<'e, EndpointRef>,
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
            endpoints: Counted::new(endpoints),
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
        // A host hole next to an FQDN fragment names no service (amendment 12, Q2).
        if url.host_external.is_some() {
            return Outcome::bucket(Bucket::External);
        }
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
        // Candidates of every service the host allows, the caller's own included: they are
        // ranked together, so excluding the own service never promotes a worse foreign match
        // (evaluator pass 1, P2). An endpoint registered with a host pattern needs that host
        // (P8).
        let found: Vec<(usize, PathMatch)> = self
            .index
            .candidates(&client, url.base_unknown)
            .into_iter()
            .filter(|(i, _)| {
                let e = &self.endpoints[*i];
                methods_match(method, &e.method)
                    && (e.hosts.is_empty()
                        || url.host.as_ref().is_some_and(|h| e.hosts.contains(h)))
                    && match scope {
                        Scope::Only(s) => e.service == s,
                        Scope::Any | Scope::Proxy => true,
                    }
            })
            .collect();
        if found.is_empty() {
            return Outcome::bucket(Bucket::Unmatched);
        }
        let best = self.most_specific(found, method, client.len());
        // The best match is the caller's own endpoint: a call within the service (invariant 5).
        if best
            .iter()
            .any(|(i, _)| self.endpoints[*i].service == site.service)
        {
            return Outcome::bucket(Bucket::OwnService);
        }
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
        let unverified = site.base_verified == Some(false);
        attrs.insert(
            "base".into(),
            if unverified {
                "unverified"
            } else {
                folded.origin.as_str()
            }
            .into(),
        );
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
            && !url.flags.opaque_tail
            && !site.config_overridden
            && !site.base_from_bean
            && !unverified
            && !e.prefix_unknown
            && !e.prefix_maybe_incomplete;
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
                let u = client_url(&url.pieces);
                if let Some(host) = u.host.or(u.host_external) {
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
        // Sites on an instance whose base comes from its construction, by Gate A.
        let bases = |v: bool| sites.iter().filter(|s| s.base_verified == Some(v)).count();
        detail.insert(
            "bases".into(),
            json!({ "verified": bases(true), "unverified": bases(false) }),
        );
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
        ("services/orders", "orders"),
        ("services/users", "users"),
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
            hosts: Vec::new(),
            prefix_maybe_incomplete: false,
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
            config_overridden: false,
            base_from_bean: false,
            base_verified: None,
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
        // A client catch-all before the last segment, read as `{}` (`wildcard_not_last`).
        let mut inner = eps.clone();
        inner.push(ep("GET", "/u/{id}/x", NOTIFICATION));
        lowered(
            site(
                AUTH,
                Some("GET"),
                Some(("http://notification/u/*/x", Literal)),
                1,
            ),
            &inner,
        );
        // An opaque tail (evaluator pass 1, P3).
        lowered(
            site(
                AUTH,
                Some("POST"),
                Some(("http://notification/internal/mfa/email{?}", Literal)),
                1,
            ),
            &eps,
        );
        // An endpoint whose service has configuration M4a does not read (P7).
        let mut unread = eps.clone();
        unread[2].prefix_maybe_incomplete = true;
        lowered(
            site(
                AUTH,
                Some("POST"),
                Some(("http://notification/internal/mfa/email", Literal)),
                1,
            ),
            &unread,
        );
        // A folded property that a profile or YAML file also sets (P7).
        let mut overridden = site(
            AUTH,
            Some("POST"),
            Some(("http://notification/internal/mfa/email", Config)),
            1,
        );
        overridden.config_overridden = true;
        lowered(overridden, &eps);
        // A base read from a `@Bean` method, taken as the injected instance (amendment 12).
        let mut bean = site(
            AUTH,
            Some("POST"),
            Some(("http://notification/internal/mfa/email", Literal)),
            1,
        );
        bean.base_from_bean = true;
        lowered(bean, &eps);
        // A base its instance's file may change after construction (amendment 13, Gate A).
        let mut unverified = site(
            AUTH,
            Some("POST"),
            Some(("http://notification/internal/mfa/email", Literal)),
            1,
        );
        unverified.base_verified = Some(false);
        let shown = lowered(unverified, &eps);
        assert!(shown.contains("\"base\":\"unverified\""), "{shown}");
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
        // A host naming the caller's own service when that service exposes nothing: still
        // `self`, never a gateway path-matched across services (evaluator pass 2, Q5, M23).
        assert_eq!(
            one(
                &eps,
                &site(
                    WEB,
                    Some("POST"),
                    Some(("http://web/api/v1/login", Literal)),
                    1
                )
            ),
            (Bucket::OwnService, vec![])
        );
        // A bracketed IPv6 loopback followed directly by a hole (Q5, M02).
        assert_eq!(
            one(
                &eps,
                &site(
                    NOTIFICATION,
                    Some("POST"),
                    Some(("http://[::1]{?}/internal/mfa/email", Literal)),
                    1
                )
            ),
            (Bucket::OwnService, vec![])
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
        // Own candidates rank with the others: an own endpoint as good as notification's makes
        // the site `self` (evaluator pass 1, P2).
        assert_eq!(
            one(
                &eps,
                &site(
                    BILLING,
                    Some("POST"),
                    Some(("/internal/mfa/email", Literal)),
                    1,
                ),
            ),
            (Bucket::OwnService, vec![])
        );
    }

    /// Evaluator pass 1, P2 (measured on piggymetrics): a suffix match needs a literal in the
    /// endpoint tail it matched, and the caller's own endpoints rank with the others, so the
    /// own-service exclusion never promotes a worse foreign candidate.
    #[test]
    fn suffix_evidence_and_own_service_ranking() {
        // `ExchangeRatesClient`: an unresolved Feign url and `/latest`; account and statistics
        // (the caller's own service) each expose `GET /{name}`.
        let eps = vec![
            ep("GET", "/{name}", AUTH),
            ep("GET", "/{accountName}", BILLING),
            ep("GET", "/{x}", NOTIFICATION),
        ];
        assert_eq!(
            one(
                &eps,
                &site(BILLING, Some("GET"), Some(("{?}/latest", Literal)), 1)
            ),
            (Bucket::Unmatched, vec![])
        );
        // An own endpoint ranked above a foreign one: `self`, not the foreign one.
        let eps = vec![ep("POST", "/x/pay", BILLING), ep("POST", "/{p}/pay", AUTH)];
        assert_eq!(
            one(
                &eps,
                &site(BILLING, Some("POST"), Some(("{?}/x/pay", Literal)), 1)
            ),
            (Bucket::OwnService, vec![])
        );
        // A foreign endpoint ranked above an own one is linked.
        let eps = vec![ep("POST", "/{p}/pay", BILLING), ep("POST", "/x/pay", AUTH)];
        let (bucket, links) = one(
            &eps,
            &site(BILLING, Some("POST"), Some(("{?}/x/pay", Literal)), 1),
        );
        assert_eq!(bucket, Bucket::Heuristic);
        assert!(
            links[0].starts_with("HTTP POST /x/pay @services/auth "),
            "{links:?}"
        );
        // A host naming another service still restricts the ranking to that service.
        let eps = vec![ep("GET", "/a/b", BILLING), ep("GET", "/a/{x}", AUTH)];
        assert_eq!(
            one(
                &eps,
                &site(BILLING, Some("GET"), Some(("http://auth/a/b", Literal)), 1)
            )
            .0,
            Bucket::Synthetic
        );
    }

    /// Evaluator pass 1, P3: a hole glued to the end of the URL is an opaque tail, so
    /// `"/api/items" + qs` matches `/api/items`, not another service's `/api/{name}` or an SPA
    /// catch-all.
    #[test]
    fn an_opaque_tail_matches_the_literal_path() {
        let eps = vec![
            ep("GET", "/api/items", "services/orders"),
            ep("GET", "/api/{name}", NOTIFICATION),
            ep("GET", "/*", AUTH),
        ];
        let (bucket, links) = one(
            &eps,
            &site(WEB, Some("GET"), Some(("/api/items{?}", Literal)), 1),
        );
        assert_eq!(bucket, Bucket::Heuristic);
        assert!(
            links[0].starts_with("HTTP GET /api/items @services/orders "),
            "{links:?}"
        );
        // A hole after `/` is still a whole-segment parameter.
        let (_, links) = one(
            &eps,
            &site(WEB, Some("GET"), Some(("/api/items/{?}", Literal)), 1),
        );
        assert!(
            links[0].starts_with("HTTP GET /{*} @services/auth "),
            "{links:?}"
        );
    }

    /// Evaluator pass 1, P1 (the evaluator's repros, folded URLs): a hole in the port keeps a
    /// literal host, so loopback is `self`, an external host is never path-matched and a host
    /// naming a service restricts candidates to it.
    #[test]
    fn port_holes_keep_the_host() {
        const ORDERS: &str = "services/orders";
        const USERS: &str = "services/users";
        let eps = vec![
            ep("GET", "/orders/{id}", ORDERS),
            ep("GET", "/health", USERS),
            ep("GET", "/users/me", USERS),
        ];
        for (service, url, want) in [
            (USERS, "http://localhost:{?}/orders/1", Bucket::OwnService),
            (
                WEB,
                "https://status.example.com:{?}/orders/1",
                Bucket::External,
            ),
            (WEB, "http://users:{?}/orders/1", Bucket::Unmatched),
            (ORDERS, "http://localhost:{?}/health", Bucket::OwnService),
            (ORDERS, "http://127.0.0.1:{?}/users/me", Bucket::OwnService),
            (ORDERS, "http://users:{?}/users/me", Bucket::Synthetic),
            // Only a hole in the host itself restricts nothing.
            (ORDERS, "http://{?}:8080/users/me", Bucket::Heuristic),
            // A hole glued to a literal port keeps the host; the rest is a suffix restricted to
            // that host's service (amendment 12, Q6).
            (WEB, "http://orders:3000{?}/orders/1", Bucket::Heuristic),
            (WEB, "http://users:3000{?}/orders/1", Bucket::Unmatched),
        ] {
            assert_eq!(
                one(&eps, &site(service, Some("GET"), Some((url, Literal)), 1)).0,
                want,
                "{service} {url}"
            );
        }
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q2 and Q6, folded URLs): a hole next to
    /// literal host text holding a dot is external; any other host hole leaves the base unknown,
    /// so the path is matched by suffix only, `heuristic` at most; a hole glued to a literal
    /// port is a suffix within that host's service.
    #[test]
    fn partial_hosts_are_external_or_matched_by_suffix() {
        const ORDERS: &str = "services/orders";
        const USERS: &str = "services/users";
        let eps = vec![
            ep("GET", "/orders/{id}", ORDERS),
            ep("POST", "/charges", ORDERS),
            ep("GET", "/api/v2/tickets", ORDERS),
            ep("GET", "/v1/orders/{id}", USERS),
        ];
        for (method, url, want) in [
            (
                "GET",
                "https://{?}.zendesk.com/api/v2/tickets",
                Bucket::External,
            ),
            (
                "POST",
                "https://api.stripe.com{?}/charges",
                Bucket::External,
            ),
            ("GET", "http://api.{?}/orders/1", Bucket::External),
            ("GET", "http://{?}/{?}", Bucket::Dynamic),
        ] {
            assert_eq!(
                one(&eps, &site(WEB, Some(method), Some((url, Literal)), 1)),
                (want, vec![]),
                "{url}"
            );
        }
        // Host unknown: suffix only, so both services' `…/orders/{}` are candidates and the
        // matched tail decides; never an exact match.
        for url in [
            "http://{?}/orders/1",
            "http://orders-{?}/orders/1",
            "http://localhost{?}/orders/1",
        ] {
            let (bucket, links) = one(&eps, &site(WEB, Some("GET"), Some((url, Literal)), 1));
            assert_eq!(bucket, Bucket::Ambiguous, "{url}");
            assert!(
                links.iter().all(|l| l.contains("\"match\":\"suffix\"")),
                "{url} {links:?}"
            );
        }
        let (bucket, links) = one(
            &eps,
            &site(
                WEB,
                Some("GET"),
                Some(("http://orders:3000{?}/orders/1", Literal)),
                1,
            ),
        );
        assert_eq!(bucket, Bucket::Heuristic);
        assert!(
            links[0].starts_with("HTTP GET /orders/{} @services/orders [heuristic]")
                && links[0].contains("\"match\":\"suffix\""),
            "{links:?}"
        );
    }

    /// Evaluator pass 1, P8: an endpoint registered with a Go `ServeMux` host pattern is matched
    /// only by a client whose host equals it.
    #[test]
    fn host_patterns_restrict_their_endpoints() {
        let mut hosted = ep("GET", "/v2/things", NOTIFICATION);
        hosted.hosts = vec!["api.example.com".into()];
        let mut named = ep("GET", "/hosted", NOTIFICATION);
        named.hosts = vec!["notification".into()];
        let eps = vec![hosted, named];
        for (url, want) in [
            ("http://notification/v2/things", Bucket::Unmatched),
            ("/v2/things", Bucket::Unmatched),
            ("{?}/v2/things", Bucket::Unmatched),
            ("http://notification:3000/hosted", Bucket::Synthetic),
            ("/hosted", Bucket::Unmatched),
        ] {
            assert_eq!(
                one(&eps, &site(AUTH, Some("GET"), Some((url, Literal)), 1)).0,
                want,
                "{url}"
            );
        }
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
        // Gate A counts (amendment 13): one unverified and one verified instance base.
        a.base_verified = Some(false);
        let mut c = site(WEB, Some("POST"), Some(("/internal/mfa/email", Literal)), 3);
        c.base_verified = Some(true);
        let sites = vec![
            a,
            b,
            c,
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
                   "unmatched": 1, "self": 1, "external": 1, "dynamic": 1,
                   "bases": {"verified": 1, "unverified": 1}})
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

    /// The candidates the stitcher examined for every site of `sites`, with the buckets.
    fn stitch_work(
        eps: &[EndpointRef],
        sites: &[Site],
        services: &[ServiceRef],
    ) -> (u64, Vec<Bucket>) {
        rb_frameworks::work::take();
        let stitcher = Stitcher::new(eps, services);
        let buckets = sites.iter().map(|s| stitcher.site(s).bucket).collect();
        (rb_frameworks::work::take(), buckets)
    }

    /// Invariant 11: four times the endpoints and sites with distinct prefixes make at most
    /// five times the candidate confirmations (linear is four; a scan of every endpoint per site
    /// gives sixteen). Counted, not timed, so machine load cannot fail it (contract M4a
    /// amendment 12, Q4).
    #[test]
    fn the_stitcher_grows_linearly() {
        let (e1, s1, v1) = wide(500);
        let (e2, s2, v2) = wide(2000);
        let (small, buckets) = stitch_work(&e1, &s1, &v1);
        assert!(
            buckets.iter().all(|b| *b == Bucket::Heuristic),
            "{buckets:?}"
        );
        let (large, _) = stitch_work(&e2, &s2, &v2);
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
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
        collect_sites(files, go_modules, properties, &[])
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

    /// The sites of `files`, folded as the indexer folds them (see [`folded_sites`]).
    fn collect_sites(
        files: &[(&str, Language, &str)],
        go_modules: &[(&str, &str)],
        properties: &[(&str, &str, &str)],
        overrides: &[(&str, &str)],
    ) -> Vec<Site> {
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
        let overrides: BTreeMap<String, Overrides> = overrides
            .iter()
            .map(|(service, key)| {
                (
                    (*service).to_owned(),
                    Overrides {
                        keys: BTreeSet::from([(*key).to_owned()]),
                        all: *key == "*",
                    },
                )
            })
            .collect();
        let inputs = Inputs {
            files: &by_path,
            index: &index,
            keys: &keys,
            service_of: &service_of,
            properties: &props,
            overrides: &overrides,
        };
        sites(&inputs)
    }

    /// Evaluator pass 1, P1, from source: a port hole written as a template literal, a
    /// concatenation, a Java `+` or `String.format`, and a Go `fmt.Sprintf` keeps the literal
    /// host, so loopback is `self`, an external host `external` and a named host restricts the
    /// candidates; `orders` exposes `GET /orders/{id}`, `users` `GET /users/me`.
    #[test]
    fn port_holes_keep_the_host_in_every_source_form() {
        let ts = "export async function f(port: number) {\n  await fetch(`http://localhost:${port}/orders/1`);\n  await fetch(\"http://localhost:\" + port + \"/orders/1\");\n  await fetch(`https://status.example.com:${port}/orders/1`);\n  await fetch(`http://users:${port}/orders/1`);\n  await fetch(`http://users:${port}/users/me`);\n  await fetch(`http://${host}:${port}/users/me`);\n}\n";
        let java = "package p;\nimport org.springframework.web.client.RestTemplate;\nclass C {\n  RestTemplate rest;\n  void a(int port) {\n    rest.getForObject(\"http://localhost:\" + port + \"/orders/1\", String.class);\n    rest.getForObject(String.format(\"http://localhost:%d/orders/1\", port), String.class);\n    rest.getForObject(String.format(\"http://users:%d/users/me\", port), String.class);\n  }\n}\n";
        let go = "package main\nimport (\n\t\"fmt\"\n\t\"net/http\"\n)\nfunc f(port int) {\n\thttp.Get(fmt.Sprintf(\"http://localhost:%d/orders/1\", port))\n\thttp.Get(fmt.Sprintf(\"https://status.example.com:%d/orders/1\", port))\n\thttp.Get(fmt.Sprintf(\"http://users:%d/users/me\", port))\n}\n";
        let sites = collect_sites(
            &[
                ("web/src/a.ts", Language::TypeScript, ts),
                ("orders/src/main/java/p/C.java", Language::Java, java),
                ("orders/cmd/main.go", Language::Go, go),
            ],
            &[],
            &[],
            &[],
        );
        let eps = vec![
            ep("GET", "/orders/{id}", "orders"),
            ep("GET", "/users/me", "users"),
        ];
        let services: Vec<ServiceRef> = ["web", "orders", "users"]
            .iter()
            .map(|s| ServiceRef {
                key: (*s).to_owned(),
                name: (*s).to_owned(),
            })
            .collect();
        let stitcher = Stitcher::new(&eps, &services);
        let got: Vec<String> = sites
            .iter()
            .map(|s| {
                let o = stitcher.site(s);
                let to = o
                    .links
                    .iter()
                    .map(|l| format!(" -> {}", eps[l.endpoint].key))
                    .collect::<String>();
                format!("{} {} {}{to}", s.at.path, s.at.span[0], o.bucket.as_str())
            })
            .collect();
        assert_eq!(
            got,
            [
                "orders/cmd/main.go 6 self",
                "orders/cmd/main.go 7 external",
                "orders/cmd/main.go 8 synthetic -> HTTP GET /users/me @users",
                "orders/src/main/java/p/C.java 5 self",
                "orders/src/main/java/p/C.java 6 self",
                "orders/src/main/java/p/C.java 7 synthetic -> HTTP GET /users/me @users",
                "web/src/a.ts 1 self",
                "web/src/a.ts 2 self",
                "web/src/a.ts 3 external",
                "web/src/a.ts 4 unmatched",
                "web/src/a.ts 5 synthetic -> HTTP GET /users/me @users",
                "web/src/a.ts 6 heuristic -> HTTP GET /users/me @users",
            ]
        );
    }

    /// The bucket (and targets) of every site of `files`, as `path line bucket [-> key]`, against
    /// `eps` in the services named by `services` (key = name).
    fn stitched(
        files: &[(&str, Language, &str)],
        eps: &[EndpointRef],
        services: &[&str],
    ) -> Vec<String> {
        let sites = collect_sites(files, &[], &[], &[]);
        let services: Vec<ServiceRef> = services
            .iter()
            .map(|s| ServiceRef {
                key: (*s).to_owned(),
                name: (*s).to_owned(),
            })
            .collect();
        let stitcher = Stitcher::new(eps, &services);
        sites
            .iter()
            .map(|s| {
                let o = stitcher.site(s);
                let to = o
                    .links
                    .iter()
                    .map(|l| format!(" -> {} [{}]", eps[l.endpoint].key, l.certainty.as_str()))
                    .collect::<String>();
                format!("{} {} {}{to}", s.at.path, s.at.span[0], o.bucket.as_str())
            })
            .collect()
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q2) from source: a hole next to a literal
    /// host fragment with a dot, as a template literal, a concatenation, Java `String.format`
    /// and `+`, and Go `fmt.Sprintf` and `+`, is external whatever path follows; a bare host
    /// hole is matched by suffix, `heuristic`.
    #[test]
    fn partial_hosts_in_every_source_form() {
        let ts = "export async function f(tenant: string, ver: string, domain: string, h: string) {\n  await fetch(`https://${tenant}.zendesk.com/api/v2/tickets`);\n  await fetch(\"https://\" + tenant + \".zendesk.com/api/v2/tickets\");\n  await fetch(`https://api.stripe.com${ver}/charges`, { method: \"POST\" });\n  await fetch(`http://api.${domain}/orders/1`);\n  await fetch(`http://${h}/orders/1`);\n}\n";
        let java = "package p;\nimport org.springframework.web.client.RestTemplate;\nclass C {\n  RestTemplate rest;\n  void a(String tenant, String ver) {\n    rest.getForObject(String.format(\"https://%s.zendesk.com/api/v2/tickets\", tenant), String.class);\n    rest.postForObject(\"https://api.stripe.com\" + ver + \"/charges\", null, String.class);\n  }\n}\n";
        let go = "package main\nimport (\n\t\"fmt\"\n\t\"net/http\"\n)\nfunc f(t, ver string) {\n\thttp.Get(fmt.Sprintf(\"https://%s.zendesk.com/api/v2/tickets\", t))\n\thttp.Post(\"https://api.stripe.com\"+ver+\"/charges\", \"application/json\", nil)\n}\n";
        let eps = vec![
            ep("GET", "/orders/{id}", "orders"),
            ep("POST", "/charges", "orders"),
            ep("GET", "/api/v2/tickets", "orders"),
        ];
        assert_eq!(
            stitched(
                &[
                    ("web/src/a.ts", Language::TypeScript, ts),
                    ("pay/src/main/java/p/C.java", Language::Java, java),
                    ("pay/cmd/main.go", Language::Go, go),
                ],
                &eps,
                &["web", "pay", "orders"],
            ),
            [
                "pay/cmd/main.go 6 external",
                "pay/cmd/main.go 7 external",
                "pay/src/main/java/p/C.java 5 external",
                "pay/src/main/java/p/C.java 6 external",
                "web/src/a.ts 1 external",
                "web/src/a.ts 2 external",
                "web/src/a.ts 3 external",
                "web/src/a.ts 4 external",
                "web/src/a.ts 5 heuristic -> HTTP GET /orders/{} @orders [heuristic]",
            ]
        );
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q1) from source: a base stated in the
    /// call's request config, or configured on a Spring client instance, decides the host; an
    /// external one is never path-matched, and a call overriding an instance's base never
    /// relies on that instance's base (the evaluator's false `synthetic`).
    #[test]
    fn bases_stated_at_the_call_or_on_the_instance_decide_the_host() {
        let calls = "import axios from \"axios\";\nimport { shared } from \"./http\";\nexport const api = axios.create({ baseURL: \"http://orders\" });\nexport async function f(body: unknown, cfg: object) {\n  await api.get(\"/orders/1\", { baseURL: \"https://api.stripe.com\" });\n  await axios.get(\"/orders/1\", { baseURL: \"https://api.stripe.com\" });\n  await axios.post(\"/charges\", body, { baseURL: \"https://api.stripe.com\" });\n  await api.get(\"/orders/1\");\n  await api.get(\"/orders/1\", cfg);\n  await shared.get(\"/orders/1\", { baseURL: \"https://api.stripe.com\" });\n  await shared.get(\"/orders/1\");\n}\n";
        let http = "import axios from \"axios\";\nexport const shared = axios.create({ baseURL: \"http://orders\" });\n";
        let java = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass C {\n  private final RestTemplate rootRt = new RestTemplateBuilder().rootUri(\"https://api.stripe.com\").build();\n  private final WebClient web;\n  C(WebClient.Builder b) { this.web = b.baseUrl(\"https://api.stripe.com\").build(); }\n  void m() {\n    rootRt.postForObject(\"/charges\", null, String.class);\n    web.get().uri(\"/orders/1\").retrieve();\n  }\n}\n";
        let eps = vec![
            ep("GET", "/orders/{id}", "orders"),
            ep("POST", "/charges", "orders"),
        ];
        assert_eq!(
            stitched(
                &[
                    ("web/src/calls.ts", Language::TypeScript, calls),
                    ("web/src/http.ts", Language::TypeScript, http),
                    ("pay/src/main/java/p/C.java", Language::Java, java),
                ],
                &eps,
                &["web", "pay", "orders"],
            ),
            [
                "pay/src/main/java/p/C.java 9 external",
                "pay/src/main/java/p/C.java 10 external",
                "web/src/calls.ts 4 external",
                "web/src/calls.ts 5 external",
                "web/src/calls.ts 6 external",
                "web/src/calls.ts 7 synthetic -> HTTP GET /orders/{} @orders [synthetic]",
                "web/src/calls.ts 8 heuristic -> HTTP GET /orders/{} @orders [heuristic]",
                "web/src/calls.ts 9 external",
                "web/src/calls.ts 10 synthetic -> HTTP GET /orders/{} @orders [synthetic]",
            ]
        );
    }

    /// `path line bucket [-> key [certainty] base=<base attr>]` per site.
    fn stitched_bases(
        files: &[(&str, Language, &str)],
        eps: &[EndpointRef],
        services: &[&str],
    ) -> Vec<String> {
        let sites = collect_sites(files, &[], &[], &[]);
        let services: Vec<ServiceRef> = services
            .iter()
            .map(|s| ServiceRef {
                key: (*s).to_owned(),
                name: (*s).to_owned(),
            })
            .collect();
        let stitcher = Stitcher::new(eps, &services);
        sites
            .iter()
            .map(|s| {
                let o = stitcher.site(s);
                let to = o
                    .links
                    .iter()
                    .map(|l| {
                        format!(
                            " -> {} [{}] base={} match={}",
                            eps[l.endpoint].key,
                            l.certainty.as_str(),
                            l.attrs["base"].as_str().unwrap_or("?"),
                            l.attrs["match"].as_str().unwrap_or("?"),
                        )
                    })
                    .collect::<String>();
                format!("{} {} {}{to}", s.at.path, s.at.span[0], o.bucket.as_str())
            })
            .collect()
    }

    /// Contract M4a amendment 13, Gate A (evaluator pass 3, R1), from source: an instance whose
    /// base its file (or a file importing it) can change after construction keeps its host but
    /// never reaches `synthetic`, and says `base: unverified`; an unchanged one still does.
    #[test]
    fn a_base_reaches_synthetic_only_when_the_instance_is_unchanged() {
        let calls = "import axios from \"axios\";\nimport { shared2, shared4, shared5 } from \"./client\";\nimport { shared6 } from \"./client2\";\nconst api = axios.create({ baseURL: \"http://orders\" });\nconst api1 = axios.create({ baseURL: \"http://orders\" });\napi1.defaults.baseURL = \"https://api.stripe.com\";\nconst api2 = axios.create({ baseURL: \"http://orders\" });\napi2.interceptors.request.use((c) => { c.baseURL = \"https://api.stripe.com\"; return c; });\nlet api3 = axios.create({ baseURL: \"http://orders\" });\nexport function t03setup() { api3 = axios.create({ baseURL: \"https://api.stripe.com\" }); }\nconst stripeOpts = { baseURL: \"https://api.stripe.com\" };\nconst api4 = axios.create({ baseURL: \"http://orders\", ...stripeOpts });\nexport async function f() {\n  await api.get(\"/orders/1\");\n  await api1.get(\"/orders/1\");\n  await api2.get(\"/orders/1\");\n  await api3.get(\"/orders/1\");\n  await api4.get(\"/orders/1\");\n  await shared2.get(\"/orders/1\");\n  await shared4.get(\"/orders/1\");\n  await shared5.get(\"/orders/1\");\n  await shared6.get(\"/orders/1\");\n}\n";
        // `shared2` is changed by its own module, `shared4` by a module importing it (and its
        // own module calls it too), `shared6` through a namespace import (which reaches every
        // export of `client2`: a namespace member is not told apart).
        let client = "import axios from \"axios\";\nexport const shared2 = axios.create({ baseURL: \"http://orders\" });\nshared2.defaults.baseURL = \"https://api.stripe.com\";\nexport const shared4 = axios.create({ baseURL: \"http://orders\" });\nexport const shared5 = axios.create({ baseURL: \"http://orders\" });\nexport async function own() { await shared4.get(\"/orders/1\"); }\n";
        let client2 = "import axios from \"axios\";\nexport const shared6 = axios.create({ baseURL: \"http://orders\" });\n";
        let setup = "import { shared4 } from \"./client\";\nimport * as c from \"./client2\";\nshared4.interceptors.request.use(rewrite);\nc.shared6.defaults.baseURL = \"https://api.stripe.com\";\n";
        let java = "package p;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass Clients {\n  private final WebClient.Builder b1 = WebClient.builder().baseUrl(\"http://orders\");\n  private final WebClient w3 = WebClient.create(\"http://orders\");\n  Clients() { b1.baseUrl(\"https://api.stripe.com\"); }\n  void j01() { b1.build().get().uri(\"/orders/1\").retrieve(); }\n  void j03() { w3.get().uri(\"/orders/1\").retrieve(); }\n}\n";
        let files = [
            ("web/src/calls.ts", Language::TypeScript, calls),
            ("web/src/client.ts", Language::TypeScript, client),
            ("web/src/client2.ts", Language::TypeScript, client2),
            ("web/src/setup.ts", Language::TypeScript, setup),
            ("pay/src/main/java/p/Clients.java", Language::Java, java),
        ];
        let eps = vec![ep("GET", "/orders/{id}", "orders")];
        let got = stitched_bases(&files, &eps, &["web", "pay", "orders"]);
        let to = "HTTP GET /orders/{} @orders";
        let synthetic = format!("synthetic -> {to} [synthetic] base=literal match=exact");
        let unverified = format!("heuristic -> {to} [heuristic] base=unverified match=exact");
        assert_eq!(
            got,
            [
                format!("pay/src/main/java/p/Clients.java 6 {unverified}"),
                format!("pay/src/main/java/p/Clients.java 7 {synthetic}"),
                format!("web/src/calls.ts 13 {synthetic}"),
                format!("web/src/calls.ts 14 {unverified}"),
                format!("web/src/calls.ts 15 {unverified}"),
                format!("web/src/calls.ts 16 {unverified}"),
                format!("web/src/calls.ts 17 {unverified}"),
                format!("web/src/calls.ts 18 {unverified}"),
                format!("web/src/calls.ts 19 {unverified}"),
                format!("web/src/calls.ts 20 {synthetic}"),
                format!("web/src/calls.ts 21 {unverified}"),
                format!("web/src/client.ts 5 {unverified}"),
            ]
        );
        // Guard: a site whose base is unverified is never `synthetic`.
        let services: Vec<ServiceRef> = ["web", "pay", "orders"]
            .iter()
            .map(|s| ServiceRef {
                key: (*s).to_owned(),
                name: (*s).to_owned(),
            })
            .collect();
        let stitcher = Stitcher::new(&eps, &services);
        let sites = collect_sites(&files, &[], &[], &[]);
        assert_eq!(
            sites
                .iter()
                .filter(|s| s.base_verified == Some(false))
                .count(),
            9
        );
        assert!(
            sites
                .iter()
                .filter(|s| s.base_verified == Some(false))
                .all(|s| stitcher.site(s).bucket != Bucket::Synthetic)
        );
    }

    /// Contract M4a amendment 13, Gate B (evaluator pass 3, R2, R4), from source: a URL whose
    /// path does not start with a literal (a leading hole under a client's base, or a second
    /// hole after a port hole) leaves its prefix unknown: it is matched by suffix only,
    /// `heuristic` at most, and an instance's base is not prepended to it.
    #[test]
    fn a_path_must_start_with_a_literal() {
        let ts = "import axios from \"axios\";\nconst orders = axios.create({ baseURL: \"http://orders\" });\nexport async function l(base: string, id: string, p: string, q: string) {\n  await orders.get(`${process.env.STRIPE_URL}/reports/1`);\n  await orders.get(base + \"/reports/1\");\n  await orders.get(`/${id}/reports/1`);\n  await fetch(`http://orders:${p}${q}/t1/reports/1`);\n  await fetch(`http://orders:${p}/t1/reports/1`);\n}\n";
        let java = "package p;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass C {\n  private final WebClient wc10 = WebClient.create(\"http://orders\");\n  void j19() { wc10.get().uri(System.getenv(\"X\") + \"/reports/1\").retrieve(); }\n}\n";
        let eps = vec![ep("GET", "/{tenant}/reports/{id}", "orders")];
        let to = "HTTP GET /{}/reports/{} @orders";
        assert_eq!(
            stitched_bases(
                &[
                    ("web/src/l.ts", Language::TypeScript, ts),
                    ("pay/src/main/java/p/C.java", Language::Java, java),
                ],
                &eps,
                &["web", "pay", "orders"],
            ),
            [
                format!(
                    "pay/src/main/java/p/C.java 4 heuristic -> {to} [heuristic] base=literal match=suffix"
                ),
                // `fetch` sites first, then axios (pack order within a file).
                format!("web/src/l.ts 6 heuristic -> {to} [heuristic] base=literal match=suffix"),
                format!("web/src/l.ts 7 synthetic -> {to} [synthetic] base=literal match=exact"),
                format!("web/src/l.ts 3 heuristic -> {to} [heuristic] base=literal match=suffix"),
                format!("web/src/l.ts 4 heuristic -> {to} [heuristic] base=literal match=suffix"),
                format!("web/src/l.ts 5 synthetic -> {to} [synthetic] base=literal match=exact"),
            ]
        );
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

    /// Evaluator pass 1, P7: a site whose URL folded a property that unread configuration may
    /// set (a profile file's key, or any key of a config client) is marked, and the mark is per
    /// site: the `@Value` site and the Feign site are each judged by the keys they read.
    #[test]
    fn a_folded_property_set_by_unread_configuration_marks_the_site() {
        let java = "package p;\nimport org.springframework.web.client.RestTemplate;\nclass C {\n  @Value(\"${users.url}\") String users;\n  @Value(\"${orders.url}\") String orders;\n  RestTemplate rest;\n  void a() { rest.getForObject(users + \"/users/me\", String.class); }\n  void b() { rest.getForObject(orders + \"/orders\", String.class); }\n  void c() { rest.getForObject(\"http://users/x\", String.class); }\n}\n";
        let marked = |overrides: &[(&str, &str)]| -> Vec<bool> {
            collect_sites(
                &[("a/src/C.java", Language::Java, java)],
                &[],
                &[
                    ("a", "users.url", "http://users"),
                    ("a", "orders.url", "http://orders"),
                ],
                overrides,
            )
            .iter()
            .map(|s| s.config_overridden)
            .collect()
        };
        assert_eq!(marked(&[]), [false, false, false]);
        assert_eq!(marked(&[("a", "users.url")]), [true, false, false]);
        assert_eq!(marked(&[("a", "*")]), [true, true, false]);
        // Another service's overrides do not mark this one's sites.
        assert_eq!(marked(&[("b", "*")]), [false, false, false]);
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
