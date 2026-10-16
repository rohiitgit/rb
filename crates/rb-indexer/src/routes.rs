//! Endpoints (contract M4a S4–S8): the route packs' per-file facts composed with prefixes across
//! files (Express mounts, Spring's context path), normalised, aggregated per endpoint key, and
//! added as `Endpoint` nodes with `Service -EXPOSES-> Endpoint` and
//! `Endpoint -HANDLED_BY-> Symbol | File`.

use std::collections::{BTreeMap, BTreeSet};

use rb_frameworks::endpoint::{
    endpoint_key, endpoint_name, grpc_key, norm_path, prefix_unknown_key,
};
use rb_frameworks::routes::{
    FileRoutes, FileScope, Framework, Handler, InterfaceRoute, RouteBase, RouterKind,
    binding_identity, gin_group_prefix,
};
use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance, Span,
};
use rb_syntax::{Arg, FileFacts, Init, Language};
use serde_json::json;

use crate::constants::Constants;
use crate::resolve::Index;
use rb_frameworks::fold::Origin;

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
    /// Services with Spring configuration M4a does not read (`config_unread`, evaluator pass 1,
    /// P7): their endpoints' deployed prefix may be missing.
    pub config_unread: &'a BTreeSet<String>,
    /// Services whose build file names Spring Data REST (contract M9b D7), each with its
    /// `spring.data.rest.base-path` when one is set; others get no data-rest endpoint.
    pub data_rest: &'a BTreeMap<String, Option<String>>,
    /// OpenAPI/Swagger and `.proto` texts by path (contract M6f S3).
    pub specs: &'a BTreeMap<&'a str, String>,
    /// Whether a path lies in any project (a spec outside all projects stays endpoint-less).
    pub has_project: &'a dyn Fn(&str) -> bool,
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
        rb_frameworks::work::tick(1);
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
        rb_frameworks::work::tick(mounts.len());
        if mounts.is_empty() {
            match self.kinds.get(&r) {
                Some(RouterKind::App) => {
                    out.known.insert((String::new(), false));
                }
                _ => out.unknown = true,
            }
        }
        for (parent, prefix, mount_file) in mounts {
            // Known prefixes at the cap and the prefix already unknown: no further mount can
            // change the result, so the rest are not explored (contract M7b S4). Like any cap,
            // the result is then not memoised.
            if out.known.len() >= MAX_PREFIXES && out.unknown {
                clean = false;
                break;
            }
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
/// import). A default import names the router its module's `export default` names
/// (`FileFacts::default_export`, contract M4b S19), one level only: a default export that is
/// itself an import is no router of that module, so its prefix stays unknown.
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
    let python = inputs
        .files
        .get(path)
        .is_some_and(|(language, _)| *language == Language::Python);
    let mut out = Vec::new();
    for (import, files) in facts.imports.iter().zip(targets) {
        // Python `from pkg import module` then `module.router`: only the submodule's own file
        // (`module.py`, `module/__init__.py`) declares the name (contract M7a S2).
        let mut submodule: Option<&str> = None;
        let exported: Option<&str> = match m.child.split_once('.') {
            None => import
                .names
                .iter()
                .find(|n| n.local == m.child)
                .map(|n| n.name.as_str()),
            Some((head, tail)) if import.alias.as_deref() == Some(head) && !tail.contains('.') => {
                Some(tail)
            }
            Some((head, tail)) if python && !tail.contains('.') => {
                submodule = import
                    .names
                    .iter()
                    .find(|n| n.local == head)
                    .map(|n| n.name.as_str());
                submodule.map(|_| tail)
            }
            Some(_) => None,
        };
        let Some(exported) = exported else { continue };
        for &target in files {
            if let Some(module) = submodule
                && !(target.ends_with(&format!("/{module}.py"))
                    || target == format!("{module}.py")
                    || target.ends_with(&format!("/{module}/__init__.py"))
                    || target == format!("{module}/__init__.py"))
            {
                continue;
            }
            // A default import names the target's default export; an anonymous
            // default-exported construction is named `default` (contract M6b S3).
            let exported = if exported == "default" {
                inputs
                    .files
                    .get(target)
                    .and_then(|(_, f)| f.default_export.as_deref())
                    .unwrap_or("default")
            } else {
                exported
            };
            if let Some(routes) = inputs.routes.get(target) {
                out.extend(
                    routes
                        .routers
                        .iter()
                        .enumerate()
                        .inspect(|_| rb_frameworks::work::tick(1))
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
    /// Call sites a parameter group's prefix was composed from (contract M6b S2):
    /// the registration's file and span stay in `at`.
    composed_from: Vec<Evidence>,
    /// From an OpenAPI/proto spec (contract M6f S3): never carries a handler itself; it joins
    /// one through the shared key when a code route registers the same endpoint.
    from_spec: bool,
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
    /// Go `ServeMux` host patterns (`example.com/x`), lower-cased without the port: a client
    /// matches only when its host is one of them. Empty: any host (evaluator pass 1, P8).
    pub hosts: Vec<String>,
    /// The service has Spring configuration M4a does not read (`config_unread`), so its deployed
    /// prefix may be missing (evaluator pass 1, P7).
    pub prefix_maybe_incomplete: bool,
    /// Operation IDs its spec registrations state (contract M6f S6).
    pub operations: Vec<String>,
    /// Matched with literal case folded: every code registration is on an Express router
    /// without a case-sensitive opt-in (contract M7i S1).
    pub fold_case: bool,
}

#[derive(Default)]
struct Endpoint {
    method: String,
    path: String,
    service: String,
    frameworks: BTreeSet<&'static str>,
    /// Operation IDs its spec registrations state (contract M6f S6, for stitching generated
    /// calls by operation name).
    operations: BTreeSet<String>,
    /// The router of an endpoint whose prefix is unknown (part of its key, amendment 10).
    router: Option<String>,
    /// `ServeMux` host patterns of its registrations (P8).
    hosts: BTreeSet<String>,
    /// A registration with no host pattern: the endpoint serves every host.
    any_host: bool,
    /// Some code registration is on an Express router that folds case, and some needs exact
    /// case (contract M7i S1); spec registrations say neither.
    case_folds: bool,
    case_exact: bool,
    registrations: Vec<Registration>,
}

/// A gin parameter group's call-site compositions (contract M6b S2).
#[derive(Default)]
struct ParamCompose {
    /// Composed prefixes with every call site's evidence, by prefix.
    composed: BTreeMap<String, Vec<Evidence>>,
    /// Call sites seen for the group.
    sites: usize,
    /// Some site did not resolve or fold: the `prefix_unknown` node stays.
    failed: bool,
}

/// Fold the group argument a call site passes: a `Group(…)` call folds through the
/// caller's bindings; a name follows its initialiser's call chain. Anything else is
/// unresolvable (the unknown node stays as the fallback).
fn fold_group_arg(scope: &FileScope<'_>, arg: Option<&Arg>) -> Option<String> {
    let chain = match arg {
        Some(Arg::Other { span }) => scope.chain_at(*span)?,
        Some(Arg::Name { text, span }) => {
            let init = scope.binding_at(text, *span)?.init.clone()?;
            match init {
                Init::Call { chain } => scope.facts.chains.get(chain)?,
                _ => return None,
            }
        }
        _ => return None,
    };
    gin_group_prefix(scope.facts, chain).flatten()
}

/// gin parameter groups composed from their call sites: `(callee file, func def,
/// param position)` → compositions. One identity per (router site, call site); the
/// endpoint key stays path-based, so identical compositions merge (contract M6b D1).
fn param_compositions<'a>(
    inputs: &Inputs<'a, '_>,
) -> BTreeMap<(&'a str, usize, u32), ParamCompose> {
    type GroupId<'a> = (&'a str, usize, u32);
    let mut groups: BTreeSet<GroupId<'a>> = BTreeSet::new();
    // Func simple name → group ids (the resolution prefilter); func key → group ids.
    let mut by_name: BTreeMap<&'a str, Vec<GroupId<'a>>> = BTreeMap::new();
    let mut by_key: BTreeMap<&'a str, Vec<GroupId<'a>>> = BTreeMap::new();
    for (&path, file) in inputs.routes {
        let facts = inputs.files[path].1;
        for r in &file.routes {
            rb_frameworks::work::tick(1);
            if r.framework != Framework::Gin {
                continue;
            }
            let RouteBase::Unknown(b) = r.base else {
                continue;
            };
            let Some(binding) = facts.bindings.get(b) else {
                continue;
            };
            let (Some(func), Some(pos)) = (binding.scope, binding.parameter) else {
                continue;
            };
            let (Some(def), Some(func_key)) =
                (facts.definitions.get(func), inputs.keys[path].get(func))
            else {
                continue;
            };
            let gid = (path, func, pos);
            groups.insert(gid);
            by_name.entry(def.name.as_str()).or_default().push(gid);
            by_key.entry(func_key.as_str()).or_default().push(gid);
        }
    }
    let mut out: BTreeMap<GroupId<'a>, ParamCompose> = BTreeMap::new();
    if groups.is_empty() {
        return out;
    }
    for (path2, (language, facts)) in inputs.files {
        if *language != Language::Go || !serves(path2) {
            continue;
        }
        let scope = FileScope::new(facts);
        for chain in &facts.chains {
            rb_frameworks::work::tick(1);
            let Some(last) = chain.links.last() else {
                continue;
            };
            if !by_name.contains_key(last.name.as_str()) {
                continue;
            }
            rb_frameworks::work::tick(1);
            let text = match &chain.root {
                Some(root) => format!("{root}.{}", last.name),
                None => last.name.clone(),
            };
            let found = inputs
                .index
                .handler_targets(path2, chain.enclosing, &text, last.span);
            let site = Evidence {
                path: (*path2).to_owned(),
                span: chain.span,
            };
            if found.len() == 1
                && let Some(gids) = by_key.get(found[0].as_str())
            {
                for &(file, func, pos) in gids {
                    let entry = out.entry((file, func, pos)).or_default();
                    entry.sites += 1;
                    // One level only: distinct prefixes are bounded by the call
                    // sites, so no prefix cap is needed (unlike mounts, which
                    // multiply across levels).
                    match last.args.get(pos as usize) {
                        Some(arg) => match fold_group_arg(&scope, Some(arg)) {
                            Some(prefix) => {
                                entry.composed.entry(prefix).or_default().push(site.clone())
                            }
                            None => entry.failed = true,
                        },
                        None => entry.failed = true,
                    }
                }
                continue;
            }
            // Ambiguous, unresolvable, or resolved elsewhere: a same-named group may
            // still be the target, so its unknown node stays (safe direction).
            let targets_group =
                found.is_empty() || found.iter().any(|k| by_key.contains_key(k.as_str()));
            if targets_group && let Some(gids) = by_name.get(last.name.as_str()) {
                for &gid in gids {
                    let entry = out.entry(gid).or_default();
                    entry.sites += 1;
                    entry.failed = true;
                }
            }
        }
    }
    // Groups no call site reached: the unknown node stays, as today.
    for gid in &groups {
        out.entry(*gid).or_default();
    }
    out
}

/// Join a composed prefix with a route path (the frameworks `join` semantics).
fn join_prefix(prefix: &str, path: &str) -> String {
    match (prefix.is_empty(), path.is_empty()) {
        (true, _) => path.to_owned(),
        (_, true) => prefix.to_owned(),
        _ => format!("{prefix}/{path}"),
    }
}

/// Whether a file's routes can serve: test code serves nothing (contract M9a D1).
fn serves(path: &str) -> bool {
    !rb_model::is_test_path(path)
}

/// Adds every endpoint with its edges; returns the route diagnostics and the endpoints, in key
/// order.
pub(crate) fn add_endpoints(
    b: &mut GraphBuilder,
    inputs: &Inputs<'_, '_>,
) -> anyhow::Result<(Vec<Diagnostic>, Vec<EndpointRef>)> {
    let mut diagnostics = Vec::new();
    // Every reader below (routes, routers and mounts, gin compositions, interface routes, gRPC
    // servers) sees production files only; gin call sites are filtered where they are found.
    let mut production: BTreeMap<&str, FileRoutes> = BTreeMap::new();
    for (&path, file) in inputs.routes {
        if serves(path) {
            production.insert(path, file.clone());
        } else if !file.routes.is_empty() {
            diagnostics.push(Diagnostic {
                kind: "routes_in_test_code".to_owned(),
                path: Some(path.to_owned()),
                span: None,
                detail: json!({ "routes": file.routes.len() }),
            });
        }
    }
    let inputs = &Inputs {
        routes: &production,
        ..*inputs
    };
    let mut composer = Composer::new(inputs);
    // gin parameter groups composed from their call sites (contract M6b S2).
    let param_composed = param_compositions(inputs);
    let mut endpoints: BTreeMap<String, Endpoint> = BTreeMap::new();
    let diag = |kind: &str, path: &str, span: Span, detail: serde_json::Value| Diagnostic {
        kind: kind.to_owned(),
        path: Some(path.to_owned()),
        span: Some(span),
        detail,
    };
    // Paths written as names fold across files, as client URLs do (contract M7c S1).
    let consts = Constants::new(inputs.files, inputs.index);
    let mut folder = rb_frameworks::fold::Folder::new(&consts);
    // Services with a `RepositoryRestConfigurer`, which can move Spring Data REST's base path
    // in code (contract M9b D7): their data-rest endpoints are capped, counted once. A
    // configurer is a Java or Kotlin class extending one, a Java method returning one (the
    // `@Bean` form, an anonymous `…Adapter` included; the annotation is not checked, as a cap
    // only lowers certainty), or a `RepositoryRestConfigurer.withConfig(…)` call (Spring Data
    // REST 3.1+), which is how a Kotlin `@Bean` with an inferred type is seen (M9b pass 1, E2).
    let configurer = |name: &str| {
        matches!(
            name.rsplit('.').next(),
            Some("RepositoryRestConfigurer" | "RepositoryRestConfigurerAdapter")
        )
    };
    let configured: BTreeSet<String> = inputs
        .files
        .iter()
        .filter(|(_, (language, facts))| {
            matches!(language, Language::Java | Language::Kotlin)
                && (facts.definitions.iter().any(|d| {
                    d.supertypes.iter().any(|s| configurer(&s.name))
                        || d.return_type.as_deref().is_some_and(configurer)
                }) || facts.chains.iter().any(|c| {
                    c.root.as_deref().and_then(|r| r.rsplit('.').next())
                        == Some("RepositoryRestConfigurer")
                        && c.links.first().is_some_and(|l| l.name == "withConfig")
                }))
        })
        .map(|(path, _)| (inputs.service_of)(path))
        .collect();
    let mut base_unread: BTreeSet<String> = BTreeSet::new();
    for (&path, file) in inputs.routes {
        for d in &file.diagnostics {
            diagnostics.push(diag(d.kind, path, d.span, json!({ "detail": d.detail })));
        }
        let service = (inputs.service_of)(path);
        let keys = &inputs.keys[path];
        // A router's identity in the repository: its file and its binding (amendment 10).
        let router_id = |binding: usize| {
            let facts = inputs.files[path].1;
            format!("{path}#{}", binding_identity(facts, binding))
        };
        for r in &file.routes {
            rb_frameworks::work::tick(1);
            // A name folds to a constant's value, or the route is counted dynamic.
            let folded: String;
            let route_path: &str = match &r.path_name {
                None => &r.path,
                Some(name) => {
                    let value = folder.fold_name(consts.ids[path], &name.text, name.span);
                    match value
                        .literal()
                        .filter(|_| matches!(value.origin, Origin::Constant | Origin::Literal))
                    {
                        Some(p) => {
                            folded = p;
                            &folded
                        }
                        None => {
                            diagnostics.push(diag(
                                "route_path_dynamic",
                                path,
                                r.span,
                                json!({ "detail": name.text }),
                            ));
                            continue;
                        }
                    }
                }
            };
            // Full paths with their certainty, the router when the prefix is unknown
            // (the path is then the tail below that router), and the call sites a
            // composed prefix comes from.
            let mut paths: Vec<(String, Certainty, Option<String>, Vec<Evidence>)> = Vec::new();
            match r.base {
                RouteBase::Root => match inputs.context_paths.get(&service) {
                    // Contract M9b D7: Spring Data REST resources exist only where the build
                    // names it, under the context and base paths, and are never `exact`.
                    ctx if r.framework == Framework::SpringDataRest => {
                        let Some(base) = inputs.data_rest.get(&service) else {
                            continue;
                        };
                        let mut full = match base {
                            Some(base) => format!("/{base}{route_path}"),
                            None => route_path.to_owned(),
                        };
                        if let Some(ctx) = ctx {
                            full = format!("{ctx}/{full}");
                        }
                        let certainty = if configured.contains(&service) {
                            if base_unread.insert(service.clone()) {
                                diagnostics.push(Diagnostic {
                                    kind: "data_rest_base_unread".to_owned(),
                                    path: Some(path.to_owned()),
                                    span: None,
                                    detail: json!({ "service": service }),
                                });
                            }
                            Certainty::Heuristic
                        } else {
                            Certainty::Synthetic
                        };
                        paths.push((full, certainty, None, Vec::new()));
                    }
                    Some(ctx) if r.framework == rb_frameworks::routes::Framework::Spring => {
                        paths.push((
                            format!("{ctx}/{route_path}"),
                            Certainty::Synthetic,
                            None,
                            Vec::new(),
                        ));
                    }
                    _ => paths.push((route_path.to_owned(), Certainty::Exact, None, Vec::new())),
                },
                RouteBase::Unknown(receiver) => {
                    let mut unknown = true;
                    if r.framework == Framework::Gin
                        && let Some(binding) = inputs.files[path].1.bindings.get(receiver)
                        && let (Some(func), Some(pos)) = (binding.scope, binding.parameter)
                        && let Some(comp) = param_composed.get(&(path, func, pos))
                    {
                        for (prefix, sites) in &comp.composed {
                            paths.push((
                                join_prefix(prefix, route_path),
                                Certainty::Heuristic,
                                None,
                                sites.clone(),
                            ));
                        }
                        unknown = comp.sites == 0 || comp.failed;
                    }
                    if unknown {
                        paths.push((
                            route_path.to_owned(),
                            Certainty::Heuristic,
                            Some(router_id(receiver)),
                            Vec::new(),
                        ));
                    }
                }
                RouteBase::Router(i) => {
                    let (prefixes, _) = composer.prefixes((path, i), 0);
                    for (p, cross) in &prefixes.known {
                        let certainty = if *cross {
                            Certainty::Heuristic
                        } else {
                            Certainty::Exact
                        };
                        paths.push((format!("{p}/{route_path}"), certainty, None, Vec::new()));
                    }
                    if prefixes.unknown {
                        // An anonymous default-exported router is identified by the
                        // file (contract M6b S3).
                        let router = match file.routers[i].binding {
                            Some(binding) => router_id(binding),
                            None => format!("{path}#default"),
                        };
                        paths.push((
                            route_path.to_owned(),
                            Certainty::Heuristic,
                            Some(router),
                            Vec::new(),
                        ));
                    }
                }
            }
            // A constant's value, not a path written at the registration (contract M7c S1).
            if r.path_name.is_some() {
                for p in &mut paths {
                    p.1 = p.1.min(Certainty::Heuristic);
                }
            }
            let method = r.method.clone();
            if paths.iter().any(|(_, _, router, _)| router.is_some()) {
                diagnostics.push(diag(
                    "route_prefix_unknown",
                    path,
                    r.span,
                    json!({
                        "framework": r.framework.as_str(),
                        "method": method,
                        "path": norm_path(route_path),
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
            for (full, path_certainty, router, composed_from) in paths {
                let norm = norm_path(&full);
                let key = match &router {
                    Some(router) => prefix_unknown_key(&method, &norm, &service, router),
                    None => endpoint_key(&method, &norm, &service),
                };
                let e = endpoints.entry(key).or_insert_with(|| Endpoint {
                    method: method.clone(),
                    path: norm.clone(),
                    service: service.clone(),
                    router: router.clone(),
                    ..Endpoint::default()
                });
                e.frameworks.insert(r.framework.as_str());
                let folds = r.framework == rb_frameworks::routes::Framework::Express
                    && matches!(r.base, RouteBase::Router(i) if !file.routers[i].case_sensitive);
                if folds {
                    e.case_folds = true;
                } else {
                    e.case_exact = true;
                }
                match &r.host {
                    Some(host) => {
                        e.hosts.insert(host.clone());
                    }
                    None => e.any_host = true,
                }
                e.registrations.push(Registration {
                    path_certainty,
                    handler: handler.clone(),
                    candidates,
                    at: Evidence {
                        path: path.to_owned(),
                        span: r.span,
                    },
                    composed_from,
                    from_spec: false,
                });
            }
        }
    }
    add_spec_endpoints(b, &mut endpoints, &mut diagnostics, inputs);
    let added = endpoints
        .into_iter()
        .map(|(key, e)| {
            let unread = inputs.config_unread.contains(&e.service);
            add_endpoint(b, key, e, unread)
        })
        .collect::<anyhow::Result<_>>()?;
    Ok((diagnostics, added))
}

/// One declaration of a proto service: its spec file, the project holding it (`None` outside
/// every project) and the service as read (contract M7a S3).
struct Declared<'s> {
    spec: &'s str,
    holder: Option<String>,
    svc: rb_frameworks::spec::ProtoService,
}

/// Spec operations and proto RPCs as endpoint registrations under the shared key space, so a
/// code route for the same method+path joins its handler for free (contract M6f S3/S4).
/// Unreadable specs are counted (`openapi_unread`); specs outside any project stay
/// endpoint-less and counted; operations with no code route are kept, `synthetic` (proto RPCs
/// `exact`), unhandled and counted per registration.
fn add_spec_endpoints(
    b: &GraphBuilder,
    endpoints: &mut BTreeMap<String, Endpoint>,
    diagnostics: &mut Vec<Diagnostic>,
    inputs: &Inputs<'_, '_>,
) {
    let diag = |kind: &str, path: &str, span: Span, detail: serde_json::Value| Diagnostic {
        kind: kind.to_owned(),
        path: Some(path.to_owned()),
        span: Some(span),
        detail,
    };
    let mut specs: Vec<(&str, &String)> = inputs.specs.iter().map(|(p, t)| (*p, t)).collect();
    specs.sort();
    // Interface mappings by service (contract M6f S4b): a spec operation matching one is
    // handled by the declaration and its implementors.
    let mut interfaces: BTreeMap<String, Vec<(&str, &InterfaceRoute)>> = BTreeMap::new();
    for (path, file) in inputs.routes {
        rb_frameworks::work::tick(1);
        if file.interface_routes.is_empty() {
            continue;
        }
        let service = (inputs.service_of)(path);
        for route in &file.interface_routes {
            rb_frameworks::work::tick(1);
            interfaces
                .entry(service.clone())
                .or_default()
                .push((path, route));
        }
    }
    // gRPC attribution (contract M7a S3): every proto service declared anywhere (inside a
    // project or not), by `(package, name)`, and the projects registering a server for it.
    let mut declared: BTreeMap<(String, String), Vec<Declared<'_>>> = BTreeMap::new();
    for (spec_path, text) in &specs {
        if !spec_path.ends_with(".proto") {
            continue;
        }
        let holder = (inputs.has_project)(spec_path).then(|| (inputs.service_of)(spec_path));
        for svc in rb_frameworks::spec::proto_services(text) {
            rb_frameworks::work::tick(1);
            declared
                .entry((svc.package.clone(), svc.name.clone()))
                .or_default()
                .push(Declared {
                    spec: spec_path,
                    holder: holder.clone(),
                    svc,
                });
        }
    }
    let mut packages_of: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (package, name) in declared.keys() {
        packages_of.entry(name).or_default().push(package);
    }
    let mut registered: BTreeMap<(String, String), BTreeMap<String, Vec<Evidence>>> =
        BTreeMap::new();
    for (path, file) in inputs.routes {
        for server in &file.grpc_servers {
            rb_frameworks::work::tick(1);
            match packages_of.get(server.service.as_str()).map(Vec::as_slice) {
                // No committed proto declares it: nothing to attribute.
                None | Some([]) => {}
                Some([package]) => registered
                    .entry(((*package).to_owned(), server.service.clone()))
                    .or_default()
                    .entry((inputs.service_of)(path))
                    .or_default()
                    .push(Evidence {
                        path: (*path).to_owned(),
                        span: server.span,
                    }),
                Some(several) => diagnostics.push(diag(
                    "grpc_registration_ambiguous",
                    path,
                    server.span,
                    json!({ "service": server.service, "packages": several }),
                )),
            }
        }
    }
    for (spec_path, text) in specs {
        rb_frameworks::work::tick(1);
        if !(inputs.has_project)(spec_path) {
            diagnostics.push(diag(
                "spec_outside_project",
                spec_path,
                [0, 0, 0, 0],
                json!({}),
            ));
            continue;
        }
        let service = (inputs.service_of)(spec_path);
        if spec_path.ends_with(".proto") {
            for svc in rb_frameworks::spec::proto_services(text) {
                // Registered services are attributed to their servers below; a service several
                // projects declare and nobody registers is counted below (contract M7a S3).
                let id = (svc.package.clone(), svc.name.clone());
                let holders: BTreeSet<&str> = declared[&id]
                    .iter()
                    .filter_map(|d| d.holder.as_deref())
                    .collect();
                if registered.contains_key(&id) || holders.len() > 1 {
                    continue;
                }
                for rpc in svc.rpcs {
                    let target = if svc.package.is_empty() {
                        format!("{}/{}", svc.name, rpc.name)
                    } else {
                        format!("{}.{}/{}", svc.package, svc.name, rpc.name)
                    };
                    let key = grpc_key(&svc.package, &svc.name, &rpc.name, &service);
                    let e = endpoints.entry(key).or_insert_with(|| Endpoint {
                        method: "GRPC".into(),
                        path: target,
                        service: service.clone(),
                        router: None,
                        ..Endpoint::default()
                    });
                    e.frameworks.insert("grpc");
                    e.any_host = true;
                    e.registrations.push(Registration {
                        path_certainty: Certainty::Exact,
                        handler: Vec::new(),
                        candidates: 0,
                        at: Evidence {
                            path: spec_path.to_owned(),
                            span: rpc.span,
                        },
                        composed_from: Vec::new(),
                        from_spec: true,
                    });
                }
            }
            continue;
        }
        let ops = if spec_path.ends_with(".json") {
            rb_frameworks::spec::openapi_json(text)
        } else {
            rb_frameworks::spec::openapi_yaml(text)
        };
        let Some(ops) = ops else {
            diagnostics.push(diag("openapi_unread", spec_path, [0, 0, 0, 0], json!({})));
            continue;
        };
        for op in ops {
            rb_frameworks::work::tick(1);
            let norm = norm_path(&op.path);
            let key = endpoint_key(&op.method, &norm, &service);
            let e = endpoints.entry(key).or_insert_with(|| Endpoint {
                method: op.method.clone(),
                path: norm.clone(),
                service: service.clone(),
                router: None,
                ..Endpoint::default()
            });
            e.frameworks.insert("openapi");
            if !op.operation.is_empty() {
                e.operations.insert(op.operation.clone());
            }
            e.any_host = true;
            // A stated document, not a served route (contract M9a D2): the endpoint is `exact`
            // only when a code registration joins it.
            e.registrations.push(Registration {
                path_certainty: Certainty::Synthetic,
                handler: Vec::new(),
                candidates: 0,
                at: Evidence {
                    path: spec_path.to_owned(),
                    span: op.span,
                },
                composed_from: Vec::new(),
                from_spec: true,
            });
            // Interface mappings the operation matches (contract M6f S4b): handled by the
            // declaration (`exact`) and each implementor (`heuristic`).
            if let Some(routes) = interfaces.get(&service) {
                for (ipath, route) in routes {
                    rb_frameworks::work::tick(1);
                    if !route.methods.iter().any(|m| m == &op.method)
                        || norm_path(&route.path) != norm
                    {
                        continue;
                    }
                    let iface =
                        NodeKey::new(NodeKind::Symbol, inputs.keys[*ipath][route.def].clone());
                    let mut handler = vec![(iface.clone(), Certainty::Exact, None)];
                    for implementor in b.implementors(&iface) {
                        if implementor == iface {
                            continue;
                        }
                        handler.push((implementor, Certainty::Heuristic, None));
                    }
                    e.registrations.push(Registration {
                        path_certainty: Certainty::Exact,
                        handler,
                        candidates: 0,
                        at: Evidence {
                            path: (*ipath).to_owned(),
                            span: route.span,
                        },
                        composed_from: Vec::new(),
                        from_spec: true,
                    });
                }
            }
        }
    }
    for ((package, name), decls) in &declared {
        let qualified = if package.is_empty() {
            name.clone()
        } else {
            format!("{package}.{name}")
        };
        let Some(projects) = registered.get(&(package.clone(), name.clone())) else {
            let mut holders: Vec<&str> = decls.iter().filter_map(|d| d.holder.as_deref()).collect();
            holders.sort_unstable();
            holders.dedup();
            if holders.len() > 1 {
                let mut declared_in: Vec<&str> = decls.iter().map(|d| d.spec).collect();
                declared_in.dedup();
                diagnostics.push(diag(
                    "grpc_implementer_unknown",
                    decls[0].spec,
                    decls[0].svc.span,
                    json!({ "service": qualified, "declared_in": declared_in }),
                ));
            }
            continue;
        };
        for (project, sites) in projects {
            // The project's own declaring copy is the evidence (`exact`); else the first
            // declaration, the attribution resting on the registration's name (`heuristic`).
            let own = decls
                .iter()
                .find(|d| d.holder.as_deref() == Some(project.as_str()));
            let (decl, certainty) = match own {
                Some(d) => (d, Certainty::Exact),
                None => (&decls[0], Certainty::Heuristic),
            };
            for rpc in &decl.svc.rpcs {
                rb_frameworks::work::tick(1);
                let key = grpc_key(package, name, &rpc.name, project);
                let e = endpoints.entry(key).or_insert_with(|| Endpoint {
                    method: "GRPC".into(),
                    path: format!("{qualified}/{}", rpc.name),
                    service: project.clone(),
                    router: None,
                    ..Endpoint::default()
                });
                e.frameworks.insert("grpc");
                e.any_host = true;
                e.registrations.push(Registration {
                    path_certainty: certainty,
                    handler: Vec::new(),
                    candidates: 0,
                    at: Evidence {
                        path: decl.spec.to_owned(),
                        span: rpc.span,
                    },
                    composed_from: sites.clone(),
                    from_spec: true,
                });
            }
        }
    }
    for e in endpoints.values() {
        if e.registrations.iter().all(|r| r.handler.is_empty()) {
            for r in e.registrations.iter().filter(|r| r.from_spec) {
                let detail = if e.method == "GRPC" {
                    json!({ "target": e.path })
                } else {
                    json!({ "method": e.method, "path": e.path })
                };
                diagnostics.push(diag(
                    "spec_operation_unhandled",
                    &r.at.path,
                    r.at.span,
                    detail,
                ));
            }
        }
    }
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
/// handler's spans. `unread`: its service has Spring configuration M4a does not read.
fn add_endpoint(
    b: &mut GraphBuilder,
    key: String,
    e: Endpoint,
    unread: bool,
) -> anyhow::Result<EndpointRef> {
    let node = NodeKey::new(NodeKind::Endpoint, key.clone());
    let mut attrs = Attrs::new();
    attrs.insert("method".into(), e.method.clone().into());
    attrs.insert("path".into(), e.path.clone().into());
    attrs.insert("service".into(), e.service.clone().into());
    attrs.insert(
        "frameworks".into(),
        e.frameworks.iter().copied().collect::<Vec<_>>().into(),
    );
    if !e.operations.is_empty() {
        attrs.insert(
            "operations".into(),
            e.operations.iter().cloned().collect::<Vec<_>>().into(),
        );
    }
    if let Some(router) = &e.router {
        attrs.insert("prefix_unknown".into(), true.into());
        attrs.insert("router".into(), router.clone().into());
    }
    let hosts: Vec<String> = if e.any_host {
        Vec::new()
    } else {
        e.hosts.iter().cloned().collect()
    };
    match hosts.as_slice() {
        [] => {}
        [host] => {
            attrs.insert("host".into(), host.clone().into());
        }
        several => {
            attrs.insert("host".into(), several.to_vec().into());
        }
    }
    if unread {
        attrs.insert("prefix_maybe_incomplete".into(), true.into());
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
        .flat_map(|r| std::iter::once(r.at.clone()).chain(r.composed_from.iter().cloned()))
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
            h.evidence.extend(r.composed_from.iter().cloned());
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
        prefix_unknown: e.router.is_some(),
        certainty: exposes,
        hosts,
        prefix_maybe_incomplete: unread,
        operations: e.operations.into_iter().collect(),
        fold_case: e.case_folds && !e.case_exact,
    })
}

#[cfg(test)]
mod tests {
    use rb_model::Graph;

    use super::*;

    /// Contract M7i S1 (stress F32): an endpoint folds case when every code registration is on
    /// an Express router without a case-sensitive opt-in; a router that opts in, or a
    /// non-Express framework, keeps exact case.
    #[test]
    fn express_endpoints_fold_case_unless_their_router_opts_in() {
        let app = "import express, { Router } from \"express\";\n\
                   const app = express();\nconst strict = Router({ caseSensitive: true });\n\
                   app.use(\"/strict\", strict);\n\
                   app.get(\"/api/notifications/:id\", h);\nstrict.get(\"/x\", h);\n";
        let fastapi = "from fastapi import FastAPI\napp = FastAPI()\n\n@app.get(\"/items\")\ndef items():\n    return []\n";
        let built = build(
            &[
                ("web/src/app.ts", Language::TypeScript, app),
                ("api/main.py", Language::Python, fastapi),
            ],
            &[],
        );
        let folds: Vec<(String, bool)> = built
            .endpoints
            .iter()
            .map(|e| (e.key.clone(), e.fold_case))
            .collect();
        assert_eq!(
            folds,
            [
                ("HTTP GET /api/notifications/{} @web".to_owned(), true),
                ("HTTP GET /items @api".to_owned(), false),
                ("HTTP GET /strict/x @web".to_owned(), false),
            ]
        );
    }

    /// No service depends on Spring Data REST.
    static NO_DATA_REST: BTreeMap<String, Option<String>> = BTreeMap::new();

    struct Built {
        graph: Graph,
        diagnostics: Vec<Diagnostic>,
        endpoints: Vec<EndpointRef>,
    }

    /// Indexes `files` (path, language, source) with every file in the service named by its
    /// first path segment, and `context_paths` per service.
    fn build(files: &[(&str, Language, &str)], context_paths: &[(&str, &str)]) -> Built {
        build_with_specs(files, &[], context_paths)
    }

    fn build_with_specs(
        files: &[(&str, Language, &str)],
        specs: &[(&str, &str)],
        context_paths: &[(&str, &str)],
    ) -> Built {
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
        let specs: Vec<(String, String)> = specs
            .iter()
            .map(|(p, t)| ((*p).to_owned(), (*t).to_owned()))
            .collect();
        build_from(
            &facts,
            &specs,
            context_paths,
            &BTreeMap::new(),
            &[],
            &NO_DATA_REST,
        )
    }

    fn build_from(
        facts: &[(String, Language, FileFacts)],
        specs: &[(String, String)],
        context_paths: &[(&str, &str)],
        exact: &BTreeMap<(&str, Span), String>,
        config_unread: &[&str],
        data_rest: &BTreeMap<String, Option<String>>,
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
        for (path, _) in specs {
            // Spec files are manifest files (File nodes with `IN_SERVICE` in the real
            // pipeline): the harness mirrors that so spec evidence validates.
            b.add_node(
                NodeKey::new(NodeKind::File, path.as_str()),
                path.as_str(),
                Attrs::new(),
            )
            .unwrap();
            let s = NodeKey::new(NodeKind::Service, service_of(path));
            if !b.has_node(&s) {
                b.add_node(s.clone(), service_of(path), Attrs::new())
                    .unwrap();
            }
            b.add_edge(NewEdge::new(
                NodeKey::new(NodeKind::File, path.as_str()),
                EdgeKind::InService,
                s,
                Provenance::Manifest,
                Certainty::Exact,
            ))
            .unwrap();
        }
        let context_paths: BTreeMap<String, String> = context_paths
            .iter()
            .map(|(s, c)| ((*s).to_owned(), (*c).to_owned()))
            .collect();
        let config_unread: BTreeSet<String> =
            config_unread.iter().map(|s| (*s).to_owned()).collect();
        // Tier B resolution, as production runs it before this stage: calls, types and
        // `IMPLEMENTS` edges exist for handler roll-down (contract M6f S4b).
        let mut tier_b = crate::resolve::add_edges(&mut b, &index, &BTreeSet::new()).unwrap();
        let spec_texts: BTreeMap<&str, String> =
            specs.iter().map(|(p, t)| (p.as_str(), t.clone())).collect();
        let has_project = |p: &str| {
            let service = p.split('/').next().unwrap_or(p);
            by_path
                .keys()
                .any(|f| f.split('/').next().unwrap_or(*f) == service)
        };
        let inputs = Inputs {
            files: &by_path,
            routes: &routes,
            keys: &keys,
            index: &index,
            exact_handlers: exact,
            service_of: &service_of,
            context_paths: &context_paths,
            config_unread: &config_unread,
            data_rest,
            specs: &spec_texts,
            has_project: &has_project,
        };
        let (mut diagnostics, endpoints) = add_endpoints(&mut b, &inputs).unwrap();
        diagnostics.splice(0..0, tier_b.drain(..));
        Built {
            graph: b.finish().unwrap(),
            diagnostics,
            endpoints,
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
        // Tier B resolution runs in this harness as in production, so its
        // `unresolved_references` counts ride along; no route diagnostic.
        assert!(
            diagnostics(&built)
                .iter()
                .all(|d| d.starts_with("unresolved_references")),
            "{:?}",
            diagnostics(&built)
        );
        let node = g
            .nodes()
            .iter()
            .find(|n| n.kind == NodeKind::Endpoint)
            .unwrap();
        assert_eq!(node.name, "GET /health");
        assert_eq!(node.attrs["frameworks"], json!(["express"]));
    }

    /// Contract M4b S19 (M4a item 1): a mount of a default import resolves through the
    /// module's default export, one level; a default export that is no router, or is itself an
    /// import, leaves the prefix unknown.
    #[test]
    fn default_imported_routers_are_mounted() {
        let app = "import express from \"express\";\nimport health from \"./routes/health\";\nimport { named } from \"./routes/named\";\nimport other from \"./routes/other\";\nimport again from \"./routes/again\";\nconst app = express();\napp.use(\"/ops\", health);\napp.use(\"/n\", named);\napp.use(\"/o\", other);\napp.use(\"/a\", again);\n";
        let health = "import { Router } from \"express\";\nconst health = Router();\nhealth.get(\"/health\", (_q, s) => s.send(\"ok\"));\nexport default health;\n";
        let named = "import { Router } from \"express\";\nexport const named = Router();\nnamed.get(\"/x\", (_q, s) => s.end());\n";
        // The default export is a function, and the module's router is not exported at all.
        let other = "import { Router } from \"express\";\nconst r = Router();\nr.get(\"/y\", (_q, s) => s.end());\nexport default function make() { return 1; }\n";
        // A default re-export of another module's router: one level only.
        let again = "import health from \"./health\";\nexport default health;\n";
        let built = build(
            &[
                ("api/src/app.ts", Language::TypeScript, app),
                ("api/src/routes/health.ts", Language::TypeScript, health),
                ("api/src/routes/named.ts", Language::TypeScript, named),
                ("api/src/routes/other.ts", Language::TypeScript, other),
                ("api/src/routes/again.ts", Language::TypeScript, again),
            ],
            &[],
        );
        assert_eq!(
            edges(&built)
                .iter()
                .filter(|e| e.starts_with("EXPOSES"))
                .cloned()
                .collect::<Vec<_>>(),
            [
                "EXPOSES api -> HTTP GET /n/x @api [heuristic]",
                "EXPOSES api -> HTTP GET /ops/health @api [heuristic]",
                "EXPOSES api -> HTTP GET /y @api ?router=api/src/routes/other.ts#r [heuristic]",
            ]
        );
    }

    /// Contract M7a S2 (stress F18, fastapi-realworld): FastAPI composes through the shared
    /// router composer. `include_router(module.router, prefix=…)` through a from-imported
    /// submodule and an aliased router import compose across files (`heuristic`, as Express
    /// cross-file mounts); in-file composition stays `exact`; a router never included and a
    /// non-literal prefix leave the prefix unknown, so unrelated routers never merge under a
    /// root path.
    #[test]
    fn fastapi_includes_compose_across_modules_and_unknown_prefixes_stay_unknown() {
        let main = "from fastapi import FastAPI\nfrom app.api import api\n\napp = FastAPI()\n\n@app.get(\"/health\")\ndef health():\n    pass\n\napp.include_router(api.router, prefix=\"/api\")\n";
        let api = "from fastapi import APIRouter\nfrom app.api.routes import articles, comments\nfrom app.api.routes.users import router as users_router\n\nrouter = APIRouter()\nrouter.include_router(articles.router, prefix=\"/articles\")\nrouter.include_router(comments.router, prefix=\"/articles/{slug}/comments\")\nrouter.include_router(users_router, prefix=\"/users\")\n";
        let articles = "from fastapi import APIRouter\n\nrouter = APIRouter()\n\n@router.delete(\"/{slug}\")\ndef delete_article(slug: str):\n    pass\n";
        let comments = "from fastapi import APIRouter\n\nrouter = APIRouter()\n\n@router.delete(\"/{comment_id}\")\ndef delete_comment(comment_id: int):\n    pass\n";
        let users = "from fastapi import APIRouter\n\nrouter = APIRouter(prefix=\"/me\")\n\n@router.get(\"\")\ndef me():\n    pass\n";
        let lone = "from fastapi import APIRouter\n\nlone = APIRouter()\n\n@lone.get(\"/l\")\ndef l():\n    pass\n";
        let dynamic = "from fastapi import FastAPI, APIRouter\nfrom app.core import settings\n\nsite = FastAPI()\nr = APIRouter()\n\n@r.get(\"/d\")\ndef d():\n    pass\n\nsite.include_router(r, prefix=settings.API_PREFIX)\n";
        let built = build(
            &[
                ("svc/app/main.py", Language::Python, main),
                ("svc/app/api/api.py", Language::Python, api),
                ("svc/app/api/routes/articles.py", Language::Python, articles),
                ("svc/app/api/routes/comments.py", Language::Python, comments),
                ("svc/app/api/routes/users.py", Language::Python, users),
                ("svc/app/lone.py", Language::Python, lone),
                ("svc/app/dynamic.py", Language::Python, dynamic),
                ("svc/app/__init__.py", Language::Python, ""),
                ("svc/app/api/__init__.py", Language::Python, ""),
                // A package router of the same name: `articles.router` names the submodule's.
                (
                    "svc/app/api/routes/__init__.py",
                    Language::Python,
                    "from fastapi import APIRouter\n\nrouter = APIRouter()\n\n@router.get(\"/pkg\")\ndef pkg():\n    pass\n",
                ),
            ],
            &[],
        );
        let mut exposes: Vec<String> = edges(&built)
            .into_iter()
            .filter(|e| e.starts_with("EXPOSES"))
            .collect();
        exposes.sort();
        assert_eq!(
            exposes,
            [
                "EXPOSES svc -> HTTP DELETE /api/articles/{} @svc [heuristic]",
                "EXPOSES svc -> HTTP DELETE /api/articles/{}/comments/{} @svc [heuristic]",
                "EXPOSES svc -> HTTP GET /api/users/me @svc [heuristic]",
                "EXPOSES svc -> HTTP GET /d @svc ?router=svc/app/dynamic.py#r.r [heuristic]",
                "EXPOSES svc -> HTTP GET /health @svc [exact]",
                "EXPOSES svc -> HTTP GET /l @svc ?router=svc/app/lone.py#lone.lone [heuristic]",
                "EXPOSES svc -> HTTP GET /pkg @svc ?router=svc/app/api/routes/__init__.py#router.router [heuristic]",
            ]
        );
    }

    /// Contract M6d S9 through M7a S2: in-file FastAPI composition stays `exact` through the
    /// shared composer: an `APIRouter`'s own prefix, `include_router` prefixes, and a router
    /// included twice serving both prefixes.
    #[test]
    fn fastapi_in_file_includes_stay_exact() {
        let src = "from fastapi import FastAPI, APIRouter\n\napp = FastAPI()\nrouter = APIRouter(prefix=\"/api\")\nv1 = APIRouter()\n\n@app.get(\"/health\")\ndef health():\n    pass\n\n@router.post(\"/login\")\ndef login():\n    pass\n\n@v1.get(\"/users\")\ndef users():\n    pass\n\n@v1.delete(\"/users/{id}\")\ndef drop_user():\n    pass\n\napp.include_router(router)\napp.include_router(v1, prefix=\"/v1\")\napp.include_router(router, prefix=\"/v2\")\n";
        let built = build(&[("svc/main.py", Language::Python, src)], &[]);
        let mut exposes: Vec<String> = edges(&built)
            .into_iter()
            .filter(|e| e.starts_with("EXPOSES"))
            .collect();
        exposes.sort();
        assert_eq!(
            exposes,
            [
                "EXPOSES svc -> HTTP DELETE /v1/users/{} @svc [exact]",
                "EXPOSES svc -> HTTP GET /health @svc [exact]",
                "EXPOSES svc -> HTTP GET /v1/users @svc [exact]",
                "EXPOSES svc -> HTTP POST /api/login @svc [exact]",
                "EXPOSES svc -> HTTP POST /v2/api/login @svc [exact]",
            ]
        );
    }

    /// Contract M7c S1 (stress F20, bookcars): an Express path written as a name folds through
    /// the cross-file constant lookup — a member of a default-imported config object, an
    /// in-file constant — and is `heuristic` (a constant's value, not a written path); a name
    /// that does not fold is counted as `route_path_dynamic`, never dropped silently.
    #[test]
    fn express_paths_written_as_names_fold_or_are_counted() {
        let app = "import express from \"express\";\nimport carRoutes from \"./routes/carRoutes\";\nconst app = express();\nconst HEALTH = \"/health\";\napp.get(HEALTH, (q, s) => s.end());\napp.use(\"/\", carRoutes);\n";
        let car = "import express from \"express\";\nimport routeNames from \"../config/carRoutes.config\";\nconst routes = express.Router();\nroutes.route(routeNames.create).post(create);\nroutes.get(routeNames.byId, byId);\nroutes.get(process.env.ROUTE, dyn);\nexport default routes;\nfunction create() {}\nfunction byId() {}\nfunction dyn() {}\n";
        let config = "const routes = {\n  create: \"/api/create-car\",\n  byId: \"/api/car/:id\",\n};\nexport default routes;\n";
        let built = build(
            &[
                ("svc/src/app.ts", Language::TypeScript, app),
                ("svc/src/routes/carRoutes.ts", Language::TypeScript, car),
                (
                    "svc/src/config/carRoutes.config.ts",
                    Language::TypeScript,
                    config,
                ),
            ],
            &[],
        );
        let mut exposes: Vec<String> = edges(&built)
            .into_iter()
            .filter(|e| e.starts_with("EXPOSES"))
            .collect();
        exposes.sort();
        assert_eq!(
            exposes,
            [
                "EXPOSES svc -> HTTP GET /api/car/{} @svc [heuristic]",
                "EXPOSES svc -> HTTP GET /health @svc [heuristic]",
                "EXPOSES svc -> HTTP POST /api/create-car @svc [heuristic]",
            ]
        );
        let dynamic: Vec<String> = diagnostics(&built)
            .into_iter()
            .filter(|d| d.starts_with("route_path_dynamic"))
            .collect();
        assert_eq!(dynamic.len(), 1, "{dynamic:?}");
        assert!(dynamic[0].contains("process.env.ROUTE"), "{dynamic:?}");
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
                "EXPOSES svc -> HTTP GET /l @svc ?router=svc/r.ts#lone [heuristic]",
                "EXPOSES svc -> HTTP GET /r @svc ?router=svc/r.ts#a [heuristic]",
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

    /// Fork F3 (contract amendment 10): an endpoint whose prefix is unknown carries its router's
    /// identity in its key, so distinct routes on different routers never merge into one node:
    /// two Express routers mounted nowhere and two gin groups passed as parameters each
    /// registering `GET /` give one endpoint per router, and a known `POST /login` stays apart
    /// from a `POST /login` under an unknown prefix (gin realworld's measured shapes).
    #[test]
    fn prefix_unknown_endpoints_are_keyed_by_their_router() {
        let express = "import { Router } from \"express\";\nexport const a = Router();\nexport const b = Router();\na.get(\"/\", (q, s) => s.end());\nb.get(\"/\", (q, s) => s.end());\n";
        let gin = "package api\nimport \"github.com/gin-gonic/gin\"\nfunc main() {\n\tr := gin.Default()\n\tr.POST(\"/login\", login)\n}\nfunc ArticlesRegister(router *gin.RouterGroup) {\n\trouter.GET(\"/\", list)\n}\nfunc UsersRegister(router *gin.RouterGroup) {\n\trouter.GET(\"/\", retrieve)\n\trouter.POST(\"/login\", login)\n}\nfunc list() {}\nfunc retrieve() {}\nfunc login() {}\n";
        let built = build(
            &[
                ("svc/r.ts", Language::TypeScript, express),
                ("svc/api.go", Language::Go, gin),
            ],
            &[],
        );
        let mut exposes: Vec<String> = edges(&built)
            .into_iter()
            .filter(|e| e.starts_with("EXPOSES"))
            .collect();
        exposes.sort();
        assert_eq!(
            exposes,
            [
                "EXPOSES svc -> HTTP GET / @svc ?router=svc/api.go#ArticlesRegister.router [heuristic]",
                "EXPOSES svc -> HTTP GET / @svc ?router=svc/api.go#UsersRegister.router [heuristic]",
                "EXPOSES svc -> HTTP GET / @svc ?router=svc/r.ts#a [heuristic]",
                "EXPOSES svc -> HTTP GET / @svc ?router=svc/r.ts#b [heuristic]",
                "EXPOSES svc -> HTTP POST /login @svc ?router=svc/api.go#UsersRegister.router [heuristic]",
                "EXPOSES svc -> HTTP POST /login @svc [exact]",
            ]
        );
        // Each node names its router; a known-prefix node has no `router`.
        for n in built
            .graph
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::Endpoint)
        {
            match n.key.split_once(" ?router=") {
                Some((_, router)) => {
                    assert_eq!(n.attrs["router"], router);
                    assert_eq!(n.attrs["prefix_unknown"], true);
                }
                None => assert!(!n.attrs.contains_key("router"), "{n:?}"),
            }
        }
        // Each `HANDLED_BY` leaves its own node: no node has two handlers.
        let handled: Vec<String> = edges(&built)
            .into_iter()
            .filter(|e| e.starts_with("HANDLED_BY"))
            .collect();
        let sources: BTreeSet<&str> = handled
            .iter()
            .map(|e| e.split(" -> ").next().unwrap())
            .collect();
        assert_eq!(sources.len(), handled.len(), "{handled:#?}");
    }

    /// F3 leaves every known-prefix key byte-identical to D1's format: an app route, a router
    /// mounted in the same file, a gin engine and group, and a Spring mapping.
    #[test]
    fn known_prefix_keys_are_unchanged() {
        let express = "import express, { Router } from \"express\";\nconst app = express();\nconst r = Router();\napp.use(\"/api\", r);\napp.get(\"/\", (q, s) => s.end());\nr.get(\"/x/:id\", (q, s) => s.end());\n";
        let gin = "package main\nimport \"github.com/gin-gonic/gin\"\nfunc main() {\n\tr := gin.Default()\n\tg := r.Group(\"/billing\")\n\tg.POST(\"/pay\", pay)\n}\nfunc pay() {}\n";
        let spring = "package p;\nimport org.springframework.web.bind.annotation.*;\n@RestController\n@RequestMapping(\"/api/v1\")\nclass C {\n  @PostMapping(\"/login\")\n  String login() { return \"\"; }\n}\n";
        let built = build(
            &[
                ("web/app.ts", Language::TypeScript, express),
                ("billing/main.go", Language::Go, gin),
                ("auth/C.java", Language::Java, spring),
            ],
            &[],
        );
        let keys: BTreeSet<String> = built
            .graph
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::Endpoint)
            .map(|n| n.key.clone())
            .collect();
        assert_eq!(
            keys,
            BTreeSet::from(
                [
                    "HTTP GET / @web",
                    "HTTP GET /api/x/{} @web",
                    "HTTP POST /billing/pay @billing",
                    "HTTP POST /api/v1/login @auth",
                ]
                .map(str::to_owned)
            )
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
                "EXPOSES svc -> HTTP GET /deep @svc ?router=svc/r.ts#r5000 [heuristic]",
                "EXPOSES svc -> HTTP GET /s/s/s/shallow @svc [exact]",
            ]
        );
    }

    /// Composition (the route pack and the endpoint stage) is linear in the mount chain: four
    /// times the chain makes at most five times the work (linear is four, quadratic sixteen).
    /// Parsing and the M3a index are built outside the counted part; the work is counted, not
    /// timed, so machine load cannot fail it (contract M4a amendment 12, Q4).
    #[test]
    fn composition_grows_linearly() {
        let work = |n: usize| {
            let src = mount_chain(n);
            let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
            let by_path = BTreeMap::from([("svc/r.ts", (Language::TypeScript, &facts))]);
            let keys = BTreeMap::from([("svc/r.ts", rb_syntax::symbol_keys("svc/r.ts", &facts))]);
            let go = BTreeMap::new();
            let index = Index::new(&by_path, &go, &keys);
            let service_of = |_: &str| "svc".to_owned();
            let (exact, context, unread) = (BTreeMap::new(), BTreeMap::new(), BTreeSet::new());
            let mut b = GraphBuilder::new();
            b.add_node(NodeKey::new(NodeKind::Service, "svc"), "svc", Attrs::new())
                .unwrap();
            b.add_node(
                NodeKey::new(NodeKind::File, "svc/r.ts"),
                "r.ts",
                Attrs::new(),
            )
            .unwrap();
            rb_frameworks::work::take();
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
                config_unread: &unread,
                data_rest: &NO_DATA_REST,
                specs: &BTreeMap::new(),
                has_project: &|_: &str| true,
            };
            let (_, endpoints) = add_endpoints(&mut b, &inputs).unwrap();
            assert_eq!(endpoints.len(), 2);
            rb_frameworks::work::take()
        };
        let (small, large) = (work(1000), work(4000));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }

    /// n routers mounted on an app under 5 prefixes each, and a cycle of `use` mounts between
    /// them (stress F11's shape: 20 routers took 44 s).
    fn cyclic_mounts(n: usize) -> String {
        let mut src = String::from("const express = require('express');\nconst app = express();\n");
        for i in 0..n {
            src.push_str(&format!(
                "const r{i} = express.Router();\nr{i}.get('/x', (q, s) => s.end());\n"
            ));
        }
        for i in 0..n {
            for j in 0..5 {
                src.push_str(&format!("app.use('/p{j}', r{i});\n"));
            }
        }
        for i in 0..2 * n {
            src.push_str(&format!("r{}.use('/n{i}', r{});\n", i % n, (i + 1) % n));
        }
        src
    }

    /// `(endpoints, counted work)` of the endpoint stage over one file.
    fn stage_work(language: Language, src: &str) -> (usize, u64) {
        let facts = rb_syntax::extract(language, src.as_bytes()).unwrap();
        let by_path = BTreeMap::from([("svc/r.js", (language, &facts))]);
        let keys = BTreeMap::from([("svc/r.js", rb_syntax::symbol_keys("svc/r.js", &facts))]);
        let go = BTreeMap::new();
        let index = Index::new(&by_path, &go, &keys);
        let service_of = |_: &str| "svc".to_owned();
        let (exact, context, unread) = (BTreeMap::new(), BTreeMap::new(), BTreeSet::new());
        let mut b = GraphBuilder::new();
        b.add_node(NodeKey::new(NodeKind::Service, "svc"), "svc", Attrs::new())
            .unwrap();
        b.add_node(
            NodeKey::new(NodeKind::File, "svc/r.js"),
            "r.js",
            Attrs::new(),
        )
        .unwrap();
        rb_frameworks::work::take();
        let routes =
            BTreeMap::from([("svc/r.js", rb_frameworks::routes::routes(language, &facts))]);
        let inputs = Inputs {
            files: &by_path,
            routes: &routes,
            keys: &keys,
            index: &index,
            exact_handlers: &exact,
            service_of: &service_of,
            context_paths: &context,
            config_unread: &unread,
            data_rest: &NO_DATA_REST,
            specs: &BTreeMap::new(),
            has_project: &|_: &str| true,
        };
        let (_, endpoints) = add_endpoints(&mut b, &inputs).unwrap();
        (endpoints.len(), rb_frameworks::work::take())
    }

    /// Contract M7b S4 (stress F11): cyclic mounts compose in work proportional to the output.
    /// Once a router's known prefixes are at the cap and its prefix is already unknown, no
    /// further mount can change it, so the rest are not explored. Doubling the routers at most
    /// quadruples the work (the output grows with the routers; exploring every simple path of
    /// the cycle grows exponentially).
    #[test]
    fn cyclic_mounts_compose_in_bounded_work() {
        let (small_endpoints, small) = stage_work(Language::JavaScript, &cyclic_mounts(6));
        let (large_endpoints, large) = stage_work(Language::JavaScript, &cyclic_mounts(12));
        // The output is pinned to what the unpruned composer produced (measured before S4).
        assert_eq!((small_endpoints, large_endpoints), (365, 725));
        assert!(
            small > 0 && large <= 4 * small,
            "{small} → {large} (endpoints {small_endpoints} → {large_endpoints})"
        );
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
                "HANDLED_BY HTTP POST /login @svc ?router=svc/routes.ts#r -> rb-heuristic . . . `svc/handlers.ts`/login(). [heuristic]"
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

    /// Contract M9b D7 (B12b): Spring Data REST resources are endpoints only in a service whose
    /// build names Spring Data REST, always `synthetic` (`heuristic` beside a
    /// `RepositoryRestConfigurer`, counted once), under the context path and the base path.
    #[test]
    fn data_rest_endpoints_are_synthetic() {
        const IMPORTS: &str = "package p;\nimport org.springframework.data.repository.*;\nimport org.springframework.data.rest.core.annotation.RepositoryRestResource;\n";
        let repo = |path: &str, base: &str, finder: &str| {
            format!(
                "{IMPORTS}@RepositoryRestResource(path = \"{path}\")\npublic interface R extends {base}<A, Long> {{\n    A {finder}(String v);\n}}\n"
            )
        };
        let files = [
            ("acct/src/A.java", repo("accounts", "CrudRepository", "findByEmail")),
            // The same repository in a service without the dependency.
            ("plain/src/P.java", repo("plain", "CrudRepository", "findByEmail")),
            ("based/src/B.java", repo("things", "Repository", "findByName")),
            ("conf/src/C.java", repo("confs", "PagingAndSortingRepository", "count")),
            (
                "conf/src/Cfg.java",
                "package p;\nimport org.springframework.data.rest.webmvc.config.RepositoryRestConfigurer;\nclass Cfg implements RepositoryRestConfigurer {}\n".to_owned(),
            ),
            // Every other way a configurer is registered caps too (M9b pass 1, E1 and E2): a
            // subclass of the 2.x `…Adapter`; the 2.5.5 reference's `@Bean` returning an
            // anonymous `…Adapter`; a Kotlin class; a Kotlin `@Bean` of `withConfig` (3.1+),
            // whose return type is inferred.
            ("adpt/src/A.java", repo("adapts", "PagingAndSortingRepository", "count")),
            (
                "adpt/src/Cfg.java",
                "package p;\nimport org.springframework.data.rest.webmvc.config.RepositoryRestConfigurerAdapter;\nclass Cfg extends RepositoryRestConfigurerAdapter {}\n".to_owned(),
            ),
            ("bean/src/B.java", repo("beans", "PagingAndSortingRepository", "count")),
            (
                "bean/src/Conf.java",
                "package p;\nimport org.springframework.context.annotation.Bean;\nimport org.springframework.data.rest.webmvc.config.RepositoryRestConfigurer;\nimport org.springframework.data.rest.webmvc.config.RepositoryRestConfigurerAdapter;\nclass Conf {\n    @Bean\n    public RepositoryRestConfigurer repositoryRestConfigurer() {\n        return new RepositoryRestConfigurerAdapter() {\n            @Override\n            public void configureRepositoryRestConfiguration(RepositoryRestConfiguration config) {\n                config.setBasePath(\"/moved\");\n            }\n        };\n    }\n}\n".to_owned(),
            ),
            ("kcfg/src/K.java", repo("kcfgs", "PagingAndSortingRepository", "count")),
            (
                "kcfg/src/Cfg.kt",
                "package p\nimport org.springframework.data.rest.webmvc.config.RepositoryRestConfigurer\nclass Cfg : RepositoryRestConfigurer\n".to_owned(),
            ),
            ("kwith/src/W.java", repo("kwiths", "PagingAndSortingRepository", "count")),
            (
                "kwith/src/Conf.kt",
                "package p\nimport org.springframework.context.annotation.Bean\nimport org.springframework.data.rest.webmvc.config.RepositoryRestConfigurer\nclass Conf {\n    @Bean\n    fun cfg() = RepositoryRestConfigurer.withConfig { c -> c.setBasePath(\"/moved\") }\n}\n".to_owned(),
            ),
            // Another type's `withConfig`, and a method returning another type, cap nothing.
            ("neg/src/N.java", repo("negs", "PagingAndSortingRepository", "count")),
            (
                "neg/src/Conf.java",
                "package p;\nclass Conf {\n    RestTemplate rt() { return Builder.withConfig(x); }\n}\n".to_owned(),
            ),
        ];
        let facts: Vec<(String, Language, FileFacts)> = files
            .iter()
            .map(|(p, src)| {
                let language = if p.ends_with(".kt") {
                    Language::Kotlin
                } else {
                    Language::Java
                };
                let f = rb_syntax::extract(language, src.as_bytes()).unwrap();
                ((*p).to_owned(), language, f)
            })
            .collect();
        let data_rest: BTreeMap<String, Option<String>> = [
            ("acct".to_owned(), None),
            ("based".to_owned(), Some("api".to_owned())),
            ("conf".to_owned(), None),
            ("adpt".to_owned(), None),
            ("bean".to_owned(), None),
            ("kcfg".to_owned(), None),
            ("kwith".to_owned(), None),
            ("neg".to_owned(), None),
        ]
        .into();
        let built = build_from(
            &facts,
            &[],
            &[("based", "/svc")],
            &BTreeMap::new(),
            &[],
            &data_rest,
        );
        let mut exposes: Vec<String> = edges(&built)
            .into_iter()
            .filter(|e| e.starts_with("EXPOSES"))
            .collect();
        exposes.sort();
        assert_eq!(
            exposes,
            [
                "EXPOSES acct -> HTTP DELETE /accounts/{} @acct [synthetic]",
                "EXPOSES acct -> HTTP GET /accounts @acct [synthetic]",
                "EXPOSES acct -> HTTP GET /accounts/search/findByEmail @acct [synthetic]",
                "EXPOSES acct -> HTTP GET /accounts/{} @acct [synthetic]",
                "EXPOSES acct -> HTTP PATCH /accounts/{} @acct [synthetic]",
                "EXPOSES acct -> HTTP POST /accounts @acct [synthetic]",
                "EXPOSES acct -> HTTP PUT /accounts/{} @acct [synthetic]",
                "EXPOSES adpt -> HTTP GET /adapts @adpt [heuristic]",
                "EXPOSES based -> HTTP GET /svc/api/things/search/findByName @based [synthetic]",
                "EXPOSES bean -> HTTP GET /beans @bean [heuristic]",
                "EXPOSES conf -> HTTP GET /confs @conf [heuristic]",
                "EXPOSES kcfg -> HTTP GET /kcfgs @kcfg [heuristic]",
                "EXPOSES kwith -> HTTP GET /kwiths @kwith [heuristic]",
                "EXPOSES neg -> HTTP GET /negs @neg [synthetic]",
            ]
        );
        let handled: Vec<String> = edges(&built)
            .into_iter()
            .filter(|e| e.starts_with("HANDLED_BY"))
            .collect();
        assert!(
            handled.contains(&"HANDLED_BY HTTP GET /accounts/search/findByEmail @acct -> rb-heuristic . . . `acct/src/A.java`/R#findByEmail(). [synthetic]".to_owned()),
            "{handled:?}"
        );
        assert!(
            handled.iter().all(|e| !e.ends_with("[exact]")),
            "{handled:?}"
        );
        let unread: Vec<String> = diagnostics(&built)
            .into_iter()
            .filter(|d| d.starts_with("data_rest_base_unread"))
            .collect();
        assert_eq!(
            unread,
            [
                r#"data_rest_base_unread {"service":"adpt"}"#,
                r#"data_rest_base_unread {"service":"bean"}"#,
                r#"data_rest_base_unread {"service":"conf"}"#,
                r#"data_rest_base_unread {"service":"kcfg"}"#,
                r#"data_rest_base_unread {"service":"kwith"}"#,
            ]
        );
    }

    /// Contract M9b C2 (B9): a gin handler written as a method value of the receiver binds to
    /// the method even when it is declared in another file than the receiver's type.
    #[test]
    fn gin_method_value_handlers_bind_across_files() {
        let handler = "package handler\nimport \"github.com/gin-gonic/gin\"\ntype Handler struct{}\nfunc (h *Handler) InitRoutes() *gin.Engine {\n\tr := gin.New()\n\tr.POST(\"/x\", h.signUp)\n\treturn r\n}\n";
        let auth = "package handler\nimport \"github.com/gin-gonic/gin\"\nfunc (h *Handler) signUp(c *gin.Context) {}\n";
        let built = build(
            &[
                ("svc/handler.go", Language::Go, handler),
                ("svc/auth.go", Language::Go, auth),
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
                "HANDLED_BY HTTP POST /x @svc -> rb-heuristic . . . `svc/auth.go`/Handler#signUp(). [heuristic]"
            ]
        );
        assert!(
            !diagnostics(&built)
                .iter()
                .any(|d| d.starts_with("route_handler_unresolved")),
            "{:?}",
            diagnostics(&built)
        );
    }

    /// S2: a group passed to a parameterised register function composes from the
    /// call site; the endpoint key is path-based, its certainty heuristic, and both
    /// sites are evidence. Unreached registrations stay `prefix_unknown`.
    #[test]
    fn gin_parameter_groups_compose_from_their_call_sites() {
        let src = "package main\nimport \"github.com/gin-gonic/gin\"\nfunc Register(router *gin.RouterGroup) {\n\trouter.GET(\"/login\", Login)\n}\nfunc Login(c *gin.Context) {\n}\nfunc main() {\n\tr := gin.Default()\n\tv1 := r.Group(\"/api\")\n\tRegister(v1.Group(\"/users\"))\n}\n";
        let built = build(&[("svc/main.go", Language::Go, src)], &[]);
        assert_eq!(
            edges(&built),
            [
                "EXPOSES svc -> HTTP GET /api/users/login @svc [heuristic]",
                "HANDLED_BY HTTP GET /api/users/login @svc -> rb-heuristic . . . `svc/main.go`/Login(). [heuristic]",
            ]
        );
        assert!(
            !diagnostics(&built)
                .iter()
                .any(|d| d.starts_with("route_prefix_unknown")),
            "{:?}",
            diagnostics(&built)
        );
        // Both sites are evidence on the composed edge.
        let exposes = built
            .graph
            .edges()
            .iter()
            .find(|e| e.kind == EdgeKind::Exposes)
            .unwrap();
        assert_eq!(exposes.evidence.len(), 2);
    }

    /// S2 across packages: the callee resolves through its import (never by bare
    /// name); the endpoint lives in the callee's service.
    #[test]
    fn gin_parameter_groups_compose_across_packages() {
        let main = "package main\nimport (\n\t\"example.com/svc/users\"\n\t\"github.com/gin-gonic/gin\"\n)\nfunc main() {\n\tr := gin.Default()\n\tv1 := r.Group(\"/api\")\n\tusers.UsersRegister(v1.Group(\"/users\"))\n}\n";
        let routers = "package users\nimport \"github.com/gin-gonic/gin\"\nfunc UsersRegister(router *gin.RouterGroup) {\n\trouter.GET(\"/login\", Login)\n}\nfunc Login(c *gin.Context) {\n}\n";
        let facts: Vec<(String, Language, FileFacts)> = [
            ("app/main.go", Language::Go, main),
            ("users/routers.go", Language::Go, routers),
        ]
        .iter()
        .map(|(p, l, src)| {
            (
                (*p).to_owned(),
                *l,
                rb_syntax::extract(*l, src.as_bytes()).unwrap(),
            )
        })
        .collect();
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
        let go: BTreeMap<String, String> =
            BTreeMap::from([("".to_owned(), "example.com/svc".to_owned())]);
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
        let (exact, context_paths, config_unread) =
            (BTreeMap::new(), BTreeMap::new(), BTreeSet::new());
        let inputs = Inputs {
            files: &by_path,
            routes: &routes,
            keys: &keys,
            index: &index,
            exact_handlers: &exact,
            service_of: &service_of,
            context_paths: &context_paths,
            config_unread: &config_unread,
            data_rest: &NO_DATA_REST,
            specs: &BTreeMap::new(),
            has_project: &|_: &str| true,
        };
        let (diagnostics, _) = add_endpoints(&mut b, &inputs).unwrap();
        let built = Built {
            graph: b.finish().unwrap(),
            diagnostics,
            endpoints: Vec::new(),
        };
        assert_eq!(
            edges(&built),
            [
                "EXPOSES users -> HTTP GET /api/users/login @users [heuristic]",
                "HANDLED_BY HTTP GET /api/users/login @users -> rb-heuristic . . . `users/routers.go`/Login(). [heuristic]",
            ]
        );
    }

    /// Contract M9a D1 (bench B7): test files feed no endpoint, prefix or composition; each one
    /// with route facts is counted once, and its other route diagnostics are not emitted.
    #[test]
    fn routes_in_test_files_are_not_endpoints() {
        let gin = "package main\nimport \"github.com/gin-gonic/gin\"\nfunc Register(router *gin.RouterGroup) {\n\trouter.GET(\"/login\", Login)\n}\nfunc Login(c *gin.Context) {\n}\nfunc main() {\n\tr := gin.Default()\n\tr.GET(\"/ping\", Login)\n\tRegister(r.Group(\"/api\"))\n}\n";
        let gin_test = "package main\nimport \"github.com/gin-gonic/gin\"\nfunc TestRoutes() {\n\tr := gin.Default()\n\tr.GET(\"/identity\", Login)\n\tRegister(r.Group(\"/t\"))\n}\n";
        let spring = "package p;\nimport org.springframework.web.bind.annotation.*;\n@RestController\nclass C { @GetMapping(\"/a\") void a() {} }\n";
        let spring_test = "package p;\nimport org.springframework.web.bind.annotation.*;\n@RestController\nclass Fake { @GetMapping(\"/fake\") void f() {} @GetMapping(Paths.X) void g() {} }\n";
        let app = "import express from \"express\";\nconst app = express();\napp.get(\"/health\", (req, res) => res.end());\n";
        // Two routes in one test file: the diagnostic counts both.
        let app_spec = "import express from \"express\";\nconst app = express();\napp.post(\"/sign-up\", (req, res) => res.end());\napp.get(\"/sign-in\", (req, res) => res.end());\n";
        let built = build(
            &[
                ("svc/main.go", Language::Go, gin),
                ("svc/main_test.go", Language::Go, gin_test),
                ("jsvc/src/main/java/p/C.java", Language::Java, spring),
                (
                    "jsvc/src/test/java/p/Fake.java",
                    Language::Java,
                    spring_test,
                ),
                ("web/src/app.ts", Language::TypeScript, app),
                ("web/src/app.spec.ts", Language::TypeScript, app_spec),
            ],
            &[],
        );
        assert_eq!(
            edges(&built)
                .iter()
                .filter(|e| e.starts_with("EXPOSES"))
                .cloned()
                .collect::<Vec<_>>(),
            [
                "EXPOSES jsvc -> HTTP GET /a @jsvc [exact]",
                "EXPOSES svc -> HTTP GET /api/login @svc [heuristic]",
                "EXPOSES svc -> HTTP GET /ping @svc [exact]",
                "EXPOSES web -> HTTP GET /health @web [exact]",
            ]
        );
        let counted: Vec<(&str, String)> = built
            .diagnostics
            .iter()
            .filter(|d| d.kind == "routes_in_test_code")
            .map(|d| (d.path.as_deref().unwrap(), d.detail.to_string()))
            .collect();
        assert_eq!(
            counted,
            [
                (
                    "jsvc/src/test/java/p/Fake.java",
                    r#"{"routes":1}"#.to_owned()
                ),
                ("svc/main_test.go", r#"{"routes":1}"#.to_owned()),
                ("web/src/app.spec.ts", r#"{"routes":2}"#.to_owned()),
            ]
        );
        assert!(
            !diagnostics(&built)
                .iter()
                .any(|d| d.starts_with("route_path_dynamic")),
            "{:?}",
            diagnostics(&built)
        );
    }

    /// Contract M6f S3/S4: spec operations become endpoints under the shared key, so a code
    /// route for the same method+path joins its handler for free; an operation with no code
    /// route is kept, `synthetic` (contract M9a D2), unhandled and counted.
    #[test]
    fn spec_operations_become_endpoints_and_join_code_routes() {
        let app = "import express from \"express\";\nexport const app = express();\napp.get(\"/users\", (req, res) => res.end());\napp.post(\"/users\", (req, res) => res.end());\n";
        let spec = "openapi: 3.0.0\npaths:\n  /users:\n    get:\n      operationId: listUsers\n    post:\n      operationId: addUser\n  /missing:\n    get:\n      operationId: getMissing\n";
        let built = build_with_specs(
            &[("api/src/app.ts", Language::TypeScript, app)],
            &[("api/openapi.yaml", spec)],
            &[],
        );
        assert_eq!(
            edges(&built),
            [
                "EXPOSES api -> HTTP GET /missing @api [synthetic]",
                "EXPOSES api -> HTTP GET /users @api [exact]",
                "EXPOSES api -> HTTP POST /users @api [exact]",
                "HANDLED_BY HTTP GET /users @api -> api/src/app.ts [exact]",
                "HANDLED_BY HTTP POST /users @api -> api/src/app.ts [exact]",
            ]
        );
        let diag = diagnostics(&built);
        assert!(
            diag.iter()
                .any(|d| d.starts_with("spec_operation_unhandled")),
            "{diag:?}"
        );
        assert!(
            !diag.iter().any(|d| d.starts_with("openapi_unread")),
            "{diag:?}"
        );
        // Operation IDs ride the endpoint refs for generated-call stitching (S6).
        let ops: Vec<(&str, Vec<&str>)> = built
            .endpoints
            .iter()
            .map(|e| {
                (
                    e.key.as_str(),
                    e.operations.iter().map(String::as_str).collect(),
                )
            })
            .collect();
        assert!(ops.contains(&("HTTP GET /users @api", vec!["listUsers"])));
        assert!(ops.contains(&("HTTP POST /users @api", vec!["addUser"])));
        assert!(ops.contains(&("HTTP GET /missing @api", vec!["getMissing"])));
    }

    /// Contract M6f S4b: a spec operation matching an interface mapping is handled by the
    /// interface declaration (`exact`) and its implementors (`heuristic`); without a spec,
    /// interface mappings stay invisible (D12).
    #[test]
    fn spec_operations_on_interfaces_roll_down_to_implementors() {
        let api = "package p;\nimport org.springframework.web.bind.annotation.*;\npublic interface UsersApi {\n  @GetMapping(\"/users\")\n  String list();\n}\n";
        let ctrl = "package p;\nclass UserController implements UsersApi {\n  public String list() { return \"\"; }\n}\n";
        let spec = "openapi: 3.0.0\npaths:\n  /users:\n    get:\n      operationId: listUsers\n";
        let files = [
            ("api/UsersApi.java", Language::Java, api),
            ("api/UserController.java", Language::Java, ctrl),
        ];
        let built = build_with_specs(&files, &[("api/openapi.yaml", spec)], &[]);
        assert_eq!(
            edges(&built),
            [
                "EXPOSES api -> HTTP GET /users @api [exact]",
                "HANDLED_BY HTTP GET /users @api -> rb-heuristic . . . `api/UserController.java`/UserController#list(). [heuristic]",
                "HANDLED_BY HTTP GET /users @api -> rb-heuristic . . . `api/UsersApi.java`/UsersApi#list(). [exact]",
            ]
        );
        // Without the spec the interface mapping stays invisible.
        let built = build(&files, &[]);
        assert!(edges(&built).is_empty());
        assert!(
            diagnostics(&built)
                .iter()
                .any(|d| d.starts_with("route_on_interface")),
            "{:?}",
            diagnostics(&built)
        );
    }

    /// Contract M6f S3: proto services become `GRPC` endpoints of their service; a spec
    /// outside any project stays endpoint-less and counted; an unreadable spec is counted.
    #[test]
    fn proto_services_and_unread_specs_are_counted() {
        let proto =
            "syntax = \"proto3\";\npackage acme;\nservice Users {\n  rpc Get (A) returns (B);\n}\n";
        let built = build_with_specs(
            &[(
                "api/src/a.ts",
                Language::TypeScript,
                "export const a = 1;\n",
            )],
            &[
                ("api/svc.proto", proto),
                ("docs/openapi.yaml", "openapi: 3.0.0\npaths:\n"),
                ("api/broken.yaml", "openapi: 3.0.0\npaths:\n  /x:\n"),
            ],
            &[],
        );
        assert_eq!(
            edges(&built),
            ["EXPOSES api -> GRPC acme.Users/Get @api [exact]",]
        );
        let diag = diagnostics(&built);
        for kind in [
            "spec_operation_unhandled",
            "spec_outside_project",
            "openapi_unread",
        ] {
            assert!(diag.iter().any(|d| d.starts_with(kind)), "{kind}: {diag:?}");
        }
    }

    /// Contract M7a S3 (stress F17, microservices-demo): a gRPC service's endpoints belong to
    /// the projects that register a server for it — `exact` where that project holds a
    /// declaring proto, `heuristic` where the proto lives elsewhere (here: outside every
    /// project). Copies in non-implementing projects expose nothing; a service declared in
    /// several projects that nobody registers gets no endpoint and one counted
    /// `grpc_implementer_unknown`; a single-copy proto with no registration keeps M6f's
    /// behaviour.
    #[test]
    fn grpc_endpoints_belong_to_registering_projects() {
        let demo = "syntax = \"proto3\";\npackage hipstershop;\nservice CartService {\n  rpc GetCart (A) returns (B) {}\n}\nservice ProductCatalogService {\n  rpc GetProduct (A) returns (B) {}\n}\nservice CurrencyService {\n  rpc Convert (A) returns (B) {}\n}\nservice AdService {\n  rpc GetAds (A) returns (B) {}\n}\n";
        let lonely = "syntax = \"proto3\";\npackage solo;\nservice Lonely {\n  rpc Ping (A) returns (B);\n}\n";
        let built = build_with_specs(
            &[
                (
                    "ads/src/AdService.java",
                    Language::Java,
                    "package hipstershop;\nclass AdService {\n  static class AdServiceImpl extends hipstershop.AdServiceGrpc.AdServiceImplBase {}\n}\n",
                ),
                (
                    "currency/server.js",
                    Language::JavaScript,
                    "function main() {\n  server.addService(shopProto.CurrencyService.service, {convert});\n}\n",
                ),
                (
                    "catalog/server.go",
                    Language::Go,
                    "package main\nfunc main() {\n\tpb.RegisterProductCatalogServiceServer(srv, svc)\n\tnotifypb.RegisterNotifyServer(srv, n)\n}\n",
                ),
                (
                    "solo/main.go",
                    Language::Go,
                    "package main\nfunc main() {}\n",
                ),
                // A one-copy proto library implemented elsewhere: its holder exposes nothing.
                ("shared/doc.go", Language::Go, "package shared\n"),
            ],
            &[
                ("ads/src/demo.proto", demo),
                ("currency/proto/demo.proto", demo),
                ("protos/demo.proto", demo),
                ("solo/svc.proto", lonely),
                (
                    "shared/notify.proto",
                    "syntax = \"proto3\";\npackage notify;\nservice Notify {\n  rpc Send (A) returns (B);\n}\n",
                ),
            ],
            &[],
        );
        let mut exposes: Vec<String> = edges(&built)
            .into_iter()
            .filter(|e| e.starts_with("EXPOSES"))
            .collect();
        exposes.sort();
        assert_eq!(
            exposes,
            [
                "EXPOSES ads -> GRPC hipstershop.AdService/GetAds @ads [exact]",
                "EXPOSES catalog -> GRPC hipstershop.ProductCatalogService/GetProduct @catalog [heuristic]",
                "EXPOSES catalog -> GRPC notify.Notify/Send @catalog [heuristic]",
                "EXPOSES currency -> GRPC hipstershop.CurrencyService/Convert @currency [exact]",
                "EXPOSES solo -> GRPC solo.Lonely/Ping @solo [exact]",
            ]
        );
        let unknown: Vec<String> = diagnostics(&built)
            .into_iter()
            .filter(|d| d.starts_with("grpc_implementer_unknown"))
            .collect();
        assert_eq!(
            unknown,
            [
                "grpc_implementer_unknown {\"declared_in\":[\"ads/src/demo.proto\",\"currency/proto/demo.proto\",\"protos/demo.proto\"],\"service\":\"hipstershop.CartService\"}"
            ]
        );
    }

    /// Contract M7a S3: a registration whose name matches services of several proto packages
    /// attributes nothing (counted); those services fall back as if unregistered.
    #[test]
    fn grpc_registrations_naming_several_packages_are_counted() {
        let a =
            "syntax = \"proto3\";\npackage a;\nservice Users {\n  rpc Get (X) returns (Y);\n}\n";
        let b =
            "syntax = \"proto3\";\npackage b;\nservice Users {\n  rpc List (X) returns (Y);\n}\n";
        let built = build_with_specs(
            &[
                (
                    "svc/main.go",
                    Language::Go,
                    "package main\nfunc main() {\n\tpb.RegisterUsersServer(srv, svc)\n}\n",
                ),
                ("pa/x.go", Language::Go, "package x\n"),
                ("pb/x.go", Language::Go, "package x\n"),
            ],
            &[("pa/a.proto", a), ("pb/b.proto", b)],
            &[],
        );
        let mut exposes: Vec<String> = edges(&built)
            .into_iter()
            .filter(|e| e.starts_with("EXPOSES"))
            .collect();
        exposes.sort();
        assert_eq!(
            exposes,
            [
                "EXPOSES pa -> GRPC a.Users/Get @pa [exact]",
                "EXPOSES pb -> GRPC b.Users/List @pb [exact]",
            ]
        );
        assert!(
            diagnostics(&built)
                .iter()
                .any(|d| d.starts_with("grpc_registration_ambiguous")),
            "{:?}",
            diagnostics(&built)
        );
    }

    /// S2 fallback: a dynamic group argument folds to nothing, so the unknown node
    /// stays (and is counted); no composed edge appears.
    #[test]
    fn gin_parameter_groups_with_dynamic_arguments_stay_unknown() {
        let src = "package main\nimport \"github.com/gin-gonic/gin\"\nfunc Register(router *gin.RouterGroup) {\n\trouter.GET(\"/login\", Login)\n}\nfunc Login(c *gin.Context) {\n}\nfunc setup(prefix string) {\n\tr := gin.Default()\n\tRegister(r.Group(prefix))\n}\nfunc main() {\n\tsetup(\"/x\")\n}\n";
        let built = build(&[("svc/main.go", Language::Go, src)], &[]);
        assert_eq!(
            edges(&built),
            [
                "EXPOSES svc -> HTTP GET /login @svc ?router=svc/main.go#Register.router [heuristic]",
                "HANDLED_BY HTTP GET /login @svc ?router=svc/main.go#Register.router -> rb-heuristic . . . `svc/main.go`/Login(). [heuristic]",
            ]
        );
        assert!(
            diagnostics(&built)
                .iter()
                .any(|d| d.starts_with("route_prefix_unknown")),
            "{:?}",
            diagnostics(&built)
        );
    }

    /// S2 is linear in call sites: four times the sites makes at most five times the
    /// work. Parsing, keys, routes and the M3a index are built outside the counted
    /// part; the composition walk is counted, not timed.
    #[test]
    fn param_group_composition_grows_linearly() {
        let work = |n: usize| {
            let mut caller =
                "package main\nimport \"github.com/gin-gonic/gin\"\nfunc main() {\n\tr := gin.Default()\n"
                    .to_owned();
            for i in 0..n {
                caller.push_str(&format!("\tRegister(r.Group(\"/p{i}\"))\n"));
            }
            caller.push_str("}\n");
            let callee = "package main\nimport \"github.com/gin-gonic/gin\"\nfunc Register(router *gin.RouterGroup) {\n\trouter.GET(\"/login\", Login)\n}\nfunc Login(c *gin.Context) {\n}\n";
            let facts: Vec<(String, Language, FileFacts)> = [
                ("svc/caller.go", Language::Go, caller.as_str()),
                ("svc/callee.go", Language::Go, callee),
            ]
            .iter()
            .map(|(p, l, src)| {
                (
                    (*p).to_owned(),
                    *l,
                    rb_syntax::extract(*l, src.as_bytes()).unwrap(),
                )
            })
            .collect();
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
            let (exact, context_paths, config_unread) =
                (BTreeMap::new(), BTreeMap::new(), BTreeSet::new());
            let inputs = Inputs {
                files: &by_path,
                routes: &routes,
                keys: &keys,
                index: &index,
                exact_handlers: &exact,
                service_of: &service_of,
                context_paths: &context_paths,
                config_unread: &config_unread,
                data_rest: &NO_DATA_REST,
                specs: &BTreeMap::new(),
                has_project: &|_: &str| true,
            };
            rb_frameworks::work::take();
            let (_, endpoints) = add_endpoints(&mut b, &inputs).unwrap();
            assert_eq!(endpoints.len(), n);
            rb_frameworks::work::take()
        };
        let (small, large) = (work(200), work(800));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }

    /// S3 end to end (the node-express-realworld shape): a chain-built router mounted
    /// through an anonymous default export composes across two files; certainty stays
    /// heuristic and no `prefix_unknown` remains.
    #[test]
    fn express_chains_through_anonymous_defaults_compose() {
        let tags = "import { Router } from \"express\";\nconst router = Router();\nfunction list(req, res) {}\nrouter.get(\"/tags\", list);\nexport default router;\n";
        let routes = "import { Router } from \"express\";\nimport tags from \"./tags\";\nconst api = Router().use(tags);\nexport default Router().use('/api', api);\n";
        let main = "import express from \"express\";\nimport routes from \"./routes\";\nconst app = express();\napp.use(routes);\n";
        let built = build(
            &[
                ("web/tags.ts", Language::TypeScript, tags),
                ("web/routes.ts", Language::TypeScript, routes),
                ("web/main.ts", Language::TypeScript, main),
            ],
            &[],
        );
        assert_eq!(
            edges(&built),
            [
                "EXPOSES web -> HTTP GET /api/tags @web [heuristic]",
                "HANDLED_BY HTTP GET /api/tags @web -> rb-heuristic . . . `web/tags.ts`/list(). [heuristic]",
            ]
        );
        assert!(
            !diagnostics(&built)
                .iter()
                .any(|d| d.starts_with("route_prefix_unknown")),
            "{:?}",
            diagnostics(&built)
        );
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

    /// Evaluator pass 1, P8 and P7: a `ServeMux` host pattern's host is an endpoint attribute
    /// (one host as a string, several as a sorted array, none once any registration has no
    /// host), and every endpoint of a service whose Spring configuration is partly unread is
    /// flagged `prefix_maybe_incomplete`.
    #[test]
    fn host_patterns_and_unread_config_mark_endpoints() {
        let go = "package main\nimport \"net/http\"\nfunc main() {\n\tmux := http.NewServeMux()\n\tmux.HandleFunc(\"GET Example.com:8080/hosted\", h)\n\tmux.HandleFunc(\"GET b.example.com/two\", h)\n\tmux.HandleFunc(\"GET a.example.com/two\", h)\n\tmux.HandleFunc(\"GET c.example.com/any\", h)\n\tmux.HandleFunc(\"GET /any\", h)\n}\nfunc h() {}\n";
        let spring = "package p;\nimport org.springframework.web.bind.annotation.*;\n@RestController\nclass C {\n  @GetMapping(\"/latest\")\n  String l() { return \"\"; }\n}\n";
        let facts: Vec<(String, Language, FileFacts)> = [
            ("go/main.go", Language::Go, go),
            ("stats/src/C.java", Language::Java, spring),
        ]
        .iter()
        .map(|(p, l, src)| {
            (
                (*p).to_owned(),
                *l,
                rb_syntax::extract(*l, src.as_bytes()).unwrap(),
            )
        })
        .collect();
        let built = build_from(
            &facts,
            &[],
            &[],
            &BTreeMap::new(),
            &["stats"],
            &NO_DATA_REST,
        );
        let attrs = |key: &str| {
            built
                .graph
                .nodes()
                .iter()
                .find(|n| n.key == key)
                .unwrap_or_else(|| panic!("{key}"))
                .attrs
                .clone()
        };
        assert_eq!(attrs("HTTP GET /hosted @go")["host"], "example.com");
        assert_eq!(
            attrs("HTTP GET /two @go")["host"],
            json!(["a.example.com", "b.example.com"])
        );
        assert!(!attrs("HTTP GET /any @go").contains_key("host"));
        assert!(!attrs("HTTP GET /any @go").contains_key("prefix_maybe_incomplete"));
        assert_eq!(
            attrs("HTTP GET /latest @stats")["prefix_maybe_incomplete"],
            true
        );
        let refs: Vec<(&str, &[String], bool)> = built
            .endpoints
            .iter()
            .map(|e| {
                (
                    e.key.as_str(),
                    e.hosts.as_slice(),
                    e.prefix_maybe_incomplete,
                )
            })
            .collect();
        assert_eq!(
            refs,
            [
                ("HTTP GET /any @go", &[][..], false),
                (
                    "HTTP GET /hosted @go",
                    &["example.com".to_owned()][..],
                    false
                ),
                ("HTTP GET /latest @stats", &[][..], true),
                (
                    "HTTP GET /two @go",
                    &["a.example.com".to_owned(), "b.example.com".to_owned()][..],
                    false
                ),
            ]
        );
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
            let built = build_from(&facts, &[], &[], exact, &[], &NO_DATA_REST);
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
