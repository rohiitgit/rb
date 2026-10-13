//! HTTP-client packs (design §4.5, contract M4a S9–S11): `fetch` and axios (TS/JS),
//! `RestTemplate`, `WebClient`/`RestClient` and Feign (Java), and Go's `net/http`, from one
//! file's syntax facts. Each client call site gives a method (or unknown), a URL value and a
//! base reference; the indexer folds them ([`crate::fold`]) and stitches them to endpoints.
//!
//! Every client is recognised through an import (or a qualified name), never by a bare short
//! name (contract invariant 6).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap, HashSet};

use rb_syntax::{
    Arg, Binding, CallChain, CallLink, Decoration, DefKind, FileFacts, Init, Language, ObjectEntry,
    Span, StrTemplate, TemplatePart, TokenRole,
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
    /// An OpenAPI-generator client call (contract M6f S6): stitched by operationId.
    OpenApi,
    /// A gRPC stub call (contract M6f S6): stitched by service and RPC name.
    Grpc,
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
            ClientKind::OpenApi => "openapi",
            ClientKind::Grpc => "grpc",
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
    /// A base that cannot be decided, with the values the file states it may take, in source
    /// order (never empty): an axios request config whose last base-setting entry is a spread
    /// of an object literal, a Spring client configured with several bases (contract M5 S19
    /// (f)). It folds like [`UrlValue::Unknown`]; the stitcher makes the site external when a
    /// candidate names an external host.
    Contested(Vec<UrlValue>),
}

impl UrlValue {
    pub(crate) fn of(arg: Option<&Arg>) -> Self {
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
    /// Spring: the call's receiver is an injected instance (nothing in the file configures the
    /// binding): the index into `FileFacts::bindings` of that binding. Its base is the one the
    /// `@Bean` method its injection point resolves to states ([`bean_base`]), which the indexer
    /// reads through DI (contract M4b S21); the pack reads none.
    pub injected: Option<usize>,
    /// Gate A (contract M4a amendments 13–16) on what carries the request: `Some(false)` when
    /// the carrier (the instance, builder or copy the request is sent through) or the request
    /// itself is not provably plain, whatever the URL's source, so the site never reaches
    /// `synthetic` (its host still restricts); otherwise `Some(true)` when the base comes from
    /// the carrier's construction and `None` when it does not. For an imported axios instance
    /// the indexer decides (it is exported, so never verified), and it caps every site of a
    /// client kind the repository may reconfigure ([`global_hooks`], [`feign_config_key`]).
    pub base_verified: Option<bool>,
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
    /// Spans of the file's `NewRequest` call chains whose request value
    /// reaches an in-process sink (contract M6b S4), in source order. The
    /// indexer turns these sites `unmatched`, never edges.
    pub in_process: BTreeSet<Span>,
    /// Generated-client call sites (contract M6f S6): operationId/RPC candidates the indexer
    /// matches against spec operations, in source order.
    pub generated: Vec<GeneratedCall>,
}

/// A generated client (OpenAPI generator, gRPC stub): what names the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedKind {
    OpenApi,
    Grpc,
}

impl GeneratedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            GeneratedKind::OpenApi => "openapi",
            GeneratedKind::Grpc => "grpc",
        }
    }
}

/// One generated-client call site: the operation (operationId or RPC name), its carrier (the
/// `*Api` receiver or stub class/service, for scoping) and the call's span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCall {
    pub kind: GeneratedKind,
    pub operation: String,
    pub carrier: String,
    pub span: Span,
}

/// Every client call site of one file, in source order per pack.
pub fn clients(language: Language, facts: &FileFacts) -> FileClients {
    let facts = &FileScope::new(facts);
    let mut out = FileClients::default();
    match language {
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            fetch(facts, &mut out);
            axios(facts, &mut out, &Gate::new(facts.facts));
            fetch_class(facts, &mut out);
            http_client(facts, &mut out);
            let claimed = claimed(&out);
            generated_calls(Language::TypeScript, facts, &mut out, &claimed);
        }
        Language::Java => {
            let bases = SpringBases::new(facts, Gate::new(facts.facts));
            rest_template(facts, &mut out, &bases);
            web_client(facts, &mut out, &bases);
            feign(facts, &mut out);
            let claimed = claimed(&out);
            generated_calls(Language::Java, facts, &mut out, &claimed);
        }
        Language::Go => {
            go_http(facts, &mut out);
            let claimed = claimed(&out);
            generated_calls(Language::Go, facts, &mut out, &claimed);
        }
        Language::Python => {
            requests_httpx(facts, &mut out);
            let claimed = claimed(&out);
            generated_calls(Language::Python, facts, &mut out, &claimed);
        }
        // Contract M6i S2: client packs land with Tier B.
        Language::Kotlin | Language::CSharp => {}
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
                // A string names no binding (amendment 17).
                TokenRole::Literal => {}
                TokenRole::Callee | TokenRole::Member { .. } | TokenRole::Other => return None,
            }
        }
        (declarations == 1).then_some(calls)
    }
}

// ---------------------------------------------------------------------------------------------
// Global hooks: a module client is plain only while every token naming its process-wide
// configuration is in an allow-listed role (contract M4a amendments 15 and 16)

/// The axios module's members whose call leaves its configuration alone: the requests,
/// `create`, and two pure predicates.
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

/// A Go `http.Client`'s request methods.
const GO_REQUESTS: &[&str] = &["Do", "Get", "Head", "Post", "PostForm"];

/// The module clients this file may reconfigure for the whole process (contract M4a amendment
/// 16), from its name tokens, by allow-list: every token naming the module's configuration
/// surface must be in an allowed role, any other role is a hook.
/// - `fetch`: every `fetch` token (an identifier, a property, a shorthand) is a callee
///   (`fetch(…)`, `window.fetch(…)`) or a `node-fetch`/`cross-fetch` import or `require`
///   declaration, so `globalThis.fetch = …`, `(global as any).fetch = …`, `fetch = …`,
///   `typeof fetch`, `{ fetch }` or a `fetch` method are hooks;
/// - axios: every token of a module binding (a default, namespace or `default as` import, a
///   `require("axios")` at any scope) is its import or `require` declaration, a callee
///   (`axios(…)`), or the receiver of a request, `create` or helper call, so `axios.defaults…`,
///   `axios.interceptors…`, `ax.default.defaults…`, aliasing, exporting or passing it are hooks;
/// - Go `net/http`: a dot import, or a `DefaultTransport`/`DefaultClient` token other than the
///   receiver of a request call (`http.DefaultClient.Get(…)`);
/// - Feign: an import from the Feign library or Spring Cloud OpenFeign outside
///   [`FEIGN_PLAIN_IMPORTS`] (a wildcard too), a token of a Feign extension point that may
///   retarget a request (`RequestInterceptor`, `Capability`, `FeignBuilderCustomizer`,
///   `Targeter`, `Feign`), a qualified `feign.…` use, a `configuration` on a `@FeignClient`, or
///   a `defaultConfiguration` on `@EnableFeignClients`.
///
/// The indexer caps every site of a kind any file of the repository hooks (bare calls and
/// axios instances alike), and every Feign site when a configuration file sets a Feign client
/// key ([`feign_config_key`]).
pub fn global_hooks(language: Language, facts: &FileFacts) -> Vec<ClientKind> {
    let mut out = Vec::new();
    let within = |inner: Span, outer: Span| {
        (outer[0], outer[1]) <= (inner[0], inner[1]) && (inner[2], inner[3]) <= (outer[2], outer[3])
    };
    let any_token = |hook: &dyn Fn(&rb_syntax::NameToken) -> bool| {
        facts.name_tokens.iter().any(|t| {
            crate::work::tick(1);
            hook(t)
        })
    };
    match language {
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            // The `import`/`require` declarations of `modules`.
            let declared =
                |modules: &[&str], requires: &HashSet<usize>, t: &rb_syntax::NameToken| {
                    facts.imports.iter().any(|i| {
                        modules.iter().any(|m| is_module(&i.module, m)) && within(t.span, i.span)
                    }) || requires
                        .iter()
                        .any(|&bi| within(t.span, facts.bindings[bi].span))
                };
            let fetch_requires: HashSet<usize> = FETCH_MODULES
                .iter()
                .flat_map(|m| module_names(facts, m, &[], false).1)
                .collect();
            if any_token(&|t| {
                t.name == "fetch"
                    && t.role != TokenRole::Callee
                    && !declared(FETCH_MODULES, &fetch_requires, t)
            }) {
                out.push(ClientKind::Fetch);
            }
            let (names, requires) = module_names(facts, "axios", &["default"], true);
            // A module name read as a destructured member's object (`const {
            // get } = axios`) reconfigures nothing (contract M6b S9): the
            // pack reads the member call, capped, at that binding.
            let member_src = |t: &rb_syntax::NameToken| {
                crate::work::tick(1);
                facts.bindings.iter().any(|b| {
                    matches!(&b.init, Some(Init::Member { object, .. }) if object == &t.name)
                        && within(t.span, b.span)
                })
            };
            if any_token(&|t| match &t.role {
                // The module named outside its import or `require` declarations (amendment
                // 17): `import x = require(…)`, `require(…).default`, a destructured or bare
                // `require`, `import(…)`, a re-export, `jest.mock(…)`.
                TokenRole::Literal => {
                    is_module(&t.name, "axios") && !declared(&["axios"], &requires, t)
                }
                TokenRole::Callee => false,
                // Reading a request member to call or alias it (`axios.get`,
                // `const get = axios.get`) reconfigures nothing; reading or
                // writing anything else (`axios.defaults`, `axios.default`)
                // may.
                TokenRole::Member { name } => {
                    names.contains(&t.name)
                        && !(AXIOS_VERBS.contains(&name.as_str()) || name == "request")
                }
                TokenRole::Receiver { call } => {
                    names.contains(&t.name) && !AXIOS_MODULE_CALLS.contains(&call.as_str())
                }
                _ => {
                    names.contains(&t.name) && !declared(&["axios"], &requires, t) && !member_src(t)
                }
            }) {
                out.push(ClientKind::Axios);
            }
            // Fetch-class libraries and `HttpClient` (contract M6b S9): every
            // token of a module binding outside its allow-listed roles hooks
            // `fetch` — the module named outside its declarations, a receiver
            // of anything but a verb or configuration call, aliasing,
            // exporting, passing. A direct call is the pack's site.
            // `got.paginate`/`got.stream` read without returning a response:
            // allow-listed, not sites (residue).
            for (module, exported, verbs, config, extra) in [
                (
                    "got",
                    &["default"] as &[&str],
                    GOT_KY_VERBS,
                    &["extend"] as &[&str],
                    &["paginate", "stream"] as &[&str],
                ),
                (
                    "ky",
                    &["default"] as &[&str],
                    GOT_KY_VERBS,
                    &["extend", "create"] as &[&str],
                    &[] as &[&str],
                ),
                (
                    "ky-universal",
                    &["default"] as &[&str],
                    GOT_KY_VERBS,
                    &["extend", "create"] as &[&str],
                    &[] as &[&str],
                ),
                (
                    "superagent",
                    &["default"] as &[&str],
                    SUPERAGENT_VERBS,
                    &["agent"] as &[&str],
                    &[] as &[&str],
                ),
                (
                    "undici",
                    &["request"] as &[&str],
                    &["request"] as &[&str],
                    &[] as &[&str],
                    &[] as &[&str],
                ),
            ] {
                let (names, requires) = module_names(facts, module, exported, true);
                if any_token(&|t| match &t.role {
                    TokenRole::Callee => false,
                    TokenRole::Member { name } => {
                        names.contains(&t.name) && !verbs.contains(&name.as_str())
                    }
                    TokenRole::Receiver { call } => {
                        names.contains(&t.name)
                            && !verbs.contains(&call.as_str())
                            && !config.contains(&call.as_str())
                            && !extra.contains(&call.as_str())
                    }
                    _ => {
                        names.contains(&t.name)
                            && !declared(&[module], &requires, t)
                            && !member_src(t)
                    }
                }) {
                    out.push(ClientKind::Fetch);
                }
            }
            // `HttpClient` providers and interceptors configure every client
            // (contract M6b S9): any non-import mention of their names hooks
            // `fetch`. The `HttpClient` name itself never hooks (its import
            // and type mentions are the normal injection use).
            if facts
                .imports
                .iter()
                .any(|i| is_module(&i.module, "@angular/common/http"))
                && [
                    "HTTP_INTERCEPTORS",
                    "HttpInterceptor",
                    "HttpHandler",
                    "provideHttpClient",
                    "withInterceptors",
                    "withInterceptorsFromDi",
                ]
                .iter()
                .any(|n| {
                    facts.mentions.get(*n).is_some_and(|roles| {
                        roles.iter().any(|r| *r != rb_syntax::MentionRole::Import)
                    })
                })
            {
                out.push(ClientKind::Fetch);
            }
        }
        Language::Go => {
            let dot = facts
                .imports
                .iter()
                .any(|i| i.module == "net/http" && i.alias.as_deref() == Some("."));
            if dot
                || any_token(&|t| {
                    rb_syntax::surface_names(language).contains(&t.name.as_str())
                        && !matches!(&t.role, TokenRole::Receiver { call } if GO_REQUESTS.contains(&call.as_str()))
                })
            {
                out.push(ClientKind::GoHttp);
            }
        }
        Language::Java => {
            let in_import =
                |t: &rb_syntax::NameToken| facts.imports.iter().any(|i| within(t.span, i.span));
            let configuring_import = facts.imports.iter().any(|i| {
                FEIGN_MODULES.iter().any(|m| dotted_under(&i.module, m))
                    && (i.wildcard
                        || i.is_static
                        || i.names.iter().any(|n| {
                            !FEIGN_PLAIN_IMPORTS.contains(&(i.module.as_str(), n.name.as_str()))
                        }))
            });
            let configured = |d: &Decoration| {
                FEIGN_PACKAGES.iter().any(|p| {
                    is_annotation(facts, d, p, "FeignClient")
                        && keyed(d, &["configuration"]).is_some()
                        || is_annotation(facts, d, p, "EnableFeignClients")
                            && keyed(d, &["defaultConfiguration"]).is_some()
                })
            };
            if configuring_import
                || any_token(&|t| {
                    rb_syntax::surface_names(language).contains(&t.name.as_str())
                        && (t.name != "feign" || !in_import(t))
                })
                || facts
                    .definitions
                    .iter()
                    .flat_map(|d| &d.decorations)
                    .any(configured)
            {
                out.push(ClientKind::Feign);
            }
        }
        Language::Python => {}
        // Contract M6i S2: client packs land with Tier B.
        Language::Kotlin | Language::CSharp => {}
    }
    out
}

/// The Feign library and Spring Cloud OpenFeign packages (subpackages included).
const FEIGN_MODULES: &[&str] = &[
    "feign",
    "org.springframework.cloud.openfeign",
    "org.springframework.cloud.netflix.feign",
];

/// The Feign imports that configure no request (contract M4a amendment 16): the declarative
/// annotations, exceptions, response decoding, logging and retries. Every other import from
/// [`FEIGN_MODULES`] (an interceptor, a `RequestTemplate`, a `Client`, an `Encoder`, a
/// `Capability`, a builder or customizer, a contract) may change where a request goes.
const FEIGN_PLAIN_IMPORTS: &[(&str, &str)] = &[
    ("feign", "FeignException"),
    ("feign", "RetryableException"),
    ("feign", "Response"),
    ("feign", "Param"),
    ("feign", "Headers"),
    ("feign", "HeaderMap"),
    ("feign", "QueryMap"),
    ("feign", "RequestLine"),
    ("feign", "Body"),
    ("feign", "Logger"),
    ("feign", "Retryer"),
    ("feign.codec", "Decoder"),
    ("feign.codec", "ErrorDecoder"),
    ("feign.codec", "DecodeException"),
    ("feign.hystrix", "FallbackFactory"),
    ("org.springframework.cloud.openfeign", "FeignClient"),
    ("org.springframework.cloud.openfeign", "EnableFeignClients"),
    ("org.springframework.cloud.openfeign", "SpringQueryMap"),
    ("org.springframework.cloud.openfeign", "FallbackFactory"),
    ("org.springframework.cloud.openfeign", "CollectionFormat"),
    ("org.springframework.cloud.netflix.feign", "FeignClient"),
    (
        "org.springframework.cloud.netflix.feign",
        "EnableFeignClients",
    ),
];

/// A TS/JS module specifier that is `module` or one of its subpaths
/// (`axios/dist/node/axios.cjs`).
fn is_module(spec: &str, module: &str) -> bool {
    spec.strip_prefix(module)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// A dotted name that is `prefix` or below it: a Java package or one of its subpackages
/// (`feign.codec`), a configuration key or one of its children.
fn dotted_under(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
}

/// A configuration key that configures Feign clients (`feign.client.config…`,
/// `spring.cloud.openfeign.client.config…`: interceptors, encoders, a URL; contract M4a
/// amendment 16): any such key in any configuration file caps every Feign site.
pub fn feign_config_key(key: &str) -> bool {
    [
        "feign.client.config",
        "spring.cloud.openfeign.client.config",
    ]
    .iter()
    .any(|p| dotted_under(key, p))
}

// ---------------------------------------------------------------------------------------------
// Files without token facts: a raw-text scan (contract M4a amendment 17)

/// What the raw-text scan of the repository's files without token facts found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawScan {
    /// The client kinds some such file may reconfigure: every site of them is capped.
    pub hooks: BTreeSet<ClientKind>,
    /// Some file without facts configures service discovery or load balancing, which may bind a
    /// service name to any host: no host-by-name stitch (F1) is `synthetic`.
    /// Parsed files no longer feed this (contract M6b S8): their text feeds
    /// `discovery_parsed` (broad, counted only) and `discovery_literal`
    /// (narrowed, caps) instead.
    pub discovery: bool,
    /// Some parsed file's text spells a discovery key path or word
    /// ([`discovery_text`], contract M5 S19 (a)): the broad rule, kept for the
    /// S8 lift count only — it never caps (contract M6b S8).
    pub discovery_parsed: bool,
    /// Some parsed file holds discovery key segments inside one
    /// brace-delimited span ([`single_literal_discovery`]): the narrowed text
    /// evidence, which caps (contract M6b S8, D7).
    pub discovery_literal: bool,
    /// DI-surface findings (contract M4b S6 gate D) as `(path, what)`
    /// ([`crate::di::surface_text`]).
    pub di_surface: Vec<(String, String)>,
    /// Every whole word of the text of such files that are not inert (comments included), for
    /// the mention gates (contract M4b S6 gate M, S9 (d)).
    pub words: BTreeSet<String>,
    /// Some non-inert file was over the byte cap: its words are unknown, so every name counts
    /// as mentioned.
    pub words_unread: bool,
}

impl RawScan {
    pub fn merge(&mut self, other: RawScan) {
        self.hooks.extend(other.hooks);
        self.discovery |= other.discovery;
        self.di_surface.extend(other.di_surface);
        self.words.extend(other.words);
        self.words_unread |= other.words_unread;
    }
}

/// The whole words of a text: maximal runs of `[A-Za-z0-9_$]` and other alphanumeric
/// characters, and each `$`-separated segment of such a run, so a nested class's binary name
/// (`Outer$Inner`) names `Inner` too (evaluator pass 1, Y1).
pub fn text_words(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for run in text
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .filter(|w| !w.is_empty())
    {
        if run.contains('$') {
            out.extend(run.split('$').filter(|w| !w.is_empty()).map(str::to_owned));
        }
        out.insert(run.to_owned());
    }
    out
}

/// The names whose appearance as a whole word in a file without token facts taints a client
/// kind: the TS/JS module clients' names, Feign's extension points (as in
/// [`rb_syntax::surface_names`]) and a qualified `feign.` use; Go's process-wide clients, in Go
/// files only ([`raw_scan`]).
const RAW_SURFACE: &[(ClientKind, &[&str])] = &[
    (
        ClientKind::Fetch,
        &["fetch", "got", "ky", "superagent", "undici"],
    ),
    (ClientKind::Axios, &["axios"]),
    (
        ClientKind::Feign,
        &[
            "RequestInterceptor",
            "Capability",
            "FeignBuilderCustomizer",
            "Targeter",
            "Feign",
            "feign.",
        ],
    ),
    (ClientKind::GoHttp, &["DefaultTransport", "DefaultClient"]),
];

/// Configuration keys under which service discovery or a load balancer may bind a service
/// name to any instance list (contract M4a amendment 17, W4). Eureka's own keys are not here:
/// Eureka resolves a name to the instances registered under it, which is F1's convention.
const DISCOVERY_PREFIXES: &[&str] = &[
    "spring.cloud.discovery",
    "spring.cloud.loadbalancer",
    "spring.cloud.kubernetes.discovery",
    "spring.cloud.consul.discovery",
];

/// Whole words that configure discovery or load balancing in the text of any file (parsed
/// ones included since contract M5 S19 (a)), matched in lower case with `_` read as `.` (so an
/// environment variable such as `SPRING_CLOUD_DISCOVERY_CLIENT_SIMPLE_INSTANCES_ORDERS_0_URI`
/// counts): the [`DISCOVERY_PREFIXES`] written out, Ribbon's keys
/// (`<service>.ribbon.listOfServers`), Spring Cloud LoadBalancer's and Ribbon's configuration
/// types, and their packages (a fully qualified type of either, with no import or annotation).
const DISCOVERY_WORDS: &[&str] = &[
    "spring.cloud.discovery",
    "spring.cloud.loadbalancer",
    "spring.cloud.kubernetes.discovery",
    "spring.cloud.consul.discovery",
    "org.springframework.cloud.loadbalancer",
    "org.springframework.cloud.netflix.ribbon",
    "ribbon",
    "listofservers",
    "loadbalancerclient",
    "loadbalancerclients",
    "serviceinstancelistsupplier",
    "ribbonclient",
    "ribbonclients",
];

/// A configuration key under one of the [`DISCOVERY_PREFIXES`]: the parsed keys catch a
/// prefix written as nested YAML; Ribbon's keys and static instance lists are whole words of
/// the text ([`DISCOVERY_WORDS`]) in any spelling.
fn discovery_key(key: &str) -> bool {
    DISCOVERY_PREFIXES.iter().any(|p| dotted_under(key, p))
}

/// A Java package whose use (an import, a fully qualified annotation) configures
/// discovery or load balancing: Spring Cloud LoadBalancer (`@LoadBalancerClient`,
/// `ServiceInstanceListSupplier`) and Ribbon (`@RibbonClient`), subpackages included
/// (contract M4a amendment 17, W4).
pub fn discovery_package(package: &str) -> bool {
    [
        "org.springframework.cloud.loadbalancer",
        "org.springframework.cloud.netflix.ribbon",
    ]
    .iter()
    .any(|p| dotted_under(package, p))
}

/// Text no runtime executes or reads as configuration: prose and npm lock files (which only
/// record what the `package.json` files declare).
pub(crate) fn inert(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some("md" | "markdown" | "txt" | "rst" | "adoc")
    ) || matches!(
        name,
        "package-lock.json"
            | "npm-shrinkwrap.json"
            | "yarn.lock"
            | "pnpm-lock.yaml"
            // Python lockfiles only record what the manifests declare, as
            // above (contract M6d S6).
            | "poetry.lock"
            | "uv.lock"
    )
}

/// The `package.json` fields whose entries name registry packages.
const DEPENDENCY_FIELDS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

/// The text of a file the scan reads: a `package.json` without its plain dependency entries
/// (a registry version range, no `:` or `/`: not an alias, a path, a URL, a workspace or a
/// GitHub package, which may stand in for the module). XML comments are read like the rest
/// (contract M5 S19 (b): a `<!--` inside CDATA used to hide what followed).
fn scanned_text(path: &str, text: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    if name == "package.json"
        && let Ok(mut json) = serde_json::from_str::<serde_json::Value>(text)
    {
        for field in DEPENDENCY_FIELDS {
            if let Some(deps) = json.get_mut(*field).and_then(|d| d.as_object_mut()) {
                deps.retain(|_, v| !v.as_str().is_some_and(|v| !v.contains([':', '/'])));
            }
        }
        return json.to_string();
    }
    text.to_owned()
}

/// Whether `word` appears in `text` with no identifier character (`[A-Za-z0-9_]`) right
/// before it or, when it ends with one, right after it.
fn has_word(text: &str, word: &str) -> bool {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_';
    let bytes = text.as_bytes();
    let tail = word.as_bytes().last().is_some_and(|&c| ident(c));
    text.match_indices(word).any(|(i, _)| {
        let end = i + word.len();
        (i == 0 || !ident(bytes[i - 1])) && (!tail || end == bytes.len() || !ident(bytes[end]))
    })
}

/// The key segments of every key-like word of `text` (contract M5 S19 (a)): a run of
/// identifier characters (`[A-Za-z0-9_$.-]`) followed, after any quotes, backslashes and
/// spaces, by a `:` that is not `::` — a JSON key (`"spring":`, `\"spring\":` inside another
/// string), a JS or YAML key (`spring:`, `'load-balancer':`). Each is split on `.`,
/// lower-cased, with `-` and `_` removed (Spring's relaxed binding). Neither strings nor
/// braces are parsed, so no value (an unbalanced brace, a quote) can hide a key; the cost is
/// that unrelated keys of one file may combine ([`json_discovery`]), which can only cap.
fn json_key_segments(text: &str) -> HashSet<String> {
    let bytes = text.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'$' | b'.');
    let mut out = HashSet::new();
    let mut i = 0;
    while i < bytes.len() {
        if !ident(bytes[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && ident(bytes[i]) {
            i += 1;
        }
        let mut j = i;
        while j < bytes.len() && matches!(bytes[j], b'"' | b'\'' | b'\\' | b' ' | b'\t') {
            j += 1;
        }
        if bytes.get(j) == Some(&b':') && bytes.get(j + 1) != Some(&b':') {
            for seg in text[start..i].split('.') {
                let seg: String = seg
                    .chars()
                    .filter(|c| !matches!(c, '-' | '_'))
                    .flat_map(char::to_lowercase)
                    .collect();
                if !seg.is_empty() {
                    out.insert(seg);
                }
            }
        }
    }
    out
}

/// Whether the JSON-style keys of `text` may spell a [`DISCOVERY_PREFIXES`] path: `spring`,
/// `cloud` and `discovery` or `loadbalancer` all among its key segments (every prefix has
/// them).
fn json_discovery(text: &str) -> bool {
    let keys = json_key_segments(text);
    keys.contains("spring")
        && keys.contains("cloud")
        && (keys.contains("discovery") || keys.contains("loadbalancer"))
}

/// Whether the text of a file configures service discovery or load balancing (contract M4a
/// amendment 17, W4; M5 S19 (a)): a [`DISCOVERY_WORDS`] whole word, or a JSON key path spelling
/// a discovery key ([`json_discovery`]). Used for every non-binary file, parsed ones included;
/// inert text ([`inert`]) is not read.
pub fn discovery_text(path: &str, text: &str) -> bool {
    if inert(path) {
        return false;
    }
    let normalised = text.to_ascii_lowercase().replace('_', ".");
    DISCOVERY_WORDS.iter().any(|w| has_word(&normalised, w)) || json_discovery(text)
}

/// Simple names of types that configure service discovery or load balancing
/// (contract M6b S8; evaluator pass 1, E2): Spring Cloud LoadBalancer's and
/// Ribbon's configuration types (the type entries of [`DISCOVERY_WORDS`])
/// plus the load-balancer itself, which routes through instance suppliers. Any
/// other spelling — a comment, a UI string, an unrelated class from the same
/// package tree — is not positively identified evidence.
const DISCOVERY_TYPES: &[&str] = &[
    "LoadBalancerClient",
    "LoadBalancerClients",
    "ReactorLoadBalancer",
    "RibbonClient",
    "RibbonClients",
    "ServiceInstanceListSupplier",
];

/// The qualifier and simple name of a type as written (`a.b.C` → (`a.b`,
/// `C`); `C` → (`None`, `C`)), cut at any type arguments (`C<X>` → `C`).
fn split_type(name: &str) -> (Option<&str>, &str) {
    let bare = name.split('<').next().unwrap_or(name).trim();
    match bare.rsplit_once('.') {
        Some((q, s)) => (Some(q), s),
        None => (None, bare),
    }
}

/// Whether a parsed file positively resolves discovery or load-balancer
/// configuration (contract M6b S8, D7): a use of a [`DISCOVERY_TYPES`] name —
/// an annotation or decorator, a supertype head, a declared field or parameter
/// type (type arguments included) or a method return type — that resolves
/// through imports to a discovery package (an explicit single-type import
/// binding it, or an on-demand import of a discovery package, which cannot
/// attribute the use, so stays capped), or is fully qualified under a
/// discovery package. Static imports bind members, never types. A bare use
/// with no import to resolve through is not evidence (it may be the
/// repository's own type of that name); neither is a type nested in another
/// type's arguments outside a binding's recorded ones.
pub fn discovery_resolved(facts: &FileFacts) -> bool {
    let resolves = |qual: Option<&str>, simple: &str| {
        if !DISCOVERY_TYPES.contains(&simple) {
            return false;
        }
        if let Some(q) = qual {
            return discovery_package(q);
        }
        facts.imports.iter().any(|i| {
            !i.is_static
                && discovery_package(&i.module)
                && (i.names.iter().any(|n| n.local == simple) || i.wildcard)
        })
    };
    facts.definitions.iter().any(|d| {
        d.decorations.iter().any(|deco| {
            let (q, s) = split_type(&deco.name);
            resolves(q, s)
        }) || d.supertypes.iter().any(|s| {
            let (q, sm) = split_type(&s.name);
            resolves(q, sm)
        }) || d.return_type.as_ref().is_some_and(|r| {
            let (q, sm) = split_type(r);
            resolves(q, sm)
        })
    }) || facts.bindings.iter().any(|b| {
        let (q, s) = split_type(&b.type_name);
        resolves(q, s)
            || b.type_args.iter().any(|a| {
                // Type arguments are recorded as simple names (a bounded
                // wildcard keeps its `?` mark, stripped for resolution here):
                // only an import can resolve them.
                let a = a.strip_prefix('?').unwrap_or(a);
                DISCOVERY_TYPES.contains(&a)
                    && facts.imports.iter().any(|i| {
                        !i.is_static
                            && discovery_package(&i.module)
                            && (i.names.iter().any(|n| n.local == *a) || i.wildcard)
                    })
            })
    })
}

/// Whether a parsed file's facts positively resolve discovery or
/// load-balancer configuration (contract M6b S8, D7): Java only — the known
/// types are Java types — with a use resolving through imports (see
/// [`discovery_resolved`]).
pub fn discovery_facts(language: Language, facts: &FileFacts) -> bool {
    language == Language::Java && discovery_resolved(facts)
}

/// The broad discovery rule against the narrowed one, `(broad, narrowed)`
/// (contract M6b S8, D7; invariant 6). Broad is raw-file evidence
/// (`unparsed.discovery`), parsed-file discovery words
/// (`unparsed.discovery_parsed`), or an unresolved Java import naming a
/// discovery package; narrowed is raw-file evidence, parsed-file
/// single-literal keys (`unparsed.discovery_literal`), or import-resolved
/// known types ([`discovery_facts`]). A split (broad without narrowed) is
/// what the stitcher's `discovery_lifted` count counts.
pub fn discovery_flags<'a>(
    unparsed: &RawScan,
    files: impl IntoIterator<Item = (Language, &'a FileFacts)>,
) -> (bool, bool) {
    let mut broad_java = false;
    let mut resolved = false;
    for (language, facts) in files {
        resolved |= discovery_facts(language, facts);
        if language != Language::Java {
            continue;
        }
        broad_java |= facts.imports.iter().any(|i| discovery_package(&i.module))
            || facts
                .definitions
                .iter()
                .flat_map(|d| &d.decorations)
                .any(|d| {
                    d.name
                        .rsplit_once('.')
                        .is_some_and(|(q, _)| discovery_package(q))
                });
    }
    let broad = unparsed.discovery || unparsed.discovery_parsed || broad_java;
    let narrowed = unparsed.discovery || unparsed.discovery_literal || resolved;
    (broad, narrowed)
}

/// Whether one brace-delimited span of `text` holds key segments spelling a
/// discovery key path ([`json_discovery`]'s segments, contract M6b S8):
/// `spring`, `cloud` and `discovery` or `loadbalancer` co-occurring inside a
/// single object literal. Nested literals count — their keys sit inside the
/// outer literal too, so each popped span's segments merge into its parent —
/// while sibling literals and brace-free key lists never combine. A single
/// linear pass with a brace stack, so 4× the input costs ~4× the counted work
/// (invariant 9). Unbalanced text can only cap (an unclosed span is checked at
/// the end); a stray `}` may split a span (documented residue, as with the
/// structure-free rule this narrows).
pub fn single_literal_discovery(text: &str) -> bool {
    fn hit(segments: &HashSet<String>) -> bool {
        segments.contains("spring")
            && segments.contains("cloud")
            && (segments.contains("discovery") || segments.contains("loadbalancer"))
    }
    // The key rule mirrors [`json_key_segments`]: a run of identifier
    // characters followed, past quotes, backslashes and spaces, by a `:` that
    // is not `::`.
    let bytes = text.as_bytes();
    let ident = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b'$' | b'.');
    let mut stack: Vec<HashSet<String>> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        crate::work::tick(1);
        let c = bytes[i];
        if c == b'{' {
            stack.push(HashSet::new());
            i += 1;
            continue;
        }
        if c == b'}' {
            if let Some(top) = stack.pop() {
                if hit(&top) {
                    return true;
                }
                if let Some(parent) = stack.last_mut() {
                    parent.extend(top);
                }
            }
            i += 1;
            continue;
        }
        if !ident(c) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && ident(bytes[i]) {
            i += 1;
        }
        let mut j = i;
        while j < bytes.len() && matches!(bytes[j], b'"' | b'\'' | b'\\' | b' ' | b'\t') {
            j += 1;
        }
        if bytes.get(j) == Some(&b':')
            && bytes.get(j + 1) != Some(&b':')
            && let Some(top) = stack.last_mut()
        {
            for seg in text[start..i].split('.') {
                let seg: String = seg
                    .chars()
                    .filter(|c| !matches!(c, '-' | '_'))
                    .flat_map(char::to_lowercase)
                    .collect();
                if !seg.is_empty() {
                    top.insert(seg);
                }
            }
        }
    }
    stack.into_iter().any(|segments| hit(&segments))
}

/// The raw-text scan of one file without token facts (contract M4a amendment 17, evaluator
/// pass 7, W1, W2, W4): a language M4a does not parse (Kotlin, Groovy, Scala, Vue, Svelte,
/// HTML, …), a generated file, one too large to parse, or any other text. Each name of
/// [`RAW_SURFACE`] found as a whole word taints its kind as a disallowed token would (Go's
/// names only in a `.go` file); [`DISCOVERY_WORDS`], or a discovery key of a settings file
/// (nested YAML too), sets `discovery`. `text` is `None` for a file over the caller's byte cap:
/// it taints every kind it would be scanned for, and discovery, so the cap can only
/// over-taint. Inert text ([`inert`]) is not scanned.
pub fn raw_scan(path: &str, text: Option<&str>) -> RawScan {
    let mut out = RawScan {
        di_surface: crate::di::surface_text(path, text)
            .into_iter()
            .map(|what| (path.to_owned(), what))
            .collect(),
        ..RawScan::default()
    };
    if inert(path) {
        return out;
    }
    match text {
        Some(text) => out.words = text_words(text),
        None => out.words_unread = true,
    }
    let go = path.to_ascii_lowercase().ends_with(".go");
    let kinds = RAW_SURFACE
        .iter()
        .filter(|(kind, _)| go || *kind != ClientKind::GoHttp);
    let Some(text) = text else {
        out.hooks = kinds.map(|(kind, _)| *kind).collect();
        out.discovery = true;
        return out;
    };
    let text = scanned_text(path, text);
    out.hooks = kinds
        .filter(|(_, names)| names.iter().any(|n| has_word(&text, n)))
        .map(|(kind, _)| *kind)
        .collect();
    let keys = match path.rsplit_once('.').map(|(_, e)| e) {
        Some("properties") => crate::fold::parse_properties(&text),
        Some("yml" | "yaml") => crate::fold::yaml_entries(&text),
        _ => Default::default(),
    };
    out.discovery = discovery_text(path, &text) || keys.keys().any(|k| discovery_key(k));
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

/// The first `key` entry among call keyword arguments (each reads as a
/// single-entry object, so any `Arg::Object` in the way is a keyword).
pub(crate) fn kwarg<'a>(args: &'a [Arg], key: &str) -> Option<&'a Arg> {
    args.iter()
        .filter_map(|a| match a {
            Arg::Object { entries, .. } => entries.iter().find(|e| e.key == key).map(|e| &e.value),
            _ => None,
        })
        .next()
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
            injected: None,
            base_verified: None,
        });
    }
}

// ---------------------------------------------------------------------------------------------
// fetch-class libraries: got, ky, superagent, undici, and Angular HttpClient
// (contract M6b S9)

/// TS/JS HTTP client modules that enter as fetch-class sites.
const FETCH_CLASS_MODULES: &[&str] = &["got", "ky", "ky-universal", "superagent", "undici"];

/// Verbs of `got` and `ky`.
const GOT_KY_VERBS: &[&str] = &["get", "post", "put", "patch", "head", "delete", "options"];

/// Verbs of `superagent` (`del` is its `delete` alias).
const SUPERAGENT_VERBS: &[&str] = &[
    "get", "post", "put", "patch", "head", "delete", "options", "del",
];

/// A configured carrier: a name bound to `got.extend`, `ky.extend` /
/// `ky.create` or `superagent.agent`, with the verbs it takes.
#[derive(Clone, Copy)]
enum Carrier {
    Got,
    Ky,
    Superagent,
}

impl Carrier {
    fn verbs(self) -> &'static [&'static str] {
        match self {
            Carrier::Got | Carrier::Ky => GOT_KY_VERBS,
            Carrier::Superagent => SUPERAGENT_VERBS,
        }
    }
}

/// The object a callee-name binding destructures (`const { get } = axios`):
/// the object as written and the binding, when the call's name resolves to
/// that binding (contract M6b S9). Covers `Init::Member` (`const { get } =
/// axios`, `const { get } = require("axios")`) and a member path kept as a
/// name (`const get = axios.get`).
fn member_object(facts: &FileScope<'_>, chain: &CallChain, name: &str) -> Option<(String, usize)> {
    let bi = facts.binding_index_at(name, chain.span)?;
    match &facts.bindings[bi].init {
        Some(Init::Member { object, .. }) => Some((object.clone(), bi)),
        Some(Init::Name { text, .. }) => text
            .rsplit_once('.')
            .map(|(object, _)| (object.to_owned(), bi)),
        _ => None,
    }
}

/// `got`, `ky`, `superagent` and `undici` as fetch-class sites (contract M6b
/// S9): module verbs and direct calls with import/`require` gates (the
/// `undici` `request` name, or its namespace's `request` member), destructured
/// members, and calls on configured carriers (`got.extend`, `ky.extend` /
/// `ky.create`, `superagent.agent` results). Carrier and member sites never
/// rise above `heuristic`; an `undici` pool or client is no fetch-class shape
/// (residue).
fn fetch_class(facts: &FileScope<'_>, out: &mut FileClients) {
    let got = module_names(facts, "got", &["default"], true);
    let ky = {
        let (mut names, mut requires) = module_names(facts, "ky", &["default"], true);
        let (u_names, u_requires) = module_names(facts, "ky-universal", &["default"], true);
        names.extend(u_names);
        requires.extend(u_requires);
        (names, requires)
    };
    let sa = module_names(facts, "superagent", &["default"], true);
    let undici = module_names(facts, "undici", &["request"], true);
    // `name` at `at` is one of the module's local names, not shadowed by a
    // parameter or variable in effect there (as for `is_axios`); the `require`
    // binding is the module.
    let is_lib = |names: &(Vec<String>, HashSet<usize>), name: &str, at: Span| {
        names.0.iter().any(|n| n == name)
            && facts
                .binding_index_at(name, at)
                .is_none_or(|bi| names.1.contains(&bi))
    };
    // A destructured object is the module itself (`const { get } =
    // require("got")`, with no import) or one of its local names; a shadowed
    // module name resolves to the shadowing binding, never the module.
    let is_lib_or_module =
        |names: &(Vec<String>, HashSet<usize>), modules: &[&str], object: &str, at: Span| {
            is_lib(names, object, at)
                || modules.iter().any(|m| is_module(object, m))
                    && facts.binding_index_at(object, at).is_none()
        };
    // Configured carriers: `const api = got.extend(…)` and kin.
    let mut carriers: HashMap<usize, Carrier> = HashMap::new();
    for (bi, b) in facts.bindings.iter().enumerate().filter(|(_, b)| !b.member) {
        let Some(c) = init_chain(facts, b) else {
            continue;
        };
        if let (Some(root), [link]) = (c.root.as_deref(), c.links.as_slice()) {
            let carrier = if is_lib(&got, root, c.span) && link.name == "extend" {
                Some(Carrier::Got)
            } else if is_lib(&ky, root, c.span) && matches!(link.name.as_str(), "extend" | "create")
            {
                Some(Carrier::Ky)
            } else if is_lib(&sa, root, c.span) && link.name == "agent" {
                Some(Carrier::Superagent)
            } else {
                None
            };
            if let Some(carrier) = carrier {
                carriers.insert(bi, carrier);
            }
        }
    }
    // A fetch-class site: `url` from the verb's first argument, `capped` for
    // configured carriers and destructured members (contract M6b S9, D8).
    let site =
        |chain: &CallChain, method: Option<String>, url: Option<&Arg>, capped: bool| ClientCall {
            kind: ClientKind::Fetch,
            method,
            url: UrlValue::of(url),
            base: None,
            instance: None,
            span: chain.span,
            source: chain.enclosing,
            injected: None,
            base_verified: capped.then_some(false),
        };
    // A direct call's method from its options (`got(url, { method })`,
    // `request(url, { method })`), `GET` by default as for `fetch`.
    let opts_method = |args: &[Arg]| match args.get(1) {
        None => Some("GET".to_owned()),
        Some(Arg::Object { .. }) => method_entry(args.get(1)).unwrap_or(Some("GET".to_owned())),
        Some(_) => None,
    };
    // Options that reconfigure the request (evaluator pass 1, E1):
    // interceptor `hooks` everywhere, a custom `agent` transport for `got`.
    // Only inline objects read: a spread may hide either, and a name may hide
    // anything, so both cap.
    let configured_opts = |keys: &[&str], args: &[Arg]| match args.get(1) {
        None => false,
        Some(Arg::Object { entries, .. }) => entries
            .iter()
            .any(|e| e.key == "..." || keys.contains(&e.key.as_str())),
        Some(_) => true,
    };
    // A verb-site method (`del` is `superagent`'s `delete` alias).
    let verb_method = |verb: &str| {
        Some(if verb == "del" {
            "DELETE".to_owned()
        } else {
            norm_method(verb)
        })
    };
    // A `superagent` plugin (`.use`) reconfigures the request (evaluator pass
    // 1, E1): any `use` link past the call itself caps the site. A leading
    // `use` is no site at all (the module call is missed, and the token
    // hooks).
    let plugin = |chain: &CallChain| chain.links.iter().skip(1).any(|l| l.name == "use");
    for chain in &facts.chains {
        let Some(first) = chain.links.first() else {
            continue;
        };
        if chain.root.is_none() {
            if first.args.is_empty() {
                continue;
            }
            if is_lib(&got, &first.name, chain.span) {
                out.calls.push(site(
                    chain,
                    opts_method(&first.args),
                    first.args.first(),
                    configured_opts(&["hooks", "agent"], &first.args),
                ));
                continue;
            }
            if is_lib(&ky, &first.name, chain.span) {
                out.calls.push(site(
                    chain,
                    opts_method(&first.args),
                    first.args.first(),
                    configured_opts(&["hooks"], &first.args),
                ));
                continue;
            }
            if is_lib(&sa, &first.name, chain.span) {
                let (method, url) = match first.args.as_slice() {
                    [url] => (Some("GET".to_owned()), Some(url)),
                    [m, url] => (literal(m).map(|m| norm_method(&m)), Some(url)),
                    _ => (None, None),
                };
                if url.is_some() {
                    out.calls.push(site(chain, method, url, plugin(chain)));
                }
                continue;
            }
            if is_lib(&undici, &first.name, chain.span) {
                out.calls.push(site(
                    chain,
                    opts_method(&first.args),
                    first.args.first(),
                    configured_opts(&["dispatcher"], &first.args),
                ));
                continue;
            }
            // Destructured members (`const { get } = got`): capped.
            if let Some((object, bi)) = member_object(facts, chain, &first.name) {
                let at = facts.bindings[bi].span;
                let mut capped = |method: Option<String>, url: Option<&Arg>| {
                    out.calls.push(site(chain, method, url, true));
                };
                if (is_lib_or_module(&got, &["got"], &object, at)
                    || is_lib_or_module(&ky, &["ky", "ky-universal"], &object, at))
                    && GOT_KY_VERBS.contains(&first.name.as_str())
                    || is_lib_or_module(&sa, &["superagent"], &object, at)
                        && SUPERAGENT_VERBS.contains(&first.name.as_str())
                {
                    capped(verb_method(&first.name), first.args.first());
                } else if is_lib_or_module(&undici, &["undici"], &object, at)
                    && first.name == "request"
                {
                    capped(opts_method(&first.args), first.args.first());
                } else if let Some(carrier) = facts
                    .binding_index_at(&object, at)
                    .and_then(|oi| carriers.get(&oi))
                    && carrier.verbs().contains(&first.name.as_str())
                {
                    capped(verb_method(&first.name), first.args.first());
                }
            }
            continue;
        }
        let root = chain.root.as_deref().unwrap_or_default();
        if is_lib(&got, root, chain.span) && GOT_KY_VERBS.contains(&first.name.as_str()) {
            out.calls.push(site(
                chain,
                verb_method(&first.name),
                first.args.first(),
                configured_opts(&["hooks", "agent"], &first.args),
            ));
            continue;
        }
        if is_lib(&ky, root, chain.span) && GOT_KY_VERBS.contains(&first.name.as_str()) {
            out.calls.push(site(
                chain,
                verb_method(&first.name),
                first.args.first(),
                configured_opts(&["hooks"], &first.args),
            ));
            continue;
        }
        if is_lib(&sa, root, chain.span) && SUPERAGENT_VERBS.contains(&first.name.as_str()) {
            out.calls.push(site(
                chain,
                verb_method(&first.name),
                first.args.first(),
                plugin(chain),
            ));
            continue;
        }
        if is_lib(&undici, root, chain.span) && first.name == "request" && !first.args.is_empty() {
            out.calls.push(site(
                chain,
                opts_method(&first.args),
                first.args.first(),
                false,
            ));
            continue;
        }
        // Calls on a configured carrier: capped.
        if let Some(carrier) = facts
            .binding_index_at(root, chain.span)
            .and_then(|bi| carriers.get(&bi))
            && carrier.verbs().contains(&first.name.as_str())
        {
            out.calls.push(site(
                chain,
                verb_method(&first.name),
                first.args.first(),
                true,
            ));
        }
    }
}

/// `requests` and `httpx` as fetch-class sites (contract M6d S7): module
/// verbs and `request`/`stream` dispatchers behind import gates, session
/// receivers bound directly to `Session`/`Client`/`AsyncClient`
/// constructions, and anonymous inline instances — all with the same verb
/// allow-list on both libraries. `base_url` folds as the base; every
/// carrier site caps at `heuristic` (fine-grained carrier verification is
/// S8). `send`, `build_request`, `mount` and other non-verb members are no
/// sites; `urllib3`/`aiohttp` stay out (residue).
fn requests_httpx(facts: &FileScope<'_>, out: &mut FileClients) {
    /// Verbs of both libraries (`request`, and httpx's `stream`, dispatch).
    const VERBS: &[&str] = &["get", "options", "head", "post", "put", "patch", "delete"];
    /// Plain per-call keyword arguments: anything else, or a spread, may
    /// reroute and caps (evaluator-pass-1 parity). `requests` hooks are
    /// response-only, so they stay plain; `httpx` request hooks rewrite.
    const REQUESTS_CALL_KEYS: &[&str] = &[
        "params",
        "data",
        "json",
        "files",
        "headers",
        "cookies",
        "auth",
        "timeout",
        "allow_redirects",
        "proxies",
        "hooks",
        "stream",
        "verify",
        "cert",
    ];
    const HTTPX_CALL_KEYS: &[&str] = &[
        "params",
        "content",
        "data",
        "files",
        "json",
        "headers",
        "cookies",
        "auth",
        "proxy",
        "timeout",
        "follow_redirects",
        "verify",
    ];
    // Local names for the two modules: an alias, or the top package (so
    // `import requests.api` still roots `requests`), plus the submodule
    // paths calls are written through. From-imported verbs/dispatchers and
    // constructors keep their own maps: a bare `get(…)` is a response, never
    // a session.
    let mut roots: HashMap<String, bool> = HashMap::new();
    // From-imported verbs/dispatchers: local name to (is-httpx, name as
    // imported). Constructors keep their own map: a bare `get(…)` is a
    // response, never a session.
    let mut verbs: HashMap<String, (bool, String)> = HashMap::new();
    let mut ctors: HashMap<String, bool> = HashMap::new();
    for i in &facts.imports {
        let requests = i.module == "requests" || i.module.starts_with("requests.");
        let httpx = i.module == "httpx" || i.module.starts_with("httpx.");
        if !requests && !httpx {
            continue;
        }
        if let Some(alias) = i.alias.as_deref() {
            roots.insert(alias.to_owned(), httpx);
        } else {
            roots.insert(
                i.module.split('.').next().unwrap_or(&i.module).to_owned(),
                httpx,
            );
        }
        for n in &i.names {
            if VERBS.contains(&n.name.as_str())
                || n.name == "request"
                || (httpx && n.name == "stream")
            {
                verbs.insert(n.local.clone(), (httpx, n.name.clone()));
            } else if (!httpx && n.name == "Session")
                || (httpx && (n.name == "Client" || n.name == "AsyncClient"))
            {
                ctors.insert(n.local.clone(), httpx);
            }
        }
    }
    // `name` at `at` still names its import: no shadowing binding in effect.
    let unshadowed = |name: &str, at: Span| facts.binding_index_at(name, at).is_none();
    // The module a call roots at, looking through the `api`/`sessions`
    // submodule paths calls are written through (so `rq.api.get` counts
    // under `import requests as rq` too).
    let mod_root = |name: &str| {
        let bare = name
            .strip_suffix(".api")
            .or_else(|| name.strip_suffix(".sessions"))
            .unwrap_or(name);
        roots.get(bare).or_else(|| roots.get(name))
    };
    let is_root = |httpx: bool, name: &str, at: Span| {
        mod_root(name).is_some_and(|h| *h == httpx) && unshadowed(name, at)
    };
    // The first `url`-shaped argument: an explicit `url` keyword, else the
    // first positional (kwargs read as single-entry objects, so any
    // `Arg::Object` in the way is a keyword). `None` when neither names one.
    let url_arg = |args: &[Arg]| {
        for a in args {
            if let Arg::Object { entries, .. } = a {
                if let Some(e) = entries.iter().find(|e| e.key == "url") {
                    return Some(e.value.clone());
                }
            } else {
                return Some(a.clone());
            }
        }
        None
    };
    // Whether the call's keywords may reroute: any key outside the plain
    // list (a spread reads as `...`, which is outside every list). `url`
    // and `method` never count: the dispatcher's native slots, inert on a
    // verb call.
    let configured = |allowed: &[&str], args: &[Arg]| {
        args.iter()
            .flat_map(|a| match a {
                Arg::Object { entries, .. } => entries.iter().collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .any(|e| e.key != "url" && e.key != "method" && !allowed.contains(&e.key.as_str()))
    };
    let site = |span: Span,
                enclosing: Option<usize>,
                method: Option<String>,
                url: UrlValue,
                base: Option<UrlValue>,
                capped: bool| {
        ClientCall {
            kind: ClientKind::Fetch,
            method,
            url,
            base,
            instance: None,
            span,
            source: enclosing,
            injected: None,
            base_verified: capped.then_some(false),
        }
    };
    // A dispatcher method (`request(method, url)`, `stream(method, url)`):
    // a literal method, else unknown (the fold may still read a name).
    let dispatch_method = |args: &[Arg]| {
        args.first()
            .and_then(literal)
            .map(|m| norm_method(&m))
            .or_else(|| {
                kwarg(args, "method")
                    .and_then(literal)
                    .map(|m| norm_method(&m))
            })
    };
    // Whether `(root, link)` constructs a session, and for which library:
    // `Session`/`Client`/`AsyncClient` on their module, or a from-imported
    // constructor alone.
    let ctor_of = |root: Option<&str>, link: &CallLink, at: Span| -> Option<bool> {
        match (root, link.name.as_str()) {
            (Some(r), "Session") if is_root(false, r, at) => Some(false),
            (Some(r), "Client" | "AsyncClient") if is_root(true, r, at) => Some(true),
            (None, name) if unshadowed(name, at) => ctors.get(name).copied(),
            _ => None,
        }
    };
    // A session receiver: a non-member name bound directly to a session
    // construction, with the base its `base_url` states (requests sessions
    // take none). Factories wait for no phase (dynamic); member receivers
    // resolve below.
    let session = |name: &str, at: Span| -> Option<(bool, Option<UrlValue>)> {
        let bi = facts.binding_index_at(name, at)?;
        let b = &facts.bindings[bi];
        if b.member {
            return None;
        }
        let chain = init_chain(facts, b)?;
        let (root, [link]) = (chain.root.as_deref(), chain.links.as_slice()) else {
            return None;
        };
        let httpx = ctor_of(root, link, b.span)?;
        let base = httpx
            .then(|| kwarg(&link.args, "base_url").map(|a| UrlValue::of(Some(a))))
            .flatten();
        Some((httpx, base))
    };
    // Session-typed names: dotted constructor paths under an imported
    // module, or bare from-imported constructor locals.
    let session_type = |t: &str| -> Option<bool> {
        match t {
            "requests.Session" if roots.values().any(|h| !h) => Some(false),
            "httpx.Client" | "httpx.AsyncClient" if roots.values().any(|h| *h) => Some(true),
            name => ctors.get(name).copied(),
        }
    };
    // A local name carrying a session type but no construction (an
    // injected client). A construction decides first, so this never doubles
    // the session above.
    let typed_local = |name: &str, at: Span| -> Option<bool> {
        let bi = facts.binding_index_at(name, at)?;
        let b = &facts.bindings[bi];
        if b.member || b.init.is_some() {
            return None;
        }
        std::iter::once(&b.type_name)
            .chain(b.type_args.iter())
            .find_map(|t| session_type(t))
    };
    // Member sessions by (class scope, member name): construction wins over
    // a bare annotation, anything else conflicting goes dynamic (an
    // annotation never runs, so it cannot overwrite; two constructions may
    // run in either order).
    #[derive(Clone, PartialEq)]
    enum MemberRes {
        Dynamic,
        Annot(bool),
        Made(bool, Option<UrlValue>),
    }
    let resolve_member = |b: &Binding| -> MemberRes {
        use MemberRes::{Annot, Dynamic, Made};
        match &b.init {
            Some(Init::Call { .. }) => {
                let chain = init_chain(facts, b);
                match chain.map(|c| (c.root.as_deref(), c.links.as_slice())) {
                    Some((root, [link])) => match ctor_of(root, link, b.span) {
                        Some(httpx) => {
                            let base = httpx
                                .then(|| {
                                    kwarg(&link.args, "base_url").map(|a| UrlValue::of(Some(a)))
                                })
                                .flatten();
                            Made(httpx, base)
                        }
                        None => Dynamic,
                    },
                    _ => Dynamic,
                }
            }
            _ => {
                if b.init.is_some() {
                    return Dynamic;
                }
                match std::iter::once(&b.type_name)
                    .chain(b.type_args.iter())
                    .find_map(|t| session_type(t))
                {
                    Some(httpx) => Annot(httpx),
                    None => Dynamic,
                }
            }
        }
    };
    fn merge_member(a: MemberRes, b: MemberRes) -> MemberRes {
        use MemberRes::{Annot, Dynamic, Made};
        match (a, b) {
            (Dynamic, _) | (_, Dynamic) => Dynamic,
            (Annot(x), Annot(y)) if x == y => Annot(x),
            (Annot(_), m @ Made(..)) | (m @ Made(..), Annot(_)) => m,
            (x, y) if x == y => x,
            _ => Dynamic,
        }
    }
    let mut member_sessions: HashMap<(usize, String), MemberRes> = HashMap::new();
    for b in facts.bindings.iter() {
        // Class-body bindings (member writes and bare annotations) by their
        // class; method locals live under function scopes and never join.
        let Some(scope) = b.scope else {
            continue;
        };
        if !matches!(
            facts.facts.definitions.get(scope).map(|d| d.kind),
            Some(DefKind::Class)
        ) {
            continue;
        }
        let resolved = resolve_member(b);
        member_sessions
            .entry((scope, b.name.clone()))
            .and_modify(|slot| {
                *slot = merge_member(slot.clone(), resolved.clone());
            })
            .or_insert(resolved);
    }
    // A verb or dispatcher site on a known receiver: `url` from the verb's
    // first argument (kept unknown when unnamed), the dispatcher's second.
    // `None` when the shape names no URL (an empty call, a dispatcher
    // without one).
    let verb_site = |link: &CallLink,
                     span: Span,
                     enclosing: Option<usize>,
                     verb: &str,
                     httpx: bool,
                     base: Option<UrlValue>,
                     capped: bool|
     -> Option<ClientCall> {
        if VERBS.contains(&verb) {
            if link.args.is_empty() {
                return None;
            }
            let url = url_arg(&link.args)
                .map(|a| UrlValue::of(Some(&a)))
                .unwrap_or(UrlValue::Unknown);
            Some(site(
                span,
                enclosing,
                Some(norm_method(verb)),
                url,
                base,
                capped
                    || configured(
                        if httpx {
                            HTTPX_CALL_KEYS
                        } else {
                            REQUESTS_CALL_KEYS
                        },
                        &link.args,
                    ),
            ))
        } else if verb == "request" || (httpx && verb == "stream") {
            // A positional URL, else a `url` keyword (a keyword `Object` in
            // the slot never reads as the URL); anything else unnamed keeps
            // an Unknown site, as for `fetch`.
            let url = match link.args.get(1) {
                Some(Arg::Template { .. } | Arg::Name { .. }) => link.args.get(1).cloned(),
                Some(_) => kwarg(&link.args, "url")
                    .cloned()
                    .or_else(|| link.args.get(1).cloned()),
                None => kwarg(&link.args, "url").cloned(),
            }?;
            Some(site(
                span,
                enclosing,
                dispatch_method(&link.args),
                UrlValue::of(Some(&url)),
                base,
                capped
                    || configured(
                        if httpx {
                            HTTPX_CALL_KEYS
                        } else {
                            REQUESTS_CALL_KEYS
                        },
                        &link.args,
                    ),
            ))
        } else {
            None
        }
    };
    for chain in &facts.chains {
        let Some(first) = chain.links.first() else {
            continue;
        };
        // An anonymous inline instance (`Session().post(…)`,
        // `Client(base_url=…).get(…)`): the verb is the second link, capped.
        if let [ctor, verb] = chain.links.as_slice()
            && let Some(httpx) = ctor_of(chain.root.as_deref(), ctor, chain.span)
            && let Some(call) = verb_site(
                verb,
                chain.span,
                chain.enclosing,
                &verb.name,
                httpx,
                httpx
                    .then(|| kwarg(&ctor.args, "base_url").map(|a| UrlValue::of(Some(a))))
                    .flatten(),
                true,
            )
        {
            out.calls.push(call);
            continue;
        }
        if chain.root.is_none() {
            // A from-imported verb or dispatcher, unshadowed.
            if unshadowed(&first.name, chain.span)
                && let Some((httpx, verb)) = verbs.get(&first.name)
                && let Some(call) = verb_site(
                    first,
                    chain.span,
                    chain.enclosing,
                    verb,
                    *httpx,
                    None,
                    false,
                )
            {
                out.calls.push(call);
            }
            continue;
        }
        let root = chain.root.as_deref().unwrap_or_default();
        // A module verb or dispatcher.
        for httpx in [false, true] {
            if is_root(httpx, root, chain.span)
                && let Some(call) = verb_site(
                    first,
                    chain.span,
                    chain.enclosing,
                    &first.name,
                    httpx,
                    None,
                    false,
                )
            {
                out.calls.push(call);
            }
        }
        // A session receiver: capped.
        if let Some((httpx, base)) = session(root, chain.span)
            && let Some(call) = verb_site(
                first,
                chain.span,
                chain.enclosing,
                &first.name,
                httpx,
                base,
                true,
            )
        {
            out.calls.push(call);
        }
        // A `self.`/`cls.` member receiver, through the enclosing class:
        // capped. Deeper paths stay dynamic.
        if let Some((obj, member)) = root.split_once('.')
            && (obj == "self" || obj == "cls")
            && !member.contains('.')
            && let Some(class) = {
                let mut scope = chain.enclosing;
                loop {
                    match scope {
                        Some(d) if facts.facts.definitions[d].kind == DefKind::Class => {
                            break Some(d);
                        }
                        Some(d) => scope = facts.facts.definitions[d].parent,
                        None => break None,
                    }
                }
            }
            && let Some(res) = member_sessions.get(&(class, member.to_owned()))
            && let (httpx, base) = match res {
                MemberRes::Made(httpx, base) => (*httpx, base.clone()),
                MemberRes::Annot(httpx) => (*httpx, None),
                MemberRes::Dynamic => continue,
            }
            && let Some(call) = verb_site(
                first,
                chain.span,
                chain.enclosing,
                &first.name,
                httpx,
                base,
                true,
            )
        {
            out.calls.push(call);
        }
        // A local name carrying a session type but no construction (an
        // injected client): capped. A construction decides first, so this
        // never doubles the session above.
        if !root.contains('.')
            && let Some(httpx) = typed_local(root, chain.span)
            && let Some(call) = verb_site(
                first,
                chain.span,
                chain.enclosing,
                &first.name,
                httpx,
                None,
                true,
            )
        {
            out.calls.push(call);
        }
    }
}
/// Angular `HttpClient` calls (contract M6b S9, D8): verbs and
/// `request(method, url)` on a receiver typed `HttpClient` — a constructor
/// parameter property, field or local — with the `@angular/common/http`
/// import present. Every site is capped at `heuristic`: repository providers
/// and interceptors are a global hook, and per-instance proof is later scope.
fn http_client(facts: &FileScope<'_>, out: &mut FileClients) {
    if !facts.imports.iter().any(|i| {
        is_module(&i.module, "@angular/common/http")
            && i.names.iter().any(|n| n.name == "HttpClient")
    }) {
        return;
    }
    // Whether the receiver is typed `HttpClient`: a member of the call's class
    // or a local in effect at the call.
    let typed = |root: &str, chain: &CallChain| {
        if let Some(member) = root.strip_prefix("this.") {
            if member.contains('.') {
                return false;
            }
            facts.members(member).iter().any(|&bi| {
                let b = &facts.bindings[bi];
                b.type_name == "HttpClient" && {
                    let mut enclosing = chain.enclosing;
                    while let Some(d) = enclosing {
                        if Some(d) == b.scope {
                            return true;
                        }
                        enclosing = facts.definitions[d].parent;
                    }
                    b.scope.is_none()
                }
            })
        } else if root.contains('.') {
            false
        } else {
            facts
                .binding_index_at(root, chain.span)
                .is_some_and(|bi| facts.bindings[bi].type_name == "HttpClient")
        }
    };
    for chain in &facts.chains {
        let Some(first) = chain.links.first() else {
            continue;
        };
        let Some(root) = chain.root.as_deref() else {
            continue;
        };
        if !typed(root, chain) || first.args.is_empty() {
            continue;
        }
        let (method, url) = if first.name == "request" {
            match first.args.as_slice() {
                [m, url, ..] => (literal(m).map(|m| norm_method(&m)), Some(url)),
                _ => continue,
            }
        } else if AXIOS_VERBS.contains(&first.name.as_str()) {
            (Some(norm_method(&first.name)), first.args.first())
        } else {
            continue;
        };
        out.calls.push(ClientCall {
            kind: ClientKind::Fetch,
            method,
            url: UrlValue::of(url),
            base: None,
            instance: None,
            span: chain.span,
            source: chain.enclosing,
            injected: None,
            base_verified: Some(false),
        });
    }
}

// ---------------------------------------------------------------------------------------------
// axios (S9)

const AXIOS_VERBS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];

/// Local names of a module's exports `exported` (`default` for a default import), plus its
/// namespace import when `namespace`, or a `require(module)` at any scope (evaluator pass 6,
/// V4), the module's subpaths included (`axios/dist/node/axios.cjs`); plus the indices of
/// those `require` bindings.
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
        .filter(|i| is_module(&i.module, module))
        .flat_map(|i| {
            i.alias.iter().filter(|_| namespace).cloned().chain(
                i.names
                    .iter()
                    .filter(|n| exported.contains(&n.name.as_str()))
                    .map(|n| n.local.clone()),
            )
        })
        .collect();
    for (i, b) in facts.bindings.iter().enumerate().filter(|(_, b)| !b.member) {
        if let Some(c) = init_chain(facts, b)
            && c.root.is_none()
            && c.links.len() == 1
            && c.links[0].name == "require"
            && c.links[0]
                .args
                .first()
                .and_then(literal)
                .is_some_and(|m| is_module(&m, module))
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

/// A call to an axios member outside the module object (contract M6b S9): a
/// destructured verb (`const { get } = axios`, `const { get } =
/// require("axios")`, `const get = axios.get`, including from a local
/// instance) or an inline `require("axios")` chain. Capped: no Gate-A proof
/// for the destructured carrier in S9 scope.
fn axios_member(
    facts: &FileScope<'_>,
    chain: &CallChain,
    link: &rb_syntax::CallLink,
    names: &[String],
    requires: &HashSet<usize>,
    instances: &HashMap<usize, (Option<UrlValue>, bool)>,
) -> Option<ClientCall> {
    // An inline `require("axios")` chain: `require("axios").get(…)`.
    if link.name == "require"
        && let Some(next) = chain.links.get(1)
        && let Some(m) = link.args.first().and_then(literal)
        && is_module(&m, "axios")
        && let Some(mut call) = axios_request(facts, chain, next, None, None)
    {
        call.base_verified = Some(false);
        return Some(call);
    }
    if !AXIOS_VERBS.contains(&link.name.as_str()) && link.name != "request" {
        return None;
    }
    let is_axios_at = |name: &str, at: Span| {
        names.iter().any(|n| n == name)
            && facts
                .binding_index_at(name, at)
                .is_none_or(|bi| requires.contains(&bi))
            // The module itself (`const { get } = require("axios")`, with no
            // import); a shadowed module name resolves to its binding, never
            // the module.
            || is_module(name, "axios") && facts.binding_index_at(name, at).is_none()
    };
    let bound = facts.binding_index_at(&link.name, chain.span)?;
    let b = &facts.bindings[bound];
    // The axios object the name destructures, shadow-checked where it is
    // read: the module (capped) or a local instance (its base and verdict).
    let (base, carrier): (Option<UrlValue>, Option<(bool, bool)>) = match &b.init {
        Some(Init::Member { object, .. }) if is_axios_at(object, b.span) => (None, None),
        Some(Init::Member { object, .. }) => match facts
            .binding_index_at(object, b.span)
            .and_then(|bi| instances.get(&bi))
        {
            Some((base, verified)) => (base.clone(), Some((*verified, base.is_some()))),
            None => return None,
        },
        Some(Init::Name { text, .. }) => match text.rsplit_once('.') {
            Some((object, _)) if is_axios_at(object, b.span) => (None, None),
            _ => return None,
        },
        _ => return None,
    };
    let mut call = axios_request(facts, chain, link, base, None)?;
    if let Some((plain, from_carrier)) = carrier {
        call.base_verified = verdict(plain && call.base_verified.is_none(), from_carrier);
    } else {
        call.base_verified = Some(false);
    }
    Some(call)
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
            let base = create_base(facts, link.args.first());
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
        .filter(|i| {
            !is_module(&i.module, "axios")
                // Fetch-class modules resolve through `fetch_class`, not the
                // axios imported-instance fallback (contract M6b S9: no
                // axios-instance double).
                && !FETCH_CLASS_MODULES
                    .iter()
                    .any(|m| is_module(&i.module, m))
        })
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
                // A config passed by name reads the object it names
                // (contract M6b S5).
                let resolved;
                let call = match first.args.first() {
                    Some(config @ Arg::Object { .. }) => {
                        config_call(facts, chain, None, None, config)
                    }
                    Some(Arg::Name { span, .. }) => {
                        resolved =
                            config_entries(facts, first.args.first()).map(|entries| Arg::Object {
                                span: *span,
                                entries: entries.to_vec(),
                            });
                        match resolved.as_ref() {
                            Some(config) => config_call(facts, chain, None, None, config),
                            None => axios_call(
                                facts,
                                chain,
                                None,
                                None,
                                first.args.first(),
                                None,
                                first.args.get(1),
                            ),
                        }
                    }
                    url => axios_call(facts, chain, None, None, url, None, first.args.get(1)),
                };
                out.calls.push(call);
            } else if let Some(call) =
                axios_member(facts, chain, first, &names, &requires, &instances)
            {
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
            let base = create_base(facts, first.args.first());
            let from_carrier = base.is_some();
            if let Some(mut call) = axios_request(facts, chain, link, base, None) {
                call.base_verified = verdict(plain && call.base_verified.is_none(), from_carrier);
                out.calls.push(call);
            }
            continue;
        }
        // Calls through the namespace's `default` member (`axios.default.get(…)`,
        // contract M6b S9): the module object under interop, capped.
        if let Some(default) = root.strip_suffix(".default")
            && is_axios(default, chain.span)
            && let Some(mut call) = axios_request(facts, chain, first, None, None)
        {
            call.base_verified = Some(false);
            out.calls.push(call);
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
        if let Some(mut call) = axios_request(facts, chain, first, base, instance) {
            if let Some((plain, from_carrier)) = carrier {
                call.base_verified = verdict(plain && call.base_verified.is_none(), from_carrier);
            }
            out.calls.push(call);
        }
    }
}

/// A request call `link` of `chain` on an instance with `base` (or on the module): the verbs
/// with their URL and request config, and `request(config)`.
fn axios_request(
    facts: &FileScope<'_>,
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
            facts,
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
    // A config passed by name reads the object it names (contract M6b S5).
    let resolved;
    Some(match link.args.first() {
        Some(config @ Arg::Object { .. }) => config_call(facts, chain, base, instance, config),
        Some(Arg::Name { span, .. }) => {
            resolved = config_entries(facts, link.args.first()).map(|entries| Arg::Object {
                span: *span,
                entries: entries.to_vec(),
            });
            match resolved.as_ref() {
                Some(config) => config_call(facts, chain, base, instance, config),
                None => unknown_request(facts, chain, base, instance),
            }
        }
        // A config that cannot be read: no URL, method or base, and not plain.
        _ => unknown_request(facts, chain, base, instance),
    })
}

/// `request(config)` with a config that cannot be read: no URL, method or
/// base, and not plain.
fn unknown_request(
    facts: &FileScope<'_>,
    chain: &CallChain,
    base: Option<UrlValue>,
    instance: Option<String>,
) -> ClientCall {
    ClientCall {
        method: None,
        url: UrlValue::Unknown,
        base: Some(UrlValue::Unknown),
        base_verified: Some(false),
        ..axios_call(facts, chain, base, instance, None, None, None)
    }
}

/// The base an `axios.create(config)` states. An inline object or one named
/// by a same-file binding reads its last `baseURL`, contested by a later
/// spread as in [`config_base`]; a name that is not an object here (a URL
/// string, a parameter) is kept for the indexer, which folds it; anything
/// else states no base. A spread anywhere in the config makes the
/// instance's carrier not plain ([`plain_config`]; spreads are never
/// folded), so a spread after `baseURL` that may replace it leaves the base
/// unverified.
fn create_base(facts: &FileScope<'_>, config: Option<&Arg>) -> Option<UrlValue> {
    if let Some(entries) = config_entries(facts, config) {
        return match entries
            .iter()
            .rev()
            .find(|e| e.key == "baseURL" || e.key == "...")
        {
            Some(e) if e.key == "baseURL" => Some(UrlValue::of(Some(&e.value))),
            Some(_) => {
                let mut candidates = Vec::new();
                object_base_candidates(facts, entries, &mut HashSet::new(), &mut candidates);
                Some(if candidates.is_empty() {
                    UrlValue::Unknown
                } else {
                    UrlValue::Contested(candidates)
                })
            }
            None => None,
        };
    }
    match config? {
        Arg::Name { text, span } => Some(UrlValue::Name {
            text: text.clone(),
            span: *span,
        }),
        _ => None,
    }
}

/// The entries of a config object: inline, or named by a same-file object
/// binding (contract M6b S5). Anything else (a parameter, an import, a call)
/// is unreadable here; spreads it cannot read are named for the indexer by
/// [`object_base_candidates`].
fn config_entries<'a>(
    facts: &'a FileScope<'_>,
    config: Option<&'a Arg>,
) -> Option<&'a [ObjectEntry]> {
    match config? {
        Arg::Object { entries, .. } => Some(entries),
        Arg::Name { text, span } => match facts.binding_index_at(text, *span) {
            Some(bi) => match &facts.bindings[bi].init {
                Some(Init::Object { entries }) => Some(entries),
                _ => None,
            },
            None => None,
        },
        _ => None,
    }
}

/// The base a request config states (contract M4a amendment 12, Q1): `None` when the call has
/// no config, or a config object without a `baseURL` after its last spread, so the instance's
/// base applies; otherwise the stated base, [`UrlValue::Unknown`] when it cannot be read (a
/// config that is not an object literal, a `baseURL` that is not a string or a name), and
/// [`UrlValue::Contested`] when a spread may set it and the file states values it may take
/// (contract M5 S19 (f): [`object_base_candidates`]). A config named by a same-file object
/// binding reads its entries (contract M6b S5).
fn config_base(facts: &FileScope<'_>, config: Option<&Arg>) -> Option<UrlValue> {
    let config = config?;
    let Some(entries) = config_entries(facts, Some(config)) else {
        return Some(UrlValue::Unknown);
    };
    match entries
        .iter()
        .rev()
        .find(|e| e.key == "baseURL" || e.key == "...")
    {
        Some(e) if e.key == "baseURL" => Some(UrlValue::of(Some(&e.value))),
        Some(_) => {
            let mut candidates = Vec::new();
            object_base_candidates(facts, entries, &mut HashSet::new(), &mut candidates);
            Some(if candidates.is_empty() {
                UrlValue::Unknown
            } else {
                UrlValue::Contested(candidates)
            })
        }
        None => None,
    }
}

/// How many spread bindings [`object_base_candidates`] follows for one config.
pub(crate) const MAX_SPREADS: usize = 4;

/// The `baseURL` values an object literal's entries may leave in effect (contract M5 S19 (f)),
/// added to `out` in source order without repeats: from the last `baseURL` entry on, that
/// entry's value and, for each later spread naming a binding initialised with an object literal
/// (`const stripeOpts = { baseURL: … }`), the values of that object, followed the same way
/// through its own spreads (a visited set and [`MAX_SPREADS`] bound the walk). A spread of
/// anything else names its value (contract M6b S5): a parameter, an import or an unresolved
/// name stays a name for the indexer, which folds it through its unique declaration. Only
/// readable values (a string or a name) are candidates.
fn object_base_candidates(
    facts: &FileScope<'_>,
    entries: &[rb_syntax::ObjectEntry],
    seen: &mut HashSet<usize>,
    out: &mut Vec<UrlValue>,
) {
    let from = entries
        .iter()
        .rposition(|e| e.key == "baseURL")
        .unwrap_or(0);
    for e in &entries[from..] {
        if e.key == "baseURL" {
            let value = UrlValue::of(Some(&e.value));
            if value != UrlValue::Unknown && !out.iter().any(|v| same_value(v, &value)) {
                out.push(value);
            }
            continue;
        }
        if e.key != "..." || seen.len() >= MAX_SPREADS {
            continue;
        }
        let Arg::Name { text, span } = &e.value else {
            continue;
        };
        let Some(bi) = facts.binding_index_at(text, *span) else {
            // Not bound here (an import, a parameter): name it for the
            // indexer, which folds it through its unique declaration.
            let value = UrlValue::Name {
                text: text.clone(),
                span: *span,
            };
            if !out.iter().any(|v| same_value(v, &value)) {
                out.push(value);
            }
            continue;
        };
        if let Some(Init::Object { entries }) = &facts.bindings[bi].init
            && seen.insert(bi)
        {
            object_base_candidates(facts, entries, seen, out);
        } else if facts.bindings[bi].init.is_none()
            && !out
                .iter()
                .any(|v| matches!(v, UrlValue::Name { text: t, .. } if t == text))
        {
            // Bound but unreadable here (a parameter): same, name it.
            out.push(UrlValue::Name {
                text: text.clone(),
                span: *span,
            });
        }
    }
}

/// A config passed by name that names nothing readable here and no string
/// (contract M6b S5): a parameter, an import, or anything not an object or a
/// string literal. Its call is dynamic (below), never suffix-matched on its
/// tail; a name that folds to a URL keeps today's handling.
fn unreadable_config(facts: &FileScope<'_>, config: Option<&Arg>) -> bool {
    let Some(Arg::Name { text, span }) = config else {
        return false;
    };
    if config_entries(facts, config).is_some() {
        return false;
    }
    !matches!(
        facts
            .binding_index_at(text, *span)
            .and_then(|bi| facts.bindings[bi].init.as_ref()),
        Some(Init::Template { .. })
    )
}

/// An axios call with its URL argument (`url`), its method (`None`: from the config, default
/// GET) and its request config: a base the config states replaces the instance's `base`.
/// A config passed by name that names nothing readable makes the call dynamic, whatever the
/// URL (contract M6b S5).
fn axios_call(
    facts: &FileScope<'_>,
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
    // A config passed by name reads the object it names (contract M6b S5):
    // its entries decide the base and Gate A plainness below.
    let resolved;
    let config = match config {
        Some(Arg::Name { span, .. }) => match config_entries(facts, config) {
            Some(entries) => {
                resolved = Arg::Object {
                    span: *span,
                    entries: entries.to_vec(),
                };
                Some(&resolved)
            }
            None => config,
        },
        _ => config,
    };
    let url = if unreadable_config(facts, config) {
        None
    } else {
        url
    };
    ClientCall {
        kind: ClientKind::Axios,
        method,
        url: UrlValue::of(url),
        base: config_base(facts, config).or(base),
        instance,
        span: chain.span,
        source: chain.enclosing,
        injected: None,
        // A request config that is not plain (an `adapter`, a spread, a variable) may send the
        // request elsewhere whatever carries it (amendment 15).
        base_verified: (!plain_config(config)).then_some(false),
    }
}

/// `axios(config)` or `request(config)` with an object literal: the URL comes from its `url`.
fn config_call(
    facts: &FileScope<'_>,
    chain: &CallChain,
    base: Option<UrlValue>,
    instance: Option<String>,
    config: &Arg,
) -> ClientCall {
    axios_call(
        facts,
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
                    && chain_kind(facts, chain) == Some(ClientKind::RestTemplate)
            }
        };
        if !is_client || first.args.is_empty() {
            continue;
        }
        // The root URI configured on the instance; a relative URL needs one (amendment 12).
        // The carrier is the instance (Gate A), or an inline construction, plain only as a bare
        // `new RestTemplate()` (amendment 16).
        let base = bases.of(receiver);
        let plain = match receiver {
            Some(bi) => bases.carrier(bi),
            None => plain_construction(facts, chain, 0),
        };
        out.calls.push(ClientCall {
            kind: ClientKind::RestTemplate,
            method,
            url: java_url(facts, first.args.first()),
            base_verified: verdict(plain, base.read),
            base: Some(base.value),
            instance: None,
            span: chain.span,
            source: chain.enclosing,
            injected: receiver.filter(|_| base.injected),
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
    if fresh_root(facts, chain).is_some() {
        BaseFact::Fresh
    } else {
        BaseFact::Unreadable
    }
}

/// The links after a fresh construction ([`fresh_root`]) that leave its URI building to
/// `baseUrl`/`rootUri` alone (Gate A, contract M4a amendments 14 and 16): the base itself,
/// `build`, and settings no URI goes through. Anything else (`uriBuilderFactory`,
/// `uriTemplateHandler`, `defaultUriVariables`, filters, interceptors, customizers, `apply`, a
/// connector or request factory, a second `create`/`builder`) may change where a request goes.
const PLAIN_LINKS: &[&str] = &[
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

/// A construction chain whose base Gate A can certify: it starts from a fresh construction
/// ([`fresh_root`]; not from a builder or client binding, whose state is not read) and its
/// links after it and before `upto` are all [`PLAIN_LINKS`].
fn plain_construction(facts: &FileScope<'_>, chain: &CallChain, upto: usize) -> bool {
    fresh_root(facts, chain).is_some_and(|n| n <= upto && plain_links(&chain.links[n..upto]))
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

/// The fresh construction a chain starts from, by allow-list (contract M4a amendment 16,
/// evaluator pass 6, V1), with the number of its links that make it: `new RestTemplate()` or
/// `new RestTemplateBuilder()` with no argument and no class body (0), or a static
/// `WebClient`/`RestClient` `builder()`, `create()` or `create(<url>)` (1). Anything else (a
/// constructor argument such as a request factory or a customizer, an anonymous subclass,
/// `RestClient.create(restTemplate)`, `RestClient.builder(restTemplate)`) is `None`.
fn fresh_root(facts: &FileScope<'_>, chain: &CallChain) -> Option<usize> {
    let root = chain.root.as_deref()?;
    if let Some(created) = root.strip_prefix("new ") {
        // The root is the creation as written: `T()` and nothing after it.
        let (type_name, rest) = created.split_once('(')?;
        let simple = type_name.split('<').next().unwrap_or("").trim();
        let simple = simple.rsplit('.').next().unwrap_or(simple);
        let bare = rest.trim() == ")";
        return (bare && matches!(simple, "RestTemplate" | "RestTemplateBuilder")).then_some(0);
    }
    fluent_kind(facts, root)?;
    let first = chain.links.first()?;
    let fresh = match (first.name.as_str(), first.args.as_slice()) {
        ("builder" | "create", []) => true,
        ("create", [url]) => url_arg(facts, url),
        _ => false,
    };
    fresh.then_some(1)
}

/// An argument that can only be a URL: a string, or a name the file binds to a `String` (a
/// constant, a `@Value` field, a parameter). A name not resolved in the file may be a client.
fn url_arg(facts: &FileScope<'_>, arg: &Arg) -> bool {
    match arg {
        Arg::Template { .. } => true,
        Arg::Name { text, span } => {
            local_binding(facts, Language::Java, text, *span).is_some_and(|bi| {
                matches!(
                    facts.bindings[bi].type_name.as_str(),
                    "String" | "java.lang.String"
                )
            })
        }
        _ => false,
    }
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
/// or calling a URI template handler setter on it. With the file's [`Gate`].
struct SpringBases<'s, 'f> {
    facts: &'s FileScope<'f>,
    /// Binding → indices of the chains that configure it.
    configured: HashMap<usize, Vec<usize>>,
    gate: Gate<'f>,
}

/// A Spring client's base as [`SpringBases::of`] reads it.
struct SpringBase {
    value: UrlValue,
    /// Nothing in the file configures the instance: it is injected (contract M4b S21).
    injected: bool,
    /// The base was read from the carrier's construction (not unknown, not a `@Bean`'s).
    read: bool,
}

/// The `@Bean` methods of a file (an import-gated `@Bean` on a method).
fn bean_methods(facts: &FileFacts) -> HashSet<usize> {
    facts
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
        .collect()
}

/// The base the `@Bean` method `method` states for a `kind` client (contract M4b S21): the one
/// value every chain in it (or in a lambda inside it) that builds a `kind` client and sets a
/// base agrees on; `None` when it is no `@Bean` method, states none, or states several or an
/// unreadable one.
pub fn bean_base(facts: &FileFacts, method: usize, kind: ClientKind) -> Option<UrlValue> {
    if !bean_methods(facts).contains(&method) {
        return None;
    }
    let scope = FileScope::new(facts);
    let mut stated = Vec::new();
    for chain in &facts.chains {
        // The chain's innermost enclosing `@Bean` method, through nested definitions.
        let mut def = chain.enclosing;
        let mut seen = 0;
        while let Some(d) = def.filter(|&d| d != method && seen < 64) {
            def = facts.definitions[d].parent;
            seen += 1;
        }
        if def != Some(method) || chain_kind(&scope, chain) != Some(kind) {
            continue;
        }
        if let base @ BaseFact::Stated(_) = chain_base(&scope, chain, chain.links.len()) {
            stated.push(base);
        }
    }
    if stated.is_empty() {
        return None;
    }
    agreed(&stated)
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
        let mut configured: HashMap<usize, Vec<usize>> = HashMap::new();
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
        }
        SpringBases {
            facts,
            configured,
            gate,
        }
    }

    /// Whether the binding `bi` is a provably plain carrier (Gate A, contract M4a amendments
    /// 14–16): it passes the [`Gate`] scan and its initialiser is a [`plain_construction`] or
    /// a bare `new RestTemplate()`. A chain that configures it later names it as an assignment
    /// target or a setter's receiver, which the scan already rejects (evaluator pass 6: a
    /// separate check of those chains was redundant, its mutant survived); a field, a
    /// parameter and an injected instance never pass the scan.
    fn carrier(&self, bi: usize) -> bool {
        let b = &self.facts.bindings[bi];
        // A binding with no initialiser has no `declaration` token or is written to, so the
        // scan below fails it.
        let init = match &b.init {
            Some(Init::Call { chain }) => Some(*chain),
            // `new RestTemplate()`: a new client with no links (an argument may be a request
            // factory, and a subclass may override anything, so neither is plain).
            Some(Init::New {
                type_name,
                args,
                body,
            }) => {
                if *args > 0 || *body || type_name.rsplit('.').next() != Some("RestTemplate") {
                    return false;
                }
                None
            }
            Some(_) => return false,
            None => None,
        };
        self.gate
            .unchanged(bi, spring_requests(self.facts, &b.type_name))
            && init.is_none_or(|ci| {
                self.facts
                    .chains
                    .get(ci)
                    .is_some_and(|c| plain_construction(self.facts, c, c.links.len()))
            })
    }

    /// The base of a client bound at `receiver`: one stated base when everything that
    /// configures the instance agrees, else unknown. An instance nothing in the file
    /// configures is injected: its base is left to the indexer, which reads the `@Bean` method
    /// its injection point resolves to (contract M4b S21; such an instance is never a plain
    /// carrier, so its base is never verified).
    ///
    /// Contract M6b S5: configuring statements are ordered by source position, later wins.
    /// A statement the file cannot read keeps the agreed-or-contested rule (its value could
    /// be anywhere). Any second configuring statement already fails the carrier scan, so a
    /// later-wins base never reaches `synthetic`.
    fn of(&self, receiver: Option<usize>) -> SpringBase {
        let unknown = SpringBase {
            value: UrlValue::Unknown,
            injected: false,
            read: false,
        };
        let Some(bi) = receiver else {
            return unknown;
        };
        let b = &self.facts.bindings[bi];
        // (source position, what was stated), the initialiser first.
        let mut stated: Vec<(Span, BaseFact)> = Vec::new();
        let mut unreadable = false;
        match &b.init {
            Some(Init::Call { chain }) => {
                if let Some(c) = self.facts.chains.get(*chain) {
                    match chain_base(self.facts, c, c.links.len()) {
                        fact @ (BaseFact::Stated(_) | BaseFact::Fresh) => {
                            stated.push((c.span, fact));
                        }
                        BaseFact::Unreadable => unreadable = true,
                    }
                }
            }
            // A constructor argument (a request factory, converters) sets no root URI.
            Some(Init::New { .. }) => stated.push((b.span, BaseFact::Fresh)),
            Some(_) => unreadable = true,
            None => {}
        }
        for &ci in self.configured.get(&bi).into_iter().flatten() {
            let c = &self.facts.chains[ci];
            match chain_base(self.facts, c, c.links.len()) {
                fact @ (BaseFact::Stated(_) | BaseFact::Fresh) => {
                    stated.push((c.span, fact));
                }
                BaseFact::Unreadable => unreadable = true,
            }
        }
        if stated.is_empty() && !unreadable {
            return SpringBase {
                injected: true,
                ..unknown
            };
        }
        if unreadable {
            // Several bases, or one that cannot be read: the stated ones are the candidates
            // (contract M5 S19 (f)).
            return self.contested(&stated, unknown);
        }
        // Every statement read: agreement as before, else the later statement wins.
        let values: Vec<&UrlValue> = stated
            .iter()
            .filter_map(|(_, f)| match f {
                BaseFact::Stated(v) => Some(v),
                BaseFact::Fresh | BaseFact::Unreadable => None,
            })
            .collect();
        let Some(first) = values.first() else {
            return SpringBase {
                value: UrlValue::Unknown,
                ..unknown
            };
        };
        if values.iter().all(|v| same_value(v, first)) {
            return SpringBase {
                value: (*first).clone(),
                injected: false,
                read: true,
            };
        }
        stated.sort_by_key(|(span, _)| (span[0], span[1], span[2], span[3]));
        let last = stated.iter().rev().find_map(|(_, f)| match f {
            BaseFact::Stated(v) if *v != UrlValue::Unknown => Some(v.clone()),
            _ => None,
        });
        match last {
            Some(value) => SpringBase {
                value,
                injected: false,
                read: true,
            },
            None => self.contested(&stated, unknown),
        }
    }

    /// The stated values as candidates (contract M5 S19 (f)): contested when
    /// they name anything readable, unknown otherwise.
    fn contested(&self, stated: &[(Span, BaseFact)], unknown: SpringBase) -> SpringBase {
        let mut candidates: Vec<UrlValue> = Vec::new();
        for (_, f) in stated {
            if let BaseFact::Stated(v) = f
                && *v != UrlValue::Unknown
                && !candidates.iter().any(|c| same_value(c, v))
            {
                candidates.push(v.clone());
            }
        }
        SpringBase {
            value: if candidates.is_empty() {
                UrlValue::Unknown
            } else {
                UrlValue::Contested(candidates)
            },
            ..unknown
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
pub(crate) fn same_value(a: &UrlValue, b: &UrlValue) -> bool {
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
                    injected: false,
                    read: true,
                },
                plain_construction(facts, chain, verb),
            ),
            BaseFact::Fresh => (
                SpringBase {
                    value: UrlValue::Unknown,
                    injected: false,
                    read: false,
                },
                plain_construction(facts, chain, verb),
            ),
            BaseFact::Unreadable => (
                bases.of(receiver),
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
            injected: receiver.filter(|_| base.injected),
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
                    injected: None,
                    // A declarative interface: no instance carries the request (amendment 15).
                    base_verified: None,
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
            name @ ("NewRequest" | "NewRequestWithContext") if go_new_request(&http, chain) => {
                if name == "NewRequest" {
                    (go_method(&link.args[0]), link.args.get(1))
                } else {
                    (go_method(&link.args[1]), link.args.get(2))
                }
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
            injected: None,
            // The package's functions and `http.DefaultClient` are the bare module client
            // (amendment 15); a `http.Client` binding is not provably plain (its `Transport`
            // may reroute), nor is a request built by `NewRequest`, which whatever `Do`es it
            // carries.
            base_verified: (!bare).then_some(false),
        });
    }
    // A request that never leaves the process is no client call (contract M6b
    // S4); the indexer buckets these spans `unmatched`, never edges.
    out.in_process = go_in_process(facts, &http, import_name("net/http/httptest").as_deref());
}

// ---------------------------------------------------------------------------------------------
// Go in-process requests (contract M6b S4)

/// How deep the sink walk follows a request through same-file helpers before
/// it stops (contract invariant 8: bounded work).
const MAX_SINK_DEPTH: usize = 8;

/// A `net/http` request construction: `NewRequest` (method, URL, body) or
/// `NewRequestWithContext` (context, method, URL, body) on the `net/http`
/// import. The same gate as the client sites in [`go_http`], so a marked
/// span is always a site.
fn go_new_request(http: &str, chain: &CallChain) -> bool {
    let (Some(root), [link]) = (chain.root.as_deref(), chain.links.as_slice()) else {
        return false;
    };
    root == http
        && match link.name.as_str() {
            "NewRequest" => link.args.len() >= 2,
            "NewRequestWithContext" => link.args.len() >= 3,
            _ => false,
        }
}

/// Whether `inner` lies within `outer` (start and end comparisons).
fn within(inner: Span, outer: Span) -> bool {
    (outer[0], outer[1]) <= (inner[0], inner[1]) && (inner[2], inner[3]) <= (outer[2], outer[3])
}

/// The bindings `binding_index_at` could resolve `name` to in `def`: the
/// same-named ones in the definition, its ancestors and the file scope.
/// Anything else is disjoint from the definition, so its extent can never
/// hold a site in it.
fn resolve_candidates(
    facts: &FileScope<'_>,
    by_scope: &HashMap<(Option<usize>, &str), Vec<usize>>,
    def: Option<usize>,
    name: &str,
) -> Vec<usize> {
    let mut out = Vec::new();
    let mut scope = def;
    loop {
        if let Some(found) = by_scope.get(&(scope, name)) {
            crate::work::tick(found.len());
            out.extend(found.iter().copied());
        }
        match scope {
            Some(d) => scope = facts.facts.definitions[d].parent,
            None => break,
        }
    }
    out
}

/// What `name` means at `at`: `binding_index_at`'s filter (the visible
/// extent holds the site) and maximum (the innermost declaration wins) over
/// the candidates — the same answer, without scanning every same-named
/// binding per site.
fn resolve_at(facts: &FileScope<'_>, candidates: &[usize], at: Span) -> Option<usize> {
    candidates
        .iter()
        .copied()
        .filter(|&bi| {
            let v = facts.bindings[bi].visible;
            (v[0], v[1]) <= (at[0], at[1]) && (at[0], at[1]) <= (v[2], v[3])
        })
        .max_by_key(|&bi| {
            let v = facts.bindings[bi].visible;
            (v[0], v[1])
        })
}

/// What `name` means at `at` in `def`, cached per (definition, name):
/// each distinct pair scans once no matter how many sites name it, which
/// keeps the sink walk linear.
fn winner(
    facts: &FileScope<'_>,
    by_scope: &HashMap<(Option<usize>, &str), Vec<usize>>,
    resolved: &mut HashMap<(Option<usize>, String), Vec<usize>>,
    def: Option<usize>,
    name: &str,
    at: Span,
) -> Option<usize> {
    let candidates = resolved
        .entry((def, name.to_owned()))
        .or_insert_with(|| resolve_candidates(facts, by_scope, def, name));
    resolve_at(facts, candidates, at)
}

/// A recorder value in a call: a recorder-bound name, or a recorder
/// construction inline.
fn with_recorder(
    facts: &FileScope<'_>,
    by_scope: &HashMap<(Option<usize>, &str), Vec<usize>>,
    resolved: &mut HashMap<(Option<usize>, String), Vec<usize>>,
    recorder_bindings: &HashSet<usize>,
    recorder_chains: &HashSet<usize>,
    def: Option<usize>,
    link: &CallLink,
) -> bool {
    link.args.iter().any(|other| match other {
        Arg::Name { text, span } => winner(facts, by_scope, resolved, def, text, *span)
            .is_some_and(|o| recorder_bindings.contains(&o)),
        Arg::Other { span } => recorder_chains
            .iter()
            .any(|&r| within(facts.chains[r].span, *span)),
        _ => false,
    })
}

/// Spans of the `http.NewRequest` call chains whose request value reaches an
/// in-process sink (contract M6b S4).
///
/// The allow-list, each matched on positively identified forms only:
/// - the request's name is an argument of a `ServeHTTP` call (any receiver:
///   a `ServeHTTP` method means a handler);
/// - the request's name is passed to the same call as an `httptest` recorder
///   value (a binding built by `httptest.NewRecorder`/`NewResponseRecorder`,
///   or one built inline), as in a direct handler invocation;
/// - the request's name is passed to a handler registered on a `ServeMux`
///   (`Handle`/`HandleFunc` with two arguments on the package functions or a
///   `ServeMux`-typed or `NewServeMux`-built receiver, mirroring the route
///   pack's gate; a `HandlerFunc` conversion names its function).
///   Registration is file-global: any direct call of the registered handler
///   in the file is in-process (the handler runs in-process with that
///   request).
///
/// The walk starts at each `NewRequest` site and follows the request's name
/// through same-file helper calls (a positional bare-name argument into a
/// uniquely named plain function), up to [`MAX_SINK_DEPTH`] deep, with a
/// visited set. Every use site must resolve to the request's own binding
/// ([`FileScope::binding_index_at`]), so a shadowing same-named local can
/// only stop the walk, never mark. A request also passed to `Do` keeps its
/// edge (sent both ways: the network wins).
///
/// Out of scope by construction: a hand-rolled recorder (no recognised
/// construction); a request held in a member (`s.req`); a request built and
/// served across files; a reassigned request name (the first binding wins);
/// a multi-value passthrough (`f(http.NewRequest(…))`, uncompilable except
/// into a `(*Request, error)` pair, which no sink shape takes by parts).
fn go_in_process(facts: &FileScope<'_>, http: &str, httptest: Option<&str>) -> BTreeSet<Span> {
    let mut new_requests = Vec::new();
    let mut recorder_chains: HashSet<usize> = HashSet::new();
    // `http.HandlerFunc(f)` conversions by span, for mux registrations.
    let mut handlerfuncs: Vec<(Span, String)> = Vec::new();
    let mut by_enclosing: HashMap<Option<usize>, Vec<usize>> = HashMap::new();
    for (i, chain) in facts.chains.iter().enumerate() {
        crate::work::tick(1);
        if go_new_request(http, chain) {
            new_requests.push(i);
        }
        if httptest.is_some_and(|h| {
            chain.root.as_deref() == Some(h)
                && matches!(chain.links.as_slice(), [link]
                    if matches!(link.name.as_str(), "NewRecorder" | "NewResponseRecorder"))
        }) {
            recorder_chains.insert(i);
        }
        if chain.root.as_deref() == Some(http)
            && let [link] = chain.links.as_slice()
            && link.name == "HandlerFunc"
            && let Some(Arg::Name { text, .. }) = link.args.first()
        {
            handlerfuncs.push((chain.span, text.clone()));
        }
        by_enclosing.entry(chain.enclosing).or_default().push(i);
    }
    if new_requests.is_empty() {
        return BTreeSet::new();
    }
    // Bindings built by a recorder chain, and handlers registered on a mux.
    let mut recorder_bindings: HashSet<usize> = HashSet::new();
    for (bi, b) in facts.bindings.iter().enumerate() {
        crate::work::tick(1);
        if matches!(&b.init, Some(Init::Call { chain }) if recorder_chains.contains(chain)) {
            recorder_bindings.insert(bi);
        }
    }
    let is_mux = |root: &str, at: Span| {
        root == http
            || facts.binding_at(root, at).is_some_and(|b| {
                b.type_name == format!("{http}.ServeMux")
                    || matches!(&b.init, Some(Init::Call { chain }) if facts.chains.get(*chain).is_some_and(|c| {
                        c.root.as_deref() == Some(http)
                            && c.links.len() == 1
                            && c.links[0].name == "NewServeMux"
                    }))
            })
    };
    let mut mux_handlers: HashSet<String> = HashSet::new();
    for chain in facts.chains.iter() {
        crate::work::tick(1);
        let Some(last) = chain.links.last() else {
            continue;
        };
        if !matches!(last.name.as_str(), "Handle" | "HandleFunc") || last.args.len() != 2 {
            continue;
        }
        let Some(root) = chain.root.as_deref() else {
            continue;
        };
        if !is_mux(root, chain.span) {
            continue;
        }
        for arg in &last.args[1..] {
            match arg {
                Arg::Name { text, .. } => {
                    mux_handlers.insert(text.clone());
                }
                Arg::Other { span } => {
                    // `mux.Handle(p, http.HandlerFunc(f))` names `f`.
                    mux_handlers.extend(
                        handlerfuncs
                            .iter()
                            .filter(|(at, _)| within(*at, *span))
                            .map(|(_, name)| name.clone()),
                    );
                }
                _ => {}
            }
        }
    }
    // Non-member bindings by (scope, name): the same index
    // `binding_index_at` walks, grouped once (one pass, linear). A visit
    // resolves a name from the scopes that can win for it — its own
    // definition, its ancestors, the file — instead of scanning every
    // same-named binding per site (which made the walk quadratic in the
    // file). The filter and the maximum are `binding_index_at`'s own, so
    // the answer is identical; a winner always lives in one of these
    // scopes (anything else is disjoint from the definition).
    let mut by_scope: HashMap<(Option<usize>, &str), Vec<usize>> = HashMap::new();
    for (bi, b) in facts.bindings.iter().enumerate() {
        crate::work::tick(1);
        if !b.member {
            by_scope
                .entry((b.scope, b.name.as_str()))
                .or_default()
                .push(bi);
        }
    }
    // Plain same-file functions by name, and their parameters by position.
    let mut funcs: HashMap<&str, Vec<usize>> = HashMap::new();
    for (di, d) in facts.facts.definitions.iter().enumerate() {
        crate::work::tick(1);
        if d.kind == DefKind::Function {
            funcs.entry(d.name.as_str()).or_default().push(di);
        }
    }
    let mut params: HashMap<(usize, u32), Vec<usize>> = HashMap::new();
    for (bi, b) in facts.bindings.iter().enumerate() {
        crate::work::tick(1);
        if let (Some(scope), Some(position)) = (b.scope, b.parameter) {
            params.entry((scope, position)).or_default().push(bi);
        }
    }
    // A request-carrying name: a non-member binding whose statement holds a
    // `NewRequest` chain and no sibling chain (`req, err := …`; a wrapper or
    // a `range` puts a sibling chain in the statement, and stays out). One
    // pass over the bindings and, per candidate, one over the chains: linear
    // in the file per `NewRequest` site.
    let mut sunk: HashSet<usize> = HashSet::new();
    let mut net: HashSet<usize> = HashSet::new();
    let mut visited: HashSet<(Option<usize>, usize)> = HashSet::new();
    // (definition, binding, request chains, depth).
    let mut work: Vec<(Option<usize>, usize, Vec<usize>, usize)> = Vec::new();
    for &nr in &new_requests {
        let span = facts.chains[nr].span;
        let enclosing = facts.chains[nr].enclosing;
        for (bi, b) in facts.bindings.iter().enumerate() {
            crate::work::tick(1);
            if b.member || !within(span, b.span) {
                continue;
            }
            let mut siblings = 0;
            for (ci, c) in facts.chains.iter().enumerate() {
                crate::work::tick(1);
                if ci != nr && within(c.span, b.span) && !within(c.span, span) {
                    siblings += 1;
                }
            }
            if siblings == 0 && visited.insert((enclosing, bi)) {
                work.push((enclosing, bi, vec![nr], 0));
            }
        }
    }
    // What a name means in the visited definition, cached per
    // (definition, name): each distinct pair scans once no matter how many
    // sites name it, which keeps the walk linear.
    let mut resolved: HashMap<(Option<usize>, String), Vec<usize>> = HashMap::new();
    while let Some((def, binding, requests, depth)) = work.pop() {
        let Some(members) = by_enclosing.get(&def) else {
            continue;
        };
        for &ci in members {
            let chain = &facts.chains[ci];
            // A callee the mux rule can name: a bare `h(…)` or `r.m(…)`.
            let callee = if chain.links.len() == 1 {
                match &chain.root {
                    None => chain.links[0].name.clone(),
                    Some(root) => format!("{root}.{}", chain.links[0].name),
                }
            } else {
                String::new()
            };
            for link in &chain.links {
                for (k, arg) in link.args.iter().enumerate() {
                    crate::work::tick(1);
                    // The request by name (resolving to its own binding), by
                    // inline construction, or not at all.
                    let carried = matches!(arg, Arg::Name { text, span }
                        if winner(facts, &by_scope, &mut resolved, def, text, *span) == Some(binding));
                    let inline: Vec<usize> = new_requests
                        .iter()
                        .copied()
                        .filter(|&n| {
                            matches!(arg, Arg::Other { span } if within(facts.chains[n].span, *span))
                        })
                        .collect();
                    if link.name == "ServeHTTP" && carried {
                        sunk.extend(requests.iter().copied());
                    }
                    if link.name == "Do" && carried {
                        net.extend(requests.iter().copied());
                    }
                    if carried {
                        if with_recorder(
                            facts,
                            &by_scope,
                            &mut resolved,
                            &recorder_bindings,
                            &recorder_chains,
                            def,
                            link,
                        ) {
                            sunk.extend(requests.iter().copied());
                        }
                        if mux_handlers.contains(&callee) {
                            sunk.extend(requests.iter().copied());
                        }
                        // Through a same-file helper: a positional bare-name
                        // argument into a uniquely named plain function.
                        if depth < MAX_SINK_DEPTH
                            && chain.root.is_none()
                            && chain.links.len() == 1
                            && let Some([target]) = funcs.get(link.name.as_str()).map(Vec::as_slice)
                            && let Some([param]) =
                                params.get(&(*target, k as u32)).map(Vec::as_slice)
                            && visited.insert((Some(*target), *param))
                        {
                            work.push((Some(*target), *param, requests.clone(), depth + 1));
                        }
                    }
                    for n in inline {
                        if with_recorder(
                            facts,
                            &by_scope,
                            &mut resolved,
                            &recorder_bindings,
                            &recorder_chains,
                            def,
                            link,
                        ) {
                            sunk.insert(n);
                        }
                    }
                }
            }
        }
    }
    sunk.difference(&net)
        .map(|&n| facts.chains[n].span)
        .collect()
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

// ---------------------------------------------------------------------------------------------
// Generated clients (contract M6f S6): OpenAPI-generator operation calls and gRPC stub calls.
// Per-file candidates; the indexer keeps only the ones naming a spec operation or RPC, so the
// pack stays generous within its allow-list and the join stays strict.

/// The last `.`-segment of a dotted path.
fn last_segment(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// An inline construction's type (`new UsersApi()` → `UsersApi`), or `None`.
fn constructed(root: &str) -> Option<&str> {
    let root = root.strip_prefix("new ")?;
    let type_name = root.split('(').next()?.trim();
    (!type_name.is_empty()).then_some(type_name)
}

/// A `New*Client` constructor call's service (`NewGreeterClient` → `Greeter`); empty when the
/// name carries none.
fn client_service(name: &str) -> Option<&str> {
    let inner = name.strip_prefix("New")?.strip_suffix("Client")?;
    (!inner.is_empty()).then_some(inner)
}

/// A `*Client` type's service (`GreeterClient` → `Greeter`); empty when it carries none.
fn client_type(name: &str) -> Option<&str> {
    let inner = name.strip_suffix("Client")?;
    (!inner.is_empty()).then_some(inner)
}

/// Framework HTTP client types: `*Client`-shaped but never generated stubs (Angular
/// `HttpClient`, Spring `WebClient`/`RestClient`). Bindings and constructions of these types
/// are URL calls (or nothing), never gRPC candidates.
fn is_framework_client(type_name: &str) -> bool {
    let simple = type_name.rsplit('.').next().unwrap_or(type_name);
    matches!(simple, "HttpClient" | "WebClient" | "RestClient")
}

/// Whether the type `name` is imported from a module whose specifier names generated code
/// (contract M7c S4): a specifier containing `grpc`, `_pb` or `proto`, or a whole path segment
/// `gen`/`generated` (`./proto/greeter_grpc_pb`, `../gen/cart_pb`, `@acme/protos`, `./gen`).
/// `google-auth-library`, or a class declared in the file, is not.
fn generated_stub_import(facts: &FileFacts, name: &str) -> bool {
    facts.imports.iter().any(|i| {
        let module = i.module.to_ascii_lowercase();
        let generated = module.contains("grpc")
            || module.contains("_pb")
            || module.contains("proto")
            || module
                .split('/')
                .any(|seg| seg == "gen" || seg == "generated");
        generated && i.names.iter().any(|n| n.local == name)
    })
}

/// The binding `root` names and its type, when it resolves to one binding.
fn binding_type<'f>(
    facts: &'f FileScope<'_>,
    language: Language,
    root: &str,
    at: Span,
) -> Option<&'f str> {
    let bi = local_binding(facts, language, root, at)?;
    Some(facts.bindings[bi].type_name.as_str())
}

/// OperationId/RPC candidates of one file: single-link calls on `*Api` receivers or
/// `*Api`-typed bindings (OpenAPI), `*Grpc` roots with 2+ links, inline `new *Stub`
/// constructions, `*Stub`-typed bindings (gRPC Java), and calls on bindings built by
/// `New*Client` constructors or `new *Client` (gRPC Go/TypeScript). Multi-link chains name
/// navigation, not operations — only the last link of a stub chain reads as the RPC.
/// The spans URL packs already claimed as call sites.
fn claimed(out: &FileClients) -> HashSet<Span> {
    out.calls.iter().map(|c| c.span).collect()
}

fn generated_calls(
    language: Language,
    facts: &FileScope<'_>,
    out: &mut FileClients,
    claimed: &HashSet<Span>,
) {
    for chain in &facts.chains {
        crate::work::tick(1);
        // A chain the URL packs already read as a call site is an HTTP client, whatever its
        // receiver is named (`mailApi.post`, Angular `this.http.post`): never a second,
        // generated reading of the same call.
        if claimed.contains(&chain.span) {
            continue;
        }
        let (Some(root), links) = (chain.root.as_deref(), chain.links.as_slice()) else {
            continue;
        };
        let [only] = links else {
            // Go/TypeScript `pb.NewXClient(conn).Rpc(ctx, req)`: exactly a constructor then the
            // RPC (contract M7a S1). Longer chains and constructor-after-constructor are SDK
            // navigation (`….NewContainerClient(c).NewBlobClient(p)`), never an RPC.
            if matches!(
                language,
                Language::Go | Language::TypeScript | Language::Tsx | Language::JavaScript
            ) && let [ctor, rpc] = links
                && let Some(service) = client_service(&ctor.name)
                && !rpc.name.is_empty()
                && client_service(&rpc.name).is_none()
            {
                out.generated.push(GeneratedCall {
                    kind: GeneratedKind::Grpc,
                    operation: rpc.name.clone(),
                    carrier: service.to_owned(),
                    span: rpc.span,
                });
                continue;
            }
            // A stub chain's last link is its RPC; anything else multi-link is navigation.
            if matches!(
                language,
                Language::Java
                    | Language::Go
                    | Language::TypeScript
                    | Language::Tsx
                    | Language::JavaScript
            ) && links.len() >= 2
                && let Some(last) = links.last()
                && !last.name.is_empty()
            {
                grpc_chain(out, root, last);
            }
            continue;
        };
        if only.name.is_empty() {
            continue;
        }
        let simplified = constructed(root).unwrap_or(root);
        let simple = last_segment(simplified.split('(').next().unwrap_or(simplified));
        // OpenAPI: `*Api` receivers, inline constructions and `*Api`-typed bindings.
        if simple.ends_with("Api") {
            out.generated.push(GeneratedCall {
                kind: GeneratedKind::OpenApi,
                operation: only.name.clone(),
                carrier: simplified.to_owned(),
                span: only.span,
            });
            continue;
        }
        if matches!(
            language,
            Language::Java | Language::TypeScript | Language::Tsx | Language::JavaScript
        ) && let Some(type_name) = binding_type(facts, language, root, chain.span)
            && last_segment(type_name).ends_with("Api")
        {
            out.generated.push(GeneratedCall {
                kind: GeneratedKind::OpenApi,
                operation: only.name.clone(),
                carrier: root.to_owned(),
                span: only.span,
            });
            continue;
        }
        // gRPC Java: `*Stub`-typed bindings and inline `new *Stub`.
        if language == Language::Java {
            if let Some(type_name) = binding_type(facts, language, root, chain.span)
                && last_segment(type_name).ends_with("Stub")
            {
                out.generated.push(GeneratedCall {
                    kind: GeneratedKind::Grpc,
                    operation: only.name.clone(),
                    carrier: root.to_owned(),
                    span: only.span,
                });
                continue;
            }
            if let Some(type_name) = constructed(root)
                && last_segment(type_name).ends_with("Stub")
                && !only.name.ends_with("Stub")
            {
                out.generated.push(GeneratedCall {
                    kind: GeneratedKind::Grpc,
                    operation: only.name.clone(),
                    carrier: type_name.to_owned(),
                    span: only.span,
                });
                continue;
            }
        }
        // gRPC Go/TypeScript: bindings whose type is a `*Client` (TypeScript `new`), whose
        // initialiser ends in a `New*Client` constructor, or whose own statement holds exactly
        // one chain ending in one (Go `client, _ := pb.NewGreeterClient(conn)`, whose binding
        // carries no initialiser: the M6b-S4 statement-span association). The constructor must
        // be the chain's last link: `resp, _ := pb.NewXClient(c).GetX(…)` binds the response,
        // not the client (contract M7a S1).
        if matches!(
            language,
            Language::Go | Language::TypeScript | Language::Tsx | Language::JavaScript
        ) && let Some(bi) = local_binding(facts, language, root, chain.span)
        {
            let binding = &facts.bindings[bi];
            // TS/JS: a `*Client` type is a stub only when imported from a generated-code
            // module (contract M7c S4); Go stubs are recognised by their constructor.
            let stub_type = |type_name: &str| {
                language == Language::Go || generated_stub_import(facts, last_segment(type_name))
            };
            let mut service = (!is_framework_client(&binding.type_name)
                && stub_type(&binding.type_name))
            .then(|| client_type(last_segment(&binding.type_name)))
            .flatten()
            .map(str::to_owned);
            if service.is_none() {
                match &binding.init {
                    Some(Init::Call { chain: ci }) => {
                        if let Some(init_chain) = facts.chains.get(*ci) {
                            crate::work::tick(1);
                            service = init_chain
                                .links
                                .last()
                                .and_then(|link| client_service(&link.name))
                                .map(str::to_owned);
                        }
                    }
                    Some(Init::New { type_name, .. })
                        if !is_framework_client(type_name) && stub_type(type_name) =>
                    {
                        service = client_type(last_segment(type_name)).map(str::to_owned);
                    }
                    None => {
                        let mut found = Vec::new();
                        for candidate in facts.chains.iter() {
                            crate::work::tick(1);
                            if !within(candidate.span, binding.span) {
                                continue;
                            }
                            if let Some(svc) =
                                candidate.links.last().and_then(|l| client_service(&l.name))
                            {
                                found.push(svc.to_owned());
                            }
                        }
                        found.sort();
                        found.dedup();
                        if let [one] = found.as_slice() {
                            service = Some(one.clone());
                        }
                    }
                    _ => {}
                }
            }
            if let Some(service) = service {
                out.generated.push(GeneratedCall {
                    kind: GeneratedKind::Grpc,
                    operation: only.name.clone(),
                    carrier: service,
                    span: only.span,
                });
            }
        }
    }
}

/// A multi-link chain's last link as a gRPC RPC when its root names the stub (`*Grpc` roots,
/// inline `new *Stub` constructions).
fn grpc_chain(out: &mut FileClients, root: &str, last: &CallLink) {
    let simplified = constructed(root).unwrap_or(root);
    let simple = last_segment(simplified.split('(').next().unwrap_or(simplified));
    if simple.ends_with("Grpc") || simple.ends_with("Stub") {
        out.generated.push(GeneratedCall {
            kind: GeneratedKind::Grpc,
            operation: last.name.clone(),
            carrier: simplified.to_owned(),
            span: last.span,
        });
    }
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
            UrlValue::Contested(candidates) => format!(
                "?[{}]",
                candidates
                    .iter()
                    .map(show_value)
                    .collect::<Vec<_>>()
                    .join("|")
            ),
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
                if c.injected.is_some() {
                    s.push_str(" injected");
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
                "rest_template GET 'http://visits/owners/{id}' base=? injected unverified",
                "rest_template PUT '{BASE}/x' base=? injected unverified",
                "rest_template ? '/y' base=? injected unverified",
                "web_client GET '/owners/{ownerId}' base='http://customers-service' unverified",
                "web_client POST 'http://visits-service/pets/visits' base=? injected unverified",
                "rest_client DELETE '/r' base=? injected unverified",
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
                "rest_template GET 'http://localhost:{port}/x/{id}' base=? injected unverified",
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
                // A request config passed by name that names nothing readable
                // is dynamic (contract M6b S5); a spread it cannot read names
                // its value for cross-file resolution.
                "axios GET ? base=? unverified",
                "axios GET '/c' base=?[cfg] unverified",
                "axios GET '/d' base='https://x.io' unverified",
                "axios GET '/e' base=?",
                "axios GET '/f' base=B",
                "axios POST '/h'",
                "axios PUT ? base=? unverified",
                "axios ? ? base=? unverified",
                "axios ? ? base=? unverified",
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
                "rest_template GET '/x' base=? injected unverified",
                "rest_template GET '/y' base=? unverified",
                "rest_template GET '/v' base='http://visits' unverified",
                "web_client GET '/v1/charges' base='https://api.stripe.com' unverified",
                // Two bases: contested, with both as candidates (contract M5 S19 (f)).
                "web_client GET '/z' base='http://b' unverified",
                "web_client GET '/u' base='http://users' unverified",
            ]
        );
        // An injected instance: marked for the indexer, which reads the base of the `@Bean`
        // method the injection point resolves to (contract M4b S21); the pack reads none.
        let bean = "package p;\nimport org.springframework.context.annotation.Bean;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nclass Cfg {\n  @Bean RestTemplate stripe(RestTemplateBuilder b) { return b.rootUri(\"https://api.stripe.com\").build(); }\n}\nclass Pay {\n  private final RestTemplate rt;\n  Pay(RestTemplate rt) { this.rt = rt; }\n  void charge() { rt.postForObject(\"/charges\", null, String.class); }\n}\n";
        assert_eq!(
            calls(Language::Java, bean),
            // An injected field is never a plain carrier (amendments 13, 15).
            ["rest_template POST '/charges' base=? injected unverified"]
        );
        // The base a `@Bean` method states, by method: one stated value, else none.
        let facts = extract(Language::Java, bean.as_bytes()).unwrap();
        let method = |name: &str| {
            facts
                .definitions
                .iter()
                .position(|d| d.name == name)
                .unwrap()
        };
        assert_eq!(
            bean_base(&facts, method("stripe"), ClientKind::RestTemplate)
                .as_ref()
                .map(show_value)
                .as_deref(),
            Some("'https://api.stripe.com'")
        );
        assert_eq!(
            bean_base(&facts, method("stripe"), ClientKind::WebClient),
            None
        );
        assert_eq!(
            bean_base(&facts, method("charge"), ClientKind::RestTemplate),
            None
        );
        let two = bean.replace(
            "{ return b.rootUri(\"https://api.stripe.com\").build(); }",
            "{ if (x) return b.rootUri(\"https://a\").build(); return b.rootUri(\"https://b\").build(); }",
        );
        let facts = extract(Language::Java, two.as_bytes()).unwrap();
        let stripe = facts
            .definitions
            .iter()
            .position(|d| d.name == "stripe")
            .unwrap();
        assert_eq!(bean_base(&facts, stripe, ClientKind::RestTemplate), None);
        // Without the `@Bean` import it is just a method.
        let plain = bean.replace("import org.springframework.context.annotation.Bean;\n", "");
        let facts = extract(Language::Java, plain.as_bytes()).unwrap();
        let stripe = facts
            .definitions
            .iter()
            .position(|d| d.name == "stripe")
            .unwrap();
        assert_eq!(bean_base(&facts, stripe, ClientKind::RestTemplate), None);
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
                "axios GET '/orders/1' base=?['http://orders'|'https://api.stripe.com'] unverified",
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
        // The module's name in its import is a string, no token of an instance named after it
        // (amendment 17).
        let named = "import Axios from \"axios\";\nconst axios = Axios.create({ baseURL: \"http://orders\" });\nexport function g() { return axios.get(\"/orders/1\"); }\n";
        assert_eq!(
            calls(Language::TypeScript, named),
            ["axios GET '/orders/1' base='http://orders'"]
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
                "axios POST '/orders' base=?['http://orders'|o] unverified",
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
                "axios GET 'http://orders/orders/1' base=cfg unverified",
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
        // A Go `http.Client` binding and a request built by `NewRequest` are not plain; the
        // package and `http.DefaultClient` are the bare module client, which the indexer caps
        // when the repository reconfigures it ([`global_hooks`]).
        let go = "package main\nimport \"net/http\"\nfunc f(c *http.Client) {\n\thttp.Get(\"http://orders/orders/1\")\n\thttp.DefaultClient.Get(\"http://orders/orders/1\")\n\tc.Get(\"http://orders/orders/1\")\n\thttp.NewRequest(\"GET\", \"http://orders/orders/1\", nil)\n}\n";
        let facts = extract(Language::Go, go.as_bytes()).unwrap();
        let verified: Vec<Option<bool>> = clients(Language::Go, &facts)
            .calls
            .iter()
            .map(|c| c.base_verified)
            .collect();
        assert_eq!(verified, [None, None, Some(false), Some(false)]);
    }

    /// Contract M4a amendment 16 (evaluator pass 6, V1): a fresh construction is plain only
    /// when it is exactly `new RestTemplate()` (no argument, no class body), a static
    /// `WebClient`/`RestClient` `create()` or `create(<url>)` (a string, or a name the file
    /// binds to a `String`: a constant, a `@Value` field, a parameter), or a no-argument
    /// `builder()`/`new RestTemplateBuilder()` followed by plain links. A client, a customizer,
    /// a request factory or a name not known to be a `String` as the argument, or a class body,
    /// caps the site, inline and local alike.
    #[test]
    fn fresh_constructions_are_allow_listed() {
        let src = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.web.client.RestClient;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass J {\n  static final String ORDERS = \"http://orders\";\n  @Value(\"${orders.url}\") private String ordersUrl;\n  void j1() {\n    RestTemplate rt = new RestTemplate();\n    RestClient.create(rt).get().uri(\"http://orders/orders/1\").retrieve();\n  }\n  void j2(RestTemplate injected) { RestClient.builder(injected).build().get().uri(\"http://orders/orders/1\").retrieve(); }\n  void j5(RestTemplate injected) {\n    RestClient c = RestClient.create(injected);\n    c.get().uri(\"http://orders/orders/1\").retrieve();\n  }\n  void j3(RestTemplateCustomizer cust) {\n    RestTemplate rt = new RestTemplateBuilder(cust).build();\n    rt.getForObject(\"http://orders/orders/1\", String.class);\n  }\n  void j4() { new RestTemplate(factory).getForObject(\"http://orders/orders/1\", String.class); }\n  void j10() {\n    RestTemplate rt = new RestTemplate() { };\n    rt.getForObject(\"http://orders/orders/1\", String.class);\n  }\n  void j10b() { new RestTemplate() { }.getForObject(\"http://orders/orders/1\", String.class); }\n  void ok(String base) {\n    WebClient.create(ORDERS).get().uri(\"/orders/1\").retrieve();\n    WebClient.create(ordersUrl).get().uri(\"/orders/1\").retrieve();\n    RestClient.create(base).get().uri(\"/orders/1\").retrieve();\n    RestClient.create().get().uri(\"http://orders/orders/1\").retrieve();\n    RestClient.builder().baseUrl(\"http://orders\").build().get().uri(\"/orders/1\").retrieve();\n    RestTemplate b = new RestTemplateBuilder().rootUri(\"http://orders\").build();\n    b.getForObject(\"/orders/1\", String.class);\n    RestTemplate cb = new CustomBuilder().build();\n    cb.getForObject(\"http://orders/orders/1\", String.class);\n    WebClient.create(Urls.ORDERS).get().uri(\"/orders/1\").retrieve();\n  }\n}\n";
        assert_eq!(
            calls(Language::Java, src),
            [
                // `j3`, `j4`, `j10`, `j10b`, then the control `b`.
                "rest_template GET 'http://orders/orders/1' base=? unverified",
                "rest_template GET 'http://orders/orders/1' base=? unverified",
                "rest_template GET 'http://orders/orders/1' base=? unverified",
                "rest_template GET 'http://orders/orders/1' base=? unverified",
                "rest_template GET '/orders/1' base='http://orders'",
                // A builder of another type is no fresh construction.
                "rest_template GET 'http://orders/orders/1' base=? unverified",
                // `j1`, `j2`, `j5`: built from a `RestTemplate`.
                "rest_client GET 'http://orders/orders/1' base=rt unverified",
                "rest_client GET 'http://orders/orders/1' base=? unverified",
                "rest_client GET 'http://orders/orders/1' base=injected unverified",
                // The controls; a constant of another file is not known to be a `String`.
                "web_client GET '/orders/1' base=ORDERS",
                "web_client GET '/orders/1' base=ordersUrl",
                "rest_client GET '/orders/1' base=base",
                "rest_client GET 'http://orders/orders/1' base=?",
                "rest_client GET '/orders/1' base='http://orders'",
                "web_client GET '/orders/1' base=Urls.ORDERS unverified",
            ]
        );
    }

    /// Contract M4a amendments 15 and 16: the module clients a file may reconfigure for the
    /// whole process, by allow-list. `fetch`: every `fetch` token is a callee or a
    /// `node-fetch`/`cross-fetch` import or `require`. axios: every token of a module binding
    /// (an import, a `require` at any scope) is its declaration, a callee, or the receiver of a
    /// request, `create` or helper call. Go: no dot import of `net/http`, and every
    /// `DefaultTransport`/`DefaultClient` token is the receiver of a request call. Feign: no
    /// `RequestInterceptor` token, no `configuration =` on a `@FeignClient` (nor
    /// `defaultConfiguration =` on `@EnableFeignClients`). Any other role is a hook.
    #[test]
    fn global_hooks_of_module_clients() {
        let hooks = |language: Language, src: &str| {
            global_hooks(language, &extract(language, src.as_bytes()).unwrap())
        };
        for clean in [
            "import axios from \"axios\";\nimport * as ax from \"axios\";\nconst api = axios.create({ baseURL: \"http://orders\" });\naxios.get(\"/a\");\naxios(\"/b\");\nax.post(\"/c\", 1);\nif (axios.isAxiosError(e)) {}\nwindow.fetch(\"/d\");\nfetch(\"/e\");\nglobalThis.location.href;\nthis.fetch(\"/f\");\n",
            "import fetch from \"node-fetch\";\nfetch(\"/a\");\n",
            "const fetch = require(\"cross-fetch\");\nfetch(\"/a\");\n",
            "export function f() {\n  const axios = require(\"axios\");\n  return axios.get(\"/a\");\n}\n",
            // Contract M4a amendment 17: the module named by an import (type-only too) or an
            // ambient module declaration (types only) is no hook.
            "import type { AxiosInstance } from \"axios\";\nimport { AxiosError } from \"axios\";\ndeclare module \"axios\" {\n  interface AxiosRequestConfig { retry?: number }\n}\n",
            "const label = \"fetching\";\nconst m = \"axiosx\";\n",
            // A subpath's `require` or import binding is the module's declaration (W5).
            "const ax = require(\"axios/dist/node/axios.cjs\");\nax.get(\"/a\");\n",
            "import ax from \"axios/dist/node/axios.cjs\";\nax.get(\"/a\");\n",
        ] {
            assert!(hooks(Language::TypeScript, clean).is_empty(), "{clean}");
        }
        for (src, kind) in [
            ("globalThis.fetch = f;\n", ClientKind::Fetch),
            ("window.fetch = f;\n", ClientKind::Fetch),
            ("global.fetch = jest.fn();\n", ClientKind::Fetch),
            ("self.fetch = f;\n", ClientKind::Fetch),
            ("const f = window.fetch;\n", ClientKind::Fetch),
            // Evaluator pass 6, V3: casts and a bare assignment.
            ("(globalThis as any).fetch = f;\n", ClientKind::Fetch),
            ("(window as any).fetch = f;\n", ClientKind::Fetch),
            ("(global as any).fetch = jest.fn();\n", ClientKind::Fetch),
            ("fetch = wrap(fetch);\n", ClientKind::Fetch),
            (
                "if (typeof fetch === \"undefined\") {}\n",
                ClientKind::Fetch,
            ),
            ("Object.assign(globalThis, { fetch });\n", ClientKind::Fetch),
            ("import { fetch } from \"undici\";\n", ClientKind::Fetch),
            ("class A { fetch() {} }\n", ClientKind::Fetch),
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
            (
                "import * as ax from \"axios\";\nax.default.defaults.adapter = a;\n",
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
            // A subpath of the module is the module (the same `defaults`).
            (
                "import ax from \"axios/dist/node/axios.cjs\";\nax.defaults.adapter = a;\n",
                ClientKind::Axios,
            ),
            // Evaluator pass 6, V4: a `require` in a function.
            (
                "export function setup() {\n  const axios = require(\"axios\");\n  axios.interceptors.request.use((c) => c);\n}\n",
                ClientKind::Axios,
            ),
            // Evaluator pass 7, W5: a `require` of a subpath is the module.
            (
                "const ax = require(\"axios/dist/node/axios.cjs\");\nax.defaults.adapter = a;\n",
                ClientKind::Axios,
            ),
            // Evaluator pass 7, W3: module objects reached through a form that is no recognised
            // binding (import-equals, a member or a destructuring after `require`, a dynamic
            // import, a re-export) name the module in a string outside an import or `require`
            // declaration: any of them caps every axios site.
            (
                "import axios = require(\"axios\");\naxios.defaults.adapter = a;\n",
                ClientKind::Axios,
            ),
            (
                "const ax = require(\"axios\").default;\nax.defaults.adapter = a;\n",
                ClientKind::Axios,
            ),
            (
                "const { default: ax } = require(\"axios\");\nax.defaults.adapter = a;\n",
                ClientKind::Axios,
            ),
            (
                "import(\"axios\").then((m) => { m.default.defaults.adapter = a; });\n",
                ClientKind::Axios,
            ),
            (
                "export { default as ax } from \"axios\";\n",
                ClientKind::Axios,
            ),
            (
                "jest.mock(\"axios/lib/adapters/http\");\n",
                ClientKind::Axios,
            ),
            // Evaluator pass 7, W6: `fetch` named by a string (a spy, a stub, a subscript).
            (
                "jest.spyOn(global, \"fetch\").mockImplementation(f);\n",
                ClientKind::Fetch,
            ),
            ("vi.stubGlobal(\"fetch\", vi.fn());\n", ClientKind::Fetch),
            (
                "(globalThis as any)[\"fetch\"] = wrapped;\n",
                ClientKind::Fetch,
            ),
            ("globalThis[`fetch`] = wrapped;\n", ClientKind::Fetch),
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
                &go("\thttp.Get(u)\n\thttp.DefaultClient.Do(req)\n\thttp.DefaultClient.Get(u)\n\thttp.DefaultClient.Head(u)\n\thttp.DefaultClient.Post(u, t, b)\n\thttp.DefaultClient.PostForm(u, v)\n\t_ = http.StatusOK")
            )
            .is_empty()
        );
        for body in [
            "\thttp.DefaultTransport = t",
            "\thttp.DefaultClient.Transport = t",
            "\thttp.DefaultClient = &http.Client{}",
            "\tc := http.DefaultClient",
            "\tt := http.DefaultTransport.(*http.Transport).Clone()",
            "\thttp.DefaultClient.CloseIdleConnections()",
        ] {
            assert_eq!(
                hooks(Language::Go, &go(body)),
                [ClientKind::GoHttp],
                "{body}"
            );
        }
        // Evaluator pass 6, V5: a dot import of `net/http`.
        assert_eq!(
            hooks(
                Language::Go,
                "package main\nimport . \"net/http\"\nfunc init() {\n\tGet(u)\n}\n"
            ),
            [ClientKind::GoHttp]
        );
        // Evaluator pass 6, V2: Feign.
        let feign = |annotation: &str| {
            format!(
                "package p;\nimport org.springframework.cloud.openfeign.FeignClient;\nimport org.springframework.cloud.openfeign.EnableFeignClients;\n{annotation}\ninterface A {{}}\n"
            )
        };
        assert!(
            hooks(
                Language::Java,
                &feign("@FeignClient(name = \"a\", url = \"http://a\")")
            )
            .is_empty()
        );
        assert!(
            hooks(
                Language::Java,
                "package p;\nimport feign.FeignException;\nimport feign.codec.ErrorDecoder;\nclass E implements ErrorDecoder {}\n"
            )
            .is_empty()
        );
        for src in [
            feign("@FeignClient(name = \"a\", configuration = RewriteConfig.class)"),
            feign("@EnableFeignClients(defaultConfiguration = RewriteConfig.class)"),
            "package p;\nimport feign.RequestInterceptor;\nclass G {}\n".to_owned(),
            "package p;\nimport feign.*;\nclass G {}\n".to_owned(),
            "package p;\nclass G implements RequestInterceptor {}\n".to_owned(),
            // Other extension points that may retarget every Feign client.
            "package p;\nimport feign.Client;\nclass T { @Bean Client c() { return null; } }\n".to_owned(),
            "package p;\nclass T { @Bean feign.Client c() { return null; } }\n".to_owned(),
            "package p;\nclass T { @Bean Capability c() { return null; } }\n".to_owned(),
            "package p;\nimport org.springframework.cloud.openfeign.FeignBuilderCustomizer;\nclass T {}\n".to_owned(),
            "package p;\nclass T { @Bean Feign.Builder b() { return null; } }\n".to_owned(),
        ] {
            assert_eq!(hooks(Language::Java, &src), [ClientKind::Feign], "{src}");
        }
    }

    /// Contract M4b S6/S9 (d): the raw-text scan also collects the whole words of non-inert
    /// files, XML comments included (CDATA cannot hide a name), and gate D's findings; a file
    /// over the cap leaves its words unknown.
    #[test]
    fn raw_scan_collects_words_and_di_surface() {
        let x = raw_scan(
            "p/ctx.xml",
            Some("<a><![CDATA[ <!-- ]]> KafkaAuditSink <!-- x --></a>"),
        );
        assert!(x.words.contains("KafkaAuditSink") && x.words.contains("CDATA"));
        assert!(!x.words_unread && x.di_surface.is_empty());
        // A binary name names the nested class (evaluator pass 1, Y1).
        let services = raw_scan(
            "p/src/main/resources/META-INF/services/com.acme.Greeter",
            Some("com.acme.Holder$LoadedImpl\n"),
        );
        for w in ["Holder$LoadedImpl", "Holder", "LoadedImpl", "acme"] {
            assert!(services.words.contains(w), "{w}");
        }
        let k = raw_scan("p/Cfg.kt", Some("@Configuration class Cfg"));
        assert_eq!(
            k.di_surface,
            [("p/Cfg.kt".to_owned(), "raw Configuration".to_owned())]
        );
        assert!(
            raw_scan("README.md", Some("KafkaAuditSink"))
                .words
                .is_empty()
        );
        // Python lockfiles only record what the manifests declare: prose in
        // them (a `transport` hook word in a description) scans nothing.
        for lock in ["svc/poetry.lock", "svc/uv.lock"] {
            assert!(
                raw_scan(lock, Some("transport adapter hooks dispatcher"))
                    .words
                    .is_empty(),
                "{lock}"
            );
        }
        assert!(
            raw_scan("w/package-lock.json", Some("KafkaAuditSink"))
                .words
                .is_empty()
        );
        let over = raw_scan("p/big.kt", None);
        assert!(over.words_unread && over.words.is_empty());
        assert_eq!(over.di_surface.len(), 1);
        let mut merged = x;
        merged.merge(over);
        assert!(merged.words_unread && merged.words.contains("KafkaAuditSink"));
    }

    /// Contract M4a amendment 17 (evaluator pass 7, W1, W2, W4): the raw-text scan of a file
    /// without token facts. A surface name as a whole word taints its kind (Go's only in a
    /// `.go` file); prose, npm lock files and plain dependency entries of a `package.json` are
    /// inert (XML comments are not since contract M5 S19 (b)); a file over the byte cap taints every kind it is scanned for, and
    /// discovery. Discovery: the listed keys in a settings file (nested YAML too) or as text,
    /// Spring Cloud LoadBalancer and Ribbon names; Eureka's own keys are not.
    #[test]
    fn raw_text_hooks_and_discovery_keys() {
        use ClientKind::{Axios, Feign, Fetch, GoHttp};
        let hooks = |path: &str, text: &str| -> Vec<ClientKind> {
            raw_scan(path, Some(text)).hooks.into_iter().collect()
        };
        for (path, text, want) in [
            ("w/src/App.vue", "axios.defaults.adapter = a;", vec![Axios]),
            ("w/public/index.html", "window.fetch = f;", vec![Fetch]),
            // HTML is not stripped like XML (M4a open item 24): `<!--` in a script is code.
            (
                "w/public/index.html",
                "<script>document.write(\"<!--\"); window.fetch = f; // --></script>",
                vec![Fetch],
            ),
            ("w/src/App.svelte", "$fetch(u)", vec![Fetch]),
            ("p/G.kt", "import feign.RequestInterceptor", vec![Feign]),
            ("p/G.groovy", "@Bean Feign.Builder b()", vec![Feign]),
            ("p/G.scala", "new Capability {}", vec![Feign]),
            ("p/G.kt", "fun t(): Targeter = x", vec![Feign]),
            ("p/G.kt", "fun c(): FeignBuilderCustomizer = x", vec![Feign]),
            ("p/a.properties", "feign.okhttp.enabled=true", vec![Feign]),
            ("g/zz.go", "http.DefaultTransport = nil", vec![GoHttp]),
            ("g/zz.go", "c := http.DefaultClient", vec![GoHttp]),
            // Whole words only; Go's names only in Go files.
            ("p/G.kt", "class Capabilityx; val prefetch = 1", vec![]),
            ("p/a.yml", "batch_fetch_size: 3\nopenfeign: x\n", vec![]),
            ("p/G.kt", "val c = DefaultClient", vec![]),
            // Inert text.
            (
                "README.md",
                "axios.defaults; fetch; RequestInterceptor",
                vec![],
            ),
            ("docs/a.txt", "fetch", vec![]),
            ("w/package-lock.json", "\"node_modules/axios\"", vec![]),
            ("w/yarn.lock", "axios@^1.7.7:", vec![]),
            (
                "w/package.json",
                "{ \"dependencies\": { \"axios\": \"^1.7.7\", \"x\": \"1.0.0 - 2.x || >=3\" }, \"peerDependencies\": { \"node-fetch\": \"*\" } }",
                vec![],
            ),
            // An XML comment is read like the rest of the file (contract M5 S19 (b)).
            (
                "p/pom.xml",
                "<p><!-- fetch; feign.Client --></p>",
                vec![Fetch, Feign],
            ),
            // Not inert: an alias, another field, an XML element, an MDX page.
            (
                "w/package.json",
                "{ \"dependencies\": { \"axios\": \"npm:fake-axios@1\" } }",
                vec![Axios],
            ),
            (
                "w/package.json",
                "{ \"jest\": { \"moduleNameMapper\": { \"^axios$\": \"<rootDir>/m.js\" } } }",
                vec![Axios],
            ),
            // Not JSON: scanned as text.
            (
                "w/package.json",
                "{ \"dependencies\": { \"axios\": \"^1\" }",
                vec![Axios],
            ),
            (
                "p/beans.xml",
                "<bean class=\"feign.Client.Default\"/>",
                vec![Feign],
            ),
            (
                "w/page.mdx",
                "export const x = (window.fetch = f);",
                vec![Fetch],
            ),
        ] {
            let want: Vec<ClientKind> = want;
            assert_eq!(hooks(path, text), want, "{path}: {text}");
        }
        let over = raw_scan("w/vendor.js", None);
        assert_eq!(
            over.hooks.into_iter().collect::<Vec<_>>(),
            [Fetch, Axios, Feign]
        );
        assert!(over.discovery);
        assert_eq!(
            raw_scan("g/big.go", None)
                .hooks
                .into_iter()
                .collect::<Vec<_>>(),
            [Fetch, Axios, Feign, GoHttp]
        );
        for (path, text) in [
            (
                "p/application.yml",
                "spring:\n  cloud:\n    discovery:\n      client:\n        simple:\n          instances:\n            orders:\n              - uri: https://api.stripe.com\n",
            ),
            (
                "p/application.properties",
                "orders.ribbon.listOfServers=api.stripe.com:443\n",
            ),
            (
                "p/application.yml",
                "orders:\n  ribbon:\n    NIWSServerListClassName: x\n",
            ),
            (
                "p/a.properties",
                "spring.cloud.loadbalancer.cache.enabled=false\n",
            ),
            (
                "p/a.yaml",
                "spring.cloud.kubernetes.discovery.all-namespaces: true\n",
            ),
            (
                "p/a.yaml",
                "spring:\n  cloud:\n    consul:\n      discovery:\n        instance-id: x\n",
            ),
            (
                "p/Lb.kt",
                "@LoadBalancerClient(name = \"orders\", configuration = [X::class])",
            ),
            ("p/Lb.groovy", "ServiceInstanceListSupplier s() { x }"),
            ("p/Rb.kt", "@RibbonClient(name = \"orders\")"),
            ("p/x.json", "{ \"listOfServers\": \"a:1\" }"),
            (
                "deploy/docker-compose.yml",
                "services:\n  pay:\n    environment:\n      - SPRING_CLOUD_DISCOVERY_CLIENT_SIMPLE_INSTANCES_ORDERS_0_URI=https://api.stripe.com\n",
            ),
            (
                "deploy/pay.env",
                "ORDERS_RIBBON_LISTOFSERVERS=api.stripe.com:443\n",
            ),
        ] {
            assert!(raw_scan(path, Some(text)).discovery, "{path}: {text}");
        }
        for (path, text) in [
            (
                "p/application.yml",
                "eureka:\n  client:\n    serviceUrl:\n      defaultZone: http://registry:8761/eureka/\n",
            ),
            ("p/a.properties", "spring.application.name=orders\n"),
            (
                "README.md",
                "spring.cloud.discovery.client.simple.instances",
            ),
        ] {
            assert!(!raw_scan(path, Some(text)).discovery, "{path}: {text}");
        }
        assert!(discovery_package(
            "org.springframework.cloud.loadbalancer.annotation"
        ));
        assert!(discovery_package(
            "org.springframework.cloud.netflix.ribbon"
        ));
        assert!(!discovery_package(
            "org.springframework.cloud.client.loadbalancer"
        ));
        assert!(!discovery_package(
            "org.springframework.cloud.loadbalancerx"
        ));
    }

    /// Contract M5 S19 (a) (M4a open item 22 (a), (b)): the discovery and load-balancer
    /// vocabulary is matched in the text of every file, parsed Java included (a fully qualified
    /// type with no import or annotation), and in JSON key paths written inside any text
    /// (`SPRING_APPLICATION_JSON` in a compose or `.env` file, a JSON file, escaped JSON in a
    /// string, a script's object literal). Eureka's own keys, other Spring Cloud keys and a
    /// discovery word as a value are not discovery.
    #[test]
    fn discovery_in_every_file_and_in_json_key_paths() {
        for (path, text) in [
            (
                "p/src/main/java/p/Lb.java",
                "class Lb {\n  @Bean org.springframework.cloud.loadbalancer.core.ServiceInstanceListSupplier s() { return null; }\n}\n",
            ),
            (
                "p/src/main/java/p/Lb.java",
                "class Lb { org.springframework.cloud.loadbalancer.core.ReactorLoadBalancer<X> lb; }\n",
            ),
            (
                "p/src/main/java/p/Rb.java",
                "class Rb { org.springframework.cloud.netflix.ribbon.StaticServerList<X> l; }\n",
            ),
            (
                "deploy/docker-compose.yml",
                "services:\n  pay:\n    environment:\n      SPRING_APPLICATION_JSON: '{\"spring\":{\"cloud\":{\"discovery\":{\"client\":{\"simple\":{\"instances\":{\"orders\":[{\"uri\":\"https://api.stripe.com\"}]}}}}}}}'\n",
            ),
            (
                "deploy/pay.env",
                "SPRING_APPLICATION_JSON={\"spring\":{\"cloud\":{\"loadbalancer\":{\"x\":1}}}}\n",
            ),
            (
                "deploy/config.json",
                "{ \"spring.cloud\": { \"kubernetes\": { \"discovery\": { \"all-namespaces\": true } } } }",
            ),
            (
                "deploy/k8s.json",
                "{\"env\":[{\"name\":\"SPRING_APPLICATION_JSON\",\"value\":\"{\\\"spring\\\":{\\\"cloud\\\":{\\\"consul\\\":{\\\"discovery\\\":{}}}}}\"}]}",
            ),
            (
                "web/src/env.ts",
                "process.env.SPRING_APPLICATION_JSON = JSON.stringify({ spring: { cloud: { 'load-balancer': { x: 1 } } } });\n",
            ),
        ] {
            assert!(discovery_text(path, text), "{path}: {text}");
            assert!(raw_scan(path, Some(text)).discovery, "{path}: {text}");
        }
        for (path, text) in [
            (
                "deploy/config.json",
                "{\"eureka\":{\"client\":{\"serviceUrl\":{\"defaultZone\":\"http://r/eureka\"}}}}",
            ),
            (
                "deploy/config.json",
                "{\"spring\":{\"cloud\":{\"config\":{\"uri\":\"http://config\"}}}}",
            ),
            (
                "deploy/config.json",
                "{\"spring\":{\"application\":{\"name\":\"discovery\"}},\"cloud\":\"aws\"}",
            ),
            (
                "p/src/main/java/p/B.java",
                "import org.springframework.cloud.client.loadbalancer.LoadBalanced;\nclass B { @LoadBalanced RestTemplate rt; }\n",
            ),
            ("README.md", "{\"spring\":{\"cloud\":{\"discovery\":{}}}}"),
        ] {
            assert!(!discovery_text(path, text), "{path}: {text}");
        }
    }

    /// Contract M6b S8 (D7): a discovery word in parsed code caps only when a
    /// use of a known discovery/load-balancer type resolves through imports to
    /// a discovery package (an explicit single-type import, or an on-demand
    /// one — unresolved, so capped), or is fully qualified under one. Bare
    /// words in comments, unused imports, unrelated types from the same
    /// package tree, static imports and Eureka's keys do not resolve.
    #[test]
    fn discovery_resolved_types_through_imports() {
        use rb_syntax::Language::Java;
        let resolved = |src: &str| {
            let facts = extract(Java, src.as_bytes()).unwrap();
            discovery_resolved(&facts)
        };
        for src in [
            // An imported annotation, used.
            "import org.springframework.cloud.loadbalancer.annotation.LoadBalancerClient;\n@LoadBalancerClient(name = \"orders\")\nclass Lb {}\n",
            // Fully qualified, no import (contract M5 S19 (a)'s case, narrowed
            // to a known type).
            "@org.springframework.cloud.loadbalancer.annotation.LoadBalancerClient(name = \"orders\")\nclass Lb {}\n",
            // On-demand: the use cannot be attributed, so capped.
            "import org.springframework.cloud.loadbalancer.annotation.*;\n@LoadBalancerClient(name = \"orders\")\nclass Lb {}\n",
            // Ribbon's configuration type, imported and used.
            "import org.springframework.cloud.netflix.ribbon.RibbonClient;\n@RibbonClient(name = \"orders\")\nclass R {}\n",
            // A supertype head through its import.
            "import org.springframework.cloud.loadbalancer.core.ServiceInstanceListSupplier;\nclass S implements ServiceInstanceListSupplier {}\n",
            // A declared field type through its import.
            "import org.springframework.cloud.loadbalancer.core.ServiceInstanceListSupplier;\nclass S {\nServiceInstanceListSupplier s;\n}\n",
            // A fully qualified return type (contract M5 S19 (a)'s row).
            "class Lb {\n@Bean org.springframework.cloud.loadbalancer.core.ServiceInstanceListSupplier s() { return null; }\n}\n",
            // Evaluator pass 1: the load-balancer itself routes through
            // instance suppliers, so its types resolve too.
            "import org.springframework.cloud.loadbalancer.core.ReactorLoadBalancer;\nclass S {\nReactorLoadBalancer lb;\n}\n",
            "class S {\norg.springframework.cloud.loadbalancer.core.ReactorLoadBalancer lb;\n}\n",
        ] {
            assert!(resolved(src), "{src}");
        }
        for src in [
            // A bare word in a comment resolves to nothing.
            "// the ribbon loadbalancerclient balances\nclass A {}\n",
            // An import with no use configures nothing.
            "import org.springframework.cloud.loadbalancer.annotation.LoadBalancerClient;\nclass A {}\n",
            // An unrelated type from the same package tree is not evidence.
            "import org.springframework.cloud.loadbalancer.foo.Other;\n@Other\nclass A {}\n",
            // Eureka resolves names to registered instances (F1's
            // convention): not discovery configuration.
            "import org.springframework.cloud.client.discovery.EnableDiscoveryClient;\n@EnableDiscoveryClient\nclass A {}\n",
            // `@LoadBalanced` is not a discovery type (contract M5 S19 (a)'s
            // negative row).
            "import org.springframework.cloud.client.loadbalancer.LoadBalanced;\nclass B {\n@LoadBalanced Object rt;\n}\n",
            // A type use cannot come from a static import.
            "import static org.springframework.cloud.loadbalancer.annotation.*;\n@LoadBalancerClient\nclass A {}\n",
            // A bare annotation with no import to resolve through.
            "@LoadBalancerClient\nclass A {}\n",
            // Evaluator pass 1: the same simple name from another package is
            // not discovery configuration.
            "import com.example.LoadBalancerClient;\n@LoadBalancerClient\nclass A {}\n",
            // Evaluator pass 1: an import that binds nothing used is not
            // evidence either (unused imports never cap).
            "import org.springframework.cloud.loadbalancer.annotation.Other;\n@LoadBalancerClient\nclass A {}\n",
        ] {
            assert!(!resolved(src), "{src}");
        }
    }

    /// Contract M6b S8: the facts rule is Java-only — the known types are
    /// Java types. A Java-shaped import in another language resolves nothing.
    #[test]
    fn discovery_facts_are_java_only() {
        use rb_syntax::Language::{Java, TypeScript};
        let java = extract(
            Java,
            b"import org.springframework.cloud.loadbalancer.annotation.LoadBalancerClient;\n@LoadBalancerClient\nclass Lb {}\n",
        )
        .unwrap();
        assert!(discovery_facts(Java, &java));
        let ts = extract(
            TypeScript,
            b"import { LoadBalancerClient } from \"org.springframework.cloud.loadbalancer.annotation\";\nconst x: LoadBalancerClient = y;\n",
        )
        .unwrap();
        // The facts alone resolve (the import binds the name) ...
        assert!(discovery_resolved(&ts));
        // ... but outside Java they are not evidence.
        assert!(!discovery_facts(TypeScript, &ts));
    }

    /// Contract M6b S8: the broad rule against the narrowed rule, side by
    /// side. Broad is raw-file evidence, parsed-file discovery words, or an
    /// unresolved Java import; narrowed is raw-file evidence, parsed-file
    /// single-literal keys, or import-resolved known types. A split (broad
    /// without narrowed) is what `discovery_lifted` counts downstream.
    #[test]
    fn discovery_flags_splits_broad_and_narrowed() {
        use rb_syntax::Language::{Java, TypeScript};
        let java_used = extract(
            Java,
            b"import org.springframework.cloud.loadbalancer.annotation.LoadBalancerClient;\n@LoadBalancerClient\nclass Lb {}\n",
        )
        .unwrap();
        let java_bare_import = extract(
            Java,
            b"import org.springframework.cloud.netflix.ribbon.RibbonClient;\nclass R {}\n",
        )
        .unwrap();
        let ts = extract(TypeScript, b"export const x = 1;\n").unwrap();
        let java_plain = extract(Java, b"class A {}\n").unwrap();
        fn files<'a>(
            j: &'a rb_syntax::FileFacts,
            t: &'a rb_syntax::FileFacts,
        ) -> Vec<(rb_syntax::Language, &'a rb_syntax::FileFacts)> {
            vec![(Java, j), (TypeScript, t)]
        }
        // Nothing anywhere: no evidence either way.
        assert_eq!(
            discovery_flags(
                &RawScan::default(),
                files(&java_plain, &ts).iter().map(|(l, f)| (*l, *f))
            ),
            (false, false)
        );
        // Resolved types narrow without broadening beyond the import (which
        // already broadens): both hold.
        assert_eq!(
            discovery_flags(
                &RawScan::default(),
                files(&java_used, &ts).iter().map(|(l, f)| (*l, *f))
            ),
            (true, true)
        );
        // A bare import broadens (unresolved) but resolves nothing: split.
        assert_eq!(
            discovery_flags(
                &RawScan::default(),
                files(&java_bare_import, &ts).iter().map(|(l, f)| (*l, *f))
            ),
            (true, false)
        );
        // Parsed-file words broaden without resolving: split.
        assert_eq!(
            discovery_flags(
                &RawScan {
                    discovery_parsed: true,
                    ..RawScan::default()
                },
                files(&java_plain, &ts).iter().map(|(l, f)| (*l, *f))
            ),
            (true, false)
        );
        // Raw-file evidence stays capped under both rules.
        assert_eq!(
            discovery_flags(
                &RawScan {
                    discovery: true,
                    ..RawScan::default()
                },
                files(&java_bare_import, &ts).iter().map(|(l, f)| (*l, *f))
            ),
            (true, true)
        );
        // Single-literal keys narrow without the broad words.
        assert_eq!(
            discovery_flags(
                &RawScan {
                    discovery_literal: true,
                    ..RawScan::default()
                },
                files(&java_plain, &ts).iter().map(|(l, f)| (*l, *f))
            ),
            (false, true)
        );
    }
    /// Contract M6b S8 (D7): JSON-style keys cap parsed-file text only when
    /// they co-occur inside one brace-delimited span — nested literals count
    /// (their keys sit inside the outer literal too), while sibling literals
    /// and brace-free key lists do not combine.
    #[test]
    fn single_literal_discovery_needs_one_literal() {
        for text in [
            // One flat literal.
            "const c = { spring: 1, cloud: 2, discovery: 3 };\n",
            // Nested: every key sits inside the outer literal.
            "const c = { spring: { cloud: { discovery: { x: 1 } } } };\n",
            // `SPRING_APPLICATION_JSON` in code (contract M5 S19 (a)'s shape).
            "process.env.SPRING_APPLICATION_JSON = JSON.stringify({ spring: { cloud: { 'load-balancer': { x: 1 } } } });\n",
            // Escaped JSON in a string still sits inside the outer braces.
            "{\"env\":[{\"name\":\"X\",\"value\":\"{\\\"spring\\\":{\\\"cloud\\\":{\\\"consul\\\":{\\\"discovery\\\":{}}}}}\"}]}",
            // Dotted keys in one literal.
            "{ \"spring.cloud\": { \"consul\": { \"discovery\": true } } }",
        ] {
            assert!(single_literal_discovery(text), "{text}");
        }
        for text in [
            // The same three keys in three sibling literals stay separate.
            "const a = { spring: 1 };\nconst b = { cloud: 2 };\nconst c = { discovery: 3 };\n",
            // Key lists without braces are not one literal.
            "spring:\ncloud:\ndiscovery:\n",
            // Eureka's keys are not discovery configuration.
            "{\"eureka\":{\"client\":{\"serviceUrl\":{\"defaultZone\":\"http://r/eureka\"}}}}",
            // `discovery` as a value is not a key.
            "{\"name\":\"discovery\",\"cloud\":\"x\"}",
            // Two of three is not a discovery path.
            "{ spring: 1, cloud: 2 }",
        ] {
            assert!(!single_literal_discovery(text), "{text}");
        }
    }

    /// Contract M6b S8, invariant 9: one brace-delimited literal per line
    /// costs linear work — 4× the literals makes at most 5× the counted
    /// steps. Counted, not timed, so machine load cannot fail it. The input
    /// never hits, so the scan runs end to end (an early hit would measure a
    /// constant).
    #[test]
    fn single_literal_discovery_grows_linearly() {
        fn work(n: usize) -> u64 {
            let mut text = String::new();
            for i in 0..n {
                text.push_str(&format!(
                    "const c{i} = {{ spring: {{ cloud: {{ x: {i} }} }} }};\n"
                ));
            }
            crate::work::take();
            assert!(!single_literal_discovery(&text));
            crate::work::take()
        }
        let (small, large) = (work(100), work(400));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }

    /// Contract M6b S9: `got`, `ky`, `superagent` and `undici` enter as
    /// fetch-class sites — module verbs and direct calls with import/require
    /// gates. `superagent.del` is `DELETE`; a direct `got`/`ky` call reads its
    /// method from the options like `fetch`. Each row is exactly one site: no
    /// axios-instance double (the `imported` fallback skips the new modules).
    #[test]
    fn fetch_class_library_sites() {
        let src = "import got from \"got\";\nimport ky from \"ky\";\nimport request from \"superagent\";\nimport { request as urequest } from \"undici\";\nimport * as undici from \"undici\";\nexport async function f() {\n  await got.get(\"http://orders/a\");\n  await got(\"http://orders/b\", { method: \"POST\" });\n  await ky.post(\"http://orders/c\");\n  await ky(\"http://orders/d\");\n  await request.get(\"http://orders/e\");\n  await request(\"DELETE\", \"http://orders/f\");\n  await request.del(\"http://orders/g\");\n  await urequest(\"http://orders/h\", { method: \"POST\" });\n  await undici.request(\"http://orders/i\");\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "fetch GET 'http://orders/a'",
                "fetch POST 'http://orders/b'",
                "fetch POST 'http://orders/c'",
                "fetch GET 'http://orders/d'",
                "fetch GET 'http://orders/e'",
                "fetch DELETE 'http://orders/f'",
                "fetch DELETE 'http://orders/g'",
                "fetch POST 'http://orders/h'",
                "fetch GET 'http://orders/i'",
            ]
        );
        // The controls: a shadowed module name and an unimported name give
        // nothing, and an undici `Client` is not a fetch-class shape.
        let shadowed = "import got from \"got\";\nexport async function f(got: any) {\n  await got.get(\"http://orders/a\");\n}\n";
        assert!(calls(Language::TypeScript, shadowed).is_empty());
        let plain = "export async function f() {\n  await got.get(\"http://orders/a\");\n}\n";
        assert!(calls(Language::TypeScript, plain).is_empty());
        let pooled = "import { Pool } from \"undici\";\nconst pool = new Pool(\"http://orders\");\nexport async function f() {\n  const r = await pool.request({ path: \"/a\", method: \"GET\" });\n}\n";
        assert!(calls(Language::TypeScript, pooled).is_empty());
        // A destructured member is a capped site, like the module's own.
        let destructured = "const { get } = require(\"got\");\nexport async function f() {\n  await get(\"http://orders/x\");\n}\n";
        assert_eq!(
            calls(Language::TypeScript, destructured),
            ["fetch GET 'http://orders/x' unverified"]
        );
    }

    /// Evaluator pass 1 (S9): a `superagent` plugin (`.use`) and `got`/`ky`
    /// interceptor or transport options (`hooks`, `agent`) reconfigure the
    /// request — capped even on the module. Unreadable (by-name) options cap
    /// too.
    #[test]
    fn evaluator_pass1_reconfigured_fetch_class_calls_are_capped() {
        let src = "import got from \"got\";\nimport ky from \"ky\";\nimport request from \"superagent\";\nimport { request as urequest } from \"undici\";\nexport async function f(plugin: any, opts: any) {\n  await request.get(\"http://orders/a\").use(plugin);\n  await got(\"http://orders/b\", { hooks: {} });\n  await got.get(\"http://orders/c\", { agent: {} });\n  await ky.post(\"http://orders/d\", { hooks: {} });\n  await ky(\"http://orders/e\", opts);\n  await urequest(\"http://orders/f\", { dispatcher: {} });\n  await got(\"http://orders/g\", { ...opts });\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "fetch GET 'http://orders/a' unverified",
                "fetch GET 'http://orders/b' unverified",
                "fetch GET 'http://orders/c' unverified",
                "fetch POST 'http://orders/d' unverified",
                "fetch ? 'http://orders/e' unverified",
                "fetch GET 'http://orders/f' unverified",
                "fetch GET 'http://orders/g' unverified",
            ]
        );
    }

    /// Contract M6b S9: instances built by `got.extend`, `ky.extend`,
    /// `ky.create` and `superagent.agent` are configured carriers — their
    /// calls stitch, but never above `heuristic`.
    #[test]
    fn configured_instances_are_capped() {
        let src = "import got from \"got\";\nimport ky from \"ky\";\nimport request from \"superagent\";\nconst api = got.extend({ prefixUrl: \"http://orders\" });\nconst kapi = ky.create({ prefixUrl: \"http://orders\" });\nconst agent = request.agent();\nexport async function f() {\n  await api.get(\"/a\");\n  await kapi.post(\"/b\");\n  await agent.get(\"http://orders/c\");\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "fetch GET '/a' unverified",
                "fetch POST '/b' unverified",
                "fetch GET 'http://orders/c' unverified",
            ]
        );
        // A non-verb on a carrier is no site.
        let other = "import got from \"got\";\nconst api = got.extend({});\nexport async function f() {\n  await api.unknown(\"/a\");\n}\n";
        assert!(calls(Language::TypeScript, other).is_empty());
    }

    /// Contract M6d S7: `requests` and `httpx` enter as fetch-class sites —
    /// module verbs and `request(method, url)` dispatchers behind import
    /// gates, with the same verb allow-list on both. A shadowed or
    /// unimported module name and an unlisted member give nothing.
    #[test]
    fn requests_httpx_module_sites() {
        let src = "import requests\nimport httpx\n\ndef f(URL, METHOD):\n    requests.get(\"http://orders/a\")\n    requests.post(URL, headers=H)\n    requests.request(\"DELETE\", \"http://orders/b\")\n    requests.request(METHOD, URL)\n    httpx.get(\"http://orders/c\")\n    httpx.request(\"post\", \"http://orders/d\")\n    httpx.get(url=\"http://orders/e\")\n";
        assert_eq!(
            calls(Language::Python, src),
            [
                "fetch GET 'http://orders/a'",
                "fetch POST URL",
                "fetch DELETE 'http://orders/b'",
                "fetch ? URL",
                "fetch GET 'http://orders/c'",
                "fetch POST 'http://orders/d'",
                "fetch GET 'http://orders/e'",
            ]
        );
        // The controls: a shadowed module name, an unimported name, an
        // unlisted member, a missing URL and a residue library give nothing.
        let shadowed =
            "import requests\n\ndef f(requests):\n    requests.get(\"http://orders/a\")\n";
        assert!(calls(Language::Python, shadowed).is_empty());
        let plain = "def f():\n    requests.get(\"http://orders/a\")\n";
        assert!(calls(Language::Python, plain).is_empty());
        let other = "import requests\n\ndef f():\n    requests.fetch(\"http://orders/a\")\n    requests.get()\n";
        assert!(calls(Language::Python, other).is_empty());
        let residue =
            "import urllib3\n\ndef f():\n    urllib3.request(\"GET\", \"http://orders/a\")\n";
        assert!(calls(Language::Python, residue).is_empty());
        // A from-imported verb shadowed by a parameter gives nothing either.
        let shadowed_from =
            "from requests import get\n\ndef f(get):\n    get(\"http://orders/a\")\n";
        assert!(calls(Language::Python, shadowed_from).is_empty());
    }

    /// Contract M6d S7: session receivers — a name directly bound to
    /// `requests.Session()`, `httpx.Client(...)` or `httpx.AsyncClient(...)`
    /// — take the same verbs, with `base_url` folded as the base. Every
    /// carrier site caps at `heuristic`, like the configured carriers of the
    /// TS fetch class.
    #[test]
    fn requests_httpx_session_sites_fold_base_url() {
        let src = "import requests\nimport httpx\n\ns = requests.Session()\nc = httpx.Client(base_url=\"http://orders\")\na = httpx.AsyncClient(base_url=BASE)\n\ndef f(URL):\n    s.get(\"http://orders/a\")\n    s.request(\"post\", URL)\n    c.get(\"/b\")\n    c.post(\"/c\", json=J)\n    a.delete(\"/d\")\n";
        assert_eq!(
            calls(Language::Python, src),
            [
                "fetch GET 'http://orders/a' unverified",
                "fetch POST URL unverified",
                "fetch GET '/b' base='http://orders' unverified",
                "fetch POST '/c' base='http://orders' unverified",
                "fetch DELETE '/d' base=BASE unverified",
            ]
        );
    }

    /// Contract M6d S7: aliased modules, from-imported verbs and
    /// constructors, and anonymous inline instances (`Session().post(…)`).
    #[test]
    fn requests_httpx_aliases_from_imports_and_inline_instances() {
        let src = "import requests as rq\nfrom httpx import Client, get as hget\nfrom requests import Session, get as rget\n\ndef f(URL):\n    rq.put(\"http://orders/a\")\n    hget(\"http://orders/b\")\n    rget(URL)\n    Session().post(\"http://orders/c\")\n    Client(base_url=\"http://orders\").delete(\"/d\")\n    rq.api.get(\"http://orders/e\")\n    rq.sessions.Session().head(\"http://orders/f\")\n";
        assert_eq!(
            calls(Language::Python, src),
            [
                "fetch PUT 'http://orders/a'",
                "fetch GET 'http://orders/b'",
                "fetch GET URL",
                "fetch POST 'http://orders/c' unverified",
                "fetch DELETE '/d' base='http://orders' unverified",
                "fetch GET 'http://orders/e'",
                "fetch HEAD 'http://orders/f' unverified",
            ]
        );
    }

    /// Contract M6d S7, evaluator-pass-1 parity: configuration surfaces that
    /// may reroute cap the site. `requests` hooks are response-only, so they
    /// stay plain; `httpx` transports, mounts, event hooks and extensions,
    /// unknown keys and spreads cap — as do all carrier sites. A `url`
    /// keyword reads like a positional one, including on dispatchers.
    #[test]
    fn requests_httpx_reconfigured_calls_are_capped() {
        let src = "import requests\nimport httpx\n\ns = requests.Session()\nc = httpx.Client(base_url=\"http://orders\", transport=T)\nc2 = httpx.Client(base_url=\"http://orders\", unknown_opt=1)\n\ndef f(opts, H, E):\n    requests.get(\"http://orders/a\", hooks=H)\n    requests.get(\"http://orders/b\", unknown_opt=1)\n    requests.get(\"http://orders/c\", **opts)\n    httpx.get(\"http://orders/d\", extensions=E)\n    s.get(\"http://orders/e\")\n    c.get(\"/f\")\n    c2.get(\"/g\")\n    requests.request(\"GET\", url=\"http://orders/h\")\n    httpx.request(method=\"POST\", url=\"http://orders/i\")\n";
        assert_eq!(
            calls(Language::Python, src),
            [
                "fetch GET 'http://orders/a'",
                "fetch GET 'http://orders/b' unverified",
                "fetch GET 'http://orders/c' unverified",
                "fetch GET 'http://orders/d' unverified",
                "fetch GET 'http://orders/e' unverified",
                "fetch GET '/f' base='http://orders' unverified",
                "fetch GET '/g' base='http://orders' unverified",
                "fetch GET 'http://orders/h'",
                "fetch POST 'http://orders/i'",
            ]
        );
    }

    /// Contract M6d S7 boundaries: `send`, `build_request`, header/mount
    /// calls and non-verb members are no sites; a factory-built session, a
    /// member receiver and an unbound `Session` reference wait for S8.
    #[test]
    fn requests_httpx_non_sites_stay_out() {
        let src = "import requests\nimport httpx\n\ns = requests.Session()\nc = httpx.Client(base_url=\"http://orders\")\nplain = requests.Session\nmade = get_session()\n\ndef f(prep, url):\n    c.send(prep)\n    c.build_request(\"GET\", url)\n    s.headers.update({\"a\": \"b\"})\n    s.mount(\"http://\", adapter)\n    s.close()\n    c.unknown(url)\n    plain.get(url)\n    made.get(url)\n    self.session.get(url)\n";
        assert!(calls(Language::Python, src).is_empty());
        // A from-imported constructor shadowed by a rebinding builds nothing.
        let shadowed_ctor = "from httpx import Client\nClient = make_client()\n\ndef f():\n    Client(base_url=\"http://orders\").get(\"/a\")\n";
        assert!(calls(Language::Python, shadowed_ctor).is_empty());
    }

    /// Contract M6d S8: `self.` member receivers — a member bound once to a
    /// session construction — resolve like plain sessions (capped), with the
    /// constructor's `base_url` as the base. Anything else on `self.` stays
    /// out.
    #[test]
    fn self_session_members_resolve() {
        let src = "import requests\nimport httpx\n\nclass C:\n    def __init__(self):\n        self.session = requests.Session()\n        self.client = httpx.Client(base_url=\"http://orders\")\n        self.repo = Repo()\n    def run(self, url):\n        self.session.get(url)\n        self.client.post(\"/a\", json=J)\n        self.repo.get(url)\n";
        assert_eq!(
            calls(Language::Python, src),
            [
                "fetch GET url unverified",
                "fetch POST '/a' base='http://orders' unverified",
            ]
        );
        // A bare annotation plus an assignment resolves to the construction.
        let annotated = "import httpx\n\nclass D:\n    cli: httpx.Client\n    def __init__(self):\n        self.cli = httpx.Client(base_url=\"http://orders\")\n    def run(self):\n        self.cli.get(\"/a\")\n";
        assert_eq!(
            calls(Language::Python, annotated),
            ["fetch GET '/a' base='http://orders' unverified"]
        );
    }

    /// Contract M6d S8: a session-typed parameter or member (FastAPI
    /// `Depends` shape) counts as a capped receiver when its type names a
    /// session constructor under an imported module; without the import, or
    /// untyped, it stays dynamic. Subscripted annotations (`Optional[.]`,
    /// unions) are out of scope — the extractor keeps no Python type
    /// arguments.
    #[test]
    fn session_typed_parameters_count_when_imported() {
        let src = "import httpx\n\nclass C:\n    def run(self, s: httpx.Client, u):\n        s.get(\"http://a\")\n        u.get(\"http://c\")\n";
        assert_eq!(
            calls(Language::Python, src),
            ["fetch GET 'http://a' unverified"]
        );
        let unimported =
            "class C:\n    def run(self, s: httpx.Client):\n        s.get(\"http://a\")\n";
        assert!(calls(Language::Python, unimported).is_empty());
        // A bare class-level annotation (injected elsewhere) resolves too.
        let member_ann = "import httpx\n\nclass C:\n    s: httpx.Client\n    def run(self):\n        self.s.get(\"http://a\")\n";
        assert_eq!(
            calls(Language::Python, member_ann),
            ["fetch GET 'http://a' unverified"]
        );
    }

    /// Contract M6d S8 boundaries: a factory-built or twice-bound member, a
    /// `None` member, a deeper path, an unknown member and a bare name with
    /// only a member present stay dynamic.
    #[test]
    fn ambiguous_members_stay_dynamic() {
        let src = "import requests\nimport httpx\n\ndef make_session():\n    return requests.Session()\n\nclass C:\n    def __init__(self, flag):\n        if flag:\n            self.s = requests.Session()\n        else:\n            self.s = make_session()\n        self.n = None\n    def run(self, url):\n        self.s.get(url)\n        self.n.get(url)\n        self.a.b.get(url)\n        self.missing.get(url)\n        session.get(url)\n";
        assert!(calls(Language::Python, src).is_empty());
        // A member of another class, a foreign receiver and a construction
        // shadowed by an annotation stay out.
        let other = "import requests\nimport httpx\n\nclass A:\n    def __init__(self):\n        self.s = requests.Session()\nclass B:\n    def run(self, url, helper, t):\n        self.s.get(url)\n        helper.session.get(url)\n";
        assert!(calls(Language::Python, other).is_empty());
        let annotated = "import httpx\n\ndef make_client():\n    return httpx.Client()\n\ndef f(url):\n    t: httpx.Client = make_client()\n    t.get(url)\n";
        assert!(calls(Language::Python, annotated).is_empty());
    }

    /// S13: n `requests`/`httpx` call sites (module verbs, session verbs)
    /// cost linear work: 4× the sites makes at most 5× the counted steps.
    /// Counted, not timed, so machine load cannot fail it.
    #[test]
    fn python_client_sites_grow_linearly() {
        let source = |n: usize| {
            let mut src =
                "import requests\nimport httpx\ns = requests.Session()\nc = httpx.Client()\n\ndef f():\n"
                    .to_owned();
            for i in 0..n {
                src.push_str(&format!(
                    "    requests.get(\"http://a/{i}\")\n    s.post(\"http://b/{i}\")\n    c.put(\"http://c/{i}\")\n"
                ));
            }
            src
        };
        let work = |n: usize| {
            let found = calls(Language::Python, &source(n));
            assert_eq!(found.len(), 3 * n);
            crate::work::take()
        };
        let (small, large) = (work(100), work(400));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }

    /// S13: n methods calling `self.` session members cost linear work: 4×
    /// the methods makes at most 5× the counted steps. Counted, not timed.
    #[test]
    fn python_member_sites_grow_linearly() {
        let source = |n: usize| {
            let mut src = "import requests\nimport httpx\n\nclass C:\n    def __init__(self):\n        self.client = httpx.Client(base_url=\"http://orders\")\n        self.session = requests.Session()\n"
                .to_owned();
            for i in 0..n {
                src.push_str(&format!(
                    "    def m{i}(self):\n        self.client.get(\"http://a/{i}\")\n        self.session.post(\"http://b/{i}\")\n"
                ));
            }
            src
        };
        let work = |n: usize| {
            let found = calls(Language::Python, &source(n));
            assert_eq!(found.len(), 2 * n);
            crate::work::take()
        };
        let (small, large) = (work(100), work(400));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }

    /// Contract M6b S9 (D8): Angular `HttpClient` calls enter capped at
    /// `heuristic` — injected (a constructor parameter property or field) or
    /// local, verbs and `request(method, url)`. Without the
    /// `@angular/common/http` import the same shape gives nothing (it may be
    /// the repository's own type of that name).
    #[test]
    fn http_client_sites_are_capped() {
        let src = "import { HttpClient } from \"@angular/common/http\";\nclass S {\n  constructor(private http: HttpClient) {}\n  run() {\n    return this.http.get(\"http://orders/a\");\n  }\n  send(x: string) {\n    return this.http.post(\"http://orders/b\", { x });\n  }\n  remove(id: string) {\n    return this.http.request(\"delete\", \"http://orders/c\");\n  }\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "fetch GET 'http://orders/a' unverified",
                "fetch POST 'http://orders/b' unverified",
                "fetch DELETE 'http://orders/c' unverified",
            ]
        );
        let own = "class HttpClient {\n  get(u: string) { return u; }\n}\nclass S {\n  constructor(private http: HttpClient) {}\n  run() {\n    return this.http.get(\"http://orders/a\");\n  }\n}\n";
        assert!(calls(Language::TypeScript, own).is_empty());
        // An import from the module that does not name `HttpClient` gates
        // nothing out: the type may still be the repository's own.
        let other_import = "import { HttpHeaders } from \"@angular/common/http\";\nclass HttpClient {\n  get(u: string) { return u; }\n}\nclass S {\n  constructor(private http: HttpClient) {}\n  run() {\n    return this.http.get(\"http://orders/a\");\n  }\n}\n";
        assert!(calls(Language::TypeScript, other_import).is_empty());
        // A `request` with no URL is no site; an unknown verb is no site.
        let nourl = "import { HttpClient } from \"@angular/common/http\";\nclass S {\n  constructor(private http: HttpClient) {}\n  run() {\n    return this.http.request(\"delete\");\n  }\n  jsonp(url: string) {\n    return this.http.jsonp(url, \"cb\");\n  }\n}\n";
        assert!(calls(Language::TypeScript, nourl).is_empty());
    }

    /// Contract M6b S9: the axios member forms enter capped at `heuristic` —
    /// destructured verbs (from the module or a `require`), an inline
    /// `require("axios")` chain, and calls through `axios.default`.
    #[test]
    fn axios_member_forms_are_capped() {
        let src = "import axios from \"axios\";\nconst { get } = axios;\nconst { post } = require(\"axios\");\nexport async function f() {\n  await get(\"/a\");\n  await post(\"/b\", { x: 1 });\n  await require(\"axios\").put(\"/c\", { x: 1 });\n  await axios.default.delete(\"/d\");\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "axios GET '/a' unverified",
                "axios POST '/b' unverified",
                "axios PUT '/c' unverified",
                "axios DELETE '/d' unverified",
            ]
        );
        // The control: a destructured verb of another module is no axios member.
        let other = "import { get } from \"./api\";\nexport async function f() {\n  await get(\"/a\");\n}\n";
        assert!(calls(Language::TypeScript, other).is_empty());
        // Evaluator pass 1: a verb destructured from a local instance reads
        // the instance's base (capped: the destructured carrier has no Gate-A
        // proof).
        let inst = "import axios from \"axios\";\nconst api = axios.create({ baseURL: \"http://orders\" });\nconst { get } = api;\nexport async function f() {\n  await get(\"/x\");\n}\n";
        assert_eq!(
            calls(Language::TypeScript, inst),
            ["axios GET '/x' base='http://orders' unverified"]
        );
    }

    /// Contract M6b S9: the repository-wide token scan covers the new
    /// libraries' configuration surfaces. Imports, plain calls and configured
    /// instances (allow-listed, per-site capped) are clean; any other role of
    /// a module name, and any `HttpClient` provider/interceptor use, hooks
    /// `fetch`.
    #[test]
    fn global_hooks_of_new_libraries() {
        use ClientKind::Fetch;
        let hooks = |src: &str| {
            global_hooks(
                Language::TypeScript,
                &extract(Language::TypeScript, src.as_bytes()).unwrap(),
            )
        };
        for clean in [
            "import got from \"got\";\ngot.get(\"/a\");\ngot(\"/b\");\nconst api = got.extend({});\napi.get(\"/c\");\nfor await (const r of got.paginate(\"/d\")) {}\n",
            "import ky from \"ky\";\nimport kyU from \"ky-universal\";\nky.post(\"/a\");\nconst k = ky.create({});\nk.get(\"/b\");\nkyU.get(\"/c\");\n",
            "import request from \"superagent\";\nrequest.get(\"/a\");\nrequest(\"DELETE\", \"/b\");\nconst agent = request.agent();\nagent.get(\"/c\");\n",
            "import { request } from \"undici\";\nimport * as undici from \"undici\";\nrequest(\"/a\");\nundici.request(\"/b\");\n",
            "import { HttpClient } from \"@angular/common/http\";\nclass S {\n  constructor(private http: HttpClient) {}\n  run() {\n    return this.http.get(\"/a\");\n  }\n}\n",
            // A destructured member's object, and a request member read to
            // alias it, reconfigure nothing (contract M6b S9).
            "import axios from \"axios\";\nconst { get } = axios;\nget(\"/a\");\n",
            "import axios from \"axios\";\nconst get = axios.get;\nget(\"/a\");\n",
            "import got from \"got\";\nconst { get } = require(\"got\");\nget(\"/a\");\n",
            // Reading a request member to alias it reconfigures nothing.
            "import got from \"got\";\nconst get = got.get;\nget(\"/a\");\n",
        ] {
            assert!(hooks(clean).is_empty(), "{clean}");
        }
        for tainted in [
            // A bare configuration reference stands in for its use.
            "import got from \"got\";\nconst e = got.extend;\n",
            "const ky = require(\"ky\");\nmodule.exports = { ky };\n",
            // `superagent` re-exported or aliased.
            "import request from \"superagent\";\nexport { request };\n",
            "import kyU from \"ky-universal\";\nexport { kyU };\n",
            // HttpClient providers and interceptors configure every client.
            "import { HTTP_INTERCEPTORS } from \"@angular/common/http\";\nexport const providers = [{ provide: HTTP_INTERCEPTORS, useClass: X, multi: true }];\n",
            "import { HttpInterceptor } from \"@angular/common/http\";\nexport class AuthInterceptor implements HttpInterceptor {}\n",
            "import { provideHttpClient, withInterceptors } from \"@angular/common/http\";\nexport const providers = [provideHttpClient(withInterceptors([x]))];\n",
        ] {
            assert_eq!(hooks(tainted), [Fetch], "{tainted}");
        }
    }

    /// Contract M6b S9: unparsed files taint `fetch` on the new libraries'
    /// words; prose and lockfiles stay inert, and `request` alone (far too
    /// common) never taints.
    #[test]
    fn raw_scan_new_library_words() {
        use ClientKind::Fetch;
        for (path, text) in [
            ("w/x.vue", "import got from 'got'; got.get(u);"),
            ("w/x.svelte", "import ky from 'ky'; ky.post(u);"),
            ("w/x.html", "const request = require('superagent');"),
            ("w/x.js", "const { request } = require('undici');"),
        ] {
            assert_eq!(
                raw_scan(path, Some(text))
                    .hooks
                    .into_iter()
                    .collect::<Vec<_>>(),
                [Fetch],
                "{path}: {text}"
            );
        }
        for (path, text) in [
            ("w/x.js", "request.get(u);"),
            ("README.md", "got ky superagent undici"),
            ("w/package-lock.json", "\"node_modules/got\""),
        ] {
            assert!(
                raw_scan(path, Some(text)).hooks.is_empty(),
                "{path}: {text}"
            );
        }
    }

    /// Contract M5 S19 (b) (M4a open item 22 (c)): XML comments are read, so a hook in a
    /// comment taints, and so does one after a `<!--` inside CDATA (which used to hide it up to
    /// the next `-->`).
    #[test]
    fn xml_comments_and_cdata_are_read() {
        use ClientKind::{Feign, Fetch};
        for (text, want) in [
            (
                "<beans><![CDATA[ <!-- ]]><bean class=\"feign.Client.Default\"/><!-- --></beans>",
                vec![Feign],
            ),
            ("<project><!-- the fetch phase --></project>", vec![Fetch]),
            ("<project><name>x</name></project>", vec![]),
        ] {
            let found: Vec<ClientKind> = raw_scan("p/pom.xml", Some(text))
                .hooks
                .into_iter()
                .collect();
            assert_eq!(found, want, "{text}");
        }
        assert!(
            raw_scan(
                "p/ctx.xml",
                Some("<a><!-- <x>orders.ribbon.listOfServers=a</x> --></a>")
            )
            .discovery
        );
    }

    /// Contract M6b S5: an `axios.create` whose last base-setting entry is
    /// a spread is contested, like a request config; a spread before the
    /// last `baseURL` is shadowed (later properties win in the literal).
    /// `create(cfg)` reads a same-file object or URL it names, and states
    /// no base otherwise (as before).
    #[test]
    fn axios_create_spread_bases() {
        let src = "import axios from \"axios\";\nimport { extOpts } from \"./opts\";\nconst API = \"http://gateway\";\nconst local = { baseURL: \"http://orders\" };\nexport async function f() {\n  const spreadOnly = axios.create({ ...local });\n  const shadowed = axios.create({ ...local, baseURL: \"http://a\" });\n  const overriding = axios.create({ baseURL: \"http://b\", ...local });\n  const imported = axios.create({ ...extOpts });\n  const byName = axios.create(local);\n  const byString = axios.create(API);\n  const byParam = axios.create(cfg);\n  await spreadOnly.get(\"/s\");\n  await shadowed.get(\"/s\");\n  await overriding.get(\"/o\");\n  await imported.get(\"/i\");\n  await byName.get(\"/n\");\n  await byString.get(\"/u\");\n  await byParam.get(\"/p\");\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "axios GET '/s' base=?['http://orders'] unverified",
                "axios GET '/s' base='http://a' unverified",
                "axios GET '/o' base=?['http://b'|'http://orders'] unverified",
                "axios GET '/i' base=?[extOpts] unverified",
                "axios GET '/n' base='http://orders' unverified",
                "axios GET '/u' base=API unverified",
                "axios GET '/p' base=cfg unverified",
            ]
        );
    }

    /// Contract M6b S5: a config passed by name reads the object it names —
    /// `axios(cfg)` parses `url`/`method`/`baseURL` from its entries, and a
    /// request config in an object binding replaces the instance's base. A
    /// name that folds to a URL stays a URL; one that names nothing readable
    /// is dynamic, never suffix-matched on its tail.
    #[test]
    fn axios_by_name_configs() {
        let src = "import axios from \"axios\";\nconst api = axios.create({ baseURL: \"http://orders\" });\nconst API = \"http://gateway/orders/1\";\nconst cfg = { url: \"/x\", method: \"post\", baseURL: \"http://pay\" };\nconst rcfg = { url: \"/r\", method: \"delete\" };\nconst headers = { headers: {} };\nexport async function f(params) {\n  await axios(cfg);\n  await axios(API);\n  await axios(params);\n  await axios.request(rcfg);\n  await axios.request(params);\n  await api.get(\"/h\", headers);\n  await api.get(\"/u\", API);\n  await api.get(\"/p\", params);\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "axios POST '/x' base='http://pay'",
                "axios GET API",
                "axios GET params",
                "axios DELETE '/r'",
                "axios ? ? base=? unverified",
                "axios GET '/h' base='http://orders'",
                "axios GET '/u' base=? unverified",
                "axios GET ? base=? unverified",
            ]
        );
    }

    /// Contract M6b S5: a spread the file cannot read names its value for
    /// the indexer, which folds it through a uniquely resolved declaration
    /// (a same-file object is expanded here as before).
    #[test]
    fn object_spread_names_unreadable_forms() {
        let src = "import axios from \"axios\";\nimport { extOpts } from \"./opts\";\nconst local = { baseURL: \"http://orders\" };\nexport async function f(cfg) {\n  await axios.get(\"/a\", { ...local });\n  await axios.get(\"/b\", { ...extOpts });\n  await axios.get(\"/c\", { ...cfg });\n  await axios.get(\"/d\", { ...missing });\n}\n";
        assert_eq!(
            calls(Language::TypeScript, src),
            [
                "axios GET '/a' base=?['http://orders'] unverified",
                "axios GET '/b' base=?[extOpts] unverified",
                "axios GET '/c' base=?[cfg] unverified",
                "axios GET '/d' base=?[missing] unverified",
            ]
        );
    }

    /// Contract M6b S5: Spring configuring statements are ordered by source
    /// position, later wins. An unreadable statement anywhere keeps the
    /// agreed-or-contested rule (its value could be anywhere).
    #[test]
    fn spring_configuring_statements_later_wins() {
        let two = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.util.DefaultUriBuilderFactory;\nclass C {\n  private final RestTemplate rt = new RestTemplateBuilder().rootUri(\"http://orders\").build();\n  C() { rt.setUriTemplateHandler(new DefaultUriBuilderFactory(\"https://users\")); }\n  String m() { return rt.getForObject(\"/x\", String.class); }\n}\n";
        assert_eq!(
            calls(Language::Java, two),
            ["rest_template GET '/x' base='https://users' unverified"]
        );
        // One statement: as before.
        let one = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nclass C {\n  private final RestTemplate rt = new RestTemplateBuilder().rootUri(\"http://orders\").build();\n  String m() { return rt.getForObject(\"/x\", String.class); }\n}\n";
        assert_eq!(
            calls(Language::Java, one),
            ["rest_template GET '/x' base='http://orders' unverified"]
        );
        // An unreadable statement keeps the contested rule.
        let mixed = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.util.DefaultUriBuilderFactory;\nclass C {\n  private final RestTemplate rt;\n  C(RestTemplateBuilder b) { this.rt = b.build(); }\n  void n() { rt.setUriTemplateHandler(new DefaultUriBuilderFactory(\"https://users\")); }\n  String m() { return rt.getForObject(\"/x\", String.class); }\n}\n";
        assert_eq!(
            calls(Language::Java, mixed),
            ["rest_template GET '/x' base=?['https://users'] unverified"]
        );
    }

    /// Contract M5 S19 (f) (M4a open item 21, part): a base that cannot be decided names the
    /// values the file states it may take. An axios request config whose last base-setting
    /// entry is a spread: the spread's object literal (one held by a binding, followed through
    /// nested spreads) and the `baseURL` before it. Contract M6b S5: a spread nothing here
    /// resolves names its value for the indexer (which folds it through its unique
    /// declaration), and Spring configuring statements are ordered by source position instead
    /// (later wins), so `j07` now reads the setter's base while the contested rule stays for
    /// any unreadable statement.
    #[test]
    fn contested_bases_name_their_candidates() {
        let ts = "import axios from \"axios\";\nconst api10 = axios.create({ baseURL: \"http://orders\" });\nconst stripeOpts = { baseURL: \"https://api.stripe.com\" };\nconst nested = { ...stripeOpts, timeout: 1 };\nconst ordersOpts = { baseURL: \"http://orders\" };\nconst self = { ...self };\nexport async function f(cfg) {\n  await api10.get(\"/orders/1\", { ...stripeOpts });\n  await api10.get(\"/orders/1\", cfg);\n  await api10.get(\"/orders/1\", { baseURL: \"http://orders\", ...stripeOpts });\n  await api10.get(\"/orders/1\", { ...stripeOpts, baseURL: \"http://orders\" });\n  await api10.get(\"/orders/1\", { ...nested });\n  await api10.get(\"/orders/1\", { ...ordersOpts });\n  await api10.get(\"/orders/1\", { ...self });\n  await api10.get(\"/orders/1\", { ...cfg });\n}\n";
        assert_eq!(
            calls(Language::TypeScript, ts),
            [
                "axios GET '/orders/1' base=?['https://api.stripe.com'] unverified",
                "axios GET ? base=? unverified",
                "axios GET '/orders/1' base=?['http://orders'|'https://api.stripe.com'] unverified",
                "axios GET '/orders/1' base='http://orders' unverified",
                "axios GET '/orders/1' base=?['https://api.stripe.com'] unverified",
                "axios GET '/orders/1' base=?['http://orders'] unverified",
                "axios GET '/orders/1' base=?[self] unverified",
                "axios GET '/orders/1' base=?[cfg] unverified",
            ]
        );
        let j07 = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.util.DefaultUriBuilderFactory;\nclass C7 {\n  private final RestTemplate rt7 = new RestTemplateBuilder().rootUri(\"http://orders\").build();\n  C7() { rt7.setUriTemplateHandler(new DefaultUriBuilderFactory(\"https://api.stripe.com\")); }\n  String j07() { return rt7.getForObject(\"/orders/1\", String.class); }\n}\n";
        assert_eq!(
            calls(Language::Java, j07),
            ["rest_template GET '/orders/1' base='https://api.stripe.com' unverified"]
        );
    }

    /// Contract M4a amendment 16: Feign client configuration keys in any configuration file.
    #[test]
    fn feign_client_config_keys() {
        for key in [
            "feign.client.config",
            "feign.client.config.default.request-interceptors[0]",
            "spring.cloud.openfeign.client.config.orders.url",
        ] {
            assert!(feign_config_key(key), "{key}");
        }
        for key in [
            "feign.hystrix.enabled",
            "feign.client.configx",
            "feign.client",
        ] {
            assert!(!feign_config_key(key), "{key}");
        }
    }

    /// Evaluator pass 5, M9: a chain whose base is not stated is `Fresh` only when it starts
    /// from a new client or builder; one from a binding (an injected builder, another
    /// instance) is unreadable, so the binding's configuration is never taken as empty.
    #[test]
    fn chain_base_reads_a_fresh_root() {
        let src = "package p;\nimport org.springframework.web.client.RestTemplate;\nimport org.springframework.boot.web.client.RestTemplateBuilder;\nimport org.springframework.web.client.RestClient;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass C {\n  void m(RestTemplateBuilder b, WebClient w) {\n    RestTemplate a = b.build();\n    RestTemplate c = new RestTemplateBuilder().build();\n    WebClient d = w.mutate().build();\n    WebClient e = WebClient.builder().build();\n    RestTemplate f = new RestTemplateBuilder(cust).build();\n    RestClient g = RestClient.builder(rt).build();\n  }\n}\n";
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
                // A builder from a customizer or a client is no fresh construction (evaluator
                // pass 6, V1).
                ("f".to_owned(), BaseFact::Unreadable),
                ("g".to_owned(), BaseFact::Unreadable),
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

    /// The span of `needle`'s first occurrence, derived from the source, never
    /// hand-counted (single-line needles only; columns are bytes, as extracted).
    fn span_of(src: &str, needle: &str) -> Span {
        let start = src.find(needle).unwrap();
        let row = src[..start].matches('\n').count() as i32;
        let line_start = src[..start].rfind('\n').map_or(0, |i| i + 1);
        let end = start + needle.len();
        assert!(
            !src[start..end].contains('\n'),
            "needle spans lines: {needle}"
        );
        [
            row,
            (start - line_start) as i32,
            row,
            (end - line_start) as i32,
        ]
    }

    /// Spans of the file's `NewRequest` call chains whose request value reaches
    /// an in-process sink (contract M6b S4).
    fn in_process(src: &str) -> BTreeSet<Span> {
        let facts = extract(Language::Go, src.as_bytes()).unwrap();
        clients(Language::Go, &facts).in_process
    }

    /// Contract M6b S4: a request passed to `ServeHTTP` never stitches,
    /// whether the handler is a `HandlerFunc` value (mattermost's API handler
    /// tests) or a mux (its web test). A request the file sends over the
    /// network (`Do`) is no in-process site.
    #[test]
    fn go_in_process_serve_http() {
        let src = "package main\nimport (\n\t\"net/http\"\n\t\"net/http/httptest\"\n)\nfunc outgoing() {\n\treq, err := http.NewRequest(\"GET\", \"/api\", nil)\n\t_ = err\n\thttpRecorder := httptest.NewRecorder()\n\thandler := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {})\n\thandler.ServeHTTP(httpRecorder, req)\n}\nfunc web() {\n\treq, err := http.NewRequest(\"GET\", \"/web\", nil)\n\t_ = err\n\tres := httptest.NewRecorder()\n\trouter.ServeHTTP(res, req)\n\t_ = res\n}\nfunc live(client *http.Client) {\n\treq, err := http.NewRequest(\"GET\", \"http://orders/orders/1\", nil)\n\t_ = err\n\tclient.Do(req)\n}\n";
        // Each site has its own URL, so each span is addressed unambiguously.
        assert_eq!(
            in_process(src),
            BTreeSet::from([
                span_of(src, "http.NewRequest(\"GET\", \"/api\", nil)"),
                span_of(src, "http.NewRequest(\"GET\", \"/web\", nil)"),
            ]),
            "{:?}",
            in_process(src)
        );
    }

    /// Contract M6b S4: a request handed to a handler alongside an
    /// `httptest` recorder never stitches (mattermost's image-proxy tests,
    /// `proxy.GetImage(recorder, request, …)`). A direct call with no
    /// recorder in it is no sink (mattermost's CSRF test passes its request
    /// to a method with no recorder; only its empty URL excludes it).
    #[test]
    fn go_in_process_recorder_co_pass() {
        let src = "package main\nimport (\n\t\"net/http\"\n\t\"net/http/httptest\"\n)\nfunc image() {\n\trecorder := httptest.NewRecorder()\n\trequest, _ := http.NewRequest(http.MethodGet, \"/image.png\", nil)\n\tproxy.GetImage(recorder, request, \"http://mock/image.png\")\n\t_ = recorder\n}\nfunc check() {\n\tr, _ := http.NewRequest(http.MethodGet, \"/csrf\", nil)\n\tif !checkToken(r) {\n\t}\n}\nfunc checkToken(r *http.Request) bool {\n\treturn true\n}\n";
        assert_eq!(
            in_process(src),
            BTreeSet::from([span_of(
                src,
                "http.NewRequest(http.MethodGet, \"/image.png\", nil)"
            )]),
            "{:?}",
            in_process(src)
        );
    }

    /// Contract M6b S4: a request passed to a handler registered on a
    /// `ServeMux` never stitches. Registration is file-global: any direct
    /// call of the registered handler in the file is in-process (the handler
    /// runs in-process with that request). Without the registration the same
    /// direct call is no sink.
    #[test]
    fn go_in_process_mux_registered_handler() {
        let served = "package main\nimport \"net/http\"\nfunc echo(w http.ResponseWriter, r *http.Request) {}\nfunc served() {\n\tmux := http.NewServeMux()\n\tmux.HandleFunc(\"/echo\", echo)\n\treq, _ := http.NewRequest(\"POST\", \"http://orders/echo\", nil)\n\techo(nil, req)\n}\n";
        assert_eq!(
            in_process(served),
            BTreeSet::from([span_of(
                served,
                "http.NewRequest(\"POST\", \"http://orders/echo\", nil)"
            )]),
            "{:?}",
            in_process(served)
        );
        let direct = "package main\nimport \"net/http\"\nfunc echo(w http.ResponseWriter, r *http.Request) {}\nfunc direct() {\n\treq, _ := http.NewRequest(\"POST\", \"http://orders/direct\", nil)\n\techo(nil, req)\n}\n";
        assert!(in_process(direct).is_empty(), "{:?}", in_process(direct));
    }

    /// Contract M6b S4, residue: a request sent both in-process and over the
    /// network keeps its network edge — the sink marks, the `Do` unmarks.
    #[test]
    fn go_in_process_network_send_keeps_the_edge() {
        let src = "package main\nimport \"net/http\"\nfunc both(client *http.Client, mux *http.ServeMux) {\n\treq, _ := http.NewRequest(\"GET\", \"http://orders/orders/1\", nil)\n\tclient.Do(req)\n\tmux.ServeHTTP(nil, req)\n}\n";
        assert!(in_process(src).is_empty(), "{:?}", in_process(src));
    }

    /// Contract M6b S4: the walk follows a request through a same-file
    /// helper (`top` builds, `helper` serves). A shadowed same-named local
    /// and an unrelated same-named string stay out: use sites must resolve
    /// to the request's own binding.
    #[test]
    fn go_in_process_through_a_helper() {
        let src = "package main\nimport \"net/http\"\nfunc helper(req *http.Request) {\n\tmux.ServeHTTP(nil, req)\n}\nfunc top() {\n\treq, _ := http.NewRequest(\"GET\", \"http://orders/x\", nil)\n\thelper(req)\n}\nfunc shadowed() {\n\treq, _ := http.NewRequest(\"GET\", \"http://orders/y\", nil)\n\t_ = req\n}\nfunc other() {\n\treq := \"not a request\"\n\tserve(nil, req)\n}\nfunc serve(w http.ResponseWriter, r *http.Request) {}\n";
        assert_eq!(
            in_process(src),
            BTreeSet::from([span_of(
                src,
                "http.NewRequest(\"GET\", \"http://orders/x\", nil)"
            )]),
            "{:?}",
            in_process(src)
        );
    }

    /// Evaluator pass 1 (S4): a hand-rolled recorder (an own `ResponseWriter`
    /// implementation, not `httptest`) is not a recognised sink — a request
    /// served through it stays stitched.
    #[test]
    fn go_in_process_hand_rolled_recorder_is_not_a_sink() {
        let src = "package main\nimport \"net/http\"\ntype R struct { h http.Header }\nfunc (r *R) Header() http.Header { return r.h }\nfunc (r *R) Write(b []byte) (int, error) { return len(b), nil }\nfunc (r *R) WriteHeader(int) {}\nfunc handler(w http.ResponseWriter, r *http.Request) {}\nfunc t() {\n\treq, _ := http.NewRequest(\"GET\", \"http://orders/x\", nil)\n\trec := &R{}\n\thandler(rec, req)\n}\n";
        assert!(in_process(src).is_empty(), "{:?}", in_process(src));
    }

    /// Contract M6b S4: a request wrapped before naming (`x := wrap(…)`)
    /// is not seeded — the name holds the wrapper's result, not the request
    /// (safe direction: the site stays stitched).
    #[test]
    fn go_in_process_wrapped_requests_are_not_seeded() {
        let src = "package main\nimport \"net/http\"\nfunc wrap(r *http.Request) *http.Request {\n\treturn r\n}\nfunc top() {\n\tx := wrap(http.NewRequest(\"GET\", \"http://orders/x\", nil))\n\tmux.ServeHTTP(nil, x)\n}\n";
        assert!(in_process(src).is_empty(), "{:?}", in_process(src));
    }

    /// Contract M6b S4, invariant 9: n helpers each serving the same request
    /// costs linear work — 4× the helpers makes at most 5× the counted
    /// steps. Counted, not timed, so machine load cannot fail it.
    #[test]
    fn go_in_process_grows_linearly() {
        fn work(n: usize) -> u64 {
            let mut src = "package main\nimport \"net/http\"\n".to_owned();
            for i in 0..n {
                src.push_str(&format!(
                    "func f{i}(req *http.Request) {{ mux.ServeHTTP(nil, req) }}\n"
                ));
            }
            src.push_str(
                "func top() {\n\treq, _ := http.NewRequest(\"GET\", \"http://orders/x\", nil)\n",
            );
            for i in 0..n {
                src.push_str(&format!("\tf{i}(req)\n"));
            }
            src.push_str("}\n");
            let facts = extract(Language::Go, src.as_bytes()).unwrap();
            crate::work::take();
            let found = clients(Language::Go, &facts).in_process;
            assert_eq!(found.len(), 1);
            crate::work::take()
        }
        let (small, large) = (work(100), work(400));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }

    /// Contract M6b S4: mutually delegating helpers terminate (visited set)
    /// in bounded counted work. Without the visited set this input revisits
    /// every level three times per level and blows past the bound (measured:
    /// 124 ops with it, 9,958 without).
    #[test]
    fn go_in_process_cycles_terminate() {
        let src = "package main\nimport \"net/http\"\nfunc a(req *http.Request) {\n\tb(req)\n\tb(req)\n\tb(req)\n}\nfunc b(req *http.Request) {\n\ta(req)\n\ta(req)\n\ta(req)\n}\nfunc top() {\n\treq, _ := http.NewRequest(\"GET\", \"http://orders/x\", nil)\n\ta(req)\n}\n";
        let facts = extract(Language::Go, src.as_bytes()).unwrap();
        crate::work::take();
        let found = clients(Language::Go, &facts).in_process;
        let ops = crate::work::take();
        assert!(found.is_empty(), "{found:?}");
        assert!(ops < 1000, "{ops} counted operations");
    }

    /// `kind operation @carrier` per generated call.
    fn generated(language: Language, src: &str) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        clients(language, &facts)
            .generated
            .iter()
            .map(|c| format!("{} {} @{}", c.kind.as_str(), c.operation, c.carrier))
            .collect()
    }

    /// Contract M6f S6: calls on `*Api` receivers (or `*Api`-typed bindings) are
    /// operationId candidates; anything else is not.
    #[test]
    fn openapi_api_receivers_are_operation_candidates() {
        let src = "import { DefaultApi } from './gen';\nconst usersApi = new DefaultApi();\nexport async function f(id) {\n  await usersApi.listUsers();\n  await new DefaultApi().addUser(id);\n  await api.list();\n  await service.list();\n}\n";
        assert_eq!(
            generated(Language::TypeScript, src),
            ["openapi listUsers @usersApi", "openapi addUser @DefaultApi"]
        );
        let java = "class C {\n  UsersApi api;\n  void f() {\n    api.getUser(1);\n    new UsersApi().addUser(2);\n    service.list();\n  }\n}\n";
        assert_eq!(
            generated(Language::Java, java),
            ["openapi getUser @api", "openapi addUser @UsersApi"]
        );
    }

    /// Contract M6f S6: gRPC stub chains (a `*Grpc` root with 2+ links, or an inline
    /// `new *Stub`) and `New*Client`-typed bindings pin their RPC.
    #[test]
    fn grpc_stub_chains_and_typed_bindings_pin_their_rpc() {
        let java = "class C {\n  void f(Object ch, Object req) {\n    GreeterGrpc.newBlockingStub(ch).sayHello(req);\n    GreeterGrpc.newFutureStub(ch).watch(req);\n    foo.bar(req);\n  }\n}\n";
        assert_eq!(
            generated(Language::Java, java),
            ["grpc sayHello @GreeterGrpc", "grpc watch @GreeterGrpc"]
        );
        let go = "package main\nimport pb \"example.com/helloworld\"\nfunc f(conn Conn, ctx Ctx, req Req) {\n  client, _ := pb.NewGreeterClient(conn)\n  client.SayHello(ctx, req)\n  db.Query(req)\n}\n";
        assert_eq!(generated(Language::Go, go), ["grpc SayHello @Greeter"]);
        let ts = "import { GreeterClient } from './gen';\nexport function f(addr, x) {\n  const client = new GreeterClient(addr);\n  client.sayHello(x);\n  other.ping(x);\n}\n";
        assert_eq!(
            generated(Language::TypeScript, ts),
            ["grpc sayHello @Greeter"]
        );
    }

    /// Contract M7a S1 (stress F19, microservices-demo): a binding is a gRPC client only when a
    /// `New*Client` constructor is the last link of what it holds, so a response
    /// (`resp, _ := pb.NewXClient(c).GetX(…)`) is no client and its getters are no RPCs; the
    /// inline `pb.NewXClient(c).Rpc(…)` chain itself is the RPC call — exactly that two-link
    /// shape, so an SDK's chained constructors (Azure's `….NewContainerClient(c).NewBlobClient(p)`,
    /// found on mattermost) are no RPC.
    #[test]
    fn grpc_go_responses_are_not_clients_and_inline_constructors_are() {
        let go = "package main\nimport pb \"example.com/demo\"\nfunc f(conn Conn, ctx Ctx, req Req) {\n  resp, err := pb.NewCartServiceClient(conn).GetCart(ctx, req)\n  _ = resp.GetItems()\n  var quote = pb.NewShippingServiceClient(conn).GetQuote(ctx, req)\n  _ = quote.GetCostUsd()\n  c, _ := pb.NewAdServiceClient(conn)\n  c.GetAds(ctx, req)\n  _ = err\n  _ = b.client.ServiceClient().NewContainerClient(b.container).Download(ctx)\n  _ = sdk.NewFooClient(c).NewBarClient(d)\n  return b.client.ServiceClient().NewContainerClient(b.container).NewBlobClient(p)\n}\n";
        let mut found = generated(Language::Go, go);
        found.sort();
        assert_eq!(
            found,
            [
                "grpc GetAds @AdService",
                "grpc GetCart @CartService",
                "grpc GetQuote @ShippingService",
            ]
        );
    }

    /// Contract M7c S4 (stress F27, bookcars): a TS/JS `*Client` is a gRPC stub only when it
    /// is imported from a generated-code module; a third-party SDK client
    /// (google-auth-library's `OAuth2Client`) or a locally declared class is not.
    #[test]
    fn ts_grpc_stubs_come_from_generated_modules() {
        let ts = "import { OAuth2Client } from 'google-auth-library';\nimport { GreeterClient } from './proto/greeter_grpc_pb';\nimport { CartClient } from '../gen/cart_pb';\nimport { ShopClient } from '@acme/protos';\nimport { AgentClient } from './agent';\nclass LocalClient {}\nexport function f(x) {\n  const auth = new OAuth2Client();\n  auth.verifyIdToken(x);\n  const g = new GreeterClient('addr');\n  g.sayHello(x);\n  const c = new CartClient('addr');\n  c.getCart(x);\n  const s = new ShopClient('addr');\n  s.list(x);\n  const l = new LocalClient();\n  l.send(x);\n  const a = new AgentClient();\n  a.run(x);\n}\n";
        assert_eq!(
            generated(Language::TypeScript, ts),
            [
                "grpc sayHello @Greeter",
                "grpc getCart @Cart",
                "grpc list @Shop",
            ]
        );
    }

    /// Contract M6f S6, invariant 5: scanning 4× the chains costs at most 5× the operations.
    #[test]
    fn generated_call_scanning_grows_linearly() {
        let work = |n: usize| {
            let mut src = String::from("class C {\n  void f(Object ch) {\n");
            for i in 0..n {
                src.push_str(&format!("    Api{i}Grpc.newStub(ch).m{i}(x);\n"));
                src.push_str(&format!("    plain{i}(x);\n"));
            }
            src.push_str("  }\n}\n");
            let facts = extract(Language::Java, src.as_bytes()).unwrap();
            crate::work::take();
            let out = clients(Language::Java, &facts);
            assert_eq!(out.generated.len(), n);
            crate::work::take()
        };
        let (small, large) = (work(50), work(200));
        assert!(large <= small * 5, "{small} -> {large}");
    }

    /// Evaluator pass 1, M6f-9 (polyglot churn): a chain the URL packs already read is an
    /// HTTP client, whatever its receiver is named — never a second, generated reading.
    #[test]
    fn url_claimed_chains_are_never_generated_candidates() {
        let src = "import got from 'got';\nconst mailApi = got.extend({ prefixUrl: 'http://n' });\nexport async function f(email: string) {\n  return mailApi.post('http://notification/internal/mfa/email', { json: { email } });\n}\n";
        let facts = extract(Language::TypeScript, src.as_bytes()).unwrap();
        let out = clients(Language::TypeScript, &facts);
        assert!(!out.calls.is_empty());
        assert!(out.generated.is_empty());
        let ng = "export class MailClient {\n  constructor(private http: HttpClient) {}\n  send(email: string) {\n    return this.http.post('http://notification/internal/mfa/email', { email });\n  }\n}\n";
        let facts = extract(Language::TypeScript, ng.as_bytes()).unwrap();
        assert!(clients(Language::TypeScript, &facts).generated.is_empty());
    }
    /// A dot import of `net/http` (and of `net/httptest`) is outside the
    /// import gate, as for the client sites themselves (S11): no sink is
    /// recognised there.
    #[test]
    fn go_in_process_needs_qualified_imports() {
        let src = "package main\nimport (\n\t. \"net/http\"\n\t. \"net/http/httptest\"\n)\nfunc t() {\n\treq, _ := NewRequest(\"GET\", \"/x\", nil)\n\trec := NewRecorder()\n\tServeHTTP(rec, req)\n}\n";
        let facts = extract(Language::Go, src.as_bytes()).unwrap();
        let out = clients(Language::Go, &facts);
        assert!(out.calls.is_empty());
        assert!(out.in_process.is_empty());
    }
}
