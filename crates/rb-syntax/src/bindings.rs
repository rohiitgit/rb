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
    /// The extent starts at the start of `from` rather than its end (hoisted `var`).
    pub from_start: bool,
    /// The initialiser expression, when there is one per name.
    pub value: Option<Node<'t>>,
    /// Declared constant (`const`, Java `final` or an interface field, Go `const`).
    pub constant: bool,
    /// The parameter's position in its function (contract M4b S1).
    pub parameter: Option<usize>,
    /// The declared type's arguments by simple name, or an array's element type.
    pub type_args: Vec<String>,
    /// The declared type is an array.
    pub array: bool,
    /// A type argument (or an array's element) has type arguments of its own.
    pub nested_type_args: bool,
    /// Java: the declaration's modifiers (contract M4b amendment 6).
    pub modifiers: Vec<String>,
    /// Java: the declarator has an initialiser.
    pub initialised: bool,
    /// TS/JS: an object pattern destructured from this object (`const { get }
    /// = axios` holds `axios.get`; a `require("module")` call holds the
    /// module), per local name (contract M6b S9).
    pub destructured_from: Option<String>,
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
            Language::Kotlin => kotlin(node, source, &mut out),
            Language::CSharp => csharp(node, source, &mut out),
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

/// A member write before its enclosing definition is resolved (contract M6b S1).
pub(crate) struct RawMemberWrite<'t> {
    pub member: String,
    /// The right-hand side, when the write is an assignment with one.
    pub value: Option<Node<'t>>,
    /// The written object is `this` (contract M6b S6: anything else still
    /// counts — an alias may name `this` — but never folds).
    pub on_this: bool,
    pub node: Node<'t>,
    pub ctor: bool,
}

/// Every `Y.x = <value>` write (TS/JS `member_expression`, Java `field_access` on any
/// object), in source order. Any right-hand side counts as a write (compound
/// assignments included: they read and write); the value summary is resolved by the
/// caller. Whether the object is `this` is recorded with the write (contract M6b S6:
/// only a `this` write folds, but any write to the name counts for single-assignment).
/// Bare-field writes (`x = …`) are not members: they may be locals. Update
/// (`this.x++`), `delete`, destructuring and for-in/of member targets are writes with
/// no value (also contract M6b S6).
pub(crate) fn member_writes<'t>(
    language: Language,
    root: Node<'t>,
    source: &[u8],
) -> Vec<RawMemberWrite<'t>> {
    if !matches!(
        language,
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Java
    ) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match node.kind() {
            // `=` and compound assignments (`+=`, `||=`, `??=`): every kind reads and
            // writes, so every kind counts for single-assignment.
            "assignment_expression" | "augmented_assignment_expression" => {
                if let (Some(left), Some(right)) = (
                    node.child_by_field_name("left"),
                    node.child_by_field_name("right"),
                ) {
                    if let Some((member, on_this)) = member_target(language, left, source) {
                        out.push(RawMemberWrite {
                            member,
                            value: Some(right),
                            on_this,
                            ctor: in_constructor(node, source),
                            node,
                        });
                    } else if left.kind() != "identifier" {
                        // A destructuring target: every member it names is
                        // written (the main loop records nested assignments
                        // with their own spans).
                        pattern_targets(language, left, source, &mut out);
                    }
                }
            }
            // `this.x++`, `++this.x`: a read and a write with no value.
            "update_expression" => {
                if let Some((member, on_this, target)) = update_target(language, node, source) {
                    out.push(RawMemberWrite {
                        member,
                        value: None,
                        on_this,
                        ctor: in_constructor(node, source),
                        node: target,
                    });
                }
            }
            // `delete this.x`.
            "unary_expression" => {
                if node
                    .child_by_field_name("operator")
                    .is_some_and(|op| text(op, source) == "delete")
                    && let Some((member, on_this, target)) = update_target(language, node, source)
                {
                    out.push(RawMemberWrite {
                        member,
                        value: None,
                        on_this,
                        ctor: in_constructor(node, source),
                        node: target,
                    });
                }
            }
            // `for (this.x of …)`.
            "for_in_statement" => {
                if let Some(left) = node.child_by_field_name("left") {
                    if let Some((member, on_this)) = member_target(language, left, source) {
                        out.push(RawMemberWrite {
                            member,
                            value: None,
                            on_this,
                            ctor: in_constructor(node, source),
                            node: left,
                        });
                    } else if left.kind() != "identifier" {
                        pattern_targets(language, left, source, &mut out);
                    }
                }
            }
            _ => {}
        }
        let mut c = node.walk();
        let children: Vec<Node<'t>> = node.children(&mut c).collect();
        stack.extend(children.into_iter().rev());
    }
    out
}

/// The member a `member_expression` (TS/JS) or `field_access` (Java) names,
/// with whether its object is `this` (`this.a.b` names `b`, not on `this`).
fn member_target(language: Language, node: Node<'_>, source: &[u8]) -> Option<(String, bool)> {
    match language {
        Language::Java => {
            if node.kind() != "field_access" {
                return None;
            }
            let field = node
                .child_by_field_name("field")
                .filter(|n| n.kind() == "identifier")?;
            let object = node.child_by_field_name("object")?;
            Some((text(field, source), object.kind() == "this"))
        }
        _ => {
            if node.kind() != "member_expression" {
                return None;
            }
            let property = node
                .child_by_field_name("property")
                .filter(|n| n.kind() == "property_identifier")?;
            let object = node.child_by_field_name("object")?;
            Some((text(property, source), object.kind() == "this"))
        }
    }
}

/// The member an update or `delete` writes: the first member operand, with
/// the member node for a precise span.
fn update_target<'t>(
    language: Language,
    node: Node<'t>,
    source: &[u8],
) -> Option<(String, bool, Node<'t>)> {
    let mut c = node.walk();
    let target = node.named_children(&mut c).find(|n| {
        matches!(
            (language, n.kind()),
            (Language::Java, "field_access")
                | (
                    Language::TypeScript | Language::Tsx | Language::JavaScript,
                    "member_expression"
                )
        )
    })?;
    let (member, on_this) = member_target(language, target, source)?;
    Some((member, on_this, target))
}

/// Member targets of a destructuring pattern: every member it names is
/// written. Computed keys and subscript objects are reads, and anything
/// inside a nested function, class, assignment or loop is recorded by the
/// main loop with its own span — none of those are descended into.
fn pattern_targets<'t>(
    language: Language,
    node: Node<'t>,
    source: &[u8],
    out: &mut Vec<RawMemberWrite<'t>>,
) {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        match node.kind() {
            "function_declaration"
            | "function_expression"
            | "generator_function"
            | "generator_function_declaration"
            | "arrow_function"
            | "method_definition"
            | "class_declaration"
            | "class_body"
            | "class_expression"
            | "lambda_expression"
            | "assignment_expression"
            | "augmented_assignment_expression"
            | "update_expression"
            | "unary_expression"
            | "for_in_statement"
            | "for_statement" => continue,
            // A computed key is read, not written: `{[this.k]: v}` reads `k`.
            "pair_pattern" => {
                let mut c = node.walk();
                let mut named: Vec<Node<'_>> = node
                    .named_children(&mut c)
                    .filter(|n| !matches!(n.kind(), "comment" | "line_comment" | "block_comment"))
                    .collect();
                if let Some(first) = named.first()
                    && member_target(language, *first, source).is_some()
                    && named.len() > 1
                {
                    named.remove(0);
                }
                stack.extend(named);
            }
            // An assignment default is read; only its target is written.
            "assignment_pattern" => {
                if let Some(left) = node.child_by_field_name("left") {
                    stack.push(left);
                } else {
                    let mut c = node.walk();
                    stack.extend(node.named_children(&mut c));
                }
            }
            // An element write names no member of its own.
            "subscript_expression" => continue,
            _ => {
                if let Some((member, on_this)) = member_target(language, node, source) {
                    out.push(RawMemberWrite {
                        member,
                        value: None,
                        on_this,
                        ctor: in_constructor(node, source),
                        node,
                    });
                } else {
                    let mut c = node.walk();
                    stack.extend(node.children(&mut c));
                }
            }
        }
    }
}

/// Whether `node` sits directly in a constructor: the nearest enclosing function-like
/// node is a `method_definition` named `constructor` (TS/JS) or a
/// `constructor_declaration` (Java). A write inside a nested function is not one: the
/// callback may run after construction (the safe direction for single-assignment).
fn in_constructor(node: Node<'_>, source: &[u8]) -> bool {
    let mut cur = node.parent();
    while let Some(n) = cur {
        match n.kind() {
            "constructor_declaration" => return true,
            "method_definition" => {
                return n
                    .child_by_field_name("name")
                    .is_some_and(|name| text(name, source) == "constructor");
            }
            "function_declaration"
            | "function_expression"
            | "generator_function"
            | "generator_function_declaration"
            | "arrow_function"
            | "function_definition"
            | "method_declaration"
            | "lambda_expression"
            | "function_literal"
            | "func_literal" => return false,
            _ => {}
        }
        cur = n.parent();
    }
    false
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
        // Contract M6i S2: scope refined with Tier B (roots verified by the S1 parse tests).
        Language::Kotlin => &["source_file"],
        Language::CSharp => &["compilation_unit"],
    }
}

/// The function (or program) a JavaScript `var` is hoisted to.
fn hoisting_scope<'t>(node: Node<'t>) -> Option<Node<'t>> {
    let mut cur = node.parent();
    while let Some(n) = cur {
        if matches!(
            n.kind(),
            "program"
                | "function_declaration"
                | "function_expression"
                | "function"
                | "generator_function"
                | "generator_function_declaration"
                | "arrow_function"
                | "method_definition"
        ) {
            return Some(n);
        }
        cur = n.parent();
    }
    None
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

/// Records `name` (when present), visible from the end of `from`; whether it was recorded.
fn push<'t>(
    out: &mut Vec<RawBinding<'t>>,
    name: Option<Node<'t>>,
    type_name: Option<String>,
    member: bool,
    node: Node<'t>,
    from: Node<'t>,
    source: &[u8],
) -> bool {
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
            from_start: false,
            value: None,
            constant: false,
            parameter: None,
            type_args: Vec::new(),
            array: false,
            nested_type_args: false,
            modifiers: Vec::new(),
            initialised: false,
            destructured_from: None,
        });
    }
    name.is_some()
}

/// Sets the position of the binding `push` just recorded, if it recorded one.
fn positioned(out: &mut [RawBinding<'_>], pushed: bool, position: Option<usize>) {
    if let (true, Some(b)) = (pushed, out.last_mut()) {
        b.parameter = position;
    }
}

/// Sets the type arguments of the binding `push` just recorded from its declared type node.
fn with_type_args(out: &mut [RawBinding<'_>], pushed: bool, ty: Option<Node<'_>>, source: &[u8]) {
    if let (true, Some(b), Some(ty)) = (pushed, out.last_mut(), ty) {
        (b.type_args, b.array) = type_args(ty, source);
        b.nested_type_args = nested_type_args(ty);
    }
}

/// Whether a declared type's arguments (or an array's element) carry type arguments of their
/// own (`List<Store<String>>`, `Store<String>[]`), which [`type_args`] does not keep
/// (evaluator pass 1, Y5).
fn nested_type_args(node: Node<'_>) -> bool {
    fn has_arguments(node: Node<'_>) -> bool {
        let mut c = node.walk();
        node.kind() == "type_arguments" || node.named_children(&mut c).any(has_arguments)
    }
    let mut c = node.walk();
    match node.kind() {
        "type_annotation" | "annotated_type" | "parenthesized_type" => node
            .named_children(&mut c)
            .filter(|n| !is_annotation(*n))
            .last()
            .is_some_and(nested_type_args),
        "generic_type" => node
            .named_children(&mut c)
            .filter(|n| n.kind() == "type_arguments")
            .any(|a| {
                let mut ac = a.walk();
                a.named_children(&mut ac).any(has_arguments)
            }),
        "array_type" => node
            .child_by_field_name("element")
            .or_else(|| node.named_child(0))
            .is_some_and(has_arguments),
        _ => false,
    }
}

/// A declared type's arguments by simple name, one level deep, and whether it is an array, in
/// which case the element type is the only argument (contract M4b S1): `List<Notifier>` →
/// `Notifier`; `Map<String, a.Sink>` → `String`, `Sink`; `Notifier[]` → `Notifier`, array.
fn type_args(node: Node<'_>, source: &[u8]) -> (Vec<String>, bool) {
    let mut c = node.walk();
    match node.kind() {
        "type_annotation" | "annotated_type" | "parenthesized_type" => node
            .named_children(&mut c)
            .filter(|n| !is_annotation(*n))
            .last()
            .map(|inner| type_args(inner, source))
            .unwrap_or_default(),
        "generic_type" => {
            let args = node
                .named_children(&mut c)
                .find(|n| n.kind() == "type_arguments");
            let names = args
                .map(|a| {
                    let mut ac = a.walk();
                    a.named_children(&mut ac)
                        .filter_map(|n| simple_type(n, source))
                        .collect()
                })
                .unwrap_or_default();
            (names, false)
        }
        "array_type" => {
            let element = node
                .child_by_field_name("element")
                .or_else(|| node.named_child(0));
            (
                element
                    .and_then(|e| simple_type(e, source))
                    .into_iter()
                    .collect(),
                true,
            )
        }
        _ => (Vec::new(), false),
    }
}

fn is_annotation(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "annotation" | "marker_annotation" | "comment" | "line_comment" | "block_comment"
    )
}

/// A type argument by its simple name: a qualified name's last identifier, a generic type's
/// name, an annotated type's type, a wildcard's bound (`?` when it has none).
fn simple_type(node: Node<'_>, source: &[u8]) -> Option<String> {
    let mut c = node.walk();
    match node.kind() {
        "comment" | "line_comment" | "block_comment" => None,
        "generic_type" => simple_type(
            node.child_by_field_name("name")
                .or_else(|| node.named_child(0))?,
            source,
        ),
        "annotated_type" | "type_annotation" | "parenthesized_type" => node
            .named_children(&mut c)
            .filter(|n| !is_annotation(*n))
            .last()
            .and_then(|inner| simple_type(inner, source)),
        "wildcard" => Some(
            node.named_children(&mut c)
                .filter(|n| !is_annotation(*n))
                .last()
                .and_then(|bound| simple_type(bound, source))
                // A bounded wildcard keeps its `?` mark, so agreement can
                // refuse it (contract M6c S9); a bare `?` stays `?`.
                .map(|bound| format!("?{bound}"))
                .unwrap_or_else(|| "?".to_owned()),
        ),
        "scoped_type_identifier" | "nested_type_identifier" | "qualified_type" => node
            .named_children(&mut c)
            .filter(|n| !is_annotation(*n))
            .last()
            .and_then(|last| simple_type(last, source)),
        _ => Some(text(node, source)),
    }
}

/// The position of a parameter node among its list's parameters (`kinds`), comments and
/// separators left out.
fn index_among(node: Node<'_>, kinds: &[&str]) -> Option<usize> {
    let parent = node.parent()?;
    let mut c = parent.walk();
    parent
        .named_children(&mut c)
        .filter(|n| kinds.contains(&n.kind()))
        .position(|n| n.id() == node.id())
}

const JAVA_PARAMETERS: &[&str] = &["formal_parameter", "spread_parameter"];
const ES_PARAMETERS: &[&str] = &["required_parameter", "optional_parameter"];
const PYTHON_PARAMETERS: &[&str] = &[
    "identifier",
    "default_parameter",
    "typed_parameter",
    "typed_default_parameter",
    "list_splat_pattern",
    "dictionary_splat_pattern",
];

/// Go: the position of a `parameter_declaration`'s first name in its function's parameters
/// (not a receiver's or named result's list); unnamed parameters take one position each.
/// Whether a Kotlin declaration sits in a type body (a member) rather than a function.
fn kotlin_member(node: Node<'_>) -> bool {
    let mut cur = node.parent();
    while let Some(n) = cur {
        match n.kind() {
            "function_declaration" | "function_body" | "block" | "source_file" => return false,
            "class_body" | "enum_class_body" => return true,
            _ => {}
        }
        cur = n.parent();
    }
    false
}

/// Kotlin parameters, properties and locals with their declared types (`val` is constant;
/// constructor-call values stay untyped, like Java `var` without `new`). Primary-constructor
/// parameters (`class_parameter`) bind like parameters (contract M6j S1: constructor injection
/// points).
fn kotlin<'t>(node: Node<'t>, source: &[u8], out: &mut Vec<RawBinding<'t>>) {
    match node.kind() {
        "parameter" | "class_parameter" => {
            let mut c = node.walk();
            let mut kids = node.named_children(&mut c);
            let name = kids.find(|n| n.kind() == "identifier");
            let mut c = node.walk();
            let ty = node
                .named_children(&mut c)
                .find(|n| n.kind() == "user_type")
                .and_then(|t| type_name(t, source));
            let pushed = push(out, name, ty, false, node, node, source);
            initialised(out, pushed, None, false);
        }
        "property_declaration" => {
            let mut c = node.walk();
            let decl = node
                .named_children(&mut c)
                .find(|n| n.kind() == "variable_declaration");
            let mut c = node.walk();
            let value = node.named_children(&mut c).find(|n| {
                matches!(
                    n.kind(),
                    "call_expression"
                        | "navigation_expression"
                        | "object_literal"
                        | "anonymous_function"
                )
            });
            if let Some(d) = decl {
                let mut c = d.walk();
                let mut kids = d.named_children(&mut c);
                let name = kids.find(|n| n.kind() == "identifier");
                let mut c = d.walk();
                let ty = d
                    .named_children(&mut c)
                    .find(|n| n.kind() == "user_type")
                    .and_then(|t| type_name(t, source));
                let member = kotlin_member(node);
                let is_val = text(node, source).trim_start().starts_with("val ");
                let pushed = push(out, name, ty, member, node, d, source);
                initialised(out, pushed, value, is_val);
            }
        }
        _ => {}
    }
}

/// C# parameters, fields, properties and locals. `var x = new Foo()` takes the construction type (the
/// Java `var` precedent); anything else without a written type stays untyped.
/// Properties bind as members (contract M6k S5): their decorations ride the node.
fn csharp<'t>(node: Node<'t>, source: &[u8], out: &mut Vec<RawBinding<'t>>) {
    match node.kind() {
        "property_declaration" => {
            let ty = node
                .child_by_field_name("type")
                .and_then(|t| type_name(t, source));
            let pushed = push(
                out,
                node.child_by_field_name("name"),
                ty,
                true,
                node,
                node,
                source,
            );
            with_type_args(out, pushed, node.child_by_field_name("type"), source);
            // Contract M6k pass 1: modifiers ride the binding so the pack can skip
            // static properties (never injected), like Java fields.
            if pushed {
                let mut c = node.walk();
                if let Some(b) = out.last_mut() {
                    b.modifiers = node
                        .children(&mut c)
                        .filter(|n| n.kind() == "modifier")
                        .map(|n| text(n, source))
                        .collect();
                }
            }
            initialised(out, pushed, None, false);
        }
        "parameter" => {
            let pushed = push(
                out,
                node.child_by_field_name("name"),
                node.child_by_field_name("type")
                    .and_then(|t| type_name(t, source)),
                false,
                node,
                node,
                source,
            );
            initialised(out, pushed, None, false);
        }
        "variable_declarator" => {
            let declaration = node.parent();
            // The extent runs to the end of the declaration: later siblings see the name.
            let extent = declaration
                .and_then(|d| d.parent())
                .filter(|p| {
                    matches!(
                        p.kind(),
                        "local_declaration_statement" | "field_declaration"
                    )
                })
                .or(declaration);
            let member = extent.is_some_and(|p| p.kind() == "field_declaration");
            let ty = declaration
                .and_then(|d| d.child_by_field_name("type"))
                .and_then(|t| {
                    if text(t, source) == "var" {
                        // `var x = <value>`: the value is the declarator's second named child
                        // (it carries no field name).
                        let mut vc = node.walk();
                        let mut values = node.named_children(&mut vc);
                        values.next();
                        values
                            .find(|v| v.kind() == "object_creation_expression")
                            .and_then(|v| type_name(v, source))
                    } else {
                        type_name(t, source)
                    }
                });
            let mut vc = node.walk();
            let mut values = node.named_children(&mut vc);
            values.next();
            let value = values.next();
            let pushed = push(
                out,
                node.child_by_field_name("name"),
                ty,
                member,
                extent.or(declaration).unwrap_or(node),
                node,
                source,
            );
            initialised(out, pushed, value, false);
        }
        _ => {}
    }
}

fn go_position(node: Node<'_>) -> Option<usize> {
    let list = node.parent().filter(|l| l.kind() == "parameter_list")?;
    let owner = list.parent()?;
    if owner
        .child_by_field_name("parameters")
        .is_none_or(|p| p.id() != list.id())
    {
        return None;
    }
    let mut c = list.walk();
    let mut base = 0;
    for p in list.named_children(&mut c) {
        if p.id() == node.id() {
            return Some(base);
        }
        if matches!(
            p.kind(),
            "parameter_declaration" | "variadic_parameter_declaration"
        ) {
            let mut pc = p.walk();
            base += p
                .children_by_field_name("name", &mut pc)
                .filter(|n| n.kind() == "identifier")
                .count()
                .max(1);
        }
    }
    None
}

/// Sets the initialiser and constness of the binding `push` just recorded, if it recorded one.
fn initialised<'t>(
    out: &mut [RawBinding<'t>],
    pushed: bool,
    value: Option<Node<'t>>,
    constant: bool,
) {
    if let (true, Some(b)) = (pushed, out.last_mut()) {
        b.value = value;
        b.constant = constant;
    }
}

/// The `n`th expression of a Go `expression_list` (or the expression itself).
fn nth_value(list: Option<Node<'_>>, n: usize) -> Option<Node<'_>> {
    let list = list?;
    if list.kind() != "expression_list" {
        return (n == 0).then_some(list);
    }
    let mut c = list.walk();
    list.named_children(&mut c).nth(n)
}

/// A Java declaration is `final`.
fn java_constant(node: Node<'_>, source: &[u8]) -> bool {
    let mut c = node.walk();
    node.children(&mut c)
        .filter(|m| m.kind() == "modifiers")
        .any(|m| {
            let mut mc = m.walk();
            m.children(&mut mc).any(|t| text(t, source) == "final")
        })
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
    // Kotlin scalars (contract M6i S2). Only spellings no Java reference type uses:
    // java.lang has Long, Short, Byte, Double, Float, Boolean and String.
    "Int",
    "Char",
    "Unit",
    // C# scalars (contract M6i S2). Same rule: long, double, short, char, void and boolean
    // are Java spellings too.
    "decimal",
    "dynamic",
    "nint",
    "nuint",
    "sbyte",
    "uint",
    "ulong",
    "ushort",
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
        // Kotlin `String?`, `acme.Foo`, `Map<String, Foo>`: the head name.
        "user_type" => {
            let mut text = text(node, source);
            if let Some(i) = text.find('<') {
                text.truncate(i);
            }
            Some(text.trim_end_matches('?').trim().to_owned())
        }
        // C# `Bar.Baz`, `List<Foo>`: the head name.
        "qualified_name" | "generic_name" => {
            let mut text = text(node, source);
            if let Some(i) = text.find('<') {
                text.truncate(i);
            }
            Some(text.trim().to_owned())
        }
        _ => None,
    }
}

/// The object an object pattern destructures (`const { get } = axios` holds
/// `axios.get` under each local name): a name, a member path as written, or
/// the module of a single-string `require("module")` call (contract M6b S9).
/// Anything else (a call, an array, a conditional) stays sourceless, as
/// before.
fn destructured_object(value: Node<'_>, source: &[u8]) -> Option<String> {
    match value.kind() {
        "identifier" | "member_expression" => Some(text(value, source)),
        "call_expression" => {
            let callee = value.child_by_field_name("function")?;
            if !(callee.kind() == "identifier" && text(callee, source) == "require") {
                return None;
            }
            let args = value.child_by_field_name("arguments")?;
            let mut c = args.walk();
            let mut named = args
                .named_children(&mut c)
                .filter(|a| !matches!(a.kind(), "comment"));
            let arg = named.next()?;
            if named.next().is_some() || arg.kind() != "string" {
                return None;
            }
            Some(
                text(arg, source)
                    .trim_matches(|c| c == '"' || c == '\'')
                    .to_owned(),
            )
        }
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
                let pushed = push(
                    out,
                    d.child_by_field_name("name"),
                    ty,
                    member,
                    node,
                    d,
                    source,
                );
                initialised(
                    out,
                    pushed,
                    d.child_by_field_name("value"),
                    java_constant(node, source),
                );
                if !is_var {
                    with_type_args(out, pushed, declared, source);
                }
                if let (true, Some(b)) = (pushed, out.last_mut()) {
                    b.modifiers = crate::decorations::java_modifiers(node, source);
                    b.initialised = d.child_by_field_name("value").is_some();
                }
            }
        }
        // An interface's or annotation type's field: implicitly `static final`.
        "constant_declaration" => {
            let ty = typed(node);
            for d in declarators(node) {
                let pushed = push(
                    out,
                    d.child_by_field_name("name"),
                    ty.clone(),
                    true,
                    node,
                    d,
                    source,
                );
                initialised(out, pushed, d.child_by_field_name("value"), true);
            }
        }
        "formal_parameter" | "spread_parameter" | "catch_formal_parameter" | "resource" => {
            let ty = typed(node).or_else(|| {
                let mut c = node.walk();
                node.named_children(&mut c)
                    .find(|n| n.kind() == "catch_type")
                    .and_then(|t| type_name(t, source))
            });
            let pushed = push(
                out,
                node.child_by_field_name("name"),
                ty,
                false,
                node,
                node,
                source,
            );
            initialised(out, pushed, None, java_constant(node, source));
            if let (true, Some(b)) = (pushed, out.last_mut()) {
                b.modifiers = crate::decorations::java_modifiers(node, source);
            }
            // A `spread_parameter` has no `name` field, so it is never recorded (unchanged).
            if node.kind() == "formal_parameter" {
                positioned(out, pushed, index_among(node, JAVA_PARAMETERS));
                with_type_args(out, pushed, node.child_by_field_name("type"), source);
            }
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
                for (i, n) in names.into_iter().enumerate() {
                    let pushed = push(out, Some(n), None, false, node, params, source);
                    positioned(out, pushed, Some(i));
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
            let pushed = push(out, name, ty, true, node, node, source);
            initialised(out, pushed, node.child_by_field_name("value"), false);
            with_type_args(out, pushed, node.child_by_field_name("type"), source);
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
                let position = index_among(node, ES_PARAMETERS);
                for n in names {
                    let pushed = push(out, Some(n), ty.clone(), member, node, node, source);
                    positioned(out, pushed, position);
                    with_type_args(out, pushed, node.child_by_field_name("type"), source);
                }
            }
        }
        // JavaScript parameters (TypeScript wraps them in `required_parameter`).
        "formal_parameters" => {
            let mut c = node.walk();
            let parameters = node
                .named_children(&mut c)
                .filter(|p| !matches!(p.kind(), "comment"));
            for (i, p) in parameters.enumerate() {
                if matches!(p.kind(), "required_parameter" | "optional_parameter") {
                    continue;
                }
                let mut names = Vec::new();
                pattern_names(p, &mut names);
                for n in names {
                    let pushed = push(out, Some(n), None, false, p, node, source);
                    positioned(out, pushed, Some(i));
                }
            }
        }
        // `x => …`: a single unparenthesised parameter.
        "arrow_function" => {
            if let Some(p) = node
                .child_by_field_name("parameter")
                .filter(|p| p.kind() == "identifier")
            {
                let pushed = push(out, Some(p), None, false, node, p, source);
                positioned(out, pushed, Some(0));
            }
        }
        "variable_declarator" => {
            let value = node.child_by_field_name("value");
            let ty = node
                .child_by_field_name("type")
                .and_then(|t| type_name(t, source))
                .or_else(|| {
                    value
                        .filter(|v| v.kind() == "new_expression")
                        .and_then(|v| type_name(v, source))
                });
            if let Some(name) = node.child_by_field_name("name") {
                let mut names = Vec::new();
                pattern_names(name, &mut names);
                // Only a plain name takes the declared or constructed type.
                let ty = ty.filter(|_| name.kind() == "identifier");
                // `var` is hoisted: in effect from the start of the enclosing function.
                let hoisted = node
                    .parent()
                    .filter(|p| p.kind() == "variable_declaration")
                    .and_then(|_| hoisting_scope(node));
                let constant = node.parent().is_some_and(|p| {
                    p.kind() == "lexical_declaration"
                        && p.child_by_field_name("kind")
                            .is_some_and(|k| text(k, source) == "const")
                });
                // Only a plain name takes the initialiser.
                let init = value.filter(|_| name.kind() == "identifier");
                // An object pattern destructured from a name holds that
                // object's member under each local name (contract M6b S9).
                let destructured_from = if name.kind() == "object_pattern" {
                    value.and_then(|v| destructured_object(v, source))
                } else {
                    None
                };
                for n in names {
                    let pushed = push(out, Some(n), ty.clone(), false, node, node, source);
                    initialised(out, pushed, init, constant);
                    if let (Some(d), Some(b)) = (&destructured_from, out.last_mut()) {
                        b.destructured_from = Some(d.clone());
                    }
                    if let (Some(h), Some(b)) = (hoisted, out.last_mut()) {
                        b.from = h;
                        b.from_start = true;
                        b.scope = Some(h);
                    }
                }
            }
        }
        "for_in_statement" => {
            if let Some(left) = node.child_by_field_name("left") {
                let hoisted = node
                    .child_by_field_name("kind")
                    .filter(|k| text(*k, source) == "var")
                    .and_then(|_| hoisting_scope(node));
                let mut names = Vec::new();
                pattern_names(left, &mut names);
                for n in names {
                    push(out, Some(n), None, false, node, left, source);
                    if let (Some(h), Some(b)) = (hoisted, out.last_mut()) {
                        b.from = h;
                        b.from_start = true;
                        b.scope = Some(h);
                    }
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
                let pushed = push(
                    out,
                    left.child_by_field_name("property"),
                    ty,
                    true,
                    node,
                    node,
                    source,
                );
                initialised(out, pushed, node.child_by_field_name("right"), false);
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
                let pushed = push(out, name, ty, false, p, node, source);
                positioned(out, pushed, index_among(p, PYTHON_PARAMETERS));
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
            let right = node.child_by_field_name("right");
            match left.kind() {
                "identifier" => {
                    let pushed = push(out, Some(left), ty, false, node, node, source);
                    initialised(out, pushed, right, false);
                }
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
                    let pushed = push(
                        out,
                        left.child_by_field_name("attribute"),
                        ty,
                        true,
                        node,
                        node,
                        source,
                    );
                    initialised(out, pushed, right, false);
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
        "var_spec" | "const_spec" | "parameter_declaration" | "field_declaration" => {
            let ty = node
                .child_by_field_name("type")
                .and_then(|t| type_name(t, source));
            let member = node.kind() == "field_declaration";
            let constant = node.kind() == "const_spec";
            // `const a, b = …` lists its commas under the `name` field too.
            let declared: Vec<Node<'t>> = names(node)
                .into_iter()
                .filter(|n| matches!(n.kind(), "identifier" | "field_identifier"))
                .collect();
            let values = node.child_by_field_name("value");
            let position = (node.kind() == "parameter_declaration")
                .then(|| go_position(node))
                .flatten();
            for (i, name) in declared.iter().enumerate() {
                let pushed = push(out, Some(*name), ty.clone(), member, node, node, source);
                positioned(out, pushed, position.map(|p| p + i));
                let value = nth_value(values, i).filter(|_| {
                    values.is_some_and(|v| {
                        v.kind() != "expression_list" || v.named_child_count() == declared.len()
                    })
                });
                initialised(out, pushed, value, constant);
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
                let pushed = push(out, name, ty, false, node, node, source);
                let value = rights
                    .get(i)
                    .copied()
                    .filter(|_| lefts.len() == rights.len());
                initialised(out, pushed, value, false);
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
    fn kotlin_properties_parameters_and_locals() {
        let src = "class K {\nval repo: Repo\nfun f(u: User, i: Int) {\nval s: Store = Store()\nvar v = Thing()\n}\n}\n";
        assert_eq!(
            bound(Language::Kotlin, src),
            [
                "*repo: Repo in K",
                "u: User in f",
                "i: ? in f",
                "s: Store in f",
                "v: ? in f",
            ]
        );
    }

    /// Contract M6j S1: primary-constructor parameters bind (constructor injection points).
    #[test]
    fn kotlin_primary_constructor_parameters_bind() {
        let src = "class C @Inject constructor(@Named(\"s\") val s: Svc, t: T) {\n}\n";
        assert_eq!(bound(Language::Kotlin, src), ["s: Svc in C", "t: T in C"]);
        let facts = extract(Language::Kotlin, src.as_bytes()).unwrap();
        let s = facts.bindings.iter().find(|b| b.name == "s").unwrap();
        assert_eq!(s.decorations.len(), 1);
        assert_eq!(s.decorations[0].name, "Named");
    }

    #[test]
    fn csharp_fields_parameters_and_locals() {
        let src = "class K {\nFoo f;\npublic Bar M(Baz b, int i) {\nvar x = new Qux();\nFoo y = null;\n}\n}\n";
        assert_eq!(
            bound(Language::CSharp, src),
            [
                "*f: Foo in K",
                "b: Baz in M",
                "i: ? in M",
                "x: Qux in M",
                "y: Foo in M",
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
        // `var` is hoisted to the start of its function, or of the file.
        let src = "function first() { pv() }\nfunction f() { h(); var h = 2; }\nvar pv = 1;\nfor (var g of xs) {}\n";
        let got = extents(Language::JavaScript, src);
        let starts: Vec<(String, (i32, i32))> =
            got.iter().map(|(n, s, _)| (n.clone(), *s)).collect();
        assert_eq!(
            starts,
            [
                ("h".into(), (1, 0)),
                ("pv".into(), (0, 0)),
                ("g".into(), (0, 0))
            ]
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

    /// `name = init [const]` for every binding with an initialiser or constness, where `init`
    /// is abbreviated: `'lit{hole}'`, `call root.link`, `name x`, `x ?? 'lit'`.
    fn inits(language: Language, src: &str) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        let template = |t: &crate::StrTemplate| {
            let parts: Vec<String> = t
                .parts
                .iter()
                .map(|p| match p {
                    crate::TemplatePart::Lit { text } => text.clone(),
                    crate::TemplatePart::Hole { expr, .. } => format!("{{{expr}}}"),
                })
                .collect();
            format!("'{}'", parts.join(""))
        };
        facts
            .bindings
            .iter()
            .filter(|b| b.init.is_some() || b.constant)
            .map(|b| {
                let init = match &b.init {
                    None => "-".to_owned(),
                    Some(crate::Init::Template { template: t }) => template(t),
                    Some(crate::Init::Call { chain }) => {
                        let c = &facts.chains[*chain];
                        let links: Vec<&str> = c.links.iter().map(|l| l.name.as_str()).collect();
                        format!(
                            "call {}{}",
                            c.root
                                .as_deref()
                                .map(|r| format!("{r}."))
                                .unwrap_or_default(),
                            links.join(".")
                        )
                    }
                    Some(crate::Init::Name { text, .. }) => format!("name {text}"),
                    Some(crate::Init::Member { object, .. }) => format!("member {object}"),
                    Some(crate::Init::New {
                        type_name,
                        args,
                        body,
                    }) => format!("new {type_name}/{args}{}", if *body { " { }" } else { "" }),
                    Some(crate::Init::Fallback { primary, default }) => {
                        format!("{primary} ?? {}", template(default))
                    }
                    Some(crate::Init::Object { entries }) => {
                        let shown: Vec<String> = entries
                            .iter()
                            .map(|e| match &e.value {
                                crate::Arg::Template { template: t } => {
                                    format!("{}: {}", e.key, template(t))
                                }
                                crate::Arg::Name { text, .. } => format!("{}: name {text}", e.key),
                                _ => format!("{}: other", e.key),
                            })
                            .collect();
                        format!("object {{{}}}", shown.join(", "))
                    }
                };
                let constant = if b.constant { " const" } else { "" };
                format!("{} = {init}{constant}", b.name)
            })
            .collect()
    }

    /// `member = <value> @<enclosing-def-start-line>[!]`, in source order: every
    /// `this.x` write (contract M6b S1). `!` marks a constructor write, `~`
    /// a write whose object is not `this` (contract M6b S6: it counts, but
    /// never folds). Spans are asserted separately (below), with needles
    /// derived from the source.
    fn write_rows(language: Language, src: &str) -> Vec<String> {
        fn template(t: &crate::StrTemplate) -> String {
            let parts: Vec<String> = t
                .parts
                .iter()
                .map(|p| match p {
                    crate::TemplatePart::Lit { text } => text.clone(),
                    crate::TemplatePart::Hole { expr, .. } => format!("{{{expr}}}"),
                })
                .collect();
            format!("'{}'", parts.join(""))
        }
        let facts = extract(language, src.as_bytes()).unwrap();
        facts
            .member_writes
            .iter()
            .map(|w| {
                let value = match &w.value {
                    None => "-".to_owned(),
                    Some(crate::Init::Template { template: t }) => template(t),
                    Some(crate::Init::Name { text, .. }) => format!("name {text}"),
                    Some(crate::Init::Member { object, .. }) => format!("member {object}"),
                    Some(crate::Init::Call { chain }) => {
                        let c = &facts.chains[*chain];
                        let links: Vec<&str> = c.links.iter().map(|l| l.name.as_str()).collect();
                        format!(
                            "call {}{}",
                            c.root
                                .as_deref()
                                .map(|r| format!("{r}."))
                                .unwrap_or_default(),
                            links.join(".")
                        )
                    }
                    Some(_) => "other".to_owned(),
                };
                let enclosing = w
                    .enclosing
                    .map(|i| facts.definitions[i].span[0].to_string())
                    .unwrap_or_else(|| "-".to_owned());
                let ctor = if w.ctor { "!" } else { "" };
                let this = if w.on_this { "" } else { "~" };
                format!("{} = {value} @{enclosing}{ctor}{this}", w.member)
            })
            .collect()
    }

    /// Spans of `facts.member_writes`, in source order.
    fn write_spans(language: Language, src: &str) -> Vec<crate::Span> {
        extract(language, src.as_bytes())
            .unwrap()
            .member_writes
            .iter()
            .map(|w| w.span)
            .collect()
    }

    /// The span of the `occurrence`th line holding the full assignment `needle`
    /// (needles are single-line): `[start_row, start_col, end_row, end_col]`.
    fn span_of(src: &str, needle: &str, occurrence: usize) -> crate::Span {
        let mut seen = 0;
        for (row, line) in src.split('\n').enumerate() {
            let mut from = 0;
            while let Some(at) = line[from..].find(needle) {
                if seen == occurrence {
                    let col = (from + at) as i32;
                    return [row as i32, col, row as i32, col + needle.len() as i32];
                }
                seen += 1;
                from += at + 1;
            }
        }
        panic!("needle {needle:?} occurrence {occurrence} not in source");
    }

    #[test]
    fn typescript_member_writes_cover_every_right_hand_side() {
        let src = "class C {\n  constructor(base: string) {\n    this.svc = `${base}/x`;\n  }\n  reset(s: string) {\n    this.svc = s;\n    this.n = 1;\n    this.m = make(s);\n  }\n}\n";
        // TypeScript constructors are not definitions (their parameters sit in the
        // class), so the constructor write is class-enclosed; `!` still marks it.
        assert_eq!(
            write_rows(Language::TypeScript, src),
            [
                "svc = '{base}/x' @0!",
                "svc = name s @4",
                "n = - @4",
                "m = call make @4",
            ]
        );
        assert_eq!(
            write_spans(Language::TypeScript, src),
            [
                span_of(src, "this.svc = `${base}/x`", 0),
                span_of(src, "this.svc = s", 0),
                span_of(src, "this.n = 1", 0),
                span_of(src, "this.m = make(s)", 0),
            ]
        );
    }

    #[test]
    fn member_writes_cover_non_this_objects() {
        let src =
            "class C {\n  go(o: O) {\n    o.x = 1;\n    this.x += 1;\n    this.a.b = 2;\n  }\n}\n";
        // `o.x` is not on `this` (it counts, marked `~`, but never folds);
        // `this.a.b` writes `b`, not a member of `this`. `+=` still writes
        // `x` (a read and a write: the safe direction for counting).
        assert_eq!(
            write_rows(Language::TypeScript, src),
            ["x = - @1~", "x = - @1", "b = - @1~"]
        );
        assert_eq!(
            write_spans(Language::TypeScript, src),
            [
                span_of(src, "o.x = 1", 0),
                span_of(src, "this.x += 1", 0),
                // `=`-family keeps the statement span, as before.
                span_of(src, "this.a.b = 2", 0),
            ]
        );
    }

    /// Contract M6b S6: every write form counts for single-assignment, on
    /// `this` or not (`~` marks a non-`this` object); only `=`-family carries
    /// a value. Update, delete, destructuring and for-in/of writes are
    /// writes with no value.
    #[test]
    fn member_write_shapes_beyond_plain_assignment() {
        let src = "class C {\n  go(o: O) {\n    o.x = 1;\n    this.x++;\n    ++this.y;\n    delete this.z;\n    [this.d] = arr;\n    ({p: this.e} = obj);\n    for (this.f of arr) {}\n  }\n}\n";
        assert_eq!(
            write_rows(Language::TypeScript, src),
            [
                "x = - @1~",
                "x = - @1",
                "y = - @1",
                "z = - @1",
                "d = - @1",
                "e = - @1",
                "f = - @1",
            ]
        );
        assert_eq!(
            write_spans(Language::TypeScript, src),
            [
                span_of(src, "o.x = 1", 0),
                span_of(src, "this.x", 0),
                span_of(src, "this.y", 0),
                span_of(src, "this.z", 0),
                span_of(src, "this.d", 0),
                span_of(src, "this.e", 0),
                span_of(src, "this.f", 0),
            ]
        );
    }

    #[test]
    fn java_member_writes_and_skipped_shapes() {
        let src = "class C {\n  String s;\n  C(String b) {\n    this.s = b;\n  }\n  void m() {\n    this.s = \"x\";\n    this.s++;\n    helper = this.s;\n    s = \"y\";\n  }\n}\n";
        // `helper = …` and the bare-field `s = …` are not member writes.
        assert_eq!(
            write_rows(Language::Java, src),
            ["s = name b @2!", "s = 'x' @5", "s = - @5"]
        );
        assert_eq!(
            write_spans(Language::Java, src),
            [
                span_of(src, "this.s = b", 0),
                span_of(src, "this.s = \"x\"", 0),
                span_of(src, "this.s", 2),
            ]
        );
    }

    #[test]
    fn go_and_python_have_no_member_writes() {
        assert!(
            extract(Language::Go, b"package p\nfunc (c C) M() {\n\tc.x = 1\n}\n")
                .unwrap()
                .member_writes
                .is_empty()
        );
        assert!(
            extract(
                Language::Python,
                b"class C:\n\tdef m(self):\n\t\tself.x = 1\n"
            )
            .unwrap()
            .member_writes
            .is_empty()
        );
    }

    #[test]
    fn typescript_initialisers_and_constants() {
        let src = "const API = import.meta.env.VITE_API_URL ?? \"http://gateway\";\nconst BASE = process.env.BASE || 'http://x';\nexport const app = express();\nconst r = express.Router();\nlet u = `${API}/u`;\nconst n = API;\nconst api = await axios.create({ baseURL: API });\nconst { a } = mk();\nclass K { base = '/k'; constructor() { this.c = http.client(); } }\nconst x = 1 + f();\nconst y = API + BASE;\nconst opts = { baseURL: \"https://api.stripe.com\", ...more, timeout: 5 };\nlet o2 = ({ \"baseURL\": API });\n";
        assert_eq!(
            inits(Language::TypeScript, src),
            [
                "API = import.meta.env.VITE_API_URL ?? 'http://gateway' const",
                "BASE = process.env.BASE ?? 'http://x' const",
                "app = call express const",
                "r = call express.Router const",
                "u = '{API}/u'",
                "n = name API const",
                "api = call axios.create const",
                // A destructured name takes no initialiser, but is still declared `const`.
                "a = - const",
                "base = '/k'",
                "c = call http.client",
                "x = - const",
                // A concatenation of names is a template of holes (S12 folds it).
                "y = '{API}{BASE}' const",
                // An object literal, one level deep, spreads in source order (contract M5
                // S19 (f): a spread naming it is resolved to its `baseURL`).
                "opts = object {baseURL: 'https://api.stripe.com', ...: name more, timeout: other} const",
                "o2 = object {baseURL: name API}",
            ]
        );
        // The call initialiser points at the chain recorded for that call.
        let facts = extract(Language::TypeScript, src.as_bytes()).unwrap();
        let app = facts.bindings.iter().find(|b| b.name == "app").unwrap();
        let Some(crate::Init::Call { chain }) = app.init else {
            panic!("{app:?}")
        };
        let start = src.find("express()").unwrap();
        assert_eq!(
            facts.chains[chain].span[1] as usize,
            start - src[..start].rfind('\n').unwrap() - 1
        );
    }

    /// Contract M6b S9: an object pattern destructured from a name holds that
    /// object's member under each local name (`member <object>`); anything
    /// else destructured (a call, an array, a conditional) takes no
    /// initialiser, as before.
    #[test]
    fn typescript_destructured_member_inits() {
        let src = "import axios from \"axios\";\nconst { get } = axios;\nconst { post } = require(\"axios\");\nconst { del } = ax.sub;\nconst { a } = mk();\nconst [b] = arr;\nconst { c } = cond ? x : y;\n";
        assert_eq!(
            inits(Language::TypeScript, src),
            [
                "get = member axios const",
                "post = member axios const",
                "del = member ax.sub const",
                "a = - const",
                "b = - const",
                "c = - const",
            ]
        );
    }

    #[test]
    fn java_initialisers_constants_and_annotations() {
        let src = "class K {\n  static final String BASE = \"/api\";\n  String mutable = \"/m\";\n  private final WebClient web = WebClient.builder().baseUrl(\"http://x\").build();\n  @Value(\"${auth.url:http://auth}\") private String authUrl;\n  K(@Value(\"${k}\") String k) { final String p = BASE + \"/x\"; }\n  void m() { RestTemplate a = new RestTemplate(); RestTemplate b = new RestTemplate(f) { }; }\n}\ninterface Paths { String ROOT = \"/r\"; }\n";
        assert_eq!(
            inits(Language::Java, src),
            [
                "BASE = '/api' const",
                "mutable = '/m'",
                "web = call WebClient.builder.baseUrl.build const",
                "p = '{BASE}/x' const",
                // A class body (an anonymous subclass) is recorded (contract M4a amendment 16).
                "a = new RestTemplate/0",
                "b = new RestTemplate/1 { }",
                "ROOT = '/r' const",
            ]
        );
        let facts = extract(Language::Java, src.as_bytes()).unwrap();
        let annotated: Vec<(String, String, String)> = facts
            .bindings
            .iter()
            .flat_map(|b| {
                b.decorations
                    .iter()
                    .map(|d| (b.name.clone(), d.name.clone(), d.args[0].value.clone()))
            })
            .collect();
        assert_eq!(
            annotated,
            [
                (
                    "authUrl".to_owned(),
                    "Value".to_owned(),
                    "${auth.url:http://auth}".to_owned()
                ),
                ("k".to_owned(), "Value".to_owned(), "${k}".to_owned()),
            ]
        );
    }

    #[test]
    fn go_initialisers_and_constants() {
        let src = "package main\nconst base = \"http://notification:3000\"\nconst (\n\tA, B = \"a\", \"b\"\n\tC = iota\n)\nvar mutable = \"/m\"\nfunc main() {\n\tr := gin.Default()\n\tapi := r.Group(\"/billing\")\n\tx, y := f()\n}\n";
        assert_eq!(
            inits(Language::Go, src),
            [
                "base = 'http://notification:3000' const",
                "A = 'a' const",
                "B = 'b' const",
                "C = - const",
                "mutable = '/m'",
                "r = call gin.Default",
                "api = call r.Group",
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

/// Contract M4b S1: a parameter's position in its function, and a declared type's arguments.
#[cfg(test)]
mod parameter_tests {
    use crate::{Language, extract};

    /// `name@position` for every binding (`-` when not a parameter).
    fn positions(language: Language, src: &str) -> Vec<String> {
        extract(language, src.as_bytes())
            .unwrap()
            .bindings
            .iter()
            .map(|b| match b.parameter {
                Some(p) => format!("{}@{p}", b.name),
                None => format!("{}@-", b.name),
            })
            .collect()
    }

    #[test]
    fn java_parameter_positions() {
        // A varargs parameter is not a binding (its name sits in a declarator; unchanged), but
        // it counts as a position.
        let src = "class K {\n  private Repo repo;\n  K(Repo r, @Value(\"x\") String s, Notifier... ns) { Store local = x(); }\n  void m(int a) { xs.forEach((p, q) -> p.go()); xs.forEach(z -> z.go()); try (Repo t = r()) {} catch (RuntimeException e) {} }\n}\n";
        assert_eq!(
            positions(Language::Java, src),
            [
                "repo@-", "r@0", "s@1", "local@-", "a@0", "p@0", "q@1", "z@0", "t@-", "e@-"
            ]
        );
    }

    #[test]
    fn typescript_and_javascript_parameter_positions() {
        let ts = "class K {\n  f: Svc;\n  constructor(private svc: Svc, x?: number, { a, b }: Opts) { const y = 1; }\n}\nconst g = (u: U, v) => u;\nitems.map(w => w);\n";
        assert_eq!(
            positions(Language::TypeScript, ts),
            [
                "f@-", "svc@0", "x@1", "a@2", "b@2", "y@-", "g@-", "u@0", "v@1", "w@0"
            ]
        );
        let js = "function f(a, [b, c], d = 1) { let e = 2; }\n";
        assert_eq!(
            positions(Language::JavaScript, js),
            ["a@0", "b@1", "c@1", "d@2", "e@-"]
        );
    }

    #[test]
    fn go_and_python_parameter_positions() {
        // The receiver and named results are not parameters; a variadic parameter is not a
        // binding (unchanged) but takes its position. `int, s string` names two strings.
        let go = "package p\nfunc (c *C) M(a, b int, _ string, d ...Repo, e Repo) (n int) { x := 1 }\nfunc F(int, s string) {}\n";
        assert_eq!(
            positions(Language::Go, go),
            [
                "c@-", "a@0", "b@1", "_@2", "e@4", "n@-", "x@-", "int@0", "s@1"
            ]
        );
        // `self` keeps position 0 although it is not a binding.
        let py = "class K:\n    def f(self, u: User, *args, k=1, **kw):\n        v = 2\n";
        assert_eq!(
            positions(Language::Python, py),
            ["u@1", "args@2", "k@3", "kw@4", "v@-"]
        );
    }

    /// `name: type_args` (with `[]` for an array) for every binding that has type arguments.
    fn args(language: Language, src: &str) -> Vec<String> {
        extract(language, src.as_bytes())
            .unwrap()
            .bindings
            .iter()
            .filter(|b| !b.type_args.is_empty() || b.array)
            .map(|b| {
                let array = if b.array { "[]" } else { "" };
                format!("{}: {}{array}", b.name, b.type_args.join(","))
            })
            .collect()
    }

    #[test]
    fn java_type_arguments_one_level_deep() {
        let src = "class K {\n  List<Notifier> ns;\n  Map<String, com.x.Sink> m;\n  Optional<List<Repo>> o;\n  ObjectProvider<? extends Repo> p;\n  Notifier[] arr;\n  Repo plain;\n  K(Set<@NonNull Notifier> s, Notifier[] vs) {}\n}\n";
        assert_eq!(
            args(Language::Java, src),
            [
                "ns: Notifier",
                "m: String,Sink",
                "o: List",
                // Contract M6c S9: a bounded wildcard keeps its `?` mark.
                "p: ?Repo",
                "arr: Notifier[]",
                "s: Notifier",
                "vs: Notifier[]"
            ]
        );
        // The declared type itself is unchanged (type arguments stripped).
        let facts = extract(Language::Java, src.as_bytes()).unwrap();
        let ns = facts.bindings.iter().find(|b| b.name == "ns").unwrap();
        assert_eq!(ns.type_name, "List");
        // Evaluator pass 1, Y5: arguments with arguments of their own are marked.
        let nested: Vec<&str> = facts
            .bindings
            .iter()
            .filter(|b| b.nested_type_args)
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(nested, ["o"]);
        let src = "class K { K(List<Store<String>> a, Store<String>[] b, Store<String> c, List<Store> d) {} }";
        let facts = extract(Language::Java, src.as_bytes()).unwrap();
        let nested: Vec<&str> = facts
            .bindings
            .iter()
            .filter(|b| b.nested_type_args)
            .map(|b| b.name.as_str())
            .collect();
        assert_eq!(nested, ["a", "b"]);
    }

    /// Contract M4b amendment 6: a Java field's modifiers and whether it has an initialiser
    /// (Lombok's constructor rules).
    #[test]
    fn java_field_modifiers_and_initialisers() {
        let src = "class K {\n  private final Repo a;\n  private final int n = 3, m;\n  static Repo s;\n  @NonNull Repo b = null;\n  K(final Repo p) { int l = 1; }\n}\n";
        let facts = extract(Language::Java, src.as_bytes()).unwrap();
        let got: Vec<String> = facts
            .bindings
            .iter()
            .map(|b| format!("{} [{}] {}", b.name, b.modifiers.join(" "), b.initialised))
            .collect();
        assert_eq!(
            got,
            [
                "a [final private] false",
                "n [final private] true",
                "m [final private] false",
                "s [static] false",
                "b [] true",
                "p [final] false",
                "l [] true"
            ]
        );
    }

    #[test]
    fn typescript_type_arguments_one_level_deep() {
        let src = "class K {\n  a: Array<Notifier>;\n  b: Map<string, ns.Sink>;\n  c: Notifier[];\n  d: Repo;\n  constructor(private e: Promise<Repo>) {}\n}\n";
        assert_eq!(
            args(Language::TypeScript, src),
            ["a: Notifier", "b: string,Sink", "c: Notifier[]", "e: Repo"]
        );
    }
}
