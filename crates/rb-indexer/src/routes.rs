//! Endpoints (contract M4a S4–S8): the route packs' per-file facts composed with prefixes across
//! files (Express mounts, Spring's context path), normalised, aggregated per endpoint key, and
//! added as `Endpoint` nodes with `Service -EXPOSES-> Endpoint` and
//! `Endpoint -HANDLED_BY-> Symbol | File`.

use std::collections::{BTreeMap, BTreeSet};

use rb_frameworks::endpoint::{endpoint_key, endpoint_name, norm_path};
use rb_frameworks::routes::{FileRoutes, Handler, RouteBase, RouterKind};
use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance, Span,
};
use rb_syntax::{FileFacts, Language};
use serde_json::json;

use crate::resolve::Index;

/// Mounts followed at most from a router to its root (contract M4a S5).
pub(crate) const MAX_MOUNT_DEPTH: usize = 32;
/// Distinct prefixes kept per router; a router reached by more paths than this is
/// `prefix_unknown` (several mounts at every level multiply the paths).
pub(crate) const MAX_PREFIXES: usize = 64;

/// What the endpoint stage reads.
pub(crate) struct Inputs<'a, 'r> {
    pub files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
    pub routes: &'r BTreeMap<&'a str, FileRoutes>,
    pub keys: &'a BTreeMap<&'a str, Vec<String>>,
    pub index: &'a Index<'a>,
    /// Handler names an exact index resolved: `(path, token span)` → symbol key.
    pub exact_handlers: &'a BTreeMap<(&'a str, Span), String>,
    /// The key of the service owning a path.
    pub service_of: &'a dyn Fn(&str) -> String,
    /// Service key → `server.servlet.context-path` from its `application.properties`.
    pub context_paths: &'a BTreeMap<String, String>,
}

/// The token a handler name ends with (`login` in `ctrl.login`) and its span, when the name is
/// written on one line (byte columns, so the token's length is its byte length).
pub(crate) fn handler_token(text: &str, span: Span) -> (&str, Span) {
    let token = text.rsplit('.').next().unwrap_or(text);
    if span[0] == span[2] && span[3] >= token.len() as i32 {
        (
            token,
            [span[2], span[3] - token.len() as i32, span[2], span[3]],
        )
    } else {
        (token, span)
    }
}

type RouterId<'a> = (&'a str, usize);

/// A mount into a router: `(parent, prefix, the file of the mount)`.
type MountInto<'a> = (RouterId<'a>, Option<String>, &'a str);

/// The prefixes a router is reached by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Prefixes {
    /// `(prefix, composed through another file)`.
    known: BTreeSet<(String, bool)>,
    unknown: bool,
}

/// Express mount composition (S5): recursive over mounts with an on-path guard, a depth cap and
/// a cap on prefixes per router; only results that hit none of them are memoised.
struct Composer<'a> {
    kinds: BTreeMap<RouterId<'a>, RouterKind>,
    /// Child → `(parent, prefix, the file of the mount)`.
    mounts_into: BTreeMap<RouterId<'a>, Vec<MountInto<'a>>>,
    memo: BTreeMap<RouterId<'a>, Prefixes>,
    on_path: BTreeSet<RouterId<'a>>,
}

impl<'a> Composer<'a> {
    fn new(inputs: &Inputs<'a, '_>) -> Self {
        let mut kinds = BTreeMap::new();
        let mut mounts_into: BTreeMap<RouterId<'a>, Vec<_>> = BTreeMap::new();
        for (&path, file) in inputs.routes {
            for (i, r) in file.routers.iter().enumerate() {
                kinds.insert((path, i), r.kind);
            }
            for m in &file.mounts {
                for child in mount_targets(inputs, path, m) {
                    mounts_into.entry(child).or_default().push((
                        (path, m.parent),
                        m.prefix.clone(),
                        path,
                    ));
                }
            }
        }
        Composer {
            kinds,
            mounts_into,
            memo: BTreeMap::new(),
            on_path: BTreeSet::new(),
        }
    }

    /// The prefixes of `r`, and whether the result is clean (no guard or cap was hit).
    fn prefixes(&mut self, r: RouterId<'a>, depth: usize) -> (Prefixes, bool) {
        if let Some(done) = self.memo.get(&r) {
            return (done.clone(), true);
        }
        if depth >= MAX_MOUNT_DEPTH || !self.on_path.insert(r) {
            return (
                Prefixes {
                    known: BTreeSet::new(),
                    unknown: true,
                },
                false,
            );
        }
        let mut out = Prefixes::default();
        let mut clean = true;
        let mounts = self.mounts_into.get(&r).cloned().unwrap_or_default();
        if mounts.is_empty() {
            match self.kinds.get(&r) {
                Some(RouterKind::App) => {
                    out.known.insert((String::new(), false));
                }
                _ => out.unknown = true,
            }
        }
        for (parent, prefix, mount_file) in mounts {
            let Some(prefix) = prefix else {
                out.unknown = true;
                continue;
            };
            let (above, c) = self.prefixes(parent, depth + 1);
            clean &= c;
            out.unknown |= above.unknown;
            let cross = mount_file != r.0;
            for (q, crossed) in above.known {
                if out.known.len() >= MAX_PREFIXES {
                    out.unknown = true;
                    break;
                }
                out.known
                    .insert((format!("{q}/{prefix}"), crossed || cross));
            }
        }
        self.on_path.remove(&r);
        if clean {
            self.memo.insert(r, out.clone());
        }
        (out, clean)
    }
}

/// The routers a mount's child names: a router of the same file, or a top-level router of a
/// module the file imports by that name (named import, or `ns.name` through a namespace
/// import). A default import names no router: export facts do not say which binding it is.
fn mount_targets<'a>(
    inputs: &Inputs<'a, '_>,
    path: &'a str,
    m: &rb_frameworks::routes::Mount,
) -> Vec<RouterId<'a>> {
    if let Some(i) = m.child_local {
        return vec![(path, i)];
    }
    let Some((_, facts)) = inputs.files.get(path) else {
        return Vec::new();
    };
    let targets = inputs.index.import_targets(path);
    let mut out = Vec::new();
    for (import, files) in facts.imports.iter().zip(targets) {
        let exported: Option<&str> = match m.child.split_once('.') {
            None => import
                .names
                .iter()
                .find(|n| n.local == m.child && n.name != "default")
                .map(|n| n.name.as_str()),
            Some((head, tail)) if import.alias.as_deref() == Some(head) && !tail.contains('.') => {
                Some(tail)
            }
            Some(_) => None,
        };
        let Some(exported) = exported else { continue };
        for &target in files {
            if let Some(routes) = inputs.routes.get(target) {
                out.extend(
                    routes
                        .routers
                        .iter()
                        .enumerate()
                        .filter(|(_, r)| r.top_level && r.name == exported)
                        .map(|(i, _)| (target, i)),
                );
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// One registration of an endpoint key.
struct Registration {
    path_certainty: Certainty,
    handler: Vec<(NodeKey, Certainty, Option<Span>)>,
    /// More than one handler candidate: `ambiguous` with this count.
    candidates: usize,
    at: Evidence,
}

/// An endpoint the stage added, as stitching reads it (contract M4a S13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EndpointRef {
    pub key: String,
    pub method: String,
    /// Normalised.
    pub path: String,
    pub service: String,
    pub prefix_unknown: bool,
    /// The certainty of its path (its `EXPOSES` edge).
    pub certainty: Certainty,
}

#[derive(Default)]
struct Endpoint {
    method: String,
    path: String,
    service: String,
    frameworks: BTreeSet<&'static str>,
    prefix_unknown: bool,
    registrations: Vec<Registration>,
}

/// Adds every endpoint with its edges; returns the route diagnostics and the endpoints, in key
/// order.
pub(crate) fn add_endpoints(
    b: &mut GraphBuilder,
    inputs: &Inputs<'_, '_>,
) -> anyhow::Result<(Vec<Diagnostic>, Vec<EndpointRef>)> {
    let mut composer = Composer::new(inputs);
    let mut endpoints: BTreeMap<String, Endpoint> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    let diag = |kind: &str, path: &str, span: Span, detail: serde_json::Value| Diagnostic {
        kind: kind.to_owned(),
        path: Some(path.to_owned()),
        span: Some(span),
        detail,
    };
    for (&path, file) in inputs.routes {
        for d in &file.diagnostics {
            diagnostics.push(diag(d.kind, path, d.span, json!({ "detail": d.detail })));
        }
        let service = (inputs.service_of)(path);
        let keys = &inputs.keys[path];
        for r in &file.routes {
            // Full paths with their certainty; `None` when the prefix is unknown.
            let mut paths: Vec<(String, Certainty, bool)> = Vec::new();
            match r.base {
                RouteBase::Root => match inputs.context_paths.get(&service) {
                    Some(ctx) if r.framework == rb_frameworks::routes::Framework::Spring => {
                        paths.push((format!("{ctx}/{}", r.path), Certainty::Synthetic, false));
                    }
                    _ => paths.push((r.path.clone(), Certainty::Exact, false)),
                },
                RouteBase::Unknown => paths.push((r.path.clone(), Certainty::Heuristic, true)),
                RouteBase::Router(i) => {
                    let (prefixes, _) = composer.prefixes((path, i), 0);
                    for (p, cross) in &prefixes.known {
                        let certainty = if *cross {
                            Certainty::Heuristic
                        } else {
                            Certainty::Exact
                        };
                        paths.push((format!("{p}/{}", r.path), certainty, false));
                    }
                    if prefixes.unknown {
                        paths.push((r.path.clone(), Certainty::Heuristic, true));
                    }
                }
            }
            let method = r.method.clone();
            if paths.iter().any(|(_, _, unknown)| *unknown) {
                diagnostics.push(diag(
                    "route_prefix_unknown",
                    path,
                    r.span,
                    json!({
                        "framework": r.framework.as_str(),
                        "method": method,
                        "path": norm_path(&r.path),
                    }),
                ));
            }
            let (handler, candidates) = handler_targets(inputs, path, keys, r);
            if handler.is_empty() {
                let (span, text) = match &r.handler {
                    Handler::Name { text, span } => (*span, text.clone()),
                    Handler::Unknown(span) if *span != [0; 4] => (*span, String::new()),
                    _ => (r.span, String::new()),
                };
                diagnostics.push(diag(
                    "route_handler_unresolved",
                    path,
                    span,
                    json!({ "framework": r.framework.as_str(), "handler": text }),
                ));
            }
            for (full, path_certainty, unknown) in paths {
                let norm = norm_path(&full);
                let key = endpoint_key(&method, &norm, &service);
                let e = endpoints.entry(key).or_insert_with(|| Endpoint {
                    method: method.clone(),
                    path: norm.clone(),
                    service: service.clone(),
                    ..Endpoint::default()
                });
                e.frameworks.insert(r.framework.as_str());
                e.prefix_unknown |= unknown;
                e.registrations.push(Registration {
                    path_certainty,
                    handler: handler.clone(),
                    candidates,
                    at: Evidence {
                        path: path.to_owned(),
                        span: r.span,
                    },
                });
            }
        }
    }
    let added = endpoints
        .into_iter()
        .map(|(key, e)| add_endpoint(b, key, e))
        .collect::<anyhow::Result<_>>()?;
    Ok((diagnostics, added))
}

/// A route's handler targets, each with its certainty and, for an inline function, its span;
/// plus the candidate count when there are several.
fn handler_targets(
    inputs: &Inputs<'_, '_>,
    path: &str,
    keys: &[String],
    r: &rb_frameworks::routes::RouteFact,
) -> (Vec<(NodeKey, Certainty, Option<Span>)>, usize) {
    let symbol = |key: &str| NodeKey::new(NodeKind::Symbol, key);
    match &r.handler {
        Handler::Definition(i) => (vec![(symbol(&keys[*i]), Certainty::Exact, None)], 1),
        Handler::Inline(span) => {
            let dst = match r.enclosing {
                Some(i) => symbol(&keys[i]),
                None => NodeKey::new(NodeKind::File, path),
            };
            (vec![(dst, Certainty::Exact, Some(*span))], 1)
        }
        Handler::Name { text, span } => {
            let (_, token_span) = handler_token(text, *span);
            let path_key = inputs
                .files
                .get_key_value(path)
                .map(|(k, _)| *k)
                .unwrap_or_default();
            if let Some(key) = inputs.exact_handlers.get(&(path_key, token_span)) {
                return (vec![(symbol(key), Certainty::Exact, None)], 1);
            }
            let found = inputs
                .index
                .handler_targets(path_key, r.enclosing, text, token_span);
            let certainty = if found.len() == 1 {
                Certainty::Heuristic
            } else {
                Certainty::Ambiguous
            };
            let n = found.len();
            (
                found
                    .into_iter()
                    .map(|k| (symbol(&k), certainty, None))
                    .collect(),
                n,
            )
        }
        Handler::Unknown(_) => (Vec::new(), 0),
    }
}

/// The node and its edges, merged over registrations (contract M4a invariants 1 and 4): each
/// edge takes the best registration's certainty, every registration's evidence, and an inline
/// handler's spans.
fn add_endpoint(b: &mut GraphBuilder, key: String, e: Endpoint) -> anyhow::Result<EndpointRef> {
    let node = NodeKey::new(NodeKind::Endpoint, key.clone());
    let mut attrs = Attrs::new();
    attrs.insert("method".into(), e.method.clone().into());
    attrs.insert("path".into(), e.path.clone().into());
    attrs.insert("service".into(), e.service.clone().into());
    attrs.insert(
        "frameworks".into(),
        e.frameworks.iter().copied().collect::<Vec<_>>().into(),
    );
    if e.prefix_unknown {
        attrs.insert("prefix_unknown".into(), true.into());
    }
    b.add_node(node.clone(), endpoint_name(&e.method, &e.path), attrs)?;

    let exposes = e
        .registrations
        .iter()
        .map(|r| r.path_certainty)
        .max()
        .unwrap_or(Certainty::Heuristic);
    let mut edge = NewEdge::new(
        NodeKey::new(NodeKind::Service, e.service.clone()),
        EdgeKind::Exposes,
        node.clone(),
        Provenance::Treesitter,
        exposes,
    );
    edge.evidence = e
        .registrations
        .iter()
        .map(|r| r.at.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    b.add_edge(edge)?;

    struct Handled {
        certainty: Certainty,
        candidates: usize,
        spans: BTreeSet<Span>,
        evidence: BTreeSet<Evidence>,
    }
    let mut handled: BTreeMap<NodeKey, Handled> = BTreeMap::new();
    for r in &e.registrations {
        for (dst, certainty, span) in &r.handler {
            let certainty = (*certainty).min(r.path_certainty);
            let h = handled.entry(dst.clone()).or_insert(Handled {
                certainty,
                candidates: 0,
                spans: BTreeSet::new(),
                evidence: BTreeSet::new(),
            });
            h.certainty = h.certainty.max(certainty);
            if certainty == Certainty::Ambiguous {
                h.candidates = h.candidates.max(r.candidates);
            }
            h.spans.extend(span);
            h.evidence.insert(r.at.clone());
        }
    }
    for (dst, h) in handled {
        let mut attrs = Attrs::new();
        if h.certainty == Certainty::Ambiguous {
            attrs.insert("candidates".into(), h.candidates.into());
        }
        if !h.spans.is_empty() {
            attrs.insert(
                "handler_spans".into(),
                h.spans.iter().map(|s| json!(s)).collect::<Vec<_>>().into(),
            );
        }
        let mut edge = NewEdge::new(
            node.clone(),
            EdgeKind::HandledBy,
            dst,
            Provenance::Treesitter,
            h.certainty,
        )
        .attrs(attrs);
        edge.evidence = h.evidence.into_iter().collect();
        b.add_edge(edge)?;
    }
    Ok(EndpointRef {
        key,
        method: e.method,
        path: e.path,
        service: e.service,
        prefix_unknown: e.prefix_unknown,
        certainty: exposes,
    })
}

#[cfg(test)]
mod tests {
    use rb_model::Graph;

    use super::*;

    struct Built {
        graph: Graph,
        diagnostics: Vec<Diagnostic>,
    }

    /// Indexes `files` (path, language, source) with every file in the service named by its
    /// first path segment, and `context_paths` per service.
    fn build(files: &[(&str, Language, &str)], context_paths: &[(&str, &str)]) -> Built {
        let facts: Vec<(String, Language, FileFacts)> = files
            .iter()
            .map(|(p, l, src)| {
                (
                    (*p).to_owned(),
                    *l,
                    rb_syntax::extract(*l, src.as_bytes()).unwrap(),
                )
            })
            .collect();
        build_from(&facts, context_paths, &BTreeMap::new())
    }

    fn build_from(
        facts: &[(String, Language, FileFacts)],
        context_paths: &[(&str, &str)],
        exact: &BTreeMap<(&str, Span), String>,
    ) -> Built {
        let by_path: BTreeMap<&str, (Language, &FileFacts)> = facts
            .iter()
            .map(|(p, l, f)| (p.as_str(), (*l, f)))
            .collect();
        let keys: BTreeMap<&str, Vec<String>> = by_path
            .iter()
            .map(|(p, (_, f))| (*p, rb_syntax::symbol_keys(p, f)))
            .collect();
        let routes: BTreeMap<&str, FileRoutes> = by_path
            .iter()
            .map(|(p, (l, f))| (*p, rb_frameworks::routes::routes(*l, f)))
            .collect();
        let go = BTreeMap::new();
        let index = Index::new(&by_path, &go, &keys);
        let mut b = GraphBuilder::new();
        let service_of = |p: &str| p.split('/').next().unwrap_or(p).to_owned();
        for (path, (_, f)) in &by_path {
            b.add_node(NodeKey::new(NodeKind::File, *path), *path, Attrs::new())
                .unwrap();
            let s = NodeKey::new(NodeKind::Service, service_of(path));
            if !b.has_node(&s) {
                b.add_node(s, service_of(path), Attrs::new()).unwrap();
            }
            crate::syntax::add_symbols(&mut b, path, f, &keys[path]).unwrap();
        }
        let context_paths: BTreeMap<String, String> = context_paths
            .iter()
            .map(|(s, c)| ((*s).to_owned(), (*c).to_owned()))
            .collect();
        let inputs = Inputs {
            files: &by_path,
            routes: &routes,
            keys: &keys,
            index: &index,
            exact_handlers: exact,
            service_of: &service_of,
            context_paths: &context_paths,
        };
        let (diagnostics, _) = add_endpoints(&mut b, &inputs).unwrap();
        Built {
            graph: b.finish().unwrap(),
            diagnostics,
        }
    }

    /// `KIND src -> dst [certainty]` for the route edges, keys shortened after the last `/`.
    fn edges(built: &Built) -> Vec<String> {
        let g = &built.graph;
        let key = |id: i64| g.nodes().iter().find(|n| n.id == id).unwrap().key.clone();
        g.edges()
            .iter()
            .filter(|e| matches!(e.kind, EdgeKind::Exposes | EdgeKind::HandledBy))
            .map(|e| {
                let (src, dst) = (key(e.src), key(e.dst));
                format!(
                    "{} {src} -> {dst} [{}]",
                    e.kind.as_str(),
                    e.certainty.as_str()
                )
            })
            .collect()
    }

    fn diagnostics(built: &Built) -> Vec<String> {
        built
            .diagnostics
            .iter()
            .map(|d| format!("{} {}", d.kind, d.detail))
            .collect()
    }

    const APP: &str = "import express from \"express\";\nimport { authRouter } from \"./routes/auth\";\nexport const app = express();\napp.use(express.json());\napp.use(\"/api/v1\", authRouter);\napp.get(\"/health\", (req, res) => res.end());\n";
    const AUTH: &str = "import { Router } from \"express\";\nimport { findUserByEmail } from \"../db/users\";\nexport const authRouter = Router();\nauthRouter.post(\"/login\", async (req, res) => {\n  await findUserByEmail(req.body.email);\n});\n";

    #[test]
    fn express_cross_file_mounts_are_heuristic_and_inline_handlers_scoped() {
        let built = build(
            &[
                ("api/src/app.ts", Language::TypeScript, APP),
                ("api/src/routes/auth.ts", Language::TypeScript, AUTH),
            ],
            &[],
        );
        assert_eq!(
            edges(&built),
            [
                "EXPOSES api -> HTTP GET /health @api [exact]",
                "EXPOSES api -> HTTP POST /api/v1/login @api [heuristic]",
                "HANDLED_BY HTTP GET /health @api -> api/src/app.ts [exact]",
                "HANDLED_BY HTTP POST /api/v1/login @api -> api/src/routes/auth.ts [heuristic]",
            ]
        );
        let g = &built.graph;
        let login = g
            .edges()
            .iter()
            .find(|e| e.kind == EdgeKind::HandledBy && e.certainty == Certainty::Heuristic)
            .unwrap();
        let arrow = AUTH.find("async (req, res)").unwrap();
        let row = AUTH[..arrow].matches('\n').count() as i64;
        assert_eq!(login.attrs["handler_spans"][0][0], row);
        assert_eq!(login.evidence[0].path, "api/src/routes/auth.ts");
        assert!(built.diagnostics.is_empty(), "{:?}", diagnostics(&built));
        let node = g
            .nodes()
            .iter()
            .find(|n| n.kind == NodeKind::Endpoint)
            .unwrap();
        assert_eq!(node.name, "GET /health");
        assert_eq!(node.attrs["frameworks"], json!(["express"]));
    }

    #[test]
    fn unmounted_routers_have_unknown_prefixes_and_mount_cycles_terminate() {
        let src = "import { Router } from \"express\";\nconst a = Router();\nconst b = Router();\na.use(\"/x\", b);\nb.use(\"/y\", a);\na.get(\"/r\", (q, s) => s.end());\nconst lone = Router();\nlone.get(\"/l\", (q, s) => s.end());\n";
        let built = build(&[("svc/r.ts", Language::TypeScript, src)], &[]);
        assert_eq!(
            edges(&built)
                .iter()
                .filter(|e| e.starts_with("EXPOSES"))
                .cloned()
                .collect::<Vec<_>>(),
            [
                "EXPOSES svc -> HTTP GET /l @svc [heuristic]",
                "EXPOSES svc -> HTTP GET /r @svc [heuristic]",
            ]
        );
        let unknown: Vec<&Diagnostic> = built
            .diagnostics
            .iter()
            .filter(|d| d.kind == "route_prefix_unknown")
            .collect();
        assert_eq!(unknown.len(), 2);
        assert!(
            built
                .graph
                .nodes()
                .iter()
                .filter(|n| n.kind == NodeKind::Endpoint)
                .all(|n| n.attrs["prefix_unknown"] == true)
        );
    }

    /// A chain of `n` routers mounted one below the other under an app.
    fn mount_chain(n: usize) -> String {
        let mut src =
            String::from("import express, { Router } from \"express\";\nconst app = express();\n");
        for i in 1..=n {
            src.push_str(&format!("const r{i} = Router();\n"));
        }
        src.push_str("app.use(\"/s\", r1);\n");
        for i in 1..n {
            src.push_str(&format!("r{i}.use(\"/s\", r{});\n", i + 1));
        }
        src.push_str(&format!(
            "r{n}.get(\"/deep\", (q, s) => s.end());\nr3.get(\"/shallow\", (q, s) => s.end());\n"
        ));
        src
    }

    #[test]
    fn a_5000_deep_mount_chain_is_cut_at_the_cap() {
        let built = build(
            &[("svc/r.ts", Language::TypeScript, &mount_chain(5000))],
            &[],
        );
        let exposes: Vec<String> = edges(&built)
            .into_iter()
            .filter(|e| e.starts_with("EXPOSES"))
            .collect();
        assert_eq!(
            exposes,
            [
                "EXPOSES svc -> HTTP GET /deep @svc [heuristic]",
                "EXPOSES svc -> HTTP GET /s/s/s/shallow @svc [exact]",
            ]
        );
    }

    /// Composition (the route pack and the endpoint stage) is linear in the mount chain:
    /// doubling it costs at most 2.5 times as much (best of three; a ratio, not a wall-clock
    /// bound). Parsing and the M3a index are built outside the timed part.
    #[test]
    fn composition_grows_linearly() {
        let time = |n: usize| {
            let src = mount_chain(n);
            let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
            let by_path = BTreeMap::from([("svc/r.ts", (Language::TypeScript, &facts))]);
            let keys = BTreeMap::from([("svc/r.ts", rb_syntax::symbol_keys("svc/r.ts", &facts))]);
            let go = BTreeMap::new();
            let index = Index::new(&by_path, &go, &keys);
            let service_of = |_: &str| "svc".to_owned();
            let (exact, context) = (BTreeMap::new(), BTreeMap::new());
            (0..3)
                .map(|_| {
                    let mut b = GraphBuilder::new();
                    b.add_node(NodeKey::new(NodeKind::Service, "svc"), "svc", Attrs::new())
                        .unwrap();
                    b.add_node(
                        NodeKey::new(NodeKind::File, "svc/r.ts"),
                        "r.ts",
                        Attrs::new(),
                    )
                    .unwrap();
                    let start = std::time::Instant::now();
                    let routes = BTreeMap::from([(
                        "svc/r.ts",
                        rb_frameworks::routes::routes(Language::TypeScript, &facts),
                    )]);
                    let inputs = Inputs {
                        files: &by_path,
                        routes: &routes,
                        keys: &keys,
                        index: &index,
                        exact_handlers: &exact,
                        service_of: &service_of,
                        context_paths: &context,
                    };
                    add_endpoints(&mut b, &inputs).unwrap();
                    start.elapsed()
                })
                .min()
                .unwrap()
        };
        let (small, large) = (time(2000), time(4000));
        let ratio = large.as_secs_f64() / small.as_secs_f64();
        assert!(ratio < 2.5, "{small:?} → {large:?} ({ratio:.2}×)");
    }

    /// S8: a handler name resolves as a call at its position, never by bare name across the
    /// repository; an unresolved one keeps the endpoint and is counted.
    #[test]
    fn handler_names_resolve_like_calls() {
        let routes = "import { Router } from \"express\";\nimport { login } from \"./handlers\";\nexport const r = Router();\nr.post(\"/login\", login);\nr.post(\"/logout\", logout);\n";
        let handlers = "export function login(req, res) {}\n";
        let unrelated = "export function login() {}\nexport function logout() {}\n";
        let built = build(
            &[
                ("svc/routes.ts", Language::TypeScript, routes),
                ("svc/handlers.ts", Language::TypeScript, handlers),
                ("svc/unrelated.ts", Language::TypeScript, unrelated),
            ],
            &[],
        );
        let handled: Vec<String> = edges(&built)
            .into_iter()
            .filter(|e| e.starts_with("HANDLED_BY"))
            .collect();
        assert_eq!(
            handled,
            [
                "HANDLED_BY HTTP POST /login @svc -> rb-heuristic . . . `svc/handlers.ts`/login(). [heuristic]"
            ]
        );
        assert!(
            diagnostics(&built)
                .iter()
                .any(|d| d.starts_with("route_handler_unresolved") && d.contains("logout")),
            "{:?}",
            diagnostics(&built)
        );
        // The endpoint of the unresolved handler is still exposed.
        assert!(
            edges(&built)
                .iter()
                .any(|e| e.contains("EXPOSES svc -> HTTP POST /logout"))
        );
    }

    #[test]
    fn duplicate_registrations_merge_into_one_node() {
        let src = "package main\nimport \"github.com/gin-gonic/gin\"\nfunc a() {}\nfunc b() {}\nfunc main() {\n\tr := gin.Default()\n\tr.GET(\"/x\", a)\n\tr.GET(\"/x/\", b)\n\tr.GET(\"//x\", a)\n}\n";
        let built = build(&[("svc/main.go", Language::Go, src)], &[]);
        assert_eq!(
            edges(&built),
            [
                "EXPOSES svc -> HTTP GET /x @svc [exact]",
                "HANDLED_BY HTTP GET /x @svc -> rb-heuristic . . . `svc/main.go`/a(). [heuristic]",
                "HANDLED_BY HTTP GET /x @svc -> rb-heuristic . . . `svc/main.go`/b(). [heuristic]",
            ]
        );
        let g = &built.graph;
        let exposes = g
            .edges()
            .iter()
            .find(|e| e.kind == EdgeKind::Exposes)
            .unwrap();
        assert_eq!(exposes.evidence.len(), 3);
        let a = g
            .edges()
            .iter()
            .find(|e| e.kind == EdgeKind::HandledBy && e.evidence.len() == 2)
            .expect("a's two registrations are one edge with two evidence rows");
        assert!(a.attrs.is_empty());
    }

    #[test]
    fn spring_context_paths_are_synthetic() {
        let src = "package p;\nimport org.springframework.web.bind.annotation.*;\n@RestController\n@RequestMapping(\"/api/v1\")\nclass AuthController {\n  @PostMapping(\"/login\")\n  String login() { return \"\"; }\n}\n";
        let built = build(
            &[("auth/src/A.java", Language::Java, src)],
            &[("auth", "/auth")],
        );
        assert_eq!(
            edges(&built),
            [
                "EXPOSES auth -> HTTP POST /auth/api/v1/login @auth [synthetic]",
                "HANDLED_BY HTTP POST /auth/api/v1/login @auth -> rb-heuristic . . . `auth/src/A.java`/AuthController#login(). [synthetic]",
            ]
        );
        let without = build(&[("auth/src/A.java", Language::Java, src)], &[]);
        assert!(edges(&without)[0].ends_with("/api/v1/login @auth [exact]"));
    }

    /// S8: in a file an exact index owns, the occurrence at the handler's token makes
    /// `HANDLED_BY` exact; without it the same name is heuristic.
    #[test]
    fn an_exact_occurrence_makes_a_named_handler_exact() {
        let app = "import express from \"express\";\nimport { login } from \"./handlers\";\nconst app = express();\napp.post(\"/login\", login);\n";
        let facts: Vec<(String, Language, FileFacts)> = [
            ("svc/app.ts", app),
            ("svc/handlers.ts", "export function login(req, res) {}\n"),
        ]
        .iter()
        .map(|(p, s)| {
            (
                (*p).to_owned(),
                Language::TypeScript,
                rb_syntax::extract(Language::TypeScript, s.as_bytes()).unwrap(),
            )
        })
        .collect();
        let start = app.find(", login)").unwrap() + 2;
        let row = app[..start].matches('\n').count() as i32;
        let col = (start - app[..start].rfind('\n').unwrap() - 1) as i32;
        let handler = rb_syntax::symbol_keys("svc/handlers.ts", &facts[1].2)[0].clone();
        let certainty = |exact: &BTreeMap<(&str, Span), String>| {
            let built = build_from(&facts, &[], exact);
            let g = &built.graph;
            let e = g
                .edges()
                .iter()
                .find(|e| e.kind == EdgeKind::HandledBy)
                .unwrap();
            let dst = &g.nodes().iter().find(|n| n.id == e.dst).unwrap().key;
            assert_eq!(dst, &handler);
            e.certainty
        };
        let exact = BTreeMap::from([(("svc/app.ts", [row, col, row, col + 5]), handler.clone())]);
        assert_eq!(certainty(&exact), Certainty::Exact);
        assert_eq!(certainty(&BTreeMap::new()), Certainty::Heuristic);
    }
}
