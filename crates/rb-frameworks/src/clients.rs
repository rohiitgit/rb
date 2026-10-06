//! HTTP-client packs (design §4.5, contract M4a S9–S11): `fetch` and axios (TS/JS),
//! `RestTemplate`, `WebClient`/`RestClient` and Feign (Java), and Go's `net/http`, from one
//! file's syntax facts. Each client call site gives a method (or unknown), a URL value and a
//! base reference; the indexer folds them ([`crate::fold`]) and stitches them to endpoints.
//!
//! Every client is recognised through an import (or a qualified name), never by a bare short
//! name (contract invariant 6).

use std::collections::{HashMap, HashSet};

use rb_syntax::{
    Arg, Binding, CallChain, Decoration, DefKind, FileFacts, Init, Language, Span, StrTemplate,
    TemplatePart,
};

use crate::endpoint::norm_method;
use crate::fold::local_binding;
use crate::routes::{FileScope, go_method, is_annotation, literal};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ClientKind {
    Fetch,
    Axios,
    RestTemplate,
    WebClient,
    RestClient,
    Feign,
    GoHttp,
}

impl ClientKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ClientKind::Fetch => "fetch",
            ClientKind::Axios => "axios",
            ClientKind::RestTemplate => "rest_template",
            ClientKind::WebClient => "web_client",
            ClientKind::RestClient => "rest_client",
            ClientKind::Feign => "feign",
            ClientKind::GoHttp => "net/http",
        }
    }
}

/// A URL or base as written, before folding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlValue {
    Template(StrTemplate),
    Name {
        text: String,
        span: Span,
    },
    /// Not a string or a name (a call result, a lambda, …).
    Unknown,
}

impl UrlValue {
    fn of(arg: Option<&Arg>) -> Self {
        match arg {
            Some(Arg::Template { template }) => UrlValue::Template(template.clone()),
            Some(Arg::Name { text, span }) => UrlValue::Name {
                text: text.clone(),
                span: *span,
            },
            _ => UrlValue::Unknown,
        }
    }

    fn lit(text: &str, span: Span) -> Self {
        UrlValue::Template(StrTemplate {
            parts: if text.is_empty() {
                Vec::new()
            } else {
                vec![TemplatePart::Lit {
                    text: text.to_owned(),
                }]
            },
            span,
        })
    }
}

/// One client call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCall {
    pub kind: ClientKind,
    /// Upper case; `None` when the method is not known statically.
    pub method: Option<String>,
    pub url: UrlValue,
    /// A base the URL is relative to (axios `baseURL`, `WebClient` `baseUrl`, Feign `url`).
    pub base: Option<UrlValue>,
    /// axios: the call is on this imported name, which the indexer must confirm is an axios
    /// instance (`axios.create(…)` bound in the imported module) before the call counts.
    pub instance: Option<String>,
    /// The client call (for Feign, the method's mapping annotation).
    pub span: Span,
    /// The definition the call is made from (for Feign, the interface method).
    pub source: Option<usize>,
    /// The base was read from a `@Bean` method of the file that builds a client of this kind,
    /// taken as the injected instance (dependency injection is M4b), so the site never reaches
    /// `synthetic` (contract M4a amendment 12).
    pub base_from_bean: bool,
    /// Gate A (contract M4a amendment 13): for a call on a client instance (or builder) whose
    /// base comes from its construction, whether nothing in the file can change that base
    /// afterwards; `None` when the call is on no such instance. An unverified base still
    /// restricts by host, but the site never reaches `synthetic`. For an imported instance the
    /// indexer decides.
    pub base_verified: Option<bool>,
    /// The name of the top-level axios instance the call is on, so the indexer can unverify
    /// it when a file importing it uses it otherwise ([`FileClients::escaped_imports`]).
    pub top_instance: Option<String>,
}

/// An axios instance bound to a name: `const api = axios.create({ baseURL })`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxiosInstance {
    pub name: String,
    pub base: Option<UrlValue>,
    pub top_level: bool,
    /// Gate A within this file: a constant (or never reassigned) binding used only as the
    /// receiver of request calls, whose `create` config has no spread after its `baseURL`.
    pub verified: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileClients {
    pub calls: Vec<ClientCall>,
    pub axios_instances: Vec<AxiosInstance>,
    /// TS/JS: imported names (and namespace aliases) this file uses other than as the receiver
    /// of a request call, sorted: an axios instance exported under such a name is unverified
    /// (contract M4a amendment 13).
    pub escaped_imports: Vec<String>,
}

/// Every client call site of one file, in source order per pack.
pub fn clients(language: Language, facts: &FileFacts) -> FileClients {
    let facts = &FileScope::new(facts);
    let mut out = FileClients::default();
    match language {
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            let uses = Uses::new(facts, language);
            fetch(facts, &mut out);
            axios(facts, &mut out, &uses);
        }
        Language::Java => {
            let bases = SpringBases::new(facts, Uses::new(facts, language));
            rest_template(facts, &mut out, &bases);
            web_client(facts, &mut out, &bases);
            feign(facts, &mut out);
        }
        Language::Go => go_http(facts, &mut out),
        Language::Python => {}
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Gate A: what may change a configured instance after its construction (amendment 13)

/// How each binding of a file is used after its declaration (from [`rb_syntax::NameUse`] facts
/// and the call chains rooted at it), indexed once per file.
struct Uses<'f> {
    /// Bindings used other than as a call's receiver: passed on, a member read or written
    /// (`api.defaults.baseURL = x`, `api.interceptors…`), spread, exported, returned.
    escaped: HashSet<usize>,
    /// Bindings assigned to after their declaration.
    written: HashSet<usize>,
    /// Per binding, the chains rooted at it: (chain index, first link's name).
    receivers: HashMap<usize, Vec<(usize, &'f str)>>,
    /// Names no binding declares (imports, TS/JS), used otherwise or as the receiver of a call
    /// not in `allowed`: per name, the first links of the chains rooted at it.
    unbound_escaped: HashSet<&'f str>,
    unbound_receivers: HashMap<&'f str, Vec<&'f str>>,
}

impl<'f> Uses<'f> {
    fn new(facts: &FileScope<'f>, language: Language) -> Self {
        let mut uses = Uses {
            escaped: HashSet::new(),
            written: HashSet::new(),
            receivers: HashMap::new(),
            unbound_escaped: HashSet::new(),
            unbound_receivers: HashMap::new(),
        };
        for u in &facts.facts.uses {
            crate::work::tick(1);
            match local_binding(facts, language, &u.name, u.span) {
                Some(bi) if u.write => {
                    uses.written.insert(bi);
                }
                Some(bi) => {
                    uses.escaped.insert(bi);
                }
                None if !u.name.contains('.') => {
                    uses.unbound_escaped.insert(&u.name);
                }
                None => {}
            }
        }
        for (ci, c) in facts.chains.iter().enumerate() {
            let (Some(root), Some(link)) = (c.root.as_deref(), c.links.first()) else {
                continue;
            };
            let simple = root.strip_prefix("this.").unwrap_or(root);
            if simple.contains(['.', '(', ' ']) {
                continue;
            }
            match local_binding(facts, language, root, c.span) {
                Some(bi) => uses
                    .receivers
                    .entry(bi)
                    .or_default()
                    .push((ci, link.name.as_str())),
                None if root == simple => uses
                    .unbound_receivers
                    .entry(root)
                    .or_default()
                    .push(link.name.as_str()),
                None => {}
            }
        }
        uses
    }

    /// Gate A for binding `bi`: never reassigned (or declared constant: a Java `final` field
    /// assigned in a constructor), never used other than as a receiver, and every call on it
    /// one of `allowed` (or the chain `own`, the one being judged).
    fn unchanged(
        &self,
        facts: &FileFacts,
        bi: usize,
        allowed: &[&str],
        own: Option<usize>,
    ) -> bool {
        (facts.bindings[bi].constant || !self.written.contains(&bi))
            && !self.escaped.contains(&bi)
            && self
                .receivers
                .get(&bi)
                .into_iter()
                .flatten()
                .all(|(ci, link)| Some(*ci) == own || allowed.contains(link))
    }

    /// Gate A for an imported name: used only as the receiver of `allowed` calls.
    fn unbound_unchanged(&self, name: &str, allowed: &[&str]) -> bool {
        !self.unbound_escaped.contains(name)
            && self
                .unbound_receivers
                .get(name)
                .into_iter()
                .flatten()
                .all(|link| allowed.contains(link))
    }
}

fn entry<'a>(arg: Option<&'a Arg>, key: &str) -> Option<&'a Arg> {
    match arg {
        Some(Arg::Object { entries, .. }) => {
            entries.iter().find(|e| e.key == key).map(|e| &e.value)
        }
        _ => None,
    }
}

/// A method from an object literal's `method` entry: `Some(None)` when the object has a
/// non-literal method, `None` when it has none.
fn method_entry(config: Option<&Arg>) -> Option<Option<String>> {
    entry(config, "method").map(|m| literal(m).map(|m| norm_method(&m)))
}

// ---------------------------------------------------------------------------------------------
// fetch (S9)

/// Modules whose default export (or `fetch` export) is `fetch`.
const FETCH_MODULES: &[&str] = &["node-fetch", "cross-fetch"];

fn fetch(facts: &FileScope<'_>, out: &mut FileClients) {
    // `node-fetch` and `cross-fetch` under any local name are `fetch` (contract M4a amendments
    // 12 and 13).
    let (mut node_fetch, mut requires) = (Vec::new(), HashSet::new());
    for module in FETCH_MODULES {
        let (names, required) = module_names(facts, module, &["default", "fetch"], false);
        node_fetch.extend(names);
        requires.extend(required);
    }
    // An import from another module or a top-level definition named `fetch` shadows the global
    // one everywhere in the file.
    let file_wide = facts.imports.iter().any(|i| {
        !FETCH_MODULES.contains(&i.module.as_str())
            && (i.alias.as_deref() == Some("fetch") || i.names.iter().any(|n| n.local == "fetch"))
    }) || facts
        .definitions
        .iter()
        .any(|d| d.name == "fetch" && d.parent.is_none());
    // `name` at `at` is `fetch`: a `node-fetch` name not shadowed by a parameter or variable
    // (its `require` binding is the module), or the global `fetch` not shadowed at all.
    let is_fetch = |name: &str, at: Span| {
        if node_fetch.iter().any(|n| n == name) {
            facts
                .binding_index_at(name, at)
                .is_none_or(|bi| requires.contains(&bi))
        } else {
            name == "fetch" && !file_wide && facts.binding_at("fetch", at).is_none()
        }
    };
    for chain in &facts.chains {
        let Some(link) = chain.links.first() else {
            continue;
        };
        let found = match chain.root.as_deref() {
            None => is_fetch(&link.name, chain.span),
            Some("window" | "globalThis" | "self") => {
                link.name == "fetch" && is_fetch("fetch", chain.span)
            }
            Some(_) => false,
        };
        if !found || link.args.is_empty() {
            continue;
        }
        let init = link.args.get(1);
        let method = match init {
            None => Some("GET".to_owned()),
            Some(Arg::Object { .. }) => method_entry(init).unwrap_or(Some("GET".to_owned())),
            Some(_) => None,
        };
        out.calls.push(ClientCall {
            kind: ClientKind::Fetch,
            method,
            url: UrlValue::of(link.args.first()),
            base: None,
            instance: None,
            span: chain.span,
            source: chain.enclosing,
            base_from_bean: false,
            base_verified: None,
            top_instance: None,
        });
    }
}

// ---------------------------------------------------------------------------------------------
// axios (S9)

const AXIOS_VERBS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];

/// Local names of a module's exports `exported` (`default` for a default import), plus its
/// namespace import when `namespace`, or a top-level `require(module)`; plus the indices of those
/// `require` bindings.
fn module_names(
    facts: &FileFacts,
    module: &str,
    exported: &[&str],
    namespace: bool,
) -> (Vec<String>, HashSet<usize>) {
    let mut requires = HashSet::new();
    let mut names: Vec<String> = facts
        .imports
        .iter()
        .filter(|i| i.module == module)
        .flat_map(|i| {
            i.alias.iter().filter(|_| namespace).cloned().chain(
                i.names
                    .iter()
                    .filter(|n| exported.contains(&n.name.as_str()))
                    .map(|n| n.local.clone()),
            )
        })
        .collect();
    for (i, b) in facts
        .bindings
        .iter()
        .enumerate()
        .filter(|(_, b)| b.scope.is_none() && !b.member)
    {
        if let Some(c) = init_chain(facts, b)
            && c.root.is_none()
            && c.links.len() == 1
            && c.links[0].name == "require"
            && c.links[0].args.first().and_then(literal).as_deref() == Some(module)
        {
            names.push(b.name.clone());
            requires.insert(i);
        }
    }
    (names, requires)
}

fn init_chain<'f>(facts: &'f FileFacts, b: &Binding) -> Option<&'f CallChain> {
    match b.init {
        Some(Init::Call { chain }) => facts.chains.get(chain),
        _ => None,
    }
}

/// The calls of an axios instance that leave its configuration as created (Gate A).
const AXIOS_REQUESTS: &[&str] = &[
    "get", "post", "put", "delete", "patch", "head", "options", "request",
];

fn axios(facts: &FileScope<'_>, out: &mut FileClients, uses: &Uses<'_>) {
    let (names, requires) = module_names(facts, "axios", &["default"], true);
    // `name` at `at` is the axios module: one of its local names, not shadowed by a parameter
    // or variable in effect there (evaluator pass 1, P6); the `require` binding is the module.
    let is_axios = |name: &str, at: Span| {
        names.iter().any(|n| n == name)
            && facts
                .binding_index_at(name, at)
                .is_none_or(|bi| requires.contains(&bi))
    };
    // Instances: `const api = axios.create({ baseURL })`, with Gate A (amendment 13).
    let mut instances: HashMap<usize, (Option<UrlValue>, bool)> = HashMap::new();
    for (bi, b) in facts.bindings.iter().enumerate().filter(|(_, b)| !b.member) {
        let Some(c) = init_chain(facts, b) else {
            continue;
        };
        if let (Some(root), [link]) = (c.root.as_deref(), c.links.as_slice())
            && is_axios(root, c.span)
            && link.name == "create"
        {
            let (base, spread) = create_base(link.args.first());
            let verified = !spread && uses.unchanged(facts, bi, AXIOS_REQUESTS, None);
            out.axios_instances.push(AxiosInstance {
                name: b.name.clone(),
                base: base.clone(),
                top_level: b.scope.is_none(),
                verified,
            });
            instances.insert(bi, (base, verified));
        }
    }
    let imported: HashSet<&str> = facts
        .imports
        .iter()
        .filter(|i| i.module != "axios")
        .flat_map(|i| {
            i.names
                .iter()
                .map(|n| n.local.as_str())
                .chain(i.alias.as_deref())
        })
        .collect();
    let mut escaped: Vec<String> = imported
        .iter()
        .filter(|n| !uses.unbound_unchanged(n, AXIOS_REQUESTS))
        .map(|n| (*n).to_owned())
        .collect();
    escaped.sort();
    out.escaped_imports = escaped;
    for chain in &facts.chains {
        let Some(first) = chain.links.first() else {
            continue;
        };
        // `axios(url | config)`, `axios(url, config)`.
        if chain.root.is_none() {
            if is_axios(&first.name, chain.span) && !first.args.is_empty() {
                let call = match first.args.first() {
                    Some(config @ Arg::Object { .. }) => config_call(chain, None, None, config),
                    url => axios_call(chain, None, None, url, None, first.args.get(1)),
                };
                out.calls.push(call);
            }
            continue;
        }
        let root = chain.root.as_deref().unwrap_or_default();
        let bound = facts.binding_index_at(root, chain.span);
        // An inline instance, `axios.create(config).get(…)`: no binding, so nothing changes
        // it after its construction but a spread in its config (evaluator pass 3, R8).
        if is_axios(root, chain.span)
            && first.name == "create"
            && let Some(link) = chain.links.get(1)
        {
            let (base, spread) = create_base(first.args.first());
            let verified = base.is_some().then_some(!spread);
            if let Some(mut call) = axios_request(chain, link, base, None) {
                call.base_verified = verified;
                out.calls.push(call);
            }
            continue;
        }
        let (base, instance, verified, top) = if is_axios(root, chain.span) {
            (None, None, None, None)
        } else if let Some((base, verified)) = bound.and_then(|bi| instances.get(&bi)) {
            let top = bound
                .filter(|&bi| facts.bindings[bi].scope.is_none())
                .map(|_| root.to_owned());
            (base.clone(), None, base.is_some().then_some(*verified), top)
        } else if bound.is_none() && imported.contains(root) {
            (None, Some(root.to_owned()), None, None)
        } else {
            continue;
        };
        if let Some(mut call) = axios_request(chain, first, base, instance) {
            call.base_verified = verified;
            call.top_instance = top;
            out.calls.push(call);
        }
    }
}

/// A request call `link` of `chain` on an instance with `base` (or on the module): the verbs
/// with their URL and request config, and `request(config)`.
fn axios_request(
    chain: &CallChain,
    link: &rb_syntax::CallLink,
    base: Option<UrlValue>,
    instance: Option<String>,
) -> Option<ClientCall> {
    let verb = link.name.as_str();
    if link.args.is_empty() {
        return None;
    }
    if AXIOS_VERBS.contains(&verb) {
        // `get/delete/head/options(url, config)`, `post/put/patch(url, data, config)`.
        let config = if matches!(verb, "post" | "put" | "patch") {
            link.args.get(2)
        } else {
            link.args.get(1)
        };
        let method = Some(norm_method(verb));
        return Some(axios_call(
            chain,
            base,
            instance,
            link.args.first(),
            method,
            config,
        ));
    }
    if verb != "request" {
        return None;
    }
    Some(match link.args.first() {
        Some(config @ Arg::Object { .. }) => config_call(chain, base, instance, config),
        // A config that cannot be read: no URL, method or base.
        _ => ClientCall {
            method: None,
            url: UrlValue::Unknown,
            base: Some(UrlValue::Unknown),
            ..axios_call(chain, base, instance, None, None, None)
        },
    })
}

/// The base an `axios.create(config)` states, and whether a spread after its `baseURL` (or
/// with none before it) may replace it: the instance keeps the stated base, unverified (Gate
/// A; spreads are never folded).
fn create_base(config: Option<&Arg>) -> (Option<UrlValue>, bool) {
    let Some(Arg::Object { entries, .. }) = config else {
        return (None, false);
    };
    let base = entries
        .iter()
        .rev()
        .find(|e| e.key == "baseURL")
        .map(|e| UrlValue::of(Some(&e.value)));
    let spread_after = entries
        .iter()
        .rev()
        .take_while(|e| e.key != "baseURL")
        .any(|e| e.key == "...");
    (base, spread_after)
}

/// The base a request config states (contract M4a amendment 12, Q1): `None` when the call has
/// no config, or a config object without a `baseURL` after its last spread, so the instance's
/// base applies; otherwise the stated base, [`UrlValue::Unknown`] when it cannot be read (a
/// config that is not an object literal, a spread that may set it, a `baseURL` that is not a
/// string or a name).
fn config_base(config: Option<&Arg>) -> Option<UrlValue> {
    match config? {
        Arg::Object { entries, .. } => {
            match entries
                .iter()
                .rev()
                .find(|e| e.key == "baseURL" || e.key == "...")
            {
                Some(e) if e.key == "baseURL" => Some(UrlValue::of(Some(&e.value))),
                Some(_) => Some(UrlValue::Unknown),
                None => None,
            }
        }
        _ => Some(UrlValue::Unknown),
    }
}

/// An axios call with its URL argument (`url`), its method (`None`: from the config, default
/// GET) and its request config: a base the config states replaces the instance's `base`.
fn axios_call(
    chain: &CallChain,
    base: Option<UrlValue>,
    instance: Option<String>,
    url: Option<&Arg>,
    method: Option<String>,
    config: Option<&Arg>,
) -> ClientCall {
    let method = method.or_else(|| match config {
        None => Some("GET".to_owned()),
        Some(Arg::Object { .. }) => method_entry(config).unwrap_or(Some("GET".to_owned())),
        Some(_) => None,
    });
    ClientCall {
        kind: ClientKind::Axios,
        method,
        url: UrlValue::of(url),
        base: config_base(config).or(base),
        instance,
        span: chain.span,
        source: chain.enclosing,
        base_from_bean: false,
        base_verified: None,
        top_instance: None,
    }
}

/// `axios(config)` or `request(config)` with an object literal: the URL comes from its `url`.
fn config_call(
    chain: &CallChain,
    base: Option<UrlValue>,
    instance: Option<String>,
    config: &Arg,
) -> ClientCall {
    axios_call(
        chain,
        base,
        instance,
        entry(Some(config), "url"),
        None,
        Some(config),
    )
}

// ---------------------------------------------------------------------------------------------
// Java: shared helpers

/// Whether a declared type (as written, possibly qualified) is `package.simple`.
fn java_type_is(facts: &FileFacts, type_name: &str, package: &str, simple: &str) -> bool {
    type_name == format!("{package}.{simple}")
        || (type_name == simple
            && facts.imports.iter().any(|i| {
                !i.is_static
                    && i.module == package
                    && (i.wildcard
                        || i.names
                            .iter()
                            .any(|n| n.name == simple && n.local == simple))
            }))
}

// ---------------------------------------------------------------------------------------------
// RestTemplate (S10)

const REST_TEMPLATE_METHODS: &[(&str, &str)] = &[
    ("getForObject", "GET"),
    ("getForEntity", "GET"),
    ("postForObject", "POST"),
    ("postForEntity", "POST"),
    ("postForLocation", "POST"),
    ("put", "PUT"),
    ("delete", "DELETE"),
    ("patchForObject", "PATCH"),
];

fn rest_template(facts: &FileScope<'_>, out: &mut FileClients, bases: &SpringBases) {
    for chain in &facts.chains {
        let (Some(root), Some(first)) = (chain.root.as_deref(), chain.links.first()) else {
            continue;
        };
        let method = match REST_TEMPLATE_METHODS.iter().find(|(m, _)| *m == first.name) {
            Some((_, m)) => Some(m.to_string()),
            None if matches!(first.name.as_str(), "exchange" | "execute") => {
                first.args.get(1).and_then(go_method_or_http)
            }
            None => continue,
        };
        let receiver = local_binding(facts, Language::Java, root, chain.span);
        let is_client = match receiver {
            Some(bi) => java_type_is(
                facts,
                &facts.bindings[bi].type_name,
                BLOCKING,
                "RestTemplate",
            ),
            // `new RestTemplate().getForObject(…)` (evaluator pass 3, R8): no root URI.
            None => {
                root.starts_with("new ")
                    && fresh_root(facts, chain)
                    && chain_kind(facts, chain) == Some(ClientKind::RestTemplate)
            }
        };
        if !is_client || first.args.is_empty() {
            continue;
        }
        // The root URI configured on the instance; a relative URL needs one (amendment 12).
        let base = bases.of(receiver, ClientKind::RestTemplate);
        let verified = match base.value {
            UrlValue::Unknown => None,
            _ => base.verified,
        };
        out.calls.push(ClientCall {
            kind: ClientKind::RestTemplate,
            method,
            url: java_url(facts, first.args.first()),
            base: Some(base.value),
            instance: None,
            span: chain.span,
            source: chain.enclosing,
            base_from_bean: base.from_bean,
            base_verified: verified,
            top_instance: None,
        });
    }
}

/// A Java URL argument: a string or a name, or `String.format(lit, args…)` as a template whose
/// verbs are holes (P1's Java form).
fn java_url(facts: &FileScope<'_>, arg: Option<&Arg>) -> UrlValue {
    match arg {
        Some(Arg::Other { span }) => ["String", "java.lang.String"]
            .iter()
            .find_map(|root| format_template(facts, root, "format", *span))
            .map_or(UrlValue::Unknown, UrlValue::Template),
        other => UrlValue::of(other),
    }
}

/// `HttpMethod.X` (Java) or a method string.
fn go_method_or_http(arg: &Arg) -> Option<String> {
    match arg {
        Arg::Name { text, .. }
            if text.starts_with("HttpMethod.") || text.contains(".HttpMethod.") =>
        {
            text.rsplit('.').next().map(norm_method)
        }
        other => literal(other).map(|m| norm_method(&m)),
    }
}

// ---------------------------------------------------------------------------------------------
// WebClient / RestClient (S10)

const REACTIVE: &str = "org.springframework.web.reactive.function.client";
const BLOCKING: &str = "org.springframework.web.client";

/// The client kind of a type name (`WebClient`, `RestClient`, their `Builder`s).
fn fluent_kind(facts: &FileFacts, type_name: &str) -> Option<ClientKind> {
    let simple = type_name.strip_suffix(".Builder").unwrap_or(type_name);
    if java_type_is(facts, simple, REACTIVE, "WebClient") {
        Some(ClientKind::WebClient)
    } else if java_type_is(facts, simple, BLOCKING, "RestClient") {
        Some(ClientKind::RestClient)
    } else {
        None
    }
}

/// What a chain that builds or configures a Spring client says about its base.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BaseFact {
    /// A base-setting link (`baseUrl(x)`, `rootUri(x)`, `create(x)`, a URI template handler or
    /// builder factory): its value, [`UrlValue::Unknown`] when it cannot be read.
    Stated(UrlValue),
    /// A fresh client or builder with no base (`new RestTemplateBuilder().build()`,
    /// `WebClient.builder().build()`).
    Fresh,
    /// Built from something not in sight (an injected builder, another instance).
    Unreadable,
}

/// The base a chain's links before `upto` set: the last base-setting link wins.
fn chain_base(facts: &FileScope<'_>, chain: &CallChain, upto: usize) -> BaseFact {
    for l in chain.links[..upto].iter().rev() {
        match l.name.as_str() {
            "baseUrl" | "rootUri" | "create" if !l.args.is_empty() => {
                return BaseFact::Stated(java_url(facts, l.args.first()));
            }
            "uriTemplateHandler" | "setUriTemplateHandler" | "uriBuilderFactory" => {
                return BaseFact::Stated(factory_base(l.args.first()));
            }
            _ => {}
        }
    }
    if fresh_root(facts, chain) {
        BaseFact::Fresh
    } else {
        BaseFact::Unreadable
    }
}

/// The base of `new DefaultUriBuilderFactory(base)`; anything else cannot be read.
fn factory_base(arg: Option<&Arg>) -> UrlValue {
    match arg {
        Some(Arg::New {
            type_name, args, ..
        }) if type_name.rsplit('.').next() == Some("DefaultUriBuilderFactory")
            && !args.is_empty() =>
        {
            UrlValue::of(args.first())
        }
        _ => UrlValue::Unknown,
    }
}

/// A chain starting from a new client or builder: `new RestTemplate…()`, or a static
/// `WebClient`/`RestClient` `create()`/`builder()`.
fn fresh_root(facts: &FileFacts, chain: &CallChain) -> bool {
    let Some(root) = chain.root.as_deref() else {
        return false;
    };
    if let Some(created) = root.strip_prefix("new ") {
        let simple = created.split(['(', '<']).next().unwrap_or("").trim();
        let simple = simple.rsplit('.').next().unwrap_or(simple);
        return matches!(simple, "RestTemplate" | "RestTemplateBuilder");
    }
    fluent_kind(facts, root).is_some()
        && chain
            .links
            .first()
            .is_some_and(|l| matches!(l.name.as_str(), "create" | "builder"))
}

/// The client kind a chain builds or configures: from its root's type (a binding, a static
/// `WebClient`/`RestClient`, or a `new RestTemplate…()`).
fn chain_kind(facts: &FileScope<'_>, chain: &CallChain) -> Option<ClientKind> {
    let root = chain.root.as_deref()?;
    let type_name = match root.strip_prefix("new ") {
        Some(created) => created
            .split(['(', '<'])
            .next()
            .unwrap_or("")
            .trim()
            .to_owned(),
        None => match local_binding(facts, Language::Java, root, chain.span) {
            Some(bi) => facts.bindings[bi].type_name.clone(),
            None => root.to_owned(),
        },
    };
    let rest = |simple: &str, package: &str| java_type_is(facts, &type_name, package, simple);
    if rest("RestTemplate", BLOCKING) || rest("RestTemplateBuilder", BOOT_CLIENT) {
        Some(ClientKind::RestTemplate)
    } else {
        fluent_kind(facts, &type_name)
    }
}

const BOOT_CLIENT: &str = "org.springframework.boot.web.client";
const SPRING_CONTEXT: &str = "org.springframework.context.annotation";

/// What configures each Spring client instance of one file, indexed once (contract M4a
/// amendment 12, Q1): per binding, the chains assigned to it (`this.rt = b.rootUri(…).build()`)
/// or calling a URI template handler setter on it; and the base facts of the file's `@Bean`
/// methods per client kind, for an instance injected from them. With the uses of every binding,
/// for Gate A (amendment 13).
struct SpringBases<'s, 'f> {
    facts: &'s FileScope<'f>,
    /// Binding → indices of the chains that configure it.
    configured: HashMap<usize, Vec<usize>>,
    beans: HashMap<ClientKind, Vec<BaseFact>>,
    uses: Uses<'f>,
}

/// A Spring client's base as [`SpringBases::of`] reads it.
struct SpringBase {
    value: UrlValue,
    from_bean: bool,
    /// Gate A: `None` when no base was read from the instance's construction.
    verified: Option<bool>,
}

/// The calls on a Spring client or builder binding that leave its configuration as built
/// (Gate A): requests, and `mutate()`, which copies.
fn spring_requests(facts: &FileFacts, type_name: &str) -> &'static [&'static str] {
    const BUILD: &[&str] = &["build"];
    const REST_TEMPLATE: &[&str] = &[
        "getForObject",
        "getForEntity",
        "postForObject",
        "postForEntity",
        "postForLocation",
        "put",
        "delete",
        "patchForObject",
        "exchange",
        "execute",
    ];
    const FLUENT: &[&str] = &[
        "get", "post", "put", "delete", "patch", "head", "options", "method", "mutate",
    ];
    if type_name.ends_with("Builder") {
        BUILD
    } else if fluent_kind(facts, type_name).is_some() {
        FLUENT
    } else {
        REST_TEMPLATE
    }
}

impl<'s, 'f> SpringBases<'s, 'f> {
    fn new(facts: &'s FileScope<'f>, uses: Uses<'f>) -> Self {
        let in_bean: HashSet<usize> = facts
            .definitions
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                d.kind == DefKind::Method
                    && d.decorations
                        .iter()
                        .any(|a| is_annotation(facts, a, SPRING_CONTEXT, "Bean"))
            })
            .map(|(i, _)| i)
            .collect();
        let mut configured: HashMap<usize, Vec<usize>> = HashMap::new();
        let mut beans: HashMap<ClientKind, Vec<BaseFact>> = HashMap::new();
        for (ci, chain) in facts.chains.iter().enumerate() {
            let target = match (&chain.assigned, chain.links.first()) {
                (Some(left), _) => Some(left.as_str()),
                (None, Some(l)) if l.name == "setUriTemplateHandler" => chain.root.as_deref(),
                _ => None,
            };
            if let Some(bi) =
                target.and_then(|t| local_binding(facts, Language::Java, t, chain.span))
            {
                configured.entry(bi).or_default().push(ci);
            }
            // A chain in a `@Bean` method (or a lambda inside one) that sets a base.
            let mut def = chain.enclosing;
            let mut seen = 0;
            while let Some(d) = def.filter(|d| !in_bean.contains(d) && seen < 64) {
                def = facts.definitions[d].parent;
                seen += 1;
            }
            if def.is_some()
                && let BaseFact::Stated(v) = chain_base(facts, chain, chain.links.len())
                && let Some(kind) = chain_kind(facts, chain)
            {
                beans.entry(kind).or_default().push(BaseFact::Stated(v));
            }
        }
        SpringBases {
            facts,
            configured,
            beans,
            uses,
        }
    }

    /// The base of a `kind` client bound at `receiver`: one stated base when everything that
    /// configures the instance agrees, else unknown. An instance nothing in the file
    /// configures is injected: the file's `@Bean` methods for that kind decide, the same way,
    /// and such a base is never verified. A base read from the instance is verified when
    /// neither the instance nor a mutable builder it was built from can change it (Gate A).
    fn of(&self, receiver: Option<usize>, kind: ClientKind) -> SpringBase {
        let unknown = SpringBase {
            value: UrlValue::Unknown,
            from_bean: false,
            verified: None,
        };
        let Some(bi) = receiver else {
            return unknown;
        };
        let b = &self.facts.bindings[bi];
        let mut chains: Vec<usize> = Vec::new();
        let mut facts: Vec<BaseFact> = Vec::new();
        match &b.init {
            Some(Init::Call { chain }) => {
                if let Some(c) = self.facts.chains.get(*chain) {
                    facts.push(chain_base(self.facts, c, c.links.len()));
                    chains.push(*chain);
                }
            }
            Some(_) => facts.push(BaseFact::Unreadable),
            None => {}
        }
        for &ci in self.configured.get(&bi).into_iter().flatten() {
            let c = &self.facts.chains[ci];
            facts.push(chain_base(self.facts, c, c.links.len()));
            chains.push(ci);
        }
        if facts.is_empty() {
            let beans = self.beans.get(&kind).map_or(&[][..], Vec::as_slice);
            return match agreed(beans) {
                Some(value) => SpringBase {
                    value,
                    from_bean: true,
                    verified: Some(false),
                },
                None => unknown,
            };
        }
        let Some(value) = agreed(&facts) else {
            return unknown;
        };
        let verified = self.uses.unchanged(
            self.facts,
            bi,
            spring_requests(self.facts, &b.type_name),
            None,
        ) && chains.iter().all(|&ci| self.builder_unchanged(ci));
        SpringBase {
            value,
            from_bean: false,
            verified: Some(verified),
        }
    }

    /// Gate A rule (3): when chain `ci` starts from a mutable builder binding
    /// (`WebClient.Builder`, `RestClient.Builder`; a `RestTemplateBuilder` returns a new
    /// builder from every call), that builder is used nowhere else but to `build()`.
    fn builder_unchanged(&self, ci: usize) -> bool {
        let c = &self.facts.chains[ci];
        let Some(root) = c.root.as_deref() else {
            return true;
        };
        let Some(rb) = local_binding(self.facts, Language::Java, root, c.span) else {
            return true;
        };
        let type_name = &self.facts.bindings[rb].type_name;
        if !(type_name.ends_with(".Builder") && fluent_kind(self.facts, type_name).is_some()) {
            return true;
        }
        self.uses.unchanged(self.facts, rb, &["build"], Some(ci))
    }
}

/// The one base all `facts` state (fresh clients state none), when none is unreadable and at
/// least one states it; two different values, or a value that cannot be read, give `None`.
fn agreed(facts: &[BaseFact]) -> Option<UrlValue> {
    let mut found: Option<&UrlValue> = None;
    for f in facts {
        match f {
            BaseFact::Unreadable | BaseFact::Stated(UrlValue::Unknown) => return None,
            BaseFact::Fresh => {}
            BaseFact::Stated(v) => match found {
                Some(prev) if !same_value(prev, v) => return None,
                _ => found = Some(v),
            },
        }
    }
    found.cloned()
}

/// Two URL values with the same text (spans aside).
fn same_value(a: &UrlValue, b: &UrlValue) -> bool {
    match (a, b) {
        (UrlValue::Template(x), UrlValue::Template(y)) => {
            x.parts.len() == y.parts.len()
                && x.parts.iter().zip(&y.parts).all(|p| match p {
                    (TemplatePart::Lit { text: s }, TemplatePart::Lit { text: t }) => s == t,
                    (TemplatePart::Hole { expr: s, .. }, TemplatePart::Hole { expr: t, .. }) => {
                        s == t
                    }
                    _ => false,
                })
        }
        (UrlValue::Name { text: x, .. }, UrlValue::Name { text: y, .. }) => x == y,
        _ => false,
    }
}

fn web_client(facts: &FileScope<'_>, out: &mut FileClients, bases: &SpringBases) {
    for (ci, chain) in facts.chains.iter().enumerate() {
        let Some(root) = chain.root.as_deref() else {
            continue;
        };
        let receiver = local_binding(facts, Language::Java, root, chain.span);
        let kind = match receiver {
            Some(bi) => fluent_kind(facts, &facts.bindings[bi].type_name),
            // `WebClient.create(…)`, `RestClient.builder()…`.
            None => fluent_kind(facts, root),
        };
        let Some(kind) = kind else { continue };
        let verb = chain.links.iter().position(|l| {
            matches!(
                l.name.as_str(),
                "get" | "post" | "put" | "delete" | "patch" | "head" | "options"
            ) && l.args.is_empty()
                || l.name == "method" && l.args.len() == 1
        });
        let Some(verb) = verb else { continue };
        let link = &chain.links[verb];
        let method = if link.name == "method" {
            go_method_or_http(&link.args[0])
        } else {
            Some(norm_method(&link.name))
        };
        let url = chain.links[verb..]
            .iter()
            .find(|l| l.name == "uri")
            .map_or_else(
                || UrlValue::lit("", chain.span),
                |l| java_url(facts, l.args.first()),
            );
        // A base stated in the call's own chain wins; otherwise the instance's (amendment 12).
        // A server-side client has no origin of its own, so with no base in sight the base is
        // unknown, never empty: a relative URI is then matched by suffix (evaluator pass 1).
        let base = match chain_base(facts, chain, verb) {
            // Its own chain's base: changed later only through a mutable builder it starts
            // from (Gate A).
            BaseFact::Stated(value) => SpringBase {
                value,
                from_bean: false,
                verified: Some(bases.builder_unchanged(ci)),
            },
            BaseFact::Fresh => SpringBase {
                value: UrlValue::Unknown,
                from_bean: false,
                verified: None,
            },
            BaseFact::Unreadable => bases.of(receiver, kind),
        };
        let verified = match base.value {
            UrlValue::Unknown => None,
            _ => base.verified,
        };
        out.calls.push(ClientCall {
            kind,
            method,
            url,
            base: Some(base.value),
            instance: None,
            span: chain.span,
            source: chain.enclosing,
            base_from_bean: base.from_bean,
            base_verified: verified,
            top_instance: None,
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Feign (S10)

const FEIGN_PACKAGES: &[&str] = &[
    "org.springframework.cloud.openfeign",
    "org.springframework.cloud.netflix.feign",
];
const SPRING_WEB: &str = "org.springframework.web.bind.annotation";
const MAPPINGS: &[(&str, Option<&str>)] = &[
    ("RequestMapping", None),
    ("GetMapping", Some("GET")),
    ("PostMapping", Some("POST")),
    ("PutMapping", Some("PUT")),
    ("DeleteMapping", Some("DELETE")),
    ("PatchMapping", Some("PATCH")),
];

/// The single static path of a mapping (`value`, `path` or positional), `""` when absent;
/// `None` when it is not one static string.
fn single_path(d: &Decoration) -> Option<String> {
    let arg = d
        .args
        .iter()
        .find(|a| matches!(a.key.as_deref(), Some("value" | "path")))
        .or_else(|| d.args.iter().find(|a| a.key.is_none()));
    match arg {
        None => Some(String::new()),
        Some(a) if a.items.len() == 1 && a.items[0].literal => Some(a.items[0].value.clone()),
        Some(a) if a.items.is_empty() && a.literal => Some(a.value.clone()),
        Some(_) => None,
    }
}

fn keyed<'d>(d: &'d Decoration, keys: &[&str]) -> Option<&'d rb_syntax::DecorationArg> {
    keys.iter()
        .find_map(|k| d.args.iter().find(|a| a.key.as_deref() == Some(k)))
}

fn feign(facts: &FileScope<'_>, out: &mut FileClients) {
    let defs = &facts.definitions;
    for (iface, def) in defs.iter().enumerate() {
        if def.kind != DefKind::Interface {
            continue;
        }
        let Some(client) = def.decorations.iter().find(|d| {
            FEIGN_PACKAGES
                .iter()
                .any(|p| is_annotation(facts, d, p, "FeignClient"))
        }) else {
            continue;
        };
        // `url`, else the service name (`name`, `value` or positional) as the host.
        let base = match keyed(client, &["url"]) {
            Some(a) if a.literal => UrlValue::lit(&a.value, client.span),
            Some(a) => UrlValue::Name {
                text: a.value.clone(),
                span: client.span,
            },
            None => match keyed(client, &["name", "value"])
                .or_else(|| client.args.iter().find(|a| a.key.is_none()))
            {
                Some(a) if a.literal => UrlValue::lit(&format!("http://{}", a.value), client.span),
                _ => UrlValue::Unknown,
            },
        };
        let feign_path = keyed(client, &["path"]).map(|a| {
            if a.literal {
                Some(a.value.clone())
            } else {
                None
            }
        });
        let class_path = def
            .decorations
            .iter()
            .find(|d| is_annotation(facts, d, SPRING_WEB, "RequestMapping"))
            .map(single_path);
        for &m in facts.children(iface) {
            let method_def = &defs[m];
            if method_def.kind != DefKind::Method {
                continue;
            }
            for d in &method_def.decorations {
                let Some((_, fixed)) = MAPPINGS
                    .iter()
                    .find(|(name, _)| is_annotation(facts, d, SPRING_WEB, name))
                else {
                    continue;
                };
                let method = match fixed {
                    Some(f) => Some(f.to_string()),
                    None => keyed(d, &["method"])
                        .filter(|a| a.items.len() <= 1)
                        .and_then(|a| {
                            a.items
                                .first()
                                .map(|i| i.value.clone())
                                .or(Some(a.value.clone()))
                        })
                        .and_then(|v| v.rsplit('.').next().map(norm_method)),
                };
                let parts = [
                    feign_path.clone().unwrap_or(Some(String::new())),
                    class_path.clone().unwrap_or(Some(String::new())),
                    single_path(d),
                ];
                let url = match parts.into_iter().collect::<Option<Vec<String>>>() {
                    Some(parts) => UrlValue::lit(
                        &parts
                            .iter()
                            .filter(|p| !p.is_empty())
                            .map(|p| format!("/{}", p.trim_start_matches('/')))
                            .collect::<String>(),
                        d.span,
                    ),
                    None => UrlValue::Unknown,
                };
                out.calls.push(ClientCall {
                    kind: ClientKind::Feign,
                    method,
                    url,
                    base: Some(base.clone()),
                    instance: None,
                    span: d.span,
                    source: Some(m),
                    base_from_bean: false,
                    base_verified: None,
                    top_instance: None,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Go net/http (S11)

fn go_http(facts: &FileScope<'_>, out: &mut FileClients) {
    let import_name = |module: &str| {
        facts
            .imports
            .iter()
            .find(|i| i.module == module)
            .and_then(|i| match i.alias.as_deref() {
                Some("_" | ".") => None,
                Some(a) => Some(a.to_owned()),
                None => module.rsplit('/').next().map(str::to_owned),
            })
    };
    let Some(http) = import_name("net/http") else {
        return;
    };
    let fmt = import_name("fmt");
    for chain in &facts.chains {
        let (Some(root), [link]) = (chain.root.as_deref(), chain.links.as_slice()) else {
            continue;
        };
        let on_package = root == http;
        let on_client = !on_package
            && (root == format!("{http}.DefaultClient")
                || facts
                    .binding_at(root, chain.span)
                    .is_some_and(|b| b.type_name == format!("{http}.Client")));
        let (method, url) = match link.name.as_str() {
            "Get" | "Head" | "Post" | "PostForm" if on_package || on_client => {
                let m = if link.name == "PostForm" {
                    "POST"
                } else {
                    &link.name
                };
                (Some(norm_method(m)), link.args.first())
            }
            "NewRequest" if on_package && link.args.len() >= 2 => {
                (go_method(&link.args[0]), link.args.get(1))
            }
            "NewRequestWithContext" if on_package && link.args.len() >= 3 => {
                (go_method(&link.args[1]), link.args.get(2))
            }
            _ => continue,
        };
        let url = match url {
            Some(Arg::Other { span }) => fmt
                .as_deref()
                .and_then(|fmt| format_template(facts, fmt, "Sprintf", *span))
                .map_or(UrlValue::Unknown, UrlValue::Template),
            other => UrlValue::of(other),
        };
        out.calls.push(ClientCall {
            kind: ClientKind::GoHttp,
            method,
            url,
            base: None,
            instance: None,
            span: chain.span,
            source: chain.enclosing,
            base_from_bean: false,
            base_verified: None,
            top_instance: None,
        });
    }
}

/// `<root>.<name>(lit, args…)` at `span` — Go's `fmt.Sprintf`, Java's `String.format` — as a
/// template whose verbs are holes naming their arguments (`%s %d %v %q` and the rest; `%%` is a
/// literal `%`).
fn format_template(
    facts: &FileScope<'_>,
    root: &str,
    name: &str,
    span: Span,
) -> Option<StrTemplate> {
    let chain = facts.chain_at(span)?;
    let [link] = chain.links.as_slice() else {
        return None;
    };
    if chain.root.as_deref() != Some(root) || link.name != name {
        return None;
    }
    let format = literal(link.args.first()?)?;
    let mut parts = Vec::new();
    let mut text = String::new();
    let mut next_arg = 1;
    let mut chars = format.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            text.push(c);
            continue;
        }
        if chars.peek() == Some(&'%') {
            chars.next();
            text.push('%');
            continue;
        }
        // Flags, width and precision, then the verb letter.
        while chars.peek().is_some_and(|c| !c.is_ascii_alphabetic()) {
            chars.next();
        }
        chars.next();
        if !text.is_empty() {
            parts.push(TemplatePart::Lit {
                text: std::mem::take(&mut text),
            });
        }
        let (expr, at) = match link.args.get(next_arg) {
            Some(Arg::Name { text, span }) => (text.clone(), *span),
            Some(other) => (String::new(), other.span()),
            None => (String::new(), span),
        };
        parts.push(TemplatePart::Hole { expr, span: at });
        next_arg += 1;
    }
    if !text.is_empty() {
        parts.push(TemplatePart::Lit { text });
    }
    Some(StrTemplate { parts, span })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rb_syntax::extract;

    fn show_value(v: &UrlValue) -> String {
        match v {
            UrlValue::Template(t) => {
                let s: String = t
                    .parts
                    .iter()
                    .map(|p| match p {
                        TemplatePart::Lit { text } => text.clone(),
                        TemplatePart::Hole { expr, .. } => format!("{{{expr}}}"),
                    })
                    .collect();
                format!("'{s}'")
            }
            UrlValue::Name { text, .. } => text.clone(),
            UrlValue::Unknown => "?".into(),
        }
    }

    /// `kind METHOD url [base] [instance]` per call.
    fn calls(language: Language, src: &str) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        clients(language, &facts)
            .calls
            .iter()
            .map(|c| {
                let mut s = format!(
                    "{} {} {}",
                    c.kind.as_str(),
                    c.method.as_deref().unwrap_or("?"),
                    show_value(&c.url)
                );
                if let Some(b) = &c.base {
                    s.push_str(&format!(" base={}", show_value(b)));
                }
                if let Some(i) = &c.instance {
                    s.push_str(&format!(" instance={i}"));
                }
                if c.base_from_bean {
                    s.push_str(" from_bean");
                }
                if c.base_verified == Some(false) {
                    s.push_str(" unverified");
                }
                s
            })
            .collect()
    }

    #[test]
    fn fetch_methods_and_shadowing() {
        let src = "export async function f(base, id) {\n  await fetch(`${base}/invoices/${id}/pay`, { method: \"POST\" });\n  await fetch(\"/a\");\n  await fetch(\"/b\", { headers });\n  await fetch(\"/c\", init);\n  await fetch(\"/d\", { method: m });\n  await window.fetch(\"/e\").then(r => r.json());\n  // fetch(\"/commented\");\n}\nfunction g(fetch) { fetch(\"/shadowed\"); }\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "fetch POST '{base}/invoices/{id}/pay'",
                "fetch GET '/a'",
                "fetch GET '/b'",
                "fetch ? '/c'",
                "fetch ? '/d'",
                "fetch GET '/e'",
            ]
        );
        // Another module's `fetch` shadows the global one.
        let imported = "import fetch from \"./my-fetch\";\nfetch(\"/x\");\n";
        assert!(calls(Language::TypeScript, imported).is_empty());
        let defined = "function fetch(u) {}\nfetch(\"/x\");\n";
        assert!(calls(Language::JavaScript, defined).is_empty());
    }

    #[test]
    fn axios_calls_configs_and_instances() {
        let src = "import http from \"axios\";\nimport { api as shared } from \"./http\";\nconst API = \"http://gateway\";\nconst api = http.create({ baseURL: API });\nexport async function f(creds) {\n  await http.post(`${API}/api/v1/login`, creds);\n  await http(\"/plain\");\n  await http({ url: \"/cfg\", method: \"put\" });\n  await http.request({ url: \"/req\" });\n  await api.get(\"/inst\");\n  await shared.delete(\"/imported\");\n  await other.get(\"/not-axios\");\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "axios POST '{API}/api/v1/login'",
                "axios GET '/plain'",
                "axios PUT '/cfg'",
                "axios GET '/req'",
                "axios GET '/inst' base=API",
                "axios DELETE '/imported' instance=shared",
            ]
        );
        let facts = extract(Language::TypeScript, src.as_bytes()).unwrap();
        assert_eq!(
            clients(Language::TypeScript, &facts).axios_instances,
            [AxiosInstance {
                name: "api".into(),
                base: Some(UrlValue::Name {
                    text: "API".into(),
                    span: {
                        let start = src.find("baseURL: API").unwrap() + "baseURL: ".len();
                        let row = src[..start].matches('\n').count() as i32;
                        let col = (start - src[..start].rfind('\n').unwrap() - 1) as i32;
                        [row, col, row, col + 3]
                    }
                }),
                top_level: true,
                verified: true,
            }]
        );
        // A parameter or local named like the import shadows it: not axios (evaluator pass 1,
        // P6), here and for `axios(…)` and `axios.create(…)`.
        let shadowed = "import axios from \"axios\";\nexport async function p(axios: { get(u: string): void }) {\n  axios.get(\"/users/me\");\n  axios(\"/x\");\n}\nexport function l() {\n  const axios = mk();\n  axios.post(\"/y\", 1);\n  const api = axios.create({ baseURL: \"http://z\" });\n  api.get(\"/z\");\n}\nexport function ok() { axios.get(\"/real\"); }\n";
        assert_eq!(calls(Language::TypeScript, shadowed), ["axios GET '/real'"]);
        let facts = extract(Language::TypeScript, shadowed.as_bytes()).unwrap();
        assert!(
            clients(Language::TypeScript, &facts)
                .axios_instances
                .is_empty()
        );
        // `const axios = require("axios")` is the module itself, not a shadow.
        let cjs = "const axios = require(\"axios\");\nfunction f() { axios.get(\"/c\"); }\n";
        assert_eq!(calls(Language::JavaScript, cjs), ["axios GET '/c'"]);
        // Without the axios import, only a call on an imported name is kept, as a candidate the
        // indexer drops unless the import resolves to an axios instance; a local is nothing.
        let none = "import http from \"./my-http\";\nhttp.post(\"/x\", 1);\nconst local = mk();\nlocal.get(\"/y\");\n";
        assert_eq!(
            calls(Language::TypeScript, none),
            ["axios POST '/x' instance=http"]
        );
    }

    #[test]
    fn rest_template_web_client_and_rest_client() {
        let src = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.web.client.RestClient;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass C {\n  private final RestTemplate restTemplate;\n  private final WebClient web = WebClient.builder().baseUrl(\"http://customers-service\").build();\n  private final WebClient.Builder builder;\n  private final RestClient rest;\n  void m(String id) {\n    restTemplate.getForObject(\"http://visits/owners/{id}\", String.class, id);\n    this.restTemplate.exchange(BASE + \"/x\", HttpMethod.PUT, null, String.class);\n    restTemplate.exchange(\"/y\", method, null, String.class);\n    web.get().uri(\"/owners/{ownerId}\", id).retrieve().bodyToMono(String.class);\n    builder.build().post().uri(\"http://visits-service/pets/visits\").retrieve();\n    rest.method(HttpMethod.DELETE).uri(\"/r\").retrieve();\n    WebClient.create(\"http://x\").get().retrieve();\n    other.getForObject(\"/no\", String.class);\n  }\n}\n";
        assert_eq!(
            calls(Language::Java, src),
            [
                "rest_template GET 'http://visits/owners/{id}' base=?",
                "rest_template PUT '{BASE}/x' base=?",
                "rest_template ? '/y' base=?",
                "web_client GET '/owners/{ownerId}' base='http://customers-service'",
                "web_client POST 'http://visits-service/pets/visits' base=?",
                "rest_client DELETE '/r' base=?",
                "web_client GET '' base='http://x'",
            ]
        );
        // A server-side client has no origin of its own: with no base in sight the base is
        // unknown, so a relative URI is matched by suffix (evaluator pass 1, suspected item); a
        // base set in a constructor is in sight (amendment 12). `String.format` is a template
        // (P1's Java form).
        let ctor = "package p;\nimport org.springframework.web.reactive.function.client.WebClient;\nimport org.springframework.web.client.RestTemplate;\nclass C {\n  private final WebClient client;\n  RestTemplate rest;\n  C(WebClient.Builder b) { this.client = b.baseUrl(\"http://customers\").build(); }\n  void m(int port, String id) {\n    client.get().uri(\"/owners\").retrieve();\n    rest.getForObject(String.format(\"http://localhost:%d/x/%s\", port, id), String.class);\n  }\n}\n";
        assert_eq!(
            calls(Language::Java, ctor),
            [
                "rest_template GET 'http://localhost:{port}/x/{id}' base=?",
                "web_client GET '/owners' base='http://customers'",
            ]
        );
        // Same short names from another package are not clients.
        let other = src
            .replace(
                "org.springframework.web.client.RestTemplate",
                "com.other.RestTemplate",
            )
            .replace(
                "org.springframework.web.client.RestClient",
                "com.other.RestClient",
            )
            .replace(
                "org.springframework.web.reactive.function.client.WebClient",
                "com.other.WebClient",
            );
        assert!(calls(Language::Java, &other).is_empty());
    }

    #[test]
    fn feign_clients() {
        let src = "package com.acme.auth;\n\nimport org.springframework.cloud.openfeign.FeignClient;\nimport org.springframework.web.bind.annotation.PostMapping;\nimport org.springframework.web.bind.annotation.RequestMapping;\n\n@FeignClient(name = \"notification\", url = \"http://notification:3000\")\npublic interface MfaClient {\n    @PostMapping(\"/internal/mfa/email\")\n    void sendCode(String email);\n}\n\n@FeignClient(value = \"visits-service\", path = \"/api\")\n@RequestMapping(\"/pets\")\ninterface Visits {\n    @RequestMapping(value = \"/visits\", method = RequestMethod.GET)\n    List<Visit> all();\n    void notMapped();\n}\n\n@FeignClient(name = \"x\", url = \"${x.url}\")\ninterface Configured { @PostMapping(\"/y\") void y(); }\n";
        assert_eq!(
            calls(Language::Java, src),
            [
                "feign POST '/internal/mfa/email' base='http://notification:3000'",
                "feign GET '/api/pets/visits' base='http://visits-service'",
                "feign POST '/y' base='${x.url}'",
            ]
        );
        let facts = extract(Language::Java, src.as_bytes()).unwrap();
        let first = &clients(Language::Java, &facts).calls[0];
        // The evidence is the mapping annotation; the source the interface method.
        let start = src.find("@PostMapping(\"/internal/mfa/email\")").unwrap();
        assert_eq!(first.span[0] as usize, src[..start].matches('\n').count());
        assert_eq!(facts.definitions[first.source.unwrap()].name, "sendCode");
        // Without the Feign import it is just an interface.
        let plain = src.replace(
            "import org.springframework.cloud.openfeign.FeignClient;\n",
            "",
        );
        assert!(calls(Language::Java, &plain).is_empty());
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q8): `node-fetch`, by default import, a
    /// `fetch` import or a top-level `require`, under any local name, is `fetch`.
    #[test]
    fn node_fetch_is_fetch() {
        let src = "import fetch from \"node-fetch\";\nimport nf from \"node-fetch\";\nexport async function f(u) {\n  await fetch(`${u}/orders/1`);\n  await nf(\"/charges\", { method: \"POST\" });\n}\nfunction g(fetch) { fetch(\"/shadowed\"); }\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            ["fetch GET '{u}/orders/1'", "fetch POST '/charges'"]
        );
        let named = "import { fetch as get } from \"node-fetch\";\nget(\"/n\");\n";
        assert_eq!(calls(Language::TypeScript, named), ["fetch GET '/n'"]);
        // The ftgo GraphQL gateway's form.
        let cjs = "const fetch = require(\"node-fetch\");\nclass P { find(id) { return fetch(`${this.url}/orders/${id}`); } }\n";
        assert_eq!(
            calls(Language::JavaScript, cjs),
            ["fetch GET '{this.url}/orders/{id}'"]
        );
        // A local named like the import shadows it.
        let local =
            "import nf from \"node-fetch\";\nfunction h() { const nf = mk(); nf(\"/no\"); }\n";
        assert!(calls(Language::TypeScript, local).is_empty());
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q1): a base stated in the call's request
    /// config replaces the instance's; a config that cannot be read, or a `baseURL` that is not
    /// a string or a name, leaves the base unknown; the instance's base applies only when the
    /// call has no config or its config has no `baseURL` (axios merges the request config over
    /// the instance defaults).
    #[test]
    fn axios_request_configs_state_the_base() {
        let src = "import axios from \"axios\";\nimport { api as shared } from \"./http\";\nconst api = axios.create({ baseURL: \"http://orders\" });\nexport async function f(body, cfg) {\n  await api.get(\"/orders/1\", { baseURL: \"https://api.stripe.com\" });\n  await axios.get(\"/orders/1\", { baseURL: \"https://api.stripe.com\" });\n  await axios.post(\"/charges\", body, { baseURL: \"https://api.stripe.com\" });\n  await api.post(\"/charges\", body);\n  await api.get(\"/a\", { params: { q: 1 } });\n  await api.get(\"/b\", cfg);\n  await api.get(\"/c\", { ...cfg });\n  await api.get(\"/d\", { ...cfg, baseURL: \"https://x.io\" });\n  await api.get(\"/e\", { baseURL: mk() });\n  await api.get(\"/f\", { baseURL: B });\n  await axios.post(\"/h\", cfg);\n  await axios.put(\"/i\", body, cfg);\n  await axios.request(cfg);\n  await axios(\"/j\", cfg);\n  await axios(\"/k\", { method: \"delete\", baseURL: \"https://api.stripe.com\" });\n  await shared.get(\"/l\", { baseURL: \"https://api.stripe.com\" });\n  await shared.get(\"/m\");\n  await api.patch(\"/n\", { baseURL: \"https://api.stripe.com\" });\n  await api.patch(\"/o\", body, { baseURL: \"https://api.stripe.com\" });\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "axios GET '/orders/1' base='https://api.stripe.com'",
                "axios GET '/orders/1' base='https://api.stripe.com'",
                "axios POST '/charges' base='https://api.stripe.com'",
                "axios POST '/charges' base='http://orders'",
                "axios GET '/a' base='http://orders'",
                "axios GET '/b' base=?",
                "axios GET '/c' base=?",
                "axios GET '/d' base='https://x.io'",
                "axios GET '/e' base=?",
                "axios GET '/f' base=B",
                "axios POST '/h'",
                "axios PUT '/i' base=?",
                "axios ? ? base=?",
                "axios ? '/j' base=?",
                "axios DELETE '/k' base='https://api.stripe.com'",
                "axios GET '/l' base='https://api.stripe.com' instance=shared",
                "axios GET '/m' instance=shared",
                // `patch(url, data, config)`: the second argument is data (evaluator pass 3, R7).
                "axios PATCH '/n' base='http://orders'",
                "axios PATCH '/o' base='https://api.stripe.com'",
            ]
        );
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q1): a Spring client's base is read from
    /// what configures the instance in the file: the call's own chain, the field or variable
    /// initialiser, an assignment (a constructor), a `setUriTemplateHandler` call, or, for an
    /// injected instance, the file's `@Bean` methods building one (marked, never `synthetic`).
    /// Several different bases, or one that cannot be read, leave it unknown.
    #[test]
    fn spring_client_bases_visible_on_the_instance() {
        let src = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.reactive.function.client.WebClient;\nimport org.springframework.web.util.DefaultUriBuilderFactory;\nclass C {\n  private final RestTemplate rootRt = new RestTemplateBuilder().rootUri(\"https://api.stripe.com\").build();\n  private final RestTemplate ctorRt;\n  private RestTemplate handlerRt = new RestTemplate();\n  private final RestTemplate injected;\n  private final RestTemplate built;\n  private final WebClient ctorWeb;\n  private WebClient twoWeb;\n  C(RestTemplateBuilder b, RestTemplate injected, WebClient.Builder wb) {\n    this.ctorRt = b.rootUri(\"http://orders\").build();\n    handlerRt.setUriTemplateHandler(new DefaultUriBuilderFactory(\"https://api.github.com\"));\n    this.injected = injected;\n    this.built = b.build();\n    this.ctorWeb = wb.baseUrl(\"https://api.stripe.com\").build();\n    this.twoWeb = wb.baseUrl(\"http://a\").build();\n  }\n  void other(WebClient.Builder wb) { twoWeb = wb.baseUrl(\"http://b\").build(); }\n  void m() {\n    rootRt.postForObject(\"/charges\", null, String.class);\n    ctorRt.getForObject(\"/orders/1\", String.class);\n    handlerRt.getForObject(\"/repos\", String.class);\n    injected.getForObject(\"/x\", String.class);\n    built.getForObject(\"/y\", String.class);\n    ctorWeb.get().uri(\"/v1/charges\").retrieve();\n    twoWeb.get().uri(\"/z\").retrieve();\n    ctorWeb.mutate().baseUrl(\"http://users\").build().get().uri(\"/u\").retrieve();\n    RestTemplate local = new RestTemplateBuilder().uriTemplateHandler(new DefaultUriBuilderFactory(\"http://visits\")).build();\n    local.getForObject(\"/v\", String.class);\n  }\n}\n";
        assert_eq!(
            calls(Language::Java, src),
            [
                "rest_template POST '/charges' base='https://api.stripe.com'",
                "rest_template GET '/orders/1' base='http://orders'",
                // A setter call is no request: the base is read, unverified (amendment 13).
                "rest_template GET '/repos' base='https://api.github.com' unverified",
                "rest_template GET '/x' base=?",
                "rest_template GET '/y' base=?",
                "rest_template GET '/v' base='http://visits'",
                // Built from `wb`, which builds `twoWeb` too (amendment 13, rule 3).
                "web_client GET '/v1/charges' base='https://api.stripe.com' unverified",
                "web_client GET '/z' base=?",
                "web_client GET '/u' base='http://users'",
            ]
        );
        // An injected instance: the file's `@Bean` method that builds one states its base.
        let bean = "package p;\nimport org.springframework.context.annotation.Bean;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nclass Cfg {\n  @Bean RestTemplate stripe(RestTemplateBuilder b) { return b.rootUri(\"https://api.stripe.com\").build(); }\n}\nclass Pay {\n  private final RestTemplate rt;\n  Pay(RestTemplate rt) { this.rt = rt; }\n  void charge() { rt.postForObject(\"/charges\", null, String.class); }\n}\n";
        assert_eq!(
            calls(Language::Java, bean),
            // A bean's base is never provably the injected one's: unverified (amendment 13).
            ["rest_template POST '/charges' base='https://api.stripe.com' from_bean unverified"]
        );
        // Two beans with different bases: unknown.
        let two = bean.replace(
            "  @Bean RestTemplate stripe",
            "  @Bean RestTemplate gh(RestTemplateBuilder b) { return b.rootUri(\"https://api.github.com\").build(); }\n  @Bean RestTemplate stripe",
        );
        assert_eq!(
            calls(Language::Java, &two),
            ["rest_template POST '/charges' base=?"]
        );
        // Without the `@Bean` import it is just a method.
        let plain = bean.replace("import org.springframework.context.annotation.Bean;\n", "");
        assert_eq!(
            calls(Language::Java, &plain),
            ["rest_template POST '/charges' base=?"]
        );
    }

    /// Contract M4a amendment 13, Gate A (evaluator pass 3, R1): an instance's base is
    /// verified only when nothing in the file can change it after its construction: a
    /// constant (or never reassigned) binding used only as the receiver of request calls. A
    /// `defaults` write, an interceptor, a reassignment, a spread after `baseURL` in the
    /// `create` config, or passing the instance on leaves the base in place but unverified.
    #[test]
    fn instance_bases_are_verified_only_when_unchanged() {
        let src = "import axios from \"axios\";\nconst api = axios.create({ baseURL: \"http://orders\" });\nconst api1 = axios.create({ baseURL: \"http://orders\" });\napi1.defaults.baseURL = \"https://api.stripe.com\";\nconst api2 = axios.create({ baseURL: \"http://orders\" });\napi2.interceptors.request.use((c) => { c.baseURL = \"https://api.stripe.com\"; return c; });\nlet api3 = axios.create({ baseURL: \"http://orders\" });\nexport function t03setup() { api3 = axios.create({ baseURL: \"https://api.stripe.com\" }); }\nconst stripeOpts = { baseURL: \"https://api.stripe.com\" };\nconst api4 = axios.create({ baseURL: \"http://orders\", ...stripeOpts });\nconst api5 = axios.create({ ...stripeOpts, baseURL: \"http://orders\" });\nconst api6 = axios.create({ baseURL: \"http://orders\" });\nconfigure(api6);\nlet api7 = axios.create({ baseURL: \"http://orders\" });\nexport async function f() {\n  await api.get(\"/orders/1\");\n  await api1.get(\"/orders/1\");\n  await api2.get(\"/orders/1\");\n  await api3.get(\"/orders/1\");\n  await api4.get(\"/orders/1\");\n  await api5.get(\"/orders/1\");\n  await api6.request({ url: \"/orders/1\" });\n  await api7.post(\"/orders/1\", {});\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "axios GET '/orders/1' base='http://orders'",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders'",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios POST '/orders/1' base='http://orders'",
            ]
        );
        let facts = extract(Language::TypeScript, src.as_bytes()).unwrap();
        let verified: Vec<(String, bool)> = clients(Language::TypeScript, &facts)
            .axios_instances
            .iter()
            .map(|a| (a.name.clone(), a.verified))
            .collect();
        assert_eq!(
            verified,
            [
                ("api".to_owned(), true),
                ("api1".to_owned(), false),
                ("api2".to_owned(), false),
                ("api3".to_owned(), false),
                ("api4".to_owned(), false),
                ("api5".to_owned(), true),
                ("api6".to_owned(), false),
                ("api7".to_owned(), true),
            ]
        );
        // An importing file reports the imported names it uses other than for requests, so the
        // indexer can unverify the exporting module's instance.
        let importer = "import { shared, shared2, shared3 } from \"./client\";\nimport * as ns from \"./ns\";\nshared.get(\"/a\");\nshared2.defaults.baseURL = \"https://api.stripe.com\";\nshared3.interceptors.request.use(f);\nns.api.get(\"/b\");\n";
        let facts = extract(Language::TypeScript, importer.as_bytes()).unwrap();
        assert_eq!(
            clients(Language::TypeScript, &facts).escaped_imports,
            ["ns", "shared2", "shared3"]
        );
    }

    /// Gate A for Spring (evaluator pass 3, R1): a client or builder field changed by a later
    /// statement, passed on, or reassigned, and an instance built from a mutable builder used
    /// outside that one chain, keep their base unverified.
    #[test]
    fn spring_bases_are_verified_only_when_unchanged() {
        let src = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass Clients {\n  private final WebClient.Builder b1 = WebClient.builder().baseUrl(\"http://orders\");\n  private final WebClient.Builder b2 = WebClient.builder().baseUrl(\"http://orders\");\n  private final WebClient w3 = WebClient.create(\"http://orders\");\n  private final RestTemplate rt4 = new RestTemplateBuilder().rootUri(\"http://orders\").build();\n  private final RestTemplate rt5 = new RestTemplateBuilder().rootUri(\"http://orders\").build();\n  private RestTemplate rt6 = new RestTemplateBuilder().rootUri(\"http://orders\").build();\n  private final WebClient w7;\n  private final WebClient w8;\n  Clients(WebClient.Builder wb, WebClient.Builder wb8) {\n    b1.baseUrl(\"https://api.stripe.com\");\n    this.w7 = wb.baseUrl(\"http://orders\").build();\n    wb.defaultHeader(\"x\", \"y\");\n    this.w8 = wb8.baseUrl(\"http://orders\").build();\n    register(rt5);\n    rt6 = null;\n  }\n  void m() {\n    b1.build().get().uri(\"/orders/1\").retrieve();\n    b2.build().get().uri(\"/orders/1\").retrieve();\n    w3.get().uri(\"/orders/1\").retrieve();\n    rt4.getForObject(\"/orders/1\", String.class);\n    rt5.getForObject(\"/orders/1\", String.class);\n    rt6.getForObject(\"/orders/1\", String.class);\n    w7.get().uri(\"/orders/1\").retrieve();\n    w8.get().uri(\"/orders/1\").retrieve();\n    w3.mutate().baseUrl(\"http://users\").build().get().uri(\"/u\").retrieve();\n  }\n}\n";
        assert_eq!(
            calls(Language::Java, src),
            [
                "rest_template GET '/orders/1' base='http://orders'",
                "rest_template GET '/orders/1' base='http://orders' unverified",
                "rest_template GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders'",
                "web_client GET '/orders/1' base='http://orders'",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders'",
                "web_client GET '/u' base='http://users'",
            ]
        );
        // A fresh initialiser states no base, so it agrees with the one a constructor
        // assigns (evaluator pass 3, R7, mutant G); the reassignment leaves it unverified.
        let fresh = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nclass C {\n  private RestTemplate rt = new RestTemplateBuilder().build();\n  C(RestTemplateBuilder b) { this.rt = b.rootUri(\"http://orders\").build(); }\n  void m() { rt.getForObject(\"/orders/1\", String.class); }\n}\n";
        assert_eq!(
            calls(Language::Java, fresh),
            ["rest_template GET '/orders/1' base='http://orders' unverified"]
        );
    }

    /// Evaluator pass 3, R8: an inline axios instance (no binding, so nothing can change it),
    /// an inline `new RestTemplate()`, and `cross-fetch` are client sites.
    #[test]
    fn inline_instances_and_cross_fetch() {
        let ts = "import axios from \"axios\";\nexport async function f() {\n  await axios.create({ baseURL: \"http://orders\" }).get(\"/orders/1\");\n  await axios.create({ baseURL: \"http://orders\", ...o }).post(\"/orders\", {});\n  await axios.create().request({ url: \"/r\" });\n}\n";
        assert_eq!(
            calls(Language::TypeScript, ts),
            [
                "axios GET '/orders/1' base='http://orders'",
                "axios POST '/orders' base='http://orders' unverified",
                "axios GET '/r'",
            ]
        );
        let cross = "import fetch from \"cross-fetch\";\nimport { fetch as cf } from \"cross-fetch\";\nfetch(\"/a\");\ncf(\"/b\", { method: \"PUT\" });\n";
        assert_eq!(
            calls(Language::TypeScript, cross),
            ["fetch GET '/a'", "fetch PUT '/b'"]
        );
        let cjs = "const fetch = require(\"cross-fetch\");\nfetch(\"/c\");\n";
        assert_eq!(calls(Language::JavaScript, cjs), ["fetch GET '/c'"]);
        let java = "package p;\nimport org.springframework.web.client.RestTemplate;\nclass C {\n  void m() {\n    new RestTemplate().getForObject(\"http://orders/orders/1\", String.class);\n    new Other().getForObject(\"/x\", String.class);\n  }\n}\n";
        assert_eq!(
            calls(Language::Java, java),
            ["rest_template GET 'http://orders/orders/1' base=?"]
        );
    }

    #[test]
    fn go_clients_and_sprintf() {
        let src = "package main\nimport (\n\t\"fmt\"\n\t\"net/http\"\n)\nconst base = \"http://notification:3000\"\nfunc notifyPaid(c *http.Client, id int) {\n\thttp.Post(\"http://notification:3000/internal/mfa/email\", \"application/json\", nil)\n\thttp.Get(fmt.Sprintf(\"%s/invoices/%d?x=%%\", base, id))\n\treq, _ := http.NewRequest(http.MethodPut, base+\"/x\", nil)\n\thttp.NewRequestWithContext(ctx, \"DELETE\", \"/y\", nil)\n\tc.Head(\"/z\")\n\thttp.DefaultClient.PostForm(\"/f\", nil)\n\tother.Get(\"/no\")\n\t_ = req\n}\n";
        assert_eq!(
            calls(Language::Go, src),
            [
                "net/http POST 'http://notification:3000/internal/mfa/email'",
                "net/http GET '{base}/invoices/{id}?x=%'",
                "net/http PUT '{base}/x'",
                "net/http DELETE '/y'",
                "net/http HEAD '/z'",
                "net/http POST '/f'",
            ]
        );
        let no_import = src.replace("\t\"net/http\"\n", "");
        assert!(calls(Language::Go, &no_import).is_empty());
    }
}
