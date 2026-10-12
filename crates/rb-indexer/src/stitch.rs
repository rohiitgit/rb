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

use rb_frameworks::clients::{AxiosInstance, ClientKind, FileClients, RawScan, UrlValue};
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
    /// A generated-client call (contract M6f S6): stitched by operation/RPC name, never by
    /// URL. `None` for URL sites.
    pub generated: Option<GeneratedSite>,
    /// The request value reaches an in-process sink (contract M6b S4): never
    /// stitched, whatever the URL folds to.
    pub in_process: bool,
    /// A Spring property the URL folded is also set by a configuration file M4a does not read
    /// (a profile, YAML or bootstrap file), so the folded value may not be the deployed one
    /// (evaluator pass 1, P7).
    pub config_overridden: bool,
    /// The base was read from a `@Bean` method taken as the injected instance (amendment 12).
    pub base_from_bean: bool,
    /// Gate A (amendments 13–15) on what carries the request: `Some(false)` when the carrier
    /// (an instance that fails the token scan, a field, an imported instance, a non-plain link,
    /// a non-plain request config, a module client the repository reconfigures) is not provably
    /// plain, whatever the URL's source: the host still restricts, the site is never
    /// `synthetic`, and its edge says `base: unverified`. Otherwise `Some(true)` for a verified
    /// base read from the carrier's construction, `None` for a plain carrier with no such base.
    pub base_verified: Option<bool>,
    /// The repository configures service discovery or a load balancer, which may bind the
    /// service a host names to any instance: a host-by-name match (F1) is never `synthetic`,
    /// and its edge says `discovery: configured` (contract M4a amendment 17, W4). This is the
    /// narrowed rule (contract M6b S8): raw-file evidence, parsed-file
    /// single-literal keys, or import-resolved known types.
    pub discovery: bool,
    /// The broad rule (contract M5 S19 (a)): parsed-file discovery words and
    /// unresolved Java imports included. Compared against [`Site::discovery`]
    /// to count the sites narrowing lifted (`discovery_lifted`, invariant 6);
    /// never caps.
    pub discovery_broad: bool,
    /// The values a contested base joined to the URL may take, folded (contract M5 S19 (f)):
    /// one naming an external host makes the site external. Empty when the base is decided or
    /// unknown with no stated value, or does not apply (an absolute URL).
    pub base_candidates: Vec<Folded>,
}

/// A generated-client call site (contract M6f S6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GeneratedSite {
    pub kind: rb_frameworks::clients::GeneratedKind,
    /// The operationId or RPC name.
    pub operation: String,
    /// The `*Api` receiver, stub class or service, for scoping.
    pub carrier: String,
}

/// A service as the host rule (D6) sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ServiceRef {
    pub key: String,
    pub name: String,
}

/// One gateway rewrite (contract M6g S3): calls under `prefix` reach `service`.
/// Resolved at build: the target names exactly one service that exposes endpoints,
/// else the rule is dropped (safe direction: the direct form stays).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GatewayRewrite {
    pub prefix: String,
    pub service: String,
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
    /// Why an `unmatched` site is unmatched: a rule that removed its only
    /// path match (`catch_all_only`, contract M4b S20), or an S4 exclusion
    /// (`in_process`, `empty_url`, contract M6b S4).
    detail: Option<&'static str>,
}

impl Outcome {
    fn bucket(bucket: Bucket) -> Self {
        Outcome {
            bucket,
            links: Vec::new(),
            candidates: 0,
            detail: None,
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
    /// Some configuration file of the repository sets a Feign client key
    /// ([`rb_frameworks::clients::feign_config_key`]; contract M4a amendment 16).
    pub feign_configured: bool,
    /// What the raw-text scan of the files without facts found (contract M4a amendment 17).
    pub unparsed: &'i RawScan,
    /// Injection points and their unique beans (contract M4b S21).
    pub points: &'i crate::di::Points,
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

    fn value(&self, folder: &mut fold::Folder<'_>, file: usize, v: &UrlValue) -> Option<Folded> {
        match v {
            UrlValue::Template(t) => Some(folder.fold_template(file, t)),
            UrlValue::Name { text, span } => Some(folder.fold_name(file, text, *span)),
            UrlValue::Unknown | UrlValue::Contested(_) => None,
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
    // The clients some file or configuration key may reconfigure for the whole process: every
    // site of those kinds is capped (amendments 15 and 16).
    let mut hooked: BTreeSet<ClientKind> = inputs
        .files
        .values()
        .flat_map(|(language, facts)| rb_frameworks::clients::global_hooks(*language, facts))
        .collect();
    if inputs.feign_configured {
        hooked.insert(ClientKind::Feign);
    }
    // A file without facts may hook a kind too, or configure discovery (amendment 17), as may
    // Java code naming Spring Cloud LoadBalancer or Ribbon by an import or a fully qualified
    // annotation. Contract M6b S8 (D7) narrows the parsed-file evidence to positively resolved
    // forms — a type use resolving through imports to a discovery package
    // ([`rb_frameworks::clients::discovery_resolved`]) or a fully qualified
    // use — and single-literal JSON keys
    // ([`rb_frameworks::clients::single_literal_discovery`]); raw-text hits
    // stay capped. The broad rule is kept to count the lifted sites.
    hooked.extend(inputs.unparsed.hooks.iter().copied());
    let (broad, discovery) = rb_frameworks::clients::discovery_flags(
        inputs.unparsed,
        inputs
            .files
            .values()
            .map(|(language, facts)| (*language, *facts)),
    );
    let mut out = Vec::new();
    // One folder for the run: per-file indexes, member proofs and clean
    // folds amortize across sites (a fresh step budget per fold, as with a
    // fresh folder per call).
    let mut folder = rb_frameworks::fold::Folder::new(&folding);
    for (&path, file_clients) in &clients {
        let file = folding.ids[path];
        let facts = inputs.files[path].1;
        let service = (inputs.service_of)(path);
        for call in &file_clients.calls {
            folding.take_asked();
            // An injected Spring client: the base of the `@Bean` method its injection point
            // resolves to (contract M4b S21).
            let from_bean = call
                .injected
                .and_then(|bi| injected_base(inputs, &folding, path, facts, bi, call.kind));
            // A base stated at the call (axios request config) replaces the instance's
            // (amendment 12).
            let stated = from_bean
                .clone()
                .or_else(|| call.base.clone().map(|b| (file, b)));
            let mut base_verified = if hooked.contains(&call.kind) {
                Some(false)
            } else {
                call.base_verified
            };
            let base = match &call.instance {
                Some(name) => match imported_instance(inputs, &clients, path, facts, name) {
                    Some((target, instance)) => {
                        // An imported instance is exported, so never a plain carrier (Gate A,
                        // amendments 14 and 15); `verified` says so.
                        if base_verified.is_none() {
                            base_verified = if instance.verified {
                                instance.base.is_some().then_some(true)
                            } else {
                                Some(false)
                            };
                        }
                        stated.or_else(|| instance.base.clone().map(|b| (folding.ids[target], b)))
                    }
                    None => continue,
                },
                None => stated,
            };
            let url = folding.value(&mut folder, file, &call.url);
            let base_candidates = match (&base, &url) {
                (Some((base_file, UrlValue::Contested(values))), Some(url)) if joins_base(url) => {
                    let mut candidates: Vec<Folded> = Vec::new();
                    for v in values {
                        // A spread the pack could not read (contract M6b S5):
                        // its object through the unique declaration, if any.
                        if let UrlValue::Name { text, span } = v {
                            let mut seen = HashSet::new();
                            let mut objects = Vec::new();
                            rb_frameworks::fold::object_values(
                                &folding,
                                *base_file,
                                text,
                                *span,
                                &mut seen,
                                &mut objects,
                            );
                            candidates.extend(
                                objects
                                    .iter()
                                    .filter_map(|o| folding.value(&mut folder, *base_file, o)),
                            );
                        }
                        candidates.extend(folding.value(&mut folder, *base_file, v));
                    }
                    candidates
                }
                _ => Vec::new(),
            };
            let url = url.map(|url| match &base {
                Some((base_file, b)) => join(folding.value(&mut folder, *base_file, b), url),
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
                // A request that never leaves the process (contract M6b
                // S4): unmatched, never an edge.
                in_process: file_clients.in_process.contains(&call.span),
                config_overridden: folding.take_asked(),
                base_from_bean: from_bean.is_some(),
                base_verified,
                discovery,
                discovery_broad: broad,
                base_candidates,
                generated: None,
            });
        }
        for gencall in &file_clients.generated {
            // Generated calls stitch by operation/RPC name (below); they fold nothing.
            let kind = match gencall.kind {
                rb_frameworks::clients::GeneratedKind::OpenApi => ClientKind::OpenApi,
                rb_frameworks::clients::GeneratedKind::Grpc => ClientKind::Grpc,
            };
            out.push(Site {
                src: NodeKey::new(NodeKind::File, path),
                at: Evidence {
                    path: path.to_owned(),
                    span: gencall.span,
                },
                service: service.clone(),
                client: kind,
                method: None,
                url: None,
                generated: Some(GeneratedSite {
                    kind: gencall.kind,
                    operation: gencall.operation.clone(),
                    carrier: gencall.carrier.clone(),
                }),
                in_process: file_clients.in_process.contains(&gencall.span),
                config_overridden: false,
                base_from_bean: false,
                base_verified: None,
                discovery: false,
                discovery_broad: false,
                base_candidates: Vec::new(),
            });
        }
    }
    out
}

/// The base of an injected Spring client bound at `bi` in `path` (contract M4b S21): the
/// injection point of the binding's owner for its type (the one of the binding's name, else
/// the only one), when it resolved to exactly one bean, a `@Bean` method stating a base for
/// `kind`; with that method's file, where the value folds.
fn injected_base(
    inputs: &Inputs<'_, '_>,
    folding: &Folding<'_, '_>,
    path: &str,
    facts: &FileFacts,
    bi: usize,
    kind: ClientKind,
) -> Option<(usize, UrlValue)> {
    let b = &facts.bindings[bi];
    let simple = |t: &str| t.rsplit('.').next().unwrap_or(t).to_owned();
    let points = inputs.points.by_owner.get(&(path.to_owned(), b.scope?))?;
    let typed: Vec<&crate::di::PointRef> = points
        .iter()
        .filter(|p| simple(&p.element) == simple(&b.type_name))
        .collect();
    let point = match typed.iter().find(|p| p.name == b.name) {
        Some(p) => p,
        None => match typed.as_slice() {
            [only] => only,
            _ => return None,
        },
    };
    let (bean_path, method) = inputs.points.unique.get(&(path.to_owned(), point.span))?;
    let (_, bean_facts) = inputs.files.get(bean_path.as_str())?;
    let value = rb_frameworks::clients::bean_base(bean_facts, *method, kind)?;
    Some((*folding.ids.get(bean_path.as_str())?, value))
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

/// Whether a client's base applies to `url` ([`join`]): not when the URL is absolute or starts
/// with an unknown part.
fn joins_base(url: &Folded) -> bool {
    let u = client_url(&url.pieces);
    !(u.host.is_some() || u.host_dynamic || url.pieces.first() == Some(&Piece::Hole))
}

/// A URL relative to a base: the base then the URL (with a `/` between unless one side has it),
/// unless the URL is absolute. An unknown base is a hole. A URL that starts with an unknown
/// part is not joined to any base (amendment 13, Gate B): that part may be absolute (axios and
/// Spring then ignore the base) or a prefix, so it is the unknown base, and the path is matched
/// by suffix only. The label is the weakest among the parts that resolved.
fn join(base: Option<Folded>, url: Folded) -> Folded {
    if !joins_base(&url) {
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

/// Whether a GRPC endpoint's target names `service` with RPC `rpc` (contract M6f S6): the
/// carrier's `Grpc` suffix is the stub class, not the service (`GreeterGrpc` serves `Greeter`),
/// and stub generators re-case the RPC's first letter (Java `sayHello` for `SayHello`), so the
/// method matches with its first letter folded.
fn grpc_target_match(e: &EndpointRef, carrier: &str, rpc: &str) -> bool {
    if e.method != "GRPC" {
        return false;
    }
    let Some((service_part, method)) = e.path.rsplit_once('/') else {
        return false;
    };
    if method.is_empty() || rpc.is_empty() {
        return false;
    }
    if method.len() != rpc.len()
        || method[1..] != rpc[1..]
        || !method[..1].eq_ignore_ascii_case(&rpc[..1])
    {
        return false;
    }
    let service = service_part.rsplit('.').next().unwrap_or(service_part);
    let carrier = carrier.strip_suffix("Grpc").unwrap_or(carrier);
    service == carrier
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
pub(crate) struct Stitcher<'e> {
    /// Counted when scanned, so a scan per site is measured (amendment 13; R6).
    endpoints: Counted<'e, EndpointRef>,
    index: EndpointIndex,
    /// Host form (a service's name, lower-cased, or the last segment of its key) → service keys.
    hosts: HashMap<String, BTreeSet<&'e str>>,
    exposing: HashSet<&'e str>,
    /// Gateway rewrites, longest prefix first (contract M6g S3).
    rewrites: Vec<GatewayRewrite>,
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
            rewrites: Vec::new(),
        }
    }

    fn with_rewrites(
        endpoints: &'e [EndpointRef],
        services: &'e [ServiceRef],
        rewrites: Vec<GatewayRewrite>,
    ) -> Self {
        let mut this = Self::new(endpoints, services);
        let mut rewrites: Vec<GatewayRewrite> = rewrites
            .into_iter()
            .filter(|r| this.exposing.contains(r.service.as_str()))
            .collect();
        // Longest prefix wins; ties keep repository order (stable sort).
        rewrites.sort_by_key(|r| std::cmp::Reverse(r.prefix.len()));
        this.rewrites = rewrites;
        this
    }

    /// Extra host spellings for known services (contract M6g S4): each `(name, service
    /// key)` joins the host→services map. A name joining several services stays ambiguous
    /// (never merged); an unknown service key reads as nothing.
    pub(crate) fn add_host_aliases(
        &mut self,
        services: &'e [ServiceRef],
        aliases: &[(String, String)],
    ) {
        for (name, key) in aliases {
            if let Some(s) = services.iter().find(|s| &s.key == key) {
                self.hosts
                    .entry(name.to_ascii_lowercase())
                    .or_default()
                    .insert(s.key.as_str());
            }
        }
    }

    /// Resolve raw `(prefix, target host)` rules to rewrites: the target names exactly one
    /// service, else the rule is dropped. Pure function of the service list (tested).
    pub(crate) fn resolve_rewrites(
        rules: &[(String, String)],
        services: &[ServiceRef],
    ) -> Vec<GatewayRewrite> {
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
        rules
            .iter()
            .filter_map(
                |(prefix, target)| match hosts.get(&target.to_ascii_lowercase()) {
                    Some(keys) if keys.len() == 1 => Some(GatewayRewrite {
                        prefix: prefix.clone(),
                        service: keys.iter().next().unwrap().to_string(),
                    }),
                    _ => None,
                },
            )
            .collect()
    }

    /// The rewrite stripping this path, if any: longest prefix with a `/` boundary.
    /// `None` when no rule matches (the direct form stays).
    fn rewrite_for(&self, path: &str) -> Option<&GatewayRewrite> {
        self.rewrites.iter().find(|r| {
            let p = r.prefix.as_str();
            path == p
                || path.starts_with(p)
                || (!p.ends_with('/') && path.starts_with(&format!("{p}/")))
        })
    }

    /// The path after stripping the rewrite prefix (`/` when nothing remains).
    fn stripped_path(path: &str, prefix: &str) -> String {
        let rest = path.strip_prefix(prefix).unwrap_or(path);
        let rest = rest.trim_start_matches('/');
        if rest.is_empty() {
            "/".to_owned()
        } else {
            format!("/{rest}")
        }
    }

    /// Whether a folded base names a host that is no service and no loopback, or a host with a
    /// hole next to an FQDN fragment.
    fn external(&self, base: &Folded) -> bool {
        let u = client_url(&base.pieces);
        u.host_external.is_some()
            || u.host
                .as_ref()
                .is_some_and(|h| !LOOPBACK.contains(&h.as_str()) && !self.hosts.contains_key(h))
    }

    /// The bucket and edges of one site (S13, S14).
    fn site(&self, site: &Site) -> Outcome {
        // An in-process request never leaves the process (contract M6b S4):
        // never stitched, whatever the URL folds to.
        if site.in_process {
            return Outcome {
                detail: Some("in_process"),
                ..Outcome::bucket(Bucket::Unmatched)
            };
        }
        // A generated-client call stitches by operation/RPC name (contract M6f S6), never by
        // URL: it folds nothing.
        if let Some(generated) = &site.generated {
            return self.generated(site, generated);
        }
        let Some(folded) = &site.url else {
            return Outcome::bucket(Bucket::Dynamic);
        };
        // An empty URL is never path-matched (contract M6b S4): no literal
        // text and no holes. `""` normalises to `/` below, so this check
        // comes first.
        if folded
            .pieces
            .iter()
            .all(|p| matches!(p, Piece::Lit(l) if l.is_empty()))
        {
            return Outcome {
                detail: Some("empty_url"),
                ..Outcome::bucket(Bucket::Unmatched)
            };
        }
        let url = client_url(&folded.pieces);
        // A host hole next to an FQDN fragment names no service (amendment 12, Q2).
        if url.host_external.is_some() {
            return Outcome::bucket(Bucket::External);
        }
        // A contested base that may name an external host is external (contract M5 S19 (f)):
        // the path is not matched by suffix across services.
        if site.base_candidates.iter().any(|b| self.external(b)) {
            return Outcome {
                detail: Some("base_candidate_external"),
                ..Outcome::bucket(Bucket::External)
            };
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
        let mut client = segments(&url.path);
        let has_hole = folded.pieces.contains(&Piece::Hole);
        if has_hole && !client.iter().any(|s| matches!(s, Segment::Lit(_))) {
            return Outcome::bucket(Bucket::Dynamic);
        }
        // Gateway rewrites (contract M6g S3): when the host does not already pin to an
        // exposing service, a matching prefix strips and scopes to the target. A rewrite
        // yielding no candidate is never applied (the direct form stays).
        let mut via_gateway: Option<String> = None;
        if !matches!(scope, Scope::Only(_))
            && let Some(rw) = self.rewrite_for(&url.path)
        {
            let stripped = Self::stripped_path(&url.path, &rw.prefix);
            let stripped_client = segments(&stripped);
            let probe: Vec<(usize, PathMatch)> = self
                .index
                .candidates(&stripped_client, url.base_unknown)
                .into_iter()
                .filter(|(i, _)| {
                    let e = &self.endpoints[*i];
                    e.service == rw.service.as_str()
                        && methods_match(site.method.as_deref().unwrap_or(ANY), &e.method)
                        && (e.hosts.is_empty()
                            || url.host.as_ref().is_some_and(|h| e.hosts.contains(h)))
                })
                .collect();
            let probe: Vec<(usize, PathMatch)> = probe
                .into_iter()
                .filter(|(i, _)| self.index.segments[*i] != [Segment::Wild])
                .collect();
            if !probe.is_empty() {
                client = stripped_client;
                scope = Scope::Only(rw.service.as_str());
                via_gateway = Some(rw.prefix.clone());
            }
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
        // An endpoint that catches every path (`/{*}`, nothing literal before the wildcard) is a
        // candidate only when the host names its service (contract M4b S20, M4a item 9): a
        // single-page app's fallback would otherwise absorb every unknown call.
        // Only catch-alls are removed, so nothing left means they were the only path matches.
        let named = matches!(scope, Scope::Only(_));
        let found: Vec<(usize, PathMatch)> = found
            .into_iter()
            .filter(|(i, _)| named || self.index.segments[*i] != [Segment::Wild])
            .collect();
        if found.is_empty() {
            return Outcome {
                detail: Some("catch_all_only"),
                ..Outcome::bucket(Bucket::Unmatched)
            };
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
        // Matched by suffix because no host resolved: the base is unknown, whatever the label of
        // the parts that did resolve (contract M5 S18, M4a open item 6). A suffix match whose
        // host resolved (a hole after the port may hold a path prefix) keeps its label.
        let unresolved = m.suffix && url.host.is_none();
        attrs.insert(
            "base".into(),
            if unresolved {
                "unresolved"
            } else if unverified {
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
        if let Some(prefix) = via_gateway {
            attrs.insert("via_gateway".into(), prefix.into());
        }
        // A host naming a service may be bound to other instances by discovery (W4).
        let rebound = site.discovery && !matches!(scope, Scope::Any);
        // ... unless narrowing lifted the cap (contract M6b S8): broad
        // evidence with no positively resolved form.
        let lifted = site.discovery_broad && !site.discovery && !matches!(scope, Scope::Any);
        if rebound {
            attrs.insert("discovery".into(), "configured".into());
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
                detail: None,
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
            && !rebound
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
            detail: lifted.then_some("discovery_lifted"),
        }
    }

    /// A generated-client call's matches (contract M6f S6): endpoints stating its operationId,
    /// or GRPC endpoints whose target names its service and RPC. Sorted by key, so the outcome
    /// never depends on endpoint order.
    fn generated_matches(&self, generated: &GeneratedSite) -> Vec<usize> {
        let mut found: Vec<usize> = match generated.kind {
            rb_frameworks::clients::GeneratedKind::OpenApi => self
                .endpoints
                .iter()
                .enumerate()
                .inspect(|_| rb_frameworks::work::tick(1))
                .filter(|(_, e)| e.operations.iter().any(|o| o == &generated.operation))
                .map(|(i, _)| i)
                .collect(),
            rb_frameworks::clients::GeneratedKind::Grpc => self
                .endpoints
                .iter()
                .enumerate()
                .inspect(|_| rb_frameworks::work::tick(1))
                .filter(|(_, e)| grpc_target_match(e, &generated.carrier, &generated.operation))
                .map(|(i, _)| i)
                .collect(),
        };
        found.sort_by_key(|&i| &self.endpoints[i].key);
        found
    }

    /// Stitches a generated-client call (contract M6f S6): the certainty never exceeds
    /// `synthetic` (closing bar (f)); a name match cannot tell a self call from a foreign one,
    /// so any own-service match makes the site self (M4a invariant 5 — self is never linked).
    fn generated(&self, site: &Site, generated: &GeneratedSite) -> Outcome {
        let found = self.generated_matches(generated);
        if found.is_empty() {
            return Outcome::bucket(Bucket::Unmatched);
        }
        if found
            .iter()
            .any(|&i| self.endpoints[i].service == site.service)
        {
            return Outcome::bucket(Bucket::OwnService);
        }
        let n = found.len();
        if n > MAX_CANDIDATES {
            return Outcome {
                candidates: n,
                ..Outcome::bucket(Bucket::TooAmbiguous)
            };
        }
        let mut attrs = Attrs::new();
        attrs.insert(
            "match".into(),
            match generated.kind {
                rb_frameworks::clients::GeneratedKind::OpenApi => "operation",
                rb_frameworks::clients::GeneratedKind::Grpc => "rpc",
            }
            .into(),
        );
        attrs.insert("client".into(), site.client.as_str().into());
        if n > 1 {
            attrs.insert("candidates".into(), n.into());
            return Outcome {
                bucket: Bucket::Ambiguous,
                links: found
                    .iter()
                    .map(|&i| Link {
                        endpoint: i,
                        certainty: Certainty::Ambiguous,
                        attrs: attrs.clone(),
                    })
                    .collect(),
                candidates: n,
                detail: None,
            };
        }
        let certainty = Certainty::Synthetic.min(self.endpoints[found[0]].certainty);
        Outcome {
            bucket: if certainty == Certainty::Synthetic {
                Bucket::Synthetic
            } else {
                Bucket::Heuristic
            },
            links: vec![Link {
                endpoint: found[0],
                certainty,
                attrs,
            }],
            candidates: 1,
            detail: None,
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
    add_consumes_with(b, sites, endpoints, services, &[], &[])
}

/// Stitches every site with gateway rewrites (contract M6g S3) and host aliases
/// (contract M6g S4).
pub(crate) fn add_consumes_with<'e>(
    b: &mut GraphBuilder,
    sites: &[Site],
    endpoints: &'e [EndpointRef],
    services: &'e [ServiceRef],
    rewrites: &[GatewayRewrite],
    aliases: &[(String, String)],
) -> anyhow::Result<Vec<Diagnostic>> {
    let mut stitcher = Stitcher::with_rewrites(endpoints, services, rewrites.to_vec());
    stitcher.add_host_aliases(services, aliases);
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
    let mut in_process = 0;
    let mut empty_url = 0;
    let mut lifted = 0;
    for site in sites {
        let outcome = stitcher.site(site);
        *counts.entry(outcome.bucket).or_default() += 1;
        match outcome.detail {
            Some("in_process") => in_process += 1,
            Some("empty_url") => empty_url += 1,
            // Contract M6b S8 (invariant 6): a stitched host match the broad
            // rule would have capped but the narrowed rule lifts.
            Some("discovery_lifted") => lifted += 1,
            _ => {}
        }
        // Contract M6b S4 (D4): in-process and empty-URL sites are
        // `unmatched` with their own diagnostic kinds, never silent drops.
        let kind = match outcome.detail {
            Some("in_process") => Some("http_in_process"),
            Some("empty_url") => Some("http_empty_url"),
            _ => outcome.bucket.diagnostic(),
        };
        if let Some(kind) = kind {
            let mut detail = json!({ "client": site.client.as_str() });
            if let Some(generated) = &site.generated {
                detail["operation"] = generated.operation.as_str().into();
                detail["carrier"] = generated.carrier.as_str().into();
            }
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
            if let Some(why) = outcome.detail {
                detail["detail"] = why.into();
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
        // Gate A (amendments 13–15): sites with a verified base read from their carrier's
        // construction, and sites whose carrier is not provably plain (capped).
        let bases = |v: bool| sites.iter().filter(|s| s.base_verified == Some(v)).count();
        detail.insert(
            "bases".into(),
            json!({ "verified": bases(true), "unverified": bases(false) }),
        );
        // Contract M6b S4 (D3, invariant 4): the `unmatched` bucket splits
        // into plain-unmatched and the exclusions, so the buckets still sum
        // to the sites. The split counts ride along only when non-zero, so
        // unrelated dumps stay byte-identical.
        if in_process > 0 || empty_url > 0 {
            let plain =
                detail["unmatched"].as_u64().unwrap_or(0) - in_process as u64 - empty_url as u64;
            detail.insert("unmatched".into(), plain.into());
            if in_process > 0 {
                detail.insert("http_in_process".into(), in_process.into());
            }
            if empty_url > 0 {
                detail.insert("http_empty_url".into(), empty_url.into());
            }
        }
        // Contract M6b S8 (D3, invariant 6): the lifted-site count rides
        // along only when non-zero. Lifted sites already count in their
        // buckets, so the buckets still sum to the sites.
        if lifted > 0 {
            detail.insert("discovery_lifted".into(), lifted.into());
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
    use rb_frameworks::endpoint::{endpoint_key, grpc_key, norm_path};
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
            operations: Vec::new(),
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
            in_process: false,
            config_overridden: false,
            base_from_bean: false,
            base_verified: None,
            discovery: false,
            discovery_broad: false,
            base_candidates: Vec::new(),
            generated: None,
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

    /// `one` with gateway rewrites (contract M6g S3).
    fn one_rewritten(
        endpoints: &[EndpointRef],
        rewrites: Vec<GatewayRewrite>,
        s: &Site,
    ) -> (Bucket, Vec<String>) {
        let services = services();
        let stitcher = Stitcher::with_rewrites(endpoints, &services, rewrites);
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

    /// Contract M6g S3: a gateway rewrite strips its prefix and scopes to the target,
    /// disambiguating two services that expose the same path.
    #[test]
    fn gateway_rewrites_strip_prefix_and_scope_to_the_target() {
        let eps = vec![
            ep("POST", "/v1/login", AUTH),
            ep("POST", "/v1/login", NOTIFICATION),
        ];
        let call = site(
            WEB,
            Some("POST"),
            Some(("http://gateway/api/v1/login", Literal)),
            1,
        );
        // Without the rewrite the full path matches nothing: no false edge.
        assert_eq!(one(&eps, &call).0, Bucket::Unmatched);
        // With `/api/` → auth the edge pins to auth and says so.
        let rw = vec![GatewayRewrite {
            prefix: "/api/".into(),
            service: AUTH.into(),
        }];
        let (bucket, links) = one_rewritten(&eps, rw, &call);
        assert_eq!(bucket, Bucket::Synthetic);
        assert_eq!(links.len(), 1);
        assert!(links[0].contains("@services/auth"), "{links:?}");
        assert!(links[0].contains("via_gateway"), "{links:?}");
    }

    /// `one` with host aliases (contract M6g S4).
    fn one_aliased(
        endpoints: &[EndpointRef],
        aliases: &[(String, String)],
        s: &Site,
    ) -> (Bucket, Vec<String>) {
        let services = services();
        let mut stitcher = Stitcher::new(endpoints, &services);
        stitcher.add_host_aliases(&services, aliases);
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

    /// Contract M6g S4: an alias makes a new host spelling pin to its service; a name
    /// aliasing two services stays ambiguous; an unknown service key reads as nothing.
    #[test]
    fn host_aliases_extend_the_host_rule() {
        let eps = vec![
            ep("POST", "/v1/login", AUTH),
            ep("POST", "/v1/login", NOTIFICATION),
        ];
        // No alias: the invented host names no service.
        let call = site(
            WEB,
            Some("POST"),
            Some(("http://login/v1/login", Literal)),
            1,
        );
        assert_eq!(one(&eps, &call).0, Bucket::External);
        // One alias: the host pins to auth alone (exact path, known method, stated host).
        let auth = vec![("login".to_owned(), AUTH.to_owned())];
        let (bucket, links) = one_aliased(&eps, &auth, &call);
        assert_eq!(bucket, Bucket::Synthetic);
        assert_eq!(links.len(), 1);
        assert!(links[0].contains("@services/auth"), "{links:?}");
        // The same name for two services: the host restricts nothing.
        let both = vec![
            ("login".to_owned(), AUTH.to_owned()),
            ("login".to_owned(), NOTIFICATION.to_owned()),
        ];
        let (bucket, links) = one_aliased(&eps, &both, &call);
        assert_eq!(bucket, Bucket::Ambiguous);
        assert_eq!(links.len(), 2);
        // An unknown service key reads as nothing: still external.
        let unknown = vec![("login".to_owned(), "services/none".to_owned())];
        assert_eq!(one_aliased(&eps, &unknown, &call).0, Bucket::External);
    }
    #[test]
    fn gateway_rewrite_resolution_drops_unknown_and_ambiguous_targets() {
        let services = services();
        let rules = vec![
            ("/api/".to_owned(), "auth".to_owned()),
            ("/nope/".to_owned(), "no-such-service".to_owned()),
            ("/amb/".to_owned(), "billing".to_owned()),
        ];
        let resolved = Stitcher::resolve_rewrites(&rules, &services);
        // `auth` names services/auth alone; the unknown names nothing. `billing` names
        // services/billing/cmd/billing alone (name and last segment agree), so it resolves.
        assert_eq!(resolved.len(), 2);
        assert!(resolved.iter().any(|r| r.prefix == "/api/"));
        // A host naming several services never resolves: a second service whose key ends
        // in `auth` makes `auth` ambiguous (0 rules), as does a shared name.
        let mut dup = services.clone();
        dup.push(ServiceRef {
            key: "other/auth".into(),
            name: "auth-clone".into(),
        });
        let dupe_rules = vec![("/a/".to_owned(), "auth".to_owned())];
        assert!(Stitcher::resolve_rewrites(&dupe_rules, &dup).is_empty());
        let both = vec![
            ServiceRef {
                key: "x".into(),
                name: "same".into(),
            },
            ServiceRef {
                key: "y".into(),
                name: "same".into(),
            },
        ];
        assert!(Stitcher::resolve_rewrites(&dupe_rules, &both).is_empty());
    }

    /// Contract M6g S3: a rewrite yielding no candidate is never applied (direct form stays),
    /// and a rewrite to a service exposing nothing is dropped at build.
    #[test]
    fn gateway_rewrites_without_a_candidate_keep_the_direct_form() {
        let eps = vec![ep("POST", "/direct/path", NOTIFICATION)];
        let direct = site(WEB, Some("POST"), Some(("/direct/path", Literal)), 1);
        let rw = vec![GatewayRewrite {
            prefix: "/api/".into(),
            service: AUTH.into(),
        }];
        // No prefix match: the direct edge lands with no rewrite mark.
        let (bucket, links) = one_rewritten(&eps, rw.clone(), &direct);
        assert_eq!(bucket, Bucket::Heuristic);
        assert!(!links[0].contains("via_gateway"), "{links:?}");
        // Prefix matches but nothing under it is exposed: direct form stays (unmatched here,
        // never a false edge to auth).
        let miss = site(WEB, Some("POST"), Some(("/api/nothing/here", Literal)), 1);
        let (bucket, links) = one_rewritten(&eps, rw, &miss);
        assert_eq!((bucket, links), (Bucket::Unmatched, vec![]));
        // A rewrite to a silent service is dropped at build: longest-prefix order still holds.
        let all = services();
        let silent = Stitcher::with_rewrites(
            &eps,
            &all,
            vec![GatewayRewrite {
                prefix: "/s/".into(),
                service: "services/orders".into(),
            }],
        );
        assert!(silent.rewrites.is_empty());
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
        // Discovery configured in the repository (amendment 17, W4): the host-by-name match
        // says so, its base stays `literal`.
        let rebound = lowered(
            Site {
                discovery: true,
                ..feign.clone()
            },
            &eps,
        );
        assert!(
            rebound.contains("\"base\":\"literal\"")
                && rebound.contains("\"discovery\":\"configured\""),
            "{rebound}"
        );
        // Without a host, discovery changes nothing.
        let path_only = one(
            &eps,
            &Site {
                discovery: true,
                ..site(WEB, Some("POST"), Some(("/api/v1/login", Literal)), 1)
            },
        );
        assert!(!path_only.1[0].contains("discovery"), "{path_only:?}");
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
                "HTTP POST /billing/invoices/{}/pay @services/billing/cmd/billing [heuristic] {\"base\":\"unresolved\",\"client\":\"fetch\",\"match\":\"suffix\"}"
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
        // A hole after `/` is still a whole-segment parameter: only the catch-all matches, and
        // a host-less call reaches no catch-all (contract M4b S20).
        let s = site(WEB, Some("GET"), Some(("/api/items/{?}", Literal)), 1);
        assert_eq!(one(&eps, &s), (Bucket::Unmatched, vec![]));
        assert_eq!(detail(&eps, &s), Some("catch_all_only"));
    }

    /// The site's diagnostic detail, if any.
    fn detail(endpoints: &[EndpointRef], s: &Site) -> Option<&'static str> {
        let services = services();
        Stitcher::new(endpoints, &services).site(s).detail
    }

    /// Contract M4b S20 (M4a item 9): an endpoint `/{*}` is a candidate only for a client whose
    /// host names its service; otherwise a site it alone matches is `unmatched`
    /// (`catch_all_only`). A catch-all under a literal prefix is unaffected.
    #[test]
    fn catch_alls_need_a_named_host() {
        let eps = vec![
            ep("GET", "/*", "services/users"),
            ep("GET", "/assets/*", NOTIFICATION),
            ep("GET", "/api/v1/login", AUTH),
        ];
        let call = |url: &str| site(WEB, Some("GET"), Some((url, Literal)), 1);
        // Evaluator pass 1's `a1`.
        let a1 = call("/totally/unknown/path");
        assert_eq!(one(&eps, &a1), (Bucket::Unmatched, vec![]));
        assert_eq!(detail(&eps, &a1), Some("catch_all_only"));
        let mut b = GraphBuilder::new();
        let diagnostics =
            add_consumes(&mut b, std::slice::from_ref(&a1), &eps, &services()).unwrap();
        let unmatched = diagnostics
            .iter()
            .find(|d| d.kind == "http_unmatched")
            .unwrap();
        assert_eq!(unmatched.detail["detail"], "catch_all_only");
        // Through a proxy host: still no catch-all.
        let proxied = call("http://gateway/totally/unknown/path");
        assert_eq!(one(&eps, &proxied).0, Bucket::Unmatched);
        assert_eq!(detail(&eps, &proxied), Some("catch_all_only"));
        // The host names the catch-all's service.
        let (bucket, links) = one(&eps, &call("http://users/totally/unknown/path"));
        assert_eq!(bucket, Bucket::Heuristic);
        assert!(
            links[0].starts_with("HTTP GET /{*} @services/users "),
            "{links:?}"
        );
        // A literal prefix before `{*}` is a path match like any other.
        let (bucket, links) = one(&eps, &call("/assets/app.js"));
        assert_eq!(bucket, Bucket::Heuristic);
        assert!(links[0].starts_with("HTTP GET /assets/{*} @services/notification "));
        // Another path match next to the catch-all: matched, with no detail.
        let login = call("/api/v1/login");
        assert_eq!(one(&eps, &login).0, Bucket::Heuristic);
        assert_eq!(detail(&eps, &login), None);
        // No path match at all: unmatched without the detail.
        let none = call("http://notification/nothing/here");
        assert_eq!(one(&eps, &none).0, Bucket::Unmatched);
        assert_eq!(detail(&eps, &none), None);
    }

    /// Contract M6b S4: an in-process request is `unmatched` and never
    /// stitched — even with a foreign absolute URL that is otherwise the
    /// `synthetic` row. Its diagnostic kind is its own (`http_in_process`,
    /// D4), not `http_unmatched`.
    #[test]
    fn in_process_requests_are_unmatched() {
        let eps = endpoints();
        let sunk = Site {
            in_process: true,
            ..site(
                AUTH,
                Some("POST"),
                Some(("http://notification:3000/internal/mfa/email", Literal)),
                1,
            )
        };
        assert_eq!(one(&eps, &sunk), (Bucket::Unmatched, vec![]));
        assert_eq!(detail(&eps, &sunk), Some("in_process"));
        let mut b = GraphBuilder::new();
        let diagnostics =
            add_consumes(&mut b, std::slice::from_ref(&sunk), &eps, &services()).unwrap();
        let diagnostic = diagnostics
            .iter()
            .find(|d| d.kind == "http_in_process")
            .unwrap();
        assert_eq!(diagnostic.detail["detail"], "in_process");
        assert!(diagnostics.iter().all(|d| d.kind != "http_unmatched"));
    }

    /// Contract M6b S4: an empty URL is never path-matched — not even to
    /// `/`, which today is what `""` normalises to. Its diagnostic kind is
    /// its own (`http_empty_url`, D4).
    #[test]
    fn empty_urls_are_never_path_matched() {
        let mut eps = endpoints();
        eps.push(ep("GET", "/", NOTIFICATION));
        let empty = site(WEB, Some("GET"), Some(("", Literal)), 1);
        assert_eq!(one(&eps, &empty), (Bucket::Unmatched, vec![]));
        assert_eq!(detail(&eps, &empty), Some("empty_url"));
        // The control: a literal `/` still matches.
        let slash = site(WEB, Some("GET"), Some(("/", Literal)), 2);
        assert_eq!(one(&eps, &slash).0, Bucket::Heuristic);
        assert_eq!(detail(&eps, &slash), None);
    }

    /// Contract M6b S4: a site that is both in-process and empty counts as
    /// in-process — the sink evidence dominates.
    #[test]
    fn in_process_beats_empty_url() {
        let eps = endpoints();
        let sunk = Site {
            in_process: true,
            ..site(WEB, Some("GET"), Some(("", Literal)), 1)
        };
        assert_eq!(one(&eps, &sunk), (Bucket::Unmatched, vec![]));
        assert_eq!(detail(&eps, &sunk), Some("in_process"));
    }

    /// Contract M6b S4, invariant 4 and D3: every exclusion lands in exactly
    /// one counted bucket (`unmatched`), the split counts ride along only
    /// when non-zero, and the buckets still sum to the sites.
    #[test]
    fn s4_exclusions_are_counted_but_only_when_non_zero() {
        let eps = endpoints();
        let sunk = Site {
            in_process: true,
            ..site(
                AUTH,
                Some("POST"),
                Some(("http://notification:3000/internal/mfa/email", Literal)),
                1,
            )
        };
        let empty = site(WEB, Some("GET"), Some(("", Literal)), 2);
        let plain = site(WEB, Some("GET"), Some(("/nowhere", Literal)), 3);
        let sites = vec![sunk, empty, plain];
        let mut b = GraphBuilder::new();
        for e in &eps {
            b.add_node(
                NodeKey::new(NodeKind::Endpoint, e.key.clone()),
                &e.key,
                Attrs::new(),
            )
            .unwrap();
        }
        for s in &sites {
            if !b.has_node(&s.src) {
                b.add_node(s.src.clone(), "f", Attrs::new()).unwrap();
            }
        }
        let diagnostics = add_consumes(&mut b, &sites, &eps, &services()).unwrap();
        let kinds: Vec<&str> = diagnostics.iter().map(|d| d.kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "http_in_process",
                "http_empty_url",
                "http_unmatched",
                "http_coverage"
            ]
        );
        let coverage = diagnostics
            .iter()
            .find(|d| d.kind == "http_coverage")
            .unwrap();
        assert_eq!(
            coverage.detail,
            json!({"sites": 3, "synthetic": 0, "heuristic": 0, "ambiguous": 0, "too_ambiguous": 0,
                   "unmatched": 1, "self": 0, "external": 0, "dynamic": 0,
                   "http_in_process": 1, "http_empty_url": 1,
                   "bases": {"verified": 0, "unverified": 0}})
        );
        // Invariant 4, extended: the buckets plus the S4 split sum to the sites.
        let total: u64 = Bucket::ALL
            .iter()
            .map(|b| coverage.detail[b.as_str()].as_u64().unwrap())
            .sum::<u64>()
            + coverage.detail["http_in_process"].as_u64().unwrap_or(0)
            + coverage.detail["http_empty_url"].as_u64().unwrap_or(0);
        assert_eq!(total, sites.len() as u64);
        // D3: no S4 site, no S4 counter.
        let mut b = GraphBuilder::new();
        let diagnostics = add_consumes(&mut b, &sites[2..], &eps, &services()).unwrap();
        let coverage = diagnostics
            .iter()
            .find(|d| d.kind == "http_coverage")
            .unwrap();
        assert!(coverage.detail.get("http_in_process").is_none());
        assert!(coverage.detail.get("http_empty_url").is_none());
    }

    /// Contract M6b S8 (D7, invariant 6): narrowing lifts the discovery cap
    /// only where no positive evidence reaches — a host-by-name match with
    /// broad evidence but no resolved evidence is `synthetic` again, and each
    /// such site is counted (`discovery_lifted`, non-zero-only). A site the
    /// narrowed rule still reaches stays capped; a host-less suffix match was
    /// never capped, so neither counts.
    #[test]
    fn discovery_narrowing_lifts_and_counts() {
        let eps = endpoints();
        let lifted = Site {
            discovery: false,
            discovery_broad: true,
            ..site(
                AUTH,
                Some("POST"),
                Some(("http://notification:3000/internal/mfa/email", Literal)),
                1,
            )
        };
        // The cap lifts: synthetic again, marked for the count.
        assert_eq!(one(&eps, &lifted).0, Bucket::Synthetic);
        assert_eq!(detail(&eps, &lifted), Some("discovery_lifted"));
        // The control: positive evidence still caps.
        let capped = Site {
            discovery: true,
            ..lifted.clone()
        };
        let (bucket, links) = one(&eps, &capped);
        assert_eq!(bucket, Bucket::Heuristic);
        assert!(
            links[0].contains("\"discovery\":\"configured\""),
            "{links:?}"
        );
        assert_eq!(detail(&eps, &capped), None);
        // A host-less suffix match never rebounds: narrowing changes nothing
        // there and counts nothing.
        let suffix = Site {
            discovery: false,
            discovery_broad: true,
            ..site(
                WEB,
                Some("POST"),
                Some(("{?}/invoices/{?}/pay", Literal)),
                2,
            )
        };
        assert_eq!(one(&eps, &suffix).0, Bucket::Heuristic);
        assert_eq!(detail(&eps, &suffix), None);
        // The count rides along only when non-zero (D3).
        let nodes = |b: &mut GraphBuilder, sites: &[Site]| {
            for e in &eps {
                if !b.has_node(&NodeKey::new(NodeKind::Endpoint, e.key.clone())) {
                    b.add_node(
                        NodeKey::new(NodeKind::Endpoint, e.key.clone()),
                        &e.key,
                        Attrs::new(),
                    )
                    .unwrap();
                }
            }
            for s in sites {
                if !b.has_node(&s.src) {
                    b.add_node(s.src.clone(), "f", Attrs::new()).unwrap();
                }
            }
        };
        let sites = vec![lifted.clone(), capped.clone(), suffix.clone()];
        let mut b = GraphBuilder::new();
        nodes(&mut b, &sites);
        let diagnostics = add_consumes(&mut b, &sites, &eps, &services()).unwrap();
        let coverage = diagnostics
            .iter()
            .find(|d| d.kind == "http_coverage")
            .unwrap();
        assert_eq!(coverage.detail["discovery_lifted"], 1);
        // No lifted site, no counter.
        let rest = vec![capped, suffix];
        let mut b = GraphBuilder::new();
        nodes(&mut b, &rest);
        let diagnostics = add_consumes(&mut b, &rest, &eps, &services()).unwrap();
        let coverage = diagnostics
            .iter()
            .find(|d| d.kind == "http_coverage")
            .unwrap();
        assert!(coverage.detail.get("discovery_lifted").is_none());
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
        // Contract M4b S22 (M4a item 5): a known base and a path of holes only. A client hole
        // matches an endpoint parameter in an exact match, so P2's suffix rule (a literal in
        // the client and in the matched tail) does not apply here: the dynamic check alone
        // stands between `fetch("/" + code)` and another service's `GET /{code}`.
        let params = vec![ep("GET", "/{code}", AUTH), ep("GET", "/{a}/{b}", AUTH)];
        for url in ["/{?}", "/{?}/{?}"] {
            assert_eq!(
                one(&params, &site(WEB, Some("GET"), Some((url, Literal)), 1)),
                (Bucket::Dynamic, vec![]),
                "{url}"
            );
        }
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
        collect_sites_with(
            files,
            go_modules,
            properties,
            overrides,
            &RawScan::default(),
        )
    }

    /// [`collect_sites`] with what the raw-text scan found in the files without facts.
    fn collect_sites_with(
        files: &[(&str, Language, &str)],
        go_modules: &[(&str, &str)],
        properties: &[(&str, &str, &str)],
        overrides: &[(&str, &str)],
        unparsed: &RawScan,
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
        // Spring DI over the same files, every first path segment a deployable service, for
        // the bases of injected clients (contract M4b S21).
        let deployable: BTreeSet<String> = by_path.keys().map(|p| service_of(p)).collect();
        let depends = BTreeMap::new();
        let empty_manifests: BTreeMap<&str, String> = BTreeMap::new();
        let resolution = crate::di::resolve(&crate::di::Inputs {
            files: &by_path,
            keys: &keys,
            index: &index,
            service_of: &service_of,
            deployable: &deployable,
            depends: &depends,
            unparsed,
            manifests: &empty_manifests,
        });
        let inputs = Inputs {
            files: &by_path,
            index: &index,
            keys: &keys,
            service_of: &service_of,
            properties: &props,
            overrides: &overrides,
            feign_configured: properties
                .iter()
                .any(|(_, k, _)| rb_frameworks::clients::feign_config_key(k)),
            unparsed,
            points: &resolution.points,
        };
        sites(&inputs)
    }

    /// Contract M4b S21 (M4a item 15): an injected Spring client takes its base from the
    /// `@Bean` method its injection point resolves to, in any file, when that resolution has
    /// exactly one candidate and the method states a base; the site is marked `base_from_bean`
    /// and is never `synthetic`. Otherwise its base is unknown, as before.
    #[test]
    fn injected_clients_take_the_resolved_beans_base() {
        let cfg = "package com.acme;\nimport org.springframework.context.annotation.Bean;\nimport org.springframework.context.annotation.Configuration;\nimport org.springframework.beans.factory.annotation.Qualifier;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\n@Configuration\npublic class Cfg {\n  @Bean @Qualifier(\"stripe\") RestTemplate stripe(RestTemplateBuilder b) { return b.rootUri(\"https://api.stripe.com\").build(); }\n  @Bean @Qualifier(\"gh\") RestTemplate gh(RestTemplateBuilder b) { return b.rootUri(\"https://api.github.com\").build(); }\n}\n";
        let pay = |param: &str| {
            format!(
                "package com.acme;\nimport org.springframework.stereotype.Service;\nimport org.springframework.beans.factory.annotation.Qualifier;\nimport org.springframework.web.client.RestTemplate;\n@Service\npublic class Pay {{\n  private final RestTemplate rt;\n  Pay({param} RestTemplate rt) {{ this.rt = rt; }}\n  void charge() {{ rt.postForObject(\"/charges\", null, String.class); }}\n}}\n"
            )
        };
        let shown = |pay: &str, cfg: &str| -> Vec<String> {
            collect_sites_with(
                &[
                    ("svc/src/main/java/com/acme/Cfg.java", Language::Java, cfg),
                    ("svc/src/main/java/com/acme/Pay.java", Language::Java, pay),
                ],
                &[],
                &[],
                &[],
                &RawScan::default(),
            )
            .iter()
            .map(|s| {
                format!(
                    "{}{}",
                    s.url.as_ref().map_or("?".to_owned(), render),
                    if s.base_from_bean { " from_bean" } else { "" }
                )
            })
            .collect()
        };
        // Two qualified beans in another file: the point's qualifier picks one.
        assert_eq!(
            shown(&pay("@Qualifier(\"stripe\")"), cfg),
            ["https://api.stripe.com/charges from_bean"]
        );
        // One bean in another file.
        let one = cfg.replace(
            "  @Bean @Qualifier(\"gh\") RestTemplate gh(RestTemplateBuilder b) { return b.rootUri(\"https://api.github.com\").build(); }\n",
            "",
        );
        assert_eq!(
            shown(&pay(""), &one),
            ["https://api.stripe.com/charges from_bean"]
        );
        // Two candidates and no qualifier: ambiguous, so the base is unknown.
        assert_eq!(shown(&pay(""), cfg), ["{?}/charges"]);
        // Two points of the type in the owner: the one named like the field.
        let two = "package com.acme;\nimport org.springframework.stereotype.Service;\nimport org.springframework.beans.factory.annotation.Qualifier;\nimport org.springframework.web.client.RestTemplate;\n@Service\npublic class Pay {\n  private final RestTemplate rt;\n  private final RestTemplate other;\n  Pay(@Qualifier(\"gh\") RestTemplate other, @Qualifier(\"stripe\") RestTemplate rt) { this.rt = rt; this.other = other; }\n  void charge() { rt.postForObject(\"/charges\", null, String.class); }\n}\n";
        assert_eq!(
            shown(two, cfg),
            ["https://api.stripe.com/charges from_bean"]
        );
        // The only point of the type, named otherwise, next to a point of another type.
        let renamed = pay("").replace(
            "Pay( RestTemplate rt) { this.rt = rt; }",
            "Pay(java.time.Clock clock, RestTemplate template) { this.rt = template; }",
        );
        assert!(renamed.contains("RestTemplate template"), "{renamed}");
        assert_eq!(
            shown(&renamed, &one),
            ["https://api.stripe.com/charges from_bean"]
        );
        // A bean method that states no base leaves it unknown too.
        let bare = one.replace(".rootUri(\"https://api.stripe.com\")", "");
        assert_eq!(shown(&pay(""), &bare), ["{?}/charges"]);
    }

    /// Evaluator pass 1, P1, from source: a port hole written as a template literal, a
    /// concatenation, a Java `+` or `String.format`, and a Go `fmt.Sprintf` keeps the literal
    /// host, so loopback is `self`, an external host `external` and a named host restricts the
    /// candidates; `orders` exposes `GET /orders/{id}`, `users` `GET /users/me`.
    #[test]
    fn port_holes_keep_the_host_in_every_source_form() {
        let ts = "export async function f(port: number) {\n  await fetch(`http://localhost:${port}/orders/1`);\n  await fetch(\"http://localhost:\" + port + \"/orders/1\");\n  await fetch(`https://status.example.com:${port}/orders/1`);\n  await fetch(`http://users:${port}/orders/1`);\n  await fetch(`http://users:${port}/users/me`);\n  await fetch(`http://${host}:${port}/users/me`);\n}\n";
        // A method-local `new RestTemplate()` used only for requests: a plain carrier, so the
        // row still tests the host (a field never is, amendment 15).
        let java = "package p;\nimport org.springframework.web.client.RestTemplate;\nclass C {\n  void a(int port) {\n    RestTemplate rest = new RestTemplate();\n    rest.getForObject(\"http://localhost:\" + port + \"/orders/1\", String.class);\n    rest.getForObject(String.format(\"http://localhost:%d/orders/1\", port), String.class);\n    rest.getForObject(String.format(\"http://users:%d/users/me\", port), String.class);\n  }\n}\n";
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

    /// Contract M6f S6: operationId calls stitch by operation name (unique → synthetic, own
    /// service → self, unknown → unmatched, several → ambiguous); RPC calls stitch by
    /// service+method; an own-service match makes the site self; certainty never exceeds
    /// synthetic.
    #[test]
    fn generated_calls_stitch_by_operation_and_rpc_name() {
        let caller = "import { UsersApi } from './gen';\nconst usersApi = new UsersApi();\nexport async function f() {\n  await usersApi.listUsers();\n  await usersApi.missing();\n}\n";
        let selfcaller = "import { UsersApi } from './gen';\nconst usersApi = new UsersApi();\nexport async function g() {\n  await usersApi.listUsers();\n}\n";
        let mut list = ep("GET", "/users", "orders");
        list.operations = vec!["listUsers".into()];
        let eps = vec![list];
        assert_eq!(
            stitched(
                &[
                    ("web/src/a.ts", Language::TypeScript, caller),
                    ("orders/src/b.ts", Language::TypeScript, selfcaller)
                ],
                &eps,
                &["web", "orders"],
            ),
            [
                "orders/src/b.ts 3 self",
                "web/src/a.ts 3 synthetic -> HTTP GET /users @orders [synthetic]",
                "web/src/a.ts 4 unmatched",
            ]
        );
        // Two foreign services sharing the operation: ambiguous; an own-service match makes
        // the site self instead of linking.
        let mut billing = ep("GET", "/users", "billing");
        billing.operations = vec!["listUsers".into()];
        let mut mine = ep("GET", "/users", "web");
        mine.operations = vec!["listUsers".into()];
        assert_eq!(
            stitched(
                &[("web/src/a.ts", Language::TypeScript, caller)],
                std::slice::from_ref(&mine),
                &["web", "orders", "billing"],
            ),
            ["web/src/a.ts 3 self", "web/src/a.ts 4 unmatched"]
        );
        let mut other = ep("GET", "/users", "orders");
        other.operations = vec!["listUsers".into()];
        let eps = vec![billing, other];
        assert_eq!(
            stitched(
                &[("web/src/a.ts", Language::TypeScript, caller)],
                &eps,
                &["web", "orders", "billing"],
            ),
            [
                "web/src/a.ts 3 ambiguous -> HTTP GET /users @billing [ambiguous] -> HTTP GET /users @orders [ambiguous]",
                "web/src/a.ts 4 unmatched"
            ]
        );
    }

    /// Contract M6f S6: gRPC stub calls stitch by service and RPC name.
    #[test]
    fn grpc_calls_stitch_by_service_and_rpc_name() {
        let java = "class C {\n  void f(Object ch, Object req) {\n    GreeterGrpc.newBlockingStub(ch).sayHello(req);\n    GreeterGrpc.newBlockingStub(ch).unknown(req);\n  }\n}\n";
        let greet = EndpointRef {
            key: grpc_key("acme", "Greeter", "SayHello", "orders"),
            method: "GRPC".into(),
            path: "acme.Greeter/SayHello".into(),
            service: "orders".into(),
            prefix_unknown: false,
            certainty: Certainty::Exact,
            hosts: Vec::new(),
            prefix_maybe_incomplete: false,
            operations: Vec::new(),
        };
        assert_eq!(
            stitched(
                &[("web/src/C.java", Language::Java, java)],
                std::slice::from_ref(&greet),
                &["web", "orders"],
            ),
            [
                "web/src/C.java 2 synthetic -> GRPC acme.Greeter/SayHello @orders [synthetic]",
                "web/src/C.java 3 unmatched",
            ]
        );
    }

    /// Contract M6b S7, from source: `scheme://user…hole…@host` matches
    /// within the literal host — here a template hole for the password with
    /// a service host after `@` stitches exactly as the literal URL would.
    #[test]
    fn userinfo_holes_match_within_the_literal_host() {
        let ts = "import axios from \"axios\";\nexport async function f(token: string) {\n  await axios.get(`http://deploy:${token}@orders/orders/1`);\n}\n";
        let eps = vec![ep("GET", "/orders/{id}", "orders")];
        assert_eq!(
            stitched(
                &[("web/src/a.ts", Language::TypeScript, ts)],
                &eps,
                &["web", "orders"],
            ),
            ["web/src/a.ts 2 synthetic -> HTTP GET /orders/{} @orders [synthetic]".to_owned(),]
        );
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
    /// relies on that instance's base (the evaluator's false `synthetic`). Contract M6b S5: a
    /// config passed by name that names nothing readable is dynamic instead.
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
                // Both instances are exported, so their bases are never verified (Gate A,
                // amendment 14): `heuristic`.
                "web/src/calls.ts 7 heuristic -> HTTP GET /orders/{} @orders [heuristic]",
                // A config passed by name that names nothing readable is dynamic
                // (contract M6b S5), never suffix-matched.
                "web/src/calls.ts 8 dynamic",
                "web/src/calls.ts 9 external",
                "web/src/calls.ts 10 heuristic -> HTTP GET /orders/{} @orders [heuristic]",
            ]
        );
    }

    /// `path line bucket [-> key [certainty] base=<base attr>]` per site.
    fn stitched_bases(
        files: &[(&str, Language, &str)],
        eps: &[EndpointRef],
        services: &[&str],
    ) -> Vec<String> {
        stitched_bases_with(files, &[], eps, services)
    }

    /// [`stitched_bases`] with `application.properties` entries `(service, key, value)`.
    fn stitched_bases_with(
        files: &[(&str, Language, &str)],
        properties: &[(&str, &str, &str)],
        eps: &[EndpointRef],
        services: &[&str],
    ) -> Vec<String> {
        stitched_bases_raw(files, properties, &RawScan::default(), eps, services)
    }

    /// [`stitched_bases_with`] with what the raw-text scan found in the files without facts.
    fn stitched_bases_raw(
        files: &[(&str, Language, &str)],
        properties: &[(&str, &str, &str)],
        unparsed: &RawScan,
        eps: &[EndpointRef],
        services: &[&str],
    ) -> Vec<String> {
        let sites = collect_sites_with(files, &[], properties, &[], unparsed);
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

    /// Contract M5 S18 (M4a open item 6): a `CONSUMES` edge matched by suffix because no host
    /// resolved (the client's base is a parameter, or the host itself has a hole) says
    /// `base: unresolved`; one matched by suffix whose host resolved (a hole after the port
    /// may hold a path prefix) keeps its label, as does an exact match.
    #[test]
    fn suffix_edges_without_a_host_say_the_base_is_unresolved() {
        let ts = "export async function f(base: string, h: string, port: number, p: string) {\n  await fetch(`${base}/orders/1`);\n  await fetch(`http://${h}/orders/1`);\n  await fetch(`http://users:${port}${p}/users/me`);\n  await fetch(\"http://users/users/me\");\n}\n";
        let eps = vec![
            ep("GET", "/orders/{id}", "orders"),
            ep("GET", "/users/me", "users"),
        ];
        assert_eq!(
            stitched_bases(
                &[("web/src/a.ts", Language::TypeScript, ts)],
                &eps,
                &["web", "orders", "users"]
            ),
            [
                "web/src/a.ts 1 heuristic -> HTTP GET /orders/{} @orders [heuristic] base=unresolved match=suffix",
                "web/src/a.ts 2 heuristic -> HTTP GET /orders/{} @orders [heuristic] base=unresolved match=suffix",
                "web/src/a.ts 3 heuristic -> HTTP GET /users/me @users [heuristic] base=literal match=suffix",
                "web/src/a.ts 4 synthetic -> HTTP GET /users/me @users [synthetic] base=literal match=exact",
            ]
        );
    }

    /// Contract M4a amendment 14, Gate A (evaluator passes 3 and 4), from source: every
    /// repro of T1–T4 (a use or write before the declaration, in a nested function or a class;
    /// an aliased or default export; a Java field changed through another reference, an inner
    /// class, a method returning `this`, a subclass, a Lombok getter or another file; a
    /// `uriBuilderFactory` next to `baseUrl`) keeps its host but never reaches `synthetic` and
    /// says `base: unverified` (or, when the factory states another host, is `external`). The
    /// controls stay `synthetic`: a method-local `WebClient` used only for requests (the
    /// Store-Microservices form), an unexported module `const` used only for requests, a
    /// function-local instance, and an inline `axios.create(…).get(…)`.
    #[test]
    fn a_base_reaches_synthetic_only_when_the_instance_is_unchanged() {
        let calls = "import axios from \"axios\";\nimport { shared5 } from \"./client\";\nexport function a01setup(url) { a01.defaults.baseURL = url; }\nexport function a02reset() { a02 = axios.create({ baseURL: \"https://api.stripe.com\" }); }\nclass Cfg { static init() { a10.defaults.baseURL = \"https://api.stripe.com\"; } }\nconst api = axios.create({ baseURL: \"http://orders\" });\nconst a01 = axios.create({ baseURL: \"http://orders\" });\nlet a02 = axios.create({ baseURL: \"http://orders\" });\nconst a10 = axios.create({ baseURL: \"http://orders\" });\nconst api1 = axios.create({ baseURL: \"http://orders\" });\napi1.defaults.baseURL = \"https://api.stripe.com\";\nexport async function f() {\n  await api.get(\"/orders/1\");\n  await a01.get(\"/orders/1\");\n  await a02.get(\"/orders/1\");\n  await a10.get(\"/orders/1\");\n  await api1.get(\"/orders/1\");\n  await shared5.get(\"/orders/1\");\n  await axios.create({ baseURL: \"http://orders\" }).get(\"/orders/1\");\n}\nexport function a06() {\n  const setup = () => { a06api.defaults.baseURL = \"https://api.stripe.com\"; };\n  const a06api = axios.create({ baseURL: \"http://orders\" });\n  setup();\n  return a06api.get(\"/orders/1\");\n}\nexport function local() {\n  const c = axios.create({ baseURL: \"http://orders\" });\n  return c.get(\"/orders/1\");\n}\n";
        // An exported instance is never verified, even one used only for requests.
        let client = "import axios from \"axios\";\nexport const shared5 = axios.create({ baseURL: \"http://orders\" });\n";
        let a04 = "import axios from \"axios\";\nconst a04api = axios.create({ baseURL: \"http://orders\" });\nexport { a04api as ordersApi };\nexport function a04() { return a04api.get(\"/orders/1\"); }\n";
        let a04mut = "import { ordersApi } from \"./a04\";\nordersApi.defaults.baseURL = \"https://api.stripe.com\";\n";
        let a05 = "import axios from \"axios\";\nconst a05api = axios.create({ baseURL: \"http://orders\" });\nexport { a05api as default };\nexport function a05() { return a05api.get(\"/orders/1\"); }\n";
        let a05mut = "import client from \"./a05\";\nclient.interceptors.request.use((c) => { c.baseURL = \"https://api.stripe.com\"; return c; });\n";
        let imports = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.web.client.RestClient;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.reactive.function.client.WebClient;\nimport org.springframework.web.util.DefaultUriBuilderFactory;\nimport lombok.Getter;\n";
        let rt = "new RestTemplateBuilder().rootUri(\"http://orders\").build()";
        let stripe = "new DefaultUriBuilderFactory(\"https://api.stripe.com\")";
        let get = "getForObject(\"/orders/1\", String.class)";
        let c1 = format!(
            "{imports}public class C1 {{\n  final RestTemplate rt1 = {rt};\n  private final RestTemplate rt4 = {rt};\n  public String k01() {{ return rt1.{get}; }}\n  public String k04() {{ return rt4.{get}; }}\n  static void tweak(C1 other) {{ other.rt4.setUriTemplateHandler({stripe}); }}\n  class Inner {{ void t() {{ C1.this.rt1.setUriTemplateHandler({stripe}); }} }}\n  C1 self() {{ return this; }}\n}}\nclass Tweak1 {{ static void tweak(C1 c) {{ c.rt1.setUriTemplateHandler({stripe}); c.self().rt4.setUriTemplateHandler({stripe}); }} }}\n"
        );
        let base2 = format!(
            "{imports}public class Base2 {{\n  protected final RestTemplate rt2 = {rt};\n  public String k02() {{ return rt2.{get}; }}\n}}\n"
        );
        let sub2 = format!(
            "{imports}public class Sub2 extends Base2 {{\n  Sub2() {{ rt2.setUriTemplateHandler({stripe}); }}\n}}\n"
        );
        let c3 = format!(
            "{imports}@Getter\npublic class C3 {{\n  private final RestTemplate rt3 = {rt};\n  public String k03() {{ return rt3.{get}; }}\n}}\n"
        );
        let use3 = format!(
            "{imports}class Use3 {{ static void t(C3 c) {{ c.getRt3().setUriTemplateHandler({stripe}); }} }}\n"
        );
        let c7 = format!(
            "{imports}public class C7 {{\n  public RestTemplate rt7 = {rt};\n  public String k07() {{ return rt7.{get}; }}\n}}\n"
        );
        let use7 = format!(
            "{imports}class Use7 {{ static void t(C7 c) {{ c.rt7 = new RestTemplateBuilder().rootUri(\"https://api.stripe.com\").build(); }} }}\n"
        );
        let c5 = format!(
            "{imports}public class C5 {{\n  private final WebClient w5 = WebClient.builder().uriBuilderFactory({stripe}).baseUrl(\"http://orders\").build();\n  private final RestClient r6 = RestClient.builder().uriBuilderFactory({stripe}).baseUrl(\"http://orders\").build();\n  void k05() {{ w5.get().uri(\"/orders/1\").retrieve(); }}\n  void k06() {{ r6.get().uri(\"/orders/1\").retrieve(); }}\n  void k15() {{\n    WebClient w = WebClient.builder().baseUrl(\"http://orders\").uriBuilderFactory(new DefaultUriBuilderFactory(\"http://orders\")).build();\n    w.get().uri(\"/orders/1\").retrieve();\n  }}\n}}\n"
        );
        let store = format!(
            "{imports}public class Store {{\n  void s01() {{\n    WebClient webClient = WebClient.builder().baseUrl(\"http://orders\").build();\n    webClient.get().uri(\"/orders/1\").retrieve();\n  }}\n}}\n"
        );
        let java = |f: &str| format!("pay/src/main/java/p/{f}.java");
        let paths: Vec<String> = [
            "C1", "Base2", "Sub2", "C3", "Use3", "C7", "Use7", "C5", "Store",
        ]
        .iter()
        .map(|f| java(f))
        .collect();
        let sources = [&c1, &base2, &sub2, &c3, &use3, &c7, &use7, &c5, &store];
        let mut files: Vec<(&str, Language, &str)> = vec![
            ("web/src/calls.ts", Language::TypeScript, calls),
            ("web/src/client.ts", Language::TypeScript, client),
            ("web/src/a04.ts", Language::TypeScript, a04),
            ("web/src/a04mut.ts", Language::TypeScript, a04mut),
            ("web/src/a05.ts", Language::TypeScript, a05),
            ("web/src/a05mut.ts", Language::TypeScript, a05mut),
        ];
        for (p, src) in paths.iter().zip(sources) {
            files.push((p.as_str(), Language::Java, src.as_str()));
        }
        let eps = vec![ep("GET", "/orders/{id}", "orders")];
        let got = stitched_bases(&files, &eps, &["web", "pay", "orders"]);
        let to = "HTTP GET /orders/{} @orders";
        let synthetic = format!("synthetic -> {to} [synthetic] base=literal match=exact");
        let unverified = format!("heuristic -> {to} [heuristic] base=unverified match=exact");
        assert_eq!(
            got,
            [
                format!("pay/src/main/java/p/Base2.java 9 {unverified}"),
                format!("pay/src/main/java/p/C1.java 10 {unverified}"),
                format!("pay/src/main/java/p/C1.java 11 {unverified}"),
                format!("pay/src/main/java/p/C3.java 10 {unverified}"),
                "pay/src/main/java/p/C5.java 10 external".to_owned(),
                "pay/src/main/java/p/C5.java 11 external".to_owned(),
                format!("pay/src/main/java/p/C5.java 14 {unverified}"),
                format!("pay/src/main/java/p/C7.java 9 {unverified}"),
                format!("pay/src/main/java/p/Store.java 10 {synthetic}"),
                format!("web/src/a04.ts 3 {unverified}"),
                format!("web/src/a05.ts 3 {unverified}"),
                format!("web/src/calls.ts 12 {synthetic}"),
                format!("web/src/calls.ts 13 {unverified}"),
                format!("web/src/calls.ts 14 {unverified}"),
                format!("web/src/calls.ts 15 {unverified}"),
                format!("web/src/calls.ts 16 {unverified}"),
                format!("web/src/calls.ts 17 {unverified}"),
                format!("web/src/calls.ts 18 {synthetic}"),
                format!("web/src/calls.ts 24 {unverified}"),
                format!("web/src/calls.ts 28 {synthetic}"),
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
        let unverified: Vec<&Site> = sites
            .iter()
            .filter(|s| s.base_verified == Some(false))
            .collect();
        assert_eq!(unverified.len(), 16);
        assert!(
            unverified
                .iter()
                .all(|s| stitcher.site(s).bucket != Bucket::Synthetic)
        );
    }

    /// Contract M4a amendment 15 (evaluator pass 5), from source: every evaluator repro (`q03`,
    /// `q04`, `q05`: a request through `x.mutate()…` with `apply`, a filter or an interceptor;
    /// `p17`, `p18`: an axios instance with an interceptor, with and without a base; `p09`: an
    /// `adapter`; `q09b`, `q12`, `q13`, `q14`: a filter or interceptor on an inline instance, a
    /// local, a field) keeps its host but is never `synthetic`, whatever the URL's source;
    /// a bare module client is capped when another file of the repository reconfigures the
    /// module (`globalThis.fetch = …`, `axios.interceptors…`, `http.DefaultTransport = …`).
    /// The controls stay `synthetic`: an unexported module `const` (`t00`), a function local, an
    /// inline `axios.create(…).get(…)`, a Store-style local `WebClient`, a Feign interface, and
    /// bare `fetch`, `axios` and Go `http` calls in a repository that does not reconfigure them.
    #[test]
    fn every_carrier_is_gated_whatever_the_url() {
        let imports = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.web.client.RestClient;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.reactive.function.client.WebClient;\nimport org.springframework.web.reactive.function.client.ExchangeFilterFunction;\n";
        let q = format!(
            "{imports}class Q {{\n  static final ExchangeFilterFunction REWRITE = ExchangeFilterFunction.ofRequestProcessor(rq -> Mono.just(ClientRequest.from(rq).url(URI.create(\"https://api.stripe.com/v1\")).build()));\n  private final WebClient q13 = WebClient.builder().filter(REWRITE).build();\n  private final RestTemplate q14 = new RestTemplateBuilder().additionalInterceptors(i).build();\n  void q03() {{\n    WebClient w = WebClient.create(\"http://orders\");\n    w.mutate().apply(b -> b.baseUrl(\"https://api.stripe.com\")).build().get().uri(\"/orders/1\").retrieve();\n  }}\n  void q04() {{\n    WebClient w = WebClient.create(\"http://orders\");\n    w.mutate().filter(REWRITE).build().get().uri(\"/orders/1\").retrieve();\n  }}\n  void q05() {{\n    RestClient r = RestClient.create(\"http://orders\");\n    r.mutate().requestInterceptor(i).build().get().uri(\"/orders/1\").retrieve();\n  }}\n  void q09b() {{ WebClient.builder().filter(REWRITE).build().get().uri(\"http://orders/orders/1\").retrieve(); }}\n  void q12() {{\n    WebClient w = WebClient.builder().filter(REWRITE).build();\n    w.get().uri(\"http://orders/orders/1\").retrieve();\n  }}\n  void q13() {{ q13.get().uri(\"http://orders/orders/1\").retrieve(); }}\n  void q14() {{ q14.getForObject(\"http://orders/orders/1\", String.class); }}\n}}\n"
        );
        let p = "import axios from \"axios\";\nconst p17api = axios.create({ timeout: 5 });\np17api.interceptors.request.use((c) => { c.url = \"https://api.stripe.com/v1/charges\"; return c; });\nconst p18api = axios.create({ baseURL: \"http://orders\" });\np18api.interceptors.request.use((c) => { c.url = \"https://api.stripe.com/v1/charges\"; return c; });\nexport function p17() { return p17api.get(\"http://orders/orders/1\"); }\nexport function p18() { return p18api.get(\"http://orders/orders/1\"); }\nexport function p09() {\n  const a = axios.create({ baseURL: \"http://orders\", adapter: myAdapter });\n  return a.get(\"/orders/1\");\n}\nexport function bare() {\n  fetch(\"http://orders/orders/1\");\n  axios.get(\"http://orders/orders/1\");\n  axios(\"http://orders/orders/1\");\n}\n";
        let hook_fetch =
            "globalThis.fetch = (u: string) => realFetch(\"https://api.stripe.com/v1\");\n";
        let hook_axios = "import axios from \"axios\";\naxios.interceptors.request.use((c) => { c.url = \"https://api.stripe.com/v1\"; return c; });\n";
        let go = "package main\nimport \"net/http\"\nfunc bare() {\n\thttp.Get(\"http://orders/orders/1\")\n}\n";
        let hook_go = "package main\nimport \"net/http\"\nfunc init() {\n\thttp.DefaultTransport = rewriting{}\n}\n";
        let eps = vec![ep("GET", "/orders/{id}", "orders")];
        let to = "HTTP GET /orders/{} @orders";
        let unverified = format!("heuristic -> {to} [heuristic] base=unverified match=exact");
        let synthetic = format!("synthetic -> {to} [synthetic] base=literal match=exact");
        let services = ["web", "pay", "orders"];
        let tainted = [
            ("pay/src/main/java/p/Q.java", Language::Java, q.as_str()),
            ("web/src/p.ts", Language::TypeScript, p),
            ("web/src/hook_fetch.ts", Language::TypeScript, hook_fetch),
            ("web/src/hook_axios.ts", Language::TypeScript, hook_axios),
            ("pay/cmd/main.go", Language::Go, go),
            ("pay/cmd/hook.go", Language::Go, hook_go),
        ];
        assert_eq!(
            stitched_bases(&tainted, &eps, &services),
            [
                format!("pay/cmd/main.go 3 {unverified}"),
                // `q14` (a `RestTemplate` field), then `q03`, `q04`, `q05`, `q09b`, `q12`, `q13`.
                format!("pay/src/main/java/p/Q.java 28 {unverified}"),
                format!("pay/src/main/java/p/Q.java 12 {unverified}"),
                format!("pay/src/main/java/p/Q.java 16 {unverified}"),
                format!("pay/src/main/java/p/Q.java 20 {unverified}"),
                format!("pay/src/main/java/p/Q.java 22 {unverified}"),
                format!("pay/src/main/java/p/Q.java 25 {unverified}"),
                format!("pay/src/main/java/p/Q.java 27 {unverified}"),
                // Bare `fetch` (hooked), then `p17`, `p18`, `p09` and bare `axios` (hooked).
                format!("web/src/p.ts 12 {unverified}"),
                format!("web/src/p.ts 5 {unverified}"),
                format!("web/src/p.ts 6 {unverified}"),
                format!("web/src/p.ts 9 {unverified}"),
                format!("web/src/p.ts 13 {unverified}"),
                format!("web/src/p.ts 14 {unverified}"),
            ]
        );
        // Guard: no site of the tainted repository is `synthetic`.
        let services_ref: Vec<ServiceRef> = services
            .iter()
            .map(|s| ServiceRef {
                key: (*s).to_owned(),
                name: (*s).to_owned(),
            })
            .collect();
        let stitcher = Stitcher::new(&eps, &services_ref);
        assert!(
            collect_sites(&tainted, &[], &[], &[])
                .iter()
                .all(|s| stitcher.site(s).bucket != Bucket::Synthetic)
        );
        // The controls, in a repository that reconfigures no module client.
        let t = "import axios from \"axios\";\nconst t00 = axios.create({ baseURL: \"http://orders\" });\nexport function f() { return t00.get(\"/orders/1\"); }\nexport function fl() {\n  const c = axios.create({ baseURL: \"http://orders\" });\n  return c.get(\"/orders/1\");\n}\nexport function inl() { return axios.create({ baseURL: \"http://orders\" }).get(\"/orders/1\"); }\nexport function bare() {\n  fetch(\"http://orders/orders/1\");\n  axios.get(\"http://orders/orders/1\");\n}\n";
        let store = format!(
            "{imports}public class Store {{\n  void s01() {{\n    WebClient webClient = WebClient.builder().baseUrl(\"http://orders\").build();\n    webClient.get().uri(\"/orders/1\").retrieve();\n  }}\n}}\n"
        );
        let feign = "package p;\nimport org.springframework.cloud.openfeign.FeignClient;\nimport org.springframework.web.bind.annotation.GetMapping;\n@FeignClient(name = \"orders\", url = \"http://orders\")\npublic interface OrdersClient {\n  @GetMapping(\"/orders/1\")\n  String one();\n}\n";
        assert_eq!(
            stitched_bases(
                &[
                    (
                        "pay/src/main/java/p/Store.java",
                        Language::Java,
                        store.as_str()
                    ),
                    (
                        "pay/src/main/java/p/OrdersClient.java",
                        Language::Java,
                        feign
                    ),
                    ("pay/cmd/main.go", Language::Go, go),
                    ("web/src/t.ts", Language::TypeScript, t),
                ],
                &eps,
                &services,
            ),
            [
                format!("pay/cmd/main.go 3 {synthetic}"),
                format!("pay/src/main/java/p/OrdersClient.java 5 {synthetic}"),
                format!("pay/src/main/java/p/Store.java 9 {synthetic}"),
                format!("web/src/t.ts 9 {synthetic}"),
                format!("web/src/t.ts 2 {synthetic}"),
                format!("web/src/t.ts 5 {synthetic}"),
                format!("web/src/t.ts 7 {synthetic}"),
                format!("web/src/t.ts 10 {synthetic}"),
            ]
        );
    }

    /// Contract M4a amendment 16 (evaluator pass 6, V1–V5), from source: a carrier is plain only
    /// when every token naming its configuration surface, anywhere in the repository, is in an
    /// allow-listed role. Every repro of the report keeps its host but never reaches
    /// `synthetic`: Spring clients built from another client, a customizer, a constructor
    /// argument or with a class body (`j1`–`j5`, `j10`); Feign under a `configuration =`
    /// class, a global `RequestInterceptor` bean, or `feign.client.config` keys; `fetch`
    /// replaced through a cast or a bare assignment; axios instances under an
    /// `axios.defaults…` hook, and a hook through a function-local `require`; a Go dot import.
    /// The controls stay `synthetic`.
    #[test]
    fn carriers_are_plain_only_by_allow_listed_roles() {
        let eps = vec![ep("GET", "/orders/{id}", "orders")];
        let services = ["web", "pay", "orders"];
        let to = "HTTP GET /orders/{} @orders";
        let unverified = format!("heuristic -> {to} [heuristic] base=unverified match=exact");
        let synthetic = format!("synthetic -> {to} [synthetic] base=literal match=exact");
        let imports = "package p;\nimport java.util.List;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.web.client.RestClient;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.reactive.function.client.WebClient;\n";
        let j = format!(
            "{imports}class J {{\n  void j1() {{\n    RestTemplate rt = new RestTemplate();\n    rt.getInterceptors().add(rewrite);\n    RestClient.create(rt).get().uri(\"http://orders/orders/1\").retrieve();\n  }}\n  void j2(RestTemplate injected) {{ RestClient.builder(injected).build().get().uri(\"http://orders/orders/1\").retrieve(); }}\n  void j5(RestTemplate injected) {{\n    RestClient c = RestClient.create(injected);\n    c.get().uri(\"http://orders/orders/1\").retrieve();\n  }}\n  void j3(RestTemplateCustomizer cust) {{\n    RestTemplate rt = new RestTemplateBuilder(cust).build();\n    rt.getForObject(\"http://orders/orders/1\", String.class);\n  }}\n  void j4() {{ new RestTemplate(new InterceptingClientHttpRequestFactory(new SimpleClientHttpRequestFactory(), List.of(rewrite))).getForObject(\"http://orders/orders/1\", String.class); }}\n  void j10() {{\n    RestTemplate rt = new org.springframework.web.client.RestTemplate() {{ }};\n    rt.getForObject(\"http://orders/orders/1\", String.class);\n  }}\n  void j10b() {{ new RestTemplate() {{ }}.getForObject(\"http://orders/orders/1\", String.class); }}\n}}\n"
        );
        assert_eq!(
            stitched_bases(
                &[("pay/src/main/java/p/J.java", Language::Java, j.as_str())],
                &eps,
                &services
            ),
            [
                // `RestTemplate` sites (`j3`, `j4`, `j10`, `j10b`), then `RestClient` (`j1`,
                // `j2`, `j5`).
                format!("pay/src/main/java/p/J.java 19 {unverified}"),
                format!("pay/src/main/java/p/J.java 21 {unverified}"),
                format!("pay/src/main/java/p/J.java 24 {unverified}"),
                format!("pay/src/main/java/p/J.java 26 {unverified}"),
                format!("pay/src/main/java/p/J.java 10 {unverified}"),
                format!("pay/src/main/java/p/J.java 12 {unverified}"),
                format!("pay/src/main/java/p/J.java 15 {unverified}"),
            ]
        );
        // The Java controls: fresh constructions with no argument or a URL argument (a string,
        // a constant of the file, a `String` field), followed by plain links.
        let ok = format!(
            "{imports}class K {{\n  static final String ORDERS = \"http://orders\";\n  void k() {{\n    RestTemplate rt = new RestTemplate();\n    rt.getForObject(\"http://orders/orders/1\", String.class);\n    new RestTemplate().getForObject(\"http://orders/orders/1\", String.class);\n    WebClient w = WebClient.create(ORDERS);\n    w.get().uri(\"/orders/1\").retrieve();\n    RestClient.create(\"http://orders\").get().uri(\"/orders/1\").retrieve();\n    RestClient.create().get().uri(\"http://orders/orders/1\").retrieve();\n    WebClient s01 = WebClient.builder().baseUrl(\"http://orders\").build();\n    s01.get().uri(\"/orders/1\").retrieve();\n  }}\n}}\n"
        );
        assert_eq!(
            stitched_bases(
                &[("pay/src/main/java/p/K.java", Language::Java, ok.as_str())],
                &eps,
                &services
            ),
            [10, 11, 13, 14, 15, 17].map(|row| {
                let base = if row == 13 { "constant" } else { "literal" };
                format!(
                    "pay/src/main/java/p/K.java {row} {}",
                    synthetic.replace("literal", base)
                )
            })
        );
        // V2: a Feign interface is plain only while the repository has no `RequestInterceptor`
        // token, no `configuration =` on a `@FeignClient`, and no Feign client config keys.
        let feign = |extra: &str| {
            format!(
                "package p;\nimport org.springframework.cloud.openfeign.FeignClient;\nimport org.springframework.web.bind.annotation.GetMapping;\n@FeignClient(name = \"orders\", url = \"http://orders\"{extra})\npublic interface OrdersFeign {{\n  @GetMapping(\"/orders/1\")\n  String one();\n}}\n"
            )
        };
        let plain = feign("");
        let configured = feign(", configuration = RewriteConfig.class");
        let interceptor = "package p;\nimport feign.RequestInterceptor;\nimport org.springframework.context.annotation.Bean;\nimport org.springframework.context.annotation.Configuration;\n@Configuration\nclass Global {\n  @Bean RequestInterceptor rewrite() { return t -> t.target(\"https://api.stripe.com\"); }\n}\n";
        let transport = "package p;\nimport feign.Client;\nimport org.springframework.context.annotation.Bean;\n@Configuration\nclass Transport {\n  @Bean Client client() { return new Client.Default(null, null); }\n}\n";
        let path = "pay/src/main/java/p/OrdersFeign.java";
        let feign_site = |verdict: &str| vec![format!("{path} 5 {verdict}")];
        let no_props: &[(&str, &str, &str)] = &[];
        let config_key: &[(&str, &str, &str)] = &[(
            "pay",
            "feign.client.config.default.request-interceptors[0]",
            "p.Rewrite",
        )];
        for (files, props, verdict) in [
            (vec![(path, plain.as_str())], no_props, &synthetic),
            (vec![(path, configured.as_str())], no_props, &unverified),
            (
                vec![
                    (path, plain.as_str()),
                    ("pay/src/main/java/p/Global.java", interceptor),
                ],
                no_props,
                &unverified,
            ),
            (vec![(path, plain.as_str())], config_key, &unverified),
            (
                vec![
                    (path, plain.as_str()),
                    ("pay/src/main/java/p/Transport.java", transport),
                ],
                no_props,
                &unverified,
            ),
        ] {
            let files: Vec<(&str, Language, &str)> = files
                .into_iter()
                .map(|(p, src)| (p, Language::Java, src))
                .collect();
            assert_eq!(
                stitched_bases_with(&files, props, &eps, &services),
                feign_site(verdict),
                "{files:?} {props:?}"
            );
        }
        // V3, V4: one hook file next to the TS controls; the hooked client's sites are capped
        // (instances too, for axios), the other client's stay `synthetic`.
        let t = "import axios from \"axios\";\nconst t00 = axios.create({ baseURL: \"http://orders\" });\nexport function f() { return t00.get(\"/orders/1\"); }\nexport function fl() {\n  const c = axios.create({ baseURL: \"http://orders\" });\n  return c.get(\"/orders/1\");\n}\nexport function inl() { return axios.create({ baseURL: \"http://orders\" }).get(\"/orders/1\"); }\nexport function bare() {\n  fetch(\"http://orders/orders/1\");\n  axios.get(\"http://orders/orders/1\");\n}\n";
        let ts_sites = |fetch: &str, axios: &str| {
            [
                format!("web/src/t.ts 9 {fetch}"),
                format!("web/src/t.ts 2 {axios}"),
                format!("web/src/t.ts 5 {axios}"),
                format!("web/src/t.ts 7 {axios}"),
                format!("web/src/t.ts 10 {axios}"),
            ]
        };
        assert_eq!(
            stitched_bases(
                &[("web/src/t.ts", Language::TypeScript, t)],
                &eps,
                &services
            ),
            ts_sites(&synthetic, &synthetic)
        );
        for (hook, fetch, axios) in [
            (
                "(globalThis as any).fetch = (u: string) => realFetch(\"https://api.stripe.com/v1\");\n",
                &unverified,
                &synthetic,
            ),
            (
                "(window as any).fetch = wrapped;\n",
                &unverified,
                &synthetic,
            ),
            (
                "(global as any).fetch = jest.fn();\n",
                &unverified,
                &synthetic,
            ),
            (
                "fetch = ((orig) => (u: any, i?: any) => orig(\"https://api.stripe.com/v1\", i))(fetch);\n",
                &unverified,
                &synthetic,
            ),
            (
                "import axios from \"axios\";\naxios.defaults.adapter = reroutingAdapter;\n",
                &synthetic,
                &unverified,
            ),
            (
                "export function setup() {\n  const axios = require(\"axios\");\n  axios.interceptors.request.use((c) => c);\n}\n",
                &synthetic,
                &unverified,
            ),
        ] {
            assert_eq!(
                stitched_bases(
                    &[
                        ("web/src/hook.ts", Language::TypeScript, hook),
                        ("web/src/t.ts", Language::TypeScript, t),
                    ],
                    &eps,
                    &services
                ),
                ts_sites(fetch, axios),
                "{hook}"
            );
        }
        // V5: a dot import of `net/http` caps every bare Go call.
        let go = "package main\nimport \"net/http\"\nfunc bare() {\n\thttp.Get(\"http://orders/orders/1\")\n}\n";
        let dot = "package main\nimport . \"net/http\"\nfunc init() {\n\tDefaultTransport = rewriting{}\n}\n";
        for (files, verdict) in [
            (vec![("pay/cmd/main.go", go)], &synthetic),
            (
                vec![("pay/cmd/hook.go", dot), ("pay/cmd/main.go", go)],
                &unverified,
            ),
        ] {
            let files: Vec<(&str, Language, &str)> = files
                .into_iter()
                .map(|(p, src)| (p, Language::Go, src))
                .collect();
            assert_eq!(
                stitched_bases(&files, &eps, &services),
                [format!("pay/cmd/main.go 3 {verdict}")]
            );
        }
    }

    /// Contract M4a amendment 17 (evaluator pass 7, W1, W2, W4), from source: a hook in a file
    /// without token facts — a Kotlin or Groovy `RequestInterceptor` bean, a Vue component's
    /// `axios.defaults…`, an HTML page's `window.fetch = …`, a generated Go or TS file, a file
    /// over the scan's byte cap — caps its client kind through the raw-text scan, prose and
    /// plain dependency entries do not; a service-discovery key caps every host-by-name
    /// `synthetic` stitch, URL-less Feign included, with a plain carrier (`base` unchanged).
    #[test]
    fn unparsed_files_and_discovery_cap_carriers() {
        let eps = vec![ep("GET", "/orders/{id}", "orders")];
        let services = ["web", "pay", "orders", "gocli"];
        let to = "HTTP GET /orders/{} @orders";
        let unverified = format!("heuristic -> {to} [heuristic] base=unverified match=exact");
        let synthetic = format!("synthetic -> {to} [synthetic] base=literal match=exact");
        let discovered = format!("heuristic -> {to} [heuristic] base=literal match=exact");
        let t = "import axios from \"axios\";\nconst t00 = axios.create({ baseURL: \"http://orders\" });\nexport function f() { return t00.get(\"/orders/1\"); }\nexport function bare() {\n  fetch(\"http://orders/orders/1\");\n  axios.get(\"http://orders/orders/1\");\n}\n";
        // A Feign client with no `url`: the service name is its host (F1).
        let feign = "package p;\nimport org.springframework.cloud.openfeign.FeignClient;\nimport org.springframework.web.bind.annotation.GetMapping;\n@FeignClient(name = \"orders\")\npublic interface OrdersFeign {\n  @GetMapping(\"/orders/1\")\n  String one();\n}\n";
        let go = "package main\nimport \"net/http\"\nfunc bare() {\n\thttp.Get(\"http://orders/orders/1\")\n}\n";
        let files = [
            ("gocli/main.go", Language::Go, go),
            (
                "pay/src/main/java/p/OrdersFeign.java",
                Language::Java,
                feign,
            ),
            ("web/src/t.ts", Language::TypeScript, t),
        ];
        // Verdicts of the Go, Feign, `fetch` and axios sites.
        let expect = |go: &str, feign: &str, fetch: &str, axios: &str| {
            vec![
                format!("gocli/main.go 3 {go}"),
                format!("pay/src/main/java/p/OrdersFeign.java 5 {feign}"),
                format!("web/src/t.ts 4 {fetch}"),
                format!("web/src/t.ts 2 {axios}"),
                format!("web/src/t.ts 5 {axios}"),
            ]
        };
        let (s, u, d) = (synthetic.as_str(), unverified.as_str(), discovered.as_str());
        let kotlin = "package p\nimport feign.RequestInterceptor\n@Configuration\nclass Global {\n  @Bean fun rewrite(): RequestInterceptor = RequestInterceptor { t -> t.target(\"https://api.stripe.com\") }\n}\n";
        let groovy = "package p\n@Configuration\nclass Global {\n  @Bean RequestInterceptor rewrite() { { t -> t.target('https://api.stripe.com') } as RequestInterceptor }\n}\n";
        let vue = "<template><div/></template>\n<script lang=\"ts\">\nimport axios from \"axios\";\naxios.defaults.adapter = reroutingAdapter;\n</script>\n";
        let html = "<html><script>\nconst realFetch = window.fetch;\nwindow.fetch = (u, i) => realFetch(\"https://api.stripe.com/v1\", i);\n</script></html>\n";
        let go_generated = "// Code generated by mockgen. DO NOT EDIT.\npackage main\nimport \"net/http\"\nfunc init() { http.DefaultTransport = nil }\n";
        let ts_generated = "/** @generated */\nimport axios from \"axios\";\naxios.defaults.adapter = reroutingAdapter;\n";
        let yml = "spring:\n  cloud:\n    discovery:\n      client:\n        simple:\n          instances:\n            orders:\n              - uri: https://api.stripe.com\n";
        // (path, text or `None` over the byte cap) of the files without facts.
        type Raw<'r> = Vec<(&'r str, Option<&'r str>)>;
        let rows: Vec<(Raw<'_>, Vec<String>)> = vec![
            (vec![], expect(s, s, s, s)),
            // W1: languages with no grammar.
            (
                vec![("pay/src/main/kotlin/p/Global.kt", Some(kotlin))],
                expect(s, u, s, s),
            ),
            (
                vec![("pay/src/main/groovy/p/Global.groovy", Some(groovy))],
                expect(s, u, s, s),
            ),
            (vec![("web/src/App.vue", Some(vue))], expect(s, s, s, u)),
            (
                vec![("web/public/index.html", Some(html))],
                expect(s, s, u, s),
            ),
            // W2: generated and oversized files.
            (
                vec![("gocli/zz_hook.go", Some(go_generated))],
                expect(u, s, s, s),
            ),
            (
                vec![("web/src/hook.ts", Some(ts_generated))],
                expect(s, s, s, u),
            ),
            // Over the byte cap: every kind a `.js` file is scanned for (not Go), and discovery.
            (vec![("web/src/vendor.js", None)], expect(d, u, u, u)),
            // Prose and a plain dependency entry are inert.
            (
                vec![
                    (
                        "web/README.md",
                        Some("Call `axios.defaults.adapter = x` or fetch.\n"),
                    ),
                    (
                        "web/package.json",
                        Some(
                            "{ \"dependencies\": { \"axios\": \"^1.7.7\" }, \"devDependencies\": { \"node-fetch\": \"~3.0\" } }\n",
                        ),
                    ),
                    (
                        "web/package-lock.json",
                        Some("{ \"packages\": { \"node_modules/axios\": {} } }\n"),
                    ),
                ],
                expect(s, s, s, s),
            ),
            // An XML comment is read (contract M5 S19 (b)): it caps Feign and `fetch`.
            (
                vec![(
                    "pay/pom.xml",
                    Some("<project><!-- the fetch phase; feign.Client --></project>\n"),
                )],
                expect(s, u, u, s),
            ),
            // Discovery as nested JSON in a compose file (contract M5 S19 (a)).
            (
                vec![(
                    "deploy/docker-compose.yml",
                    Some(
                        "services:\n  pay:\n    environment:\n      SPRING_APPLICATION_JSON: '{\"spring\":{\"cloud\":{\"discovery\":{\"client\":{\"simple\":{\"instances\":{\"orders\":[{\"uri\":\"https://api.stripe.com\"}]}}}}}}}'\n",
                    ),
                )],
                expect(d, d, d, d),
            ),
            // A dependency aliased elsewhere is no plain entry.
            (
                vec![(
                    "web/package.json",
                    Some("{ \"dependencies\": { \"axios\": \"file:./fake-axios\" } }\n"),
                )],
                expect(s, s, s, u),
            ),
            (
                vec![(
                    "web/package.json",
                    Some("{ \"browser\": { \"axios\": \"./fake-axios.js\" } }\n"),
                )],
                expect(s, s, s, u),
            ),
            // W4: service-discovery configuration.
            (
                vec![("pay/src/main/resources/application.yml", Some(yml))],
                expect(d, d, d, d),
            ),
            (
                vec![(
                    "pay/src/main/resources/application.properties",
                    Some("orders.ribbon.listOfServers=api.stripe.com:443\n"),
                )],
                expect(d, d, d, d),
            ),
            (
                vec![(
                    "pay/src/main/kotlin/p/Lb.kt",
                    Some(
                        "@LoadBalancerClient(name = \"orders\", configuration = [Rebind::class])\nclass Lb\n",
                    ),
                )],
                expect(d, d, d, d),
            ),
        ];
        for (raw, verdicts) in rows {
            let mut unparsed = RawScan::default();
            for (path, text) in &raw {
                unparsed.merge(rb_frameworks::clients::raw_scan(path, *text));
            }
            assert_eq!(
                stitched_bases_raw(&files, &[], &unparsed, &eps, &services),
                verdicts,
                "{raw:?}"
            );
        }
        // W4 in Java code: an import from Spring Cloud LoadBalancer or Ribbon, or a fully
        // qualified annotation.
        let lb = "package p;\nimport org.springframework.cloud.loadbalancer.annotation.LoadBalancerClient;\n@LoadBalancerClient(name = \"orders\", configuration = Rebind.class)\nclass Lb {}\n";
        let annotated = "package p;\n@org.springframework.cloud.loadbalancer.annotation.LoadBalancerClient(name = \"orders\", configuration = Rebind.class)\nclass Lb {}\n";
        for java in [lb, annotated] {
            let mut with_lb = files.to_vec();
            with_lb.insert(1, ("pay/src/main/java/p/Lb.java", Language::Java, java));
            assert_eq!(
                stitched_bases_raw(&with_lb, &[], &RawScan::default(), &eps, &services),
                expect(d, d, d, d),
                "{java}"
            );
        }
        // Spring Cloud's client-side `@LoadBalanced` (another package) is no discovery key.
        let balanced = "package p;\nimport org.springframework.cloud.client.loadbalancer.LoadBalanced;\nclass B {}\n";
        let mut with_balanced = files.to_vec();
        with_balanced.insert(1, ("pay/src/main/java/p/B.java", Language::Java, balanced));
        assert_eq!(
            stitched_bases_raw(&with_balanced, &[], &RawScan::default(), &eps, &services),
            expect(s, s, s, s)
        );
    }

    /// Contract M5 S19 (f) (M4a open item 21, part), from source: a stated base that cannot be
    /// decided but names an external host among its candidates is external, whatever the path
    /// (evaluator pass 3's `t10` with a resolvable spread, a `baseURL` before such a spread, a
    /// nested spread, and `j07`'s two Spring bases, now later-wins but still external through
    /// the decided host); with only internal candidates the path is matched by suffix as
    /// before. Contract M6b S5: a config passed by name that resolves to nothing readable is
    /// dynamic (`t25`'s unreadable config), never suffix-matched; a spread it cannot read names
    /// its value for cross-file resolution. A URL that is absolute ignores the base and its
    /// candidates.
    #[test]
    fn contested_bases_with_an_external_candidate_are_external() {
        let ts = "import axios from \"axios\";\nconst api10 = axios.create({ baseURL: \"http://orders\" });\nconst stripeOpts = { baseURL: \"https://api.stripe.com\" };\nconst nested = { ...stripeOpts, timeout: 1 };\nconst ordersOpts = { baseURL: \"http://orders\" };\nexport async function f(cfg) {\n  await api10.get(\"/orders/1\", { ...stripeOpts });\n  await api10.get(\"/orders/1\", cfg);\n  await api10.get(\"/orders/1\", { baseURL: \"http://orders\", ...stripeOpts });\n  await api10.get(\"/orders/1\", { ...nested });\n  await api10.get(\"/orders/1\", { ...ordersOpts });\n  await api10.get(\"/orders/1\", { ...cfg });\n  await api10.get(\"http://orders/orders/1\", { ...stripeOpts });\n}\n";
        let java = |name: &str, other: &str| {
            format!(
                "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.util.DefaultUriBuilderFactory;\nclass {name} {{\n  private final RestTemplate rt = new RestTemplateBuilder().rootUri(\"http://orders\").build();\n  {name}() {{ rt.setUriTemplateHandler(new DefaultUriBuilderFactory(\"{other}\")); }}\n  String get() {{ return rt.getForObject(\"/orders/1\", String.class); }}\n}}\n"
            )
        };
        let j07 = java("C7", "https://api.stripe.com");
        let internal = java("C8", "http://billing");
        let eps = vec![
            ep("GET", "/orders/{id}", "orders"),
            ep("GET", "/billing", "billing"),
        ];
        let suffix =
            "heuristic -> HTTP GET /orders/{} @orders [heuristic] base=unresolved match=suffix";
        assert_eq!(
            stitched_bases(
                &[
                    ("pay/src/main/java/p/C7.java", Language::Java, &j07),
                    ("pay/src/main/java/p/C8.java", Language::Java, &internal),
                    ("web/src/t.ts", Language::TypeScript, ts),
                ],
                &eps,
                &["web", "pay", "orders", "billing"]
            ),
            [
                "pay/src/main/java/p/C7.java 7 external".to_owned(),
                // Later-wins restricts to the billing host, whose endpoints
                // hold no such path: the suffix edge is gone.
                "pay/src/main/java/p/C8.java 7 unmatched".to_owned(),
                "web/src/t.ts 6 external".to_owned(),
                "web/src/t.ts 7 dynamic".to_owned(),
                "web/src/t.ts 8 external".to_owned(),
                "web/src/t.ts 9 external".to_owned(),
                format!("web/src/t.ts 10 {suffix}"),
                format!("web/src/t.ts 11 {suffix}"),
                "web/src/t.ts 12 heuristic -> HTTP GET /orders/{} @orders [heuristic] base=unverified match=exact".to_owned(),
            ]
        );
    }

    /// Contract M6b S5, from source: a spread from an imported const reads
    /// its `baseURL` through the unique declaration. With an external base
    /// among the candidates the site is external (no edge); with a service
    /// base the relative path stitches as before.
    #[test]
    fn contested_cross_file_spreads() {
        let opts = "export const extOpts = { baseURL: \"https://api.stripe.com\" };\nexport const orderOpts = { baseURL: \"http://orders\" };\n";
        let use_ = "import axios from \"axios\";\nimport { extOpts, orderOpts } from \"./opts\";\nexport async function f() {\n  await axios.get(\"/a\", { ...extOpts });\n  await axios.get(\"/b\", { ...orderOpts });\n}\n";
        let eps = vec![ep("GET", "/a", "orders"), ep("GET", "/b", "orders")];
        assert_eq!(
            stitched_bases(
                &[
                    ("web/opts.ts", Language::TypeScript, opts),
                    ("web/use.ts", Language::TypeScript, use_),
                ],
                &eps,
                &["web", "orders"],
            ),
            [
                "web/use.ts 3 external".to_owned(),
                "web/use.ts 4 heuristic -> HTTP GET /b @orders [heuristic] base=unresolved match=suffix".to_owned(),
            ]
        );
    }

    /// Contract M4a amendment 13, Gate B (evaluator pass 3, R2, R4), from source: a URL whose
    /// path does not start with a literal (a leading hole under a client's base, or a second
    /// hole after a port hole) leaves its prefix unknown: it is matched by suffix only,
    /// `heuristic` at most, and an instance's base is not prepended to it.
    #[test]
    fn a_path_must_start_with_a_literal() {
        let ts = "import axios from \"axios\";\nconst orders = axios.create({ baseURL: \"http://orders\" });\nexport async function l(base: string, id: string, p: string, q: string) {\n  await orders.get(`${process.env.STRIPE_URL}/reports/1`);\n  await orders.get(base + \"/reports/1\");\n  await orders.get(`/${id}/reports/1`);\n  await fetch(`http://orders:${p}${q}/t1/reports/1`);\n  await fetch(`http://orders:${p}/t1/reports/1`);\n}\n";
        let java = "package p;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass C {\n  void j19() {\n    WebClient wc10 = WebClient.create(\"http://orders\");\n    wc10.get().uri(System.getenv(\"X\") + \"/reports/1\").retrieve();\n  }\n}\n";
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
                    "pay/src/main/java/p/C.java 5 heuristic -> {to} [heuristic] base=unresolved match=suffix"
                ),
                // `fetch` sites first, then axios (pack order within a file). The suffix match
                // whose host resolved keeps its label; those without a host are `unresolved`
                // (contract M5 S18).
                format!("web/src/l.ts 6 heuristic -> {to} [heuristic] base=literal match=suffix"),
                format!("web/src/l.ts 7 synthetic -> {to} [synthetic] base=literal match=exact"),
                format!(
                    "web/src/l.ts 3 heuristic -> {to} [heuristic] base=unresolved match=suffix"
                ),
                format!(
                    "web/src/l.ts 4 heuristic -> {to} [heuristic] base=unresolved match=suffix"
                ),
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
