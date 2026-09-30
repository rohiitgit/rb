//! Names bound to a known type (design §5.4): the receivers whose member calls can be resolved.

use tree_sitter::Node;

use crate::{Language, text};

/// A binding before its scope is mapped to a definition index.
pub(crate) struct RawBinding<'t> {
    pub name: String,
    pub type_name: String,
    pub member: bool,
    /// The declaring node: its enclosing definition is the binding's scope.
    pub node: Node<'t>,
}

/// Every typed binding in the file, in source order.
pub(crate) fn bindings<'t>(
    language: Language,
    root: Node<'t>,
    source: &[u8],
) -> Vec<RawBinding<'t>> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match language {
            Language::Java => java(node, source, &mut out),
            Language::TypeScript | Language::Tsx | Language::JavaScript => {
                ecmascript(node, source, &mut out);
            }
            Language::Python => python(node, source, &mut out),
            Language::Go => go(node, source, &mut out),
        }
        let mut c = node.walk();
        let children: Vec<Node<'t>> = node.children(&mut c).collect();
        stack.extend(children.into_iter().rev());
    }
    out
}

fn push<'t>(
    out: &mut Vec<RawBinding<'t>>,
    name: Option<Node<'t>>,
    type_name: Option<String>,
    member: bool,
    node: Node<'t>,
    source: &[u8],
) {
    // A declaration without a known type is kept (empty `type_name`): it still shadows outer
    // bindings and marks the name as a variable rather than a type.
    let type_name = type_name
        .filter(|t| !BUILTIN_TYPES.contains(&t.as_str()))
        .unwrap_or_default();
    if let Some(name) = name {
        out.push(RawBinding {
            name: text(name, source),
            type_name,
            member,
            node,
        });
    }
}

/// Go and Python built-ins that are spelled like ordinary type names (Java and TypeScript give
/// scalars their own node kinds).
const BUILTIN_TYPES: &[&str] = &[
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
    "float32",
    "float64",
    "complex64",
    "complex128",
    "string",
    "bool",
    "byte",
    "rune",
    "error",
    "any",
    "str",
    "float",
    "bytes",
    "list",
    "dict",
    "set",
    "tuple",
    "object",
    "None",
    "Any",
];

/// A type node as a name: type arguments, pointers and annotations stripped. Built-in scalars,
/// arrays, unions and other composite types give `None`.
fn type_name(node: Node<'_>, source: &[u8]) -> Option<String> {
    match node.kind() {
        "type_annotation" | "type" | "pointer_type" | "parenthesized_type" => {
            type_name(node.named_child(0)?, source)
        }
        "generic_type" => type_name(
            node.child_by_field_name("name")
                .or_else(|| node.named_child(0))?,
            source,
        ),
        "unary_expression" => type_name(node.child_by_field_name("operand")?, source),
        "composite_literal" => type_name(node.child_by_field_name("type")?, source),
        "new_expression" => type_name(node.child_by_field_name("constructor")?, source),
        "object_creation_expression" => type_name(node.child_by_field_name("type")?, source),
        "type_identifier"
        | "identifier"
        | "scoped_type_identifier"
        | "nested_type_identifier"
        | "qualified_type"
        | "scoped_identifier"
        | "attribute"
        | "member_expression" => Some(text(node, source)),
        _ => None,
    }
}

fn java<'t>(node: Node<'t>, source: &[u8], out: &mut Vec<RawBinding<'t>>) {
    let declarators = |n: Node<'t>| {
        let mut c = n.walk();
        n.children_by_field_name("declarator", &mut c)
            .collect::<Vec<_>>()
    };
    match node.kind() {
        "field_declaration" | "local_variable_declaration" => {
            let member = node.kind() == "field_declaration";
            let declared = node.child_by_field_name("type");
            let is_var = declared.is_some_and(|t| text(t, source) == "var");
            for d in declarators(node) {
                let ty = if is_var {
                    d.child_by_field_name("value")
                        .filter(|v| v.kind() == "object_creation_expression")
                        .and_then(|v| type_name(v, source))
                } else {
                    declared.and_then(|t| type_name(t, source))
                };
                push(out, d.child_by_field_name("name"), ty, member, node, source);
            }
        }
        "formal_parameter" | "spread_parameter" => {
            let ty = node
                .child_by_field_name("type")
                .and_then(|t| type_name(t, source));
            push(
                out,
                node.child_by_field_name("name"),
                ty,
                false,
                node,
                source,
            );
        }
        _ => {}
    }
}

fn ecmascript<'t>(node: Node<'t>, source: &[u8], out: &mut Vec<RawBinding<'t>>) {
    match node.kind() {
        // JavaScript parameters (TypeScript wraps them in `required_parameter`).
        "formal_parameters" => {
            let mut c = node.walk();
            for p in node.named_children(&mut c) {
                let name = match p.kind() {
                    "identifier" => Some(p),
                    "assignment_pattern" => p
                        .child_by_field_name("left")
                        .filter(|l| l.kind() == "identifier"),
                    _ => None,
                };
                if name.is_some() {
                    push(out, name, None, false, p, source);
                }
            }
        }
        "public_field_definition" | "field_definition" => {
            let ty = node
                .child_by_field_name("type")
                .or_else(|| node.child_by_field_name("value"))
                .and_then(|t| type_name(t, source));
            let name = node
                .child_by_field_name("name")
                .or_else(|| node.child_by_field_name("property"));
            push(out, name, ty, true, node, source);
        }
        "required_parameter" | "optional_parameter" => {
            let mut c = node.walk();
            // `constructor(private repo: Repo)` also declares the member `this.repo`.
            let member = node
                .children(&mut c)
                .any(|n| matches!(n.kind(), "accessibility_modifier" | "readonly"));
            let ty = node
                .child_by_field_name("type")
                .and_then(|t| type_name(t, source));
            let name = node
                .child_by_field_name("pattern")
                .filter(|p| p.kind() == "identifier");
            push(out, name, ty, member, node, source);
        }
        "variable_declarator" => {
            let ty = node
                .child_by_field_name("type")
                .and_then(|t| type_name(t, source))
                .or_else(|| {
                    node.child_by_field_name("value")
                        .filter(|v| v.kind() == "new_expression")
                        .and_then(|v| type_name(v, source))
                });
            let name = node
                .child_by_field_name("name")
                .filter(|n| n.kind() == "identifier");
            push(out, name, ty, false, node, source);
        }
        // `this.x = new T()`
        "assignment_expression" => {
            let Some(left) = node
                .child_by_field_name("left")
                .filter(|l| l.kind() == "member_expression")
            else {
                return;
            };
            let on_this = left
                .child_by_field_name("object")
                .is_some_and(|o| o.kind() == "this");
            let ty = node
                .child_by_field_name("right")
                .filter(|v| v.kind() == "new_expression")
                .and_then(|v| type_name(v, source));
            if on_this {
                push(
                    out,
                    left.child_by_field_name("property"),
                    ty,
                    true,
                    node,
                    source,
                );
            }
        }
        _ => {}
    }
}

fn python<'t>(node: Node<'t>, source: &[u8], out: &mut Vec<RawBinding<'t>>) {
    match node.kind() {
        "parameters" | "lambda_parameters" => {
            let mut c = node.walk();
            for p in node.named_children(&mut c) {
                let ty = p
                    .child_by_field_name("type")
                    .and_then(|t| type_name(t, source));
                let name = match p.kind() {
                    "identifier" => Some(p),
                    "default_parameter" | "typed_default_parameter" => {
                        p.child_by_field_name("name")
                    }
                    "typed_parameter" => {
                        let mut pc = p.walk();
                        p.named_children(&mut pc).find(|n| n.kind() == "identifier")
                    }
                    _ => None,
                };
                // `self`/`cls` are handled as the enclosing class.
                let name = name.filter(|n| !matches!(text(*n, source).as_str(), "self" | "cls"));
                if name.is_some() {
                    push(out, name, ty, false, p, source);
                }
            }
        }
        "assignment" => {
            let Some(left) = node.child_by_field_name("left") else {
                return;
            };
            // `x: T = …` names its type; `x = T(…)` constructs one (the resolver keeps it only
            // when `T` resolves to a class).
            let ty = node
                .child_by_field_name("type")
                .and_then(|t| type_name(t, source))
                .or_else(|| {
                    node.child_by_field_name("right")
                        .filter(|r| r.kind() == "call")
                        .and_then(|r| r.child_by_field_name("function"))
                        .and_then(|f| type_name(f, source))
                });
            match left.kind() {
                "identifier" => push(out, Some(left), ty, false, node, source),
                "attribute"
                    if left
                        .child_by_field_name("object")
                        .is_some_and(|o| text(o, source) == "self") =>
                {
                    push(
                        out,
                        left.child_by_field_name("attribute"),
                        ty,
                        true,
                        node,
                        source,
                    );
                }
                _ => {}
            }
        }
        _ => {}
    }
}

fn go<'t>(node: Node<'t>, source: &[u8], out: &mut Vec<RawBinding<'t>>) {
    let names = |n: Node<'t>| {
        let mut c = n.walk();
        n.children_by_field_name("name", &mut c).collect::<Vec<_>>()
    };
    match node.kind() {
        "var_spec" | "parameter_declaration" | "field_declaration" => {
            let ty = node
                .child_by_field_name("type")
                .and_then(|t| type_name(t, source));
            let member = node.kind() == "field_declaration";
            for name in names(node) {
                push(out, Some(name), ty.clone(), member, node, source);
            }
        }
        "short_var_declaration" => {
            let (Some(left), Some(right)) = (
                node.child_by_field_name("left"),
                node.child_by_field_name("right"),
            ) else {
                return;
            };
            let mut lc = left.walk();
            let mut rc = right.walk();
            for (name, value) in left
                .named_children(&mut lc)
                .zip(right.named_children(&mut rc))
            {
                let is_literal = match value.kind() {
                    "composite_literal" => true,
                    "unary_expression" => value
                        .child_by_field_name("operand")
                        .is_some_and(|o| o.kind() == "composite_literal"),
                    _ => false,
                };
                let ty = is_literal.then(|| type_name(value, source)).flatten();
                push(
                    out,
                    Some(name).filter(|n| n.kind() == "identifier"),
                    ty,
                    false,
                    node,
                    source,
                );
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::{Language, extract};

    /// `name: Type in scope` (`*` marks a member), in source order.
    fn bound(language: Language, src: &str) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        facts
            .bindings
            .iter()
            .map(|b| {
                let scope = b
                    .scope
                    .map_or("-".to_owned(), |i| facts.definitions[i].name.clone());
                let star = if b.member { "*" } else { "" };
                let ty = if b.type_name.is_empty() {
                    "?"
                } else {
                    &b.type_name
                };
                format!("{star}{}: {ty} in {scope}", b.name)
            })
            .collect()
    }

    #[test]
    fn java_fields_parameters_and_locals() {
        let src = "class K { private Repo repo; private List<User> users; int n; void f(User u, int i) { Store s = x(); var t = new Thing(); var v = x(); a.b.C c; } }\n";
        assert_eq!(
            bound(Language::Java, src),
            [
                "*repo: Repo in K",
                "*users: List in K",
                "*n: ? in K",
                "u: User in f",
                "i: ? in f",
                "s: Store in f",
                "t: Thing in f",
                "v: ? in f",
                "c: a.b.C in f"
            ]
        );
    }

    #[test]
    fn typescript_fields_parameter_properties_and_variables() {
        let src = "class K { repo: UserRepo; n: number; constructor(private svc: Svc, x: number) { this.cache = new Cache(); } f(u: User) { const s: Store = mk(); const n = new Thing(); let m = mk(); } }\nconst top = new Top();\n";
        assert_eq!(
            bound(Language::TypeScript, src),
            [
                "*repo: UserRepo in K",
                "*n: ? in K",
                "*svc: Svc in K",
                // TypeScript's tags define no `constructor` symbol: its parameters sit in the class.
                "x: ? in K",
                "*cache: Cache in K",
                "u: User in f",
                "s: Store in f",
                "n: Thing in f",
                "m: ? in f",
                "top: Top in -"
            ]
        );
        assert_eq!(
            bound(
                Language::JavaScript,
                "function f(a, b = 1) { const t = new Thing(); }\n"
            ),
            ["a: ? in f", "b: ? in f", "t: Thing in f"]
        );
    }

    #[test]
    fn python_annotations_constructions_and_self_members() {
        let src = "class K:\n    def f(self, u: User, v, w: int = 1):\n        self.repo = Repo()\n        s: Store = mk()\n        t = Thing()\n        n = 3\n";
        assert_eq!(
            bound(Language::Python, src),
            [
                "u: User in f",
                "v: ? in f",
                "w: ? in f",
                "*repo: Repo in K",
                "s: Store in f",
                "t: Thing in f",
                "n: ? in f"
            ]
        );
    }

    #[test]
    fn go_vars_parameters_fields_and_literals() {
        let src = "package p\ntype Server struct { repo Repo; n int }\nfunc (s *Server) Run(r Req, a, b int) { var st Store; t := Thing{}; p := &Thing{}; q := NewThing() }\n";
        assert_eq!(
            bound(Language::Go, src),
            [
                "*repo: Repo in Server",
                "*n: ? in Server",
                "s: Server in Run",
                "r: Req in Run",
                "a: ? in Run",
                "b: ? in Run",
                "st: Store in Run",
                "t: Thing in Run",
                "p: Thing in Run",
                "q: ? in Run"
            ]
        );
    }
}
