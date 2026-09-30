//! Names declared in code (design §5.4): the receivers whose member calls can be resolved, and the
//! extent over which each declaration is in effect.

use tree_sitter::Node;

use crate::{Language, text};

/// A binding before its scope is mapped to a definition index.
pub(crate) struct RawBinding<'t> {
    pub name: String,
    pub type_name: String,
    pub member: bool,
    /// The declaring node: its enclosing definition is the binding's scope.
    pub node: Node<'t>,
    /// The binding is in effect from the end of `from` to the end of `to`.
    pub from: Node<'t>,
    pub to: Node<'t>,
    /// An explicit scope, for names whose scope is not the nearest scope kind (`except … as e`).
    pub scope: Option<Node<'t>>,
}

/// Every binding in the file, in source order.
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
    for b in &mut out {
        b.to = b.scope.unwrap_or_else(|| scope_of(language, b.from));
    }
    out
}

/// Node kinds that end a declaration's visibility: the nearest such ancestor of the declaring
/// node. Python names are function-scoped; blocks are listed for the other languages.
fn scope_kinds(language: Language) -> &'static [&'static str] {
    match language {
        Language::Python => &[
            "module",
            "function_definition",
            "lambda",
            "class_definition",
            "list_comprehension",
            "set_comprehension",
            "dictionary_comprehension",
            "generator_expression",
        ],
        Language::Go => &[
            "source_file",
            "block",
            "func_literal",
            "function_declaration",
            "method_declaration",
            "for_statement",
            "if_statement",
            "expression_switch_statement",
            "type_switch_statement",
            "select_statement",
            "expression_case",
            "type_case",
            "default_case",
            "communication_case",
            // Parameter names of function types and interface methods end with their signature.
            "function_type",
            "interface_type",
            "method_elem",
        ],
        Language::TypeScript | Language::Tsx | Language::JavaScript => &[
            "program",
            "statement_block",
            "class_body",
            "function_declaration",
            "function_expression",
            "generator_function",
            "generator_function_declaration",
            "method_definition",
            "arrow_function",
            "for_statement",
            "for_in_statement",
            "catch_clause",
            // Parameter names of type-level signatures end with the signature.
            "function_type",
            "constructor_type",
            "function_signature",
            "method_signature",
            "abstract_method_signature",
            "call_signature",
            "construct_signature",
            "interface_body",
            "object_type",
        ],
        Language::Java => &[
            "program",
            "block",
            "class_body",
            "method_declaration",
            "constructor_declaration",
            "lambda_expression",
            "enhanced_for_statement",
            "for_statement",
            "try_with_resources_statement",
            "catch_clause",
            "if_statement",
            "switch_block",
        ],
    }
}

fn scope_of<'t>(language: Language, node: Node<'t>) -> Node<'t> {
    let kinds = scope_kinds(language);
    let mut cur = node;
    while let Some(p) = cur.parent() {
        if kinds.contains(&p.kind()) {
            return p;
        }
        cur = p;
    }
    cur
}

/// Records `name` (when present), visible from the end of `from`.
fn push<'t>(
    out: &mut Vec<RawBinding<'t>>,
    name: Option<Node<'t>>,
    type_name: Option<String>,
    member: bool,
    node: Node<'t>,
    from: Node<'t>,
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
            from,
            to: node,
            scope: None,
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
        "type_annotation" | "type" | "pointer_type" | "parenthesized_type" | "catch_type" => {
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

/// Every identifier inside a destructuring pattern (`{ a, b: c }`, `[x, y]`, `a, b`).
fn pattern_names<'t>(node: Node<'t>, out: &mut Vec<Node<'t>>) {
    match node.kind() {
        "identifier" | "shorthand_property_identifier_pattern" => out.push(node),
        "pair_pattern" => {
            if let Some(v) = node.child_by_field_name("value") {
                pattern_names(v, out);
            }
        }
        _ => {
            let mut c = node.walk();
            for child in node.named_children(&mut c) {
                pattern_names(child, out);
            }
        }
    }
}

fn java<'t>(node: Node<'t>, source: &[u8], out: &mut Vec<RawBinding<'t>>) {
    let declarators = |n: Node<'t>| {
        let mut c = n.walk();
        n.children_by_field_name("declarator", &mut c)
            .collect::<Vec<_>>()
    };
    let typed = |n: Node<'t>| {
        n.child_by_field_name("type")
            .and_then(|t| type_name(t, source))
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
                push(
                    out,
                    d.child_by_field_name("name"),
                    ty,
                    member,
                    node,
                    d,
                    source,
                );
            }
        }
        "formal_parameter" | "spread_parameter" | "catch_formal_parameter" | "resource" => {
            let ty = typed(node).or_else(|| {
                let mut c = node.walk();
                node.named_children(&mut c)
                    .find(|n| n.kind() == "catch_type")
                    .and_then(|t| type_name(t, source))
            });
            push(
                out,
                node.child_by_field_name("name"),
                ty,
                false,
                node,
                node,
                source,
            );
        }
        "enhanced_for_statement" => {
            let name = node.child_by_field_name("name");
            push(
                out,
                name,
                typed(node),
                false,
                node,
                name.unwrap_or(node),
                source,
            );
        }
        "instanceof_expression" => {
            let ty = node
                .child_by_field_name("right")
                .and_then(|t| type_name(t, source));
            push(
                out,
                node.child_by_field_name("name"),
                ty,
                false,
                node,
                node,
                source,
            );
        }
        "lambda_expression" => {
            if let Some(params) = node.child_by_field_name("parameters") {
                let mut names = Vec::new();
                match params.kind() {
                    "identifier" => names.push(params),
                    "inferred_parameters" => pattern_names(params, &mut names),
                    _ => {}
                }
                for n in names {
                    push(out, Some(n), None, false, node, params, source);
                }
            }
        }
        _ => {}
    }
}

fn ecmascript<'t>(node: Node<'t>, source: &[u8], out: &mut Vec<RawBinding<'t>>) {
    match node.kind() {
        "public_field_definition" | "field_definition" => {
            let ty = node
                .child_by_field_name("type")
                .or_else(|| node.child_by_field_name("value"))
                .and_then(|t| type_name(t, source));
            let name = node
                .child_by_field_name("name")
                .or_else(|| node.child_by_field_name("property"));
            push(out, name, ty, true, node, node, source);
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
            if let Some(pattern) = node.child_by_field_name("pattern") {
                let mut names = Vec::new();
                pattern_names(pattern, &mut names);
                for n in names {
                    push(out, Some(n), ty.clone(), member, node, node, source);
                }
            }
        }
        // JavaScript parameters (TypeScript wraps them in `required_parameter`).
        "formal_parameters" => {
            let mut c = node.walk();
            for p in node.named_children(&mut c) {
                if matches!(p.kind(), "required_parameter" | "optional_parameter") {
                    continue;
                }
                let mut names = Vec::new();
                pattern_names(p, &mut names);
                for n in names {
                    push(out, Some(n), None, false, p, node, source);
                }
            }
        }
        // `x => …`: a single unparenthesised parameter.
        "arrow_function" => {
            if let Some(p) = node
                .child_by_field_name("parameter")
                .filter(|p| p.kind() == "identifier")
            {
                push(out, Some(p), None, false, node, p, source);
            }
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
            if let Some(name) = node.child_by_field_name("name") {
                let mut names = Vec::new();
                pattern_names(name, &mut names);
                // Only a plain name takes the declared or constructed type.
                let ty = ty.filter(|_| name.kind() == "identifier");
                for n in names {
                    push(out, Some(n), ty.clone(), false, node, node, source);
                }
            }
        }
        "for_in_statement" => {
            if let Some(left) = node.child_by_field_name("left") {
                let mut names = Vec::new();
                pattern_names(left, &mut names);
                for n in names {
                    push(out, Some(n), None, false, node, left, source);
                }
            }
        }
        "catch_clause" => {
            if let Some(p) = node.child_by_field_name("parameter") {
                let mut names = Vec::new();
                pattern_names(p, &mut names);
                for n in names {
                    push(out, Some(n), None, false, node, p, source);
                }
            }
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
                    "list_splat_pattern" | "dictionary_splat_pattern" => p.named_child(0),
                    _ => None,
                };
                // `self`/`cls` are handled as the enclosing class.
                let name = name.filter(|n| !matches!(text(*n, source).as_str(), "self" | "cls"));
                push(out, name, ty, false, p, node, source);
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
                "identifier" => push(out, Some(left), ty, false, node, node, source),
                "pattern_list" | "tuple_pattern" | "list_pattern" => {
                    let mut names = Vec::new();
                    pattern_names(left, &mut names);
                    for n in names {
                        push(out, Some(n), None, false, node, node, source);
                    }
                }
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
                        node,
                        source,
                    );
                }
                _ => {}
            }
        }
        // `for x in …`, `[… for x in …]`. A comprehension's body precedes its `for`, so its
        // variable is in effect from the comprehension's opening bracket.
        "for_statement" | "for_in_clause" => {
            if let Some(left) = node.child_by_field_name("left") {
                let from = if node.kind() == "for_in_clause" {
                    node.parent().and_then(|c| c.child(0)).unwrap_or(left)
                } else {
                    left
                };
                let mut names = Vec::new();
                pattern_names(left, &mut names);
                for n in names {
                    push(out, Some(n), None, false, node, from, source);
                }
            }
        }
        // `with … as x` (function-scoped), `except E as x` (deleted after the clause).
        "as_pattern" => {
            if let Some(target) = node.child_by_field_name("alias") {
                let except = node.parent().filter(|p| p.kind() == "except_clause");
                let mut names = Vec::new();
                pattern_names(target, &mut names);
                for n in names {
                    push(out, Some(n), None, false, node, node, source);
                    if let (Some(e), Some(b)) = (except, out.last_mut()) {
                        b.scope = Some(e);
                    }
                }
            }
        }
        // `(x := …)`
        "named_expression" => {
            push(
                out,
                node.child_by_field_name("name"),
                None,
                false,
                node,
                node,
                source,
            );
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
                push(out, Some(name), ty.clone(), member, node, node, source);
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
            let lefts: Vec<Node<'t>> = left.named_children(&mut lc).collect();
            let rights: Vec<Node<'t>> = right.named_children(&mut rc).collect();
            for (i, name) in lefts.iter().enumerate() {
                // A composite literal types its name only in a one-to-one assignment.
                let ty = rights
                    .get(i)
                    .filter(|_| lefts.len() == rights.len())
                    .and_then(|value| {
                        let is_literal = match value.kind() {
                            "composite_literal" => true,
                            "unary_expression" => value
                                .child_by_field_name("operand")
                                .is_some_and(|o| o.kind() == "composite_literal"),
                            _ => false,
                        };
                        is_literal.then(|| type_name(*value, source)).flatten()
                    });
                let name = Some(*name).filter(|n| n.kind() == "identifier");
                push(out, name, ty, false, node, node, source);
            }
        }
        // `for k, v := range …`
        "range_clause" => {
            if let Some(left) = node.child_by_field_name("left") {
                let mut names = Vec::new();
                pattern_names(left, &mut names);
                for n in names {
                    push(out, Some(n), None, false, node, node, source);
                }
            }
        }
        // `switch x := v.(type)`
        "type_switch_statement" => {
            if let Some(alias) = node.child_by_field_name("alias") {
                let mut names = Vec::new();
                pattern_names(alias, &mut names);
                for n in names {
                    push(out, Some(n), None, false, node, alias, source);
                }
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

    /// `name [l0:c0-l1:c1]`: where each non-member binding is in effect.
    fn visible(language: Language, src: &str) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        facts
            .bindings
            .iter()
            .filter(|b| !b.member)
            .map(|b| {
                let v = b.visible;
                format!("{} [{}:{}-{}:{}]", b.name, v[0], v[1], v[2], v[3])
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
    fn java_loop_lambda_resource_catch_and_pattern_locals() {
        let src = "class K { void m(List<Repo> xs) { for (Repo repo : xs) {} xs.forEach(repo -> repo.save()); xs.forEach((a, b) -> a.go()); try (Repo r = new Repo()) {} catch (RuntimeException e) {} if (o instanceof Repo p) {} } }\n";
        assert_eq!(
            bound(Language::Java, src),
            [
                "xs: List in m",
                "repo: Repo in m",
                "repo: ? in m",
                "a: ? in m",
                "b: ? in m",
                "r: Repo in m",
                "e: RuntimeException in m",
                "p: Repo in m"
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
    fn typescript_loops_destructuring_arrows_and_catch() {
        let src = "function f() { for (const item of items) {} const { client, deep: { inner } } = o; const [a, b] = arr; items.map(item => item.go()); items.map(({ x }) => x); try {} catch (err) {} }\n";
        assert_eq!(
            bound(Language::TypeScript, src),
            [
                "item: ? in f",
                "client: ? in f",
                "inner: ? in f",
                "a: ? in f",
                "b: ? in f",
                "item: ? in f",
                "x: ? in f",
                "err: ? in f"
            ]
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
    fn python_loops_with_except_comprehensions_tuples_walrus_and_lambdas() {
        let src = "def f():\n    for user in users:\n        pass\n    with open() as conn:\n        pass\n    try:\n        pass\n    except Exception as err:\n        pass\n    x = [item.run() for item in items]\n    a, conn2 = 1, 2\n    if (user2 := get()):\n        pass\n    g = lambda q, *rest: q\n";
        assert_eq!(
            bound(Language::Python, src),
            [
                "user: ? in f",
                "conn: ? in f",
                "err: ? in f",
                "x: ? in f",
                "item: ? in f",
                "a: ? in f",
                "conn2: ? in f",
                "user2: ? in f",
                "g: ? in f",
                "q: ? in f",
                "rest: ? in f"
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

    #[test]
    fn go_ranges_multi_assignment_type_switches_and_closures() {
        let src = "package p\nfunc f() {\n  for _, user := range users { }\n  a, client := g()\n  x, y := Thing{}, Thing{}\n  switch v := x.(type) { case int: }\n  go func(c int) { c := c.Copy() }()\n  if v, ok := m[k]; ok { }\n}\n";
        assert_eq!(
            bound(Language::Go, src),
            [
                "_: ? in f",
                "user: ? in f",
                "a: ? in f",
                "client: ? in f",
                "x: Thing in f",
                "y: Thing in f",
                "v: ? in f",
                "c: ? in f",
                "c: ? in f",
                "v: ? in f",
                "ok: ? in f"
            ]
        );
    }

    #[test]
    fn visibility_extents_follow_scopes() {
        // Go: a range variable ends with its loop; a closure's own `c` ends with the closure and
        // starts only after its initialiser, so `c.Copy()` still sees the parameter.
        let go = "package p\nfunc H(c *Ctx) {\n\tfor _, u := range us {\n\t}\n\tgo func() { c := c.Copy() }()\n\tc.Next()\n}\n";
        assert_eq!(
            visible(Language::Go, go),
            [
                "c [1:13-6:1]",
                "_ [2:21-3:2]",
                "u [2:21-3:2]",
                "c [4:26-4:28]"
            ]
        );
        // TypeScript: block-scoped `const`, an arrow parameter ends with the arrow function.
        let ts = "function f(repo: Repo) {\n  if (x) { const repo = 1 }\n  items.map(item => item.go());\n  repo.save();\n}\n";
        assert_eq!(
            visible(Language::TypeScript, ts),
            ["repo [0:21-4:1]", "repo [1:25-1:27]", "item [2:16-2:29]"]
        );
        // Python: function-scoped names; a comprehension variable ends with the comprehension;
        // an `except … as e` name ends with its clause.
        let py = "def f(a):\n    xs = [i for i in a]\n    try:\n        pass\n    except E as e:\n        pass\n    b = 1\n";
        assert_eq!(
            visible(Language::Python, py),
            [
                "a [0:8-6:9]",
                "xs [1:23-6:9]",
                "i [1:10-1:23]",
                "e [4:17-5:12]",
                "b [6:9-6:9]"
            ]
        );
        // Java: an enhanced-for variable ends with the loop, a lambda parameter with the lambda.
        let java = "class K {\n  void m(List<Repo> xs) {\n    for (Repo r : xs) {}\n    xs.forEach(r -> r.save());\n    Repo r2 = null;\n  }\n}\n";
        assert_eq!(
            visible(Language::Java, java),
            [
                "xs [1:22-5:3]",
                "r [2:15-2:24]",
                "r [3:16-3:28]",
                "r2 [4:18-5:3]"
            ]
        );
    }

    /// `(row, column)` just after the first occurrence of `needle`.
    fn after(src: &str, needle: &str) -> (i32, i32) {
        let end = src.find(needle).expect(needle) + needle.len();
        let before = &src[..end];
        let row = before.matches('\n').count();
        let col = end - before.rfind('\n').map_or(0, |i| i + 1);
        (row as i32, col as i32)
    }

    /// `(name, start, end)` of a non-member binding's extent.
    type Extent = (String, (i32, i32), (i32, i32));

    fn extents(language: Language, src: &str) -> Vec<Extent> {
        extract(language, src.as_bytes())
            .unwrap()
            .bindings
            .into_iter()
            .filter(|b| !b.member)
            .map(|b| {
                let v = b.visible;
                (b.name, (v[0], v[1]), (v[2], v[3]))
            })
            .collect()
    }

    #[test]
    fn only_the_except_name_ends_with_its_clause() {
        let src = "def f(r):\n    try:\n        pass\n    except Exception as e:\n        r = Resp()\n    r.m1()\n";
        let got = extents(Language::Python, src);
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(
            got[1],
            ("e".into(), after(src, "as e"), after(src, "r = Resp()"))
        );
        // The assignment inside the clause is an ordinary function-scoped name.
        assert_eq!(
            got[2],
            ("r".into(), after(src, "Resp()"), after(src, "r.m1()"))
        );
    }

    #[test]
    fn signature_parameters_end_with_their_signature() {
        let src = "const r: Req = mk();\ninterface H { do(r: Resp): void }\nfunction over(r: Resp): void;\nfunction over(r: any) {}\ntype Fn2 = (w: Resp) => void;\n";
        let got = extents(Language::TypeScript, src);
        let rows: Vec<(String, i32, i32)> =
            got.iter().map(|(n, s, e)| (n.clone(), s.0, e.0)).collect();
        // Every signature parameter starts and ends on its own line; the constant spans the file.
        assert_eq!(
            rows,
            [
                ("r".into(), 0, 5),
                ("r".into(), 1, 1),
                ("r".into(), 2, 2),
                ("r".into(), 3, 3),
                ("w".into(), 4, 4)
            ]
        );

        let go = "package p\nvar cb func(c *Ctx)\ntype H interface { Do(s *Ctx) }\nfunc f(x int) {\n\tswitch x {\n\tcase 1:\n\t\tr := Resp{}\n\t\tr.M1()\n\tcase 2:\n\t\tr.M2()\n\t}\n}\n";
        let got = extents(Language::Go, go);
        let rows: Vec<(String, i32, i32)> =
            got.iter().map(|(n, s, e)| (n.clone(), s.0, e.0)).collect();
        // `cb` spans the file; `c` and `s` end on their own line; `r` ends with its case clause,
        // after `r.M1()` and before `r.M2()`.
        assert_eq!(
            &rows[..4],
            [
                ("cb".into(), 1, 12),
                ("c".into(), 1, 1),
                ("s".into(), 2, 2),
                ("x".into(), 3, 11)
            ]
        );
        let (name, start, end) = &rows[4];
        assert_eq!(name, "r");
        assert_eq!(*start, after(go, "r := Resp{}").0);
        assert!(
            *end >= after(go, "r.M1()").0 && *end < after(go, "r.M2()").0,
            "{rows:?}"
        );
    }
}
