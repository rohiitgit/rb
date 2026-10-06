//! Every token of a name that may hold a configured object (contract M4a amendment 14): where a
//! name that a call initialises (an HTTP client instance, a builder) or that a call is assigned
//! to appears anywhere in the file, as its declaration, as the receiver of a member call, or
//! otherwise. Name-based and scope-insensitive on purpose: a pack scans every token of the name
//! in a range (an enclosing function, or the whole file) and proves nothing a shadowing name
//! could break. Module objects and a few configuration-surface names are recorded too, so a pack
//! can see a process-wide hook (`axios.defaults…`, `(globalThis as any).fetch = …`, Go's
//! `http.DefaultTransport = …`, a Feign `RequestInterceptor`; contract M4a amendments 15 and
//! 16). Beyond [`SURFACE_NAMES`], nothing here knows a framework.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::{Binding, CallChain, Import, Language, NameToken, TokenRole, span, text};

/// The names through which a module client's process-wide configuration is reached (contract
/// M4a amendment 16): recorded in every position, also as a property, field or type name, so
/// a pack can require every one of them to be in an allow-listed role.
pub fn surface_names(language: Language) -> &'static [&'static str] {
    match language {
        Language::TypeScript | Language::Tsx | Language::JavaScript => SURFACE_NAMES[0],
        Language::Go => SURFACE_NAMES[1],
        Language::Java => SURFACE_NAMES[2],
        Language::Python => &[],
    }
}

/// [`surface_names`] per language: TS/JS, Go, Java (Feign's extension points that may
/// retarget a request, and the `feign` package name of a qualified use).
const SURFACE_NAMES: [&[&str]; 3] = [
    &["fetch"],
    &["DefaultTransport", "DefaultClient"],
    &[
        "RequestInterceptor",
        "Capability",
        "FeignBuilderCustomizer",
        "Targeter",
        "Feign",
        "feign",
    ],
];

/// The names whose tokens are recorded: names a binding (a field or member too) initialises
/// with a call or a Java `new`, and simple targets of an assignment from a call (TypeScript,
/// JavaScript and Java); the local names of TS/JS default and namespace imports of a package
/// (not a relative path); and the language's [`surface_names`].
pub(crate) fn candidates(
    language: Language,
    bindings: &[Binding],
    chains: &[CallChain],
    imports: &[Import],
) -> HashSet<String> {
    let mut out: HashSet<String> = surface_names(language)
        .iter()
        .map(|n| (*n).to_owned())
        .collect();
    if !records(language) || language == Language::Go {
        return out;
    }
    let mut add = |name: &str| {
        if !name.is_empty() && !name.contains(['.', '(', ' ', '/']) {
            out.insert(name.to_owned());
        }
    };
    if language != Language::Java {
        for i in imports.iter().filter(|i| !i.module.starts_with(['.', '/'])) {
            if let Some(a) = &i.alias {
                add(a);
            }
            for n in i.names.iter().filter(|n| n.name == "default") {
                add(&n.local);
            }
        }
    }
    for b in bindings {
        if matches!(
            b.init,
            Some(crate::Init::Call { .. } | crate::Init::New { .. })
        ) {
            add(&b.name);
        }
    }
    for target in chains.iter().filter_map(|c| c.assigned.as_deref()) {
        add(target);
    }
    out
}

fn records(language: Language) -> bool {
    matches!(
        language,
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Java | Language::Go
    )
}

/// Every token of a candidate name, in source order.
pub(crate) fn tokens(
    language: Language,
    root: Node<'_>,
    source: &[u8],
    candidates: &HashSet<String>,
) -> Vec<NameToken> {
    let mut out = Vec::new();
    if candidates.is_empty() || !records(language) {
        return out;
    }
    let surface = surface_names(language);
    let mut stack = vec![root];
    let mut cursor = root.walk();
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        let named = matches!(
            kind,
            "identifier"
                | "shorthand_property_identifier"
                | "shorthand_property_identifier_pattern"
        );
        // A surface name also as a property, field or type name (amendment 16).
        let surface_only = matches!(
            kind,
            "property_identifier" | "field_identifier" | "type_identifier"
        );
        if named || surface_only {
            let name = text(node, source);
            if candidates.contains(&name) && (named || surface.contains(&name.as_str())) {
                out.push(NameToken {
                    role: role(language, node, source),
                    name,
                    span: span(node),
                });
            }
        }
        let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    out
}

fn is_field(parent: Node<'_>, field: &str, node: Node<'_>) -> bool {
    parent
        .child_by_field_name(field)
        .is_some_and(|c| c.id() == node.id())
}

/// What one token is: the name of a TS/JS `const`/`let` declarator that is not exported, or of
/// a Java local variable declarator; the callee of a TS/JS call; the receiver of a member call
/// (no optional chaining); the object of a member access that is not called (TS/JS `x.m`, Go
/// `x.M`, read or written, also continued: `x.m.n = …`); anything else. A property or field
/// name is judged as the member access it names (`(globalThis as any).fetch = …` is an
/// assignment target, `window.fetch(…)` a callee); a type name, or a property name that names
/// no member access (an object key, a method definition), is anything else.
fn role(language: Language, node: Node<'_>, source: &[u8]) -> TokenRole {
    let Some(parent) = node.parent() else {
        return TokenRole::Other;
    };
    if matches!(node.kind(), "property_identifier" | "field_identifier") {
        let named = matches!(parent.kind(), "member_expression" | "selector_expression")
            && (is_field(parent, "property", node) || is_field(parent, "field", node));
        return if named {
            role(language, parent, source)
        } else {
            TokenRole::Other
        };
    }
    if node.kind() == "type_identifier" {
        return TokenRole::Other;
    }
    let java = language == Language::Java;
    if language == Language::Go {
        if parent.kind() != "selector_expression" || !is_field(parent, "operand", node) {
            return TokenRole::Other;
        }
        let Some(field) = parent.child_by_field_name("field") else {
            return TokenRole::Other;
        };
        let called = parent
            .parent()
            .is_some_and(|c| c.kind() == "call_expression" && is_field(c, "function", parent));
        let name = text(field, source);
        return if called {
            TokenRole::Receiver { call: name }
        } else {
            TokenRole::Member { name }
        };
    }
    if parent.kind() == "variable_declarator" && is_field(parent, "name", node) {
        let declaration = parent.parent();
        let declared = if java {
            declaration.is_some_and(|d| d.kind() == "local_variable_declaration")
        } else {
            declaration.is_some_and(|d| {
                d.kind() == "lexical_declaration"
                    && d.parent().is_none_or(|p| p.kind() != "export_statement")
            })
        };
        return if declared {
            TokenRole::Declaration
        } else {
            TokenRole::Other
        };
    }
    let call = if java {
        (parent.kind() == "method_invocation" && is_field(parent, "object", node))
            .then(|| parent.child_by_field_name("name"))
            .flatten()
    } else {
        let optional = |n: Node<'_>| {
            let mut cursor = n.walk();
            n.children(&mut cursor)
                .any(|c| matches!(c.kind(), "optional_chain" | "?."))
        };
        if parent.kind() == "call_expression" && is_field(parent, "function", node) {
            return TokenRole::Callee;
        }
        if parent.kind() != "member_expression"
            || !is_field(parent, "object", node)
            || optional(parent)
        {
            return TokenRole::Other;
        }
        let Some(property) = parent.child_by_field_name("property") else {
            return TokenRole::Other;
        };
        match parent
            .parent()
            .filter(|c| c.kind() == "call_expression" && is_field(*c, "function", parent))
        {
            Some(call) if optional(call) => None,
            Some(_) => Some(property),
            None => {
                return TokenRole::Member {
                    name: text(property, source),
                };
            }
        }
    };
    match call {
        Some(name) => TokenRole::Receiver {
            call: text(name, source),
        },
        None => TokenRole::Other,
    }
}

#[cfg(test)]
mod tests {
    use crate::{Language, TokenRole, extract};

    /// `name@row role` per recorded token.
    fn tokens(language: Language, src: &str) -> Vec<String> {
        extract(language, src.as_bytes())
            .unwrap()
            .name_tokens
            .iter()
            .map(|t| {
                let role = match &t.role {
                    TokenRole::Declaration => "decl".to_owned(),
                    TokenRole::Callee => "callee".to_owned(),
                    TokenRole::Receiver { call } => format!(".{call}"),
                    TokenRole::Member { name } => format!("~{name}"),
                    TokenRole::Other => "other".to_owned(),
                };
                format!("{}@{} {role}", t.name, t.span[0])
            })
            .collect()
    }

    /// Contract M4a amendment 14: every token of a name a call initialises (or a call is
    /// assigned to) is recorded wherever it is, before its declaration and in nested functions
    /// too, as a declaration (a non-exported `const`/`let`, a Java local), the receiver of a
    /// member call, a TS/JS callee (amendment 16), or something else (an export, a member
    /// read, optional chaining, a write, an argument, a shorthand property, a parameter, a
    /// field).
    #[test]
    fn name_tokens_of_configured_names() {
        let ts = "import axios from \"axios\";\nfunction setup() { api.defaults.baseURL = \"x\"; }\nconst api = axios.create({ baseURL: \"http://orders\" });\napi.get(\"/a\");\napi?.get(\"/b\");\napi.get?.(\"/c\");\napi(\"/d\");\nexport { api as ordersApi };\nexport const ex = axios.create();\nlet lv = axios.create();\nlv = other;\nvar vv = axios.create();\nmodule.exports = { api };\nconst { api: a2 } = x;\nfunction f(api) { return api[\"defaults\"]; }\nconst plain = 1;\nuse(plain);\n";
        assert_eq!(
            tokens(Language::TypeScript, ts),
            // The `axios` import is a module object (amendment 15); a member write is a
            // `member` token.
            [
                "axios@0 other",
                "api@1 ~defaults",
                "api@2 decl",
                "axios@2 .create",
                "api@3 .get",
                "api@4 other",
                "api@5 other",
                "api@6 callee",
                "api@7 other",
                "ex@8 other",
                "axios@8 .create",
                "lv@9 decl",
                "axios@9 .create",
                "lv@10 other",
                "vv@11 other",
                "axios@11 .create",
                "api@12 other",
                "api@14 other",
                "api@14 other",
            ]
        );
        let java = "package p;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass C {\n  private final WebClient web = WebClient.create(\"http://a\");\n  void m(C other) {\n    WebClient c = WebClient.create(\"http://b\");\n    c.get().uri(\"/x\").retrieve();\n    this.web.get();\n    other.web.mutate();\n    Runnable r = () -> keep(c);\n    c = null;\n    java.util.function.Supplier<?> s = c::get;\n  }\n}\n";
        assert_eq!(
            tokens(Language::Java, java),
            // A field's declarator, `this.web` and `other.web` are no declaration: a field is
            // never verified. Any identifier counts, a package name's too.
            [
                "web@1 other",
                "web@3 other",
                "c@5 decl",
                "c@6 .get",
                "web@7 other",
                "web@8 other",
                "c@9 other",
                "c@10 other",
                "c@11 other",
            ]
        );
        // Python records nothing; Go records only its imports' names (below).
        assert!(
            tokens(
                Language::Go,
                "package m\nvar c = mk()\nfunc f() { use(c) }\n"
            )
            .is_empty()
        );
        assert!(tokens(Language::Python, "import requests\nc = requests.get(u)\n").is_empty());
    }

    /// Contract M4a amendments 15 and 16: the tokens of module objects (a TS/JS default or
    /// namespace import of a package, never of a relative path) are recorded, and so are the
    /// configuration-surface names ([`SURFACE_NAMES`]: `fetch`; Go's `DefaultTransport` and
    /// `DefaultClient`; Feign's extension points and the `feign` package name) in every position, also as a property,
    /// field or type name, which is judged as the member access it names. A member access that
    /// is not called is a `member` token naming its first member, and a callee a `callee` token,
    /// so a pack can tell a process-wide hook (`axios.defaults.baseURL = …`, `(globalThis as
    /// any).fetch = …`, `fetch = wrap(fetch)`, `http.DefaultTransport = …`) from a request
    /// (`axios.get(…)`, `axios(…)`, `window.fetch(…)`, `http.DefaultClient.Get(…)`).
    #[test]
    fn module_and_surface_name_tokens() {
        let ts = "import axios from \"axios\";\nimport * as ns from \"ky\";\nimport local from \"./local\";\nimport { named } from \"pkg\";\naxios.defaults.baseURL = \"x\";\naxios.interceptors.request.use(f);\naxios.get(\"/a\");\naxios(\"/b\");\nglobalThis.fetch = f;\nwindow.fetch(\"/c\");\nglobal.fetch.bind(null);\nself?.fetch;\nns.x;\nlocal.y;\nnamed.z;\n(globalThis as any).fetch = jest.fn();\nfetch = wrap(fetch);\nfetch(\"/d\");\nconst o = { fetch, k: typeof fetch };\nclass A { fetch() {} }\n";
        assert_eq!(
            tokens(Language::TypeScript, ts),
            [
                "axios@0 other",
                "ns@1 other",
                "axios@4 ~defaults",
                "axios@5 ~interceptors",
                "axios@6 .get",
                "axios@7 callee",
                "fetch@8 other",
                "fetch@9 callee",
                "fetch@10 .bind",
                "fetch@11 other",
                "ns@12 ~x",
                "fetch@15 other",
                "fetch@16 other",
                "fetch@16 other",
                "fetch@17 callee",
                "fetch@18 other",
                "fetch@18 other",
                "fetch@19 other",
            ]
        );
        let go = "package main\nimport (\n\t\"net/http\"\n\tf \"fmt\"\n\t_ \"embed\"\n)\nfunc main() {\n\thttp.DefaultTransport = t\n\thttp.DefaultClient.Transport = t\n\thttp.DefaultClient.Do(req)\n\thttp.Get(\"/a\")\n\tf.Println(http.StatusOK)\n\t_ = f.Sprint()\n\tDefaultClient.Get(\"/b\")\n\tc := &http.DefaultClient\n}\n";
        assert_eq!(
            tokens(Language::Go, go),
            [
                "DefaultTransport@7 other",
                "DefaultClient@8 ~Transport",
                "DefaultClient@9 .Do",
                "DefaultClient@13 .Get",
                "DefaultClient@14 other",
            ]
        );
        let java = "package p;\nimport feign.RequestInterceptor;\nclass C {\n  @Bean RequestInterceptor rewrite() { return t -> t.target(\"x\"); }\n  static class R implements feign.RequestInterceptor {}\n  @Bean feign.Client client() { return null; }\n}\n";
        assert_eq!(
            tokens(Language::Java, java),
            [
                "feign@1 other",
                "RequestInterceptor@1 other",
                "RequestInterceptor@3 other",
                "feign@4 other",
                "RequestInterceptor@4 other",
                "feign@5 other",
            ]
        );
    }
}
