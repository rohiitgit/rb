//! Syntax extraction (design §4.4): Tree-sitter definitions and references per file.
//!
//! [`extract`] is a pure function of `(language, bytes)`; together with [`EXTRACTOR_VERSION`] its
//! output can be cached by blob SHA. [`symbol_keys`] derives stable heuristic symbol keys (design
//! §5.4) from a file's path and facts alone.

mod decorations;
mod keys;
mod queries;
mod strings;

use std::collections::{HashMap, HashSet};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser, QueryCursor, StreamingIterator};

pub use keys::symbol_keys;

/// Grammar versions (checked against Cargo.lock by a test). Query text and the extractor's own
/// source code are fingerprinted automatically by [`extractor_id`], so no manual bump is needed.
pub const EXTRACTOR_VERSION: &str = "rb-syntax;tree-sitter-0.27.0;typescript-0.23.2;javascript-0.25.0;java-0.23.5;python-0.25.0;go-0.25.0";

/// The cache identity of this extractor: [`EXTRACTOR_VERSION`] plus a fingerprint of every
/// bundled query and of the extraction source code. Any edit to either invalidates cached facts,
/// so a stale cache can never be served (a manual version bump was forgotten twice).
pub fn extractor_id() -> &'static str {
    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    ID.get_or_init(|| {
        format!(
            "{EXTRACTOR_VERSION};code-{:016x}",
            fnv1a(fingerprint_input().as_bytes())
        )
    })
}

/// Everything that determines `extract`'s output besides the grammars.
fn fingerprint_input() -> String {
    [
        queries::all_sources().as_str(),
        include_str!("lib.rs"),
        include_str!("queries.rs"),
        include_str!("strings.rs"),
        include_str!("decorations.rs"),
    ]
    .join("\n")
}

/// 64-bit FNV-1a: tiny, stable across platforms and releases, good enough for a fingerprint.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Definitions nested deeper than this are dropped (reported via `nesting_truncated`). Keys embed
/// their ancestors, so unbounded depth would make key size quadratic in file size.
pub const MAX_NESTING: usize = 64;

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
    /// Java annotations, Python/TypeScript decorators, in source order.
    #[serde(default)]
    pub decorations: Vec<Decoration>,
    /// Classes and interfaces this definition extends or implements, in source order.
    #[serde(default)]
    pub supertypes: Vec<Supertype>,
    /// Python class bodies: `NAME = 'string'` assignments (e.g. `__tablename__`), in order.
    #[serde(default)]
    pub class_constants: Vec<ClassConstant>,
}

/// An annotation or decorator on a definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decoration {
    /// As written, possibly qualified (`Table`, `app.route`, `javax.persistence.Entity`).
    pub name: String,
    pub args: Vec<DecorationArg>,
}

/// One decoration argument. String literals are decoded; anything else is its source text. An
/// object literal argument (`@Entity({ name: 'x' })`) contributes its pairs as keyed arguments.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DecorationArg {
    pub key: Option<String>,
    pub value: String,
}

/// A named supertype; `name` is as written (possibly qualified), type arguments by simple name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Supertype {
    pub name: String,
    pub type_args: Vec<String>,
}

/// A string constant assigned in a Python class body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassConstant {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reference {
    pub kind: RefKind,
    pub name: String,
    pub span: Span,
    /// Index into `FileFacts::definitions` of the innermost definition containing the reference.
    pub enclosing: Option<usize>,
}

/// A string expression that looks like SQL (design §7.1). The SQL parser decides downstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlCandidate {
    /// Normalised text: escapes decoded, interpolations and non-literal operands as `$n`.
    pub text: String,
    /// Span of the whole string expression.
    pub span: Span,
    /// Index into `FileFacts::definitions` of the innermost definition containing it.
    pub enclosing: Option<usize>,
    /// Name of the function it is a direct argument of (or that tags it, for templates).
    pub callee: Option<String>,
}

/// Everything extracted from one blob. Definitions are sorted by position (a Go method's parent
/// may be a type declared later in the file).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct FileFacts {
    pub definitions: Vec<Definition>,
    pub references: Vec<Reference>,
    /// Span of the first syntax error, if the file did not parse cleanly.
    pub syntax_error: Option<Span>,
    /// Definitions deeper than [`MAX_NESTING`] were dropped.
    #[serde(default)]
    pub nesting_truncated: bool,
    /// SQL-looking string expressions, in source order.
    #[serde(default)]
    pub sql: Vec<SqlCandidate>,
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
    let raw_defs = drop_locals(raw_defs, grammar.anonymous_scopes);
    raw_refs.sort_by_key(|r| (r.name.start_byte(), r.kind));

    let lexical_parent = nesting(&raw_defs);
    let depth = depths(&lexical_parent);
    let kept: Vec<Option<usize>> = kept_indices(&depth);
    let nesting_truncated = kept.iter().any(Option::is_none);
    let mut definitions: Vec<Definition> = raw_defs
        .iter()
        .zip(&lexical_parent)
        .zip(&kept)
        .filter(|(_, new)| new.is_some())
        .map(|((d, parent), _)| Definition {
            kind: d.kind,
            name: text(d.name, source),
            name_span: span(d.name),
            span: span(d.node),
            // A kept definition's parent is shallower, so it is kept too.
            parent: parent.and_then(|p| kept[p]),
            receiver: (language == Language::Go)
                .then(|| go_receiver(d.node, source))
                .flatten(),
            decorations: decorations::decorations(language, d.node, source),
            supertypes: decorations::supertypes(language, d.node, source),
            class_constants: decorations::class_constants(language, d.node, source),
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
            enclosing: enclosing(r.name.start_byte(), &starts, &ends, &lexical_parent)
                .map(|i| nearest_kept(i, &kept, &lexical_parent)),
        })
        .collect();

    Ok(FileFacts {
        definitions,
        references,
        syntax_error: root.has_error().then(|| first_error(root)).flatten(),
        nesting_truncated,
        sql: strings::sql_candidates(language, root, source)
            .into_iter()
            .map(|c| SqlCandidate {
                span: span(c.node),
                enclosing: enclosing(c.node.start_byte(), &starts, &ends, &lexical_parent)
                    .map(|i| nearest_kept(i, &kept, &lexical_parent)),
                callee: c.callee,
                text: c.text,
            })
            .collect(),
    })
}

/// Removes definitions that live inside an anonymous function (callbacks, lambdas, anonymous
/// classes, func literals) or inside another local: they are locals, not symbols (design §8.2).
/// A function value that *is* a captured definition (`const f = () => {}`) is not anonymous.
/// `defs` must be sorted outer-first, so each definition's ancestors are decided before it.
fn drop_locals<'t>(defs: Vec<RawDef<'t>>, anonymous_scopes: &[&str]) -> Vec<RawDef<'t>> {
    let mut is_local: HashMap<usize, bool> = HashMap::with_capacity(defs.len());
    let mut kept = Vec::with_capacity(defs.len());
    for d in defs {
        let local = local_by_ancestry(d.node, &is_local, anonymous_scopes);
        is_local.insert(d.node.id(), local);
        if !local {
            kept.push(d);
        }
    }
    kept
}

fn local_by_ancestry(
    node: Node<'_>,
    is_local: &HashMap<usize, bool>,
    anonymous_scopes: &[&str],
) -> bool {
    let mut cur = node.parent();
    while let Some(n) = cur {
        if let Some(&local) = is_local.get(&n.id()) {
            return local;
        }
        let names_a_definition = n.parent().is_some_and(|p| is_local.contains_key(&p.id()));
        if anonymous_scopes.contains(&n.kind()) && !names_a_definition {
            return true;
        }
        cur = n.parent();
    }
    false
}

/// Nesting depth of each definition (parents precede children in `parent`'s order).
fn depths(parent: &[Option<usize>]) -> Vec<usize> {
    let mut depth = vec![0; parent.len()];
    for i in 0..parent.len() {
        depth[i] = parent[i].map_or(0, |p| depth[p] + 1);
    }
    depth
}

/// New index of each definition within `MAX_NESTING`, `None` for dropped ones.
fn kept_indices(depth: &[usize]) -> Vec<Option<usize>> {
    let mut next = 0;
    depth
        .iter()
        .map(|&d| {
            (d < MAX_NESTING).then(|| {
                next += 1;
                next - 1
            })
        })
        .collect()
}

/// The new index of `i`, or of its closest kept ancestor (depth 0 is always kept).
fn nearest_kept(mut i: usize, kept: &[Option<usize>], parent: &[Option<usize>]) -> usize {
    loop {
        match (kept[i], parent[i]) {
            (Some(new), _) => return new,
            (None, Some(p)) => i = p,
            (None, None) => unreachable!("top-level definitions are always kept"),
        }
    }
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

/// Go methods become children of their receiver type when it is declared at the top level of the
/// same file. Receivers always name package-level types, so types local to a function (including
/// the method's own body) never qualify; that also keeps the parent relation acyclic.
fn attach_go_methods(defs: &mut [Definition]) {
    let types: HashMap<String, usize> = defs
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind == DefKind::Type && d.parent.is_none())
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
    fn go_methods_only_attach_to_top_level_types() {
        // The receiver type is declared inside the method itself: must not become its parent.
        let own_body = "package p\nfunc (t T) M() {\n\ttype T struct{}\n}\n";
        let facts = extract(Language::Go, own_body.as_bytes()).unwrap();
        let m = facts.definitions.iter().find(|d| d.name == "M").unwrap();
        assert_eq!((m.parent, m.receiver.as_deref()), (None, Some("T")));

        // A same-named type local to another function is not the receiver either.
        let other_fn = "package p\nfunc f() {\n\ttype T struct{}\n}\nfunc (t T) M() {}\n";
        assert_eq!(
            outline(Language::Go, other_fn),
            ["function f@1:5", "type f.T@2:6", "method T.M@4:11"]
        );
    }

    #[test]
    fn nesting_is_capped_so_cost_stays_linear() {
        let depth = MAX_NESTING + 40;
        let src = format!("{}{}\n", "function a(){".repeat(depth), "}".repeat(depth));
        let facts = extract(Language::JavaScript, src.as_bytes()).unwrap();
        assert_eq!(facts.definitions.len(), MAX_NESTING);
        assert!(facts.nesting_truncated);
        assert!(
            facts
                .definitions
                .iter()
                .all(|d| d.parent.is_none_or(|p| p < facts.definitions.len()))
        );
        let shallow = extract(Language::JavaScript, b"function a(){ function b(){} }\n").unwrap();
        assert!(!shallow.nesting_truncated);
    }

    #[test]
    fn typescript_type_aliases_and_enums_are_definitions() {
        let src = "export type Id = string\nenum Color { Red }\nexport const enum Dir { Up }\n";
        assert_eq!(
            outline(Language::TypeScript, src),
            ["type Id@0:12", "class Color@1:5", "class Dir@2:18"]
        );
    }

    #[test]
    fn definitions_inside_anonymous_functions_are_locals() {
        // Test-style callbacks: the aliases are locals, not symbols (design §8.2).
        let ts = "describe('a', () => {\n  type Expected = 1\n  function helper() { go() }\n})\ndescribe('b', () => { type Expected = 2 })\nconst f = () => { type T = 1 }\n";
        assert_eq!(
            outline(Language::TypeScript, ts),
            ["function f@5:6", "type f.T@5:23"]
        );
        // A call inside a local definition still counts, attributed to no named definition.
        assert_eq!(
            refs(Language::TypeScript, ts, RefKind::Call),
            ["describe in -", "go in -", "describe in -"]
        );

        let java = "class A {\n  void m() {\n    Runnable r = new Runnable() { public void run() { go(); } };\n  }\n}\n";
        assert_eq!(
            outline(Language::Java, java),
            ["class A@0:6", "method A.m@1:7"]
        );
        assert_eq!(refs(Language::Java, java, RefKind::Call), ["go in m"]);

        let go = "package p\nfunc F() {\n\tg := func() { type L struct{} }\n\ttype K struct{}\n\t_ = g\n}\n";
        assert_eq!(
            outline(Language::Go, go),
            ["function F@1:5", "type F.K@3:6"]
        );
    }

    #[test]
    fn typescript_namespaces_are_modules() {
        let src =
            "namespace N { export function a() {} }\nnamespace M { export function a() {} }\n";
        assert_eq!(
            outline(Language::TypeScript, src),
            [
                "module N@0:10",
                "function N.a@0:30",
                "module M@1:10",
                "function M.a@1:30"
            ]
        );
    }

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

    #[test]
    fn extractor_id_fingerprints_queries_and_code() {
        let id = extractor_id();
        assert!(id.starts_with(EXTRACTOR_VERSION), "{id}");
        let (_, hash) = id.rsplit_once(";code-").expect("code fingerprint present");
        assert_eq!(hash.len(), 16);
        assert_eq!(
            hash,
            format!("{:016x}", fnv1a(fingerprint_input().as_bytes()))
        );
        let input = fingerprint_input();
        assert!(
            input.contains("fn drop_locals"),
            "extractor code is fingerprinted"
        );
        assert!(
            input.contains("JAVA_SUPPLEMENT"),
            "query supplements are fingerprinted"
        );
        // FNV-1a reference value and sensitivity.
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_ne!(fnv1a(b"(a) @name"), fnv1a(b"(b) @name"));
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
