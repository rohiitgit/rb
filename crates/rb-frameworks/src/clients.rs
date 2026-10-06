//! HTTP-client packs (design §4.5, contract M4a S9–S11): `fetch` and axios (TS/JS),
//! `RestTemplate`, `WebClient`/`RestClient` and Feign (Java), and Go's `net/http`, from one
//! file's syntax facts. Each client call site gives a method (or unknown), a URL value and a
//! base reference; the indexer folds them ([`crate::fold`]) and stitches them to endpoints.
//!
//! Every client is recognised through an import (or a qualified name), never by a bare short
//! name (contract invariant 6).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use rb_syntax::{
    Arg, Binding, CallChain, Decoration, DefKind, FileFacts, Init, Language, Span, StrTemplate,
    TemplatePart, TokenRole,
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
    /// Gate A (contract M4a amendments 13–15) on what carries the request: `Some(false)` when
    /// the carrier (the instance, builder or copy the request is sent through) or the request
    /// itself is not provably plain, whatever the URL's source, so the site never reaches
    /// `synthetic` (its host still restricts); otherwise `Some(true)` when the base comes from
    /// the carrier's construction and `None` when it does not. For an imported axios instance
    /// the indexer decides (it is exported, so never verified); for a [`Self::module_call`] the
    /// indexer caps it when the repository reconfigures that module ([`global_hooks`]).
    pub base_verified: Option<bool>,
    /// The call is on a client module itself: global `fetch` (or `node-fetch`/`cross-fetch`),
    /// the axios default export, Go's `net/http` package or `http.DefaultClient` (amendment
    /// 15, a bare module client).
    pub module_call: bool,
}

/// An axios instance bound to a name: `const api = axios.create({ baseURL })`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxiosInstance {
    pub name: String,
    pub base: Option<UrlValue>,
    pub top_level: bool,
    /// Gate A ([`Gate`]): a function local or an unexported module-level `const` whose every
    /// token is its declaration or the receiver of a request call, and whose `create` config
    /// is plain ([`plain_config`]): a provably plain carrier (amendment 15).
    pub verified: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileClients {
    pub calls: Vec<ClientCall>,
    pub axios_instances: Vec<AxiosInstance>,
}

/// Every client call site of one file, in source order per pack.
pub fn clients(language: Language, facts: &FileFacts) -> FileClients {
    let facts = &FileScope::new(facts);
    let mut out = FileClients::default();
    match language {
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            fetch(facts, &mut out);
            axios(facts, &mut out, &Gate::new(facts.facts));
        }
        Language::Java => {
            let bases = SpringBases::new(facts, Gate::new(facts.facts));
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
// Gate A: a conservative, scope-insensitive scan (contract M4a amendment 14)

/// Gate A over one file's [`rb_syntax::NameToken`]s: whether a binding provably still holds
/// what its declaration built. Name-based on purpose, so a shadowing name can only make a base
/// unverified, never verified.
struct Gate<'f> {
    facts: &'f FileFacts,
    /// Per name, its tokens in source order.
    tokens: HashMap<&'f str, Vec<&'f rb_syntax::NameToken>>,
    /// Per binding: `None` when a token is neither its one declaration nor a call receiver,
    /// else the calls made on it.
    scanned: RefCell<HashMap<usize, Option<Vec<&'f str>>>>,
}

impl<'f> Gate<'f> {
    fn new(facts: &'f FileFacts) -> Self {
        let mut tokens: HashMap<&str, Vec<&rb_syntax::NameToken>> = HashMap::new();
        for t in &facts.name_tokens {
            crate::work::tick(1);
            tokens.entry(t.name.as_str()).or_default().push(t);
        }
        Gate {
            facts,
            tokens,
            scanned: RefCell::new(HashMap::new()),
        }
    }

    /// Gate A for binding `bi` (contract M4a amendment 14): every token of its name in its
    /// whole scope (the enclosing function or method; for top-level code, which must be a
    /// `const`, the whole file), before its declaration and in nested functions too, is either
    /// its one declaration or the receiver of an `allowed` call. Only a TS/JS non-exported
    /// `const`/`let` declarator or a Java local variable declarator is a declaration
    /// ([`TokenRole::Declaration`]), so a field, a member, a parameter, a `var` or an exported
    /// binding is never verified; nor is a name declared twice in the scope.
    fn unchanged(&self, bi: usize, allowed: &[&str]) -> bool {
        let scanned = self
            .scanned
            .borrow_mut()
            .entry(bi)
            .or_insert_with(|| self.scan(bi))
            .clone();
        scanned.is_some_and(|calls| calls.iter().all(|c| allowed.contains(c)))
    }

    fn scan(&self, bi: usize) -> Option<Vec<&'f str>> {
        let b = &self.facts.bindings[bi];
        let range = match b.scope {
            Some(d) => Some(self.facts.definitions[d].span),
            None if b.constant => None,
            None => return None,
        };
        let all = self
            .tokens
            .get(b.name.as_str())
            .map_or(&[][..], Vec::as_slice);
        let start = |s: Span| (s[0], s[1]);
        let end = |s: Span| (s[2], s[3]);
        let from = range.map_or(0, |r| all.partition_point(|t| start(t.span) < start(r)));
        let mut declarations = 0;
        let mut calls = Vec::new();
        for t in &all[from..] {
            if range.is_some_and(|r| start(t.span) > end(r)) {
                break;
            }
            crate::work::tick(1);
            match &t.role {
                TokenRole::Declaration => declarations += 1,
                TokenRole::Receiver { call } => calls.push(call.as_str()),
                TokenRole::Member { .. } | TokenRole::Other => return None,
            }
        }
        (declarations == 1).then_some(calls)
    }
}

// ---------------------------------------------------------------------------------------------
// Global hooks: a bare module client is plain only while nothing reconfigures the module
// (contract M4a amendment 15)

/// The JavaScript global objects whose `fetch` member is the global `fetch`.
const GLOBAL_OBJECTS: &[&str] = &["globalThis", "window", "global", "self"];

/// The axios module's members that send a request or build an instance, and its pure helpers.
const AXIOS_MODULE_CALLS: &[&str] = &[
    "get",
    "post",
    "put",
    "delete",
    "patch",
    "head",
    "options",
    "request",
    "create",
    "isAxiosError",
    "isCancel",
];

/// A Go `http.Client`'s methods.
const GO_CLIENT_METHODS: &[&str] = &[
    "Do",
    "Get",
    "Head",
    "Post",
    "PostForm",
    "CloseIdleConnections",
];

/// The module clients this file may reconfigure for the whole process (contract M4a amendment
/// 15), from its name tokens, name-based and scope-insensitive like [`Gate`]:
/// - `fetch`: the `fetch` member of a global object written, read or passed on without being
///   called (`globalThis.fetch = …`, `window.fetch = …`, `const f = global.fetch`);
/// - axios: any token of the axios module's local name other than its import or `require`
///   declaration, a call of it (`axios(…)`), or a request, `create` or helper call on it, so
///   `axios.defaults…`, `axios.interceptors…`, aliasing it, exporting it or passing it on;
/// - Go `net/http`: `http.DefaultTransport` in any use, and `http.DefaultClient` in any use but
///   as the receiver of a client method (`http.DefaultClient = …`, `….Transport = …`, `c :=
///   http.DefaultClient`).
///
/// The indexer caps every [`ClientCall::module_call`] of a kind any file of the repository
/// reconfigures.
pub fn global_hooks(language: Language, facts: &FileFacts) -> Vec<ClientKind> {
    let mut out = Vec::new();
    let within = |inner: Span, outer: Span| {
        (outer[0], outer[1]) <= (inner[0], inner[1]) && (inner[2], inner[3]) <= (outer[2], outer[3])
    };
    match language {
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            if facts.name_tokens.iter().any(|t| {
                crate::work::tick(1);
                GLOBAL_OBJECTS.contains(&t.name.as_str())
                    && matches!(&t.role, TokenRole::Member { name } if name == "fetch")
            }) {
                out.push(ClientKind::Fetch);
            }
            let (names, requires) = module_names(facts, "axios", &["default"], true);
            let callees: HashSet<(i32, i32)> = facts
                .chains
                .iter()
                .filter(|c| c.root.is_none())
                .filter_map(|c| c.links.first())
                .map(|l| (l.span[0], l.span[1]))
                .collect();
            let declared = |t: &rb_syntax::NameToken| {
                facts.imports.iter().any(|i| within(t.span, i.span))
                    || requires
                        .iter()
                        .any(|&bi| within(t.span, facts.bindings[bi].span))
            };
            if facts.name_tokens.iter().any(|t| {
                crate::work::tick(1);
                names.contains(&t.name)
                    && !match &t.role {
                        TokenRole::Declaration => true,
                        TokenRole::Receiver { call } => AXIOS_MODULE_CALLS.contains(&call.as_str()),
                        TokenRole::Member { .. } => false,
                        TokenRole::Other => {
                            callees.contains(&(t.span[0], t.span[1])) || declared(t)
                        }
                    }
            }) {
                out.push(ClientKind::Axios);
            }
        }
        Language::Go => {
            let Some(http) = facts
                .imports
                .iter()
                .find(|i| i.module == "net/http")
                .and_then(|i| match i.alias.as_deref() {
                    Some("_" | ".") => None,
                    Some(a) => Some(a.to_owned()),
                    None => Some("http".to_owned()),
                })
            else {
                return out;
            };
            let default_client = format!("{http}.DefaultClient");
            let requests: HashSet<(i32, i32)> = facts
                .chains
                .iter()
                .filter(|c| {
                    c.root.as_deref() == Some(default_client.as_str())
                        && c.links
                            .first()
                            .is_some_and(|l| GO_CLIENT_METHODS.contains(&l.name.as_str()))
                })
                .map(|c| (c.span[0], c.span[1]))
                .collect();
            if facts.name_tokens.iter().any(|t| {
                crate::work::tick(1);
                t.name == http
                    && match &t.role {
                        TokenRole::Member { name } if name == "DefaultTransport" => true,
                        TokenRole::Member { name } if name == "DefaultClient" => {
                            !requests.contains(&(t.span[0], t.span[1]))
                        }
                        _ => false,
                    }
            }) {
                out.push(ClientKind::GoHttp);
            }
        }
        Language::Java | Language::Python => {}
    }
    out
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
            module_call: true,
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

/// The axios config keys that leave where a request goes to `baseURL` and `url` (Gate A,
/// contract M4a amendment 15). Anything else (`adapter`, `transformRequest`,
/// `transformResponse`, `httpAgent`, `httpsAgent`, `proxy`, `lookup`, `socketPath`,
/// `beforeRedirect`, …) may replace the transport or rewrite the request.
const PLAIN_AXIOS_KEYS: &[&str] = &[
    "baseURL",
    "url",
    "method",
    "headers",
    "params",
    "paramsSerializer",
    "data",
    "timeout",
    "timeoutErrorMessage",
    "withCredentials",
    "auth",
    "responseType",
    "responseEncoding",
    "xsrfCookieName",
    "xsrfHeaderName",
    "withXSRFToken",
    "onUploadProgress",
    "onDownloadProgress",
    "maxContentLength",
    "maxBodyLength",
    "maxRedirects",
    "validateStatus",
    "decompress",
    "signal",
    "cancelToken",
    "transitional",
];

/// An axios `create` or request config Gate A can certify (amendment 15): none, or an object
/// literal whose every key is in [`PLAIN_AXIOS_KEYS`] (a spread may hold anything).
fn plain_config(config: Option<&Arg>) -> bool {
    match config {
        None => true,
        Some(Arg::Object { entries, .. }) => entries
            .iter()
            .all(|e| PLAIN_AXIOS_KEYS.contains(&e.key.as_str())),
        Some(_) => false,
    }
}

/// Gate A's verdict on a call (amendment 15): `Some(false)` when its carrier or request is not
/// provably plain; else `Some(true)` when the base comes from the carrier's construction,
/// `None` when it does not.
fn verdict(plain: bool, base_from_carrier: bool) -> Option<bool> {
    if plain {
        base_from_carrier.then_some(true)
    } else {
        Some(false)
    }
}

fn axios(facts: &FileScope<'_>, out: &mut FileClients, gate: &Gate<'_>) {
    let (names, requires) = module_names(facts, "axios", &["default"], true);
    // `name` at `at` is the axios module: one of its local names, not shadowed by a parameter
    // or variable in effect there (evaluator pass 1, P6); the `require` binding is the module.
    let is_axios = |name: &str, at: Span| {
        names.iter().any(|n| n == name)
            && facts
                .binding_index_at(name, at)
                .is_none_or(|bi| requires.contains(&bi))
    };
    // Instances: `const api = axios.create({ baseURL })`, with Gate A (amendment 14).
    let mut instances: HashMap<usize, (Option<UrlValue>, bool)> = HashMap::new();
    for (bi, b) in facts.bindings.iter().enumerate().filter(|(_, b)| !b.member) {
        let Some(c) = init_chain(facts, b) else {
            continue;
        };
        if let (Some(root), [link]) = (c.root.as_deref(), c.links.as_slice())
            && is_axios(root, c.span)
            && link.name == "create"
        {
            let verified = plain_config(link.args.first()) && gate.unchanged(bi, AXIOS_REQUESTS);
            let base = create_base(link.args.first());
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
    for chain in &facts.chains {
        let Some(first) = chain.links.first() else {
            continue;
        };
        // `axios(url | config)`, `axios(url, config)`.
        if chain.root.is_none() {
            if is_axios(&first.name, chain.span) && !first.args.is_empty() {
                let mut call = match first.args.first() {
                    Some(config @ Arg::Object { .. }) => config_call(chain, None, None, config),
                    url => axios_call(chain, None, None, url, None, first.args.get(1)),
                };
                call.module_call = true;
                out.calls.push(call);
            }
            continue;
        }
        let root = chain.root.as_deref().unwrap_or_default();
        let bound = facts.binding_index_at(root, chain.span);
        // An inline instance, `axios.create(config).get(…)`: no binding, so nothing changes
        // it after its construction (evaluator pass 3, R8); a plain carrier when its config is
        // plain (amendment 15).
        if is_axios(root, chain.span)
            && first.name == "create"
            && let Some(link) = chain.links.get(1)
        {
            let plain = plain_config(first.args.first());
            let base = create_base(first.args.first());
            let from_carrier = base.is_some();
            if let Some(mut call) = axios_request(chain, link, base, None) {
                call.base_verified = verdict(plain && call.base_verified.is_none(), from_carrier);
                out.calls.push(call);
            }
            continue;
        }
        // The module itself (a bare module client), a local instance (its Gate A verdict), or
        // an imported name the indexer resolves.
        let module = is_axios(root, chain.span);
        let (base, instance, carrier) = if module {
            (None, None, None)
        } else if let Some((base, verified)) = bound.and_then(|bi| instances.get(&bi)) {
            (base.clone(), None, Some((*verified, base.is_some())))
        } else if bound.is_none() && imported.contains(root) {
            (None, Some(root.to_owned()), None)
        } else {
            continue;
        };
        if let Some(mut call) = axios_request(chain, first, base, instance) {
            if let Some((plain, from_carrier)) = carrier {
                call.base_verified = verdict(plain && call.base_verified.is_none(), from_carrier);
            }
            call.module_call = module;
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
        // A config that cannot be read: no URL, method or base, and not plain.
        _ => ClientCall {
            method: None,
            url: UrlValue::Unknown,
            base: Some(UrlValue::Unknown),
            base_verified: Some(false),
            ..axios_call(chain, base, instance, None, None, None)
        },
    })
}

/// The base an `axios.create(config)` states (its last `baseURL`). A spread anywhere in the
/// config makes the instance's carrier not plain ([`plain_config`]; spreads are never folded),
/// so a spread after `baseURL` that may replace it leaves the base unverified.
fn create_base(config: Option<&Arg>) -> Option<UrlValue> {
    let Some(Arg::Object { entries, .. }) = config else {
        return None;
    };
    entries
        .iter()
        .rev()
        .find(|e| e.key == "baseURL")
        .map(|e| UrlValue::of(Some(&e.value)))
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
        // A request config that is not plain (an `adapter`, a spread, a variable) may send the
        // request elsewhere whatever carries it (amendment 15).
        base_verified: (!plain_config(config)).then_some(false),
        module_call: false,
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
        // The carrier is the instance (Gate A), or an inline `new RestTemplate()`, plain.
        let base = bases.of(receiver, ClientKind::RestTemplate);
        let plain = receiver.is_none_or(|bi| bases.carrier(bi));
        out.calls.push(ClientCall {
            kind: ClientKind::RestTemplate,
            method,
            url: java_url(facts, first.args.first()),
            base_verified: verdict(plain, base.read),
            base: Some(base.value),
            instance: None,
            span: chain.span,
            source: chain.enclosing,
            base_from_bean: base.from_bean,
            module_call: false,
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

/// The base a chain's links before `upto` set. A `WebClient`/`RestClient` builder ignores
/// `baseUrl` once a `uriBuilderFactory` is set, and a `RestTemplateBuilder`'s `rootUri` wraps
/// whatever URI template handler it has, whatever the order (evaluator pass 4, T4); otherwise
/// the last base-setting link wins.
fn chain_base(facts: &FileScope<'_>, chain: &CallChain, upto: usize) -> BaseFact {
    let links = &chain.links[..upto];
    let last = |names: &[&str]| {
        links
            .iter()
            .rev()
            .find(|l| names.contains(&l.name.as_str()) && !l.args.is_empty())
    };
    let base = last(&["uriBuilderFactory"])
        .map(|l| factory_base(l.args.first()))
        .or_else(|| last(&["rootUri"]).map(|l| java_url(facts, l.args.first())))
        .or_else(|| {
            last(&[
                "baseUrl",
                "create",
                "uriTemplateHandler",
                "setUriTemplateHandler",
            ])
            .map(|l| match l.name.as_str() {
                "baseUrl" | "create" => java_url(facts, l.args.first()),
                _ => factory_base(l.args.first()),
            })
        });
    if let Some(value) = base {
        return BaseFact::Stated(value);
    }
    if fresh_root(facts, chain) {
        BaseFact::Fresh
    } else {
        BaseFact::Unreadable
    }
}

/// The links of a construction chain that leave its URI building to `baseUrl`/`rootUri` alone
/// (Gate A, contract M4a amendment 14, rule 4): the fresh root's `builder`/`create`, the base
/// itself, `build`, and settings no URI goes through. Anything else (`uriBuilderFactory`,
/// `uriTemplateHandler`, `defaultUriVariables`, filters, interceptors, customizers, `apply`, a
/// connector or request factory) may change where a request goes.
const PLAIN_LINKS: &[&str] = &[
    "builder",
    "create",
    "baseUrl",
    "rootUri",
    "build",
    "defaultHeader",
    "defaultHeaders",
    "defaultCookie",
    "defaultCookies",
    "codecs",
    "exchangeStrategies",
    "messageConverters",
    "additionalMessageConverters",
    "errorHandler",
    "defaultStatusHandler",
    "basicAuthentication",
    "setConnectTimeout",
    "setReadTimeout",
    "connectTimeout",
    "readTimeout",
];

/// A construction chain whose base Gate A can certify: it starts from a new client or builder
/// (not from a builder or client binding, whose state is not read) and its links before `upto`
/// are all [`PLAIN_LINKS`].
fn plain_construction(facts: &FileFacts, chain: &CallChain, upto: usize) -> bool {
    fresh_root(facts, chain) && plain_links(&chain.links[..upto])
}

/// Every link is one of [`PLAIN_LINKS`].
fn plain_links(links: &[rb_syntax::CallLink]) -> bool {
    links.iter().all(|l| PLAIN_LINKS.contains(&l.name.as_str()))
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
/// methods per client kind, for an instance injected from them. With the file's [`Gate`].
struct SpringBases<'s, 'f> {
    facts: &'s FileScope<'f>,
    /// Binding → indices of the chains that configure it.
    configured: HashMap<usize, Vec<usize>>,
    beans: HashMap<ClientKind, Vec<BaseFact>>,
    gate: Gate<'f>,
}

/// A Spring client's base as [`SpringBases::of`] reads it.
struct SpringBase {
    value: UrlValue,
    from_bean: bool,
    /// The base was read from the carrier's construction (not unknown, not a `@Bean`'s).
    read: bool,
}

/// The calls on a Spring client or builder binding that leave its configuration as built
/// (Gate A): requests. Not `mutate()`: the copy it returns is configured by links the scan of
/// the binding does not read (evaluator pass 5, U1).
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
        "get", "post", "put", "delete", "patch", "head", "options", "method",
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
    fn new(facts: &'s FileScope<'f>, gate: Gate<'f>) -> Self {
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
            gate,
        }
    }

    /// Whether the binding `bi` is a provably plain carrier (Gate A, contract M4a amendments
    /// 14 and 15): it passes the [`Gate`] scan, something in sight builds it, and every chain
    /// that configures it is a [`plain_construction`]. A field, a parameter, an injected
    /// instance and one configured by a non-plain link never are.
    fn carrier(&self, bi: usize) -> bool {
        let b = &self.facts.bindings[bi];
        // A binding with no initialiser has no `declaration` token or is written to, so the
        // scan below fails it.
        let init = match &b.init {
            Some(Init::Call { chain }) => Some(*chain),
            // `new RestTemplate()`: a new client with no links (an argument may be a request
            // factory, and a subclass may override anything, so neither is plain).
            Some(Init::New { type_name, args }) => {
                if *args > 0 || type_name.rsplit('.').next() != Some("RestTemplate") {
                    return false;
                }
                None
            }
            Some(_) => return false,
            None => None,
        };
        let chains: Vec<usize> = init
            .into_iter()
            .chain(self.configured.get(&bi).into_iter().flatten().copied())
            .collect();
        self.gate
            .unchanged(bi, spring_requests(self.facts, &b.type_name))
            && chains.iter().all(|&ci| {
                self.facts
                    .chains
                    .get(ci)
                    .is_some_and(|c| plain_construction(self.facts, c, c.links.len()))
            })
    }

    /// The base of a `kind` client bound at `receiver`: one stated base when everything that
    /// configures the instance agrees, else unknown. An instance nothing in the file
    /// configures is injected: the file's `@Bean` methods for that kind decide, the same way
    /// (such an instance is never a plain carrier, so its base is never verified).
    fn of(&self, receiver: Option<usize>, kind: ClientKind) -> SpringBase {
        let unknown = SpringBase {
            value: UrlValue::Unknown,
            from_bean: false,
            read: false,
        };
        let Some(bi) = receiver else {
            return unknown;
        };
        let b = &self.facts.bindings[bi];
        let mut facts: Vec<BaseFact> = Vec::new();
        match &b.init {
            Some(Init::Call { chain }) => {
                if let Some(c) = self.facts.chains.get(*chain) {
                    facts.push(chain_base(self.facts, c, c.links.len()));
                }
            }
            // A constructor argument (a request factory, converters) sets no root URI.
            Some(Init::New { .. }) => facts.push(BaseFact::Fresh),
            Some(_) => facts.push(BaseFact::Unreadable),
            None => {}
        }
        for &ci in self.configured.get(&bi).into_iter().flatten() {
            let c = &self.facts.chains[ci];
            facts.push(chain_base(self.facts, c, c.links.len()));
        }
        if facts.is_empty() {
            let beans = self.beans.get(&kind).map_or(&[][..], Vec::as_slice);
            return match agreed(beans) {
                Some(value) => SpringBase {
                    value,
                    from_bean: true,
                    read: false,
                },
                None => unknown,
            };
        }
        let Some(value) = agreed(&facts) else {
            return unknown;
        };
        SpringBase {
            value,
            from_bean: false,
            read: true,
        }
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
    for chain in &facts.chains {
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
        // The carrier (Gate A, amendments 14 and 15): a chain that builds its own client is
        // plain when it is a [`plain_construction`]; a request on a binding is plain when the
        // binding is a plain carrier and the request chain's own links before the verb are
        // plain (`x.mutate().filter(…)…` is not: evaluator pass 5, U1).
        let (base, plain) = match chain_base(facts, chain, verb) {
            BaseFact::Stated(value) => (
                SpringBase {
                    value,
                    from_bean: false,
                    read: true,
                },
                plain_construction(facts, chain, verb),
            ),
            BaseFact::Fresh => (
                SpringBase {
                    value: UrlValue::Unknown,
                    from_bean: false,
                    read: false,
                },
                plain_construction(facts, chain, verb),
            ),
            BaseFact::Unreadable => (
                bases.of(receiver, kind),
                receiver.is_some_and(|bi| bases.carrier(bi)) && plain_links(&chain.links[..verb]),
            ),
        };
        out.calls.push(ClientCall {
            kind,
            method,
            url,
            base_verified: verdict(plain, base.read && base.value != UrlValue::Unknown),
            base: Some(base.value),
            instance: None,
            span: chain.span,
            source: chain.enclosing,
            base_from_bean: base.from_bean,
            module_call: false,
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
                    // A declarative interface: no instance carries the request (amendment 15).
                    base_verified: None,
                    module_call: false,
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
        let on_default = root == format!("{http}.DefaultClient");
        let on_client = !on_package
            && (on_default
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
        let bare = on_default || on_package && !link.name.starts_with("NewRequest");
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
            // The package's functions and `http.DefaultClient` are the bare module client
            // (amendment 15); a `http.Client` binding is not provably plain (its `Transport`
            // may reroute), nor is a request built by `NewRequest`, which whatever `Do`es it
            // carries.
            base_verified: (!bare).then_some(false),
            module_call: bare,
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
        let src = "import http from \"axios\";\nimport { client as shared } from \"./http\";\nconst API = \"http://gateway\";\nconst api = http.create({ baseURL: API });\nexport async function f(creds) {\n  await http.post(`${API}/api/v1/login`, creds);\n  await http(\"/plain\");\n  await http({ url: \"/cfg\", method: \"put\" });\n  await http.request({ url: \"/req\" });\n  await api.get(\"/inst\");\n  await shared.delete(\"/imported\");\n  await other.get(\"/not-axios\");\n}\n";
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
                // A field is never a plain carrier, whatever the URL's source (amendments 14
                // and 15).
                "rest_template GET 'http://visits/owners/{id}' base=? unverified",
                "rest_template PUT '{BASE}/x' base=? unverified",
                "rest_template ? '/y' base=? unverified",
                "web_client GET '/owners/{ownerId}' base='http://customers-service' unverified",
                "web_client POST 'http://visits-service/pets/visits' base=? unverified",
                "rest_client DELETE '/r' base=? unverified",
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
                "rest_template GET 'http://localhost:{port}/x/{id}' base=? unverified",
                "web_client GET '/owners' base='http://customers' unverified",
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
        let src = "import axios from \"axios\";\nimport { client as shared } from \"./http\";\nconst api = axios.create({ baseURL: \"http://orders\" });\nexport async function f(body, cfg) {\n  await api.get(\"/orders/1\", { baseURL: \"https://api.stripe.com\" });\n  await axios.get(\"/orders/1\", { baseURL: \"https://api.stripe.com\" });\n  await axios.post(\"/charges\", body, { baseURL: \"https://api.stripe.com\" });\n  await api.post(\"/charges\", body);\n  await api.get(\"/a\", { params: { q: 1 } });\n  await api.get(\"/b\", cfg);\n  await api.get(\"/c\", { ...cfg });\n  await api.get(\"/d\", { ...cfg, baseURL: \"https://x.io\" });\n  await api.get(\"/e\", { baseURL: mk() });\n  await api.get(\"/f\", { baseURL: B });\n  await axios.post(\"/h\", cfg);\n  await axios.put(\"/i\", body, cfg);\n  await axios.request(cfg);\n  await axios(\"/j\", cfg);\n  await axios(\"/k\", { method: \"delete\", baseURL: \"https://api.stripe.com\" });\n  await shared.get(\"/l\", { baseURL: \"https://api.stripe.com\" });\n  await shared.get(\"/m\");\n  await api.patch(\"/n\", { baseURL: \"https://api.stripe.com\" });\n  await api.patch(\"/o\", body, { baseURL: \"https://api.stripe.com\" });\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "axios GET '/orders/1' base='https://api.stripe.com'",
                "axios GET '/orders/1' base='https://api.stripe.com'",
                "axios POST '/charges' base='https://api.stripe.com'",
                "axios POST '/charges' base='http://orders'",
                "axios GET '/a' base='http://orders'",
                // A request config that is not a plain object literal (a variable, a spread)
                // may hold an `adapter`: not plain, whatever the base (amendment 15).
                "axios GET '/b' base=? unverified",
                "axios GET '/c' base=? unverified",
                "axios GET '/d' base='https://x.io' unverified",
                "axios GET '/e' base=?",
                "axios GET '/f' base=B",
                "axios POST '/h'",
                "axios PUT '/i' base=? unverified",
                "axios ? ? base=? unverified",
                "axios ? '/j' base=? unverified",
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
            // Fields are never verified, nor plain carriers whatever their base (amendment
            // 15); the local `local` is configured by a URI template handler, and
            // `ctorWeb.mutate()…` starts from a binding (amendment 14).
            [
                "rest_template POST '/charges' base='https://api.stripe.com' unverified",
                "rest_template GET '/orders/1' base='http://orders' unverified",
                "rest_template GET '/repos' base='https://api.github.com' unverified",
                "rest_template GET '/x' base=? unverified",
                "rest_template GET '/y' base=? unverified",
                "rest_template GET '/v' base='http://visits' unverified",
                "web_client GET '/v1/charges' base='https://api.stripe.com' unverified",
                "web_client GET '/z' base=? unverified",
                "web_client GET '/u' base='http://users' unverified",
            ]
        );
        // An injected instance: the file's `@Bean` method that builds one states its base.
        let bean = "package p;\nimport org.springframework.context.annotation.Bean;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nclass Cfg {\n  @Bean RestTemplate stripe(RestTemplateBuilder b) { return b.rootUri(\"https://api.stripe.com\").build(); }\n}\nclass Pay {\n  private final RestTemplate rt;\n  Pay(RestTemplate rt) { this.rt = rt; }\n  void charge() { rt.postForObject(\"/charges\", null, String.class); }\n}\n";
        assert_eq!(
            calls(Language::Java, bean),
            // A bean's base is never provably the injected one's: unverified (amendment 13).
            ["rest_template POST '/charges' base='https://api.stripe.com' from_bean unverified"]
        );
        // Two beans with different bases: unknown. An injected field is never a plain
        // carrier (amendment 15).
        let two = bean.replace(
            "  @Bean RestTemplate stripe",
            "  @Bean RestTemplate gh(RestTemplateBuilder b) { return b.rootUri(\"https://api.github.com\").build(); }\n  @Bean RestTemplate stripe",
        );
        assert_eq!(
            calls(Language::Java, &two),
            ["rest_template POST '/charges' base=? unverified"]
        );
        // Without the `@Bean` import it is just a method.
        let plain = bean.replace("import org.springframework.context.annotation.Bean;\n", "");
        assert_eq!(
            calls(Language::Java, &plain),
            ["rest_template POST '/charges' base=? unverified"]
        );
    }

    /// Contract M4a amendment 14, Gate A (evaluator passes 3 and 4): an axios instance's base
    /// is verified only for a function local or an unexported module-level `const` whose every
    /// token (before its declaration and in nested functions too) is its declaration or the
    /// receiver of a request call. A `defaults` write, an interceptor, a reassignment, a module
    /// `let`, a spread after `baseURL`, passing it on, aliasing it, exporting it in any form, or
    /// a second declaration of the name leaves the base in place but unverified.
    #[test]
    fn instance_bases_are_verified_only_when_unchanged() {
        let src = "import axios from \"axios\";\nexport function a01setup(url) { a01.defaults.baseURL = url; }\nexport function a02reset() { a02 = axios.create({ baseURL: \"https://api.stripe.com\" }); }\nclass Cfg { static init() { a10.defaults.baseURL = \"https://api.stripe.com\"; } }\nconst api = axios.create({ baseURL: \"http://orders\" });\nconst a01 = axios.create({ baseURL: \"http://orders\" });\nlet a02 = axios.create({ baseURL: \"http://orders\" });\nconst a10 = axios.create({ baseURL: \"http://orders\" });\nconst api1 = axios.create({ baseURL: \"http://orders\" });\napi1.defaults.baseURL = \"https://api.stripe.com\";\nconst api2 = axios.create({ baseURL: \"http://orders\" });\napi2.interceptors.request.use((c) => { c.baseURL = \"https://api.stripe.com\"; return c; });\nconst stripeOpts = { baseURL: \"https://api.stripe.com\" };\nconst api4 = axios.create({ baseURL: \"http://orders\", ...stripeOpts });\nconst api5 = axios.create({ ...stripeOpts, baseURL: \"http://orders\" });\nconst api6 = axios.create({ baseURL: \"http://orders\" });\nconfigure(api6);\nlet api7 = axios.create({ baseURL: \"http://orders\" });\nconst a04 = axios.create({ baseURL: \"http://orders\" });\nexport { a04 as ordersApi };\nconst a05 = axios.create({ baseURL: \"http://orders\" });\nexport { a05 as default };\nexport const ex = axios.create({ baseURL: \"http://orders\" });\nconst cj = axios.create({ baseURL: \"http://orders\" });\nmodule.exports = { cj };\nconst al = axios.create({ baseURL: \"http://orders\" });\nconst al2 = al;\nconst op = axios.create({ baseURL: \"http://orders\" });\nop!.defaults.baseURL = \"https://api.stripe.com\";\nfunction shadow() { const op = 1; return op; }\nexport async function f() {\n  await api.get(\"/orders/1\");\n  await a01.get(\"/orders/1\");\n  await a02.get(\"/orders/1\");\n  await a10.get(\"/orders/1\");\n  await api1.get(\"/orders/1\");\n  await api2.get(\"/orders/1\");\n  await api4.get(\"/orders/1\");\n  await api5.get(\"/orders/1\");\n  await api6.request({ url: \"/orders/1\" });\n  await api7.post(\"/orders/1\", {});\n  await a04.get(\"/orders/1\");\n  await a05.get(\"/orders/1\");\n  await ex.get(\"/orders/1\");\n  await cj.get(\"/orders/1\");\n  await al.get(\"/orders/1\");\n}\nexport function a06() {\n  const setup = () => { a06api.defaults.baseURL = \"https://api.stripe.com\"; };\n  const a06api = axios.create({ baseURL: \"http://orders\" });\n  setup();\n  return a06api.get(\"/orders/1\");\n}\nexport function local() {\n  const c = axios.create({ baseURL: \"http://orders\" });\n  return c.get(\"/orders/1\");\n}\nexport function nested() {\n  const n = axios.create({ baseURL: \"http://orders\" });\n  const later = () => n.get(\"/orders/2\");\n  { const n = 2; }\n  return n.get(\"/orders/1\");\n}\nexport const arrow = async () => {\n  const ar = axios.create({ baseURL: \"http://orders\" });\n  return ar.get(\"/orders/1\");\n};\nclass Svc {\n  m() {\n    const sv = axios.create({ baseURL: \"http://orders\" });\n    return sv.get(\"/orders/1\");\n  }\n}\n";
        let facts = extract(Language::TypeScript, src.as_bytes()).unwrap();
        let verified: Vec<String> = clients(Language::TypeScript, &facts)
            .axios_instances
            .iter()
            .map(|a| format!("{} {}", a.name, a.verified))
            .collect();
        assert_eq!(
            verified,
            [
                "api true",
                "a01 false",
                "a02 false",
                "a10 false",
                "api1 false",
                "api2 false",
                "api4 false",
                // A spread anywhere in the `create` config may hold an `adapter` (amendment 15).
                "api5 false",
                "api6 false",
                "api7 false",
                "a04 false",
                "a05 false",
                "ex false",
                "cj false",
                "al false",
                "op false",
                "a06api false",
                "c true",
                "n false",
                "ar true",
                "sv true",
            ]
        );
        // The calls on them: `api` and the locals `c`, `ar`, `sv` keep a verified base.
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "axios GET '/orders/1' base='http://orders'",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios POST '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders'",
                "axios GET '/orders/2' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders'",
                "axios GET '/orders/1' base='http://orders'",
            ]
        );
    }

    /// Contract M4a amendment 14, Gate A for Spring (evaluator passes 3 and 4): a field is
    /// never verified (another reference, a subclass, another file or a generated accessor may
    /// change it); a method local is verified when its every token is its declaration or the
    /// receiver of a request call (a builder: of `build`) and it was built by a plain chain from
    /// a new client or builder. The Store-Microservices and ftgo forms stay verified.
    #[test]
    fn spring_bases_are_verified_only_when_unchanged() {
        let src = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.reactive.function.client.WebClient;\nimport org.springframework.web.util.DefaultUriBuilderFactory;\nclass Clients {\n  private final RestTemplate rt1 = new RestTemplateBuilder().rootUri(\"http://orders\").build();\n  private final WebClient w0 = WebClient.create(\"http://orders\");\n  void k01() { rt1.getForObject(\"/orders/1\", String.class); }\n  void k02() { w0.get().uri(\"/orders/1\").retrieve(); }\n  void store(String host) {\n    WebClient webClient = WebClient.builder().baseUrl(\"http://\" + host + \":8080\").build();\n    webClient.get().uri(\"/orders/1\").retrieve();\n  }\n  void ftgo(int port) {\n    WebClient client = WebClient.create(\"http://localhost:\" + port + \"/orders\");\n    client.get().retrieve();\n  }\n  void l1() {\n    RestTemplate rt = new RestTemplateBuilder().rootUri(\"http://orders\").build();\n    rt.getInterceptors().add(stripe);\n    rt.getForObject(\"/orders/1\", String.class);\n  }\n  void l2() {\n    WebClient w = WebClient.create(\"http://orders\");\n    Runnable r = () -> tweak(w);\n    w.get().uri(\"/orders/1\").retrieve();\n  }\n  void l3() {\n    WebClient w = WebClient.create(\"http://orders\");\n    w = WebClient.create(\"http://orders\");\n    w.get().uri(\"/orders/1\").retrieve();\n  }\n  void l4(WebClient.Builder wb) {\n    WebClient w = wb.baseUrl(\"http://orders\").build();\n    w.get().uri(\"/orders/1\").retrieve();\n  }\n  void l5() {\n    WebClient.Builder b = WebClient.builder().baseUrl(\"http://orders\");\n    b.build().get().uri(\"/orders/1\").retrieve();\n  }\n  void l6() {\n    WebClient.Builder b = WebClient.builder().baseUrl(\"http://orders\");\n    b.defaultHeader(\"x\", \"y\");\n    b.build().get().uri(\"/orders/1\").retrieve();\n  }\n  void l7() {\n    WebClient w = WebClient.create(\"http://orders\");\n    java.util.function.Supplier<?> s = w::mutate;\n    w.get().uri(\"/orders/1\").retrieve();\n  }\n  void l8() {\n    WebClient.create(\"http://orders\").get().uri(\"/orders/1\").retrieve();\n    w0.mutate().baseUrl(\"http://users\").build().get().uri(\"/u\").retrieve();\n  }\n}\nclass Tweak { static void tweak(Clients c) { c.rt1.setUriTemplateHandler(new DefaultUriBuilderFactory(\"https://api.stripe.com\")); } }\n";
        assert_eq!(
            calls(Language::Java, src),
            [
                "rest_template GET '/orders/1' base='http://orders' unverified",
                "rest_template GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://{host}:8080'",
                "web_client GET '' base='http://localhost:{port}/orders'",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders'",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders'",
                "web_client GET '/u' base='http://users' unverified",
            ]
        );
        // A fresh initialiser states no base, so it agrees with the one a constructor
        // assigns (evaluator pass 3, R7, mutant G); a field stays unverified.
        let fresh = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nclass C {\n  private RestTemplate rt = new RestTemplateBuilder().build();\n  C(RestTemplateBuilder b) { this.rt = b.rootUri(\"http://orders\").build(); }\n  void m() { rt.getForObject(\"/orders/1\", String.class); }\n}\n";
        assert_eq!(
            calls(Language::Java, fresh),
            ["rest_template GET '/orders/1' base='http://orders' unverified"]
        );
    }

    /// Evaluator pass 4, T4: a `WebClient`/`RestClient` builder ignores `baseUrl` once a
    /// `uriBuilderFactory` is set, whatever the order, and a `RestTemplateBuilder`'s `rootUri`
    /// wins over its handler; any link that may change URI building beyond `baseUrl`/`rootUri`
    /// leaves the base unverified (contract M4a amendment 14, rule 4).
    #[test]
    fn uri_builder_factories_override_the_base_and_unverify_it() {
        let src = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.web.client.RestClient;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.reactive.function.client.WebClient;\nimport org.springframework.web.util.DefaultUriBuilderFactory;\nclass C {\n  void m() {\n    WebClient w1 = WebClient.builder().uriBuilderFactory(new DefaultUriBuilderFactory(\"https://api.stripe.com\")).baseUrl(\"http://orders\").build();\n    WebClient w2 = WebClient.builder().baseUrl(\"http://orders\").uriBuilderFactory(new DefaultUriBuilderFactory(\"https://api.stripe.com\")).build();\n    RestClient r1 = RestClient.builder().uriBuilderFactory(new DefaultUriBuilderFactory(\"https://api.stripe.com\")).baseUrl(\"http://orders\").build();\n    RestClient r2 = RestClient.builder().baseUrl(\"http://orders\").uriBuilderFactory(new DefaultUriBuilderFactory(\"https://api.stripe.com\")).build();\n    RestTemplate t1 = new RestTemplateBuilder().rootUri(\"http://orders\").uriTemplateHandler(new DefaultUriBuilderFactory(\"https://api.stripe.com\")).build();\n    WebClient w3 = WebClient.builder().baseUrl(\"http://orders\").defaultUriVariables(vars).build();\n    WebClient w4 = WebClient.builder().baseUrl(\"http://orders\").defaultHeader(\"a\", \"b\").build();\n    w1.get().uri(\"/orders/1\").retrieve();\n    w2.get().uri(\"/orders/1\").retrieve();\n    r1.get().uri(\"/orders/1\").retrieve();\n    r2.get().uri(\"/orders/1\").retrieve();\n    t1.getForObject(\"/orders/1\", String.class);\n    w3.get().uri(\"/orders/1\").retrieve();\n    w4.get().uri(\"/orders/1\").retrieve();\n    WebClient.builder().uriBuilderFactory(new DefaultUriBuilderFactory(\"https://api.stripe.com\")).baseUrl(\"http://orders\").build().get().uri(\"/orders/1\").retrieve();\n  }\n}\n";
        assert_eq!(
            calls(Language::Java, src),
            [
                "rest_template GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='https://api.stripe.com' unverified",
                "web_client GET '/orders/1' base='https://api.stripe.com' unverified",
                "rest_client GET '/orders/1' base='https://api.stripe.com' unverified",
                "rest_client GET '/orders/1' base='https://api.stripe.com' unverified",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders'",
                "web_client GET '/orders/1' base='https://api.stripe.com' unverified",
            ]
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

    /// Contract M4a amendment 15 (evaluator pass 5, U1–U3): a site may reach `synthetic` only
    /// when what carries the request is provably plain, whatever the URL's source (an absolute
    /// literal, a configured property, a base). Not plain: a request sent through `x.mutate()…`
    /// (U1); an instance with no base read but a filter or interceptor, a field, an injected
    /// instance (U2); an axios config with a transport key or a spread (U3). Plain: a verified
    /// local, an inline fresh construction with plain links, a bare module client, Feign.
    #[test]
    fn carriers_are_gated_whatever_the_url() {
        let java = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.web.client.RestClient;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass Q {\n  private final WebClient q13 = WebClient.builder().filter(REWRITE).build();\n  private final RestTemplate q14 = new RestTemplateBuilder().additionalInterceptors(i).build();\n  private final RestTemplate bk8 = new RestTemplate();\n  void q03() {\n    WebClient w = WebClient.create(\"http://orders\");\n    w.mutate().apply(b -> b.baseUrl(\"https://api.stripe.com\")).build().get().uri(\"/orders/1\").retrieve();\n  }\n  void q04() {\n    WebClient w = WebClient.create(\"http://orders\");\n    w.mutate().filter(REWRITE).build().get().uri(\"/orders/1\").retrieve();\n  }\n  void q05() {\n    RestClient r = RestClient.create(\"http://orders\");\n    r.mutate().requestInterceptor(i).build().get().uri(\"/orders/1\").retrieve();\n  }\n  void q11() {\n    WebClient w = WebClient.create(\"http://orders\");\n    w.mutate().build().get().uri(\"/orders/1\").retrieve();\n  }\n  void q09b() { WebClient.builder().filter(REWRITE).build().get().uri(\"http://orders/orders/1\").retrieve(); }\n  void q12() {\n    WebClient w = WebClient.builder().filter(REWRITE).build();\n    w.get().uri(\"http://orders/orders/1\").retrieve();\n  }\n  void q13() { q13.get().uri(\"http://orders/orders/1\").retrieve(); }\n  void q14() { q14.getForObject(\"http://orders/orders/1\", String.class); }\n  void bk8() { bk8.getForObject(usersUrl + \"/users/me\", String.class); }\n  void f1() {\n    RestTemplate rf = new RestTemplate(factory);\n    rf.getForObject(\"http://orders/orders/1\", String.class);\n  }\n  void m1() {\n    WebClient w = WebClient.create(\"http://orders\");\n    WebClient copy = w.mutate().build();\n    w.get().uri(\"/orders/1\").retrieve();\n  }\n  void l9() {\n    WebClient.Builder b = WebClient.builder().baseUrl(\"http://orders\");\n    b.build().mutate().filter(REWRITE).build().get().uri(\"/orders/1\").retrieve();\n  }\n  void sub() {\n    RestTemplate cr = new MyTemplate();\n    cr.getForObject(\"http://orders/orders/1\", String.class);\n    RestTemplate h = new RestTemplate();\n    h.setUriTemplateHandler(new DefaultUriBuilderFactory(\"http://visits\"));\n    h.getForObject(\"/v\", String.class);\n  }\n  void ok() {\n    new RestTemplate().getForObject(\"http://orders/orders/1\", String.class);\n    RestTemplate rt = new RestTemplate();\n    rt.getForObject(\"http://orders/orders/1\", String.class);\n    WebClient.create(\"http://orders\").get().uri(\"/orders/1\").retrieve();\n    WebClient.builder().defaultHeader(\"a\", \"b\").build().get().uri(\"http://orders/orders/1\").retrieve();\n    WebClient s01 = WebClient.builder().baseUrl(\"http://orders\").build();\n    s01.get().uri(\"/orders/1\").retrieve();\n  }\n}\n";
        assert_eq!(
            calls(Language::Java, java),
            [
                "rest_template GET 'http://orders/orders/1' base=? unverified",
                "rest_template GET '{usersUrl}/users/me' base=? unverified",
                "rest_template GET 'http://orders/orders/1' base=? unverified",
                // A subclass of `RestTemplate` may override anything; a `new RestTemplate()`
                // states no root URI, so a handler set on it is its base (not plain).
                "rest_template GET 'http://orders/orders/1' base=? unverified",
                "rest_template GET '/v' base='http://visits' unverified",
                "rest_template GET 'http://orders/orders/1' base=?",
                "rest_template GET 'http://orders/orders/1' base=?",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "rest_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET 'http://orders/orders/1' base=? unverified",
                "web_client GET 'http://orders/orders/1' base=? unverified",
                "web_client GET 'http://orders/orders/1' base=? unverified",
                // `mutate()` is no request call, even when the copy is not used to send one;
                // a builder used only for `build()` is plain, but the request chain's own
                // links after it are not.
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders' unverified",
                "web_client GET '/orders/1' base='http://orders'",
                "web_client GET 'http://orders/orders/1' base=?",
                "web_client GET '/orders/1' base='http://orders'",
            ]
        );
        let ts = "import axios from \"axios\";\nconst p17api = axios.create({ timeout: 5 });\np17api.interceptors.request.use((c) => { c.url = \"https://api.stripe.com/v1/charges\"; return c; });\nconst p18api = axios.create({ baseURL: \"http://orders\" });\np18api.interceptors.request.use((c) => c);\nexport function p17() { return p17api.get(\"http://orders/orders/1\"); }\nexport function p18() { return p18api.get(\"http://orders/orders/1\"); }\nexport function p09() {\n  const a = axios.create({ baseURL: \"http://orders\", adapter: myAdapter });\n  const b = axios.create({ baseURL: \"http://orders\", transformRequest: [t] });\n  const c = axios.create({ baseURL: \"http://orders\", transformResponse: [t] });\n  const d = axios.create({ baseURL: \"http://orders\", httpAgent: ag });\n  const e = axios.create({ baseURL: \"http://orders\", httpsAgent: ag });\n  const g = axios.create({ baseURL: \"http://orders\", proxy: px });\n  const h = axios.create(cfg);\n  a.get(\"/orders/1\"); b.get(\"/orders/1\"); c.get(\"/orders/1\"); d.get(\"/orders/1\"); e.get(\"/orders/1\"); g.get(\"/orders/1\"); h.get(\"http://orders/orders/1\");\n  axios.get(\"http://orders/orders/1\", { adapter: myAdapter });\n  axios.create({ adapter: myAdapter }).get(\"http://orders/orders/1\");\n}\nexport function ok() {\n  const v = axios.create({ baseURL: \"http://orders\", timeout: 5, headers: { a: \"b\" } });\n  v.get(\"/orders/1\", { params: { q: 1 }, timeout: 2 });\n  v.get(\"/orders/1\", { adapter: myAdapter });\n  axios.create({ timeout: 5 }).get(\"http://orders/orders/1\", { adapter: myAdapter });\n  axios(\"http://orders/orders/1\");\n  axios.create({ timeout: 5 }).get(\"http://orders/orders/1\");\n  axios.get(\"http://orders/orders/1\");\n  fetch(\"http://orders/orders/1\");\n}\n";
        assert_eq!(
            calls(Language::TypeScript, ts),
            [
                "fetch GET 'http://orders/orders/1'",
                "axios GET 'http://orders/orders/1' unverified",
                "axios GET 'http://orders/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET 'http://orders/orders/1' unverified",
                "axios GET 'http://orders/orders/1' unverified",
                "axios GET 'http://orders/orders/1' unverified",
                "axios GET '/orders/1' base='http://orders'",
                // A request config with an `adapter` on a plain carrier is not plain either.
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET 'http://orders/orders/1' unverified",
                "axios GET 'http://orders/orders/1'",
                "axios GET 'http://orders/orders/1'",
                "axios GET 'http://orders/orders/1'",
            ]
        );
        // Only the module itself is a bare module client, which the indexer caps when the
        // repository reconfigures it ([`global_hooks`]).
        let facts = extract(Language::TypeScript, ts.as_bytes()).unwrap();
        let modules: Vec<bool> = clients(Language::TypeScript, &facts)
            .calls
            .iter()
            .map(|c| c.module_call)
            .collect();
        let mut expected = vec![false; 18];
        for i in [0, 10, 15, 17] {
            expected[i] = true;
        }
        assert_eq!(modules, expected);
        let go = "package main\nimport \"net/http\"\nfunc f(c *http.Client) {\n\thttp.Get(\"http://orders/orders/1\")\n\thttp.DefaultClient.Get(\"http://orders/orders/1\")\n\tc.Get(\"http://orders/orders/1\")\n\thttp.NewRequest(\"GET\", \"http://orders/orders/1\", nil)\n}\n";
        let facts = extract(Language::Go, go.as_bytes()).unwrap();
        let modules: Vec<(bool, Option<bool>)> = clients(Language::Go, &facts)
            .calls
            .iter()
            .map(|c| (c.module_call, c.base_verified))
            .collect();
        assert_eq!(
            modules,
            [
                (true, None),
                (true, None),
                (false, Some(false)),
                (false, Some(false))
            ]
        );
    }

    /// Contract M4a amendment 15: the module clients a file reconfigures for the whole process.
    /// A request on the module, a call of it, `create` and its import are no hook; a member
    /// read or write, an alias, an export, or passing it on is.
    #[test]
    fn global_hooks_of_module_clients() {
        let hooks = |language: Language, src: &str| {
            global_hooks(language, &extract(language, src.as_bytes()).unwrap())
        };
        let clean = "import axios from \"axios\";\nimport * as ax from \"axios\";\nconst api = axios.create({ baseURL: \"http://orders\" });\naxios.get(\"/a\");\naxios(\"/b\");\nax.post(\"/c\", 1);\nif (axios.isAxiosError(e)) {}\nwindow.fetch(\"/d\");\nfetch(\"/e\");\nglobalThis.location.href;\n";
        assert!(hooks(Language::TypeScript, clean).is_empty());
        for (src, kind) in [
            ("globalThis.fetch = f;\n", ClientKind::Fetch),
            ("window.fetch = f;\n", ClientKind::Fetch),
            ("global.fetch = jest.fn();\n", ClientKind::Fetch),
            ("self.fetch = f;\n", ClientKind::Fetch),
            ("const f = window.fetch;\n", ClientKind::Fetch),
            (
                "import axios from \"axios\";\naxios.defaults.baseURL = \"https://api.stripe.com\";\n",
                ClientKind::Axios,
            ),
            (
                "import axios from \"axios\";\naxios.interceptors.request.use((c) => c);\n",
                ClientKind::Axios,
            ),
            (
                "import http from \"axios\";\nconst ax = http;\n",
                ClientKind::Axios,
            ),
            (
                "import axios from \"axios\";\nexport default axios;\n",
                ClientKind::Axios,
            ),
            (
                "import * as ax from \"axios\";\nsetup(ax);\n",
                ClientKind::Axios,
            ),
            // A call on the module that is no request, `create` or helper.
            (
                "import axios from \"axios\";\naxios.use(plugin);\n",
                ClientKind::Axios,
            ),
            (
                "const axios = require(\"axios\");\naxios.defaults.adapter = a;\n",
                ClientKind::Axios,
            ),
        ] {
            assert_eq!(hooks(Language::TypeScript, src), [kind], "{src}");
        }
        // A `var` require is the module's declaration, not a hook.
        assert!(
            hooks(
                Language::JavaScript,
                "var axios = require(\"axios\");\naxios.get(\"/a\");\n"
            )
            .is_empty()
        );
        // Without the axios import, `axios` is another name.
        assert!(hooks(Language::TypeScript, "axios.defaults.x = 1;\n").is_empty());
        let go =
            |body: &str| format!("package main\nimport \"net/http\"\nfunc f() {{\n{body}\n}}\n");
        assert!(
            hooks(
                Language::Go,
                &go("\thttp.Get(u)\n\thttp.DefaultClient.Do(req)\n\t_ = http.StatusOK")
            )
            .is_empty()
        );
        for body in [
            "\thttp.DefaultTransport = t",
            "\thttp.DefaultClient.Transport = t",
            "\thttp.DefaultClient = &http.Client{}",
            "\tc := http.DefaultClient",
            "\tt := http.DefaultTransport.(*http.Transport).Clone()",
        ] {
            assert_eq!(
                hooks(Language::Go, &go(body)),
                [ClientKind::GoHttp],
                "{body}"
            );
        }
    }

    /// Evaluator pass 5, M9: a chain whose base is not stated is `Fresh` only when it starts
    /// from a new client or builder; one from a binding (an injected builder, another
    /// instance) is unreadable, so the binding's configuration is never taken as empty.
    #[test]
    fn chain_base_reads_a_fresh_root() {
        let src = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass C {\n  void m(RestTemplateBuilder b, WebClient w) {\n    RestTemplate a = b.build();\n    RestTemplate c = new RestTemplateBuilder().build();\n    WebClient d = w.mutate().build();\n    WebClient e = WebClient.builder().build();\n  }\n}\n";
        let facts = extract(Language::Java, src.as_bytes()).unwrap();
        let scope = FileScope::new(&facts);
        let got: Vec<(String, BaseFact)> = facts
            .bindings
            .iter()
            .filter_map(|b| Some((b.name.clone(), init_chain(&facts, b)?)))
            .map(|(name, c)| (name, chain_base(&scope, c, c.links.len())))
            .collect();
        assert_eq!(
            got,
            [
                ("a".to_owned(), BaseFact::Unreadable),
                ("c".to_owned(), BaseFact::Fresh),
                ("d".to_owned(), BaseFact::Unreadable),
                ("e".to_owned(), BaseFact::Fresh),
            ]
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
                // A request built by `NewRequest` is carried by whatever `Do`es it, and a
                // `http.Client` binding may carry any `Transport`: not provably plain
                // (amendment 15). The package and `http.DefaultClient` are the module client.
                "net/http PUT '{base}/x' unverified",
                "net/http DELETE '/y' unverified",
                "net/http HEAD '/z' unverified",
                "net/http POST '/f'",
            ]
        );
        let no_import = src.replace("\t\"net/http\"\n", "");
        assert!(calls(Language::Go, &no_import).is_empty());
    }
}
