//! Tier B resolution (design §5.4): `CALLS`, `REFERENCES` and `IMPLEMENTS` edges from Tree-sitter
//! facts alone, with provenance `heuristic`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance, Span,
};
use rb_syntax::{Binding, DefKind, Definition, FileFacts, Init, Language, RefKind};
use serde_json::json;

use crate::modules::{Modules, dir};

/// A definition: its file and its index in that file's `FileFacts::definitions`.
type Sym<'a> = (&'a str, usize);

/// Inheritance levels searched for a member; deeper hierarchies stay unresolved.
const MAX_INHERITANCE_DEPTH: usize = 64;

/// Adds the resolved edges for every file of `index`; returns one `unresolved_references`
/// diagnostic per file that has unresolved call or type references. Files in `excluded` (those an
/// exact SCIP index covers) get no edges and no diagnostic, but their definitions still resolve
/// references from other files.
pub(crate) fn add_edges(
    b: &mut GraphBuilder,
    index: &Index<'_>,
    excluded: &BTreeSet<&str>,
) -> anyhow::Result<Vec<Diagnostic>> {
    let mut edges = Edges::default();
    let mut diagnostics = Vec::new();
    for (&path, (language, facts)) in index.files {
        if excluded.contains(path) {
            continue;
        }
        let keys = &index.keys[path];
        let source = |enclosing: Option<usize>| match enclosing {
            Some(i) => NodeKey::new(NodeKind::Symbol, keys[i].clone()),
            None => NodeKey::new(NodeKind::File, path),
        };
        let (mut calls, mut types, mut external) = (0u64, 0u64, 0u64);
        let mut forms: BTreeMap<&str, u64> = BTreeMap::new();
        for r in &facts.references {
            index.forms.borrow_mut().clear();
            let at = Evidence {
                path: path.to_owned(),
                span: r.span,
            };
            let (found, is_call) = match r.kind {
                RefKind::Call => (
                    index.call(
                        path,
                        r.enclosing,
                        &r.name,
                        r.qualifier.as_deref(),
                        Some(r.span),
                    ),
                    true,
                ),
                RefKind::Class | RefKind::Type => {
                    // The name in `type Conn struct{}` is the declaration, not a reference.
                    if r.enclosing
                        .is_some_and(|i| facts.definitions[i].name_span == r.span)
                    {
                        continue;
                    }
                    let name = match &r.qualifier {
                        Some(q) => format!("{q}.{}", r.name),
                        None => r.name.clone(),
                    };
                    let types = index.resolve_type(path, r.enclosing, &name);
                    (
                        types
                            .into_iter()
                            .map(|t| (EdgeKind::References, t))
                            .collect(),
                        false,
                    )
                }
                // `implements X` becomes `IMPLEMENTS` below.
                RefKind::Implementation => continue,
            };
            if found.is_empty() {
                let head = r
                    .qualifier
                    .as_deref()
                    .map_or(r.name.as_str(), |q| q.split('.').next().unwrap_or(q));
                if r.qualifier.is_none() && is_builtin(*language, &r.name) {
                    // Built-ins are not resolution failures.
                } else if index.is_external(path, r.enclosing, head, Some(r.span)) {
                    external += 1;
                } else if is_call {
                    calls += 1;
                } else {
                    types += 1;
                }
                continue;
            }
            // Each new form the resolved reference used counts once (contract M9c D6).
            for &form in index.forms.borrow().iter() {
                *forms.entry(form).or_default() += 1;
            }
            edges.record_all(source(r.enclosing), &found, index, &at);
        }
        if calls + types + external > 0 {
            diagnostics.push(Diagnostic {
                kind: "unresolved_references".into(),
                path: Some(path.to_owned()),
                span: None,
                detail: json!({
                    "calls": calls,
                    "types": types,
                    "external": external,
                    "language": language.as_str(),
                }),
            });
        }
        if !forms.is_empty() {
            let detail: serde_json::Map<String, serde_json::Value> = TIER_B_FORMS
                .iter()
                .map(|&k| ((*k).to_owned(), json!(forms.get(k).copied().unwrap_or(0))))
                .collect();
            diagnostics.push(Diagnostic {
                kind: "tier_b_forms".into(),
                path: Some(path.to_owned()),
                span: None,
                detail: detail.into(),
            });
        }
        index.implements(path, facts, &mut edges);
    }
    edges.emit(b)?;
    Ok(diagnostics)
}

/// Go's predeclared identifiers and common Python built-ins: references to them are neither
/// resolved nor counted as failures.
const GO_BUILTINS: &[&str] = &[
    "bool",
    "byte",
    "complex64",
    "complex128",
    "error",
    "float32",
    "float64",
    "int",
    "int8",
    "int16",
    "int32",
    "int64",
    "rune",
    "string",
    "uint",
    "uint8",
    "uint16",
    "uint32",
    "uint64",
    "uintptr",
    "any",
    "comparable",
    "append",
    "cap",
    "clear",
    "close",
    "complex",
    "copy",
    "delete",
    "imag",
    "len",
    "make",
    "max",
    "min",
    "new",
    "panic",
    "print",
    "println",
    "real",
    "recover",
];
const PYTHON_BUILTINS: &[&str] = &[
    "abs",
    "all",
    "any",
    "bool",
    "bytes",
    "callable",
    "dict",
    "dir",
    "enumerate",
    "filter",
    "float",
    "format",
    "frozenset",
    "getattr",
    "hasattr",
    "hash",
    "id",
    "input",
    "int",
    "isinstance",
    "issubclass",
    "iter",
    "len",
    "list",
    "map",
    "max",
    "min",
    "next",
    "object",
    "open",
    "ord",
    "chr",
    "print",
    "range",
    "repr",
    "reversed",
    "round",
    "set",
    "setattr",
    "sorted",
    "str",
    "sum",
    "super",
    "tuple",
    "type",
    "vars",
    "zip",
    "Exception",
    "ValueError",
    "TypeError",
    "KeyError",
    "RuntimeError",
    "staticmethod",
    "classmethod",
    "property",
    "NotImplementedError",
];

fn is_builtin(language: Language, name: &str) -> bool {
    match language {
        Language::Go => GO_BUILTINS.contains(&name),
        Language::Python => PYTHON_BUILTINS.contains(&name),
        _ => false,
    }
}

/// What a qualifier such as `this.repo` or `util` stands for.
/// A call qualifier's receiver.
pub(crate) enum Container<'a> {
    Types(Vec<Sym<'a>>),
    Module(Vec<&'a str>),
}

/// The Tier B resolver over a snapshot's files (design §5.4); built once per snapshot and shared
/// by `CALLS` resolution and the route packs' handler names (contract M4a S8).
pub(crate) struct Index<'a> {
    files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
    modules: Modules<'a>,
    keys: &'a BTreeMap<&'a str, Vec<String>>,
    /// Per file: definitions by name, directly inside each definition and at top level.
    scopes: BTreeMap<&'a str, Scopes<'a>>,
    /// Per file: the files each import refers to, parallel to `FileFacts::imports`.
    imports: BTreeMap<&'a str, Vec<Vec<&'a str>>>,
    /// Resolved direct supertypes of a type (memoised; resolving them never needs members).
    supers: RefCell<BTreeMap<Sym<'a>, Vec<Sym<'a>>>>,
    /// Type resolutions in progress. Bindings can make them cyclic (`session =
    /// session.Session()`): a resolution already in progress finds nothing.
    resolving: RefCell<BTreeSet<(&'a str, Option<usize>, String)>>,
    /// Per file: non-member bindings by name, ordered by where they come into effect.
    locals: BTreeMap<&'a str, BTreeMap<&'a str, Locals<'a>>>,
    /// Per file: definitions by name, at every nesting level.
    defs_by_name: BTreeMap<&'a str, BTreeMap<&'a str, Vec<usize>>>,
    /// Per file: member bindings by (owning type, name), in source order.
    fields: BTreeMap<&'a str, BTreeMap<(usize, &'a str), Vec<&'a Binding>>>,
    /// Top-level definitions by name per Java package and per Go (directory, package), so the
    /// same-package level costs a lookup rather than a scan of the package.
    packages: BTreeMap<(&'a str, &'a str), BTreeMap<&'a str, Vec<Sym<'a>>>>,
    /// Go methods declared in another file than their receiver type (`parent: None`, `receiver:
    /// Some`), by (the method's package key, receiver type name), then by method name (contract
    /// M9b D5). They are members of that type and never package-level names.
    go_methods: GoMethods<'a>,
    /// Go: call references by the span of their name, per file, so a binding initialised by a
    /// call finds the reference at its chain's last link (contract M9c D4).
    call_refs: BTreeMap<&'a str, BTreeMap<Span, usize>>,
    /// Go: top-level non-member bindings by package key, then by name, with their files
    /// (contract M9c D4, package vars).
    package_vars: PackageVars<'a>,
    /// Go: the embedded member bindings of each struct (contract M9c D4). Never supertypes.
    embeds: BTreeMap<Sym<'a>, Vec<&'a Binding>>,
    /// The result types of a Go callable, with the forms resolving them used (memoised).
    results: RefCell<Memo<'a, Sym<'a>>>,
    /// The result types of a Go call chain's last link, by (file, chain index) (memoised).
    chain_results: RefCell<Memo<'a, (&'a str, usize)>>,
    /// Call chains being resolved, so a chain that needs itself finds nothing.
    calling: RefCell<BTreeSet<(&'a str, usize)>>,
    /// The new Tier B forms (contract M9c D6) the reference being resolved has used.
    forms: RefCell<BTreeSet<&'static str>>,
}

/// [`Index::go_methods`]: (package key, receiver type name) → method name → definitions.
type GoMethods<'a> = BTreeMap<((&'a str, &'a str), &'a str), BTreeMap<&'a str, Vec<Sym<'a>>>>;

/// [`Index::package_vars`]: package key → name → (file, binding).
type PackageVars<'a> = BTreeMap<(&'a str, &'a str), BTreeMap<&'a str, Vec<(&'a str, &'a Binding)>>>;

/// Memoised types, with the [`TIER_B_FORMS`] their resolution used, replayed on a hit.
type Memo<'a, K> = BTreeMap<K, (Vec<Sym<'a>>, BTreeSet<&'static str>)>;

/// The keys of the `tier_b_forms` diagnostic (contract M9c D6).
const TIER_B_FORMS: [&str; 11] = [
    "return_type",
    "call_segment",
    "type_alias",
    "embedded",
    "package_var",
    "default_import",
    "super_call",
    "star_import",
    "reexport",
    "module_variable",
    "inherited_init",
];

/// The declarations of one name in one file, sorted by the start of their extent. Extents are
/// nested or disjoint (scopes are), so `parent` links each declaration to the nearest enclosing
/// one, and a lookup walks that chain instead of scanning every declaration.
#[derive(Default)]
struct Locals<'a> {
    bindings: Vec<&'a Binding>,
    parent: Vec<Option<usize>>,
}

impl<'a> Locals<'a> {
    fn new(mut bindings: Vec<&'a Binding>) -> Self {
        bindings.sort_by_key(|b| (b.visible[0], b.visible[1], b.span[0], b.span[1]));
        let mut parent = Vec::with_capacity(bindings.len());
        let mut open: Vec<usize> = Vec::new();
        for (i, b) in bindings.iter().enumerate() {
            let start = (b.visible[0], b.visible[1]);
            while open
                .last()
                .is_some_and(|&o| (bindings[o].visible[2], bindings[o].visible[3]) < start)
            {
                open.pop();
            }
            parent.push(open.last().copied());
            open.push(i);
        }
        Locals { bindings, parent }
    }

    /// The innermost declaration in effect at `at` that `accept`s; a rejected one is skipped
    /// in favour of the declaration enclosing it.
    fn at_where(
        &self,
        at: rb_model::Span,
        accept: impl Fn(&Binding) -> bool,
    ) -> Option<&'a Binding> {
        let pos = (at[0], at[1]);
        let i = self
            .bindings
            .partition_point(|b| (b.visible[0], b.visible[1]) <= pos);
        let mut cur = i.checked_sub(1);
        while let Some(k) = cur {
            rb_frameworks::work::tick(1);
            let b = self.bindings[k];
            if (b.visible[2], b.visible[3]) >= pos && accept(b) {
                return Some(b);
            }
            cur = self.parent[k];
        }
        None
    }
}

/// The §5.4 level a simple name resolves at: the first level with candidates
/// (contract M6c S5). `Miss` means no level produced any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TypeLevel {
    Local,
    Import,
    SamePackage,
    ImportOnDemand,
    Miss,
}

/// The package a file's same-package level searches: `("", package)` for Java, `(dir, package)`
/// for Go (a directory may also hold an external `_test` package).
pub(crate) fn package_key<'a>(
    path: &'a str,
    language: Language,
    facts: &'a FileFacts,
) -> Option<(&'a str, &'a str)> {
    let package = facts.package.as_deref().unwrap_or_default();
    match language {
        Language::Java => Some(("", package)),
        // Kotlin packages resolve like Java ones (contract M6i S2).
        Language::Kotlin => Some(("", package)),
        Language::Go => Some((dir(path), package)),
        // C# namespaces: the file's top-level module definition, when it has one.
        // Contract M6k S3: files without one share the global namespace key —
        // global-namespace types are visible across files with no import (minimal
        // hosting's Program.cs leg). Namespaced files keep their own key.
        Language::CSharp => Some(
            facts
                .definitions
                .iter()
                .find(|d| d.kind == DefKind::Module && d.parent.is_none())
                .map(|d| ("", d.name.as_str()))
                .unwrap_or(("", "")),
        ),
        _ => None,
    }
}

/// Nested type resolutions (through bindings and dotted names) followed at most.
const MAX_TYPE_DEPTH: usize = 64;

#[derive(Default)]
struct Scopes<'a> {
    top: BTreeMap<&'a str, Vec<usize>>,
    of: BTreeMap<usize, BTreeMap<&'a str, Vec<usize>>>,
}

/// Sorted, without repeats.
fn dedup<T: Ord>(mut v: Vec<T>) -> Vec<T> {
    v.sort_unstable();
    v.dedup();
    v
}

fn is_type(kind: DefKind) -> bool {
    matches!(kind, DefKind::Class | DefKind::Interface | DefKind::Type)
}

fn is_callable(kind: DefKind) -> bool {
    matches!(kind, DefKind::Function | DefKind::Method | DefKind::Class)
}

/// `s` split at each `sep` outside brackets and string or rune literals, each part trimmed
/// (contract M9c D4): `a.Keep(cfg.Size(x.y)).Run` at `.` is `a`, `Keep(cfg.Size(x.y))`, `Run`.
fn split_top(s: &str, sep: u8) -> Vec<&str> {
    let bytes = s.as_bytes();
    let (mut out, mut depth, mut start, mut i) = (Vec::new(), 0usize, 0, 0);
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth = depth.saturating_sub(1),
            b'"' | b'\'' | b'`' => i = literal_end(bytes, i),
            c if c == sep && depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    out.push(s[start.min(s.len())..].trim());
    out
}

/// A Go qualifier's segments (contract M9c D4): split at `.` at bracket depth 0.
fn split_qualifier(q: &str) -> Vec<&str> {
    split_top(q, b'.')
}

/// The index of the quote closing the literal opened at `open` (`"…"` and `'…'` with escapes,
/// a raw `` `…` `` without), or the length when it never closes.
fn literal_end(bytes: &[u8], open: usize) -> usize {
    let quote = bytes[open];
    let mut i = open + 1;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if quote != b'`' => i += 1,
            c if c == quote => return i,
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

/// The callee of a Go call segment `name(…)`: an identifier and one argument list that closes
/// the segment; `None` for any other segment (`m[k]`, `f(a)(b)`, `(T)`, `F[T](x)`).
fn call_segment(seg: &str) -> Option<&str> {
    let open = seg.find('(')?;
    let name = &seg[..open];
    if !is_identifier(name) {
        return None;
    }
    let bytes = seg.as_bytes();
    let (mut depth, mut i) = (0usize, open);
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return (i == bytes.len() - 1).then_some(name);
                }
            }
            b'"' | b'\'' | b'`' => i = literal_end(bytes, i),
            _ => {}
        }
        i += 1;
    }
    None
}

/// An ASCII identifier (Go names outside ASCII are left unresolved).
fn is_identifier(s: &str) -> bool {
    s.bytes().next().is_some_and(|c| !c.is_ascii_digit())
        && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// The type a Go result names when it is usable (contract M9c D4): one named type, or the first
/// of a list of two whose second is `error`, `*` stripped; `None` for anything else (`(T, U)`,
/// three results, named results, `[]T`).
fn go_result_type(result: &str) -> Option<&str> {
    let result = result.trim();
    let one = match result.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        Some(list) => match split_top(list, b',').as_slice() {
            [one] => *one,
            [first, "error"] => *first,
            _ => return None,
        },
        None => result,
    };
    go_named_type(one)
}

/// A named Go type as written, `*` and type arguments stripped (`*store.Store[T]` is
/// `store.Store`); `None` for any other type (`[]T`, `map[K]V`, `func()`, `struct{}`).
fn go_named_type(t: &str) -> Option<&str> {
    let t = t.trim().trim_start_matches('*').trim_start();
    let t = match t.find('[') {
        Some(i) if i > 0 && t.ends_with(']') => &t[..i],
        Some(_) => return None,
        None => t,
    };
    let named = match t.split_once('.') {
        Some((package, name)) => is_identifier(package) && is_identifier(name),
        None => is_identifier(t),
    };
    named.then_some(t)
}

impl<'a> Index<'a> {
    /// Exact manifest resolution for bare specifiers (contract M6c S4).
    /// Default (no manifests) resolves nothing, as before.
    pub(crate) fn with_aliases(mut self, aliases: crate::rta::AliasMaps) -> Self {
        self.modules = self.modules.with_aliases(aliases);
        self.imports = Self::resolve_imports(self.files, &self.modules);
        self
    }

    fn resolve_imports(
        files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
        modules: &Modules<'a>,
    ) -> BTreeMap<&'a str, Vec<Vec<&'a str>>> {
        let mut imports = BTreeMap::new();
        for (&path, (_, facts)) in files {
            imports.insert(
                path,
                facts
                    .imports
                    .iter()
                    .map(|i| modules.resolve(path, i))
                    .collect(),
            );
        }
        imports
    }

    pub(crate) fn new(
        files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
        go_modules: &'a BTreeMap<String, String>,
        keys: &'a BTreeMap<&'a str, Vec<String>>,
    ) -> Self {
        let modules = Modules::new(files, go_modules);
        let mut scopes = BTreeMap::new();
        let mut raw_locals: BTreeMap<&str, BTreeMap<&str, Vec<&Binding>>> = BTreeMap::new();
        let mut defs_by_name: BTreeMap<&str, BTreeMap<&str, Vec<usize>>> = BTreeMap::new();
        let mut fields: BTreeMap<&str, BTreeMap<(usize, &str), Vec<&Binding>>> = BTreeMap::new();
        let mut packages: BTreeMap<(&str, &str), BTreeMap<&str, Vec<Sym>>> = BTreeMap::new();
        let mut go_methods: GoMethods = BTreeMap::new();
        let mut call_refs: BTreeMap<&str, BTreeMap<Span, usize>> = BTreeMap::new();
        let mut package_vars: PackageVars = BTreeMap::new();
        let mut embeds: BTreeMap<Sym, Vec<&Binding>> = BTreeMap::new();
        for (&path, (language, facts)) in files {
            if *language == Language::Go {
                let refs = call_refs.entry(path).or_default();
                for (i, r) in facts.references.iter().enumerate() {
                    if r.kind == RefKind::Call {
                        refs.insert(r.span, i);
                    }
                }
                for b in facts.bindings.iter().filter(|b| b.member && b.embedded) {
                    if let Some(t) = b.scope {
                        embeds.entry((path, t)).or_default().push(b);
                    }
                }
                if let Some(key) = package_key(path, *language, facts) {
                    let vars = package_vars.entry(key).or_default();
                    // A top-level function type's parameters (`var cb func(c *Ctx)`) are no vars.
                    for b in facts
                        .bindings
                        .iter()
                        .filter(|b| !b.member && b.scope.is_none() && b.parameter.is_none())
                    {
                        vars.entry(b.name.as_str()).or_default().push((path, b));
                    }
                }
            }
            let (l, f) = (
                raw_locals.entry(path).or_default(),
                fields.entry(path).or_default(),
            );
            let by_name = defs_by_name.entry(path).or_default();
            for (i, d) in facts.definitions.iter().enumerate() {
                by_name.entry(d.name.as_str()).or_default().push(i);
            }
            for b in &facts.bindings {
                match (b.member, b.scope) {
                    (true, Some(t)) => f.entry((t, b.name.as_str())).or_default().push(b),
                    (true, None) => {}
                    (false, scope) => {
                        l.entry(b.name.as_str()).or_default().push(b);
                        // A Python class-body assignment is a class attribute: `self.x` reads it.
                        if let Some(t) = scope.filter(|&t| {
                            *language == Language::Python && is_type(facts.definitions[t].kind)
                        }) {
                            f.entry((t, b.name.as_str())).or_default().push(b);
                        }
                    }
                }
            }
            if let Some(key) = package_key(path, *language, facts) {
                let table = packages.entry(key).or_default();
                for (i, d) in facts
                    .definitions
                    .iter()
                    .enumerate()
                    .filter(|(_, d)| d.parent.is_none())
                {
                    if let Some(receiver) = d.receiver.as_deref() {
                        go_methods
                            .entry((key, receiver))
                            .or_default()
                            .entry(d.name.as_str())
                            .or_default()
                            .push((path, i));
                    }
                    // A method of a type declared in another file is no package-level name.
                    if d.receiver.is_none() {
                        table.entry(d.name.as_str()).or_default().push((path, i));
                    }
                }
            }
            let mut sc = Scopes::default();
            for (i, d) in facts.definitions.iter().enumerate() {
                let table = match (d.parent, &d.receiver) {
                    (Some(p), _) => sc.of.entry(p).or_default(),
                    // A method of a type declared in another file is no top-level name.
                    (None, Some(_)) => continue,
                    (None, None) => &mut sc.top,
                };
                table.entry(d.name.as_str()).or_default().push(i);
            }
            scopes.insert(path, sc);
        }
        let imports = Self::resolve_imports(files, &modules);
        Index {
            files,
            modules,
            keys,
            scopes,
            imports,
            supers: RefCell::new(BTreeMap::new()),
            resolving: RefCell::new(BTreeSet::new()),
            defs_by_name,
            locals: raw_locals
                .into_iter()
                .map(|(path, by_name)| {
                    let by_name = by_name
                        .into_iter()
                        .map(|(name, bs)| (name, Locals::new(bs)))
                        .collect();
                    (path, by_name)
                })
                .collect(),
            fields,
            packages,
            go_methods,
            call_refs,
            package_vars,
            embeds,
            results: RefCell::new(BTreeMap::new()),
            chain_results: RefCell::new(BTreeMap::new()),
            calling: RefCell::new(BTreeSet::new()),
            forms: RefCell::new(BTreeSet::new()),
        }
    }

    fn language(&self, path: &str) -> Language {
        self.files[path].0
    }

    /// The keys of the functions a handler name (`login`, `ctrl.login`, `h.Pay`) written at `at`
    /// in `path` resolves to, as a call of that name and qualifier at that position (contract
    /// M4a S8): never by bare name across the repository. Sorted, without repeats.
    pub(crate) fn handler_targets(
        &self,
        path: &'a str,
        scope: Option<usize>,
        text: &str,
        at: rb_model::Span,
    ) -> Vec<String> {
        let (qualifier, name) = match text.rsplit_once('.') {
            Some((q, n)) => (Some(q), n),
            None => (None, text),
        };
        dedup(
            self.call(path, scope, name, qualifier, Some(at))
                .into_iter()
                .filter(|(kind, _)| *kind == EdgeKind::Calls)
                .map(|(_, s)| self.key(s).to_owned())
                .collect(),
        )
    }

    /// The files each import of `path` refers to, parallel to its `FileFacts::imports`.
    pub(crate) fn import_targets(&self, path: &str) -> &[Vec<&'a str>] {
        self.imports.get(path).map_or(&[], Vec::as_slice)
    }

    /// Where a constant named in `path` at `at` and not bound in that file is declared
    /// (contract M4a S12), as `(file, owning type, declared name)`: a named import
    /// (`import { BASE as B }` names the target files' top-level `BASE`), a Java static import
    /// (`ROOT` of the imported type), or a qualified name (`Paths.ROOT`, `cfg.Base`, `ns.API`)
    /// through what its qualifier names, as for a call (a variable never names a module or a
    /// type). Sorted, without repeats.
    pub(crate) fn constant_declarations(
        &self,
        path: &'a str,
        name: &str,
        at: rb_model::Span,
    ) -> Vec<(&'a str, Option<usize>, String)> {
        if let Some((qualifier, member)) = name.rsplit_once('.') {
            return match self.container(path, None, qualifier, Some(at)) {
                Some(Container::Types(ts)) => ts
                    .into_iter()
                    .map(|(f, t)| (f, Some(t), member.to_owned()))
                    .collect(),
                Some(Container::Module(files)) => files
                    .into_iter()
                    .map(|f| (f, None, member.to_owned()))
                    .collect(),
                None => Vec::new(),
            };
        }
        let language = self.language(path);
        let mut out = Vec::new();
        for (import, targets) in self.facts(path).imports.iter().zip(&self.imports[path]) {
            if language == Language::Java {
                let named = import.wildcard || import.names.iter().any(|n| n.local == name);
                if import.is_static && named {
                    let module: Vec<&str> = import.module.split('.').collect();
                    for &f in targets {
                        for (tf, t) in self.java_type_path(f, &module) {
                            out.push((tf, Some(t), name.to_owned()));
                        }
                    }
                }
                continue;
            }
            for n in import
                .names
                .iter()
                .filter(|n| n.local == name && n.name != "default")
            {
                out.extend(targets.iter().map(|&f| (f, None, n.name.clone())));
            }
        }
        dedup(out)
    }

    fn facts(&self, path: &str) -> &'a FileFacts {
        self.files[path].1
    }

    /// The types a type name written in `path` at `scope` resolves to (§5.4 levels, dotted
    /// names through their container), as `(file, definition)`; for DI's injected and bean
    /// types (contract M4b S4).
    pub(crate) fn type_candidates(
        &self,
        path: &'a str,
        scope: Option<usize>,
        name: &str,
    ) -> Vec<(&'a str, usize)> {
        self.resolve_type(path, scope, name)
    }

    /// Whether the type name `name` written in `path` may come from outside the repository
    /// (contract M5 S19 (e), orphan rule (e)): an import binding `name` through which no
    /// repository type is found (a library's type of that name, which Tier B's later levels
    /// would miss), or an on-demand import (Java `.*`, Python `*`, Go `.`) of a package or module
    /// with no repository file. An import is outside when it reaches no repository file (a Java
    /// single-type import reaches the file of its class, or of the outer class of a nested one).
    pub(crate) fn imported_from_outside(&self, path: &'a str, name: &str) -> bool {
        let language = self.language(path);
        self.facts(path)
            .imports
            .iter()
            .zip(&self.imports[path])
            .any(|(import, targets)| {
                let on_demand = import.wildcard
                    || (language == Language::Go && import.alias.as_deref() == Some("."));
                let binds = on_demand
                    || import.alias.as_deref() == Some(name)
                    || import.names.iter().any(|n| n.local == name);
                !import.is_static && binds && targets.is_empty()
            })
    }

    /// Whether `path` has an on-demand import reaching no repository file
    /// (contract M6c S5): a wildcard import (Java `.*`, Python `*`) or a Go dot-import
    /// of a package or module with no repository file. Static imports never count, as in
    /// [`Index::imported_from_outside`]. A wildcard binds every head, so the check needs
    /// no name.
    pub(crate) fn on_demand_from_outside(&self, path: &'a str) -> bool {
        let language = self.language(path);
        self.facts(path)
            .imports
            .iter()
            .zip(&self.imports[path])
            .any(|(import, targets)| {
                let on_demand = import.wildcard
                    || (language == Language::Go && import.alias.as_deref() == Some("."));
                !import.is_static && on_demand && targets.is_empty()
            })
    }

    /// The level a simple type name resolves at (contract M6c S5): the first §5.4 level
    /// with candidates. Dotted names take the container path instead, which has no level.
    pub(crate) fn type_level(
        &self,
        path: &'a str,
        scope: Option<usize>,
        name: &str,
    ) -> Option<TypeLevel> {
        if name.contains('.') {
            return None;
        }
        Some(self.levels(path, scope, name, is_type, false).1)
    }

    /// The resolved direct supertypes of a type (all candidates of each declared name).
    pub(crate) fn direct_supertypes(&self, t: (&'a str, usize)) -> Vec<(&'a str, usize)> {
        self.supertypes(t)
    }

    fn def(&self, s: Sym<'a>) -> &'a Definition {
        &self.facts(s.0).definitions[s.1]
    }

    fn key(&self, s: Sym<'a>) -> &str {
        &self.keys[s.0][s.1]
    }

    /// Definitions named `name` directly inside `parent` (`None`: top level) of `path`.
    fn named(
        &self,
        path: &'a str,
        parent: Option<usize>,
        name: &str,
        want: fn(DefKind) -> bool,
    ) -> Vec<Sym<'a>> {
        let sc = &self.scopes[path];
        let table = match parent {
            Some(p) => sc.of.get(&p),
            None => Some(&sc.top),
        };
        table
            .and_then(|t| t.get(name))
            .into_iter()
            .flatten()
            .filter(|&&i| want(self.facts(path).definitions[i].kind))
            .map(|&i| (path, i))
            .collect()
    }

    /// `scope` and its enclosing definitions, innermost first.
    fn ancestors(&self, path: &str, scope: Option<usize>) -> Vec<usize> {
        let defs = &self.facts(path).definitions;
        let mut out = Vec::new();
        let mut cur = scope;
        while let Some(i) = cur {
            out.push(i);
            cur = defs[i].parent;
        }
        out
    }

    fn enclosing_type(&self, path: &'a str, scope: Option<usize>) -> Option<Sym<'a>> {
        let defs = &self.facts(path).definitions;
        self.ancestors(path, scope)
            .into_iter()
            .find(|&i| is_type(defs[i].kind))
            .map(|i| (path, i))
    }

    /// Candidates for a type name used in `path` at `scope` (§5.4 levels; dotted names through
    /// their container).
    fn resolve_type(&self, path: &'a str, scope: Option<usize>, name: &str) -> Vec<Sym<'a>> {
        let key = (path, scope, name.to_owned());
        {
            let mut resolving = self.resolving.borrow_mut();
            if resolving.len() >= MAX_TYPE_DEPTH || !resolving.insert(key.clone()) {
                return Vec::new();
            }
        }
        let found = self.resolve_type_unguarded(path, scope, name);
        // Go `type A = B` stands for what `B` names in the alias's file (contract M9c D4),
        // resolved while this name is still in progress, so `A = B; B = A` finds nothing.
        let found = dedup(
            found
                .into_iter()
                .flat_map(|t| {
                    let def = self.def(t);
                    let Some(target) = def.alias_of.as_deref() else {
                        return vec![t];
                    };
                    let types = go_named_type(target)
                        .map(|n| self.resolve_type(t.0, def.parent, n))
                        .unwrap_or_default();
                    if !types.is_empty() {
                        self.note("type_alias");
                    }
                    types
                })
                .collect(),
        );
        self.resolving.borrow_mut().remove(&key);
        found
    }

    fn resolve_type_unguarded(
        &self,
        path: &'a str,
        scope: Option<usize>,
        name: &str,
    ) -> Vec<Sym<'a>> {
        if let Some((head, last)) = name.rsplit_once('.') {
            let found = match self.container(path, scope, head, None) {
                Some(Container::Types(ts)) => ts
                    .into_iter()
                    .flat_map(|t| self.named(t.0, Some(t.1), last, is_type))
                    .collect(),
                Some(Container::Module(files)) => files
                    .into_iter()
                    .flat_map(|f| self.named(f, None, last, is_type))
                    .collect(),
                None => Vec::new(),
            };
            return if found.is_empty() {
                self.java_qualified(path, name)
            } else {
                found
            };
        }
        self.levels(path, scope, name, is_type, false).0
    }

    /// A fully qualified Java class name (`com.acme.util.Strings`).
    fn java_qualified(&self, path: &str, name: &str) -> Vec<Sym<'a>> {
        if self.language(path) != Language::Java {
            return Vec::new();
        }
        let last = name.rsplit('.').next().unwrap_or(name);
        self.modules
            .java_class(name)
            .into_iter()
            .flat_map(|f| self.named(f, None, last, is_type))
            .collect()
    }

    /// The §5.4 levels for a simple name: lexical scopes, the same file, imports, the same
    /// package. The first level with candidates wins. `callable` selects call semantics: class
    /// bodies are scopes only in Java, and there through inheritance. Returns the winners and
    /// the level that produced them (contract M6c S5).
    fn levels(
        &self,
        path: &'a str,
        scope: Option<usize>,
        name: &str,
        want: fn(DefKind) -> bool,
        callable: bool,
    ) -> (Vec<Sym<'a>>, TypeLevel) {
        let language = self.language(path);
        let defs = &self.facts(path).definitions;
        for a in self.ancestors(path, scope) {
            let found = if is_type(defs[a].kind) && callable {
                if language == Language::Java {
                    self.members((path, a), name, want)
                } else {
                    Vec::new()
                }
            } else {
                self.named(path, Some(a), name, want)
            };
            if !found.is_empty() {
                return (found, TypeLevel::Local);
            }
        }
        let found = self.named(path, None, name, want);
        if !found.is_empty() {
            return (found, TypeLevel::Local);
        }
        // Java (JLS §6.4.1): single-type imports shadow the package, which shadows on-demand
        // (wildcard) imports. Elsewhere imports come before the package.
        let found = self.imported(path, name, want, false);
        if !found.is_empty() {
            return (found, TypeLevel::Import);
        }
        if self.language(path) == Language::Java {
            let found = self.same_package(path, name, want);
            if !found.is_empty() {
                return (found, TypeLevel::SamePackage);
            }
            let found = self.imported(path, name, want, true);
            if !found.is_empty() {
                return (found, TypeLevel::ImportOnDemand);
            }
            return (Vec::new(), TypeLevel::Miss);
        }
        let found = self.same_package(path, name, want);
        if !found.is_empty() {
            return (found, TypeLevel::SamePackage);
        }
        (Vec::new(), TypeLevel::Miss)
    }

    /// The definition that binding `b` of `name` also is: the kept symbol declared by the same
    /// name token (`const f = () => …` starts with `f`). Definitions inside callbacks are not
    /// kept, so a function held in such a variable stays a plain variable; a same-named property
    /// inside the value (`const run = { run: () => … }`) is not the variable.
    fn defines(&self, path: &'a str, name: &str, b: &Binding) -> Option<Sym<'a>> {
        let defs = &self.facts(path).definitions;
        self.defs_by_name[path]
            .get(name)
            .into_iter()
            .flatten()
            .find(|&&i| {
                let s = defs[i].name_span;
                (s[0], s[1]) == (b.span[0], b.span[1]) && is_callable(defs[i].kind)
            })
            .map(|&i| (path, i))
    }

    /// Types along a dotted path inside file `f` (`Util.Nested`): a top-level type followed by
    /// types nested in it, trying each suffix of the path.
    fn java_type_path(&self, f: &'a str, segments: &[&str]) -> Vec<Sym<'a>> {
        for start in 0..segments.len() {
            let mut types = self.named(f, None, segments[start], is_type);
            for seg in &segments[start + 1..] {
                if types.is_empty() {
                    break;
                }
                types = types
                    .into_iter()
                    .flat_map(|t| self.named(t.0, Some(t.1), seg, is_type))
                    .collect();
            }
            if !types.is_empty() {
                return types;
            }
        }
        Vec::new()
    }

    /// Definitions `name` refers to through the file's imports: single-name imports, or with
    /// `on_demand` only Java wildcard (package or static) imports. Kotlin star imports ride the
    /// wildcard arm at the import level, so for Kotlin imports (named and wildcard alike)
    /// precede the package, the non-Java order.
    fn imported(
        &self,
        path: &'a str,
        name: &str,
        want: fn(DefKind) -> bool,
        on_demand: bool,
    ) -> Vec<Sym<'a>> {
        let language = self.language(path);
        let mut out = Vec::new();
        for (import, targets) in self.facts(path).imports.iter().zip(&self.imports[path]) {
            // A top-level definition of the target files; for Java also a member of the class the
            // import names (`import a.Util.Nested`, `import a.Util.*`).
            let module: Vec<&str> = import.module.split('.').collect();
            let top = |n: &str| -> Vec<Sym<'a>> {
                let found: Vec<Sym<'a>> = targets
                    .iter()
                    .flat_map(|&f| self.named(f, None, n, want))
                    .collect();
                if !found.is_empty() || language != Language::Java {
                    return found;
                }
                targets
                    .iter()
                    .flat_map(|&f| self.java_type_path(f, &module))
                    .flat_map(|c| self.named(c.0, Some(c.1), n, want))
                    .collect()
            };
            if language == Language::Java && import.wildcard != on_demand {
                continue;
            }
            if language != Language::Java && on_demand {
                continue;
            }
            match language {
                Language::Java if import.is_static => {
                    let named = import.wildcard || import.names.iter().any(|n| n.local == name);
                    if named {
                        for &f in targets {
                            for c in self.java_type_path(f, &module) {
                                out.extend(self.named(c.0, Some(c.1), name, want));
                            }
                        }
                    }
                }
                Language::Java | Language::Kotlin if import.wildcard => out.extend(top(name)),
                Language::Go if import.alias.as_deref() == Some(".") => out.extend(top(name)),
                // C# plain usings (`using A;`) import every name of the namespace, like Java
                // on-demand imports — including namespace members, which are file-visible.
                Language::CSharp if !import.is_static && import.alias.is_none() => {
                    out.extend(top(name));
                    out.extend(targets.iter().flat_map(|&f| {
                        self.facts(f)
                            .definitions
                            .iter()
                            .enumerate()
                            .filter(|(_, d)| d.parent.is_none() && d.kind == DefKind::Module)
                            .flat_map(|(m, _)| self.named(f, Some(m), name, want))
                    }));
                }
                _ => {
                    for n in import.names.iter().filter(|n| n.local == name) {
                        if n.name != "default" {
                            out.extend(top(&n.name));
                        } else if matches!(
                            language,
                            Language::TypeScript | Language::Tsx | Language::JavaScript
                        ) {
                            // `import x from './m'` names what `m`'s `export default` names
                            // (contract M9c D2); a target without one gives nothing.
                            for &f in targets {
                                if let Some(exported) = self.facts(f).default_export.as_deref() {
                                    let found = self.named(f, None, exported, want);
                                    if !found.is_empty() {
                                        self.note("default_import");
                                    }
                                    out.extend(found);
                                }
                            }
                        }
                    }
                }
            }
        }
        out
    }

    fn same_package(&self, path: &'a str, name: &str, want: fn(DefKind) -> bool) -> Vec<Sym<'a>> {
        let (language, facts) = self.files[path];
        let found = |key: &(&str, &str)| {
            self.packages
                .get(key)
                .and_then(|table| table.get(name))
                .into_iter()
                .flatten()
                .copied()
                .filter(|&(f, i)| f != path && want(self.def((f, i)).kind))
                .collect::<Vec<_>>()
        };
        let Some(key) = package_key(path, language, facts) else {
            return Vec::new();
        };
        let hit = found(&key);
        if !hit.is_empty() {
            return hit;
        }
        // Contract M6k pass 1: C# enclosing namespaces are in scope without any
        // `using` (nearest first, hiding like the language). Without this, names
        // a parent namespace declares silently miss once usings stop accidentally
        // covering them through the outer-namespace fallback.
        if language == Language::CSharp {
            let mut ns = key.1;
            while let Some((outer, _)) = ns.rsplit_once('.') {
                ns = outer;
                let hit = found(&("", ns));
                if !hit.is_empty() {
                    return hit;
                }
            }
            // The global namespace is always in scope too.
            if !key.1.is_empty() {
                return found(&("", ""));
            }
        }
        Vec::new()
    }

    /// Members named `name` of `t`, else of its nearest supertypes that have one (breadth-first,
    /// each type once, at most [`MAX_INHERITANCE_DEPTH`] levels).
    /// Go has no supertypes: the next levels are the embedded types (contract M9c D4), and a
    /// tie at one depth gives every candidate.
    fn members(&self, t: Sym<'a>, name: &str, want: fn(DefKind) -> bool) -> Vec<Sym<'a>> {
        let mut seen = BTreeSet::from([t]);
        let mut level = vec![t];
        for depth in 0..MAX_INHERITANCE_DEPTH {
            let found: Vec<Sym<'a>> = level
                .iter()
                .flat_map(|&c| {
                    let mut own = self.named(c.0, Some(c.1), name, want);
                    own.extend(self.go_methods_of(c, name, want));
                    own
                })
                .collect();
            if !found.is_empty() {
                if depth > 0 && self.language(t.0) == Language::Go {
                    self.note("embedded");
                }
                return found;
            }
            level = level
                .iter()
                .flat_map(|&c| {
                    let mut next = self.supertypes(c);
                    next.extend(self.embedded_types(c));
                    next
                })
                .filter(|s| seen.insert(*s))
                .collect();
            if level.is_empty() {
                break;
            }
        }
        Vec::new()
    }

    /// The methods named `name` that Go files declare for the top-level type `t` outside its own
    /// file (contract M9b D5): looked up by the package of `t`'s file, so a same-named type of
    /// another directory, or of the external `_test` package beside it, gets none of them.
    fn go_methods_of(&self, t: Sym<'a>, name: &str, want: fn(DefKind) -> bool) -> Vec<Sym<'a>> {
        let (language, facts) = self.files[t.0];
        let def = self.def(t);
        if language != Language::Go || def.parent.is_some() {
            return Vec::new();
        }
        let Some(key) = package_key(t.0, language, facts) else {
            return Vec::new();
        };
        self.go_methods
            .get(&(key, def.name.as_str()))
            .and_then(|methods| methods.get(name))
            .into_iter()
            .flatten()
            .copied()
            .filter(|&s| want(self.def(s).kind))
            .collect()
    }

    /// The resolved direct supertypes of `t` (all candidates of each name).
    fn supertypes(&self, t: Sym<'a>) -> Vec<Sym<'a>> {
        if let Some(known) = self.supers.borrow().get(&t) {
            return known.clone();
        }
        let def = self.def(t);
        let found: Vec<Sym<'a>> = def
            .supertypes
            .iter()
            .flat_map(|s| self.resolve_type(t.0, def.parent, &s.name))
            .filter(|&s| s != t)
            .collect();
        self.supers.borrow_mut().insert(t, found.clone());
        found
    }

    /// Records that the reference being resolved used `form` (contract M9c D6).
    fn note(&self, form: &'static str) {
        debug_assert!(TIER_B_FORMS.contains(&form), "{form}");
        self.forms.borrow_mut().insert(form);
    }

    /// `compute`'s types, memoised in `memo` under `key` with the forms it used, which a hit
    /// records again, so a memoised resolution counts its forms like a fresh one.
    fn memoised<K: Ord + Copy>(
        &self,
        memo: &RefCell<Memo<'a, K>>,
        key: K,
        compute: impl FnOnce() -> Vec<Sym<'a>>,
    ) -> Vec<Sym<'a>> {
        let hit = memo.borrow().get(&key).cloned();
        if let Some((types, forms)) = hit {
            self.forms.borrow_mut().extend(forms);
            return types;
        }
        let outer = self.forms.take();
        let types = compute();
        let used = self.forms.replace(outer);
        self.forms.borrow_mut().extend(used.iter().copied());
        memo.borrow_mut().insert(key, (types.clone(), used));
        types
    }

    /// The types Go struct `t` embeds (contract M9c D4): never supertypes, never `IMPLEMENTS`.
    fn embedded_types(&self, t: Sym<'a>) -> Vec<Sym<'a>> {
        self.embeds
            .get(&t)
            .into_iter()
            .flatten()
            .filter(|b| !b.type_name.is_empty())
            .flat_map(|b| self.resolve_type(t.0, b.scope, &b.type_name))
            .collect()
    }

    /// The types a binding of `path` stands for: its declared type, else (Go) the result type of
    /// the call that initialises it (contract M9c D4).
    fn binding_types(&self, path: &'a str, b: &'a Binding) -> Vec<Sym<'a>> {
        if !b.type_name.is_empty() {
            return self.resolve_type(path, b.scope, &b.type_name);
        }
        let Some(Init::Call { chain }) = b.init else {
            return Vec::new();
        };
        if self.language(path) != Language::Go {
            return Vec::new();
        }
        let types = self.memoised(&self.chain_results, (path, chain), || {
            self.chain_result(path, chain)
        });
        if !types.is_empty() {
            self.note("return_type");
        }
        types
    }

    /// The result types of the callables the reference at chain `chain`'s last link resolves to.
    fn chain_result(&self, path: &'a str, chain: usize) -> Vec<Sym<'a>> {
        let facts = self.facts(path);
        let Some(link) = facts.chains.get(chain).and_then(|c| c.links.last()) else {
            return Vec::new();
        };
        let Some(&i) = self
            .call_refs
            .get(path)
            .and_then(|refs| refs.get(&link.span))
        else {
            return Vec::new();
        };
        {
            let mut calling = self.calling.borrow_mut();
            if calling.len() >= MAX_TYPE_DEPTH || !calling.insert((path, chain)) {
                return Vec::new();
            }
        }
        let r = &facts.references[i];
        let callees = self.call(
            path,
            r.enclosing,
            &r.name,
            r.qualifier.as_deref(),
            Some(r.span),
        );
        self.calling.borrow_mut().remove(&(path, chain));
        self.results_of(callees)
    }

    /// The result types of the functions and methods among `callees`.
    fn results_of(&self, callees: impl IntoIterator<Item = (EdgeKind, Sym<'a>)>) -> Vec<Sym<'a>> {
        dedup(
            callees
                .into_iter()
                .filter(|&(kind, s)| {
                    kind == EdgeKind::Calls
                        && matches!(self.def(s).kind, DefKind::Function | DefKind::Method)
                })
                .flat_map(|(_, s)| self.result_types(s))
                .collect(),
        )
    }

    /// The type a Go callable returns (contract M9c D4): one named type, or the first of two
    /// results whose second is `error`, resolved in the callable's file. Memoised.
    fn result_types(&self, s: Sym<'a>) -> Vec<Sym<'a>> {
        self.memoised(&self.results, s, || {
            let def = self.def(s);
            def.return_type
                .as_deref()
                .and_then(go_result_type)
                .map(|t| self.resolve_type(s.0, def.parent, t))
                .unwrap_or_default()
        })
    }

    /// The package-level var `name` of `path`'s Go package (contract M9c D4): the package must
    /// declare the name exactly once, of any kind, as a var with a type, and a non-test file
    /// never sees a test file's var.
    fn package_var(&self, path: &'a str, name: &str) -> Option<(&'a str, &'a Binding)> {
        let (language, facts) = self.files[path];
        if language != Language::Go {
            return None;
        }
        let key = package_key(path, language, facts)?;
        let vars = self.package_vars.get(&key)?.get(name)?;
        let defs = self
            .packages
            .get(&key)
            .and_then(|table| table.get(name))
            .map_or(0, Vec::len);
        let [(file, b)] = vars.as_slice() else {
            return None;
        };
        let hidden = file.ends_with("_test.go") && !path.ends_with("_test.go");
        (defs == 0 && !b.type_name.is_empty() && !hidden).then_some((*file, *b))
    }

    /// The declaration `name` refers to at position `at` in `path`: the innermost local whose
    /// extent contains `at` (a class body's own names are not locals of its methods); else (Java)
    /// a field of an enclosing class. An untyped declaration still counts: it hides outer ones and
    /// marks the name as a variable. Without a position, only fields are considered.
    fn binding(
        &self,
        path: &'a str,
        scope: Option<usize>,
        name: &str,
        at: Option<rb_model::Span>,
    ) -> Option<&'a Binding> {
        let defs = &self.facts(path).definitions;
        if let Some(at) = at {
            // A class body's own names are not locals of its methods; look past them.
            let accept = |b: &Binding| {
                !b.scope
                    .is_some_and(|t| is_type(defs[t].kind) && scope != Some(t))
            };
            if let Some(b) = self.locals[path]
                .get(name)
                .and_then(|l| l.at_where(at, accept))
            {
                return Some(b);
            }
        }
        if self.language(path) != Language::Java {
            return None;
        }
        self.ancestors(path, scope).into_iter().find_map(|a| {
            self.fields[path]
                .get(&(a, name))
                .and_then(|bs| bs.first().copied())
        })
    }

    /// Whether the unresolved name `head` (a bare name, or the first segment of a qualifier)
    /// comes from an external package: an import that resolves to no file of the snapshot, directly
    /// or as the declared type of a binding.
    fn is_external(
        &self,
        path: &'a str,
        scope: Option<usize>,
        head: &str,
        at: Option<rb_model::Span>,
    ) -> bool {
        let head = match self.binding(path, scope, head, at) {
            Some(b) if b.type_name.is_empty() => return false,
            Some(b) => b.type_name.split('.').next().unwrap_or(&b.type_name),
            None => head,
        };
        let language = self.language(path);
        let facts = self.facts(path);
        facts
            .imports
            .iter()
            .zip(&self.imports[path])
            .filter(|(_, targets)| targets.is_empty())
            .any(|(import, _)| {
                import.alias.as_deref() == Some(head)
                    || import.names.iter().any(|n| n.local == head)
                    || (import.alias.is_none()
                        && import.names.is_empty()
                        && match language {
                            Language::Go => import.module.rsplit('/').next() == Some(head),
                            Language::Python => import.module.split('.').next() == Some(head),
                            _ => false,
                        })
            })
    }

    /// What the dotted qualifier `q` of a member call stands for, or `None` when unknown.
    pub(crate) fn container(
        &self,
        path: &'a str,
        scope: Option<usize>,
        q: &str,
        at: Option<rb_model::Span>,
    ) -> Option<Container<'a>> {
        // Go splits at bracket depth 0, so `a.B(x.y).C` is three segments (contract M9c D4).
        let go = self.language(path) == Language::Go;
        let segments: Vec<&str> = if go {
            split_qualifier(q)
        } else {
            q.split('.').collect()
        };
        let (mut target, consumed) = self.first_segment(path, scope, &segments, at)?;
        for seg in &segments[consumed..] {
            target = if go && seg.contains(['(', '[', '{']) {
                // A call steps to its result; any other bracketed segment stops the walk.
                self.call_step(target, call_segment(seg)?)?
            } else {
                self.step(target, seg)?
            };
        }
        Some(target)
    }

    /// The container a Go call segment `name(…)` stands for: the result types of the members
    /// named `name` of the current types, or of a module's top-level functions (contract M9c
    /// D4).
    fn call_step(&self, target: Container<'a>, name: &str) -> Option<Container<'a>> {
        let callees: Vec<Sym<'a>> = match target {
            Container::Types(ts) => ts
                .into_iter()
                .flat_map(|t| self.members(t, name, is_callable))
                .collect(),
            Container::Module(files) => files
                .into_iter()
                .flat_map(|f| self.named(f, None, name, is_callable))
                .collect(),
        };
        let types = self.results_of(callees.into_iter().map(|s| (EdgeKind::Calls, s)));
        if types.is_empty() {
            return None;
        }
        self.note("call_segment");
        Some(Container::Types(types))
    }

    fn first_segment(
        &self,
        path: &'a str,
        scope: Option<usize>,
        segments: &[&str],
        at: Option<rb_model::Span>,
    ) -> Option<(Container<'a>, usize)> {
        let language = self.language(path);
        let s0 = segments[0];
        match s0 {
            "this" | "self" => {
                return Some((Container::Types(vec![self.enclosing_type(path, scope)?]), 1));
            }
            "super" => {
                let class = self.enclosing_type(path, scope)?;
                return Some((Container::Types(self.supertypes(class)), 1));
            }
            _ => {}
        }
        // Go: a call at the head (`Setup(t).App`) stands for its callees' result types (contract
        // M9c D4); any other bracketed head stops the walk.
        if language == Language::Go && s0.contains(['(', '[', '{']) {
            let types = self.results_of(self.call(path, scope, call_segment(s0)?, None, at));
            if types.is_empty() {
                return None;
            }
            self.note("call_segment");
            return Some((Container::Types(types), 1));
        }
        // A variable: its declared type (Go: else its initialising call's result), or nothing.
        // Never a same-named type or module.
        if let Some(b) = self.binding(path, scope, s0, at) {
            let types = self.binding_types(path, b);
            return (!types.is_empty()).then_some((Container::Types(types), 1));
        }
        let imports = self.facts(path).imports.iter().zip(&self.imports[path]);
        for (import, targets) in imports {
            if targets.is_empty() {
                continue;
            }
            match language {
                // `import a.b` binds `a`; a call on `a.b` names the module.
                Language::Python if import.alias.is_none() && import.names.is_empty() => {
                    let module: Vec<&str> = import.module.split('.').collect();
                    if segments.len() >= module.len() && segments[..module.len()] == module[..] {
                        return Some((Container::Module(targets.clone()), module.len()));
                    }
                }
                Language::Go if import.alias.is_none() => {
                    let package = targets
                        .first()
                        .and_then(|f| self.facts(f).package.as_deref());
                    if package == Some(s0) {
                        return Some((Container::Module(targets.clone()), 1));
                    }
                }
                _ => {}
            }
            if import.alias.as_deref() == Some(s0) {
                return Some((Container::Module(targets.clone()), 1));
            }
            for n in import.names.iter().filter(|n| n.local == s0) {
                let mut types: Vec<Sym<'a>> = targets
                    .iter()
                    .flat_map(|&f| self.named(f, None, &n.name, is_type))
                    .collect();
                if types.is_empty() && language == Language::Java {
                    let module: Vec<&str> = import.module.split('.').collect();
                    types = targets
                        .iter()
                        .flat_map(|&f| self.java_type_path(f, &module))
                        .flat_map(|c| self.named(c.0, Some(c.1), &n.name, is_type))
                        .collect();
                }
                if !types.is_empty() {
                    return Some((Container::Types(types), 1));
                }
                // `from pkg import mod`: a submodule.
                let submodule: Vec<&'a str> = targets
                    .iter()
                    .copied()
                    .filter(|f| {
                        f.ends_with(&format!("/{}.py", n.name))
                            || f.ends_with(&format!("/{}/__init__.py", n.name))
                    })
                    .collect();
                if !submodule.is_empty() {
                    return Some((Container::Module(submodule), 1));
                }
            }
        }
        // Go: a var any file of the package declares (contract M9c D4), after the file's
        // imports; a variable too, so its type or nothing.
        if at.is_some()
            && let Some((file, b)) = self.package_var(path, s0)
        {
            let types = self.resolve_type(file, b.scope, &b.type_name);
            if types.is_empty() {
                return None;
            }
            self.note("package_var");
            return Some((Container::Types(types), 1));
        }
        let types = self.resolve_type(path, scope, s0);
        if !types.is_empty() {
            return Some((Container::Types(types), 1));
        }
        // A fully qualified Java class: `com.acme.util.Strings`.
        (1..=segments.len()).rev().find_map(|k| {
            let types = self.java_qualified(path, &segments[..k].join("."));
            (!types.is_empty()).then_some((Container::Types(types), k))
        })
    }

    /// The container one member access further on.
    fn step(&self, target: Container<'a>, seg: &str) -> Option<Container<'a>> {
        let found: Vec<Sym<'a>> = match target {
            Container::Types(ts) => ts
                .into_iter()
                .flat_map(|t| {
                    let declared = self.fields[t.0].get(&(t.1, seg));
                    let mut own: Vec<Sym<'a>> = declared
                        .into_iter()
                        .flatten()
                        .filter(|b| !b.type_name.is_empty())
                        .flat_map(|b| self.resolve_type(t.0, Some(t.1), &b.type_name))
                        .collect();
                    own.extend(self.named(t.0, Some(t.1), seg, is_type));
                    if declared.is_none() && own.is_empty() {
                        own = self.promoted_field(t, seg);
                    }
                    own
                })
                .collect(),
            Container::Module(files) => files
                .into_iter()
                .flat_map(|f| self.named(f, None, seg, is_type))
                .collect(),
        };
        // Deduplicated, so repeated bindings or declarations never multiply along a chain.
        let found = dedup(found);
        (!found.is_empty()).then_some(Container::Types(found))
    }

    /// The types of the field `seg` that Go struct `t` promotes from the structs it embeds
    /// (contract M9c D4): breadth-first, the shallowest depth declaring the field wins, and a
    /// tie at that depth gives every candidate. Empty for any other language.
    fn promoted_field(&self, t: Sym<'a>, seg: &str) -> Vec<Sym<'a>> {
        let mut seen = BTreeSet::from([t]);
        let mut level: Vec<Sym<'a>> = self
            .embedded_types(t)
            .into_iter()
            .filter(|s| seen.insert(*s))
            .collect();
        for _ in 0..MAX_INHERITANCE_DEPTH {
            if level.is_empty() {
                break;
            }
            let declared: Vec<(Sym<'a>, &'a Binding)> = level
                .iter()
                .flat_map(|&c| {
                    self.fields[c.0]
                        .get(&(c.1, seg))
                        .into_iter()
                        .flatten()
                        .map(move |b| (c, *b))
                })
                .collect();
            if !declared.is_empty() {
                let types = dedup(
                    declared
                        .into_iter()
                        .filter(|(_, b)| !b.type_name.is_empty())
                        .flat_map(|(c, b)| self.resolve_type(c.0, Some(c.1), &b.type_name))
                        .collect(),
                );
                if !types.is_empty() {
                    self.note("embedded");
                }
                return types;
            }
            level = level
                .iter()
                .flat_map(|&c| self.embedded_types(c))
                .filter(|s| seen.insert(*s))
                .collect();
        }
        Vec::new()
    }

    /// Candidates of a call reference, each with the edge it gives. Shared
    /// with the inherited-call producer (contract M6c S7), which must not
    /// double an edge Tier B links itself.
    pub(crate) fn call(
        &self,
        path: &'a str,
        scope: Option<usize>,
        name: &str,
        qualifier: Option<&str>,
        at: Option<rb_model::Span>,
    ) -> Vec<(EdgeKind, Sym<'a>)> {
        // A bare call through a variable (`cb()`) is never linked by name to a function: it
        // resolves to the definition the variable itself is (`const f = () => …`), or to nothing.
        // Java keeps methods and variables in separate namespaces (JLS §6.4.2).
        let variable = (self.language(path) != Language::Java)
            .then(|| at.and_then(|at| self.binding(path, scope, name, Some(at))))
            .flatten()
            .filter(|b| !b.member);
        let found = match (qualifier, variable) {
            (None, Some(b)) => self.defines(path, name, b).into_iter().collect(),
            (None, None) => self.levels(path, scope, name, is_callable, true).0,
            (Some(q), _) => match self.container(path, scope, q, at) {
                Some(Container::Types(ts)) => ts
                    .into_iter()
                    .flat_map(|t| self.members(t, name, is_callable))
                    .collect(),
                Some(Container::Module(files)) => files
                    .into_iter()
                    .flat_map(|f| self.named(f, None, name, is_callable))
                    .collect(),
                None => Vec::new(),
            },
        };
        found
            .into_iter()
            .map(|s| {
                if !is_type(self.def(s).kind) {
                    return (EdgeKind::Calls, s);
                }
                // Calling a class constructs it: its declared constructor, else the class.
                match self.constructor(s) {
                    Some(c) => (EdgeKind::Calls, c),
                    None => (EdgeKind::References, s),
                }
            })
            .collect()
    }

    fn constructor(&self, class: Sym<'a>) -> Option<Sym<'a>> {
        let name = match self.language(class.0) {
            Language::Java => self.def(class).name.as_str(),
            Language::Python => "__init__",
            Language::TypeScript | Language::Tsx | Language::JavaScript => "constructor",
            // Contract M6i S2: constructor dispatch lands with Tier B (Go precedent: none).
            Language::Go | Language::Kotlin | Language::CSharp => return None,
        };
        self.named(class.0, Some(class.1), name, |k| {
            matches!(k, DefKind::Method | DefKind::Function)
        })
        .into_iter()
        .next()
    }

    /// `IMPLEMENTS` for the types of `path`: type → resolved supertype, and method → the method
    /// of the same name it overrides in the nearest supertype that has one.
    fn implements(&self, path: &'a str, facts: &FileFacts, edges: &mut Edges) {
        for (i, def) in facts.definitions.iter().enumerate() {
            if !is_type(def.kind) || def.supertypes.is_empty() {
                continue;
            }
            let t = (path, i);
            let supers = self.supertypes(t);
            let at = Evidence {
                path: path.to_owned(),
                span: def.name_span,
            };
            let found: Vec<(EdgeKind, Sym<'a>)> =
                supers.iter().map(|&s| (EdgeKind::Implements, s)).collect();
            let src = NodeKey::new(NodeKind::Symbol, self.key(t).to_owned());
            edges.record_all(src, &found, self, &at);
            let methods = self.scopes[path].of.get(&i).into_iter().flatten();
            for &m in methods.flat_map(|(_, ms)| ms) {
                let method = &facts.definitions[m];
                if !matches!(method.kind, DefKind::Method | DefKind::Function) {
                    continue;
                }
                let overridden: Vec<(EdgeKind, Sym<'a>)> = supers
                    .iter()
                    .flat_map(|&s| {
                        self.members(s, &method.name, |k| {
                            matches!(k, DefKind::Method | DefKind::Function)
                        })
                    })
                    .map(|s| (EdgeKind::Implements, s))
                    .collect();
                let at = Evidence {
                    path: path.to_owned(),
                    span: method.name_span,
                };
                let src = NodeKey::new(NodeKind::Symbol, self.key((path, m)).to_owned());
                edges.record_all(src, &overridden, self, &at);
            }
        }
    }
}

/// Resolved edges, aggregated per `(source, kind, target)`.
#[derive(Default)]
struct Edges(BTreeMap<(NodeKey, EdgeKind, NodeKey), Resolved>);

struct Resolved {
    certainty: Certainty,
    /// Largest candidate count among the ambiguous sites.
    candidates: usize,
    evidence: BTreeSet<Evidence>,
}

impl Edges {
    /// One site's candidates: `heuristic` when there is one target, `ambiguous` otherwise.
    fn record_all<'a>(
        &mut self,
        src: NodeKey,
        found: &[(EdgeKind, Sym<'a>)],
        index: &Index<'a>,
        at: &Evidence,
    ) {
        let targets: BTreeSet<(EdgeKind, &str)> =
            found.iter().map(|&(k, s)| (k, index.key(s))).collect();
        let n = targets.len();
        let certainty = if n == 1 {
            Certainty::Heuristic
        } else {
            Certainty::Ambiguous
        };
        for (kind, key) in targets {
            let dst = NodeKey::new(NodeKind::Symbol, key.to_owned());
            let e = self
                .0
                .entry((src.clone(), kind, dst))
                .or_insert_with(|| Resolved {
                    certainty,
                    candidates: 0,
                    evidence: BTreeSet::new(),
                });
            e.certainty = e.certainty.max(certainty);
            if certainty == Certainty::Ambiguous {
                e.candidates = e.candidates.max(n);
            }
            e.evidence.insert(at.clone());
        }
    }

    fn emit(self, b: &mut GraphBuilder) -> anyhow::Result<()> {
        for ((src, kind, dst), r) in self.0 {
            let mut attrs = Attrs::new();
            if r.certainty == Certainty::Ambiguous {
                attrs.insert("candidates".into(), r.candidates.into());
            }
            let mut edge =
                NewEdge::new(src, kind, dst, Provenance::Heuristic, r.certainty).attrs(attrs);
            edge.evidence = r.evidence.into_iter().collect();
            b.add_edge(edge)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use rb_model::{Attrs, EdgeKind, Graph, NodeKey, NodeKind};

    use super::*;

    struct Built {
        graph: Graph,
        diagnostics: Vec<Diagnostic>,
    }

    fn build(files: &[(&str, Language, &str)], go_modules: &[(&str, &str)]) -> Built {
        build_with_manifests(files, go_modules, &[])
    }

    /// [`build`] with manifest texts by path (`tsconfig.json`, `package.json`) for the
    /// index's alias maps, as `lib.rs` builds them.
    fn build_with_manifests(
        files: &[(&str, Language, &str)],
        go_modules: &[(&str, &str)],
        manifests: &[(&str, &str)],
    ) -> Built {
        let facts: Vec<(String, Language, FileFacts)> = files
            .iter()
            .map(|(p, l, src)| {
                (
                    (*p).to_owned(),
                    *l,
                    rb_syntax::extract(*l, src.as_bytes()).unwrap(),
                )
            })
            .collect();
        let by_path: BTreeMap<&str, (Language, &FileFacts)> = facts
            .iter()
            .map(|(p, l, f)| (p.as_str(), (*l, f)))
            .collect();
        let go: BTreeMap<String, String> = go_modules
            .iter()
            .map(|(d, m)| ((*d).to_owned(), (*m).to_owned()))
            .collect();
        let keys: BTreeMap<&str, Vec<String>> = by_path
            .iter()
            .map(|(p, (_, f))| (*p, rb_syntax::symbol_keys(p, f)))
            .collect();
        let mut b = GraphBuilder::new();
        for (path, (_, f)) in &by_path {
            b.add_node(NodeKey::new(NodeKind::File, *path), *path, Attrs::new())
                .unwrap();
            crate::syntax::add_symbols(&mut b, path, f, &keys[path]).unwrap();
        }
        let index = Index::new(&by_path, &go, &keys);
        let index = if manifests.is_empty() {
            index
        } else {
            index.with_aliases(crate::rta::AliasMaps::build(
                &manifests
                    .iter()
                    .map(|(p, t)| (*p, (*t).to_owned()))
                    .collect(),
            ))
        };
        let diagnostics = add_edges(&mut b, &index, &BTreeSet::new()).unwrap();
        Built {
            graph: b.finish().unwrap(),
            diagnostics,
        }
    }

    /// Runs `f` on the index of `files` (no Go modules, no manifests), for assertions on the
    /// index itself rather than on its edges.
    fn with_index(files: &[(&str, Language, &str)], f: impl FnOnce(&Index<'_>)) {
        let facts: Vec<(String, Language, FileFacts)> = files
            .iter()
            .map(|(p, l, src)| {
                (
                    (*p).to_owned(),
                    *l,
                    rb_syntax::extract(*l, src.as_bytes()).unwrap(),
                )
            })
            .collect();
        let by_path: BTreeMap<&str, (Language, &FileFacts)> = facts
            .iter()
            .map(|(p, l, f)| (p.as_str(), (*l, f)))
            .collect();
        let keys: BTreeMap<&str, Vec<String>> = by_path
            .iter()
            .map(|(p, (_, f))| (*p, rb_syntax::symbol_keys(p, f)))
            .collect();
        let go = BTreeMap::new();
        f(&Index::new(&by_path, &go, &keys));
    }

    /// `src -> dst [certainty]` (plus `(n)` candidates) for edges of `kind`, keys shortened to
    /// `file-stem:descriptor`.
    fn edges(built: &Built, kind: EdgeKind) -> Vec<String> {
        let g = &built.graph;
        let short = |id: i64| {
            let key = g.nodes().iter().find(|n| n.id == id).unwrap().key.clone();
            match key.rsplit_once("`/") {
                Some((prefix, desc)) => {
                    let file = prefix.rsplit('/').next().unwrap_or_default();
                    let stem = file.split('.').next().unwrap_or_default();
                    format!("{stem}:{desc}")
                }
                None => key,
            }
        };
        let mut out: Vec<String> = g
            .edges()
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| {
                let mut s = format!(
                    "{} -> {} [{}]",
                    short(e.src),
                    short(e.dst),
                    e.certainty.as_str()
                );
                if let Some(n) = e.attrs.get("candidates") {
                    s.push_str(&format!(" ({n})"));
                }
                s
            })
            .collect();
        out.sort();
        out
    }

    /// `(path, unresolved calls, unresolved types, external)` per file with any.
    fn unresolved(built: &Built) -> Vec<(String, u64, u64, u64)> {
        built
            .diagnostics
            .iter()
            .filter(|d| d.kind == "unresolved_references")
            .map(|d| {
                (
                    d.path.clone().unwrap_or_default(),
                    d.detail["calls"].as_u64().unwrap_or(0),
                    d.detail["types"].as_u64().unwrap_or(0),
                    d.detail["external"].as_u64().unwrap_or(0),
                )
            })
            .collect()
    }

    const J: Language = Language::Java;
    const TS: Language = Language::TypeScript;
    const PY: Language = Language::Python;
    const GO: Language = Language::Go;
    const KT: Language = Language::Kotlin;
    const CS: Language = Language::CSharp;

    /// Contract M6i S2: Kotlin calls resolve across files (bare top-level, typed-receiver
    /// methods); constructor calls reference their class; interface implementations link.
    #[test]
    fn kotlin_calls_types_and_implements_resolve() {
        let built = build(
            &[
                (
                    "svc/util.kt",
                    KT,
                    "package acme\nfun shout(s: String): String {\nreturn s\n}\nclass Greeter : GreeterApi {\nfun greet(name: String): String {\nreturn shout(name)\n}\n}\ninterface GreeterApi {\nfun greet(name: String): String\n}\n",
                ),
                (
                    "svc/main.kt",
                    KT,
                    "package acme\nfun main() {\nval g: Greeter = Greeter()\ng.greet(\"hi\")\nshout(\"yo\")\n}\n",
                ),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "main:main(). -> util:Greeter#greet(). [heuristic]",
                "main:main(). -> util:shout(). [heuristic]",
                "util:Greeter#greet(). -> util:shout(). [heuristic]",
            ]
        );
        assert_eq!(
            edges(&built, EdgeKind::Implements),
            [
                "util:Greeter# -> util:GreeterApi# [heuristic]",
                "util:Greeter#greet(). -> util:GreeterApi#greet(). [heuristic]",
            ]
        );
    }

    /// Contract M6i S2: C# calls resolve across files (static type calls, typed-receiver
    /// methods); constructor calls reference their class; interface implementations link.
    #[test]
    fn csharp_calls_types_and_implements_resolve() {
        let built = build(
            &[
                (
                    "svc/Util.cs",
                    CS,
                    "namespace Acme {\npublic static class Util {\npublic static string Shout(string s) {\nreturn s;\n}\n}\npublic interface IGreeter {\nstring Greet(string n);\n}\npublic class Greeter : IGreeter {\npublic string Greet(string n) {\nreturn Util.Shout(n);\n}\n}\n}\n",
                ),
                (
                    "svc/Main.cs",
                    CS,
                    "using Acme;\nclass Program {\nstatic void Main() {\nvar g = new Greeter();\ng.Greet(\"hi\");\n}\n}\n",
                ),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "Main:Program#Main(). -> Util:Acme/Greeter#Greet(). [heuristic]",
                "Util:Acme/Greeter#Greet(). -> Util:Acme/Util#Shout(). [heuristic]",
            ]
        );
        assert_eq!(
            edges(&built, EdgeKind::Implements),
            [
                "Util:Acme/Greeter# -> Util:Acme/IGreeter# [heuristic]",
                "Util:Acme/Greeter#Greet(). -> Util:Acme/IGreeter#Greet(). [heuristic]",
            ]
        );
    }

    /// Contract M6i S2 follow-up: Kotlin wildcard imports resolve names like Java's do
    /// (a star-imported class used through an explicitly-typed binding).
    #[test]
    fn kotlin_wildcard_imports_resolve() {
        let built = build(
            &[
                (
                    "model/User.kt",
                    KT,
                    "package com.acme.model\nclass User {\nfun name(): String {\nreturn \"u\"\n}\n}\n",
                ),
                (
                    "web/Ctl.kt",
                    KT,
                    "package com.acme.web\nimport com.acme.model.*\nclass Ctl {\nfun run(): String {\nval u: User = User()\nreturn u.name()\n}\n}\n",
                ),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            ["Ctl:Ctl#run(). -> User:User#name(). [heuristic]"]
        );
        assert_eq!(unresolved(&built), []);
    }

    #[test]
    fn java_levels_typed_receivers_statics_and_inheritance() {
        let built = build(
            &[
                (
                    "src/web/Ctl.java",
                    J,
                    r#"package com.acme.web;
import com.acme.svc.UserService;
import static com.acme.util.Strings.trim;
class Ctl extends Base {
    private UserService users;
    void handle(String id) {
        users.find(trim(id));
        helper();
        inherited();
        Helper h = new Helper();
        h.go();
        unknown.save();
        com.acme.util.Strings.trim(id);
    }
    void helper() {}
}
"#,
                ),
                (
                    "src/web/Base.java",
                    J,
                    "package com.acme.web;\nclass Base { void inherited() {} }\n",
                ),
                (
                    "src/web/Helper.java",
                    J,
                    "package com.acme.web;\nclass Helper { Helper() {} void go() {} }\n",
                ),
                (
                    "src/svc/UserService.java",
                    J,
                    "package com.acme.svc;\npublic class UserService { public User find(String id) { return null; } }\n",
                ),
                (
                    "src/util/Strings.java",
                    J,
                    "package com.acme.util;\npublic class Strings { public static String trim(String s) { return s; } }\n",
                ),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "Ctl:Ctl#handle(). -> Base:Base#inherited(). [heuristic]",
                "Ctl:Ctl#handle(). -> Ctl:Ctl#helper(). [heuristic]",
                "Ctl:Ctl#handle(). -> Helper:Helper#go(). [heuristic]",
                "Ctl:Ctl#handle(). -> Strings:Strings#trim(). [heuristic]",
                "Ctl:Ctl#handle(). -> UserService:UserService#find(). [heuristic]",
            ]
        );
        assert_eq!(
            edges(&built, EdgeKind::Implements),
            ["Ctl:Ctl# -> Base:Base# [heuristic]"]
        );
        // `new Helper()` is a class reference (Tree-sitter does not tag it as a call).
        assert!(
            edges(&built, EdgeKind::References)
                .contains(&"Ctl:Ctl#handle(). -> Helper:Helper# [heuristic]".to_owned())
        );
        // `unknown.save()` has no known receiver type; the qualified `com.acme…trim` resolves.
        assert_eq!(unresolved(&built), [("src/web/Ctl.java".into(), 1, 0, 0)]);
    }

    #[test]
    fn typescript_imports_this_members_and_namespaces() {
        let built = build(
            &[
                (
                    "src/app.ts",
                    TS,
                    r#"import { UserRepo } from './repo';
import * as util from './util';
import express from 'express';
export class App {
  constructor(private repo: UserRepo) {}
  run(id: string) {
    this.repo.find(id);
    this.log(id);
    util.format(id);
    local();
    express();
    const r = new UserRepo();
    r.find(id);
  }
  log(s: string) {}
}
function local() {}
"#,
                ),
                (
                    "src/repo.ts",
                    TS,
                    "export class UserRepo { find(id: string) {} }\n",
                ),
                ("src/util.ts", TS, "export function format(s: string) {}\n"),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "app:App#run(). -> app:App#log(). [heuristic]",
                "app:App#run(). -> app:local(). [heuristic]",
                "app:App#run(). -> repo:UserRepo#find(). [heuristic]",
                "app:App#run(). -> util:format(). [heuristic]",
            ]
        );
        // `new UserRepo()` without a declared constructor references the class.
        assert!(
            edges(&built, EdgeKind::References)
                .contains(&"app:App#run(). -> repo:UserRepo# [heuristic]".to_owned())
        );
        // `express()` comes from an external package.
        assert_eq!(unresolved(&built), [("src/app.ts".into(), 0, 0, 1)]);
    }

    /// Contract M9c C2 (D1): bookcars' `admin/src/pages/ChangePassword.tsx` calls
    /// `helper.info` through `import * as helper from '@/utils/helper'`, an alias of
    /// `admin/tsconfig.json`, whose comments and trailing commas strict JSON refused. A named
    /// import through the alias resolves too. `frontend/`'s broken tsconfig resolves nothing.
    #[test]
    fn namespace_calls_behind_jsonc_paths_resolve() {
        let page = r#"import * as helper from '@/utils/helper'
import { error } from '@/utils/helper'
export function ChangePassword() {
  helper.info('updated')
  error()
}
"#;
        let helper = "export const info = (message: string) => {\n  toast(message)\n}\n\
                      export function error() {}\n";
        let built = build_with_manifests(
            &[
                ("admin/src/pages/ChangePassword.tsx", Language::Tsx, page),
                ("admin/src/utils/helper.ts", TS, helper),
                ("frontend/src/pages/ChangePassword.tsx", Language::Tsx, page),
                ("frontend/src/utils/helper.ts", TS, helper),
            ],
            &[],
            &[
                (
                    "admin/tsconfig.json",
                    r#"{
  "compilerOptions": {
    /* Bundler mode */
    "moduleResolution": "bundler",
    "paths": {
      "@/*": ["./src/*"],
    },
  },
  "include": ["src"],
}
"#,
                ),
                (
                    "frontend/tsconfig.json",
                    r#"{"compilerOptions": {"paths": {"@/*": ["./src/*"]}}"#,
                ),
            ],
        );
        let calls = edges(&built, EdgeKind::Calls);
        let admin: Vec<&String> = calls.iter().filter(|e| e.contains("-> helper:")).collect();
        assert_eq!(
            admin,
            [
                "ChangePassword:ChangePassword(). -> helper:error(). [heuristic]",
                "ChangePassword:ChangePassword(). -> helper:info(). [heuristic]",
            ],
            "{calls:?}"
        );
        // Both callers are `ChangePassword:…`; only admin's resolve, so each edge is admin's.
        let admin_calls = built
            .graph
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| {
                let key = |id| {
                    built
                        .graph
                        .nodes()
                        .iter()
                        .find(|n| n.id == id)
                        .unwrap()
                        .key
                        .clone()
                };
                (key(e.src), key(e.dst))
            })
            .filter(|(s, d)| s.contains("admin/src/pages") && d.contains("admin/src/utils"))
            .count();
        assert_eq!(admin_calls, 2);
    }

    /// Contract M9c C3 (D2): an `import x from './m'` names the definition `m`'s
    /// `export default` names, whatever the local name; a target without a default export
    /// gives nothing, as before.
    #[test]
    fn default_imports_resolve_to_the_default_export() {
        let built = build(
            &[
                (
                    "src/app.ts",
                    TS,
                    r#"import f from './m'
import local from './n'
import h from './o'
export function run() {
  f()
  local()
  h()
}
"#,
                ),
                ("src/m.ts", TS, "function f() {}\nexport default f\n"),
                (
                    "src/n.ts",
                    TS,
                    "export default function named() {}\nexport function local() {}\n",
                ),
                ("src/o.ts", TS, "export function h() {}\n"),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "app:run(). -> m:f(). [heuristic]",
                "app:run(). -> n:named(). [heuristic]",
            ]
        );
    }

    #[test]
    fn python_modules_self_members_and_relative_imports() {
        let built = build(
            &[
                (
                    "app/views.py",
                    PY,
                    r#"import app.util
from .repo import Repo
from . import util as u

class View:
    def __init__(self):
        self.repo = Repo()

    def get(self, x):
        self.repo.load(x)
        self.render(x)
        app.util.fmt(x)
        u.fmt(x)
        helper(x)
        x.strip()

    def render(self, x):
        pass

def helper(x):
    pass
"#,
                ),
                (
                    "app/repo.py",
                    PY,
                    "class Repo:\n    def __init__(self):\n        pass\n    def load(self, x):\n        pass\n",
                ),
                ("app/util.py", PY, "def fmt(x):\n    pass\n"),
                ("app/__init__.py", PY, ""),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "views:View#__init__(). -> repo:Repo#__init__(). [heuristic]",
                "views:View#get(). -> repo:Repo#load(). [heuristic]",
                "views:View#get(). -> util:fmt(). [heuristic]",
                "views:View#get(). -> views:View#render(). [heuristic]",
                "views:View#get(). -> views:helper(). [heuristic]",
            ]
        );
        // `x.strip()`: an untyped receiver is never linked by name.
        assert_eq!(unresolved(&built), [("app/views.py".into(), 1, 0, 0)]);
    }

    #[test]
    fn go_packages_receivers_fields_and_same_directory() {
        let built = build(
            &[
                (
                    "svc/server.go",
                    GO,
                    r#"package svc
import "example.com/app/store"
type Server struct { db store.DB }
func (s *Server) Run() {
    s.db.Get()
    s.helper()
    shared()
    store.Open()
    fmt.Println()
}
func (s *Server) helper() {}
"#,
                ),
                ("svc/shared.go", GO, "package svc\nfunc shared() {}\n"),
                (
                    "store/db.go",
                    GO,
                    "package store\ntype DB struct {}\nfunc (d *DB) Get() {}\nfunc Open() {}\n",
                ),
            ],
            &[("", "example.com/app")],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "server:Server#Run(). -> db:DB#Get(). [heuristic]",
                "server:Server#Run(). -> db:Open(). [heuristic]",
                "server:Server#Run(). -> server:Server#helper(). [heuristic]",
                "server:Server#Run(). -> shared:shared(). [heuristic]",
            ]
        );
        assert_eq!(unresolved(&built), [("svc/server.go".into(), 1, 0, 0)]);
    }

    /// Contract M9b D5: a Go method declared in another file than its receiver type is a member
    /// of that type, found through the type's own package (directory and package name), and is
    /// never a bare package-level name.
    #[test]
    fn go_methods_in_sibling_files_are_members() {
        let built = build(
            &[
                ("svc/h.go", GO, "package svc\ntype H struct{}\n"),
                (
                    "svc/m.go",
                    GO,
                    "package svc\nfunc (h *H) M() {}\nfunc bare() { M() }\n",
                ),
                (
                    "svc/use.go",
                    GO,
                    "package svc\nfunc run(h *H) {\n\th.M()\n\th.T()\n\th.N()\n}\nfunc other() { M() }\n",
                ),
                // An internal test file attaches like any other.
                ("svc/z_test.go", GO, "package svc\nfunc (h *H) T() {}\n"),
                // The external test package in the same directory has its own `H`.
                ("svc/x_test.go", GO, "package svc_test\ntype H struct{}\n"),
                (
                    "svc/y_test.go",
                    GO,
                    "package svc_test\nfunc (h *H) N() {}\nfunc use3(h *H) { h.M() }\n",
                ),
                // A function-local `H` is another type: the package `H`'s methods are not its
                // own (M9b pass 1, E1).
                (
                    "svc/loc.go",
                    GO,
                    "package svc\nfunc loc() {\n\ttype H struct{}\n\tvar h H\n\th.M()\n}\n",
                ),
                // Another package with its own `H`, which has no `M`.
                ("b/h.go", GO, "package b\ntype H struct{}\n"),
                ("b/use.go", GO, "package b\nfunc run2(h *H) { h.M() }\n"),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "use:run(). -> m:H#M(). [heuristic]",
                "use:run(). -> z_test:H#T(). [heuristic]",
            ]
        );
    }

    /// Contract M9c C5 (D4, call results and call segments, Go only): a local initialised by a
    /// call takes the callee's result type, `T` or `(T, error)` with `*` stripped; `(T, U)` and
    /// three results give none; several callees give several candidates. A qualifier is split
    /// at bracket depth 0, so `a.Keep(cfg.Size(x.y))` is two segments, and a call segment steps
    /// to the result of a member, a module's function, or (at the head) a callable of the
    /// levels. A bracketed segment that is not a call (`repo[0]`) stops the walk.
    #[test]
    fn go_call_results_and_segments_type_receivers() {
        let built = build(
            &[
                (
                    "svc/svc.go",
                    GO,
                    "package svc
type Service struct {
\trepo *Repo
}
type Repo struct{}
type Box struct{}
func New() *Service { return nil }
func NewE() (*Service, error) { return nil, nil }
func NewPair() (*Service, *Repo) { return nil, nil }
func NewThree() (*Service, *Repo, error) { return nil, nil, nil }
func (s *Service) Srv() *Service { return s }
func (s *Service) Keep(n int) Box { return Box{} }
func (s *Service) Run() {}
func (r *Repo) Get() {}
func (b Box) Save() {}
func head() {
\tNew().Srv().Run()
}
",
                ),
                (
                    "use/use.go",
                    GO,
                    "package use
import \"example.com/app/svc\"
func f1() {
\ta := svc.New()
\ta.Run()
}
func f2() {
\tb, err := svc.NewE()
\tb.Run()
}
func f3() {
\tc, d := svc.NewPair()
\tc.Run()
}
func f4() {
\te := svc.NewThree()
\te.Run()
}
func f5(cfg Config) {
\ta := svc.New()
\ta.Keep(cfg.Size(x.y)).Save()
}
func f6() {
\tsvc.New().Srv().Run()
}
func f7() {
\ta := svc.New()
\ta.repo[0].Get()
}
",
                ),
                (
                    "multi/a.go",
                    GO,
                    "package multi\ntype A struct{}\nfunc (a *A) Hi() {}\nfunc Make() *A { return nil }\n",
                ),
                (
                    "multi/b.go",
                    GO,
                    "package multi\ntype B struct{}\nfunc (b *B) Hi() {}\nfunc Make() *B { return nil }\n",
                ),
                (
                    "multi/m.go",
                    GO,
                    "package multi\nfunc g() {\n\tm := Make()\n\tm.Hi()\n}\n",
                ),
            ],
            &[("", "example.com/app")],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "m:g(). -> a:A#Hi(). [ambiguous] (2)",
                "m:g(). -> a:Make(). [ambiguous] (2)",
                "m:g(). -> b:B#Hi(). [ambiguous] (2)",
                "m:g(). -> b:Make(). [ambiguous] (2)",
                "svc:head(). -> svc:New(). [heuristic]",
                "svc:head(). -> svc:Service#Run(). [heuristic]",
                "svc:head(). -> svc:Service#Srv(). [heuristic]",
                "use:f1(). -> svc:New(). [heuristic]",
                "use:f1(). -> svc:Service#Run(). [heuristic]",
                "use:f2(). -> svc:NewE(). [heuristic]",
                "use:f2(). -> svc:Service#Run(). [heuristic]",
                "use:f3(). -> svc:NewPair(). [heuristic]",
                "use:f4(). -> svc:NewThree(). [heuristic]",
                "use:f5(). -> svc:Box#Save(). [heuristic]",
                "use:f5(). -> svc:New(). [heuristic]",
                "use:f5(). -> svc:Service#Keep(). [heuristic]",
                "use:f6(). -> svc:New(). [heuristic]",
                "use:f6(). -> svc:Service#Run(). [heuristic]",
                "use:f6(). -> svc:Service#Srv(). [heuristic]",
                "use:f7(). -> svc:New(). [heuristic]",
            ]
        );
    }

    /// Contract M9c C5 (D4, aliases): `type A = B` resolves to what `B` resolves to in the
    /// alias's file, through a chain of aliases, a pointer target and a sibling file; a cycle
    /// (`A = B; B = A`) resolves to nothing. Type references land on the target as calls do.
    #[test]
    fn go_type_aliases_resolve_to_their_target() {
        let built = build(
            &[
                (
                    "web/ctx.go",
                    GO,
                    "package web\ntype Context struct{}\nfunc (c *Context) JSON() {}\n",
                ),
                (
                    "api/handlers.go",
                    GO,
                    "package api
import \"example.com/app/web\"
type Context = web.Context
type Ctx2 = Context
type P = *web.Context
type A = B
type B = A
func h1(c *Context) { c.JSON() }
func h2(c *Ctx2) { c.JSON() }
func h3(c P) { c.JSON() }
func h4(c *A) { c.JSON() }
",
                ),
                (
                    "api/other.go",
                    GO,
                    "package api\nfunc h5(c Context) { c.JSON() }\n",
                ),
            ],
            &[("", "example.com/app")],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "handlers:h1(). -> ctx:Context#JSON(). [heuristic]",
                "handlers:h2(). -> ctx:Context#JSON(). [heuristic]",
                "handlers:h3(). -> ctx:Context#JSON(). [heuristic]",
                "other:h5(). -> ctx:Context#JSON(). [heuristic]",
            ]
        );
        let functions: Vec<String> = edges(&built, EdgeKind::References)
            .into_iter()
            .filter(|e| e.contains(":h"))
            .collect();
        assert_eq!(
            functions,
            [
                "handlers:h1(). -> ctx:Context# [heuristic]",
                "handlers:h2(). -> ctx:Context# [heuristic]",
                "handlers:h3(). -> ctx:Context# [heuristic]",
                "other:h5(). -> ctx:Context# [heuristic]",
            ]
        );
    }

    /// Contract M9c C5 (D4, embedding): a member the type and its sibling-file methods lack is
    /// searched in the embedded types breadth-first; the shallower depth wins, a tie at one
    /// depth is ambiguous, promoted fields walk the same way, and embedding never makes an
    /// `IMPLEMENTS` edge.
    #[test]
    fn go_embedded_types_promote_members() {
        let built = build(
            &[
                (
                    "p/types.go",
                    GO,
                    "package p
type Logger struct{}
func (l *Logger) Write() {}
type Base struct {
\tLog *Logger
}
func (b *Base) Hello() {}
func (b *Base) Both() {}
type Mid struct {
\t*Base
}
func (m *Mid) Name() {}
type Other struct{}
func (o *Other) Both() {}
func (o *Other) Name() {}
type Top struct {
\t*Mid
\tOther
}
func (t *Top) Own() {}
",
                ),
                ("p/more.go", GO, "package p\nfunc (b *Base) Later() {}\n"),
                (
                    "p/use.go",
                    GO,
                    "package p
func own(t *Top) { t.Own() }
func deep(t *Top) { t.Hello() }
func tie(t *Top) { t.Name() }
func near(t *Top) { t.Both() }
func field(m *Mid) { m.Log.Write() }
func explicit(t *Top) { t.Mid.Name() }
func later(t *Top) { t.Later() }
",
                ),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "use:deep(). -> types:Base#Hello(). [heuristic]",
                "use:explicit(). -> types:Mid#Name(). [heuristic]",
                "use:field(). -> types:Logger#Write(). [heuristic]",
                "use:later(). -> more:Base#Later(). [heuristic]",
                "use:near(). -> types:Other#Both(). [heuristic]",
                "use:own(). -> types:Top#Own(). [heuristic]",
                "use:tie(). -> types:Mid#Name(). [ambiguous] (2)",
                "use:tie(). -> types:Other#Name(). [ambiguous] (2)",
            ]
        );
        assert_eq!(edges(&built, EdgeKind::Implements), Vec::<String>::new());
        // Embedding never enters `supertypes()`, which DI reads through `direct_supertypes`;
        // the embedded type is seen, as an embed.
        let src = "package p\ntype Base struct{}\ntype Top struct {\n\t*Base\n}\n";
        with_index(&[("p/t.go", GO, src)], |index| {
            let top = ("p/t.go", 1);
            assert_eq!(index.def(top).name, "Top");
            assert_eq!(index.embedded_types(top), [("p/t.go", 0)]);
            assert_eq!(index.direct_supertypes(top), Vec::<Sym>::new());
        });
    }

    /// Contract M9c C5 (D4, package vars): a qualifier head with no local binding is a
    /// package-level var of a sibling file when the package declares that name exactly once
    /// (any kind), the var has a type, and a non-test file is not reading a test file's var. A
    /// local of the same name shadows it; another directory never sees it.
    #[test]
    fn go_package_vars_of_sibling_files_type_names() {
        let built = build(
            &[
                (
                    "svc/helper.go",
                    GO,
                    "package svc
type Helper struct{}
func (h *Helper) Setup() {}
type Other struct{}
var shared *Helper
var twice *Helper
var mixed *Helper
",
                ),
                (
                    "svc/d.go",
                    GO,
                    "package svc\nvar twice *Helper\nfunc mixed() {}\n",
                ),
                (
                    "svc/main_test.go",
                    GO,
                    "package svc\nvar mainHelper *Helper\n",
                ),
                (
                    "svc/a_test.go",
                    GO,
                    "package svc
func TestA() { mainHelper.Setup() }
func TestB() {
\tmainHelper := Other{}
\tmainHelper.Setup()
}
func TestC() { shared.Setup() }
",
                ),
                (
                    "svc/b.go",
                    GO,
                    "package svc
func run() { mainHelper.Setup() }
func run2() { shared.Setup() }
func run3() { twice.Setup() }
func run4() { mixed.Setup() }
",
                ),
                (
                    "other/x.go",
                    GO,
                    "package other\nfunc o() { shared.Setup() }\n",
                ),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "a_test:TestA(). -> helper:Helper#Setup(). [heuristic]",
                "a_test:TestC(). -> helper:Helper#Setup(). [heuristic]",
                "b:run2(). -> helper:Helper#Setup(). [heuristic]",
            ]
        );
    }

    /// Contract M9c C7 (D6): one `tier_b_forms` diagnostic per file that resolved a reference
    /// through a new form, every key present; a reference counts each form it used once (two
    /// call segments in one qualifier count once), an unresolved reference counts nothing, and a
    /// file that used no form has no diagnostic.
    #[test]
    fn tier_b_forms_count_each_form() {
        let built = build(
            &[
                (
                    "svc/svc.go",
                    GO,
                    "package svc\ntype Service struct{}\nfunc New() *Service { return nil }\nfunc (s *Service) Srv() *Service { return s }\nfunc (s *Service) Run() {}\n",
                ),
                (
                    "use/use.go",
                    GO,
                    "package use
import \"example.com/app/svc\"
type Alias = svc.Service
type Wrap struct {
\t*svc.Service
}
func f(s *svc.Service, al *Alias, w *Wrap) {
\ta := svc.New()
\ta.Run()
\ts.Srv().Srv().Run()
\tal.Run()
\tw.Run()
\tglobal.Run()
\ta.Missing()
}
",
                ),
                (
                    "use/vars.go",
                    GO,
                    "package use\nimport \"example.com/app/svc\"\nvar global *svc.Service\n",
                ),
                (
                    "use/plain.go",
                    GO,
                    "package use\nimport \"example.com/app/svc\"\nfunc p(s *svc.Service) { s.Run() }\n",
                ),
                ("web/m.ts", TS, "export default function g() {}\n"),
                (
                    "web/u.ts",
                    TS,
                    "import f from './m';\nfunction run() { f(); f(); }\n",
                ),
            ],
            &[("", "example.com/app")],
        );
        let forms: Vec<(String, serde_json::Value)> = built
            .diagnostics
            .iter()
            .filter(|d| d.kind == "tier_b_forms")
            .map(|d| (d.path.clone().unwrap_or_default(), d.detail.clone()))
            .collect();
        let zero = json!({
            "return_type": 0, "call_segment": 0, "type_alias": 0, "embedded": 0,
            "package_var": 0, "default_import": 0, "super_call": 0, "star_import": 0,
            "reexport": 0, "module_variable": 0, "inherited_init": 0,
        });
        let with = |pairs: &[(&str, u64)]| {
            let mut v = zero.clone();
            for (k, n) in pairs {
                v[*k] = json!(n);
            }
            v
        };
        assert_eq!(
            forms,
            [
                (
                    "use/use.go".to_owned(),
                    with(&[
                        ("return_type", 1),
                        ("call_segment", 2),
                        ("type_alias", 2),
                        ("embedded", 1),
                        ("package_var", 1),
                    ])
                ),
                ("web/u.ts".to_owned(), with(&[("default_import", 2)])),
            ]
        );
    }

    #[test]
    fn several_candidates_are_ambiguous_and_levels_stop_early() {
        let built = build(
            &[
                (
                    "a/Main.java",
                    J,
                    "package p;\nclass Main { void m(Shape s) { s.area(); } }\n",
                ),
                (
                    "a/Shape.java",
                    J,
                    "package p;\nclass Shape { double area() { return 0; } }\n",
                ),
                (
                    "b/Shape.java",
                    J,
                    "package p;\nclass Shape { double area() { return 1; } }\n",
                ),
                (
                    "go/a.go",
                    GO,
                    "package g\nfunc helper() {}\nfunc run() { helper() }\n",
                ),
                ("go/b.go", GO, "package g\nfunc helper2() {}\n"),
            ],
            &[],
        );
        let calls = edges(&built, EdgeKind::Calls);
        assert_eq!(
            calls,
            [
                "Main:Main#m(). -> Shape:Shape#area(). [ambiguous] (2)",
                "Main:Main#m(). -> Shape:Shape#area(). [ambiguous] (2)",
                // The same file wins over the same directory.
                "a:run(). -> a:helper(). [heuristic]",
            ]
        );
    }

    #[test]
    fn implements_types_and_overridden_methods() {
        let built = build(
            &[
                (
                    "src/Repo.java",
                    J,
                    "package r;\ninterface Repo { void save(); }\nclass SqlRepo implements Repo { public void save() {} void other() {} }\nclass Cached extends SqlRepo { public void save() {} }\n",
                ),
                (
                    "src/shapes.ts",
                    TS,
                    "interface Shape { area(): number }\nclass Sq implements Shape { area() { return 1 } }\nclass Ext extends Unknown {}\n",
                ),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Implements),
            [
                "Repo:Cached# -> Repo:SqlRepo# [heuristic]",
                "Repo:Cached#save(). -> Repo:SqlRepo#save(). [heuristic]",
                "Repo:SqlRepo# -> Repo:Repo# [heuristic]",
                "Repo:SqlRepo#save(). -> Repo:Repo#save(). [heuristic]",
                "shapes:Sq# -> shapes:Shape# [heuristic]",
                "shapes:Sq#area(). -> shapes:Shape#area(). [heuristic]",
            ]
        );
    }

    #[test]
    fn type_references_resolve_through_imports() {
        let built = build(
            &[
                (
                    "src/a.ts",
                    TS,
                    "import { User } from './user';\nexport function f(u: User): Missing { return u; }\n",
                ),
                ("src/user.ts", TS, "export interface User { id: string }\n"),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::References),
            ["a:f(). -> user:User# [heuristic]"]
        );
        assert_eq!(unresolved(&built), [("src/a.ts".into(), 0, 1, 0)]);
    }

    #[test]
    fn cyclic_and_deep_hierarchies_terminate() {
        let mut deep = String::from("package d;\nclass C0 { void top() {} }\n");
        for i in 1..3000 {
            deep.push_str(&format!("class C{i} extends C{} {{}}\n", i - 1));
        }
        deep.push_str("class Use { void m(C2999 c) { c.top(); } }\n");
        let built = build(
            &[
                (
                    "src/Cycle.java",
                    J,
                    "package c;\nclass A extends B {}\nclass B extends A {}\nclass U { void m(A a) { a.missing(); } }\n",
                ),
                ("src/Deep.java", J, &deep),
            ],
            &[],
        );
        let calls = edges(&built, EdgeKind::Calls);
        // Past the inheritance depth cap the method is not found, but nothing hangs.
        assert!(calls.iter().all(|c| !c.contains("missing")), "{calls:?}");
    }

    #[test]
    fn builtins_are_skipped_and_external_packages_counted_apart() {
        let built = build(
            &[(
                "billing/invoice.go",
                GO,
                "package invoice\nimport \"database/sql\"\nfunc Total(lines []int64) int64 { var t int64; return t + int64(len(lines)) }\nfunc Mark(db *sql.DB) error { _, err := db.Exec(\"x\"); return err }\nfunc run() { missing() }\n",
            )],
            &[],
        );
        // `int64`, `error`, `len`: built-in. `sql.DB` and `db.Exec`: the external `database/sql`.
        assert_eq!(unresolved(&built), [("billing/invoice.go".into(), 1, 0, 2)]);
        let built = build(
            &[(
                "app/x.py",
                PY,
                "import requests\ndef f(items):\n    print(len(items))\n    requests.get('u')\n",
            )],
            &[],
        );
        assert_eq!(unresolved(&built), [("app/x.py".into(), 0, 0, 1)]);
    }

    #[test]
    fn self_referential_bindings_terminate() {
        let built = build(
            &[(
                "app/s.py",
                PY,
                "def f():\n    session = session.Session()\n    session.get()\n    a = b.X()\n    b = a.Y()\n    a.go()\n",
            )],
            &[],
        );
        assert!(edges(&built, EdgeKind::Calls).is_empty());
        let ts = build(
            &[(
                "src/a.ts",
                TS,
                "function f() { const x: x.T = mk(); x.go(); }\nclass C { c: C.D; m() { this.c.go(); } }\n",
            )],
            &[],
        );
        assert!(edges(&ts, EdgeKind::Calls).is_empty());
    }

    #[test]
    fn variables_never_resolve_through_a_same_named_type() {
        let go = build(
            &[(
                "main.go",
                GO,
                "package main\nimport \"net/http\"\ntype client struct{}\nfunc (c *client) Do(x int) {}\ntype handler struct{}\nfunc (h handler) ServeHTTP() {}\nfunc run() {\n    client := &http.Client{}\n    client.Do(nil)\n    handler := mux.Handler(nil)\n    handler.ServeHTTP()\n}\n",
            )],
            &[],
        );
        assert!(
            edges(&go, EdgeKind::Calls)
                .iter()
                .all(|e| !e.starts_with("main:run().")),
            "{:?}",
            edges(&go, EdgeKind::Calls)
        );
        let js = build(
            &[(
                "src/app.js",
                Language::JavaScript,
                "class Hono { get() {} }\nconst app = new Hono();\napp.get();\nfunction register(app) { app.get(); }\n",
            )],
            &[],
        );
        // The untyped parameter hides the typed module-level `app`.
        assert_eq!(
            edges(&js, EdgeKind::Calls),
            ["src/app.js -> app:Hono#get(). [heuristic]"]
        );
        let py = build(
            &[(
                "app/m.py",
                PY,
                "class Repo:\n    def save(self):\n        pass\n\nclass Svc:\n    repo = Repo()\n    def run(self):\n        repo.save()\n        self.repo.save()\n",
            )],
            &[],
        );
        // A class-body name is an attribute, not a local of its methods.
        assert_eq!(
            edges(&py, EdgeKind::Calls),
            ["m:Svc#run(). -> m:Repo#save(). [heuristic]"]
        );
        assert_eq!(unresolved(&py), [("app/m.py".into(), 1, 0, 0)]);
    }

    #[test]
    fn java_same_package_beats_wildcard_imports() {
        let built = build(
            &[
                (
                    "p/Widget.java",
                    J,
                    "package p;\npublic class Widget { public void spin() {} }\n",
                ),
                (
                    "q/Widget.java",
                    J,
                    "package q;\npublic class Widget { public void spin() {} }\n",
                ),
                (
                    "q/User.java",
                    J,
                    "package q;\nimport p.*;\nclass User { void run(Widget w) { w.spin(); } }\n",
                ),
            ],
            &[],
        );
        let calls = edges(&built, EdgeKind::Calls);
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(built.graph.edges().iter().any(|e| {
            let dst = &built
                .graph
                .nodes()
                .iter()
                .find(|n| n.id == e.dst)
                .unwrap()
                .key;
            e.kind == EdgeKind::Calls && dst.contains("`q/Widget.java`")
        }));
    }

    #[test]
    fn rebinding_follows_the_nearest_preceding_assignment() {
        let built = build(
            &[(
                "app/r.py",
                PY,
                "class A:\n    def go(self):\n        pass\n\nclass B:\n    def go(self):\n        pass\n\ndef f():\n    x = A()\n    x.go()\n    x = B()\n    x.go()\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "r:f(). -> r:A#__init__(). [heuristic]",
                "r:f(). -> r:A#go(). [heuristic]",
                "r:f(). -> r:B#go(). [heuristic]",
            ]
            .iter()
            .filter(|e| !e.contains("__init__"))
            .map(|e| (*e).to_owned())
            .collect::<Vec<_>>()
        );
    }

    #[test]
    fn go_test_packages_are_not_part_of_the_imported_package() {
        for test_file in ["util/util_ext_test.go", "util/0_test.go"] {
            let built = build(
                &[
                    ("util/util.go", GO, "package util\nfunc Helper() {}\n"),
                    (test_file, GO, "package util_test\nfunc Helper() {}\n"),
                    (
                        "cmd/main.go",
                        GO,
                        "package main\nimport \"example.com/m/util\"\nfunc main() { util.Helper() }\n",
                    ),
                ],
                &[("", "example.com/m")],
            );
            assert_eq!(
                edges(&built, EdgeKind::Calls),
                ["main:main(). -> util:Helper(). [heuristic]"],
                "{test_file}"
            );
        }
    }

    #[test]
    fn recursive_calls_are_edges() {
        let built = build(
            &[(
                "src/a.ts",
                TS,
                "function fact(n: number): number { return fact(n - 1); }\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            ["a:fact(). -> a:fact(). [heuristic]"]
        );
    }

    #[test]
    fn field_chains_binding_chains_and_big_packages_stay_fast() {
        // Duplicate member bindings and duplicate classes must not multiply per segment.
        let chain = vec!["f"; 24].join(".");
        let src = format!(
            "class X:\n    def a(self):\n        self.f = X()\n    def b(self):\n        self.f = X()\n    def m(self):\n        pass\n\nclass X:\n    def c(self):\n        self.f = X()\n\ndef run(x: X):\n    x.{chain}.m()\n"
        );
        let started = std::time::Instant::now();
        build(&[("app/x.py", PY, &src)], &[]);
        assert!(
            started.elapsed().as_secs() < 2,
            "field chain {:?}",
            started.elapsed()
        );

        // A long chain of bindings in one function.
        let mut src = String::from("def run(x0):\n");
        for i in 1..4000 {
            src.push_str(&format!("    x{i} = x{}.m()\n    x{i}.go()\n", i - 1));
        }
        let started = std::time::Instant::now();
        build(&[("app/y.py", PY, &src)], &[]);
        assert!(
            started.elapsed().as_secs() < 3,
            "binding chain {:?}",
            started.elapsed()
        );

        // Many files in one Java package, each with unresolved calls.
        let files: Vec<(String, String)> = (0..1500)
            .map(|i| {
                (
                    format!("p/C{i}.java"),
                    format!("package p;\nclass C{i} {{ void m() {{ a(); b(); c(); new Missing(); d(); e(); }} }}\n"),
                )
            })
            .collect();
        let refs: Vec<(&str, Language, &str)> = files
            .iter()
            .map(|(p, s)| (p.as_str(), J, s.as_str()))
            .collect();
        let started = std::time::Instant::now();
        build(&refs, &[]);
        assert!(
            started.elapsed().as_secs() < 5,
            "package scan {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn every_declaration_form_makes_a_variable() {
        // Each variable shares its name with a type, package or field; none may resolve to it.
        let py = build(
            &[
                (
                    "pkg/models.py",
                    PY,
                    "class user:\n    def save(self):\n        pass\nclass conn:\n    def close(self):\n        pass\nclass item:\n    def run(self):\n        pass\nclass err:\n    def log(self):\n        pass\n",
                ),
                (
                    "pkg/app.py",
                    PY,
                    "from pkg.models import user, conn, item, err\ndef f(users, items):\n    for user in users:\n        user.save()\n    with open() as conn:\n        conn.close()\n    x = [item.run() for item in items]\n    try:\n        pass\n    except Exception as err:\n        err.log()\n    a, conn = 1, 2\n    conn.close()\n    if (user := get()):\n        user.save()\n",
                ),
            ],
            &[],
        );
        assert!(
            edges(&py, EdgeKind::Calls).is_empty(),
            "{:?}",
            edges(&py, EdgeKind::Calls)
        );

        let go = build(
            &[
                ("user/user.go", GO, "package user\nfunc Save() {}\n"),
                (
                    "main.go",
                    GO,
                    "package main\nimport \"example.com/m/user\"\ntype client struct{}\nfunc (c *client) Do() {}\nfunc run(users []int, x any) {\n    for _, user := range users { user.Save() }\n    a, client := f()\n    client.Do()\n    switch client := x.(type) { case int: client.Do() }\n}\n",
                ),
            ],
            &[("", "example.com/m")],
        );
        assert!(
            edges(&go, EdgeKind::Calls).is_empty(),
            "{:?}",
            edges(&go, EdgeKind::Calls)
        );

        let ts = build(
            &[
                (
                    "src/lib.ts",
                    TS,
                    "export class item { go() {} }\nexport class client { run() {} }\nexport class err { log() {} }\n",
                ),
                (
                    "src/app.ts",
                    TS,
                    "import { item, client, err } from './lib';\nfunction f(items: any[], o: any) {\n  for (const item of items) item.go();\n  const { client } = o;\n  client.run();\n  items.map(item => item.go());\n  try {} catch (err) { err.log(); }\n}\n",
                ),
            ],
            &[],
        );
        assert!(
            edges(&ts, EdgeKind::Calls).is_empty(),
            "{:?}",
            edges(&ts, EdgeKind::Calls)
        );

        let java = build(
            &[(
                "p/K.java",
                J,
                "package p;\nclass Other { void save() {} void close() {} }\nclass Repo { void save() {} void close() {} }\nclass K {\n  private Other repo;\n  void m(java.util.List<Repo> xs) {\n    for (Repo repo : xs) { repo.save(); }\n    xs.forEach(repo -> repo.save());\n    try (Repo repo = new Repo()) { repo.close(); } catch (RuntimeException repo) { repo.save(); }\n    if (xs instanceof Repo repo) { repo.save(); }\n    repo.save();\n  }\n}\n",
            )],
            &[],
        );
        let calls = edges(&java, EdgeKind::Calls);
        // The field `repo` (type Other) is reached only by the last call.
        assert_eq!(
            calls.iter().filter(|e| e.contains("Other#save")).count(),
            1,
            "{calls:?}"
        );
        assert_eq!(
            calls.iter().filter(|e| e.contains("Repo#save")).count(),
            1,
            "{calls:?}"
        );
        assert_eq!(
            calls.iter().filter(|e| e.contains("Repo#close")).count(),
            1,
            "{calls:?}"
        );
        assert!(
            !calls.iter().any(|e| e.contains("Other#close")),
            "{calls:?}"
        );
    }

    #[test]
    fn declarations_are_block_scoped_and_start_after_their_initialiser() {
        let go = build(
            &[(
                "p/h.go",
                GO,
                "package p\ntype Ctx struct{}\nfunc (c *Ctx) Copy() *Ctx { return c }\nfunc (c *Ctx) Next() {}\nfunc Handle(c *Ctx) {\n    go func() { c := c.Copy(); c.Next() }()\n    c.Next()\n}\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&go, EdgeKind::Calls),
            [
                "h:Handle(). -> h:Ctx#Copy(). [heuristic]",
                "h:Handle(). -> h:Ctx#Next(). [heuristic]"
            ]
        );
        let ts = build(
            &[(
                "src/f.ts",
                TS,
                "class Repo { save() {} }\nfunction f(repo: Repo) {\n  if (x) { const repo = 1 }\n  repo.save();\n  [1].forEach(() => { let repo = 2 });\n  repo.save();\n}\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&ts, EdgeKind::Calls),
            ["f:f(). -> f:Repo#save(). [heuristic]"]
        );
        let py = build(
            &[(
                "app/c.py",
                PY,
                "class Client:\n    def session(self):\n        pass\n\ndef f():\n    client = Client()\n    client = client.session()\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&py, EdgeKind::Calls),
            ["c:f(). -> c:Client#session(). [heuristic]"]
        );
    }

    #[test]
    fn a_declaration_does_not_reference_itself() {
        let built = build(
            &[(
                "p/t.go",
                GO,
                "package p\ntype Conn struct { next *Conn }\ntype Plain struct{}\n",
            )],
            &[],
        );
        // Only the genuine recursive field type references `Conn`.
        assert_eq!(
            edges(&built, EdgeKind::References),
            ["t:Conn# -> t:Conn# [heuristic]"]
        );
    }

    #[test]
    fn java_nested_class_imports_resolve() {
        let built = build(
            &[
                (
                    "x/Util.java",
                    J,
                    "package x;\npublic class Util { public static class Nested { public static void n() {} } }\n",
                ),
                (
                    "y/Use.java",
                    J,
                    "package y;\nimport x.Util.Nested;\nimport static x.Util.Nested.n;\nclass Use { void m() { Nested.n(); n(); } }\n",
                ),
            ],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            ["Use:Use#m(). -> Util:Util#Nested#n(). [heuristic]"]
        );
    }

    /// The resolution of `n` declarations of one name grows linearly: four times the sites
    /// examine at most five times the declarations (linear is four, a quadratic lookup
    /// sixteen). Counted, not timed, so host load cannot fail it (contract M3b amendment 67;
    /// counted since contract M4a amendment 12, Q4).
    #[test]
    fn many_declarations_of_one_name_stay_fast() {
        // Rebindings in one function; then closed sibling scopes, each with its own `x`, before
        // module-level calls whose `x` is the enclosing declaration (the walk must skip the
        // siblings through the parent links, not one by one).
        let resolve = |n: usize, siblings: bool| {
            let mut src = String::from("class A:\n    def f(self):\n        pass\n\n");
            if siblings {
                src.push_str("x = A()\n");
                for i in 0..n {
                    src.push_str(&format!("def g{i}():\n    x = A()\n    x.f()\n"));
                }
                for _ in 0..n {
                    src.push_str("x.f()\n");
                }
            } else {
                src.push_str("def run():\n");
                for _ in 0..n {
                    src.push_str("    x = A()\n    x.f()\n");
                }
            }
            // Parsing is not under test: count the resolution alone.
            let facts = rb_syntax::extract(PY, src.as_bytes()).unwrap();
            let by_path: BTreeMap<&str, (Language, &FileFacts)> =
                BTreeMap::from([("app/many.py", (PY, &facts))]);
            let keys =
                BTreeMap::from([("app/many.py", rb_syntax::symbol_keys("app/many.py", &facts))]);
            let mut b = GraphBuilder::new();
            b.add_node(
                NodeKey::new(NodeKind::File, "app/many.py"),
                "app/many.py",
                Attrs::new(),
            )
            .unwrap();
            crate::syntax::add_symbols(&mut b, "app/many.py", &facts, &keys["app/many.py"])
                .unwrap();
            rb_frameworks::work::take();
            let go = BTreeMap::new();
            let index = Index::new(&by_path, &go, &keys);
            add_edges(&mut b, &index, &BTreeSet::new()).unwrap();
            let work = rb_frameworks::work::take();
            let calls = b
                .finish()
                .unwrap()
                .edges()
                .iter()
                .filter(|e| e.kind == EdgeKind::Calls)
                .count();
            // Every `x.f()` site resolves to the one method, so the edges aggregate into one per
            // caller.
            assert_eq!(calls, if siblings { n + 1 } else { 1 });
            work
        };
        for (small, large) in [(5_000, 20_000), (1_000, 4_000)]
            .into_iter()
            .zip([false, true])
            .map(|((a, b), siblings)| (resolve(a, siblings), resolve(b, siblings)))
        {
            assert!(small > 0 && large <= 5 * small, "{small} → {large}");
        }
    }

    #[test]
    fn except_bodies_class_bodies_and_signatures_do_not_capture_names() {
        // C1: an assignment inside `except` is function-scoped.
        let py = build(
            &[(
                "a/e.py",
                PY,
                "class Req:\n    def m1(self):\n        pass\nclass Resp:\n    def m1(self):\n        pass\ndef f(r: Req):\n    try:\n        pass\n    except Exception:\n        r = Resp()\n    r.m1()\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&py, EdgeKind::Calls),
            ["e:f(). -> e:Resp#m1(). [heuristic]"]
        );
        // C2: a class-body name is skipped in favour of the enclosing declaration.
        let py = build(
            &[(
                "a/k.py",
                PY,
                "class Req:\n    def m2(self):\n        pass\nclass Resp:\n    def m2(self):\n        pass\nr = Req()\nclass K:\n    r = Resp()\n    def m(self):\n        r.m2()\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&py, EdgeKind::Calls),
            ["k:K#m(). -> k:Req#m2(). [heuristic]"]
        );
        // C3: interface, overload and function-type parameters end with their signature.
        let ts = build(
            &[(
                "src/s.ts",
                TS,
                "class Req { m1() {} m4() {} }\nclass Resp { m1() {} m4() {} }\nconst r: Req = mk();\ninterface H { do(r: Resp): void }\nfunction over(r: Resp): void;\nfunction over(r: any) {}\nconst w: Req = mk();\ntype Fn2 = (w: Resp) => void;\nfunction g() { r.m1(); w.m4(); }\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&ts, EdgeKind::Calls),
            [
                "s:g(). -> s:Req#m1(). [heuristic]",
                "s:g(). -> s:Req#m4(). [heuristic]"
            ]
        );
        let go = build(
            &[
                ("cfg/cfg.go", GO, "package cfg\nfunc Load() {}\n"),
                (
                    "p/t.go",
                    GO,
                    "package p\nimport c \"example.com/m/cfg\"\ntype Ctx struct{}\nfunc (x *Ctx) Load() {}\nvar cb func(c *Ctx)\ntype H interface { Do(c *Ctx) }\nfunc f() { c.Load() }\n",
                ),
            ],
            &[("", "example.com/m")],
        );
        assert_eq!(
            edges(&go, EdgeKind::Calls),
            ["t:f(). -> cfg:Load(). [heuristic]"]
        );
    }

    #[test]
    fn bare_calls_through_variables_are_unresolved() {
        for (path, language, src) in [
            (
                "a/fn.py",
                PY,
                "def fn():\n    pass\ndef apply(fn):\n    return fn()\ndef g():\n    h = 1\n    h()\n",
            ),
            (
                "src/fn.ts",
                TS,
                "function fn() {}\nfunction apply(fn: () => void) { fn(); }\n",
            ),
            (
                "p/fn.go",
                GO,
                "package p\nfunc fn() {}\nfunc apply(fn func()) { fn() }\n",
            ),
        ] {
            let built = build(&[(path, language, src)], &[]);
            assert!(
                edges(&built, EdgeKind::Calls).is_empty(),
                "{path}: {:?}",
                edges(&built, EdgeKind::Calls)
            );
            assert_eq!(unresolved(&built).len(), 1, "{path}");
        }
    }

    #[test]
    fn go_case_clauses_are_scopes() {
        let built = build(
            &[(
                "p/sw.go",
                GO,
                "package p\ntype Req struct{}\nfunc (r Req) M1() {}\nfunc (r Req) M2() {}\ntype Resp struct{}\nfunc (r Resp) M1() {}\nfunc (r Resp) M2() {}\nfunc f(r Req, x int) {\n\tswitch x {\n\tcase 1:\n\t\tr := Resp{}\n\t\tr.M1()\n\tcase 2:\n\t\tr.M2()\n\t}\n}\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&built, EdgeKind::Calls),
            [
                "sw:f(). -> sw:Req#M2(). [heuristic]",
                "sw:f(). -> sw:Resp#M1(). [heuristic]"
            ]
        );
    }

    #[test]
    fn function_valued_constants_java_locals_and_hoisted_vars() {
        // D1: a function held in a `const` is a definition; calls to it resolve.
        let js = build(
            &[(
                "src/h.js",
                Language::JavaScript,
                "const helper = () => 1;\nlet other = function () {};\nexport function run() { helper(); other(); }\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&js, EdgeKind::Calls),
            [
                "h:run(). -> h:helper(). [heuristic]",
                "h:run(). -> h:other(). [heuristic]"
            ]
        );
        // D2: Java methods are never shadowed by same-named variables.
        let java = build(
            &[(
                "p/Svc.java",
                J,
                "package p;\nclass Svc { int count() { return 0; } int size() { return 0; } void a() { int count = 0; count = count(); } void b(int size) { size(); } }\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&java, EdgeKind::Calls),
            [
                "Svc:Svc#a(). -> Svc:Svc#count(). [heuristic]",
                "Svc:Svc#b(). -> Svc:Svc#size(). [heuristic]"
            ]
        );
        // D3: a hoisted `var` hides the module function even before its declaration.
        let hoist = build(
            &[(
                "src/v.js",
                Language::JavaScript,
                "function g() {}\nfunction f() { g(); var g = 2; }\n",
            )],
            &[],
        );
        assert!(
            edges(&hoist, EdgeKind::Calls).is_empty(),
            "{:?}",
            edges(&hoist, EdgeKind::Calls)
        );
        // D5: decorator built-ins are not unresolved calls.
        let py = build(
            &[(
                "a/d.py",
                PY,
                "class K:\n    def h():\n        pass\n    h = staticmethod(h)\n",
            )],
            &[],
        );
        assert!(unresolved(&py).is_empty(), "{:?}", unresolved(&py));
    }

    #[test]
    fn functions_in_callbacks_and_unrecognised_forms_stay_variables() {
        // E1: a function-valued const inside a callback is not a symbol; calling it never falls
        // through to an import or a module function of the same name.
        let js = build(
            &[
                (
                    "src/lib.js",
                    Language::JavaScript,
                    "export function fmt() {}\nexport function fmtLocal() {}\n",
                ),
                (
                    "src/cb.js",
                    Language::JavaScript,
                    "import { fmt } from './lib.js';\nfunction build(items) { return items.map(x => { const fmt = () => x; return fmt(); }); }\ndescribe('x', () => { const fmt = () => 1; fmt(); });\nfunction fmtLocal() {}\nitems.forEach(function () { const fmtLocal = () => 1; fmtLocal(); });\n",
                ),
            ],
            &[],
        );
        assert!(
            edges(&js, EdgeKind::Calls).is_empty(),
            "{:?}",
            edges(&js, EdgeKind::Calls)
        );
        // E3: generator and class expressions are not captured as definitions; calls through
        // them are unresolved, never linked to same-named module symbols.
        let js = build(
            &[(
                "src/gc.js",
                Language::JavaScript,
                "function gen() {}\nclass K { static build() {} }\nfunction useGen() { const gen = function* () {}; gen(); }\nfunction useClass() { const K = class { static build() {} }; K.build(); }\n",
            )],
            &[],
        );
        assert!(
            edges(&js, EdgeKind::Calls).is_empty(),
            "{:?}",
            edges(&js, EdgeKind::Calls)
        );
        // Module-level function-valued constants are definitions and still resolve.
        let js = build(
            &[(
                "src/ok.js",
                Language::JavaScript,
                "const helper = () => 1;\nfunction run() { helper(); }\n",
            )],
            &[],
        );
        assert_eq!(
            edges(&js, EdgeKind::Calls),
            ["ok:run(). -> ok:helper(). [heuristic]"]
        );
    }

    #[test]
    fn var_hoisting_covers_loops_and_the_whole_file() {
        let js = build(
            &[(
                "src/v.js",
                Language::JavaScript,
                "function g() {}\nfunction pv() {}\nfunction first() { pv(); }\nfunction second() { pv(); }\nfunction s3() { for (var g of xs) {} g(); }\nvar pv = makePv();\n",
            )],
            &[],
        );
        // `pv` is hoisted over the whole file and `g` over `s3`: no call resolves.
        assert!(
            edges(&js, EdgeKind::Calls).is_empty(),
            "{:?}",
            edges(&js, EdgeKind::Calls)
        );
    }

    #[test]
    fn a_function_valued_variable_resolves_to_its_own_definition() {
        // An object property named like the variable is not the variable.
        let js = build(
            &[(
                "src/o.js",
                Language::JavaScript,
                "const run = { run: () => 1 };\nfunction callsRun() { run(); }\n",
            )],
            &[],
        );
        assert!(
            edges(&js, EdgeKind::Calls).is_empty(),
            "{:?}",
            edges(&js, EdgeKind::Calls)
        );
        // Each call site goes to the `h` in effect there, not to every `h` in the function.
        let js = build(
            &[(
                "src/n.js",
                Language::JavaScript,
                "function g() { const h = () => 1; if (c) { const h = () => 2; return h(); } return h(); }\n",
            )],
            &[],
        );
        let calls = edges(&js, EdgeKind::Calls);
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(
            calls.iter().all(|e| e.ends_with("[heuristic]")),
            "{calls:?}"
        );
        assert_ne!(calls[0], calls[1]);
    }
}
