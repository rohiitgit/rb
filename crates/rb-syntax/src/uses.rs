//! Uses of names that may hold a configured object (contract M4a amendment 13): where a name
//! that a call initialises (an HTTP client instance, a builder) appears other than as the
//! receiver of a call, a callee or a declaration. A pack decides from these, with the call
//! chains rooted at the name, whether what the declaration configured can be changed later:
//! `api.defaults.baseURL = x` reads `api`'s member, `use(api)` passes it on, `api = other`
//! reassigns it. Nothing here knows a framework.

use std::collections::HashSet;

use tree_sitter::Node;

use crate::{Binding, CallChain, Import, Init, Language, NameUse, span, text};

/// The names whose uses are recorded: names a binding initialises with a call, targets of an
/// assignment from a call, the receivers of those calls (`b` in `b.baseUrl(x).build()`), and
/// (TS/JS) imported names and namespace aliases. `this.x` counts as `x`.
pub(crate) fn candidates(
    language: Language,
    bindings: &[Binding],
    chains: &[CallChain],
    imports: &[Import],
) -> HashSet<String> {
    let mut out = HashSet::new();
    if !records(language) {
        return out;
    }
    let mut add = |name: &str| {
        let name = name.strip_prefix("this.").unwrap_or(name);
        if !name.is_empty() && !name.contains(['.', '(', ' ']) {
            out.insert(name.to_owned());
        }
    };
    let construction = |c: &CallChain, add: &mut dyn FnMut(&str)| {
        if let Some(root) = &c.root {
            add(root);
        }
    };
    for b in bindings {
        if let Some(Init::Call { chain }) = b.init {
            add(&b.name);
            if let Some(c) = chains.get(chain) {
                construction(c, &mut add);
            }
        }
    }
    for c in chains {
        if let Some(target) = &c.assigned {
            add(target);
            construction(c, &mut add);
        }
    }
    if language != Language::Java {
        for i in imports {
            for n in &i.names {
                add(&n.local);
            }
            if let Some(alias) = &i.alias {
                add(alias);
            }
        }
    }
    out
}

fn records(language: Language) -> bool {
    matches!(
        language,
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Java
    )
}

/// Every use of a candidate name, in source order.
pub(crate) fn uses(
    language: Language,
    root: Node<'_>,
    source: &[u8],
    candidates: &HashSet<String>,
) -> Vec<NameUse> {
    let mut out = Vec::new();
    if candidates.is_empty() || !records(language) {
        return out;
    }
    let java = language == Language::Java;
    let mut stack = vec![root];
    let mut cursor = root.walk();
    while let Some(node) = stack.pop() {
        let name = match node.kind() {
            "identifier" | "shorthand_property_identifier" => Some(text(node, source)),
            // `this.x`: the member `x` of the enclosing object.
            "member_expression" | "field_access" => this_member(node, source),
            _ => None,
        };
        if let Some(name) = name
            && candidates.contains(name.strip_prefix("this.").unwrap_or(&name))
            && let Some(write) = role(java, node)
        {
            out.push(NameUse {
                name,
                span: span(node),
                write,
            });
        }
        let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    out
}

/// `this.x` as written, for a member access on `this`.
fn this_member(node: Node<'_>, source: &[u8]) -> Option<String> {
    let object = node.child_by_field_name("object")?;
    let member = node
        .child_by_field_name("property")
        .or_else(|| node.child_by_field_name("field"))?;
    (object.kind() == "this").then(|| format!("this.{}", text(member, source)))
}

fn is_field(parent: Node<'_>, field: &str, node: Node<'_>) -> bool {
    parent
        .child_by_field_name(field)
        .is_some_and(|c| c.id() == node.id())
}

/// How `node` uses its name: `None` when it is not a use (a declaration, an import, a call's
/// receiver or callee, a member name), else whether it is written.
fn role(java: bool, node: Node<'_>) -> Option<bool> {
    let parent = node.parent()?;
    let kind = parent.kind();
    // Declared, imported or labelled here; a member's own name.
    if is_field(parent, "name", node)
        || is_field(parent, "alias", node)
        || is_field(parent, "key", node)
        || is_field(parent, "property", node)
        || is_field(parent, "field", node)
        || matches!(
            kind,
            "import_clause"
                | "namespace_import"
                | "import_specifier"
                | "export_specifier"
                | "labeled_statement"
                | "break_statement"
                | "continue_statement"
                | "type_query"
                | "nested_identifier"
                | "formal_parameters"
                | "inferred_parameters"
                | "scoped_identifier"
                | "import_declaration"
                | "package_declaration"
        )
        || (kind == "catch_clause" && is_field(parent, "parameter", node))
    {
        return None;
    }
    if java {
        // The receiver of a method call, or a lambda's parameter.
        if (kind == "method_invocation" && is_field(parent, "object", node))
            || (kind == "lambda_expression" && is_field(parent, "parameters", node))
        {
            return None;
        }
        return Some(match kind {
            "assignment_expression" => is_field(parent, "left", node),
            "update_expression" => true,
            _ => false,
        });
    }
    // TS/JS: a callee, or the object of a called member (`api.get(…)`).
    let called = |n: Node<'_>| {
        n.parent()
            .is_some_and(|p| p.kind() == "call_expression" && is_field(p, "function", n))
    };
    if called(node)
        || (kind == "new_expression" && is_field(parent, "constructor", node))
        || (kind == "member_expression" && is_field(parent, "object", node) && called(parent))
        || (matches!(kind, "required_parameter" | "optional_parameter")
            && is_field(parent, "pattern", node))
        || (kind == "arrow_function" && is_field(parent, "parameter", node))
    {
        return None;
    }
    // A destructuring pattern: a target when assigned, a declaration otherwise.
    let mut child = node;
    let mut up = parent;
    while matches!(
        up.kind(),
        "array_pattern" | "object_pattern" | "pair_pattern" | "rest_pattern" | "assignment_pattern"
    ) {
        // `{ a = dflt }`: the default is read, not bound.
        if up.kind() == "assignment_pattern" && is_field(up, "right", child) {
            return Some(false);
        }
        child = up;
        up = up.parent()?;
    }
    let target = |p: Node<'_>, c: Node<'_>| match p.kind() {
        "assignment_expression" | "augmented_assignment_expression" | "for_in_statement" => {
            is_field(p, "left", c)
        }
        "update_expression" => true,
        _ => false,
    };
    if child.id() != node.id() {
        // Inside a pattern: written when the pattern is assigned to, else declared.
        return target(up, child).then_some(true);
    }
    Some(target(parent, node))
}

#[cfg(test)]
mod tests {
    use crate::{Language, extract};

    /// `name@row write?` per recorded use.
    fn uses(language: Language, src: &str) -> Vec<String> {
        extract(language, src.as_bytes())
            .unwrap()
            .uses
            .iter()
            .map(|u| {
                format!(
                    "{}@{}{}",
                    u.name,
                    u.span[0],
                    if u.write { " write" } else { "" }
                )
            })
            .collect()
    }

    /// Contract M4a amendment 13: every use of a name a call initialises (or an imported name)
    /// that is not a call's receiver, a callee or a declaration is recorded, with writes marked.
    #[test]
    fn uses_of_configured_names() {
        let ts = "import axios from \"axios\";\nimport { shared } from \"./client\";\nimport * as ns from \"./ns\";\nconst api = axios.create({ baseURL: \"http://orders\" });\nlet api3 = axios.create({});\napi.get(\"/a\");\napi(\"/b\");\napi.defaults.baseURL = \"https://x\";\napi.interceptors.request.use((c) => c);\nuse(api);\nconst o = { api, ...api };\nexport default api;\napi3 = other;\n[api3] = list;\nshared.defaults.x = 1;\nshared.get(\"/c\");\nns.shared.get(\"/d\");\nfunction f(api) { return api; }\nconst g = (api3) => 1;\nconst plain = 1;\nuse(plain);\nclass K { constructor() { this.api = axios.create(); } m() { this.api.get(\"/e\"); keep(this.api); } }\n";
        assert_eq!(
            uses(Language::TypeScript, ts),
            [
                "api@7",
                "api@8",
                "api@9",
                "api@10",
                "api@10",
                "api@11",
                "api3@12 write",
                "api3@13 write",
                "shared@14",
                "ns@16",
                "api@17",
                "this.api@21 write",
                "this.api@21",
            ]
        );
        let java = "package p;\nimport org.springframework.web.reactive.function.client.WebClient;\nclass C {\n  private final WebClient.Builder b1 = WebClient.builder().baseUrl(\"http://orders\");\n  private final WebClient web;\n  private int n;\n  C(WebClient.Builder wb) {\n    this.web = wb.baseUrl(\"http://a\").build();\n    b1.baseUrl(\"https://x\");\n    register(b1);\n    register(this.web);\n    n = 1;\n  }\n  void m() { b1.build().get().uri(\"/x\").retrieve(); web.get(); web = null; Runnable r = () -> keep(wb); }\n}\n";
        assert_eq!(
            uses(Language::Java, java),
            [
                "this.web@7 write",
                "b1@9",
                "this.web@10",
                "web@13 write",
                "wb@13",
            ]
        );
        // Go and Python record nothing.
        assert!(
            uses(
                Language::Go,
                "package m\nvar c = mk()\nfunc f() { use(c) }\n"
            )
            .is_empty()
        );
    }
}
