//! Mentions of names, by role (contract M4b S1): every identifier, type identifier and
//! property-name token of a Java or TS/JS file, aggregated per name, plus the whole words of
//! string literals. Consumers allow-list the roles a name may take (S6 gate M, S9 (c)).

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Range;

use tree_sitter::Node;

use crate::{Language, MentionRole};

pub(crate) fn mentions(
    language: Language,
    root: Node<'_>,
    source: &[u8],
    declared: &HashSet<Range<usize>>,
) -> BTreeMap<String, BTreeSet<MentionRole>> {
    let mut out: BTreeMap<String, BTreeSet<MentionRole>> = BTreeMap::new();
    let java = match language {
        Language::Java => true,
        Language::TypeScript | Language::Tsx | Language::JavaScript => false,
        Language::Python | Language::Go => return out,
    };
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        let kind = node.kind();
        if kind.ends_with("string_fragment") {
            for word in words(&source[node.byte_range()]) {
                out.entry(word).or_default().insert(MentionRole::String);
            }
            continue;
        }
        if TOKENS.contains(&kind) {
            let role = if declared.contains(&node.byte_range()) {
                MentionRole::Declaration
            } else if java {
                java_role(node, source)
            } else {
                es_role(node)
            };
            let name = String::from_utf8_lossy(&source[node.byte_range()]).into_owned();
            out.entry(name).or_default().insert(role);
            continue;
        }
        let mut c = node.walk();
        stack.extend(node.children(&mut c));
    }
    out
}

/// The token kinds recorded: identifiers, type identifiers and property names.
const TOKENS: &[&str] = &[
    "identifier",
    "type_identifier",
    "property_identifier",
    "shorthand_property_identifier",
    "shorthand_property_identifier_pattern",
    "private_property_identifier",
];

/// The whole words of a string fragment: maximal runs of identifier characters.
fn words(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$'))
        .filter(|w| !w.is_empty())
        .map(str::to_owned)
        .collect()
}

fn is_field(parent: Node<'_>, field: &str, child: Node<'_>) -> bool {
    parent
        .child_by_field_name(field)
        .is_some_and(|f| f.id() == child.id())
}

/// The role of a Java token, from the structures it sits in (innermost first).
fn java_role(node: Node<'_>, source: &[u8]) -> MentionRole {
    let mut child = node;
    let mut member = false;
    while let Some(parent) = child.parent() {
        match parent.kind() {
            "import_declaration" | "package_declaration" => return MentionRole::Import,
            "annotation" | "marker_annotation" => {
                if is_field(parent, "name", child) {
                    return MentionRole::Annotation;
                }
                break;
            }
            "class_literal" => {
                return if is_logger_argument(parent, source) {
                    MentionRole::Logger
                } else {
                    MentionRole::ClassLiteral
                };
            }
            "method_reference" => {
                return if parent.named_child(0).is_some_and(|f| f.id() == child.id()) {
                    MentionRole::MethodRef
                } else {
                    MentionRole::Other
                };
            }
            "object_creation_expression" => {
                if is_field(parent, "type", child) {
                    return MentionRole::New;
                }
                break;
            }
            "superclass" | "super_interfaces" | "extends_interfaces" => {
                return MentionRole::Supertype;
            }
            "type_arguments" => return MentionRole::Type,
            "type_list"
            | "generic_type"
            | "scoped_type_identifier"
            | "annotated_type"
            | "scoped_identifier" => {}
            "field_access" => member |= is_field(parent, "object", child),
            "method_invocation" => {
                if is_field(parent, "object", child) {
                    return MentionRole::Member;
                }
                break;
            }
            _ => break,
        }
        child = parent;
    }
    if member {
        MentionRole::Member
    } else if node.kind() == "type_identifier" {
        MentionRole::Type
    } else {
        MentionRole::Other
    }
}

/// A class literal that is the only argument of a `getLogger(…)` call.
fn is_logger_argument(literal: Node<'_>, source: &[u8]) -> bool {
    let Some(arguments) = literal.parent().filter(|a| a.kind() == "argument_list") else {
        return false;
    };
    let mut c = arguments.walk();
    let only = arguments
        .named_children(&mut c)
        .filter(|n| !matches!(n.kind(), "line_comment" | "block_comment"))
        .count()
        == 1;
    only && arguments
        .parent()
        .filter(|call| call.kind() == "method_invocation")
        .and_then(|call| call.child_by_field_name("name"))
        .is_some_and(|name| &source[name.byte_range()] == b"getLogger")
}

/// The role of a TS/JS token, from the structures it sits in (innermost first).
fn es_role(node: Node<'_>) -> MentionRole {
    let mut child = node;
    let mut member = false;
    while let Some(parent) = child.parent() {
        match parent.kind() {
            "decorator" => return MentionRole::Annotation,
            "import_specifier" | "export_specifier" => {
                let aliased = parent.child_by_field_name("alias").is_some();
                return if aliased && is_field(parent, "name", child) {
                    MentionRole::Other
                } else {
                    MentionRole::Import
                };
            }
            "import_clause" | "namespace_import" | "named_imports" | "import_statement" => {
                return MentionRole::Import;
            }
            "extends_clause" => {
                if is_field(parent, "value", child) {
                    return MentionRole::Supertype;
                }
                break;
            }
            // JavaScript's heritage is `extends <expression>` with no clause node.
            "implements_clause" | "extends_type_clause" | "class_heritage" => {
                return MentionRole::Supertype;
            }
            "type_arguments" => return MentionRole::Type,
            "new_expression" => {
                if is_field(parent, "constructor", child) {
                    return if member {
                        MentionRole::Member
                    } else {
                        MentionRole::New
                    };
                }
                break;
            }
            "member_expression" | "nested_identifier" | "nested_type_identifier" => {
                let qualifier = is_field(parent, "object", child)
                    || is_field(parent, "module", child)
                    || parent.named_child(0).is_some_and(|f| {
                        f.id() == child.id() && parent.kind() == "nested_identifier"
                    });
                member |= qualifier;
            }
            "call_expression" => {
                if !is_field(parent, "function", child) {
                    break;
                }
            }
            "generic_type" => {}
            _ => break,
        }
        child = parent;
    }
    if member {
        MentionRole::Member
    } else if node.kind() == "type_identifier" {
        MentionRole::Type
    } else {
        MentionRole::Other
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::MentionRole::{self, *};
    use crate::{Language, extract};

    fn roles(language: Language, src: &str, name: &str) -> Vec<MentionRole> {
        extract(language, src.as_bytes())
            .unwrap()
            .mentions
            .get(name)
            .map(|r| r.iter().copied().collect())
            .unwrap_or_default()
    }

    /// One row per role: the name `X` occurs in that role only.
    #[test]
    fn java_one_row_per_role() {
        for (src, want) in [
            ("class X {}", vec![Declaration]),
            ("class K { X() {} void X() {} }", vec![Declaration]),
            ("import a.b.X;", vec![Import]),
            ("import static a.b.X.m;", vec![Import]),
            ("package a.X;", vec![Import]),
            ("class K extends X {}", vec![Supertype]),
            ("class K implements a.X, Y {}", vec![Supertype]),
            ("interface K extends X {}", vec![Supertype]),
            ("class K extends B<X> {}", vec![Type]),
            (
                "class K { X f; List<X> g; void m(X p) throws X { Object o = (X) p; } }",
                vec![Type],
            ),
            ("class K { Object o = new X(); }", vec![New]),
            ("class K { Object o = new X() { }; }", vec![New]),
            ("class K { Object o = new a.X(); }", vec![New]),
            ("class K { Object o = X.class; }", vec![ClassLiteral]),
            ("@Import(X.class) class K {}", vec![ClassLiteral]),
            (
                "class K { Object o = X::new; Object p = X::make; }",
                vec![MethodRef],
            ),
            (
                "class K { Object o = X.y; Object p = X.make(); }",
                vec![Member],
            ),
            ("@X class K {}", vec![Annotation]),
            ("@a.X(\"v\") class K {}", vec![Annotation]),
            (
                "class K { Logger l = LoggerFactory.getLogger(X.class); }",
                vec![Logger],
            ),
            ("class K { String s = \"com.x.X\"; }", vec![String]),
            (
                "class K { void m() { X(); y.X(); int X = 1; } }",
                vec![Other],
            ),
            ("class K { Object o = a.X.y; }", vec![Member]),
        ] {
            assert_eq!(roles(Language::Java, src, "X"), want, "{src}");
        }
    }

    #[test]
    fn java_qualified_names_logger_and_two_roles() {
        // A qualified type: each segment takes the position's role.
        let src = "class K { a.b.X f = new a.b.X(); }";
        assert_eq!(roles(Language::Java, src, "a"), [Type, New]);
        assert_eq!(roles(Language::Java, src, "X"), [Type, New]);
        // A class literal next to another argument of getLogger is a class literal.
        let src = "class K { Object l = getLogger(X.class, 1); }";
        assert_eq!(roles(Language::Java, src, "X"), [ClassLiteral]);
        // A string's words, each.
        let src = "class K { String s = \"com.x.Foo\"; }";
        for word in ["com", "x", "Foo"] {
            assert_eq!(roles(Language::Java, src, word), [String], "{word}");
        }
        // The same name in two roles in one file.
        let src = "import a.X;\nclass K { X f = new X(); }";
        assert_eq!(roles(Language::Java, src, "X"), [Import, Type, New]);
        // Comments are not mentions.
        assert!(roles(Language::Java, "// X\nclass K {}", "X").is_empty());
    }

    #[test]
    fn typescript_one_row_per_role() {
        for (src, want) in [
            ("class X {}", vec![Declaration]),
            ("export default class X {}", vec![Declaration]),
            ("function X() {}", vec![Declaration]),
            ("interface X {}", vec![Declaration]),
            ("import { X } from './x';", vec![Import]),
            ("import X from './x';", vec![Import]),
            ("import * as X from './x';", vec![Import]),
            ("export { X } from './x';", vec![Import]),
            ("class K extends X {}", vec![Supertype]),
            ("class K implements X {}", vec![Supertype]),
            ("interface K extends X {}", vec![Supertype]),
            ("class K extends B<X> {}", vec![Type]),
            ("let k: X; let j: Array<X>;", vec![Type]),
            ("const o = new X();", vec![New]),
            ("const o = new m.X();", vec![New]),
            ("const o = X.y; X.make();", vec![Member]),
            ("@X() class K {}", vec![Annotation]),
            ("const s = 'X';", vec![String]),
            ("const s = `a ${b} X`;", vec![String]),
            ("X(); const X = 1; export default X;", vec![Other]),
            // An aliased specifier: the importer or exporter uses another name.
            ("import { X as Y } from './x';", vec![Other]),
            ("export { X as Y };", vec![Other]),
            ("const C = m.X;", vec![Other]),
        ] {
            assert_eq!(roles(Language::TypeScript, src, "X"), want, "{src}");
        }
        // The alias itself is an import; a namespace qualifier is a member.
        assert_eq!(
            roles(Language::TypeScript, "import { X as Y } from './x';", "Y"),
            [Import]
        );
        assert_eq!(
            roles(Language::TypeScript, "const o = new m.X();", "m"),
            [Member]
        );
        assert_eq!(
            roles(Language::JavaScript, "const o = new X();", "X"),
            [New]
        );
        assert_eq!(
            roles(Language::JavaScript, "class K extends X {}", "X"),
            [Supertype]
        );
    }

    #[test]
    fn python_and_go_record_no_mentions() {
        for (language, src) in [
            (Language::Python, "class X:\n    pass\n"),
            (Language::Go, "package p\ntype X struct{}\n"),
        ] {
            let facts = extract(language, src.as_bytes()).unwrap();
            assert_eq!(facts.mentions, Default::default());
        }
        let _: BTreeSet<MentionRole> = BTreeSet::new();
    }
}
