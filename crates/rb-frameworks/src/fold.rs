//! URL constant folding (contract M4a S12): a URL template's holes resolve through constants in
//! effect, imported constants, concatenation, env references with a literal default, Java
//! `@Value` fields and Spring placeholders backed by `application.properties`. Mutable bindings
//! never fold.
//!
//! Termination is by construction: an on-path guard on bindings, a depth cap
//! ([`MAX_FOLD_DEPTH`]) and a step budget ([`MAX_FOLD_STEPS`]); only results computed without
//! hitting any of them are memoised (lesson 2026-09-30).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rb_syntax::{Binding, DefKind, FileFacts, Init, Language, Span, StrTemplate, TemplatePart};

use crate::endpoint::Piece;
use crate::routes::FileScope;

/// Nested resolutions followed at most.
pub const MAX_FOLD_DEPTH: usize = 16;
/// Name resolutions per fold at most, so even a wide cyclic input stays bounded.
pub const MAX_FOLD_STEPS: usize = 10_000;

/// Where a folded value comes from, weakest last. A result is labelled by the weakest source
/// that contributed text to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    Unresolved,
    EnvDefault,
    Config,
    Constant,
    Literal,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Unresolved => "unresolved",
            Origin::EnvDefault => "env_default",
            Origin::Config => "config",
            Origin::Constant => "constant",
            Origin::Literal => "literal",
        }
    }
}

/// A folded value: literal pieces, holes nothing resolved, and its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folded {
    pub pieces: Vec<Piece>,
    /// The weakest source among the parts that resolved; [`Origin::Unresolved`] when nothing
    /// resolved to text.
    pub origin: Origin,
}

impl Folded {
    fn hole() -> Self {
        Folded {
            pieces: vec![Piece::Hole],
            origin: Origin::Unresolved,
        }
    }

    fn text(text: String, origin: Origin) -> Self {
        Folded {
            pieces: vec![Piece::Lit(text)],
            origin,
        }
    }

    /// The text, when no hole is left.
    pub fn literal(&self) -> Option<String> {
        self.pieces
            .iter()
            .map(|p| match p {
                Piece::Lit(l) => Some(l.as_str()),
                Piece::Hole => None,
            })
            .collect()
    }
}

/// What folding needs beyond one file's facts. `file` identifies a file to the caller.
pub trait Lookup {
    fn facts(&self, file: usize) -> &FileFacts;
    fn language(&self, file: usize) -> Language;
    /// A name not bound in `file` (an imported constant, `Type.CONST` of another file, Go
    /// `pkg.Const`), written at `at`: the file and binding index it names, if any. The position
    /// lets a local variable shadow a same-named package or type qualifier.
    fn external(&self, file: usize, name: &str, at: Span) -> Option<(usize, usize)>;
    /// The `application.properties` value of `key` for the service owning `file`.
    fn property(&self, file: usize, key: &str) -> Option<String>;
}

/// The value of a URL template written in `file`.
pub fn fold_template(lookup: &dyn Lookup, file: usize, template: &StrTemplate) -> Folded {
    Folder::new(lookup).template(file, template, 0).0
}

/// The value of a name (`API`, `this.base`, `Paths.ROOT`) written in `file` at `at`.
pub fn fold_name(lookup: &dyn Lookup, file: usize, name: &str, at: Span) -> Folded {
    Folder::new(lookup).name(file, name, at, 0).0
}

/// Whether a name reads the environment (`process.env.X`, `import.meta.env.X`).
pub fn is_env(name: &str) -> bool {
    name.starts_with("process.env.") || name.starts_with("import.meta.env.")
}

struct Folder<'l> {
    lookup: &'l dyn Lookup,
    /// Each file's lookup indexes, built on first use.
    scopes: HashMap<usize, FileScope<'l>>,
    on_path: HashSet<(usize, Span)>,
    memo: HashMap<(usize, Span), Folded>,
    steps: usize,
}

/// A result and whether it is clean (no guard, cap or budget was hit while computing it).
type Out = (Folded, bool);

impl<'l> Folder<'l> {
    fn new(lookup: &'l dyn Lookup) -> Self {
        Folder {
            lookup,
            scopes: HashMap::new(),
            on_path: HashSet::new(),
            memo: HashMap::new(),
            steps: 0,
        }
    }

    fn template(&mut self, file: usize, t: &StrTemplate, depth: usize) -> Out {
        let mut pieces = Vec::new();
        let mut origin = Origin::Literal;
        let mut any_text = false;
        let mut clean = true;
        for part in &t.parts {
            match part {
                TemplatePart::Lit { text } => {
                    let f = if self.lookup.language(file) == Language::Java && text.contains("${") {
                        placeholders(self.lookup, file, text)
                    } else {
                        Folded::text(text.clone(), Origin::Literal)
                    };
                    if f.origin != Origin::Unresolved {
                        any_text = true;
                        origin = origin.min(f.origin);
                    }
                    pieces.extend(f.pieces);
                }
                TemplatePart::Hole { expr, span } => {
                    let (f, c) = self.name(file, expr, *span, depth);
                    clean &= c;
                    if f.origin != Origin::Unresolved {
                        any_text = true;
                        origin = origin.min(f.origin);
                    }
                    pieces.extend(f.pieces);
                }
            }
        }
        let origin = if any_text { origin } else { Origin::Unresolved };
        (Folded { pieces, origin }, clean)
    }

    fn name(&mut self, file: usize, name: &str, at: Span, depth: usize) -> Out {
        self.steps += 1;
        if depth >= MAX_FOLD_DEPTH || self.steps > MAX_FOLD_STEPS {
            return (Folded::hole(), false);
        }
        if is_env(name) {
            return (Folded::hole(), true);
        }
        let lookup = self.lookup;
        let language = lookup.language(file);
        let scope = self
            .scopes
            .entry(file)
            .or_insert_with(|| FileScope::new(lookup.facts(file)));
        let local = local_binding(scope, language, name, at)
            .map(|b| (file, b))
            .or_else(|| lookup.external(file, name, at));
        match local {
            Some((f, b)) => self.binding(f, b, depth + 1),
            None => (Folded::hole(), true),
        }
    }

    fn binding(&mut self, file: usize, index: usize, depth: usize) -> Out {
        let facts = self.lookup.facts(file);
        let Some(b) = facts.bindings.get(index) else {
            return (Folded::hole(), true);
        };
        let key = (file, b.span);
        if let Some(done) = self.memo.get(&key) {
            return (done.clone(), true);
        }
        if !self.on_path.insert(key) {
            return (Folded::hole(), false);
        }
        let out = self.binding_value(file, b, depth);
        self.on_path.remove(&key);
        if out.1 {
            self.memo.insert(key, out.0.clone());
        }
        out
    }

    fn binding_value(&mut self, file: usize, b: &Binding, depth: usize) -> Out {
        // `@Value("${k:d}")` (Java): configuration, whatever the field's mutability.
        if let Some(value) = b
            .decorations
            .iter()
            .find(|d| d.name.rsplit('.').next() == Some("Value"))
            .and_then(|d| {
                d.args
                    .iter()
                    .find(|a| a.key.is_none() || a.key.as_deref() == Some("value"))
            })
        {
            return (placeholders(self.lookup, file, &value.value), true);
        }
        if !b.constant {
            return (Folded::hole(), true);
        }
        match &b.init {
            Some(Init::Template { template }) => {
                let (f, clean) = self.template(file, template, depth);
                (relabel(f, Origin::Constant), clean)
            }
            Some(Init::Name { text, span }) => {
                let (f, clean) = self.name(file, text, *span, depth);
                (relabel(f, Origin::Constant), clean)
            }
            Some(Init::Fallback { primary, default }) => {
                if is_env(primary) {
                    let (f, clean) = self.template(file, default, depth);
                    return (relabel(f, Origin::EnvDefault), clean);
                }
                let (f, clean) = self.name(file, primary, b.span, depth);
                (relabel(f, Origin::Constant), clean)
            }
            Some(Init::Call { .. }) | None => (Folded::hole(), true),
        }
    }
}

/// A folded constant is at best `constant`: its text was not written at the use.
fn relabel(mut f: Folded, at_best: Origin) -> Folded {
    if f.origin != Origin::Unresolved {
        f.origin = f.origin.min(at_best);
    }
    f
}

/// The binding `name` refers to within its own file: a local or parameter in effect, a member
/// (`this.x`, a Java field by its bare name) of an enclosing type, or `Type.CONST` of a type
/// declared in the file. Returns its index in `facts.bindings`.
pub(crate) fn local_binding(
    facts: &FileScope<'_>,
    language: Language,
    name: &str,
    at: Span,
) -> Option<usize> {
    if !name.contains('.')
        && let Some(i) = facts.binding_index_at(name, at)
    {
        return Some(i);
    }
    let (owner, member) = match name.strip_prefix("this.") {
        Some(member) if !member.contains('.') => (None, member),
        _ if !name.contains('.') && language == Language::Java => (None, name),
        _ => match name.rsplit_once('.') {
            Some((owner, member)) if !owner.contains('.') => (Some(owner), member),
            _ => return None,
        },
    };
    let contains =
        |span: Span| (span[0], span[1]) <= (at[0], at[1]) && (at[2], at[3]) <= (span[2], span[3]);
    facts
        .members(member)
        .iter()
        .map(|&i| (i, &facts.bindings[i]))
        .filter(|(_, b)| {
            let Some(scope) = b.scope.map(|s| &facts.definitions[s]) else {
                return false;
            };
            if !matches!(scope.kind, DefKind::Class | DefKind::Interface) {
                return false;
            }
            match owner {
                Some(owner) => scope.name == owner,
                None => contains(scope.span),
            }
        })
        // The innermost enclosing type wins.
        .max_by_key(|(_, b)| b.scope)
        .map(|(i, _)| i)
}

/// Resolves `${k}` / `${k:default}` placeholders in a Spring string.
fn placeholders(lookup: &dyn Lookup, file: usize, text: &str) -> Folded {
    let mut pieces = Vec::new();
    let mut origin = Origin::Literal;
    let mut resolved_any = false;
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        pieces.push(Piece::Lit(rest[..start].to_owned()));
        if start > 0 {
            resolved_any = true;
        }
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            pieces.push(Piece::Lit(rest[start..].to_owned()));
            rest = "";
            break;
        };
        let inner = &after[..end];
        let (key, default) = match inner.split_once(':') {
            Some((k, d)) => (k.trim(), Some(d)),
            None => (inner.trim(), None),
        };
        match (lookup.property(file, key), default) {
            (Some(v), _) => {
                pieces.push(Piece::Lit(v));
                origin = origin.min(Origin::Config);
                resolved_any = true;
            }
            (None, Some(d)) => {
                pieces.push(Piece::Lit(d.to_owned()));
                origin = origin.min(Origin::EnvDefault);
                resolved_any = true;
            }
            (None, None) => pieces.push(Piece::Hole),
        }
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        pieces.push(Piece::Lit(rest.to_owned()));
        resolved_any = true;
    }
    let pieces: Vec<Piece> = pieces
        .into_iter()
        .filter(|p| !matches!(p, Piece::Lit(l) if l.is_empty()))
        .collect();
    Folded {
        pieces,
        origin: if resolved_any {
            origin
        } else {
            Origin::Unresolved
        },
    }
}

/// A Java `.properties` file as key → value: `key=value`, `key: value` or `key value`; `#` and
/// `!` comments; a trailing backslash continues the line; `\t \n \r \f \uHHHH` and other
/// escaped characters are decoded. The last assignment of a key wins.
pub fn parse_properties(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut logical = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if logical.is_empty() && (trimmed.is_empty() || trimmed.starts_with(['#', '!'])) {
            continue;
        }
        let trailing = trimmed.len() - trimmed.trim_end_matches('\\').len();
        if trailing % 2 == 1 && lines.peek().is_some() {
            logical.push_str(&trimmed[..trimmed.len() - 1]);
            continue;
        }
        logical.push_str(trimmed);
        let entry = std::mem::take(&mut logical);
        let (key, value) = split_property(&entry);
        out.insert(unescape(&key), unescape(value.trim_start()));
    }
    out
}

/// The dotted keys a Spring YAML file sets (`spring:` then `  cloud:` gives `spring` and
/// `spring.cloud`), read by indentation: comments, document markers and list items are skipped
/// and quotes around a key dropped. Not a YAML parser (none is a dependency): it answers only
/// whether a file sets a key, for the `config_unread` caps (evaluator pass 1, P7).
pub fn yaml_keys(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            if trimmed.starts_with("---") {
                stack.clear();
            }
            continue;
        }
        let Some(key) = yaml_key(trimmed) else {
            continue;
        };
        let indent = line.len() - trimmed.len();
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let full = stack
            .iter()
            .map(|(_, k)| k.as_str())
            .chain([key])
            .collect::<Vec<_>>()
            .join(".");
        out.insert(full);
        stack.push((indent, key.to_owned()));
    }
    out
}

/// The key of a `key: value` or `key:` line, unquoted.
fn yaml_key(line: &str) -> Option<&str> {
    let colon = line
        .char_indices()
        .find(|&(i, c)| c == ':' && line[i + 1..].chars().next().is_none_or(char::is_whitespace))
        .map(|(i, _)| i)?;
    let key = line[..colon].trim();
    let key = key
        .strip_prefix('\'')
        .and_then(|k| k.strip_suffix('\''))
        .or_else(|| key.strip_prefix('"').and_then(|k| k.strip_suffix('"')))
        .unwrap_or(key);
    (!key.is_empty() && !key.contains(char::is_whitespace)).then_some(key)
}

/// Splits at the first unescaped `=`, `:` or whitespace (with surrounding whitespace).
fn split_property(line: &str) -> (String, &str) {
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '=' | ':' => return (line[..i].trim_end().to_owned(), &line[i + 1..]),
            c if c.is_whitespace() => {
                let rest = line[i..].trim_start();
                let rest = rest.strip_prefix(['=', ':']).unwrap_or(rest);
                return (line[..i].to_owned(), rest);
            }
            _ => {}
        }
    }
    (line.to_owned(), "")
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
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => {
                        out.push_str("\\u");
                        out.push_str(&hex);
                    }
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rb_syntax::extract;

    /// Files by index, with TS named imports and Java/Go qualified names resolved by module
    /// path → file index, and one properties map.
    struct Files {
        files: Vec<(Language, FileFacts, &'static str)>,
        properties: BTreeMap<String, String>,
    }

    impl Files {
        fn new(files: &[(Language, &'static str, &str)]) -> Self {
            Files {
                files: files
                    .iter()
                    .map(|(l, path, src)| (*l, extract(*l, src.as_bytes()).unwrap(), *path))
                    .collect(),
                properties: BTreeMap::new(),
            }
        }

        fn top_level(&self, file: usize, name: &str) -> Option<usize> {
            let facts = &self.files[file].1;
            facts.bindings.iter().position(|b| {
                b.name == name
                    && (b.scope.is_none()
                        || facts.definitions[b.scope.unwrap()].kind == DefKind::Class)
            })
        }
    }

    impl Lookup for Files {
        fn facts(&self, file: usize) -> &FileFacts {
            &self.files[file].1
        }
        fn language(&self, file: usize) -> Language {
            self.files[file].0
        }
        fn external(&self, file: usize, name: &str, _at: Span) -> Option<(usize, usize)> {
            let facts = &self.files[file].1;
            let (head, member) = name
                .rsplit_once('.')
                .map_or((None, name), |(h, m)| (Some(h), m));
            let target = match head {
                // TS: a named import.
                None => {
                    let import = facts
                        .imports
                        .iter()
                        .find(|i| i.names.iter().any(|n| n.local == name))?;
                    let module = import.module.trim_start_matches("./");
                    self.files.iter().position(|(_, _, p)| *p == module)?
                }
                // Java `Type.CONST` / Go `pkg.Const`: a file named like the qualifier.
                Some(h) => self.files.iter().position(|(_, _, p)| *p == h)?,
            };
            Some((target, self.top_level(target, member)?))
        }
        fn property(&self, _file: usize, key: &str) -> Option<String> {
            self.properties.get(key).cloned()
        }
    }

    fn show(f: &Folded) -> String {
        let pieces: String = f
            .pieces
            .iter()
            .map(|p| match p {
                Piece::Lit(l) => l.clone(),
                Piece::Hole => "{?}".to_owned(),
            })
            .collect();
        format!("{pieces} [{}]", f.origin.as_str())
    }

    /// Folds the first argument of the last call in file 0.
    fn fold_last_call(files: &Files) -> String {
        let facts = files.facts(0);
        let chain = facts.chains.last().unwrap();
        match &chain.links.last().unwrap().args[0] {
            rb_syntax::Arg::Template { template } => show(&fold_template(files, 0, template)),
            rb_syntax::Arg::Name { text, span } => show(&fold_name(files, 0, text, *span)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn same_file_constants_concatenation_and_env_defaults() {
        let cases = [
            (
                "const A = \"http://a\";\nget(`${A}/x`);\n",
                "http://a/x [constant]",
            ),
            ("get(\"/lit\");\n", "/lit [literal]"),
            (
                "const H = \"http://h\";\nconst P = H + \"/p\";\nget(P + \"/x\");\n",
                "http://h/p/x [constant]",
            ),
            (
                "const API = import.meta.env.VITE_API_URL ?? \"http://gateway\";\nget(`${API}/api/v1/login`);\n",
                "http://gateway/api/v1/login [env_default]",
            ),
            (
                "const B = process.env.B || 'http://b';\nget(B);\n",
                "http://b [env_default]",
            ),
            ("get(`${process.env.X}/y`);\n", "{?}/y [literal]"),
            // Mutable bindings never fold.
            ("let M = \"http://m\";\nget(`${M}/x`);\n", "{?}/x [literal]"),
            ("function f(id) { get(`/u/${id}`); }\n", "/u/{?} [literal]"),
            // A call is not folded.
            ("const C = mk();\nget(C);\n", "{?} [unresolved]"),
            // A shadowing local hides the constant.
            (
                "const S = \"/outer\";\nfunction f() { let S = x; get(S); }\n",
                "{?} [unresolved]",
            ),
        ];
        for (src, want) in cases {
            let files = Files::new(&[(Language::TypeScript, "a", src)]);
            assert_eq!(fold_last_call(&files), want, "{src}");
        }
    }

    #[test]
    fn java_fields_values_and_placeholders() {
        let src = "class C {\n  static final String BASE = \"http://auth\";\n  String mutable = \"http://m\";\n  @Value(\"${auth.url:http://default}\") String authUrl;\n  @Value(\"${missing}\") String missing;\n  void m(String id) {\n    a(BASE + \"/x/\" + id);\n    a(mutable + \"/m\");\n    a(authUrl + \"/v\");\n    a(missing + \"/n\");\n    a(this.authUrl);\n    a(C.BASE);\n    a(\"${feign.url}/p\");\n  }\n}\n";
        let mut files = Files::new(&[(Language::Java, "C", src)]);
        let all = |files: &Files| -> Vec<String> {
            let facts = files.facts(0);
            facts
                .chains
                .iter()
                .map(|c| match &c.links[0].args[0] {
                    rb_syntax::Arg::Template { template } => {
                        show(&fold_template(files, 0, template))
                    }
                    rb_syntax::Arg::Name { text, span } => show(&fold_name(files, 0, text, *span)),
                    other => panic!("{other:?}"),
                })
                .collect()
        };
        assert_eq!(
            all(&files),
            [
                "http://auth/x/{?} [constant]",
                "{?}/m [literal]",
                "http://default/v [env_default]",
                "{?}/n [literal]",
                "http://default [env_default]",
                "http://auth [constant]",
                "{?}/p [literal]",
            ]
        );
        files
            .properties
            .insert("auth.url".into(), "http://configured".into());
        files
            .properties
            .insert("feign.url".into(), "http://notification:3000".into());
        let got = all(&files);
        assert_eq!(got[2], "http://configured/v [config]");
        assert_eq!(got[6], "http://notification:3000/p [config]");
    }

    #[test]
    fn imported_constants_across_files() {
        let files = Files::new(&[
            (
                Language::TypeScript,
                "a",
                "import { BASE } from \"./b\";\nget(`${BASE}/x`);\n",
            ),
            (
                Language::TypeScript,
                "b",
                "import { ROOT } from \"./c\";\nexport const BASE = ROOT + \"/api\";\n",
            ),
            (
                Language::TypeScript,
                "c",
                "export const ROOT = \"http://svc\";\n",
            ),
        ]);
        assert_eq!(fold_last_call(&files), "http://svc/api/x [constant]");
        let java = Files::new(&[
            (
                Language::Java,
                "C",
                "class C { void m() { a(Paths.ROOT + \"/x\"); } }\n",
            ),
            (
                Language::Java,
                "Paths",
                "class Paths { static final String ROOT = \"/root\"; }\n",
            ),
        ]);
        assert_eq!(fold_last_call(&java), "/root/x [constant]");
        let go = Files::new(&[
            (
                Language::Go,
                "main",
                "package main\nfunc f() { get(cfg.Base + \"/x\") }\n",
            ),
            (
                Language::Go,
                "cfg",
                "package cfg\nconst Base = \"http://billing\"\n",
            ),
        ]);
        assert_eq!(fold_last_call(&go), "http://billing/x [constant]");
        // A Go `var` is mutable: never folded, even across packages.
        let go = Files::new(&[
            (
                Language::Go,
                "main2",
                "package main\nfunc f() { get(cfg.Mutable + \"/x\") }\n",
            ),
            (
                Language::Go,
                "cfg",
                "package cfg\nvar Mutable = \"http://m\"\n",
            ),
        ]);
        assert_eq!(fold_last_call(&go), "{?}/x [literal]");
    }

    /// Every recursion edge has its cycle test: same file (Java fields see each other), across
    /// files through imports, and a self-reference.
    #[test]
    fn cycles_terminate_unresolved() {
        let java = Files::new(&[(
            Language::Java,
            "C",
            "class C {\n  static final String A = B;\n  static final String B = A;\n  static final String S = S + \"/x\";\n  void m() { a(A); a(S); }\n}\n",
        )]);
        let facts = java.facts(0);
        let got: Vec<String> = facts
            .chains
            .iter()
            .map(|c| match &c.links[0].args[0] {
                rb_syntax::Arg::Name { text, span } => show(&fold_name(&java, 0, text, *span)),
                other => panic!("{other:?}"),
            })
            .collect();
        // `S = S + "/x"` keeps its literal part around the hole it cannot fill.
        assert_eq!(got, ["{?} [unresolved]", "{?}/x [constant]"]);

        let ts = Files::new(&[
            (
                Language::TypeScript,
                "a",
                "import { B } from \"./b\";\nexport const A = B;\nget(A);\n",
            ),
            (
                Language::TypeScript,
                "b",
                "import { A } from \"./a\";\nexport const B = A;\n",
            ),
        ]);
        assert_eq!(fold_last_call(&ts), "{?} [unresolved]");
    }

    /// A 1,000-link chain is cut at the depth cap; a short one resolves.
    #[test]
    fn long_chains_stop_at_the_depth_cap() {
        let mut src = String::from("const A0 = \"/x\";\n");
        for i in 1..1000 {
            src.push_str(&format!("const A{i} = A{};\n", i - 1));
        }
        src.push_str("get(A10);\nget(A999);\n");
        let files = Files::new(&[(Language::TypeScript, "a", &src)]);
        let facts = files.facts(0);
        let got: Vec<String> = facts
            .chains
            .iter()
            .map(|c| match &c.links[0].args[0] {
                rb_syntax::Arg::Name { text, span } => show(&fold_name(&files, 0, text, *span)),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(got, ["/x [constant]", "{?} [unresolved]"]);
    }

    /// A wide diamond (each constant the concatenation of the one below with itself) costs one
    /// resolution per constant and per use, not one per path: clean results are memoised, so
    /// 2^14 paths take at most two resolutions per level.
    #[test]
    fn wide_diamonds_are_memoised() {
        let mut src = String::from("const D0 = \"a\";\n");
        for i in 1..15 {
            src.push_str(&format!("const D{i} = D{} + D{};\n", i - 1, i - 1));
        }
        src.push_str("get(D14);\n");
        let files = Files::new(&[(Language::TypeScript, "a", &src)]);
        let facts = files.facts(0);
        let rb_syntax::Arg::Name { text, span } = &facts.chains[0].links[0].args[0] else {
            panic!()
        };
        let mut folder = Folder::new(&files);
        let (folded, clean) = folder.name(0, text, *span, 0);
        assert!(clean);
        assert_eq!(folded.literal().unwrap(), "a".repeat(1 << 14));
        assert!(folder.steps <= 2 * 15, "{} resolutions", folder.steps);
    }

    /// Evaluator pass 1, P5: a result computed while a guard was hit is never memoised. In
    /// `A + B` with `A = B + "x"` and `B = A`, folding `A` meets `B` on the way back to `A` (the
    /// on-path guard): `B` is a hole there only because `A` was on the path. Memoised, the later
    /// `B` would read as that hole instead of its own value, `{?}x`.
    #[test]
    fn results_cut_by_a_guard_are_not_memoised() {
        let java = Files::new(&[(
            Language::Java,
            "C",
            "class C {\n  static final String A = B + \"x\";\n  static final String B = A;\n  void m() { a(A + B); }\n}\n",
        )]);
        assert_eq!(fold_last_call(&java), "{?}x{?}x [constant]");
    }

    /// Evaluator pass 1, P5: the step budget bounds a wide input whose results are never clean
    /// (so never memoised): each `Di = D(i-1) + D(i-1)` down to a self-reference costs 2^14
    /// resolutions without the budget; with it, folding stops after `MAX_FOLD_STEPS` and the
    /// result is not clean.
    #[test]
    fn the_step_budget_bounds_unclean_diamonds() {
        let mut src = String::from("const Z = Z;\nconst D0 = Z + \"a\";\n");
        for i in 1..15 {
            src.push_str(&format!("const D{i} = D{} + D{};\n", i - 1, i - 1));
        }
        src.push_str("get(D14);\n");
        let files = Files::new(&[(Language::TypeScript, "a", &src)]);
        let facts = files.facts(0);
        let rb_syntax::Arg::Name { text, span } = &facts.chains[0].links[0].args[0] else {
            panic!()
        };
        let mut folder = Folder::new(&files);
        let (folded, clean) = folder.name(0, text, *span, 0);
        assert!(!clean);
        assert!(folded.pieces.contains(&Piece::Hole));
        // Past the budget, each pending level returns after one more (refused) resolution.
        assert!(
            folder.steps <= MAX_FOLD_STEPS + MAX_FOLD_DEPTH,
            "{} resolutions",
            folder.steps
        );
    }

    /// Evaluator pass 1, P7: the dotted keys a Spring YAML file sets, by indentation (no YAML
    /// parser is a dependency); used only to ask whether an unread file sets a key, so a key too
    /// many is a cap too many, never a wrong value.
    #[test]
    fn yaml_keys_are_dotted_by_indentation() {
        let text = "# comment\nspring:\n  application:\n    name: b\n  cloud.config:\n    uri: \"http://config:8888\"\n\nusers:\n  url: http://legacy  # trailing\n  list:\n    - a: 1\n'quoted.key': x\n---\nserver:\n  port: 1\n  servlet.context-path: /accounts\nnot a key\n";
        let keys: Vec<String> = yaml_keys(text).into_iter().collect();
        assert_eq!(
            keys,
            [
                "quoted.key",
                "server",
                "server.port",
                "server.servlet.context-path",
                "spring",
                "spring.application",
                "spring.application.name",
                "spring.cloud.config",
                "spring.cloud.config.uri",
                "users",
                "users.list",
                "users.url",
            ]
        );
    }

    #[test]
    fn properties_files() {
        let text = "# comment\n! also\nserver.servlet.context-path=/auth\nfeign.url : http://n:3000\nkey value with spaces\nlong = a\\\n    b\nesc=tab\\there \\u0041\nempty=\nserver.servlet.context-path=/last\n";
        let p = parse_properties(text);
        assert_eq!(
            p.get("server.servlet.context-path").map(String::as_str),
            Some("/last")
        );
        assert_eq!(
            p.get("feign.url").map(String::as_str),
            Some("http://n:3000")
        );
        assert_eq!(p.get("key").map(String::as_str), Some("value with spaces"));
        assert_eq!(p.get("long").map(String::as_str), Some("ab"));
        assert_eq!(p.get("esc").map(String::as_str), Some("tab\there A"));
        assert_eq!(p.get("empty").map(String::as_str), Some(""));
        assert_eq!(p.len(), 6);
    }
}
