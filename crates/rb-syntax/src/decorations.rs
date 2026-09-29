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
                "element_value_pair" | "keyword_argument" => args.push(DecorationArg {
                    key: field_text(arg, &["key", "name"], source),
                    value: arg
                        .child_by_field_name("value")
                        .map(|v| value_text(language, v, source))
                        .unwrap_or_default(),
                }),
                // TypeORM-style options objects: their pairs become keyed arguments.
                "object" => {
                    let mut pc = arg.walk();
                    for pair in arg.named_children(&mut pc).filter(|p| p.kind() == "pair") {
                        args.push(DecorationArg {
                            key: pair
                                .child_by_field_name("key")
                                .map(|k| value_text(language, k, source)),
                            value: pair
                                .child_by_field_name("value")
                                .map(|v| value_text(language, v, source))
                                .unwrap_or_default(),
                        });
                    }
                }
                "comment" => {}
                _ => args.push(DecorationArg {
                    key: None,
                    value: value_text(language, arg, source),
                }),
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
            if let Some(t) = def
                .child_by_field_name("superclass")
                .and_then(|s| s.named_child(0))
            {
                out.push(supertype(t, source));
            }
            for list in def.children(&mut cursor) {
                if matches!(list.kind(), "super_interfaces" | "extends_interfaces") {
                    out.extend(
                        type_list_items(list)
                            .into_iter()
                            .map(|t| supertype(t, source)),
                    );
                }
            }
        }
        Language::Python => {
            if let Some(bases) = def.child_by_field_name("superclasses") {
                let mut c = bases.walk();
                out.extend(
                    bases
                        .named_children(&mut c)
                        .filter(|b| b.kind() != "keyword_argument")
                        .map(|b| supertype(b, source)),
                );
            }
        }
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            for heritage in def
                .children(&mut cursor)
                .filter(|c| c.kind() == "class_heritage")
            {
                let mut hc = heritage.walk();
                for clause in heritage.named_children(&mut hc) {
                    match clause.kind() {
                        // `extends Base<Id>`: the type arguments are a sibling of the value.
                        "extends_clause" => {
                            if let Some(value) = clause.child_by_field_name("value") {
                                out.push(Supertype {
                                    name: text(value, source),
                                    type_args: clause
                                        .child_by_field_name("type_arguments")
                                        .map(|a| named_texts(a, source))
                                        .unwrap_or_default(),
                                });
                            }
                        }
                        "implements_clause" => {
                            let mut cc = clause.walk();
                            out.extend(
                                clause.named_children(&mut cc).map(|t| supertype(t, source)),
                            );
                        }
                        _ => {}
                    }
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
            Some(ClassConstant {
                name: text(left, source),
                value: crate::strings::string_value(language, right, source)?,
            })
        })
        .collect()
}

/// A type node (`Foo`, `a.b.Foo`, `Foo<A, B>`, Python `models.Model`) as a supertype.
fn supertype(node: Node<'_>, source: &[u8]) -> Supertype {
    if node.kind() == "generic_type" {
        let name = node
            .named_child(0)
            .map(|n| text(n, source))
            .unwrap_or_default();
        let mut c = node.walk();
        let type_args = node
            .named_children(&mut c)
            .find(|n| n.kind() == "type_arguments")
            .map(|a| named_texts(a, source))
            .unwrap_or_default();
        return Supertype { name, type_args };
    }
    Supertype {
        name: text(node, source),
        type_args: Vec::new(),
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
        .collect()
}

fn named_texts(node: Node<'_>, source: &[u8]) -> Vec<String> {
    let mut c = node.walk();
    node.named_children(&mut c)
        .map(|n| text(n, source))
        .collect()
}

fn value_text(language: Language, node: Node<'_>, source: &[u8]) -> String {
    crate::strings::string_value(language, node, source).unwrap_or_else(|| text(node, source))
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
}
