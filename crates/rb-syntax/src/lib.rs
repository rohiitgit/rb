//! Syntax extraction (design §4.4): Tree-sitter definitions and references per file.
//!
//! [`extract`] is a pure function of `(language, bytes)`; together with [`EXTRACTOR_VERSION`] its
//! output can be cached by blob SHA. [`symbol_keys`] derives stable heuristic symbol keys (design
//! §5.4) from a file's path and facts alone.

mod keys;
mod queries;

use std::collections::{HashMap, HashSet};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser, QueryCursor, StreamingIterator};

pub use keys::symbol_keys;

/// Bump when extraction output can change for the same input. Includes the grammar versions,
/// which a test checks against Cargo.lock.
pub const EXTRACTOR_VERSION: &str = "rb-syntax-1;tree-sitter-0.27.0;typescript-0.23.2;javascript-0.25.0;java-0.23.5;python-0.25.0;go-0.25.0";

/// Files larger than this are not parsed (design §4.1).
pub const MAX_PARSE_BYTES: u64 = 1024 * 1024;

pub type Span = rb_model::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Language {
    TypeScript,
    Tsx,
    JavaScript,
    Java,
    Python,
    Go,
}

impl Language {
    pub fn as_str(self) -> &'static str {
        match self {
            Language::TypeScript => "typescript",
            Language::Tsx => "tsx",
            Language::JavaScript => "javascript",
            Language::Java => "java",
            Language::Python => "python",
            Language::Go => "go",
        }
    }

    /// Maps `FileEntry::lang` (from `rb_git::language_for_path`) to a parser, if one exists.
    pub fn from_lang(lang: &str) -> Option<Self> {
        Some(match lang {
            "typescript" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "javascript" => Language::JavaScript,
            "java" => Language::Java,
            "python" => Language::Python,
            "go" => Language::Go,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DefKind {
    Module,
    Class,
    Interface,
    Type,
    Function,
    Method,
    Constant,
}

impl DefKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DefKind::Module => "module",
            DefKind::Class => "class",
            DefKind::Interface => "interface",
            DefKind::Type => "type",
            DefKind::Function => "function",
            DefKind::Method => "method",
            DefKind::Constant => "constant",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RefKind {
    Call,
    Class,
    Type,
    Implementation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Definition {
    pub kind: DefKind,
    pub name: String,
    pub name_span: Span,
    /// Span of the whole definition node.
    pub span: Span,
    /// Index into `FileFacts::definitions` of the innermost enclosing definition.
    pub parent: Option<usize>,
    /// Go method receiver type, when the type is not defined in the same file (else `parent`).
    pub receiver: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub kind: RefKind,
    pub name: String,
    pub span: Span,
    /// Index into `FileFacts::definitions` of the innermost definition containing the reference.
    pub enclosing: Option<usize>,
}

/// Everything extracted from one blob. Definitions are sorted by position (a Go method's parent
/// may be a type declared later in the file).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileFacts {
    pub definitions: Vec<Definition>,
    pub references: Vec<Reference>,
    /// Span of the first syntax error, if the file did not parse cleanly.
    pub syntax_error: Option<Span>,
}

/// Parses `source` and extracts its definitions and references.
pub fn extract(language: Language, source: &[u8]) -> anyhow::Result<FileFacts> {
    let grammar = queries::for_language(language);
    let mut parser = Parser::new();
    parser.set_language(&grammar.language)?;
    let tree = parser
        .parse(source, None)
        .context("tree-sitter produced no tree")?;
    let root = tree.root_node();

    let (mut raw_defs, mut raw_refs) = capture(grammar, root, source);
    raw_defs.sort_by_key(|d| (d.node.start_byte(), std::cmp::Reverse(d.node.end_byte())));
    raw_refs.sort_by_key(|r| (r.name.start_byte(), r.kind));

    let lexical_parent = nesting(&raw_defs);
    let mut definitions: Vec<Definition> = raw_defs
        .iter()
        .zip(&lexical_parent)
        .map(|(d, parent)| Definition {
            kind: d.kind,
            name: text(d.name, source),
            name_span: span(d.name),
            span: span(d.node),
            parent: *parent,
            receiver: (language == Language::Go)
                .then(|| go_receiver(d.node, source))
                .flatten(),
        })
        .collect();
    attach_go_methods(&mut definitions);
    for i in 0..definitions.len() {
        let parent_is_class = definitions[i]
            .parent
            .is_some_and(|p| definitions[p].kind == DefKind::Class);
        if definitions[i].kind == DefKind::Function && parent_is_class {
            definitions[i].kind = DefKind::Method;
        }
    }

    let starts: Vec<usize> = raw_defs.iter().map(|d| d.node.start_byte()).collect();
    let ends: Vec<usize> = raw_defs.iter().map(|d| d.node.end_byte()).collect();
    let references = raw_refs
        .iter()
        .map(|r| Reference {
            kind: r.kind,
            name: text(r.name, source),
            span: span(r.name),
            enclosing: enclosing(r.name.start_byte(), &starts, &ends, &lexical_parent),
        })
        .collect();

    Ok(FileFacts {
        definitions,
        references,
        syntax_error: root.has_error().then(|| first_error(root)).flatten(),
    })
}

struct RawDef<'t> {
    kind: DefKind,
    node: Node<'t>,
    name: Node<'t>,
}

struct RawRef<'t> {
    kind: RefKind,
    name: Node<'t>,
}

/// Runs the tag query; de-duplicates nodes captured by more than one pattern (first one wins).
fn capture<'t>(
    grammar: &queries::Grammar,
    root: Node<'t>,
    source: &[u8],
) -> (Vec<RawDef<'t>>, Vec<RawRef<'t>>) {
    let names = grammar.query.capture_names();
    let mut defs = Vec::new();
    let mut refs = Vec::new();
    let mut seen_defs = HashSet::new();
    let mut seen_refs = HashSet::new();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&grammar.query, root, source);
    while let Some(m) = matches.next() {
        let mut name = None;
        let mut role = None;
        for c in m.captures() {
            let capture = names[c.index as usize];
            if capture == "name" {
                name = Some(c.node);
            } else if let Some(kind) = capture.strip_prefix("definition.").and_then(def_kind) {
                role = Some((Some(kind), None, c.node));
            } else if let Some(kind) = capture.strip_prefix("reference.").and_then(ref_kind) {
                role = Some((None, Some(kind), c.node));
            }
        }
        let (Some(name), Some(role)) = (name, role) else {
            continue;
        };
        match role {
            (Some(kind), _, node) => {
                if seen_defs.insert((node.byte_range(), name.byte_range())) {
                    defs.push(RawDef { kind, node, name });
                }
            }
            (None, Some(kind), _) => {
                if seen_refs.insert((name.byte_range(), kind)) {
                    refs.push(RawRef { kind, name });
                }
            }
            (None, None, _) => {}
        }
    }
    (defs, refs)
}

fn def_kind(tag: &str) -> Option<DefKind> {
    Some(match tag {
        "module" => DefKind::Module,
        "class" => DefKind::Class,
        "interface" => DefKind::Interface,
        "type" => DefKind::Type,
        "function" => DefKind::Function,
        "method" => DefKind::Method,
        "constant" => DefKind::Constant,
        _ => return None,
    })
}

fn ref_kind(tag: &str) -> Option<RefKind> {
    Some(match tag {
        "call" => RefKind::Call,
        "class" => RefKind::Class,
        "type" => RefKind::Type,
        "implementation" => RefKind::Implementation,
        _ => return None,
    })
}

/// Innermost strictly-enclosing definition of each definition. `defs` is sorted by start
/// ascending, end descending, so a stack sweep suffices.
fn nesting(defs: &[RawDef<'_>]) -> Vec<Option<usize>> {
    let mut stack: Vec<usize> = Vec::new();
    let mut parents = Vec::with_capacity(defs.len());
    for (i, d) in defs.iter().enumerate() {
        while stack
            .last()
            .is_some_and(|&top| defs[top].node.end_byte() < d.node.end_byte())
        {
            stack.pop();
        }
        parents.push(stack.last().copied());
        stack.push(i);
    }
    parents
}

/// Innermost definition whose node contains `byte`: the last definition starting at or before
/// it, then up the lexical parent chain until one also ends after it.
fn enclosing(
    byte: usize,
    starts: &[usize],
    ends: &[usize],
    parent: &[Option<usize>],
) -> Option<usize> {
    let mut candidate = starts.partition_point(|&s| s <= byte).checked_sub(1);
    while let Some(i) = candidate {
        if ends[i] > byte {
            return Some(i);
        }
        candidate = parent[i];
    }
    None
}

/// Go: the receiver's type name of a `method_declaration`, e.g. `Circle` for `(c *Circle)`.
fn go_receiver(node: Node<'_>, source: &[u8]) -> Option<String> {
    let receiver = node.child_by_field_name("receiver")?;
    let mut cursor = receiver.walk();
    let mut stack = vec![receiver];
    while let Some(n) = stack.pop() {
        if n.kind() == "type_identifier" {
            return Some(text(n, source));
        }
        let children: Vec<Node<'_>> = n.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    None
}

/// Go methods become children of their receiver type when it is declared in the same file.
fn attach_go_methods(defs: &mut [Definition]) {
    let types: HashMap<String, usize> = defs
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind == DefKind::Type)
        .rev() // so the first declaration wins
        .map(|(i, d)| (d.name.clone(), i))
        .collect();
    for d in defs.iter_mut() {
        if d.parent.is_none()
            && let Some(&t) = d.receiver.as_ref().and_then(|r| types.get(r))
        {
            d.parent = Some(t);
            d.receiver = None;
        }
    }
}

/// First `ERROR` or `MISSING` node in document order.
fn first_error(root: Node<'_>) -> Option<Span> {
    let mut cursor = root.walk();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        if n.is_error() || n.is_missing() {
            return Some(span(n));
        }
        if n.has_error() {
            let children: Vec<Node<'_>> = n.children(&mut cursor).collect();
            stack.extend(children.into_iter().rev());
        }
    }
    None
}

fn text(node: Node<'_>, source: &[u8]) -> String {
    String::from_utf8_lossy(&source[node.byte_range()]).into_owned()
}

fn span(node: Node<'_>) -> Span {
    let n = |v: usize| i32::try_from(v).unwrap_or(i32::MAX);
    let (s, e) = (node.start_position(), node.end_position());
    [n(s.row), n(s.column), n(e.row), n(e.column)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `kind qualified.name@row:col` per definition, where the qualifier is the parent chain (or
    /// the Go receiver).
    fn outline(language: Language, src: &str) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        let qualified = |i: usize| {
            let mut parts = vec![facts.definitions[i].name.clone()];
            let mut cur = facts.definitions[i].parent;
            while let Some(p) = cur {
                parts.push(facts.definitions[p].name.clone());
                cur = facts.definitions[p].parent;
            }
            if let Some(r) = &facts.definitions[i].receiver {
                parts.push(r.clone());
            }
            parts.reverse();
            parts.join(".")
        };
        facts
            .definitions
            .iter()
            .enumerate()
            .map(|(i, d)| {
                format!(
                    "{} {}@{}:{}",
                    d.kind.as_str(),
                    qualified(i),
                    d.name_span[0],
                    d.name_span[1]
                )
            })
            .collect()
    }

    /// `kind name in enclosing-name` for each reference of the given kind.
    fn refs(language: Language, src: &str, kind: RefKind) -> Vec<String> {
        let facts = extract(language, src.as_bytes()).unwrap();
        facts
            .references
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| {
                let enclosing = r
                    .enclosing
                    .map_or("-".to_owned(), |i| facts.definitions[i].name.clone());
                format!("{} in {enclosing}", r.name)
            })
            .collect()
    }

    const JAVA: &str = "package com.acme;

public class AuthController {
    private final Mfa mfa;
    public AuthController(Mfa mfa) { this.mfa = mfa; }
    public String login(String e) { return mfa.send(e); }
    public String login(String e, String p) { return login(e); }
    public record LoginRequest(String email) {}
    enum Mode { A, B }
}
interface Mfa { String send(String e); }
";

    #[test]
    fn java_definitions_including_records_enums_and_constructors() {
        assert_eq!(
            outline(Language::Java, JAVA),
            [
                "class AuthController@2:13",
                "method AuthController.AuthController@4:11",
                "method AuthController.login@5:18",
                "method AuthController.login@6:18",
                "class AuthController.LoginRequest@7:18",
                "class AuthController.Mode@8:9",
                "interface Mfa@10:10",
                "method Mfa.send@10:23",
            ]
        );
        assert_eq!(
            refs(Language::Java, JAVA, RefKind::Call),
            ["send in login", "login in login"]
        );
    }

    const PYTHON: &str = "import os

class Auth:
    def login(self, email):
        return check(email)

    class Inner:
        pass

def check(email):
    def helper(x):
        return x
    return helper(email)

MAX = 3
";

    #[test]
    fn python_methods_nested_functions_and_constants() {
        assert_eq!(
            outline(Language::Python, PYTHON),
            [
                "class Auth@2:6",
                "method Auth.login@3:8",
                "class Auth.Inner@6:10",
                "function check@9:4",
                "function check.helper@10:8",
                "constant MAX@14:0",
            ]
        );
        assert_eq!(
            refs(Language::Python, PYTHON, RefKind::Call),
            ["check in login", "helper in check"]
        );
    }

    const GO: &str = "package shapes

type Circle struct{ R float64 }
type Square struct{ S float64 }

func (c *Circle) Area() float64 { return c.R * c.R * 3 }
func (s Square) Area() float64 { return s.S * s.S }

func Total(xs []Circle) float64 {
\tt := 0.0
\tfor _, x := range xs {
\t\tt += x.Area()
\t}
\treturn t
}

func (p *Polygon) Sides() int { return 0 }
";

    #[test]
    fn go_methods_attach_to_their_receiver_type() {
        assert_eq!(
            outline(Language::Go, GO),
            [
                "type Circle@2:5",
                "type Square@3:5",
                "method Circle.Area@5:17",
                "method Square.Area@6:16",
                "function Total@8:5",
                // Receiver type not defined in this file: kept as `receiver`, no parent.
                "method Polygon.Sides@16:18",
            ]
        );
        let facts = extract(Language::Go, GO.as_bytes()).unwrap();
        // Same-file receiver types become the lexical parent (so CONTAINS can be drawn).
        let circle = facts.definitions.iter().position(|d| d.name == "Circle");
        for area in facts.definitions.iter().filter(|d| d.name == "Area") {
            assert!(area.parent.is_some() && area.receiver.is_none(), "{area:?}");
        }
        assert_eq!(facts.definitions[2].parent, circle);
        let sides = facts
            .definitions
            .iter()
            .find(|d| d.name == "Sides")
            .unwrap();
        assert_eq!(
            (sides.parent, sides.receiver.as_deref()),
            (None, Some("Polygon"))
        );
        assert_eq!(refs(Language::Go, GO, RefKind::Call), ["Area in Total"]);
    }

    const TS: &str = "export interface Creds { email: string }
export abstract class Base { abstract run(): void; }
export class AuthService extends Base {
  run(): void {}
  login(c: Creds): string { return hash(c.email); }
}
export function hash(s: string): string {
  const inner = (x: string) => x;
  return inner(s);
}
";

    #[test]
    fn typescript_uses_javascript_and_typescript_tags() {
        assert_eq!(
            outline(Language::TypeScript, TS),
            [
                "interface Creds@0:17",
                "class Base@1:22",
                "method Base.run@1:38",
                "class AuthService@2:13",
                "method AuthService.run@3:2",
                "method AuthService.login@4:2",
                "function hash@6:16",
                "function hash.inner@7:8",
            ]
        );
        assert_eq!(
            refs(Language::TypeScript, TS, RefKind::Call),
            ["hash in login", "inner in hash"]
        );
    }

    #[test]
    fn tsx_and_javascript_grammars() {
        let tsx = "export function App() { return <div onClick={() => go()} />; }\n";
        assert_eq!(outline(Language::Tsx, tsx), ["function App@0:16"]);
        assert_eq!(refs(Language::Tsx, tsx, RefKind::Call), ["go in App"]);

        let js = "class K { m() { return new K(); } }\n";
        assert_eq!(
            outline(Language::JavaScript, js),
            ["class K@0:6", "method K.m@0:10"]
        );
        assert_eq!(refs(Language::JavaScript, js, RefKind::Class), ["K in m"]);
    }

    #[test]
    fn syntax_errors_keep_the_well_formed_parts() {
        let src = "def ok():\n    pass\n\ndef broken(:\n";
        let facts = extract(Language::Python, src.as_bytes()).unwrap();
        assert!(facts.definitions.iter().any(|d| d.name == "ok"));
        let err = facts.syntax_error.expect("syntax error reported");
        assert_eq!(err[0], 3, "first error on line 3, got {err:?}");
        assert_eq!(
            extract(Language::Python, b"def ok():\n    pass\n")
                .unwrap()
                .syntax_error,
            None
        );
    }

    #[test]
    fn spans_are_byte_columns_with_crlf_and_non_ascii() {
        let src = "def café():\r\n    pass\r\ndef b():\r\n    pass\r\n";
        let facts = extract(Language::Python, src.as_bytes()).unwrap();
        let spans: Vec<(String, Span)> = facts
            .definitions
            .iter()
            .map(|d| (d.name.clone(), d.name_span))
            .collect();
        assert_eq!(
            spans,
            [
                ("café".to_owned(), [0, 4, 0, 9]),
                ("b".to_owned(), [2, 4, 2, 5])
            ]
        );
    }

    #[test]
    fn facts_round_trip_through_json() {
        let facts = extract(Language::Java, JAVA.as_bytes()).unwrap();
        assert!(!facts.definitions.is_empty());
        let json = serde_json::to_vec(&facts).unwrap();
        assert_eq!(serde_json::from_slice::<FileFacts>(&json).unwrap(), facts);
    }

    #[test]
    fn language_mapping() {
        for (lang, expected) in [
            ("typescript", Some(Language::TypeScript)),
            ("tsx", Some(Language::Tsx)),
            ("javascript", Some(Language::JavaScript)),
            ("sql", None),
        ] {
            assert_eq!(Language::from_lang(lang), expected, "{lang}");
            if let Some(l) = expected {
                assert_eq!(l.as_str(), lang);
            }
        }
    }

    /// EXTRACTOR_VERSION must change whenever a grammar version changes.
    #[test]
    fn extractor_version_matches_cargo_lock() {
        let lock =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.lock"))
                .unwrap();
        let version_of = |name: &str| {
            let marker = format!("name = \"{name}\"\nversion = \"");
            let start = lock
                .find(&marker)
                .unwrap_or_else(|| panic!("{name} not in Cargo.lock"))
                + marker.len();
            lock[start..].split('"').next().unwrap().to_owned()
        };
        for (crate_name, tag) in [
            ("tree-sitter", "tree-sitter"),
            ("tree-sitter-typescript", "typescript"),
            ("tree-sitter-javascript", "javascript"),
            ("tree-sitter-java", "java"),
            ("tree-sitter-python", "python"),
            ("tree-sitter-go", "go"),
        ] {
            let pinned = format!(";{tag}-{};", version_of(crate_name));
            assert!(
                format!("{EXTRACTOR_VERSION};").contains(&pinned),
                "EXTRACTOR_VERSION lacks {pinned}"
            );
        }
    }
}
