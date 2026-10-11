//! Route packs (design §4.5, contract M4a S4–S6): Spring MVC, Express, gin and Go `net/http`
//! route facts from one file's syntax facts. Pure functions; the indexer composes prefixes across
//! files (Express mounts, Spring's context path), normalises and builds `Endpoint` nodes.
//!
//! A framework is recognised only through import facts (or a qualified name), never by a bare
//! short name (contract invariant 6).

use std::collections::{HashMap, HashSet};
use std::ops::Deref;

use rb_syntax::{
    Arg, Binding, CallChain, Decoration, DecorationArg, DefKind, FileFacts, Init, Language, Span,
    TemplatePart,
};

use crate::endpoint::{ANY, norm_method};
use crate::work::Counted;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Framework {
    Spring,
    Express,
    Gin,
    Chi,
    FastAPI,
    NetHttp,
}

impl Framework {
    pub fn as_str(self) -> &'static str {
        match self {
            Framework::Spring => "spring",
            Framework::Express => "express",
            Framework::Gin => "gin",
            Framework::Chi => "chi",
            Framework::FastAPI => "fastapi",
            Framework::NetHttp => "net/http",
        }
    }
}

/// What a route's handler is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handler {
    /// The decorated method: index into `FileFacts::definitions`.
    Definition(usize),
    /// An inline function at this span.
    Inline(Span),
    /// A name or member path to resolve (`login`, `ctrl.login`, `h.Login`), at its span.
    Name { text: String, span: Span },
    /// Something else (a call that is not a one-level wrapper, …): unresolved.
    Unknown(Span),
}

/// Where a route's path starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteBase {
    /// Complete within the file (Spring before its context path, a gin engine, `net/http`).
    Root,
    /// An Express app or router of this file: index into [`FileRoutes::routers`]. Its prefix
    /// comes from mounts, possibly in other files.
    Router(usize),
    /// A gin group whose prefix is unknown (reached as a function parameter, through a dynamic
    /// `Group(prefix)`, a cycle or the depth cap): index into `FileFacts::bindings` of the
    /// registration's receiver, the router whose identity keys the endpoint ([`binding_identity`]).
    Unknown(usize),
}

/// One route registration or mapping, before cross-file composition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteFact {
    pub framework: Framework,
    /// Upper case, or [`ANY`].
    pub method: String,
    /// The raw path, composed within the file (Spring class × method, gin groups); not yet
    /// normalised.
    pub path: String,
    pub base: RouteBase,
    pub handler: Handler,
    /// The evidence: the mapping annotation or the registration call.
    pub span: Span,
    /// The definition containing the registration (`None` at top level).
    pub enclosing: Option<usize>,
    /// A Go `ServeMux` host pattern's host (`example.com/x`), lower-cased without the port; the
    /// route serves only requests for that host (evaluator pass 1, P8).
    pub host: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouterKind {
    /// `express()`: a root unless mounted.
    App,
    /// `Router()`: needs a mount for its prefix.
    Router,
}

/// An Express app or router bound to a name in this file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterDecl {
    pub name: String,
    pub kind: RouterKind,
    /// The binding's span (its identity in the file).
    pub span: Span,
    /// Index into `FileFacts::bindings` of the binding ([`binding_identity`]);
    /// `None` for an anonymous default-exported construction, identified as
    /// `{file}#default` (contract M6b S3).
    pub binding: Option<usize>,
    /// Bound at the top level of the file, so other modules may import it.
    pub top_level: bool,
}

/// `parent.use(prefix, child)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// Index into [`FileRoutes::routers`].
    pub parent: usize,
    /// `None` when the prefix is not a static string; `""` for `use(child)`.
    pub prefix: Option<String>,
    /// The mounted router as written: a local router or an imported name.
    pub child: String,
    /// A router of this file the child names, if it is one.
    pub child_local: Option<usize>,
    pub span: Span,
}

/// A route-level diagnostic found within one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteDiagnostic {
    pub kind: &'static str,
    pub span: Span,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileRoutes {
    pub routes: Vec<RouteFact>,
    pub routers: Vec<RouterDecl>,
    pub mounts: Vec<Mount>,
    pub diagnostics: Vec<RouteDiagnostic>,
}

/// Every route fact of one file.
pub fn routes(language: Language, facts: &FileFacts) -> FileRoutes {
    let facts = &FileScope::new(facts);
    let mut out = FileRoutes::default();
    match language {
        Language::Java => spring(facts, &mut out),
        Language::TypeScript | Language::Tsx | Language::JavaScript => express(facts, &mut out),
        Language::Go => {
            gin(facts, &mut out);
            chi(facts, &mut out);
            net_http(facts, &mut out);
        }
        Language::Python => fastapi(facts, &mut out),
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Shared helpers

fn simple(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn before(a: Span, b: Span) -> bool {
    (a[0], a[1]) <= (b[0], b[1])
}

/// One file's facts with the lookups the packs and the folder make, indexed once, so a lookup
/// costs the number of same-named bindings rather than the size of the file (a scan per call
/// made route and client extraction quadratic in the file; `composition_grows_linearly`).
/// It dereferences to the facts.
pub struct FileScope<'f> {
    pub facts: &'f FileFacts,
    /// The file's bindings and call chains, counted when scanned (they shadow the
    /// [`FileFacts`] fields, so every scan a pack makes through its scope is measured; R6).
    pub bindings: Counted<'f, Binding>,
    pub chains: Counted<'f, CallChain>,
    /// Non-member bindings by name.
    locals_by_name: HashMap<&'f str, Vec<usize>>,
    /// Member bindings by name.
    members_by_name: HashMap<&'f str, Vec<usize>>,
    /// The chain recorded at each span (the first, if several share one).
    chain_by_span: HashMap<Span, usize>,
    /// Definitions by parent, in source order.
    children_of: HashMap<usize, Vec<usize>>,
}

impl<'f> FileScope<'f> {
    pub fn new(facts: &'f FileFacts) -> Self {
        let mut locals: HashMap<&str, Vec<usize>> = HashMap::new();
        let mut members: HashMap<&str, Vec<usize>> = HashMap::new();
        for (i, b) in facts.bindings.iter().enumerate() {
            let table = if b.member { &mut members } else { &mut locals };
            table.entry(b.name.as_str()).or_default().push(i);
        }
        let mut chain_by_span = HashMap::new();
        for (i, c) in facts.chains.iter().enumerate() {
            chain_by_span.entry(c.span).or_insert(i);
        }
        let mut children_of: HashMap<usize, Vec<usize>> = HashMap::new();
        for (i, d) in facts.definitions.iter().enumerate() {
            if let Some(p) = d.parent {
                children_of.entry(p).or_default().push(i);
            }
        }
        FileScope {
            facts,
            bindings: Counted::new(&facts.bindings),
            chains: Counted::new(&facts.chains),
            locals_by_name: locals,
            members_by_name: members,
            chain_by_span,
            children_of,
        }
    }

    /// The index of the non-member binding of `name` in effect at `at`: the innermost one
    /// whose extent holds it.
    pub fn binding_index_at(&self, name: &str, at: Span) -> Option<usize> {
        self.locals_by_name
            .get(name)?
            .iter()
            .copied()
            .inspect(|_| crate::work::tick(1))
            .filter(|&i| {
                let v = self.facts.bindings[i].visible;
                before([v[0], v[1], 0, 0], at) && before(at, [v[2], v[3], 0, 0])
            })
            .max_by_key(|&i| {
                let v = self.facts.bindings[i].visible;
                (v[0], v[1])
            })
    }

    /// The non-member binding of `name` in effect at `at`.
    pub fn binding_at(&self, name: &str, at: Span) -> Option<&'f Binding> {
        self.binding_index_at(name, at)
            .map(|i| &self.facts.bindings[i])
    }

    /// The member bindings named `name`, in source order.
    pub fn members(&self, name: &str) -> &[usize] {
        let found = self
            .members_by_name
            .get(name)
            .map_or(&[][..], Vec::as_slice);
        crate::work::tick(found.len());
        found
    }

    /// The definitions directly inside `parent`, in source order.
    pub fn children(&self, parent: usize) -> &[usize] {
        let found = self.children_of.get(&parent).map_or(&[][..], Vec::as_slice);
        crate::work::tick(found.len());
        found
    }

    /// The chain recorded at `span`.
    pub fn chain_at(&self, span: Span) -> Option<&'f CallChain> {
        crate::work::tick(1);
        self.chain_by_span
            .get(&span)
            .map(|&i| &self.facts.chains[i])
    }
}

impl Deref for FileScope<'_> {
    type Target = FileFacts;
    fn deref(&self) -> &FileFacts {
        self.facts
    }
}

/// A binding's identity within its file, stable across unrelated edits (no position): its name
/// qualified by the names of its enclosing definitions, outermost first, with a Go method's
/// receiver type when the type is declared in another file (`UsersRegister.router` for a
/// parameter, `authRouter` at top level). Keys an endpoint whose prefix is unknown (contract M4a
/// amendment 10).
pub fn binding_identity(facts: &FileFacts, binding: usize) -> String {
    let b = &facts.bindings[binding];
    let mut names = vec![b.name.as_str()];
    let mut seen = HashSet::new();
    let mut cur = b.scope;
    while let Some(i) = cur.filter(|&i| i < facts.definitions.len() && seen.insert(i)) {
        let d = &facts.definitions[i];
        names.push(&d.name);
        if d.parent.is_none()
            && let Some(receiver) = &d.receiver
        {
            names.push(receiver);
        }
        cur = d.parent;
    }
    names.reverse();
    names.join(".")
}

/// The chain a binding is initialised with.
fn init_chain<'f>(facts: &'f FileFacts, b: &Binding) -> Option<&'f CallChain> {
    match b.init {
        Some(Init::Call { chain }) => facts.chains.get(chain),
        _ => None,
    }
}

/// A template with no hole, as its text.
pub fn literal(arg: &Arg) -> Option<String> {
    match arg {
        Arg::Template { template } => match template.parts.as_slice() {
            [] => Some(String::new()),
            [TemplatePart::Lit { text }] => Some(text.clone()),
            _ => None,
        },
        _ => None,
    }
}

/// The handler among a registration's arguments: the last function-like one (an inline
/// function or a name), looking through one wrapper call (`asyncHandler(fn)`,
/// `http.HandlerFunc(f)`).
fn handler(facts: &FileScope<'_>, args: &[Arg]) -> Handler {
    let Some(last) = args.last() else {
        return Handler::Unknown([0; 4]);
    };
    match last {
        Arg::Function { span } => Handler::Inline(*span),
        Arg::Name { text, span } => Handler::Name {
            text: text.clone(),
            span: *span,
        },
        other => {
            let span = other.span();
            let wrapped = facts
                .chain_at(span)
                .filter(|c| c.links.len() == 1)
                .and_then(|c| c.links[0].args.last());
            match wrapped {
                Some(Arg::Function { span }) => Handler::Inline(*span),
                Some(Arg::Name { text, span }) => Handler::Name {
                    text: text.clone(),
                    span: *span,
                },
                _ => Handler::Unknown(span),
            }
        }
    }
}

fn join(prefix: &str, path: &str) -> String {
    match (prefix.is_empty(), path.is_empty()) {
        (true, _) => path.to_owned(),
        (_, true) => prefix.to_owned(),
        _ => format!("{prefix}/{path}"),
    }
}

// ---------------------------------------------------------------------------------------------
// Spring MVC (S4)

const SPRING_WEB: &str = "org.springframework.web.bind.annotation";
const SPRING_STEREOTYPE: &str = "org.springframework.stereotype";

const MAPPINGS: &[(&str, Option<&str>)] = &[
    ("RequestMapping", None),
    ("GetMapping", Some("GET")),
    ("PostMapping", Some("POST")),
    ("PutMapping", Some("PUT")),
    ("DeleteMapping", Some("DELETE")),
    ("PatchMapping", Some("PATCH")),
];

/// Whether the annotation `d` is `package.simple`: qualified as written, or imported by name or
/// by a wildcard of its package.
pub fn is_annotation(facts: &FileFacts, d: &Decoration, package: &str, simple_name: &str) -> bool {
    if d.name == format!("{package}.{simple_name}") {
        return true;
    }
    d.name == simple_name
        && facts.imports.iter().any(|i| {
            !i.is_static
                && i.module == package
                && (i.wildcard
                    || i.names
                        .iter()
                        .any(|n| n.name == simple_name && n.local == simple_name))
        })
}

/// The method's mapping annotations as `(annotation, fixed method)`.
fn mappings<'f>(
    facts: &FileFacts,
    decorations: &'f [Decoration],
) -> Vec<(&'f Decoration, Option<&'static str>)> {
    decorations
        .iter()
        .filter_map(|d| {
            MAPPINGS
                .iter()
                .find(|(name, _)| is_annotation(facts, d, SPRING_WEB, name))
                .map(|(_, m)| (d, *m))
        })
        .collect()
}

/// Static paths of a mapping (`value`, `path` or positional; arrays give several). `Err` with the
/// text when a path is not a static string. No path is `[""]`.
fn mapping_paths(d: &Decoration) -> Result<Vec<String>, String> {
    let arg = d
        .args
        .iter()
        .find(|a| matches!(a.key.as_deref(), Some("value" | "path")))
        .or_else(|| d.args.iter().find(|a| a.key.is_none()));
    let Some(arg) = arg else {
        return Ok(vec![String::new()]);
    };
    let values: Vec<&DecorationArg> = if arg.items.is_empty() && !arg.value.starts_with('{') {
        vec![arg]
    } else {
        arg.items.iter().collect()
    };
    if values.is_empty() {
        return Ok(vec![String::new()]);
    }
    values
        .into_iter()
        .map(|v| {
            if v.literal {
                Ok(v.value.clone())
            } else {
                Err(v.value.clone())
            }
        })
        .collect()
}

/// `method = RequestMethod.X` (or an array); none is [`ANY`].
fn mapping_methods(d: &Decoration) -> Vec<String> {
    let Some(arg) = d.args.iter().find(|a| a.key.as_deref() == Some("method")) else {
        return vec![ANY.to_owned()];
    };
    let values: Vec<&str> = if arg.items.is_empty() {
        vec![arg.value.as_str()]
    } else {
        arg.items.iter().map(|i| i.value.as_str()).collect()
    };
    let mut methods: Vec<String> = values.into_iter().map(|v| norm_method(simple(v))).collect();
    methods.sort();
    methods.dedup();
    methods
}

fn spring(facts: &FileScope<'_>, out: &mut FileRoutes) {
    let defs = &facts.definitions;
    for (class, def) in defs.iter().enumerate() {
        let methods: Vec<usize> = facts
            .children(class)
            .iter()
            .copied()
            .filter(|&m| defs[m].kind == DefKind::Method)
            .collect();
        match def.kind {
            DefKind::Interface => {
                // Feign clients are HTTP clients (S10); any other interface's mappings are
                // counted, not resolved (D12).
                if def
                    .decorations
                    .iter()
                    .any(|d| simple(&d.name) == "FeignClient")
                {
                    continue;
                }
                for &m in &methods {
                    for (d, _) in mappings(facts, &defs[m].decorations) {
                        out.diagnostics.push(RouteDiagnostic {
                            kind: "route_on_interface",
                            span: d.span,
                            detail: format!("{}#{}", def.name, defs[m].name),
                        });
                    }
                }
            }
            DefKind::Class => {
                let controller = def.decorations.iter().any(|d| {
                    is_annotation(facts, d, SPRING_WEB, "RestController")
                        || is_annotation(facts, d, SPRING_STEREOTYPE, "Controller")
                });
                if !controller {
                    continue;
                }
                let class_mapping = def
                    .decorations
                    .iter()
                    .find(|d| is_annotation(facts, d, SPRING_WEB, "RequestMapping"));
                let class_paths = match class_mapping.map(|d| (d, mapping_paths(d))) {
                    None => vec![String::new()],
                    Some((_, Ok(paths))) => paths,
                    Some((d, Err(text))) => {
                        dynamic(out, d.span, text);
                        continue;
                    }
                };
                for &m in &methods {
                    for (d, fixed) in mappings(facts, &defs[m].decorations) {
                        let paths = match mapping_paths(d) {
                            Ok(paths) => paths,
                            Err(text) => {
                                dynamic(out, d.span, text);
                                continue;
                            }
                        };
                        let verbs =
                            fixed.map_or_else(|| mapping_methods(d), |f| vec![f.to_owned()]);
                        for prefix in &class_paths {
                            for path in &paths {
                                for verb in &verbs {
                                    out.routes.push(RouteFact {
                                        framework: Framework::Spring,
                                        method: verb.clone(),
                                        path: join(prefix, path),
                                        base: RouteBase::Root,
                                        handler: Handler::Definition(m),
                                        span: d.span,
                                        enclosing: Some(m),
                                        host: None,
                                    });
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

fn dynamic(out: &mut FileRoutes, span: Span, text: String) {
    out.diagnostics.push(RouteDiagnostic {
        kind: "route_path_dynamic",
        span,
        detail: text,
    });
}

// ---------------------------------------------------------------------------------------------
// Express (S5)

const EXPRESS_VERBS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "head", "options", "all",
];

/// Local names bound to the `express` module (default import, namespace import or
/// `require("express")`), and to its named `Router` export.
struct ExpressNames {
    module: HashSet<String>,
    router: HashSet<String>,
}

fn express_names(facts: &FileFacts) -> ExpressNames {
    let mut names = ExpressNames {
        module: HashSet::new(),
        router: HashSet::new(),
    };
    for i in facts.imports.iter().filter(|i| i.module == "express") {
        names.module.extend(i.alias.clone());
        for n in &i.names {
            match n.name.as_str() {
                "default" => {
                    names.module.insert(n.local.clone());
                }
                "Router" => {
                    names.router.insert(n.local.clone());
                }
                _ => {}
            }
        }
    }
    // `const express = require("express")`, top-level only.
    for b in facts
        .bindings
        .iter()
        .filter(|b| b.scope.is_none() && !b.member)
    {
        if let Some(c) = init_chain(facts, b)
            && c.root.is_none()
            && c.links.len() == 1
            && c.links[0].name == "require"
            && c.links[0].args.first().and_then(literal).as_deref() == Some("express")
        {
            names.module.insert(b.name.clone());
        }
    }
    names
}

/// The router kind a binding's initialiser creates, if any.
fn router_kind(names: &ExpressNames, chain: &CallChain) -> Option<RouterKind> {
    // Only the first link decides: a construction followed by any links (`Router()`,
    // `Router().use(…)`, `express()`) binds; the rest are mounts and routes on it.
    let first = chain.links.first()?;
    match (chain.root.as_deref(), first.name.as_str()) {
        (None, f) if names.module.contains(f) => Some(RouterKind::App),
        (None, f) if names.router.contains(f) => Some(RouterKind::Router),
        (Some(m), "Router") if names.module.contains(m) => Some(RouterKind::Router),
        _ => None,
    }
}

fn express(facts: &FileScope<'_>, out: &mut FileRoutes) {
    let names = express_names(facts);
    if names.module.is_empty() && names.router.is_empty() {
        return;
    }
    let mut binding_router: HashMap<usize, usize> = HashMap::new(); // binding → router
    // Init-chain span → router, so a chain-built binding's own links (`api.use(…)`)
    // resolve to it below.
    let mut chain_router: HashMap<Span, usize> = HashMap::new();
    for (bi, b) in facts.bindings.iter().enumerate() {
        if b.member {
            continue;
        }
        if let Some(chain) = init_chain(facts, b)
            && let Some(kind) = router_kind(&names, chain)
        {
            chain_router.insert(chain.span, out.routers.len());
            binding_router.insert(bi, out.routers.len());
            out.routers.push(RouterDecl {
                name: b.name.clone(),
                kind,
                span: b.span,
                binding: Some(bi),
                top_level: b.scope.is_none(),
            });
        }
    }
    // The anonymous default-exported router construction, if any (contract M6b S3):
    // the default-exported chain starting with a Router construction. Only one
    // default export exists per module, so at most one.
    let default_router: Option<usize> = facts.default_export_expr.and_then(|span| {
        facts
            .chains
            .iter()
            .find(|c| c.span == span)
            .filter(|c| router_kind(&names, c) == Some(RouterKind::Router))
            .map(|c| {
                out.routers.push(RouterDecl {
                    name: "default".to_owned(),
                    kind: RouterKind::Router,
                    span: c.span,
                    binding: None,
                    top_level: true,
                });
                out.routers.len() - 1
            })
    });
    let router_names: HashSet<String> = out.routers.iter().map(|r| r.name.clone()).collect();
    let router_at = |name: &str, at: Span| -> Option<usize> {
        if !router_names.contains(name) {
            return None;
        }
        let bi = facts.binding_index_at(name, at)?;
        binding_router.get(&bi).copied()
    };
    for chain in &facts.chains {
        // The router this chain registers on: a bound name, the chain-built binding
        // whose init this chain is, or the anonymous default export's own
        // construction. Anything else (including unexported inline constructions)
        // registers nowhere.
        let router = match chain.root.as_deref() {
            Some(root) => router_at(root, chain.span),
            None => chain_router.get(&chain.span).copied().or_else(|| {
                default_router.filter(|_| Some(chain.span) == facts.default_export_expr)
            }),
        };
        let Some(router) = router else {
            continue;
        };
        let links = &chain.links;
        // `use(prefix?, child…)` mounts on any link; `verb(path, …handlers)` and
        // `route(path).verb(h).verb(h2)` routes.
        let (path, verbs) = match links.first() {
            Some(l) if l.name == "route" && links.len() > 1 => (l.args.first(), &links[1..]),
            _ => (None, &links[..]),
        };
        for link in verbs {
            if link.name == "use" {
                let (prefix, children) = match link.args.first() {
                    Some(a @ Arg::Template { .. }) => (literal(a), &link.args[1..]),
                    _ => (Some(String::new()), &link.args[..]),
                };
                for child in children {
                    if let Arg::Name { text, .. } = child {
                        out.mounts.push(Mount {
                            parent: router,
                            prefix: prefix.clone(),
                            child: text.clone(),
                            child_local: router_at(text, chain.span),
                            span: chain.span,
                        });
                    }
                }
                continue;
            }
            if !EXPRESS_VERBS.contains(&link.name.as_str()) {
                continue;
            }
            let (path_arg, handlers) = match path {
                Some(p) => (Some(p), &link.args[..]),
                None if link.args.len() >= 2 => (link.args.first(), &link.args[1..]),
                None => continue,
            };
            let Some(path_arg) = path_arg else { continue };
            if !matches!(path_arg, Arg::Template { .. }) {
                continue;
            }
            let Some(path) = literal(path_arg) else {
                dynamic(out, chain.span, format!("{path_arg:?}"));
                continue;
            };
            out.routes.push(RouteFact {
                framework: Framework::Express,
                method: norm_method(&link.name),
                path,
                base: RouteBase::Router(router),
                handler: handler(facts, handlers),
                span: chain.span,
                enclosing: chain.enclosing,
                host: None,
            });
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Go: gin (S6)

const GIN_VERBS: &[&str] = &[
    "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "Any",
];

/// Groups nested through bindings are followed at most this deep.
pub const MAX_GROUP_DEPTH: usize = 32;

/// The local name of a Go import of `module` (its alias, or the last path segment).
fn go_import_name(facts: &FileFacts, module: &str) -> Option<String> {
    facts
        .imports
        .iter()
        .find(|i| i.module == module)
        .and_then(|i| match i.alias.as_deref() {
            Some("_" | ".") => None,
            Some(a) => Some(a.to_owned()),
            None => module.rsplit('/').next().map(str::to_owned),
        })
}

/// The prefix a gin receiver stands for: `Some(Some(p))` known, `Some(None)` a gin group whose
/// prefix is unknown (a parameter), `None` not gin.
fn gin_prefix(
    facts: &FileScope<'_>,
    gin: &str,
    name: &str,
    at: Span,
    seen: &mut HashSet<Span>,
) -> Option<Option<String>> {
    let b = facts.binding_at(name, at)?;
    if seen.len() >= MAX_GROUP_DEPTH || !seen.insert(b.span) {
        return Some(None);
    }
    if b.init.is_none() {
        let ty = b.type_name.as_str();
        return if ty == format!("{gin}.Engine") {
            Some(Some(String::new()))
        } else if ["RouterGroup", "IRouter", "IRoutes"]
            .iter()
            .any(|t| ty == format!("{gin}.{t}"))
        {
            Some(None)
        } else {
            None
        };
    }
    let chain = init_chain(facts, b)?;
    gin_chain_prefix(facts, gin, chain, chain.links.len(), seen)
}

/// The prefix of a gin chain's receiver composed with its first `upto` links, which must all be
/// `Group(prefix)` (or `Default()`/`New()` on the gin package).
fn gin_chain_prefix(
    facts: &FileScope<'_>,
    gin: &str,
    chain: &CallChain,
    upto: usize,
    seen: &mut HashSet<Span>,
) -> Option<Option<String>> {
    let links = &chain.links[..upto];
    let root = chain.root.as_deref()?;
    let (mut prefix, rest) = if root == gin {
        match links.first().map(|l| l.name.as_str()) {
            Some("Default" | "New") => (Some(String::new()), &links[1..]),
            _ => return None,
        }
    } else {
        (gin_prefix(facts, gin, root, chain.span, seen)?, links)
    };
    for link in rest {
        if link.name != "Group" {
            return None;
        }
        let p = link.args.first().and_then(literal);
        prefix = match (prefix, p) {
            (Some(a), Some(b)) => Some(join(&a, &b)),
            _ => None,
        };
    }
    Some(prefix)
}

/// The prefix a call chain stands for as a gin group argument: its receiver composed
/// with every link, which must all be `Group(literal)` (contract M6b S2). The endpoint
/// stage folds the group a call site passes to a parameterised register function with
/// this. `Some(Some(p))` known, `Some(None)` a gin group of unknown prefix, `None`
/// not gin at all.
pub fn gin_group_prefix(facts: &FileFacts, chain: &CallChain) -> Option<Option<String>> {
    let scope = FileScope::new(facts);
    let gin = go_import_name(&scope, "github.com/gin-gonic/gin")?;
    let mut seen = HashSet::new();
    gin_chain_prefix(&scope, &gin, chain, chain.links.len(), &mut seen)
}

fn gin(facts: &FileScope<'_>, out: &mut FileRoutes) {
    let Some(gin) = go_import_name(facts, "github.com/gin-gonic/gin") else {
        return;
    };
    for chain in &facts.chains {
        let Some(last) = chain.links.last() else {
            continue;
        };
        let (method, args) = match last.name.as_str() {
            "Handle" if last.args.len() >= 3 => {
                let Some(m) = go_method(&last.args[0]) else {
                    continue;
                };
                (m, &last.args[1..])
            }
            v if GIN_VERBS.contains(&v) && last.args.len() >= 2 => (norm_method(v), &last.args[..]),
            _ => continue,
        };
        let mut seen = HashSet::new();
        let Some(prefix) = gin_chain_prefix(facts, &gin, chain, chain.links.len() - 1, &mut seen)
        else {
            continue;
        };
        let Some(path) = literal(&args[0]) else {
            dynamic(out, chain.span, format!("{:?}", args[0]));
            continue;
        };
        let (path, base) = match prefix {
            Some(p) => (join(&p, &path), RouteBase::Root),
            // An unknown prefix always comes from a receiver binding (`gin_prefix`).
            None => match chain
                .root
                .as_deref()
                .and_then(|root| facts.binding_index_at(root, chain.span))
            {
                Some(receiver) => (path, RouteBase::Unknown(receiver)),
                None => continue,
            },
        };
        out.routes.push(RouteFact {
            framework: Framework::Gin,
            method,
            path,
            base,
            handler: handler(facts, &args[1..]),
            span: chain.span,
            enclosing: chain.enclosing,
            host: None,
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Go: chi (contract M6d S9)

/// chi import paths (v5 current, v4 the legacy module name).
const CHI_MODULES: &[&str] = &["github.com/go-chi/chi/v5", "github.com/go-chi/chi"];

/// The local name of a chi import: its alias, or the package name (the last
/// path segment, past a `/vN` major-version suffix).
fn chi_import_name(facts: &FileFacts, module: &str) -> Option<String> {
    facts
        .imports
        .iter()
        .find(|i| i.module == module)
        .and_then(|i| match i.alias.as_deref() {
            Some("_" | ".") => None,
            Some(a) => Some(a.to_owned()),
            None => {
                let mut segments: Vec<&str> = module.split('/').collect();
                if segments.last().is_some_and(|s| {
                    s.len() > 1 && s.starts_with('v') && s[1..].bytes().all(|b| b.is_ascii_digit())
                }) {
                    segments.pop();
                }
                segments.last().map(|s| (*s).to_owned())
            }
        })
}

/// chi verbs, as written (Go is case-sensitive and the API spells them so).
const CHI_VERBS: &[&str] = &[
    "Get", "Post", "Put", "Delete", "Patch", "Head", "Options", "Trace", "Connect",
];

/// Whether a span lies within another (start and end comparisons).
fn within(inner: Span, outer: Span) -> bool {
    (outer[0], outer[1]) <= (inner[0], inner[1]) && (inner[2], inner[3]) <= (outer[2], outer[3])
}

/// The argument functions of one file: (span, chain index, link index,
/// argument index), in chain order. Built once per file on first use, so a
/// closure parameter resolves to its `Route`/`Group` call without scanning
/// every chain per parameter (contract M6d S13: the per-parameter scan made
/// sibling `Route` closures quadratic).
#[derive(Default)]
struct FuncCache {
    built: bool,
    /// Parameter binding index → (chain index, link index) of the innermost
    /// argument function containing the parameter's declaration, if any.
    /// Argument spans are syntax nodes, so they nest or are disjoint, and
    /// the innermost container of a declaration site is the literal
    /// declaring it; the sweep below evaluates exactly the minimum the old
    /// per-parameter scan computed (smallest span, ties in chain order).
    by_param: HashMap<usize, Option<(usize, usize)>>,
}

impl FuncCache {
    /// The call taking the parameter's function literal, if any.
    fn containing(&mut self, facts: &FileScope<'_>, param: usize) -> Option<(usize, usize)> {
        if !self.built {
            self.build(facts);
        }
        self.by_param.get(&param).copied().flatten()
    }

    fn build(&mut self, facts: &FileScope<'_>) {
        self.built = true;
        let mut funcs: Vec<(Span, usize, usize, usize)> = Vec::new();
        for (ci, c) in facts.chains.iter().enumerate() {
            for (li, link) in c.links.iter().enumerate() {
                for (ai, arg) in link.args.iter().enumerate() {
                    if let Arg::Function { span } = arg {
                        funcs.push((*span, ci, li, ai));
                    }
                }
            }
        }
        let mut params: Vec<(Span, usize)> = Vec::new();
        for (bi, b) in facts.bindings.iter().enumerate() {
            if b.parameter.is_some() {
                params.push((b.span, bi));
            }
        }
        if funcs.is_empty() || params.is_empty() {
            return;
        }
        // Innermost-first: the event sort below is stable, so pre-sorting
        // fixes the order opens at one position push in — smallest span on
        // top, ties in chain order, which is the old scan's rule (strictly
        // smaller replaces, so the first of equals wins).
        funcs.sort_by(|a, b| {
            span_size(&a.0)
                .cmp(&span_size(&b.0))
                .then(a.1.cmp(&b.1))
                .then(a.2.cmp(&b.2))
                .then(a.3.cmp(&b.3))
        });
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        enum Kind {
            Close,
            Open,
            Query,
        }
        // At one position closes come first (a function ending where a
        // parameter starts does not contain it), then opens (one starting
        // there does), then the queries. Opens are inserted in reverse
        // function order so the stable sort pushes smallest-first.
        let mut events: Vec<((i32, i32), Kind, usize)> = Vec::new();
        for (fi, (s, _, _, _)) in funcs.iter().enumerate().rev() {
            events.push(((s[0], s[1]), Kind::Open, fi));
        }
        for (fi, (s, _, _, _)) in funcs.iter().enumerate() {
            events.push(((s[2], s[3]), Kind::Close, fi));
        }
        for (pi, (s, _)) in params.iter().enumerate() {
            events.push(((s[0], s[1]), Kind::Query, pi));
        }
        events.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut stack: Vec<usize> = Vec::new();
        for (_, kind, idx) in events {
            match kind {
                Kind::Close => {
                    if stack.last() == Some(&idx) {
                        stack.pop();
                    } else if let Some(pos) = stack.iter().rposition(|&f| f == idx) {
                        stack.remove(pos);
                    }
                }
                Kind::Open => stack.push(idx),
                Kind::Query => {
                    let (pspan, bi) = params[idx];
                    let mut answer = None;
                    while let Some(&fi) = stack.last() {
                        let (fspan, ci, li, _) = funcs[fi];
                        if within(pspan, fspan) {
                            answer = Some((ci, li));
                            break;
                        }
                        stack.pop();
                    }
                    self.by_param.insert(bi, answer);
                }
            }
        }
    }
}

/// Resolution state for one file's chi receivers: the per-parameter
/// prefix memo, the in-progress guard, and the function index. Bundled so
/// the recursive resolver stays under clippy's argument limit.
struct ChiState {
    funcs: FuncCache,
    memo: HashMap<usize, Option<Option<String>>>,
    in_progress: HashSet<usize>,
}

/// The prefix a chi receiver name stands for: `Some(Some(p))` known,
/// `Some(None)` unknown-but-chi (a dynamic `Route` prefix or the depth
/// cap), `None` not chi. Bindings to `NewRouter`/`NewMux` root at `""`;
/// closure parameters resolve through their `Route`/`Group` call (memoised:
/// each parameter resolves once no matter how many inner chains name it).
fn chi_prefix(
    facts: &FileScope<'_>,
    chi: &str,
    name: &str,
    at: Span,
    state: &mut ChiState,
    depth: usize,
) -> Option<Option<String>> {
    let bi = facts.binding_index_at(name, at)?;
    let b = &facts.bindings[bi];
    // `r := chi.NewRouter()` / `chi.NewMux()`.
    if let Some(c) = init_chain(facts, b)
        && let (Some(root), [link]) = (c.root.as_deref(), c.links.as_slice())
        && root == chi
        && matches!(link.name.as_str(), "NewRouter" | "NewMux")
    {
        return Some(Some(String::new()));
    }
    // `sub := r.With(mw)` (middleware does not change the prefix).
    if let Some(c) = init_chain(facts, b)
        && let (Some(root), links) = (c.root.as_deref(), c.links.as_slice())
        && !links.is_empty()
        && links.iter().all(|l| l.name == "With")
    {
        if depth + 1 >= MAX_GROUP_DEPTH || !state.in_progress.insert(bi) {
            return Some(None);
        }
        let found = chi_prefix(facts, chi, root, c.span, state, depth + 1);
        state.in_progress.remove(&bi);
        return found;
    }
    // A closure parameter: through its `Route`/`Group` call. Anything else
    // (a named function's parameter, a plain variable) is not chi.
    b.parameter?;
    if let Some(known) = state.memo.get(&bi) {
        return known.clone();
    }
    if depth >= MAX_GROUP_DEPTH || !state.in_progress.insert(bi) {
        return Some(None);
    }
    let found = chi_closure_prefix(facts, chi, bi, state, depth + 1);
    state.in_progress.remove(&bi);
    state.memo.insert(bi, found.clone());
    found
}

/// The prefix of the `Route(literal)` (or prefix-less `Group`) call taking
/// the parameter's function, composed over the call's own receiver. `None`
/// for named functions and non-chi calls. `Mount` stays out (topology
/// packs' job); a `Mount`ed sub-router's routes resolve where it is built.
fn chi_closure_prefix(
    facts: &FileScope<'_>,
    chi: &str,
    param_idx: usize,
    state: &mut ChiState,
    depth: usize,
) -> Option<Option<String>> {
    // The parameter's own function literal, through the per-file index:
    // nested `Route` calls contain everything inside them too.
    let (ci, li) = state.funcs.containing(facts, param_idx)?;
    let chain = &facts.chains[ci];
    // All links before the terminal carry middleware (`With`); the terminal
    // declares the group.
    let terminal = chain.links.last()?;
    if chain.links.len() - 1 != li || !chain.links[..li].iter().all(|l| l.name == "With") {
        return None;
    }
    let root = chain.root.as_deref()?;
    let outer = chi_prefix(facts, chi, root, chain.span, state, depth)?;
    match terminal.name.as_str() {
        "Route" if terminal.args.len() >= 2 => Some(match literal(&terminal.args[0]) {
            Some(p) => outer.map(|a| join(&a, &p)),
            None => None,
        }),
        "Group" => Some(outer),
        _ => None,
    }
}

/// The rough size of a span, for innermost-function selection.
fn span_size(span: &Span) -> i64 {
    i64::from(span[2] - span[0]) * 1_000_000 + i64::from(span[3] - span[1])
}

fn chi(facts: &FileScope<'_>, out: &mut FileRoutes) {
    let Some(chi) = CHI_MODULES.iter().find_map(|m| chi_import_name(facts, m)) else {
        return;
    };
    let mut state = ChiState {
        funcs: FuncCache::default(),
        memo: HashMap::new(),
        in_progress: HashSet::new(),
    };
    for chain in &facts.chains {
        let Some(last) = chain.links.last() else {
            continue;
        };
        let (method, args) = match last.name.as_str() {
            v if CHI_VERBS.contains(&v) && last.args.len() >= 2 => (norm_method(v), &last.args[..]),
            "Method" if last.args.len() >= 3 => {
                let Some(m) = go_method(&last.args[0]) else {
                    continue;
                };
                (m, &last.args[1..])
            }
            "Handle" if last.args.len() >= 2 => (ANY.to_owned(), &last.args[..]),
            _ => continue,
        };
        // Links before the last carry middleware (`With`); the receiver
        // resolves first.
        if !chain.links[..chain.links.len() - 1]
            .iter()
            .all(|l| l.name == "With")
        {
            continue;
        }
        let Some(root) = chain.root.as_deref() else {
            continue;
        };
        let Some(prefix) = chi_prefix(facts, &chi, root, chain.span, &mut state, 0) else {
            continue;
        };
        let Some(path) = literal(&args[0]) else {
            dynamic(out, chain.span, format!("{:?}", args[0]));
            continue;
        };
        let (path, base) = match prefix {
            Some(p) => (join(&p, &path), RouteBase::Root),
            // An unknown prefix always comes from a closure parameter
            // (`chi_prefix`); its binding keys the endpoint.
            None => match facts.binding_index_at(root, chain.span) {
                Some(receiver) => (path, RouteBase::Unknown(receiver)),
                None => continue,
            },
        };
        out.routes.push(RouteFact {
            framework: Framework::Chi,
            method,
            path,
            base,
            handler: handler(facts, &args[1..]),
            span: chain.span,
            enclosing: chain.enclosing,
            host: None,
        });
    }
}

// ---------------------------------------------------------------------------------------------
// ---------------------------------------------------------------------------------------------
// Python: FastAPI (contract M6d S9)

/// FastAPI decorator methods, as written.
const FASTAPI_METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// A FastAPI app or router bound in this file: its own prefix (`None` when
/// a `prefix` keyword is present but not a literal; `""` for apps and when
/// absent), and the binding for endpoint identity.
struct FastAPIApp {
    prefix: Option<String>,
    binding: usize,
}

fn fastapi(facts: &FileScope<'_>, out: &mut FileRoutes) {
    // Import gates (contract invariant 6): the `fastapi` module and
    // from-imported constructors, unshadowed where read.
    let mut module: Option<String> = None;
    // From-imported constructor locals: true for `APIRouter`.
    let mut ctors: HashMap<String, bool> = HashMap::new();
    for i in &facts.imports {
        if i.module != "fastapi" {
            continue;
        }
        if let Some(alias) = i.alias.as_deref() {
            module = Some(alias.to_owned());
        } else {
            module = Some("fastapi".to_owned());
        }
        for n in &i.names {
            if n.name == "APIRouter" {
                ctors.insert(n.local.clone(), true);
            } else if n.name == "FastAPI" {
                ctors.insert(n.local.clone(), false);
            }
        }
    }
    let unshadowed = |name: &str, at: Span| facts.binding_index_at(name, at).is_none();
    // Apps and routers: `app = FastAPI()`, `router = APIRouter(prefix=…)`,
    // through the module or a from-imported constructor.
    let mut apps: HashMap<usize, FastAPIApp> = HashMap::new();
    for (bi, b) in facts.bindings.iter().enumerate().filter(|(_, b)| !b.member) {
        let Some(c) = init_chain(facts, b) else {
            continue;
        };
        let (root, links) = (c.root.as_deref(), c.links.as_slice());
        let [link] = links else {
            continue;
        };
        // `None` when not a FastAPI construction.
        let found: Option<bool> = match (root, link.name.as_str()) {
            (Some(r), "FastAPI") if Some(r) == module.as_deref() && unshadowed(r, c.span) => {
                Some(false)
            }
            (Some(r), "APIRouter") if Some(r) == module.as_deref() && unshadowed(r, c.span) => {
                Some(true)
            }
            (None, name) if ctors.contains_key(name) && unshadowed(name, c.span) => {
                Some(ctors[name])
            }
            _ => None,
        };
        let Some(is_router) = found else {
            continue;
        };
        let prefix = if is_router {
            match crate::clients::kwarg(&link.args, "prefix") {
                None => Some(String::new()),
                Some(arg) => literal(arg),
            }
        } else {
            Some(String::new())
        };
        apps.insert(
            bi,
            FastAPIApp {
                prefix,
                binding: bi,
            },
        );
    }
    // Routes per decorator, with the local path and the router they hang
    // off (`None` for apps); prefixes compose in the post-pass below.
    let mut locals: Vec<String> = Vec::new();
    let mut routers_of: Vec<Option<usize>> = Vec::new();
    for (di, d) in facts.facts.definitions.iter().enumerate() {
        if !matches!(d.kind, DefKind::Function | DefKind::Method) {
            continue;
        }
        for deco in &d.decorations {
            let Some((obj, method)) = deco.name.rsplit_once('.') else {
                continue;
            };
            if obj.contains('.') {
                continue;
            }
            let Some(bi) = facts.binding_index_at(obj, deco.span) else {
                continue;
            };
            let Some(app) = apps.get(&bi) else {
                continue;
            };
            let methods: Vec<String> = if FASTAPI_METHODS.contains(&method) {
                vec![norm_method(method)]
            } else if method == "api_route" {
                match deco
                    .args
                    .iter()
                    .find(|a| a.key.as_deref() == Some("methods"))
                {
                    // Starlette serves an `api_route` without methods on all
                    // of them, like gin's `Any`.
                    None => vec![ANY.to_owned()],
                    Some(m) if !m.items.is_empty() && m.items.iter().all(|i| i.literal) => {
                        m.items.iter().map(|i| norm_method(&i.value)).collect()
                    }
                    // A non-literal method set cannot be enumerated: dynamic.
                    _ => continue,
                }
            } else {
                continue;
            };
            let Some(path) = deco
                .args
                .iter()
                .find(|a| a.key.is_none())
                .filter(|a| a.literal)
                .map(|a| a.value.clone())
            else {
                dynamic(out, deco.span, format!("@{obj}.{method}"));
                continue;
            };
            for m in methods {
                out.routes.push(RouteFact {
                    framework: Framework::FastAPI,
                    method: m,
                    path: path.clone(),
                    base: match &app.prefix {
                        Some(_) => RouteBase::Root,
                        None => RouteBase::Unknown(app.binding),
                    },
                    handler: Handler::Definition(di),
                    span: deco.span,
                    enclosing: Some(di),
                    host: None,
                });
                locals.push(path.clone());
                routers_of.push(Some(bi));
            }
        }
    }
    // `app.include_router(router)` / `router.include_router(sub)` edges:
    // (parent binding, child binding, extra prefix).
    let mut includes: Vec<(usize, usize, String)> = Vec::new();
    for chain in &facts.chains {
        let Some(last) = chain.links.last() else {
            continue;
        };
        if last.name != "include_router" || chain.links.len() != 1 {
            continue;
        }
        let Some(root) = chain.root.as_deref() else {
            continue;
        };
        if root.contains('.') {
            continue;
        }
        let Some(parent) = facts.binding_index_at(root, chain.span) else {
            continue;
        };
        if !apps.contains_key(&parent) {
            continue;
        }
        let Some(Arg::Name { text, .. }) = last.args.first() else {
            continue;
        };
        if text.contains('.') {
            continue;
        }
        let Some(child) = facts.binding_index_at(text, chain.span) else {
            continue;
        };
        if !apps.contains_key(&child) || child == parent {
            continue;
        }
        let extra = match crate::clients::kwarg(&last.args, "prefix") {
            None => String::new(),
            Some(arg) => match literal(arg) {
                Some(p) => p,
                None => continue,
            },
        };
        includes.push((parent, child, extra));
    }
    // Full prefixes per router: its own, under every include edge into it
    // (an include cycle or unknown component composes nothing: the route
    // keeps its collected path). Parents per router, in edge order: one
    // linear pass, so composing does not rescan every edge per level
    // (contract M6d S13).
    let mut parents_of: HashMap<usize, Vec<(usize, String)>> = HashMap::new();
    for (parent, child, extra) in &includes {
        crate::work::tick(1);
        parents_of
            .entry(*child)
            .or_default()
            .push((*parent, extra.clone()));
    }
    // Whether the include edges hold a directed cycle (iterative DFS, one
    // linear pass). Shared-prefix memoisation below is only valid when no
    // composition path revisits a router; a cyclic file keeps the legacy
    // per-route walk (a router including its includer is degenerate input).
    fn has_include_cycle(includes: &[(usize, usize, String)]) -> bool {
        let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
        for (parent, child, _) in includes {
            crate::work::tick(1);
            children.entry(*parent).or_default().push(*child);
        }
        let mut color: HashMap<usize, u8> = HashMap::new();
        for &root in children.keys() {
            if color.contains_key(&root) {
                continue;
            }
            color.insert(root, 1);
            let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
            while let Some((node, ci)) = stack.pop() {
                let kids: &[usize] = children.get(&node).map_or(&[], Vec::as_slice);
                if ci < kids.len() {
                    stack.push((node, ci + 1));
                    let next = kids[ci];
                    crate::work::tick(1);
                    match color.get(&next) {
                        Some(1) => return true,
                        Some(_) => {}
                        None => {
                            color.insert(next, 1);
                            stack.push((next, 0));
                        }
                    }
                } else {
                    color.insert(node, 2);
                }
            }
        }
        false
    }
    fn full_prefix(
        apps: &HashMap<usize, FastAPIApp>,
        includes: &[(usize, usize, String)],
        router: usize,
        seen: &mut HashSet<usize>,
        depth: usize,
    ) -> Vec<String> {
        if depth >= MAX_GROUP_DEPTH || !seen.insert(router) {
            return Vec::new();
        }
        let own = match apps.get(&router) {
            Some(FastAPIApp {
                prefix: Some(p), ..
            }) => p.clone(),
            _ => {
                seen.remove(&router);
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        let mut included = false;
        for (parent, child, extra) in includes {
            crate::work::tick(1);
            if *child != router {
                continue;
            }
            included = true;
            for base in full_prefix(apps, includes, *parent, seen, depth + 1) {
                out.push(join(&join(&base, extra), &own));
            }
        }
        if !included {
            out.push(own);
        }
        seen.remove(&router);
        out
    }
    /// Shared-prefix composition over an acyclic include graph: (prefix,
    /// levels above the router) per router, memoised by router. Levels
    /// replace the legacy walk's depth parameter: a path of k edges reaches
    /// exactly the calls below the depth cap, so the top level keeps levels
    /// below `MAX_GROUP_DEPTH`. Diamond paths still expand per path
    /// (amendment 14 F4: a router included twice serves both), which bounds
    /// work by the output. The `seen` guard never fires on an acyclic graph
    /// (firing means a repeated router on one path, i.e. a cycle), so
    /// memoised values are path-independent; it stays as termination
    /// defense, exactly as in the legacy walk.
    fn full_prefix_levels(
        apps: &HashMap<usize, FastAPIApp>,
        parents_of: &HashMap<usize, Vec<(usize, String)>>,
        router: usize,
        seen: &mut HashSet<usize>,
        memo: &mut HashMap<usize, Vec<(String, usize)>>,
    ) -> Vec<(String, usize)> {
        if let Some(hit) = memo.get(&router) {
            return hit.clone();
        }
        if !seen.insert(router) {
            return Vec::new();
        }
        let own = match apps.get(&router) {
            Some(FastAPIApp {
                prefix: Some(p), ..
            }) => p.clone(),
            _ => {
                seen.remove(&router);
                memo.insert(router, Vec::new());
                return Vec::new();
            }
        };
        let mut out = Vec::new();
        let mut included = false;
        if let Some(parents) = parents_of.get(&router) {
            for (parent, extra) in parents {
                crate::work::tick(1);
                included = true;
                for (base, levels) in full_prefix_levels(apps, parents_of, *parent, seen, memo) {
                    out.push((join(&join(&base, extra), &own), levels + 1));
                }
            }
        }
        if !included {
            out.push((own, 0));
        }
        seen.remove(&router);
        memo.insert(router, out.clone());
        out
    }
    // One prefix list per router: a route's composition is a pure function
    // of its router, so routes sharing one share the work.
    let mut seen_routers = HashSet::new();
    let mut routers = Vec::new();
    for router in routers_of.iter().flatten() {
        if seen_routers.insert(*router) {
            routers.push(*router);
        }
    }
    let mut composed: HashMap<usize, Vec<String>> = HashMap::new();
    if has_include_cycle(&includes) {
        for router in &routers {
            composed.insert(
                *router,
                full_prefix(&apps, &includes, *router, &mut HashSet::new(), 0),
            );
        }
    } else {
        let mut memo: HashMap<usize, Vec<(String, usize)>> = HashMap::new();
        for router in &routers {
            composed.insert(
                *router,
                full_prefix_levels(&apps, &parents_of, *router, &mut HashSet::new(), &mut memo)
                    .into_iter()
                    .filter(|(_, levels)| *levels < MAX_GROUP_DEPTH)
                    .map(|(prefix, _)| prefix)
                    .collect(),
            );
        }
    }
    // Compose include_router edges into router paths: one row per prefix
    // (a router included twice serves both).
    let mut rewritten: Vec<(usize, Vec<String>)> = Vec::new();
    for (i, route) in out.routes.iter().enumerate() {
        let (Some(router), Some(local)) = (routers_of.get(i).copied().flatten(), locals.get(i))
        else {
            continue;
        };
        // Zero (unknown) or identical prefixes: keep the collected row.
        let fresh: Vec<String> = composed.get(&router).map_or(Vec::new(), |prefixes| {
            prefixes
                .iter()
                .map(|prefix| join(prefix, local))
                .filter(|composed| composed != &route.path)
                .collect()
        });
        if !fresh.is_empty() {
            rewritten.push((i, fresh));
        }
    }
    for (i, paths) in rewritten {
        let mut paths = paths.into_iter();
        if let Some(first) = paths.next() {
            out.routes[i].path = first;
        }
        for path in paths {
            let mut clone = out.routes[i].clone();
            clone.path = path;
            out.routes.push(clone);
        }
    }
}

/// A method argument: a string, or `http.MethodX`.
/// A method argument: a string, or `http.MethodX`.
pub(crate) fn go_method(arg: &Arg) -> Option<String> {
    if let Some(m) = literal(arg) {
        return Some(norm_method(&m));
    }
    match arg {
        Arg::Name { text, .. } => simple(text).strip_prefix("Method").map(norm_method),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Go: net/http (S6)

fn net_http(facts: &FileScope<'_>, out: &mut FileRoutes) {
    let Some(http) = go_import_name(facts, "net/http") else {
        return;
    };
    for chain in &facts.chains {
        let [link] = chain.links.as_slice() else {
            continue;
        };
        if !matches!(link.name.as_str(), "HandleFunc" | "Handle") || link.args.len() != 2 {
            continue;
        }
        let Some(root) = chain.root.as_deref() else {
            continue;
        };
        let is_mux = root == http
            || facts.binding_at(root, chain.span).is_some_and(|b| {
                b.type_name == format!("{http}.ServeMux")
                    || init_chain(facts, b).is_some_and(|c| {
                        c.root.as_deref() == Some(http.as_str())
                            && c.links.len() == 1
                            && c.links[0].name == "NewServeMux"
                    })
            });
        if !is_mux {
            continue;
        }
        let Some(pattern) = literal(&link.args[0]) else {
            dynamic(out, chain.span, format!("{:?}", link.args[0]));
            continue;
        };
        let (method, host, path) = serve_mux_pattern(&pattern);
        out.routes.push(RouteFact {
            framework: Framework::NetHttp,
            method,
            path,
            base: RouteBase::Root,
            handler: handler(facts, &link.args[1..]),
            span: chain.span,
            enclosing: chain.enclosing,
            host,
        });
    }
}

/// A Go 1.22 `ServeMux` pattern `[METHOD ][HOST]/[PATH]` as `(method, host, path)`; the host is
/// lower-cased without its port. A path ending in `/` (without `{$}`) matches its whole subtree,
/// so it gets a trailing `{*}`.
pub fn serve_mux_pattern(pattern: &str) -> (String, Option<String>, String) {
    let pattern = pattern.trim();
    let (method, rest) = match pattern.split_once([' ', '\t']) {
        Some((m, rest)) if !m.contains('/') => (norm_method(m), rest.trim_start()),
        _ => (ANY.to_owned(), pattern),
    };
    let start = rest.find('/').unwrap_or(rest.len());
    let host = &rest[..start];
    let (host, _) = crate::endpoint::split_port(host);
    let host = (!host.is_empty()).then(|| host.to_ascii_lowercase());
    let path = if start < rest.len() {
        &rest[start..]
    } else {
        "/"
    };
    let path = if path.ends_with('/') {
        format!("{path}{{*}}")
    } else {
        path.to_owned()
    };
    (method, host, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::norm_path;
    use rb_syntax::extract;

    /// `METHOD path base handler` per route, the path normalised.
    fn summary(language: Language, src: &str) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        let r = routes(language, &facts);
        r.routes
            .iter()
            .map(|r| {
                let handler = match &r.handler {
                    Handler::Definition(i) => format!("def {}", facts.definitions[*i].name),
                    Handler::Inline(_) => "inline".to_owned(),
                    Handler::Name { text, .. } => format!("name {text}"),
                    Handler::Unknown(_) => "unknown".to_owned(),
                };
                let base = match r.base {
                    RouteBase::Root => "root".to_owned(),
                    RouteBase::Router(i) => format!("router {}", r_name(&facts, language, i)),
                    RouteBase::Unknown(b) => format!("unknown {}", binding_identity(&facts, b)),
                };
                let host = r
                    .host
                    .as_ref()
                    .map_or(String::new(), |h| format!(" host={h}"));
                format!("{} {} {base} {handler}{host}", r.method, norm_path(&r.path))
            })
            .collect()
    }

    fn r_name(facts: &FileFacts, language: Language, i: usize) -> String {
        routes(language, facts).routers[i].name.clone()
    }

    fn diagnostics(language: Language, src: &str) -> Vec<(String, String)> {
        let facts = extract(language, src.as_bytes()).unwrap();
        routes(language, &facts)
            .diagnostics
            .into_iter()
            .map(|d| (d.kind.to_owned(), d.detail))
            .collect()
    }

    /// The span of the first occurrence of `token`, from the text.
    fn at(src: &str, token: &str) -> Span {
        let start = src.find(token).expect(token);
        let pos = |byte: usize| {
            let before = &src[..byte];
            let row = before.matches('\n').count() as i32;
            (row, (byte - before.rfind('\n').map_or(0, |i| i + 1)) as i32)
        };
        let (a, b) = (pos(start), pos(start + token.len()));
        [a.0, a.1, b.0, b.1]
    }

    const SPRING: &str = "package com.acme.auth;

import org.springframework.web.bind.annotation.*;

@RestController
@RequestMapping(\"/api/v1\")
public class AuthController {
    @PostMapping(\"/login\")
    public String login(@RequestBody String email) { return \"t\"; }
    @GetMapping(\"/login\")
    public String loginPage() { return \"p\"; }
    // @PostMapping(\"/logout\")
    public void logout() {}
    @RequestMapping(value = {\"/a\", \"/b\"}, method = {RequestMethod.GET, RequestMethod.PUT})
    public void multi() {}
    @RequestMapping
    public void root() {}
    @GetMapping
    public void empty() {}
}
";

    #[test]
    fn spring_class_and_method_mappings_compose() {
        assert_eq!(
            summary(Language::Java, SPRING),
            [
                "POST /api/v1/login root def login",
                "GET /api/v1/login root def loginPage",
                "GET /api/v1/a root def multi",
                "PUT /api/v1/a root def multi",
                "GET /api/v1/b root def multi",
                "PUT /api/v1/b root def multi",
                "ANY /api/v1 root def root",
                "GET /api/v1 root def empty",
            ]
        );
        let facts = extract(Language::Java, SPRING.as_bytes()).unwrap();
        let r = routes(Language::Java, &facts);
        // The evidence is the method's mapping annotation.
        assert_eq!(r.routes[0].span, at(SPRING, "@PostMapping(\"/login\")"));
    }

    #[test]
    fn spring_needs_a_stereotype_and_its_import() {
        // D12: a type-level @RequestMapping alone is not a controller.
        let plain = "package p;\nimport org.springframework.web.bind.annotation.*;\n@RequestMapping(\"/x\")\nclass Legacy { @GetMapping(\"/y\") void y() {} }\n";
        assert!(summary(Language::Java, plain).is_empty());
        // @Controller from the stereotype package, imported by name.
        let controller = "package p;\nimport org.springframework.stereotype.Controller;\nimport org.springframework.web.bind.annotation.GetMapping;\n@Controller\nclass C { @GetMapping(\"/y\") void y() {} }\n";
        assert_eq!(summary(Language::Java, controller), ["GET /y root def y"]);
        // Same short names from another package: not Spring.
        let other = "package p;\nimport com.other.RestController;\nimport com.other.GetMapping;\n@RestController\nclass C { @GetMapping(\"/y\") void y() {} }\n";
        assert!(summary(Language::Java, other).is_empty());
        // Qualified names need no import.
        let qualified = "package p;\n@org.springframework.web.bind.annotation.RestController\nclass C { @org.springframework.web.bind.annotation.GetMapping(\"/q\") void y() {} }\n";
        assert_eq!(summary(Language::Java, qualified), ["GET /q root def y"]);
    }

    #[test]
    fn spring_interfaces_and_feign() {
        let iface = "package p;\nimport org.springframework.web.bind.annotation.*;\n@RequestMapping(\"/api\")\ninterface PetApi { @GetMapping(\"/pets\") List<Pet> list(); }\n";
        assert!(summary(Language::Java, iface).is_empty());
        assert_eq!(
            diagnostics(Language::Java, iface),
            [("route_on_interface".to_owned(), "PetApi#list".to_owned())]
        );
        let feign = "package com.acme.auth;\nimport org.springframework.cloud.openfeign.FeignClient;\nimport org.springframework.web.bind.annotation.PostMapping;\n@FeignClient(name = \"notification\", url = \"http://notification:3000\")\npublic interface MfaClient {\n    @PostMapping(\"/internal/mfa/email\")\n    void sendCode(String email);\n}\n";
        assert!(summary(Language::Java, feign).is_empty());
        assert!(diagnostics(Language::Java, feign).is_empty());
    }

    #[test]
    fn spring_dynamic_paths_are_counted() {
        let src = "package p;\nimport org.springframework.web.bind.annotation.*;\n@RestController\nclass C { @GetMapping(Paths.BASE) void y() {} @GetMapping(\"/ok\") void z() {} }\n";
        assert_eq!(summary(Language::Java, src), ["GET /ok root def z"]);
        assert_eq!(
            diagnostics(Language::Java, src),
            [("route_path_dynamic".to_owned(), "Paths.BASE".to_owned())]
        );
    }

    const EXPRESS_APP: &str = "import express from \"express\";\nimport { authRouter } from \"./routes/auth\";\n\nexport const app = express();\napp.use(express.json());\napp.use(\"/api/v1\", authRouter);\napp.get(\"/health\", health);\n// app.post(\"/commented\", h);\n";

    #[test]
    fn express_apps_routes_and_mounts() {
        assert_eq!(
            summary(Language::TypeScript, EXPRESS_APP),
            ["GET /health router app name health"]
        );
        let facts = extract(Language::TypeScript, EXPRESS_APP.as_bytes()).unwrap();
        let r = routes(Language::TypeScript, &facts);
        assert_eq!(
            r.routers,
            [RouterDecl {
                name: "app".into(),
                kind: RouterKind::App,
                span: facts
                    .bindings
                    .iter()
                    .find(|b| b.name == "app")
                    .unwrap()
                    .span,
                binding: Some(facts.bindings.iter().position(|b| b.name == "app").unwrap()),
                top_level: true,
            }]
        );
        assert_eq!(r.mounts.len(), 1, "{:?}", r.mounts);
        let m = &r.mounts[0];
        assert_eq!(
            (
                m.parent,
                m.prefix.as_deref(),
                m.child.as_str(),
                m.child_local
            ),
            (0, Some("/api/v1"), "authRouter", None)
        );
    }

    #[test]
    fn express_routers_route_chains_and_wrappers() {
        let src = "import { Router } from \"express\";\nexport const authRouter = Router();\nconst sub = Router();\nauthRouter.post(\"/login\", async (req, res) => { res.json(1); });\nauthRouter.route(\"/x/:id\").get(getX).put(auth, asyncHandler(async (req, res) => {}));\nauthRouter.use(\"/sub\", sub);\nsub.delete(\"/y\", ctrl.remove);\nauthRouter.all(\"/any\", h);\nauthRouter.get(\"setting\");\n";
        assert_eq!(
            summary(Language::TypeScript, src),
            [
                "POST /login router authRouter inline",
                "GET /x/{} router authRouter name getX",
                "PUT /x/{} router authRouter inline",
                "DELETE /y router sub name ctrl.remove",
                "ANY /any router authRouter name h",
            ]
        );
        let facts = extract(Language::TypeScript, src.as_bytes()).unwrap();
        let r = routes(Language::TypeScript, &facts);
        assert_eq!(r.mounts[0].child_local, Some(1));
        // The inline handler's span is the arrow function's.
        let Handler::Inline(span) = r.routes[0].handler else {
            panic!()
        };
        assert_eq!(span, at(src, "async (req, res) => { res.json(1); }"));
        assert_eq!(
            r.routes[0].span,
            at(
                src,
                "authRouter.post(\"/login\", async (req, res) => { res.json(1); })"
            )
        );
    }

    /// S3: a binding whose init chain starts with a Router construction binds, and
    /// every `use` link on any chain is a mount (not only single-link chains).
    #[test]
    fn express_chain_built_routers_bind_with_their_mounts() {
        let src = "import { Router } from \"express\";\nimport tags from \"./tags\";\nconst api = Router().use(tags).use(\"/v2\", tags);\napi.get(\"/ping\", h);\n";
        let facts = extract(Language::TypeScript, src.as_bytes()).unwrap();
        let r = routes(Language::TypeScript, &facts);
        assert_eq!(r.routers.len(), 1);
        assert_eq!(r.routers[0].name, "api");
        assert_eq!(r.routers[0].kind, RouterKind::Router);
        assert!(r.routers[0].binding.is_some());
        assert_eq!(r.mounts.len(), 2);
        assert_eq!(
            r.mounts
                .iter()
                .map(|m| (m.parent, m.prefix.clone(), m.child.clone()))
                .collect::<Vec<_>>(),
            [
                (0, Some(String::new()), "tags".to_owned()),
                (0, Some("/v2".to_owned()), "tags".to_owned())
            ]
        );
        assert_eq!(
            summary(Language::TypeScript, src),
            ["GET /ping router api name h"]
        );
    }

    /// S3: `export default Router().use(…)` binds an anonymous top-level router named
    /// `default`; an unexported inline construction binds nothing.
    #[test]
    fn express_anonymous_default_routers_bind() {
        let src = "import { Router } from \"express\";\nimport api from \"./api\";\nexport default Router().use('/api', api);\n";
        let facts = extract(Language::TypeScript, src.as_bytes()).unwrap();
        let r = routes(Language::TypeScript, &facts);
        assert_eq!(r.routers.len(), 1);
        assert_eq!(r.routers[0].name, "default");
        assert_eq!(r.routers[0].binding, None);
        assert!(r.routers[0].top_level);
        assert_eq!(r.mounts.len(), 1);
        assert_eq!(
            (
                r.mounts[0].parent,
                r.mounts[0].prefix.clone(),
                r.mounts[0].child.as_str()
            ),
            (0, Some("/api".to_owned()), "api")
        );
        // No default export: the same construction binds nothing.
        let inline = "import { Router } from \"express\";\nRouter().get('/x', h);\n";
        let facts = extract(Language::TypeScript, inline.as_bytes()).unwrap();
        let r = routes(Language::TypeScript, &facts);
        assert!(r.routers.is_empty());
        assert!(r.routes.is_empty());
    }

    /// S13: one `use` chain with n links costs linear work: 4× the links makes at
    /// most 5× the counted steps.
    #[test]
    fn express_use_chains_grow_linearly() {
        let work = |n: usize| {
            let mut src = "import { Router } from \"express\";\nconst api = Router()".to_owned();
            for i in 0..n {
                src.push_str(&format!(".use('/u{i}', c{i})"));
            }
            src.push_str(";\n");
            let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
            crate::work::take();
            let r = routes(Language::TypeScript, &facts);
            assert_eq!(r.mounts.len(), n);
            crate::work::take()
        };
        let (small, large) = (work(100), work(400));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }

    #[test]
    fn express_needs_the_express_import() {
        // A `Router()` or `app` from elsewhere is not Express.
        let src = "import { Router } from \"./my-router\";\nconst r = Router();\nr.get(\"/x\", h);\nconst app = makeApp();\napp.post(\"/y\", h);\n";
        assert!(summary(Language::TypeScript, src).is_empty());
        // `require("express")` and `express.Router()`.
        let cjs = "const express = require(\"express\");\nconst router = express.Router();\nrouter.get(\"/r\", h);\nmodule.exports = router;\n";
        assert_eq!(
            summary(Language::JavaScript, cjs),
            ["GET /r router router name h"]
        );
        // A parameter named like an app is not one.
        let param = "import express from \"express\";\nexport function routes(app) { app.get(\"/p\", h); }\n";
        assert!(summary(Language::JavaScript, param).is_empty());
    }

    const GIN: &str = "package main\n\nimport (\n\t\"net/http\"\n\n\t\"github.com/gin-gonic/gin\"\n)\n\nfunc main() {\n\tr := gin.Default()\n\tapi := r.Group(\"/billing\")\n\tapi.POST(\"/invoices/:id/pay\", payInvoice)\n\tr.POST(\"/internal/mfa/email\", mfaEcho)\n\tv := api.Group(\"/v\")\n\tv.GET(\"/x\", func(c *gin.Context) {})\n\tr.Group(\"/inline\").Handle(\"PATCH\", \"/y\", auth, h.Y)\n\tr.Handle(http.MethodDelete, \"/z\", h.Z)\n\tr.Any(\"/all\", h.All)\n}\n\nfunc register(g *gin.RouterGroup, e *gin.Engine) {\n\tg.GET(\"/param\", h.P)\n\te.GET(\"/engine\", h.E)\n}\n";

    #[test]
    fn gin_engines_groups_and_parameters() {
        assert_eq!(
            summary(Language::Go, GIN),
            [
                "POST /billing/invoices/{}/pay root name payInvoice",
                "POST /internal/mfa/email root name mfaEcho",
                "GET /billing/v/x root inline",
                "PATCH /inline/y root name h.Y",
                "DELETE /z root name h.Z",
                "ANY /all root name h.All",
                "GET /param unknown register.g name h.P",
                "GET /engine root name h.E",
            ]
        );
        // Without the gin import nothing is a gin route.
        let no_import = GIN.replace("\t\"github.com/gin-gonic/gin\"\n", "");
        assert!(
            summary(Language::Go, &no_import)
                .iter()
                .all(|r| !r.contains("billing"))
        );
        // An aliased import.
        let aliased = GIN
            .replace(
                "\t\"github.com/gin-gonic/gin\"",
                "\tg2 \"github.com/gin-gonic/gin\"",
            )
            .replace("gin.Default()", "g2.Default()");
        assert!(
            summary(Language::Go, &aliased)
                .contains(&"POST /billing/invoices/{}/pay root name payInvoice".to_owned())
        );
    }

    /// Contract M6d S9: chi routers enter as routes — top-level verbs,
    /// nested `Route` prefixes, prefix-less `Group`, `With` middleware
    /// chains, and `Method` dispatchers. Handler names resolve like gin's.
    #[test]
    fn chi_verbs_nested_routes_and_middleware() {
        let src = "package main\nimport \"github.com/go-chi/chi/v5\"\nfunc main() {\n\tr := chi.NewRouter()\n\tr.Get(\"/health\", health)\n\tr.Route(\"/api\", func(r chi.Router) {\n\t\tr.Post(\"/login\", login)\n\t\tr.Route(\"/v1\", func(r chi.Router) {\n\t\t\tr.Get(\"/users\", users)\n\t\t})\n\t})\n\tr.Group(func(r chi.Router) {\n\t\tr.Delete(\"/cache\", clear)\n\t})\n\tr.With(auth).Put(\"/profile\", update)\n\tsub := r.With(auth)\n\tsub.Get(\"/priv\", priv)\n\tr.Method(\"PATCH\", \"/patch\", patcher)\n\tr.Handle(\"/raw\", raw)\n}\n";
        assert_eq!(
            summary(Language::Go, src),
            [
                "GET /health root name health",
                "POST /api/login root name login",
                "GET /api/v1/users root name users",
                "DELETE /cache root name clear",
                "PUT /profile root name update",
                "GET /priv root name priv",
                "PATCH /patch root name patcher",
                "ANY /raw root name raw",
            ]
        );
        // Without the chi import nothing is a chi route; an aliased import
        // and `NewMux` work the same.
        let no_import = src.replace("\nimport \"github.com/go-chi/chi/v5\"", "");
        assert!(summary(Language::Go, &no_import).is_empty());
        let aliased = src
            .replace(
                "\"github.com/go-chi/chi/v5\"",
                "c \"github.com/go-chi/chi/v5\"",
            )
            .replace("chi.NewRouter()", "c.NewMux()");
        assert!(
            summary(Language::Go, &aliased).contains(&"POST /api/login root name login".to_owned())
        );
    }

    /// Contract M6d S9 boundaries: dynamic paths count, `Use` mounts no
    /// routes, `Mount` stays out (topology packs' job), and a named
    /// register function's router parameter does not resolve (cross-function
    /// group folding is the gin endpoint stage's job, not chi's inline one).
    #[test]
    fn chi_dynamic_mount_and_named_registers_stay_out() {
        let src = "package main\nimport \"github.com/go-chi/chi/v5\"\nfunc main() {\n\tr := chi.NewRouter()\n\tr.Get(p, h)\n\tr.Use(auth)\n\tr.Mount(\"/sub\", sub)\n\tr.With(func(r chi.Router) http.Handler {\n\t\tr.Get(\"/inner\", h)\n\t\treturn h\n\t}).Get(\"/w\", w)\n\tr.Foo(bar).Get(\"/direct\", h)\n\tr.Foo(bar).Route(\"/bad\", func(r chi.Router) {\n\t\tr.Get(\"/x\", h)\n\t})\n}\nfunc Register(r chi.Router) {\n\tr.Get(\"/x\", h)\n}\n";
        assert_eq!(summary(Language::Go, src), ["GET /w root name w"]);
        assert_eq!(
            diagnostics(Language::Go, src),
            [(
                "route_path_dynamic".to_owned(),
                "Name { text: \"p\", span: [4, 7, 4, 8] }".to_owned()
            )]
        );
    }

    /// S13: n sibling `Route` closures (n distinct parameters, each resolving
    /// through its own `Route` call) cost linear work: 4× the closures makes
    /// at most 5× the counted steps. Counted, not timed, so machine load
    /// cannot fail it. Parameters are distinctly named to isolate the chi
    /// walk: same-name shadowing bindings scan linearly in the shared
    /// `binding_index_at` machinery (pre-existing, all packs alike —
    /// measured 15.6× on this shape with one shared name, recorded in the
    /// contract amendment, not introduced here).
    #[test]
    fn chi_sibling_routes_grow_linearly() {
        let source = |n: usize| {
            let mut src = "package main\nimport \"github.com/go-chi/chi/v5\"\nfunc main() {\nr := chi.NewRouter()\n"
                .to_owned();
            for i in 0..n {
                src.push_str(&format!(
                    "r.Route(\"/p{i}\", func(r{i} chi.Router) {{\nr{i}.Get(\"/x\", h)\n}})\n"
                ));
            }
            src.push_str("}\n");
            src
        };
        let work = |n: usize| {
            let facts = extract(Language::Go, source(n).as_bytes()).unwrap();
            crate::work::take();
            let r = routes(Language::Go, &facts);
            assert_eq!(r.routes.len(), n);
            crate::work::take()
        };
        let (small, large) = (work(100), work(400));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }

    /// Contract M6d S9: FastAPI decorators enter as routes — app and router
    /// verbs, `APIRouter` prefixes, and `include_router` composition (with
    /// its own prefix). Handlers are the decorated functions.
    #[test]
    fn fastapi_decorators_prefixes_and_includes() {
        let src = "from fastapi import FastAPI, APIRouter\n\napp = FastAPI()\nrouter = APIRouter(prefix=\"/api\")\nv1 = APIRouter()\n\n@app.get(\"/health\")\ndef health():\n    pass\n\n@router.post(\"/login\")\ndef login():\n    pass\n\n@v1.get(\"/users\")\ndef users():\n    pass\n\n@v1.delete(\"/users/{id}\")\ndef drop_user():\n    pass\n\napp.include_router(router)\napp.include_router(v1, prefix=\"/v1\")\napp.include_router(router, prefix=\"/v2\")\n";
        assert_eq!(
            summary(Language::Python, src),
            [
                "GET /health root def health",
                "POST /api/login root def login",
                "GET /v1/users root def users",
                "DELETE /v1/users/{} root def drop_user",
                "POST /v2/api/login root def login",
            ]
        );
        // Without the fastapi import nothing is a route.
        let no_import = src.replace("from fastapi import FastAPI, APIRouter\n", "");
        assert!(summary(Language::Python, &no_import).is_empty());
    }

    /// S13: an include cycle keeps the legacy per-route walk — a router
    /// including its includer is degenerate input, and shared-prefix
    /// memoisation is only valid when no path revisits a router. This pins
    /// the legacy rows (a route keeps its collected path where the cycle
    /// cuts composition); the always-memo mutant serves y at `/y`.
    #[test]
    fn fastapi_include_cycles_keep_legacy_rows() {
        let src = "from fastapi import FastAPI, APIRouter\n\napp = FastAPI()\na = APIRouter(prefix=\"/a\")\nb = APIRouter(prefix=\"/b\")\n\n@app.get(\"/h\")\ndef h():\n    pass\n\n@a.get(\"/x\")\ndef x():\n    pass\n\n@b.get(\"/y\")\ndef y():\n    pass\n\napp.include_router(a)\na.include_router(b)\nb.include_router(a)\n";
        assert_eq!(
            summary(Language::Python, src),
            [
                "GET /h root def h",
                "GET /a/x root def x",
                "GET /a/b/y root def y",
            ]
        );
    }

    /// S13: n routers in one include chain (one route per router, each
    /// composing over every ancestor) cost linear work: 4× the routers
    /// makes at most 5× the counted steps. Counted, not timed, so machine
    /// load cannot fail it. Diamond includes fan out by design (each path
    /// serves the route, amendment 14 F4), so the ratio shape is a chain,
    /// whose work must stay linear in the input.
    #[test]
    fn fastapi_include_chains_grow_linearly() {
        let source = |n: usize| {
            let mut src = "from fastapi import FastAPI, APIRouter\n\napp = FastAPI()\n".to_owned();
            for i in 0..n {
                src.push_str(&format!("r{i} = APIRouter(prefix=\"/s{i}\")\n"));
            }
            for i in 0..n {
                src.push_str(&format!("@r{i}.get(\"/x\")\ndef h{i}():\n    pass\n\n"));
            }
            src.push_str("app.include_router(r0)\n");
            for i in 0..(n - 1) {
                src.push_str(&format!("r{i}.include_router(r{})\n", i + 1));
            }
            src
        };
        let work = |n: usize| {
            let facts = extract(Language::Python, source(n).as_bytes()).unwrap();
            crate::work::take();
            let r = routes(Language::Python, &facts);
            assert_eq!(r.routes.len(), n);
            crate::work::take()
        };
        let (small, large) = (work(100), work(400));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }

    /// Contract M6d S9 boundaries: dynamic paths count, `api_route` spreads
    /// its literal methods, websockets and undecorated functions stay out.
    #[test]
    fn fastapi_dynamic_api_route_and_non_routes() {
        let src = "from fastapi import FastAPI\n\napp = FastAPI()\n\n@app.get(p)\ndef dyn():\n    pass\n\n@app.api_route(\"/x\", methods=[\"GET\", \"POST\"])\ndef multi():\n    pass\n\n@app.api_route(\"/any\")\ndef any_method():\n    pass\n\n@app.websocket(\"/ws\")\nasync def ws():\n    pass\n\ndef plain():\n    pass\n";
        assert_eq!(
            summary(Language::Python, src),
            [
                "GET /x root def multi",
                "POST /x root def multi",
                "ANY /any root def any_method",
            ]
        );
        assert_eq!(
            diagnostics(Language::Python, src),
            [("route_path_dynamic".to_owned(), "@app.get".to_owned())]
        );
    }

    /// S2's foldable argument: a `Group(literal)` chain folds through the caller's
    /// bindings; a dynamic prefix stays unknown-but-gin; a non-group chain is not gin.
    /// The join is raw (`/api` + `/users`); `norm_path` cleans it downstream, as for
    /// `gin()`'s own paths.
    #[test]
    fn gin_group_argument_chains_fold() {
        let src = "package main\nimport \"github.com/gin-gonic/gin\"\nfunc main() {\n\tr := gin.Default()\n\tv1 := r.Group(\"/api\")\n\tusers.UsersRegister(v1.Group(\"/users\"))\n\tdyn := r.Group(prefix)\n\tusers.OtherRegister(dyn)\n}\n";
        let facts = rb_syntax::extract(Language::Go, src.as_bytes()).unwrap();
        let groups: Vec<String> = facts
            .chains
            .iter()
            .filter(|c| c.links.last().is_some_and(|l| l.name == "Group"))
            .map(|c| match gin_group_prefix(&facts, c) {
                Some(Some(p)) => format!("={p}"),
                Some(None) => "?".to_owned(),
                None => "-".to_owned(),
            })
            .collect();
        assert_eq!(groups, ["=/api", "=/api//users", "?"]);
    }

    /// A group of unknown prefix is identified by its receiver binding, qualified by the
    /// enclosing definitions and, for a Go method whose type is declared elsewhere, the receiver
    /// type; positions never enter it (contract M4a amendment 10).
    #[test]
    fn unknown_groups_are_identified_by_their_receiver() {
        let src = "package api\nimport \"github.com/gin-gonic/gin\"\ntype Local struct{}\nfunc (l *Local) Register(g *gin.RouterGroup) {\n\tg.GET(\"/a\", h)\n}\nfunc (r *Remote) Register(g *gin.RouterGroup) {\n\tv := g.Group(\"/v\")\n\tv.GET(\"/b\", h)\n}\n";
        assert_eq!(
            summary(Language::Go, src),
            [
                "GET /a unknown Local.Register.g name h",
                "GET /b unknown Remote.Register.v name h",
            ]
        );
        // Moving the code down a line changes no identity.
        let moved = src.replace("package api\n", "package api\n\n// moved\n");
        assert_eq!(summary(Language::Go, &moved), summary(Language::Go, src));
    }

    /// A group cycle through bindings (`a` from `b`, `b` from `a`) terminates with an unknown
    /// prefix; a chain of groups deeper than the cap is cut the same way.
    #[test]
    fn gin_group_cycles_and_depth_terminate() {
        // Go's extents never let a binding see a later one, so the cycle is built by widening
        // both package-level bindings' extents to the whole file.
        let cycle = "package main\nimport \"github.com/gin-gonic/gin\"\nvar a = b.Group(\"/a\")\nvar b = a.Group(\"/b\")\nfunc f() { a.GET(\"/x\", h) }\n";
        let mut facts = extract(Language::Go, cycle.as_bytes()).unwrap();
        for b in &mut facts.bindings {
            b.visible = [0, 0, 100, 0];
        }
        let got: Vec<(String, String)> = routes(Language::Go, &facts)
            .routes
            .into_iter()
            .map(|r| match r.base {
                RouteBase::Unknown(b) => (r.path, binding_identity(&facts, b)),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(got, [("/x".to_owned(), "a".to_owned())]);

        let n = 5_000;
        let mut src = String::from(
            "package main\nimport \"github.com/gin-gonic/gin\"\nfunc f() {\n\tg0 := gin.New()\n",
        );
        for i in 1..=n {
            src.push_str(&format!("\tg{i} := g{}.Group(\"/s\")\n", i - 1));
        }
        src.push_str(&format!(
            "\tg{n}.GET(\"/deep\", h)\n\tg3.GET(\"/shallow\", h)\n}}\n"
        ));
        let got = summary(Language::Go, &src);
        assert_eq!(got[0], "GET /deep unknown f.g5000 name h");
        assert_eq!(got[1], "GET /s/s/s/shallow root name h");
    }

    /// The packs index each file once (`FileScope`): four times the routers, mounts and client
    /// instances of a file make at most five times the lookups' work (linear is four; a scan per
    /// lookup, which made this quadratic, gives sixteen). The work is counted, not timed, so
    /// machine load cannot fail it (contract M4a amendment 12, Q4).
    #[test]
    fn packs_grow_linearly() {
        let source = |n: usize| {
            let mut src = String::from(
                "import express, { Router } from \"express\";\nimport axios from \"axios\";\nconst app = express();\n",
            );
            for i in 1..=n {
                src.push_str(&format!(
                    "const r{i} = Router();\nconst c{i} = axios.create({{ baseURL: \"http://s{i}\" }});\n"
                ));
            }
            src.push_str("app.use(\"/s\", r1);\n");
            for i in 1..n {
                src.push_str(&format!(
                    "r{i}.use(\"/s\", r{});\nr{i}.get(\"/x\", h);\nc{i}.get(\"/y\");\n",
                    i + 1
                ));
            }
            src
        };
        let work = |n: usize| {
            let facts = extract(Language::TypeScript, source(n).as_bytes()).unwrap();
            crate::work::take();
            let r = routes(Language::TypeScript, &facts);
            let c = crate::clients::clients(Language::TypeScript, &facts);
            assert_eq!((r.routes.len(), c.calls.len()), (n - 1, n - 1));
            crate::work::take()
        };
        let (small, large) = (work(500), work(2000));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }

    #[test]
    fn net_http_patterns() {
        let src = "package main\nimport \"net/http\"\nfunc main() {\n\thttp.HandleFunc(\"POST /items/{id}\", create)\n\tmux := http.NewServeMux()\n\tmux.HandleFunc(\"GET example.com/files/{path...}\", files)\n\tmux.Handle(\"/static/\", http.HandlerFunc(static))\n\tmux.HandleFunc(\"/exact/{$}\", exact)\n\tmux.HandleFunc(\"Example.com:8080/hosted\", hosted)\n\tother.HandleFunc(\"/no\", h)\n}\n";
        // A host pattern keeps its host, lower-cased without the port (evaluator pass 1, P8).
        assert_eq!(
            summary(Language::Go, src),
            [
                "POST /items/{} root name create",
                "GET /files/{*} root name files host=example.com",
                "ANY /static/{*} root name static",
                "ANY /exact root name exact",
                "ANY /hosted root name hosted host=example.com",
            ]
        );
        assert_eq!(
            serve_mux_pattern("/"),
            ("ANY".to_owned(), None, "/{*}".to_owned())
        );
        assert_eq!(
            serve_mux_pattern("GET api.example.com/v2/things"),
            (
                "GET".to_owned(),
                Some("api.example.com".to_owned()),
                "/v2/things".to_owned()
            )
        );
    }
}
