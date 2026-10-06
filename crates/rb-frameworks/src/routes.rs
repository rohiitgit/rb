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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Framework {
    Spring,
    Express,
    Gin,
    NetHttp,
}

impl Framework {
    pub fn as_str(self) -> &'static str {
        match self {
            Framework::Spring => "spring",
            Framework::Express => "express",
            Framework::Gin => "gin",
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
    /// Index into `FileFacts::bindings` of the binding ([`binding_identity`]).
    pub binding: usize,
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
            net_http(facts, &mut out);
        }
        Language::Python => {}
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
        self.members_by_name.get(name).map_or(&[], Vec::as_slice)
    }

    /// The definitions directly inside `parent`, in source order.
    pub fn children(&self, parent: usize) -> &[usize] {
        self.children_of.get(&parent).map_or(&[], Vec::as_slice)
    }

    /// The chain recorded at `span`.
    pub fn chain_at(&self, span: Span) -> Option<&'f CallChain> {
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
pub(crate) fn literal(arg: &Arg) -> Option<String> {
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
    let links: Vec<&str> = chain.links.iter().map(|l| l.name.as_str()).collect();
    match (chain.root.as_deref(), links.as_slice()) {
        (None, [f]) if names.module.contains(*f) => Some(RouterKind::App),
        (None, [f]) if names.router.contains(*f) => Some(RouterKind::Router),
        (Some(m), ["Router"]) if names.module.contains(m) => Some(RouterKind::Router),
        _ => None,
    }
}

fn express(facts: &FileScope<'_>, out: &mut FileRoutes) {
    let names = express_names(facts);
    if names.module.is_empty() && names.router.is_empty() {
        return;
    }
    let mut binding_router: HashMap<usize, usize> = HashMap::new(); // binding → router
    for (bi, b) in facts.bindings.iter().enumerate() {
        if b.member {
            continue;
        }
        if let Some(kind) = init_chain(facts, b).and_then(|c| router_kind(&names, c)) {
            binding_router.insert(bi, out.routers.len());
            out.routers.push(RouterDecl {
                name: b.name.clone(),
                kind,
                span: b.span,
                binding: bi,
                top_level: b.scope.is_none(),
            });
        }
    }
    let router_names: HashSet<String> = out.routers.iter().map(|r| r.name.clone()).collect();
    let router_at = |name: &str, at: Span| -> Option<usize> {
        if !router_names.contains(name) {
            return None;
        }
        let bi = facts.binding_index_at(name, at)?;
        binding_router.get(&bi).copied()
    };
    for chain in &facts.chains {
        let Some(root) = chain.root.as_deref() else {
            continue;
        };
        let Some(router) = router_at(root, chain.span) else {
            continue;
        };
        let links = &chain.links;
        // `use(prefix?, child…)` mounts.
        if links.len() == 1 && links[0].name == "use" {
            let args = &links[0].args;
            let (prefix, children) = match args.first() {
                Some(a @ Arg::Template { .. }) => (literal(a), &args[1..]),
                _ => (Some(String::new()), &args[..]),
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
        // `verb(path, …handlers)` and `route(path).verb(h).verb(h2)`.
        let (path, verbs) = match links.first() {
            Some(l) if l.name == "route" && links.len() > 1 => (l.args.first(), &links[1..]),
            _ => (None, &links[..]),
        };
        for link in verbs {
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
                binding: facts.bindings.iter().position(|b| b.name == "app").unwrap(),
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

    /// The packs index each file once (`FileScope`): doubling a file of routers, mounts and
    /// client instances costs at most 2.5 times as much (best of three; a ratio, not a
    /// wall-clock bound). A scan per lookup made this quadratic.
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
        let time = |n: usize| {
            let facts = extract(Language::TypeScript, source(n).as_bytes()).unwrap();
            (0..3)
                .map(|_| {
                    let start = std::time::Instant::now();
                    let r = routes(Language::TypeScript, &facts);
                    let c = crate::clients::clients(Language::TypeScript, &facts);
                    assert_eq!((r.routes.len(), c.calls.len()), (n - 1, n - 1));
                    start.elapsed()
                })
                .min()
                .unwrap()
        };
        let (small, large) = (time(2000), time(4000));
        let ratio = large.as_secs_f64() / small.as_secs_f64();
        assert!(ratio < 2.5, "{small:?} → {large:?} ({ratio:.2}×)");
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
