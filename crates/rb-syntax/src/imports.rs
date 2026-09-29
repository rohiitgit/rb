//! Import statements and package clauses, per language (design §5.4, level 3).

use tree_sitter::Node;

use crate::{Import, ImportedName, Language, span, text};

/// Every import in the file, in source order.
pub(crate) fn imports(language: Language, root: Node<'_>, source: &[u8]) -> Vec<Import> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(node) = stack.pop() {
        match (language, node.kind()) {
            (Language::TypeScript | Language::Tsx | Language::JavaScript, "import_statement") => {
                out.extend(es_import(node, source));
            }
            (Language::Java, "import_declaration") => out.extend(java_import(node, source)),
            (Language::Python, "import_statement") => out.extend(python_import(node, source)),
            (Language::Python, "import_from_statement") => {
                out.extend(python_from_import(node, source));
            }
            (Language::Go, "import_spec") => out.extend(go_import(node, source)),
            _ => {
                let mut c = node.walk();
                let children: Vec<Node<'_>> = node.children(&mut c).collect();
                stack.extend(children.into_iter().rev());
            }
        }
    }
    out
}

/// The Java `package` or Go `package` clause.
pub(crate) fn package(language: Language, root: Node<'_>, source: &[u8]) -> Option<String> {
    let kind = match language {
        Language::Java => "package_declaration",
        Language::Go => "package_clause",
        _ => return None,
    };
    let mut c = root.walk();
    let clause = root.named_children(&mut c).find(|n| n.kind() == kind)?;
    let mut cc = clause.walk();
    let name = clause
        .named_children(&mut cc)
        .find(|n| n.kind() != "annotation" && n.kind() != "marker_annotation")?;
    Some(text(name, source))
}

fn import(node: Node<'_>, module: String) -> Import {
    Import {
        module,
        names: Vec::new(),
        alias: None,
        wildcard: false,
        is_static: false,
        span: span(node),
    }
}

fn named(name: String, local: String) -> ImportedName {
    ImportedName { name, local }
}

/// A string literal's content without its quotes.
fn unquote(node: Node<'_>, source: &[u8]) -> String {
    text(node, source)
        .trim_matches(|c| c == '"' || c == '\'' || c == '`')
        .to_owned()
}

/// `import D, { a, b as c } from './x'`, `import * as m from 'x'`, `import './x'`.
fn es_import(node: Node<'_>, source: &[u8]) -> Option<Import> {
    let module = unquote(node.child_by_field_name("source")?, source);
    let mut out = import(node, module);
    let mut c = node.walk();
    for clause in node
        .named_children(&mut c)
        .filter(|n| n.kind() == "import_clause")
    {
        let mut cc = clause.walk();
        for part in clause.named_children(&mut cc) {
            match part.kind() {
                "identifier" => out.names.push(named("default".into(), text(part, source))),
                "namespace_import" => {
                    let mut nc = part.walk();
                    out.alias = part
                        .named_children(&mut nc)
                        .find(|n| n.kind() == "identifier")
                        .map(|n| text(n, source));
                }
                "named_imports" => {
                    let mut nc = part.walk();
                    for spec in part
                        .named_children(&mut nc)
                        .filter(|n| n.kind() == "import_specifier")
                    {
                        let Some(name) = spec.child_by_field_name("name") else {
                            continue;
                        };
                        let name = text(name, source);
                        let local = spec
                            .child_by_field_name("alias")
                            .map_or_else(|| name.clone(), |a| text(a, source));
                        out.names.push(named(name, local));
                    }
                }
                _ => {}
            }
        }
    }
    Some(out)
}

/// `import a.b.C;`, `import a.b.*;`, `import static a.B.m;`, `import static a.B.*;`.
fn java_import(node: Node<'_>, source: &[u8]) -> Option<Import> {
    let mut c = node.walk();
    let children: Vec<Node<'_>> = node.children(&mut c).collect();
    let is_static = children.iter().any(|n| n.kind() == "static");
    let wildcard = children.iter().any(|n| n.kind() == "asterisk");
    let path = children
        .iter()
        .find(|n| matches!(n.kind(), "scoped_identifier" | "identifier"))?;
    let path = text(*path, source);
    let mut out = if wildcard {
        import(node, path)
    } else {
        let (module, name) = path.rsplit_once('.')?;
        let mut out = import(node, module.to_owned());
        out.names.push(named(name.to_owned(), name.to_owned()));
        out
    };
    out.wildcard = wildcard;
    out.is_static = is_static;
    Some(out)
}

/// `import a.b`, `import a.b as c` (one entry per module).
fn python_import(node: Node<'_>, source: &[u8]) -> Vec<Import> {
    let mut c = node.walk();
    node.children_by_field_name("name", &mut c)
        .filter_map(|n| match n.kind() {
            "dotted_name" => Some(import(node, text(n, source))),
            "aliased_import" => {
                let mut out = import(node, text(n.child_by_field_name("name")?, source));
                out.alias = n.child_by_field_name("alias").map(|a| text(a, source));
                Some(out)
            }
            _ => None,
        })
        .collect()
}

/// `from [.]a import b [as c], d` and `from a import *`.
fn python_from_import(node: Node<'_>, source: &[u8]) -> Option<Import> {
    let module = text(node.child_by_field_name("module_name")?, source);
    let mut out = import(node, module.split_whitespace().collect());
    let mut c = node.walk();
    out.wildcard = node
        .named_children(&mut c)
        .any(|n| n.kind() == "wildcard_import");
    let mut c = node.walk();
    for n in node.children_by_field_name("name", &mut c) {
        match n.kind() {
            "dotted_name" => {
                let name = text(n, source);
                out.names.push(named(name.clone(), name));
            }
            "aliased_import" => {
                let Some(name) = n.child_by_field_name("name") else {
                    continue;
                };
                let name = text(name, source);
                let local = n
                    .child_by_field_name("alias")
                    .map_or_else(|| name.clone(), |a| text(a, source));
                out.names.push(named(name, local));
            }
            _ => {}
        }
    }
    Some(out)
}

/// `import "x/y"`, `import f "x/y"` (a spec, alone or inside an import block).
fn go_import(node: Node<'_>, source: &[u8]) -> Option<Import> {
    let mut out = import(node, unquote(node.child_by_field_name("path")?, source));
    out.alias = node.child_by_field_name("name").map(|n| text(n, source));
    Some(out)
}

#[cfg(test)]
mod tests {
    use crate::{Import, Language, extract};

    /// `module [names] alias wildcard static` per import.
    fn summary(language: Language, src: &str) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        facts
            .imports
            .iter()
            .map(|i: &Import| {
                let names: Vec<String> = i
                    .names
                    .iter()
                    .map(|n| {
                        if n.name == n.local {
                            n.name.clone()
                        } else {
                            format!("{}->{}", n.name, n.local)
                        }
                    })
                    .collect();
                let mut s = format!("{} [{}]", i.module, names.join(","));
                if let Some(a) = &i.alias {
                    s.push_str(&format!(" as {a}"));
                }
                if i.wildcard {
                    s.push_str(" *");
                }
                if i.is_static {
                    s.push_str(" static");
                }
                s
            })
            .collect()
    }

    #[test]
    fn typescript_and_javascript_imports() {
        let src = "import D, { a, b as c } from './x';\nimport * as m from 'lib';\nimport './side';\nimport type { T } from \"./types\";\n";
        let expected = [
            "./x [default->D,a,b->c]",
            "lib [] as m",
            "./side []",
            "./types [T]",
        ];
        assert_eq!(summary(Language::TypeScript, src), expected);
        assert_eq!(
            summary(Language::JavaScript, "import D, { a } from './x';\n"),
            ["./x [default->D,a]"]
        );
    }

    #[test]
    fn java_imports_and_package() {
        let src = "package com.acme.web;\nimport java.util.List;\nimport com.acme.*;\nimport static a.B.m;\nimport static a.B.*;\nclass K {}\n";
        assert_eq!(
            summary(Language::Java, src),
            [
                "java.util [List]",
                "com.acme [] *",
                "a.B [m] static",
                "a.B [] * static"
            ]
        );
        let facts = extract(Language::Java, src.as_bytes()).unwrap();
        assert_eq!(facts.package.as_deref(), Some("com.acme.web"));
    }

    #[test]
    fn python_imports() {
        let src = "import a.b\nimport a.b as c, d\nfrom . import x\nfrom ..p.q import y as z, w\nfrom m import *\ndef f():\n    import inner\n";
        assert_eq!(
            summary(Language::Python, src),
            [
                "a.b []",
                "a.b [] as c",
                "d []",
                ". [x]",
                "..p.q [y->z,w]",
                "m [] *",
                "inner []"
            ]
        );
    }

    #[test]
    fn go_imports_and_package() {
        let src = "package svc\nimport \"fmt\"\nimport (\n  f \"x/y\"\n  _ \"z\"\n)\n";
        assert_eq!(
            summary(Language::Go, src),
            ["fmt []", "x/y [] as f", "z [] as _"]
        );
        let facts = extract(Language::Go, src.as_bytes()).unwrap();
        assert_eq!(facts.package.as_deref(), Some("svc"));
    }
}
