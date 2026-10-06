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
}

/// An axios instance bound to a name: `const api = axios.create({ baseURL })`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AxiosInstance {
    pub name: String,
    pub base: Option<UrlValue>,
    pub top_level: bool,
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
            axios(facts, &mut out);
        }
        Language::Java => {
            rest_template(facts, &mut out);
            web_client(facts, &mut out);
            feign(facts, &mut out);
        }
        Language::Go => go_http(facts, &mut out),
        Language::Python => {}
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

fn fetch(facts: &FileScope<'_>, out: &mut FileClients) {
    // An import or a top-level definition named `fetch` shadows it everywhere in the file.
    let file_wide =
        facts.imports.iter().any(|i| {
            i.alias.as_deref() == Some("fetch") || i.names.iter().any(|n| n.local == "fetch")
        }) || facts
            .definitions
            .iter()
            .any(|d| d.name == "fetch" && d.parent.is_none());
    let shadowed = |at: Span| file_wide || facts.binding_at("fetch", at).is_some();
    for chain in &facts.chains {
        let global = match chain.root.as_deref() {
            None => true,
            Some("window" | "globalThis" | "self") => true,
            Some(_) => false,
        };
        let Some(link) = chain.links.first().filter(|l| l.name == "fetch") else {
            continue;
        };
        if !global || link.args.is_empty() || shadowed(chain.span) {
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
        });
    }
}

// ---------------------------------------------------------------------------------------------
// axios (S9)

const AXIOS_VERBS: &[&str] = &["get", "post", "put", "delete", "patch", "head", "options"];

/// Local names of the `axios` module: default or namespace import, or a top-level
/// `require("axios")`.
fn axios_names(facts: &FileFacts) -> Vec<String> {
    let mut names: Vec<String> = facts
        .imports
        .iter()
        .filter(|i| i.module == "axios")
        .flat_map(|i| {
            i.alias.iter().cloned().chain(
                i.names
                    .iter()
                    .filter(|n| n.name == "default")
                    .map(|n| n.local.clone()),
            )
        })
        .collect();
    for b in facts
        .bindings
        .iter()
        .filter(|b| b.scope.is_none() && !b.member)
    {
        if let Some(c) = init_chain(facts, b)
            && c.root.is_none()
            && c.links.len() == 1
            && c.links[0].name == "require"
            && c.links[0].args.first().and_then(literal).as_deref() == Some("axios")
        {
            names.push(b.name.clone());
        }
    }
    names
}

fn init_chain<'f>(facts: &'f FileFacts, b: &Binding) -> Option<&'f CallChain> {
    match b.init {
        Some(Init::Call { chain }) => facts.chains.get(chain),
        _ => None,
    }
}

fn axios(facts: &FileScope<'_>, out: &mut FileClients) {
    let names = axios_names(facts);
    // Instances: `const api = axios.create({ baseURL })`.
    let mut instances: HashMap<usize, Option<UrlValue>> = HashMap::new();
    for (bi, b) in facts.bindings.iter().enumerate().filter(|(_, b)| !b.member) {
        let Some(c) = init_chain(facts, b) else {
            continue;
        };
        if let (Some(root), [link]) = (c.root.as_deref(), c.links.as_slice())
            && names.iter().any(|n| n == root)
            && link.name == "create"
        {
            let base = entry(link.args.first(), "baseURL").map(|a| UrlValue::of(Some(a)));
            out.axios_instances.push(AxiosInstance {
                name: b.name.clone(),
                base: base.clone(),
                top_level: b.scope.is_none(),
            });
            instances.insert(bi, base);
        }
    }
    let imported: HashSet<&str> = facts
        .imports
        .iter()
        .filter(|i| i.module != "axios")
        .flat_map(|i| i.names.iter().map(|n| n.local.as_str()))
        .collect();
    for chain in &facts.chains {
        let Some(first) = chain.links.first() else {
            continue;
        };
        // `axios(url | config)`.
        if chain.root.is_none() {
            if names.contains(&first.name) && !first.args.is_empty() {
                out.calls.push(axios_call(
                    chain,
                    None,
                    None,
                    first.args.first(),
                    first.args.get(1),
                ));
            }
            continue;
        }
        let root = chain.root.as_deref().unwrap_or_default();
        let bound = facts.binding_index_at(root, chain.span);
        let (base, instance) = if names.iter().any(|n| n == root) {
            (None, None)
        } else if let Some(base) = bound.and_then(|bi| instances.get(&bi)) {
            (base.clone(), None)
        } else if bound.is_none() && imported.contains(root) {
            (None, Some(root.to_owned()))
        } else {
            continue;
        };
        let verb = first.name.as_str();
        if AXIOS_VERBS.contains(&verb) && !first.args.is_empty() {
            let mut call = axios_call(chain, base, instance, None, None);
            call.method = Some(norm_method(verb));
            call.url = UrlValue::of(first.args.first());
            out.calls.push(call);
        } else if verb == "request" && !first.args.is_empty() {
            out.calls
                .push(axios_call(chain, base, instance, first.args.first(), None));
        }
    }
}

/// `axios(url, config?)`, `axios(config)` or `request(config)`: the URL and method come from the
/// first argument or the config's `url` and `method` (default GET).
fn axios_call(
    chain: &CallChain,
    base: Option<UrlValue>,
    instance: Option<String>,
    first: Option<&Arg>,
    second: Option<&Arg>,
) -> ClientCall {
    let (url, config) = match first {
        Some(config @ Arg::Object { .. }) => {
            (UrlValue::of(entry(Some(config), "url")), Some(config))
        }
        other => (UrlValue::of(other), second),
    };
    let base = entry(config, "baseURL")
        .map(|a| UrlValue::of(Some(a)))
        .or(base);
    ClientCall {
        kind: ClientKind::Axios,
        method: method_entry(config).unwrap_or(Some("GET".to_owned())),
        url,
        base,
        instance,
        span: chain.span,
        source: chain.enclosing,
    }
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

/// The binding a Java receiver (`x`, `this.x`) names at `at`.
fn java_receiver<'f>(facts: &FileScope<'f>, root: &str, at: Span) -> Option<&'f Binding> {
    local_binding(facts, Language::Java, root, at).map(|i| &facts.facts.bindings[i])
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

fn rest_template(facts: &FileScope<'_>, out: &mut FileClients) {
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
        let is_client = java_receiver(facts, root, chain.span).is_some_and(|b| {
            java_type_is(
                facts,
                &b.type_name,
                "org.springframework.web.client",
                "RestTemplate",
            )
        });
        if !is_client || first.args.is_empty() {
            continue;
        }
        out.calls.push(ClientCall {
            kind: ClientKind::RestTemplate,
            method,
            url: UrlValue::of(first.args.first()),
            base: None,
            instance: None,
            span: chain.span,
            source: chain.enclosing,
        });
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

/// A base from a `baseUrl(x)` or `create(x)` link.
fn fluent_base(chain: &CallChain, upto: usize) -> Option<UrlValue> {
    chain.links[..upto]
        .iter()
        .rev()
        .find(|l| matches!(l.name.as_str(), "baseUrl" | "create") && !l.args.is_empty())
        .map(|l| UrlValue::of(l.args.first()))
}

fn web_client(facts: &FileScope<'_>, out: &mut FileClients) {
    for chain in &facts.chains {
        let Some(root) = chain.root.as_deref() else {
            continue;
        };
        let receiver = java_receiver(facts, root, chain.span);
        let kind = match receiver {
            Some(b) => fluent_kind(facts, &b.type_name),
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
                |l| UrlValue::of(l.args.first()),
            );
        let base = fluent_base(chain, verb).or_else(|| {
            let init = receiver.and_then(|b| init_chain(facts, b))?;
            fluent_base(init, init.links.len())
        });
        out.calls.push(ClientCall {
            kind,
            method,
            url,
            base,
            instance: None,
            span: chain.span,
            source: chain.enclosing,
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
                .and_then(|fmt| sprintf(facts, fmt, *span))
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
        });
    }
}

/// `fmt.Sprintf(lit, args…)` at `span` as a template whose verbs are holes naming their
/// arguments (`%s %d %v %q` and the rest; `%%` is a literal `%`).
fn sprintf(facts: &FileScope<'_>, fmt: &str, span: Span) -> Option<StrTemplate> {
    let chain = facts.chain_at(span)?;
    let [link] = chain.links.as_slice() else {
        return None;
    };
    if chain.root.as_deref() != Some(fmt) || link.name != "Sprintf" {
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
        let imported = "import fetch from \"node-fetch\";\nfetch(\"/x\");\n";
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
            }]
        );
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
                "rest_template GET 'http://visits/owners/{id}'",
                "rest_template PUT '{BASE}/x'",
                "rest_template ? '/y'",
                "web_client GET '/owners/{ownerId}' base='http://customers-service'",
                "web_client POST 'http://visits-service/pets/visits'",
                "rest_client DELETE '/r'",
                "web_client GET '' base='http://x'",
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
