//! SQL candidates (design §7.1): string expressions whose text looks like SQL.
//!
//! A candidate is a string literal, a template string, or a `+` concatenation containing at least
//! one literal. Its text is normalised: escapes are decoded, and every interpolation or non-literal
//! operand becomes a positional parameter `$1`, `$2`, … Only candidates whose text starts with a
//! SQL keyword are kept; the SQL parser decides the rest downstream.

use tree_sitter::Node;

use crate::Language;

/// A string expression that may be SQL, before its enclosing definition is resolved.
pub(crate) struct RawCandidate<'t> {
    pub node: Node<'t>,
    pub text: String,
    /// Name of the function this expression is a direct argument of (or tags, for templates).
    pub callee: Option<String>,
}

/// Statements a candidate's text must start with (after whitespace and SQL comments).
const SQL_KEYWORDS: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "MERGE", "CREATE", "ALTER", "DROP", "TRUNCATE",
];

/// Node kinds that make up string expressions and calls, per grammar.
struct Syntax {
    literals: &'static [&'static str],
    /// Interpolations inside a literal (template substitutions, f-string fields).
    holes: &'static [&'static str],
    /// Binary `+` expressions.
    concat: &'static [&'static str],
    /// Adjacent literals that the language joins (Python `"a" "b"`).
    implicit_concat: &'static [&'static str],
    arguments: &'static [&'static str],
    calls: &'static [&'static str],
    /// Framework metadata (annotations, decorators) is left to framework packs.
    skip: &'static [&'static str],
    /// Literal kinds whose content has no escapes (Go raw strings).
    raw_literals: &'static [&'static str],
    /// `"""` literals are Java text blocks (opening line and common indentation dropped).
    text_blocks: bool,
}

fn syntax(language: Language) -> Syntax {
    match language {
        Language::TypeScript | Language::Tsx | Language::JavaScript => Syntax {
            literals: &["string", "template_string"],
            holes: &["template_substitution"],
            concat: &["binary_expression"],
            implicit_concat: &[],
            arguments: &["arguments"],
            calls: &["call_expression"],
            skip: &["decorator"],
            raw_literals: &[],
            text_blocks: false,
        },
        Language::Java => Syntax {
            literals: &["string_literal"],
            holes: &[],
            concat: &["binary_expression"],
            implicit_concat: &[],
            arguments: &["argument_list"],
            calls: &["method_invocation"],
            skip: &["annotation", "marker_annotation"],
            raw_literals: &[],
            text_blocks: true,
        },
        Language::Python => Syntax {
            literals: &["string"],
            holes: &["interpolation"],
            concat: &["binary_operator"],
            implicit_concat: &["concatenated_string"],
            arguments: &["argument_list"],
            calls: &["call"],
            skip: &["decorator"],
            raw_literals: &[],
            text_blocks: false,
        },
        Language::Go => Syntax {
            literals: &["interpreted_string_literal", "raw_string_literal"],
            holes: &[],
            concat: &["binary_expression"],
            implicit_concat: &[],
            arguments: &["argument_list"],
            calls: &["call_expression"],
            skip: &[],
            raw_literals: &["raw_string_literal"],
            text_blocks: false,
        },
    }
}

pub(crate) fn sql_candidates<'t>(
    language: Language,
    root: Node<'t>,
    source: &[u8],
) -> Vec<RawCandidate<'t>> {
    let syn = syntax(language);
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut cursor = root.walk();
    // Pre-order, so the outermost string expression is met first and its parts are not revisited.
    while let Some(node) = stack.pop() {
        if syn.skip.contains(&node.kind()) {
            continue;
        }
        if is_string_expr(node, &syn) {
            let mut holes = 0;
            let text = flatten(node, &syn, source, &mut holes);
            if looks_like_sql(&text) {
                out.push(RawCandidate {
                    node,
                    text,
                    callee: callee(node, &syn, source),
                });
            }
            continue;
        }
        let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    out
}

fn is_plus(node: Node<'_>, syn: &Syntax) -> bool {
    syn.concat.contains(&node.kind())
        && node
            .child_by_field_name("operator")
            .is_some_and(|op| op.kind() == "+")
}

fn is_string_expr(node: Node<'_>, syn: &Syntax) -> bool {
    let kind = node.kind();
    if syn.literals.contains(&kind) || syn.implicit_concat.contains(&kind) {
        return true;
    }
    is_plus(node, syn)
        && [
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ]
        .into_iter()
        .flatten()
        .any(|operand| is_string_expr(operand, syn))
}

/// The normalised text of a string expression; non-literal parts become `$n`.
fn flatten(node: Node<'_>, syn: &Syntax, source: &[u8], holes: &mut usize) -> String {
    let kind = node.kind();
    if syn.literals.contains(&kind) {
        return literal_text(node, syn, source, holes);
    }
    if syn.implicit_concat.contains(&kind) {
        let mut cursor = node.walk();
        let parts: Vec<Node<'_>> = node.named_children(&mut cursor).collect();
        return parts
            .into_iter()
            .map(|p| flatten(p, syn, source, holes))
            .collect();
    }
    if is_plus(node, syn) {
        let operands = [
            node.child_by_field_name("left"),
            node.child_by_field_name("right"),
        ];
        return operands
            .into_iter()
            .flatten()
            .map(|operand| {
                if is_string_expr(operand, syn) {
                    flatten(operand, syn, source, holes)
                } else {
                    hole(holes)
                }
            })
            .collect();
    }
    hole(holes)
}

fn hole(holes: &mut usize) -> String {
    *holes += 1;
    format!("${holes}")
}

/// A literal's value: interpolations replaced, delimiters and prefixes removed, escapes decoded.
fn literal_text(node: Node<'_>, syn: &Syntax, source: &[u8], holes: &mut usize) -> String {
    let mut text = String::new();
    let mut at = node.start_byte();
    let mut cursor = node.walk();
    let children: Vec<Node<'_>> = node.children(&mut cursor).collect();
    for child in children
        .into_iter()
        .filter(|c| syn.holes.contains(&c.kind()))
    {
        text.push_str(&String::from_utf8_lossy(&source[at..child.start_byte()]));
        text.push_str(&hole(holes));
        at = child.end_byte();
    }
    text.push_str(&String::from_utf8_lossy(&source[at..node.end_byte()]));

    let prefix: String = text.chars().take_while(char::is_ascii_alphabetic).collect();
    let rest = &text[prefix.len()..];
    let width = if rest.starts_with("\"\"\"") || rest.starts_with("'''") {
        3
    } else {
        1
    };
    let body = rest
        .get(width..rest.len().saturating_sub(width))
        .unwrap_or_default();
    let raw = prefix.contains(['r', 'R']) || syn.raw_literals.contains(&node.kind());
    let body = if width == 3 && syn.text_blocks {
        java_text_block(body)
    } else {
        body.to_owned()
    };
    if raw { body } else { unescape(&body) }
}

/// Java text blocks drop the opening line and the indentation common to all lines (JLS 3.10.6).
fn java_text_block(body: &str) -> String {
    let mut lines: Vec<&str> = body.split('\n').collect();
    if lines.len() > 1 {
        lines.remove(0);
    }
    let last = lines.len().saturating_sub(1);
    let indent = lines
        .iter()
        .enumerate()
        .filter(|(i, l)| *i == last || !l.trim().is_empty())
        .map(|(_, l)| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| l.get(indent..).unwrap_or_default().trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('\n') | None => {} // line continuation
            Some(other) => out.push(other),
        }
    }
    out
}

/// Name of the call this expression is a direct argument of, or the tag of a tagged template.
fn callee(node: Node<'_>, syn: &Syntax, source: &[u8]) -> Option<String> {
    let parent = node.parent()?;
    let call = if syn.arguments.contains(&parent.kind()) {
        parent.parent()?
    } else {
        parent
    };
    if !syn.calls.contains(&call.kind()) {
        return None;
    }
    let function = call
        .child_by_field_name("function")
        .or_else(|| call.child_by_field_name("name"))?;
    let name = ["property", "attribute", "field"]
        .iter()
        .find_map(|f| function.child_by_field_name(f))
        .unwrap_or(function);
    name.kind()
        .contains("identifier")
        .then(|| String::from_utf8_lossy(&source[name.byte_range()]).into_owned())
}

/// Starts with a SQL statement keyword once leading whitespace and SQL comments are skipped.
fn looks_like_sql(text: &str) -> bool {
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, r)| r);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, r)| r);
        } else {
            break;
        }
    }
    let word: String = rest.chars().take_while(char::is_ascii_alphabetic).collect();
    SQL_KEYWORDS.contains(&word.to_ascii_uppercase().as_str())
}

#[cfg(test)]
mod tests {
    use crate::{Language, extract};

    /// `(text, callee, enclosing definition name)` per candidate.
    fn candidates(language: Language, src: &str) -> Vec<(String, Option<String>, Option<String>)> {
        let facts = extract(language, src.as_bytes()).unwrap();
        facts
            .sql
            .iter()
            .map(|c| {
                (
                    c.text.clone(),
                    c.callee.clone(),
                    c.enclosing.map(|i| facts.definitions[i].name.clone()),
                )
            })
            .collect()
    }

    fn c(
        text: &str,
        callee: Option<&str>,
        enclosing: Option<&str>,
    ) -> (String, Option<String>, Option<String>) {
        (
            text.into(),
            callee.map(Into::into),
            enclosing.map(Into::into),
        )
    }

    #[test]
    fn typescript_templates_tags_concatenation_and_escapes() {
        let src = r#"
export async function find(id: string, table: string) {
  await db.query(`SELECT * FROM users WHERE id = ${id} AND org = ${org}`);
  await sql`DELETE FROM sessions WHERE uid = ${id}`;
  const q = "SELECT * FROM " + table + " WHERE x = 1";
  log("Update failed");
  return pool.query("SELECT *\nFROM orders");
}
const greeting = "hello";
"#;
        assert_eq!(
            candidates(Language::TypeScript, src),
            [
                c(
                    "SELECT * FROM users WHERE id = $1 AND org = $2",
                    Some("query"),
                    Some("find")
                ),
                c(
                    "DELETE FROM sessions WHERE uid = $1",
                    Some("sql"),
                    Some("find")
                ),
                c("SELECT * FROM $1 WHERE x = 1", None, Some("find")),
                c("Update failed", Some("log"), Some("find")),
                c("SELECT *\nFROM orders", Some("query"), Some("find")),
            ]
        );
    }

    #[test]
    fn java_literals_text_blocks_and_concatenation() {
        let src = "class Repo {\n  String name(long id) {\n    return jdbc.queryForObject(\"SELECT name FROM users WHERE id = ?\", String.class, id);\n  }\n  void all() {\n    jdbc.update(\"\"\"\n        UPDATE users\n        SET active = true\n        \"\"\");\n    String q = \"SELECT * \" + \"FROM teams\";\n  }\n}\n";
        assert_eq!(
            candidates(Language::Java, src),
            [
                c(
                    "SELECT name FROM users WHERE id = ?",
                    Some("queryForObject"),
                    Some("name")
                ),
                c(
                    "UPDATE users\nSET active = true\n",
                    Some("update"),
                    Some("all")
                ),
                c("SELECT * FROM teams", None, Some("all")),
            ]
        );
    }

    #[test]
    fn annotations_and_decorators_are_framework_metadata_not_candidates() {
        let java = "interface R {\n  @Query(\"SELECT u FROM User u\")\n  List<User> all();\n}\n";
        assert!(candidates(Language::Java, java).is_empty());
        let py = "@app.route(\"SELECT\")\ndef f():\n    pass\n";
        assert!(candidates(Language::Python, py).is_empty());
    }

    #[test]
    fn python_fstrings_raw_and_implicit_concatenation() {
        let src = "def load(cur, uid):\n    cur.execute(f\"SELECT * FROM users WHERE id = {uid}\")\n    cur.execute(\"SELECT * \" \"FROM orders\")\n    q = r\"DELETE FROM t WHERE p ~ '\\d'\"\n\nQ = 'WITH x AS (SELECT 1) SELECT * FROM x'\n";
        assert_eq!(
            candidates(Language::Python, src),
            [
                c(
                    "SELECT * FROM users WHERE id = $1",
                    Some("execute"),
                    Some("load")
                ),
                c("SELECT * FROM orders", Some("execute"), Some("load")),
                c("DELETE FROM t WHERE p ~ '\\d'", None, Some("load")),
                // A module-level constant owns its literal (more precise than the file).
                c("WITH x AS (SELECT 1) SELECT * FROM x", None, Some("Q")),
            ]
        );
    }

    #[test]
    fn go_raw_and_interpreted_strings() {
        let src = "package p\nfunc Get(db *sql.DB, id int) {\n\tdb.QueryRow(`SELECT name\n  FROM users WHERE id = $1`, id)\n\tdb.Exec(\"DELETE FROM \" + table)\n}\n";
        assert_eq!(
            candidates(Language::Go, src),
            [
                c(
                    "SELECT name\n  FROM users WHERE id = $1",
                    Some("QueryRow"),
                    Some("Get")
                ),
                c("DELETE FROM $1", Some("Exec"), Some("Get")),
            ]
        );
    }

    #[test]
    fn only_sql_looking_strings_are_kept() {
        let src = "const a = 'hello'; const b = '  -- note\\n select 1'; const c = 'selection'; const d = `/* x */ INSERT INTO t VALUES (1)`;\n";
        let texts: Vec<String> = candidates(Language::JavaScript, src)
            .into_iter()
            .map(|c| c.0)
            .collect();
        assert_eq!(
            texts,
            ["  -- note\n select 1", "/* x */ INSERT INTO t VALUES (1)"]
        );
    }
}
