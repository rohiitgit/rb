//! Tier B resolution (design §5.4): `CALLS`, `REFERENCES` and `IMPLEMENTS` edges from Tree-sitter
//! facts alone, with provenance `heuristic`.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};

use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance,
};
use rb_syntax::{Binding, DefKind, Definition, FileFacts, Language, RefKind};
use serde_json::json;

use crate::modules::{Modules, dir};

/// A definition: its file and its index in that file's `FileFacts::definitions`.
type Sym<'a> = (&'a str, usize);

/// Inheritance levels searched for a member; deeper hierarchies stay unresolved.
const MAX_INHERITANCE_DEPTH: usize = 64;

/// Adds the resolved edges for every file; returns one `unresolved_references` diagnostic per file
/// that has unresolved call or type references.
pub(crate) fn add_edges(
    b: &mut GraphBuilder,
    files: &BTreeMap<&str, (Language, &FileFacts)>,
    go_modules: &BTreeMap<String, String>,
) -> anyhow::Result<Vec<Diagnostic>> {
    let index = Index::new(files, go_modules);
    let mut edges = Edges::default();
    let mut diagnostics = Vec::new();
    for (&path, (language, facts)) in files {
        let keys = &index.keys[path];
        let source = |enclosing: Option<usize>| match enclosing {
            Some(i) => NodeKey::new(NodeKind::Symbol, keys[i].clone()),
            None => NodeKey::new(NodeKind::File, path),
        };
        let (mut calls, mut types, mut external) = (0u64, 0u64, 0u64);
        for r in &facts.references {
            let at = Evidence {
                path: path.to_owned(),
                span: r.span,
            };
            let (found, is_call) = match r.kind {
                RefKind::Call => (
                    index.call(path, r.enclosing, &r.name, r.qualifier.as_deref()),
                    true,
                ),
                RefKind::Class | RefKind::Type => {
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
                } else if index.is_external(path, r.enclosing, head) {
                    external += 1;
                } else if is_call {
                    calls += 1;
                } else {
                    types += 1;
                }
                continue;
            }
            edges.record_all(source(r.enclosing), &found, &index, &at);
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
enum Container<'a> {
    Types(Vec<Sym<'a>>),
    Module(Vec<&'a str>),
}

struct Index<'a> {
    files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
    modules: Modules<'a>,
    keys: BTreeMap<&'a str, Vec<String>>,
    /// Per file: definitions by name, directly inside each definition and at top level.
    scopes: BTreeMap<&'a str, Scopes<'a>>,
    /// Per file: the files each import refers to, parallel to `FileFacts::imports`.
    imports: BTreeMap<&'a str, Vec<Vec<&'a str>>>,
    /// Resolved direct supertypes of a type (memoised; resolving them never needs members).
    supers: RefCell<BTreeMap<Sym<'a>, Vec<Sym<'a>>>>,
}

#[derive(Default)]
struct Scopes<'a> {
    top: BTreeMap<&'a str, Vec<usize>>,
    of: BTreeMap<usize, BTreeMap<&'a str, Vec<usize>>>,
}

fn is_type(kind: DefKind) -> bool {
    matches!(kind, DefKind::Class | DefKind::Interface | DefKind::Type)
}

fn is_callable(kind: DefKind) -> bool {
    matches!(kind, DefKind::Function | DefKind::Method | DefKind::Class)
}

impl<'a> Index<'a> {
    fn new(
        files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
        go_modules: &'a BTreeMap<String, String>,
    ) -> Self {
        let modules = Modules::new(files, go_modules);
        let mut keys = BTreeMap::new();
        let mut scopes = BTreeMap::new();
        let mut imports = BTreeMap::new();
        for (&path, (_, facts)) in files {
            keys.insert(path, rb_syntax::symbol_keys(path, facts));
            let mut sc = Scopes::default();
            for (i, d) in facts.definitions.iter().enumerate() {
                let table = match d.parent {
                    Some(p) => sc.of.entry(p).or_default(),
                    None => &mut sc.top,
                };
                table.entry(d.name.as_str()).or_default().push(i);
            }
            scopes.insert(path, sc);
            imports.insert(
                path,
                facts
                    .imports
                    .iter()
                    .map(|i| modules.resolve(path, i))
                    .collect(),
            );
        }
        Index {
            files,
            modules,
            keys,
            scopes,
            imports,
            supers: RefCell::new(BTreeMap::new()),
        }
    }

    fn language(&self, path: &str) -> Language {
        self.files[path].0
    }

    fn facts(&self, path: &str) -> &'a FileFacts {
        self.files[path].1
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
        if let Some((head, last)) = name.rsplit_once('.') {
            let found = match self.container(path, scope, head) {
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
        self.levels(path, scope, name, is_type, false)
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
    /// bodies are scopes only in Java, and there through inheritance.
    fn levels(
        &self,
        path: &'a str,
        scope: Option<usize>,
        name: &str,
        want: fn(DefKind) -> bool,
        callable: bool,
    ) -> Vec<Sym<'a>> {
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
                return found;
            }
        }
        let found = self.named(path, None, name, want);
        if !found.is_empty() {
            return found;
        }
        let found = self.imported(path, name, want);
        if !found.is_empty() {
            return found;
        }
        self.same_package(path, name, want)
    }

    /// Definitions `name` refers to through the file's imports.
    fn imported(&self, path: &'a str, name: &str, want: fn(DefKind) -> bool) -> Vec<Sym<'a>> {
        let language = self.language(path);
        let mut out = Vec::new();
        for (import, targets) in self.facts(path).imports.iter().zip(&self.imports[path]) {
            let top = |n: &str| -> Vec<Sym<'a>> {
                targets
                    .iter()
                    .flat_map(|&f| self.named(f, None, n, want))
                    .collect()
            };
            match language {
                Language::Java if import.is_static => {
                    let class = import.module.rsplit('.').next().unwrap_or_default();
                    let named = import.wildcard || import.names.iter().any(|n| n.local == name);
                    if named {
                        for &f in targets {
                            for c in self.named(f, None, class, is_type) {
                                out.extend(self.named(c.0, Some(c.1), name, want));
                            }
                        }
                    }
                }
                Language::Java if import.wildcard => out.extend(top(name)),
                Language::Go if import.alias.as_deref() == Some(".") => out.extend(top(name)),
                _ => {
                    for n in import
                        .names
                        .iter()
                        .filter(|n| n.local == name && n.name != "default")
                    {
                        out.extend(top(&n.name));
                    }
                }
            }
        }
        out
    }

    fn same_package(&self, path: &'a str, name: &str, want: fn(DefKind) -> bool) -> Vec<Sym<'a>> {
        let files = match self.language(path) {
            Language::Java => self
                .modules
                .package_files(self.facts(path).package.as_deref().unwrap_or_default()),
            Language::Go => self.modules.go_dir(dir(path)),
            _ => Vec::new(),
        };
        files
            .into_iter()
            .filter(|&f| f != path)
            .flat_map(|f| self.named(f, None, name, want))
            .collect()
    }

    /// Members named `name` of `t`, else of its nearest supertypes that have one (breadth-first,
    /// each type once, at most [`MAX_INHERITANCE_DEPTH`] levels).
    fn members(&self, t: Sym<'a>, name: &str, want: fn(DefKind) -> bool) -> Vec<Sym<'a>> {
        let mut seen = BTreeSet::from([t]);
        let mut level = vec![t];
        for _ in 0..MAX_INHERITANCE_DEPTH {
            let found: Vec<Sym<'a>> = level
                .iter()
                .flat_map(|&c| self.named(c.0, Some(c.1), name, want))
                .collect();
            if !found.is_empty() {
                return found;
            }
            level = level
                .iter()
                .flat_map(|&c| self.supertypes(c))
                .filter(|s| seen.insert(*s))
                .collect();
            if level.is_empty() {
                break;
            }
        }
        Vec::new()
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

    /// A binding named `name` visible at `scope`: the innermost local, else (Java) a field of an
    /// enclosing class.
    fn binding(&self, path: &'a str, scope: Option<usize>, name: &str) -> Option<&'a Binding> {
        let facts = self.facts(path);
        let chain = self.ancestors(path, scope);
        let local = facts
            .bindings
            .iter()
            .filter(|b| !b.member && b.name == name)
            .filter_map(|b| match b.scope {
                None => Some((usize::MAX, b)),
                Some(s) => chain.iter().position(|&a| a == s).map(|depth| (depth, b)),
            })
            .min_by_key(|(depth, _)| *depth)
            .map(|(_, b)| b);
        if local.is_some() || self.language(path) != Language::Java {
            return local;
        }
        chain.iter().find_map(|&a| {
            facts
                .bindings
                .iter()
                .find(|b| b.member && b.scope == Some(a) && b.name == name)
        })
    }

    /// Whether the unresolved name `head` (a bare name, or the first segment of a qualifier)
    /// comes from an external package: an import that resolves to no file of the snapshot, directly
    /// or as the declared type of a binding.
    fn is_external(&self, path: &'a str, scope: Option<usize>, head: &str) -> bool {
        let head = match self.binding(path, scope, head) {
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
    fn container(&self, path: &'a str, scope: Option<usize>, q: &str) -> Option<Container<'a>> {
        let segments: Vec<&str> = q.split('.').collect();
        let (mut target, consumed) = self.first_segment(path, scope, &segments)?;
        for seg in &segments[consumed..] {
            target = self.step(target, seg)?;
        }
        Some(target)
    }

    fn first_segment(
        &self,
        path: &'a str,
        scope: Option<usize>,
        segments: &[&str],
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
        if let Some(b) = self.binding(path, scope, s0) {
            let types = self.resolve_type(path, b.scope, &b.type_name);
            if !types.is_empty() {
                return Some((Container::Types(types), 1));
            }
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
                let types: Vec<Sym<'a>> = targets
                    .iter()
                    .flat_map(|&f| self.named(f, None, &n.name, is_type))
                    .collect();
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
                    let fields = self
                        .facts(t.0)
                        .bindings
                        .iter()
                        .filter(|b| b.member && b.scope == Some(t.1) && b.name == seg)
                        .flat_map(|b| self.resolve_type(t.0, Some(t.1), &b.type_name));
                    fields
                        .chain(self.named(t.0, Some(t.1), seg, is_type))
                        .collect::<Vec<_>>()
                })
                .collect(),
            Container::Module(files) => files
                .into_iter()
                .flat_map(|f| self.named(f, None, seg, is_type))
                .collect(),
        };
        (!found.is_empty()).then_some(Container::Types(found))
    }

    /// Candidates of a call reference, each with the edge it gives.
    fn call(
        &self,
        path: &'a str,
        scope: Option<usize>,
        name: &str,
        qualifier: Option<&str>,
    ) -> Vec<(EdgeKind, Sym<'a>)> {
        let found = match qualifier {
            None => self.levels(path, scope, name, is_callable, true),
            Some(q) => match self.container(path, scope, q) {
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
            Language::Go => return None,
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
            if dst == src {
                continue;
            }
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
        let mut b = GraphBuilder::new();
        for (path, (_, f)) in &by_path {
            b.add_node(NodeKey::new(NodeKind::File, *path), *path, Attrs::new())
                .unwrap();
            crate::syntax::add_symbols(&mut b, path, f).unwrap();
        }
        let diagnostics = add_edges(&mut b, &by_path, &go).unwrap();
        Built {
            graph: b.finish().unwrap(),
            diagnostics,
        }
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
}
