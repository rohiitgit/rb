//! Every token of a name that may hold a configured object (contract M4a amendment 14): where a
//! name that a call initialises (an HTTP client instance, a builder) or that a call is assigned
//! to appears anywhere in the file, as its declaration, as the receiver of a member call, or
//! otherwise. Name-based and scope-insensitive on purpose: a pack scans every token of the name
//! in a range (an enclosing function, or the whole file) and proves nothing a shadowing name
//! could break. Nothing here knows a framework.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::{Binding, CallChain, Language, NameToken, TokenRole, span, text};

/// The names whose tokens are recorded: names a binding (a field or member too) initialises
/// with a call, and simple targets of an assignment from a call. TypeScript, JavaScript and
/// Java only.
pub(crate) fn candidates(
    language: Language,
    bindings: &[Binding],
    chains: &[CallChain],
) -> HashSet<String> {
    let mut out = HashSet::new();
    if !records(language) {
        return out;
    }
    let mut add = |name: &str| {
        if !name.is_empty() && !name.contains(['.', '(', ' ']) {
            out.insert(name.to_owned());
        }
    };
    for b in bindings {
        if matches!(b.init, Some(crate::Init::Call { .. })) {
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
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Java
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
    let java = language == Language::Java;
    let mut stack = vec![root];
    let mut cursor = root.walk();
    while let Some(node) = stack.pop() {
        if matches!(
            node.kind(),
            "identifier"
                | "shorthand_property_identifier"
                | "shorthand_property_identifier_pattern"
        ) {
            let name = text(node, source);
            if candidates.contains(&name) {
                out.push(NameToken {
                    role: role(java, node, source),
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
/// a Java local variable declarator; the receiver of a member call (no optional chaining);
/// anything else.
fn role(java: bool, node: Node<'_>, source: &[u8]) -> TokenRole {
    let Some(parent) = node.parent() else {
        return TokenRole::Other;
    };
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
        parent
            .parent()
            .filter(|call| {
                parent.kind() == "member_expression"
                    && is_field(parent, "object", node)
                    && !optional(parent)
                    && call.kind() == "call_expression"
                    && is_field(*call, "function", parent)
                    && !optional(*call)
            })
            .and_then(|_| parent.child_by_field_name("property"))
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
                    TokenRole::Receiver { call } => format!(".{call}"),
                    TokenRole::Other => "other".to_owned(),
                };
                format!("{}@{} {role}", t.name, t.span[0])
            })
            .collect()
    }

    /// Contract M4a amendment 14: every token of a name a call initialises (or a call is
    /// assigned to) is recorded wherever it is, before its declaration and in nested functions
    /// too, as a declaration (a non-exported `const`/`let`, a Java local), the receiver of a
    /// member call, or something else (an export, a member read, optional chaining, a write,
    /// an argument, a shorthand property, a parameter, a field).
    #[test]
    fn name_tokens_of_configured_names() {
        let ts = "import axios from \"axios\";\nfunction setup() { api.defaults.baseURL = \"x\"; }\nconst api = axios.create({ baseURL: \"http://orders\" });\napi.get(\"/a\");\napi?.get(\"/b\");\napi.get?.(\"/c\");\napi(\"/d\");\nexport { api as ordersApi };\nexport const ex = axios.create();\nlet lv = axios.create();\nlv = other;\nvar vv = axios.create();\nmodule.exports = { api };\nconst { api: a2 } = x;\nfunction f(api) { return api[\"defaults\"]; }\nconst plain = 1;\nuse(plain);\n";
        assert_eq!(
            tokens(Language::TypeScript, ts),
            [
                "api@1 other",
                "api@2 decl",
                "api@3 .get",
                "api@4 other",
                "api@5 other",
                "api@6 other",
                "api@7 other",
                "ex@8 other",
                "lv@9 decl",
                "lv@10 other",
                "vv@11 other",
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
        // Go and Python record nothing.
        assert!(
            tokens(
                Language::Go,
                "package m\nvar c = mk()\nfunc f() { use(c) }\n"
            )
            .is_empty()
        );
    }
}
