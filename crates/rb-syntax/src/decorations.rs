//! Framework-neutral facts about definitions (design §4.5): the decorations on a definition (Java
//! annotations, Python and TypeScript decorators) with their literal arguments, the supertypes a
//! class or interface names, and a Python class body's string constants (`__tablename__ = 'x'`).
//! Framework packs interpret them; nothing here knows about JPA, Prisma or Django.

use tree_sitter::Node;

use crate::{ClassConstant, Decoration, DecorationArg, Language, Supertype};

pub(crate) fn decorations(language: Language, def: Node<'_>, source: &[u8]) -> Vec<Decoration> {
    let mut cursor = def.walk();
    let nodes: Vec<Node<'_>> = match language {
        Language::Java => def
            .children(&mut cursor)
            .filter(|c| c.kind() == "modifiers")
            .flat_map(|m| {
                let mut c = m.walk();
                m.named_children(&mut c)
                    .filter(|a| matches!(a.kind(), "annotation" | "marker_annotation"))
                    .collect::<Vec<_>>()
            })
            .collect(),
        Language::Python => def
            .parent()
            .filter(|p| p.kind() == "decorated_definition")
            .map(|p| {
                let mut c = p.walk();
                p.named_children(&mut c)
                    .filter(|d| d.kind() == "decorator")
                    .collect()
            })
            .unwrap_or_default(),
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            let mut found = Vec::new();
            // `@Dec export class X` puts the decorator on the export statement.
            if let Some(export) = def.parent().filter(|p| p.kind() == "export_statement") {
                let mut c = export.walk();
                found.extend(export.children_by_field_name("decorator", &mut c));
            }
            found.extend(def.children_by_field_name("decorator", &mut cursor));
            // Method decorators are the siblings right before the method in the class body.
            if def.parent().is_some_and(|p| p.kind() == "class_body") {
                let mut preceding = Vec::new();
                let mut sibling = def.prev_named_sibling();
                while let Some(s) = sibling.filter(|s| s.kind() == "decorator") {
                    preceding.push(s);
                    sibling = s.prev_named_sibling();
                }
                found.extend(preceding.into_iter().rev());
            }
            found
        }
        Language::Go => Vec::new(),
    };
    nodes
        .into_iter()
        .map(|n| decoration(language, n, source))
        .collect()
}

fn decoration(language: Language, node: Node<'_>, source: &[u8]) -> Decoration {
    // Java: annotation(name, arguments). Python/TS: decorator(<call | name expression>).
    let (name_node, arguments) = match language {
        Language::Java => (
            node.child_by_field_name("name"),
            node.child_by_field_name("arguments"),
        ),
        _ => match node.named_child(0) {
            Some(call) if matches!(call.kind(), "call" | "call_expression") => (
                call.child_by_field_name("function"),
                call.child_by_field_name("arguments"),
            ),
            other => (other, None),
        },
    };
    let mut args = Vec::new();
    if let Some(arguments) = arguments {
        let mut c = arguments.walk();
        for arg in arguments.named_children(&mut c) {
            match arg.kind() {
                "element_value_pair" | "keyword_argument" => {
                    let (value, literal) = arg
                        .child_by_field_name("value")
                        .map(|v| value_text(language, v, source))
                        .unwrap_or_default();
                    args.push(DecorationArg {
                        key: field_text(arg, &["key", "name"], source),
                        value,
                        literal,
                    });
                }
                // TypeORM-style options objects: their pairs become keyed arguments.
                "object" => {
                    let mut pc = arg.walk();
                    for pair in arg.named_children(&mut pc).filter(|p| p.kind() == "pair") {
                        let (value, literal) = pair
                            .child_by_field_name("value")
                            .map(|v| value_text(language, v, source))
                            .unwrap_or_default();
                        args.push(DecorationArg {
                            key: pair
                                .child_by_field_name("key")
                                .map(|k| value_text(language, k, source).0),
                            value,
                            literal,
                        });
                    }
                }
                "comment" => {}
                _ => {
                    let (value, literal) = value_text(language, arg, source);
                    args.push(DecorationArg {
                        key: None,
                        value,
                        literal,
                    });
                }
            }
        }
    }
    Decoration {
        name: name_node.map(|n| text(n, source)).unwrap_or_default(),
        args,
    }
}

pub(crate) fn supertypes(language: Language, def: Node<'_>, source: &[u8]) -> Vec<Supertype> {
    let mut out = Vec::new();
    let mut cursor = def.walk();
    match language {
        Language::Java => {
            if let Some(t) = def.child_by_field_name("superclass").and_then(|s| {
                let mut c = s.walk();
                s.named_children(&mut c).find(|n| !is_comment(*n))
            }) {
                out.push(supertype(t, source));
            }
            for list in def.children(&mut cursor) {
                if matches!(list.kind(), "super_interfaces" | "extends_interfaces") {
                    let implements = list.kind() == "super_interfaces";
                    out.extend(type_list_items(list).into_iter().map(|t| Supertype {
                        implements,
                        ..supertype(t, source)
                    }));
                }
            }
        }
        Language::Python => {
            if let Some(bases) = def.child_by_field_name("superclasses") {
                let mut c = bases.walk();
                out.extend(
                    bases
                        .named_children(&mut c)
                        .filter(|b| b.kind() != "keyword_argument" && !is_comment(*b))
                        .map(|b| supertype(b, source)),
                );
            }
        }
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            for child in def.children(&mut cursor) {
                match child.kind() {
                    "class_heritage" => {
                        let mut hc = child.walk();
                        for clause in child.named_children(&mut hc) {
                            match clause.kind() {
                                // `extends Base<Id>`: the span is the name's; the type
                                // arguments are a sibling. A value that is not a name
                                // (`mixin(Base)`, `(Base)`) declares no supertype: there is
                                // no type name an index occurrence could resolve.
                                "extends_clause" => {
                                    if let Some(value) = clause.child_by_field_name("value")
                                        && matches!(
                                            value.kind(),
                                            "identifier"
                                                | "member_expression"
                                                | "nested_identifier"
                                        )
                                    {
                                        out.push(Supertype {
                                            name: text(value, source),
                                            type_args: clause
                                                .child_by_field_name("type_arguments")
                                                .map(|a| named_texts(a, source))
                                                .unwrap_or_default(),
                                            implements: false,
                                            span: crate::span(value),
                                        });
                                    }
                                }
                                "implements_clause" => {
                                    let mut cc = clause.walk();
                                    out.extend(
                                        clause
                                            .named_children(&mut cc)
                                            .filter(|t| !is_comment(*t))
                                            .map(|t| Supertype {
                                                implements: true,
                                                ..supertype(t, source)
                                            }),
                                    );
                                }
                                _ => {}
                            }
                        }
                    }
                    // `interface X extends A, B<T>`.
                    "extends_type_clause" => {
                        let mut cc = child.walk();
                        out.extend(
                            child
                                .named_children(&mut cc)
                                .filter(|t| !is_comment(*t))
                                .map(|t| supertype(t, source)),
                        );
                    }
                    _ => {}
                }
            }
        }
        Language::Go => {}
    }
    out
}

pub(crate) fn class_constants(
    language: Language,
    def: Node<'_>,
    source: &[u8],
) -> Vec<ClassConstant> {
    if language != Language::Python || def.kind() != "class_definition" {
        return Vec::new();
    }
    let Some(body) = def.child_by_field_name("body") else {
        return Vec::new();
    };
    let mut c = body.walk();
    body.named_children(&mut c)
        .filter(|s| s.kind() == "expression_statement")
        .filter_map(|s| s.named_child(0).filter(|a| a.kind() == "assignment"))
        .filter_map(|a| {
            let left = a
                .child_by_field_name("left")
                .filter(|l| l.kind() == "identifier")?;
            let right = a.child_by_field_name("right")?;
            let (value, literal) = match right.kind() {
                "true" => ("True".to_owned(), true),
                "false" => ("False".to_owned(), true),
                // Any other value is kept, so a dynamic table name is seen as one.
                _ => crate::strings::string_expression(language, right, source)
                    .unwrap_or_else(|| (text(right, source), false)),
            };
            Some(ClassConstant {
                name: text(left, source),
                value,
                literal,
            })
        })
        .collect()
}

/// A type node (`Foo`, `a.b.Foo`, `Foo<A, B>`, `@Tag Foo`, Python `models.Model`) as a
/// supertype. The span is the name's only (contract M3b amendment 43): a generic type's name
/// child without its arguments, an annotated type's type without its annotations.
fn supertype(node: Node<'_>, source: &[u8]) -> Supertype {
    let mut c = node.walk();
    match node.kind() {
        "generic_type" => {
            let children: Vec<Node<'_>> = node.named_children(&mut c).collect();
            let name = children.iter().find(|n| n.kind() != "type_arguments");
            Supertype {
                name: name.map(|n| text(*n, source)).unwrap_or_default(),
                type_args: children
                    .iter()
                    .find(|n| n.kind() == "type_arguments")
                    .map(|a| named_texts(*a, source))
                    .unwrap_or_default(),
                implements: false,
                span: crate::span(*name.unwrap_or(&node)),
            }
        }
        "annotated_type" => {
            let inner = node
                .named_children(&mut c)
                .filter(|n| !matches!(n.kind(), "annotation" | "marker_annotation"))
                .last();
            match inner {
                Some(inner) => supertype(inner, source),
                None => plain_supertype(node, source),
            }
        }
        _ => plain_supertype(node, source),
    }
}

fn plain_supertype(node: Node<'_>, source: &[u8]) -> Supertype {
    Supertype {
        name: text(node, source),
        type_args: Vec::new(),
        implements: false,
        span: crate::span(node),
    }
}

fn type_list_items(node: Node<'_>) -> Vec<Node<'_>> {
    let mut c = node.walk();
    node.named_children(&mut c)
        .flat_map(|n| {
            if n.kind() == "type_list" {
                let mut lc = n.walk();
                n.named_children(&mut lc).collect::<Vec<_>>()
            } else {
                vec![n]
            }
        })
        .filter(|n| !is_comment(*n))
        .collect()
}

/// Tree-sitter's comment nodes are named extras that may sit anywhere in a heritage clause
/// (contract M3b amendment 55); they are never a supertype.
fn is_comment(node: Node<'_>) -> bool {
    matches!(node.kind(), "comment" | "line_comment" | "block_comment")
}

fn named_texts(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut c = node.walk();
    node.named_children(&mut c)
        .map(|n| text(n, source))
        .collect()
}

/// A value's decoded text and whether it is a static string (see `DecorationArg::literal`).
fn value_text(language: Language, node: Node<'_>, source: &[u8]) -> (String, bool) {
    crate::strings::string_expression(language, node, source)
        .unwrap_or_else(|| (text(node, source), false))
}

/// Declared type parameter names of a Java or TypeScript class/interface.
pub(crate) fn type_params(language: Language, def: Node<'_>, source: &[u8]) -> Vec<String> {
    if !matches!(
        language,
        Language::Java | Language::TypeScript | Language::Tsx
    ) {
        return Vec::new();
    }
    let Some(params) = def.child_by_field_name("type_parameters") else {
        return Vec::new();
    };
    let mut c = params.walk();
    params
        .named_children(&mut c)
        .filter(|p| p.kind() == "type_parameter")
        .filter_map(|p| {
            p.child_by_field_name("name")
                .or_else(|| p.named_child(0))
                .map(|n| text(n, source))
        })
        .collect()
}

fn field_text(node: Node<'_>, fields: &[&str], source: &[u8]) -> Option<String> {
    fields
        .iter()
        .find_map(|f| node.child_by_field_name(f))
        .map(|n| text(n, source))
}

fn text(node: Node<'_>, source: &[u8]) -> String {
    String::from_utf8_lossy(&source[node.byte_range()]).into_owned()
}

#[cfg(test)]
mod tests {
    use crate::{Language, extract};

    /// `def: @Name(key=value, value)` lines for every decorated definition.
    fn decorated(language: Language, src: &str) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        facts
            .definitions
            .iter()
            .flat_map(|d| {
                d.decorations.iter().map(move |dec| {
                    let args: Vec<String> = dec
                        .args
                        .iter()
                        .map(|a| match &a.key {
                            Some(k) => format!("{k}={}", a.value),
                            None => a.value.clone(),
                        })
                        .collect();
                    format!("{}: @{}({})", d.name, dec.name, args.join(", "))
                })
            })
            .collect()
    }

    fn supers(language: Language, src: &str) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        facts
            .definitions
            .iter()
            .filter(|d| !d.supertypes.is_empty())
            .map(|d| {
                let s: Vec<String> = d
                    .supertypes
                    .iter()
                    .map(|t| {
                        if t.type_args.is_empty() {
                            t.name.clone()
                        } else {
                            format!("{}<{}>", t.name, t.type_args.join(","))
                        }
                    })
                    .collect();
                format!("{}: {}", d.name, s.join(" "))
            })
            .collect()
    }

    const JAVA: &str = r#"@Entity @Table(name = "users", schema = "auth")
public class User extends Base implements Serializable, Comparable<User> {
    @Query(value = "SELECT 1 FROM t", nativeQuery = true)
    List<User> all();
    @Deprecated void old() {}
}
interface UserRepository extends org.springframework.data.jpa.repository.JpaRepository<User, Long> {
    @Query("SELECT u FROM User u")
    List<User> everyone();
}
"#;

    #[test]
    fn java_annotations_and_supertypes() {
        assert_eq!(
            decorated(Language::Java, JAVA),
            [
                "User: @Entity()",
                "User: @Table(name=users, schema=auth)",
                "all: @Query(value=SELECT 1 FROM t, nativeQuery=true)",
                "old: @Deprecated()",
                "everyone: @Query(SELECT u FROM User u)",
            ]
        );
        assert_eq!(
            supers(Language::Java, JAVA),
            [
                "User: Base Serializable Comparable<User>",
                // Names are kept as written; packs compare the last segment.
                "UserRepository: org.springframework.data.jpa.repository.JpaRepository<User,Long>"
            ]
        );
    }

    const PYTHON: &str = "@app.route('/x', methods=['GET'])
def f():
    pass

class User(Base, models.Model, metaclass=M):
    __tablename__ = 'users'
    name = 'not a table'

    class Meta:
        db_table = \"u\"
";

    #[test]
    fn python_decorators_supertypes_and_class_constants() {
        assert_eq!(
            decorated(Language::Python, PYTHON),
            ["f: @app.route(/x, methods=['GET'])"]
        );
        assert_eq!(
            supers(Language::Python, PYTHON),
            ["User: Base models.Model"]
        );
        let facts = extract(Language::Python, PYTHON.as_bytes()).unwrap();
        let constants = |name: &str| -> Vec<(String, String)> {
            facts
                .definitions
                .iter()
                .find(|d| d.name == name)
                .unwrap()
                .class_constants
                .iter()
                .map(|c| (c.name.clone(), c.value.clone()))
                .collect()
        };
        assert_eq!(
            constants("User"),
            [
                ("__tablename__".to_owned(), "users".to_owned()),
                ("name".to_owned(), "not a table".to_owned())
            ]
        );
        assert_eq!(constants("Meta"), [("db_table".to_owned(), "u".to_owned())]);
    }

    #[test]
    fn literal_flags_concatenation_booleans_and_type_parameters() {
        let java = r#"@Table(name = Tables.ACCOUNTS) class A {}
interface R { @Query(value = "SELECT * " + "FROM people", nativeQuery = true) void q(); }
interface BaseRepo<T, ID> extends JpaRepository<T, ID> {}
"#;
        let f = extract(Language::Java, java.as_bytes()).unwrap();
        let table = &f.definitions[0].decorations[0].args[0];
        assert_eq!(
            (table.value.as_str(), table.literal),
            ("Tables.ACCOUNTS", false)
        );
        let query = &f.definitions[2].decorations[0].args[0];
        assert_eq!(
            (query.value.as_str(), query.literal),
            ("SELECT * FROM people", true)
        );
        let base = f.definitions.iter().find(|d| d.name == "BaseRepo").unwrap();
        assert_eq!(base.type_params, ["T", "ID"]);

        let ts = "@Entity(TABLE) class A {}\n@Entity(`orders_${s}`) class B {}\nclass C<K, V> {}\n";
        let f = extract(Language::TypeScript, ts.as_bytes()).unwrap();
        let args: Vec<(&str, bool)> = f
            .definitions
            .iter()
            .flat_map(|d| &d.decorations)
            .map(|d| (d.args[0].value.as_str(), d.args[0].literal))
            .collect();
        assert_eq!(args, [("TABLE", false), ("orders_$1", false)]);
        assert_eq!(f.definitions[2].type_params, ["K", "V"]);

        let py = "class M(models.Model):\n    __tablename__ = f\"{P}_users\"\n    class Meta:\n        abstract = True\n        proxy = False\n";
        let f = extract(Language::Python, py.as_bytes()).unwrap();
        let constants: Vec<(String, String, bool)> = f
            .definitions
            .iter()
            .flat_map(|d| &d.class_constants)
            .map(|c| (c.name.clone(), c.value.clone(), c.literal))
            .collect();
        assert_eq!(
            constants,
            [
                ("__tablename__".into(), "$1_users".into(), false),
                ("abstract".into(), "True".into(), true),
                ("proxy".into(), "False".into(), true),
            ]
        );
    }

    #[test]
    fn implemented_interfaces_are_marked() {
        let java = "class A extends B implements C, D {}\nclass E implements F {}\ninterface G extends H {}\n";
        let f = extract(Language::Java, java.as_bytes()).unwrap();
        let marks = |i: usize| -> Vec<(String, bool)> {
            f.definitions[i]
                .supertypes
                .iter()
                .map(|s| (s.name.clone(), s.implements))
                .collect()
        };
        assert_eq!(
            marks(0),
            [("B".into(), false), ("C".into(), true), ("D".into(), true)]
        );
        assert_eq!(marks(1), [("F".into(), true)]);
        assert_eq!(marks(2), [("H".into(), false)]);
        let ts = "class A extends B implements C {}\n";
        let f = extract(Language::TypeScript, ts.as_bytes()).unwrap();
        let got: Vec<bool> = f.definitions[0]
            .supertypes
            .iter()
            .map(|s| s.implements)
            .collect();
        assert_eq!(got, [false, true]);
    }

    #[test]
    fn non_string_constants_are_kept_as_non_literal_and_parentheses_unwrap() {
        let py = "class M(Base):
    __tablename__ = LOG_TABLE
    a = \"{}_x\".format(p)
    b = \"%s_x\" % P
    c = (\"paren\")
";
        let f = extract(Language::Python, py.as_bytes()).unwrap();
        let constants: Vec<(String, String, bool)> = f.definitions[0]
            .class_constants
            .iter()
            .map(|c| (c.name.clone(), c.value.clone(), c.literal))
            .collect();
        assert_eq!(
            constants,
            [
                ("__tablename__".into(), "LOG_TABLE".into(), false),
                ("a".into(), "\"{}_x\".format(p)".into(), false),
                ("b".into(), "\"%s_x\" % P".into(), false),
                ("c".into(), "paren".into(), true),
            ]
        );
        let java = "@Table(name = (\"paren_t\")) class A {}\n";
        let f = extract(Language::Java, java.as_bytes()).unwrap();
        let arg = &f.definitions[0].decorations[0].args[0];
        assert_eq!((arg.value.as_str(), arg.literal), ("paren_t", true));
    }

    const TS: &str = "@Entity('users') export class User extends Base<Id> implements Named {
  @Column() name: string;
  @Get(':id') find() {}
  @UseGuards(Auth) @Post() save() {}
}
export @Entity({ name: 'orders', schema: 'shop' }) class Order {}
";

    #[test]
    fn typescript_decorators_supertypes_and_object_arguments() {
        assert_eq!(
            decorated(Language::TypeScript, TS),
            [
                "User: @Entity(users)",
                "find: @Get(:id)",
                "save: @UseGuards(Auth)",
                "save: @Post()",
                "Order: @Entity(name=orders, schema=shop)",
            ]
        );
        assert_eq!(supers(Language::TypeScript, TS), ["User: Base<Id> Named"]);
    }

    /// An interface's `extends` list names supertypes (contract M3b amendment 28), each with
    /// the span of its name, so the join can resolve it through the occurrence there.
    #[test]
    fn typescript_interfaces_extend_interfaces() {
        let src = "interface Mid extends Base, Other<T> { extra(): void }\nclass Svc implements Mid {}\nclass Sub extends Svc {}\n";
        assert_eq!(
            supers(Language::TypeScript, src),
            ["Mid: Base Other<T>", "Svc: Mid", "Sub: Svc"]
        );
        let f = extract(Language::TypeScript, src.as_bytes()).unwrap();
        let spans: Vec<(String, bool, crate::Span)> = f
            .definitions
            .iter()
            .flat_map(|d| &d.supertypes)
            .map(|s| (s.name.clone(), s.implements, s.span))
            .collect();
        assert_eq!(
            spans,
            [
                ("Base".into(), false, at(src, 0, "Base")),
                ("Other".into(), false, at(src, 0, "Other")),
                ("Mid".into(), true, at(src, 1, "Mid")),
                ("Svc".into(), false, at(src, 2, "Svc")),
            ]
        );
    }

    /// `line`'s first `token` as a span.
    fn at(src: &str, line: usize, token: &str) -> crate::Span {
        let col = src.lines().nth(line).unwrap().find(token).unwrap() as i32;
        let line = line as i32;
        [line, col, line, col + token.len() as i32]
    }

    /// Contract M3b amendment 43: a supertype's span is its name only, the qualified name as
    /// written, without type arguments (`Marker<Marker.Disc>` → `Marker`) and without type-use
    /// annotations (`@Tag Shape` → `Shape`); a TypeScript `extends` value that is not a name
    /// (`mixin(Abs)`) declares no supertype.
    #[test]
    fn supertype_spans_cover_the_name_only() {
        let java = "package p;\nclass Face extends Marker<Marker.Disc> implements @Tag Shape, a.b.Outer.Inner, Repo<List<String>> {}\n";
        let f = extract(Language::Java, java.as_bytes()).unwrap();
        let spans: Vec<(String, Vec<String>, crate::Span)> = f.definitions[0]
            .supertypes
            .iter()
            .map(|s| (s.name.clone(), s.type_args.clone(), s.span))
            .collect();
        assert_eq!(
            spans,
            [
                (
                    "Marker".into(),
                    vec!["Marker.Disc".into()],
                    at(java, 1, "Marker")
                ),
                ("Shape".into(), vec![], at(java, 1, "Shape")),
                (
                    "a.b.Outer.Inner".into(),
                    vec![],
                    at(java, 1, "a.b.Outer.Inner")
                ),
                (
                    "Repo".into(),
                    vec!["List<String>".into()],
                    at(java, 1, "Repo")
                ),
            ]
        );
        let ts = "class A extends Base<Base.Options> implements NS.Inner, Other<T> {}\nclass B extends NS.Base {}\nclass C extends mixin(Abs) {}\nclass D extends (Abs) {}\n";
        let f = extract(Language::TypeScript, ts.as_bytes()).unwrap();
        let spans: Vec<(String, String, Vec<String>, crate::Span)> = f
            .definitions
            .iter()
            .flat_map(|d| d.supertypes.iter().map(move |s| (d.name.clone(), s)))
            .map(|(d, s)| (d, s.name.clone(), s.type_args.clone(), s.span))
            .collect();
        assert_eq!(
            spans,
            [
                (
                    "A".into(),
                    "Base".into(),
                    vec!["Base.Options".into()],
                    at(ts, 0, "Base")
                ),
                ("A".into(), "NS.Inner".into(), vec![], at(ts, 0, "NS.Inner")),
                (
                    "A".into(),
                    "Other".into(),
                    vec!["T".into()],
                    at(ts, 0, "Other")
                ),
                ("B".into(), "NS.Base".into(), vec![], at(ts, 1, "NS.Base")),
            ],
            "`C extends mixin(Abs)` and `D extends (Abs)` declare no supertype"
        );
    }

    /// Contract M3b amendment 55: comments are named extras in Tree-sitter, so a comment inside
    /// a heritage clause must not become a supertype (`extends /* c */ Base` lost every edge of
    /// the class) or an extra list item; names and spans are the types' only.
    #[test]
    fn comments_in_heritage_clauses_are_not_supertypes() {
        let names_and_spans = |language: Language, src: &str| -> Vec<(String, crate::Span)> {
            extract(language, src.as_bytes())
                .unwrap()
                .definitions
                .iter()
                .flat_map(|d| d.supertypes.iter().map(|s| (s.name.clone(), s.span)))
                .collect()
        };
        let java = "package p;\nclass Split extends /* c */ Base implements\n // c\n I, /* d */ J {}\ninterface K extends /* e */ I, // f\n J {}\nenum Color implements Runnable, /* note */ Cloneable { RED; public void run() {} }\n";
        assert_eq!(
            names_and_spans(Language::Java, java),
            [
                ("Base".into(), at(java, 1, "Base")),
                ("I".into(), at(java, 3, "I")),
                ("J".into(), at(java, 3, "J")),
                ("I".into(), at(java, 4, "I")),
                ("J".into(), at(java, 5, "J")),
                ("Runnable".into(), at(java, 6, "Runnable")),
                ("Cloneable".into(), at(java, 6, "Cloneable")),
            ]
        );
        let python = "class A(  # c\n    Base,  # d\n    Mixin):\n    pass\n";
        assert_eq!(
            names_and_spans(Language::Python, python),
            [
                ("Base".into(), at(python, 1, "Base")),
                ("Mixin".into(), at(python, 2, "Mixin")),
            ]
        );
        let ts = "class A extends /* c */ Base implements I /* d */, // e\n J {}\ninterface K extends /* f */ I, J {}\n";
        assert_eq!(
            names_and_spans(Language::TypeScript, ts),
            [
                ("Base".into(), at(ts, 0, "Base")),
                ("I".into(), at(ts, 0, "I")),
                ("J".into(), at(ts, 1, "J")),
                ("I".into(), at(ts, 2, "I")),
                ("J".into(), at(ts, 2, "J")),
            ]
        );
    }
}
