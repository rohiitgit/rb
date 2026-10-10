//! RTA as a proof of absence (contract M4b S9, D4): a Java or TS/JS type is proven never
//! instantiated only when every mention of its name, in every file, is in a role that cannot
//! create one, and every type naming it as a supertype is proven too. Its `IMPLEMENTS` edges
//! (and those of the methods it contains) then carry `impl_instantiated: false`; nothing else
//! changes. Every rule that fails keeps the type, and the first one is counted.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rb_frameworks::clients::RawScan;
use rb_frameworks::work;
use rb_model::{Diagnostic, EdgeKind, GraphBuilder, NodeKey, NodeKind};
use rb_syntax::{DefKind, FileFacts, Language, MentionRole};
use serde_json::{Value, json};

use crate::di::is_test_path;
use crate::liveness::Words;

type Sym<'a> = (&'a str, usize);

/// The first failing rule of a kept type (S9), in the order they are checked.
pub(crate) const RULES: &[&str] = &[
    "language",
    "test",
    "library",
    "surface",
    "mention",
    "string",
    "annotation",
    "default_export",
    "fragment",
    "subtype",
];

/// What the proof reads.
pub(crate) struct Inputs<'a> {
    pub files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
    pub service_of: &'a dyn Fn(&str) -> String,
    /// Deployable services (`role = service`).
    pub deployable: &'a BTreeSet<String>,
    pub unparsed: &'a RawScan,
    /// The whole-word occurrences of the definitions' names in every non-binary file (S9 (d),
    /// evaluator pass 1, Y14).
    pub words: &'a Words,
    /// The interfaces Spring Data repositories extend, by simple name (fragments, D3).
    pub fragments: &'a BTreeSet<String>,
    /// Exact manifest resolution for module objects (contract M6c S4).
    pub aliases: &'a AliasMaps,
}

/// The proof over a snapshot: the types considered, each with its first failing rule or
/// `None` when proven.
#[derive(Debug, Default)]
pub(crate) struct Proof<'a> {
    pub types: BTreeMap<Sym<'a>, Option<&'static str>>,
}

fn simple(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// The roles that cannot create an instance (S9 (c)): a declaration (of the type or a
/// same-named one, or of its constructors), an import, a supertype head.
const ALLOWED: &[MentionRole] = &[
    MentionRole::Declaration,
    MentionRole::Import,
    MentionRole::Supertype,
];

fn is_type(kind: DefKind) -> bool {
    matches!(kind, DefKind::Class | DefKind::Interface)
}

fn parsed_by_rta(language: Language) -> bool {
    matches!(
        language,
        Language::Java | Language::TypeScript | Language::Tsx | Language::JavaScript
    )
}

/// Proves or keeps each type in `considered` (S9 (a)–(i)).
pub(crate) fn prove<'a>(inputs: &Inputs<'a>, considered: &BTreeSet<Sym<'a>>) -> Proof<'a> {
    // Every mention role of every name in the parsed Java/TS/JS files.
    let mut roles: BTreeMap<&str, BTreeSet<MentionRole>> = BTreeMap::new();
    // Static imports name a class whose members may be used unqualified (enum constants).
    let mut static_imported: BTreeSet<&str> = BTreeSet::new();
    // (g): an unnamed-instantiation surface anywhere.
    let mut surface = inputs
        .unparsed
        .di_surface
        .iter()
        .any(|(_, what)| what == "beans.xml");
    for (language, facts) in inputs.files.values() {
        if !parsed_by_rta(*language) {
            continue;
        }
        for (name, r) in &facts.mentions {
            work::tick(1);
            roles.entry(name.as_str()).or_default().extend(r);
        }
        for i in &facts.imports {
            if i.is_static {
                static_imported.insert(simple(&i.module));
            }
            if i.module.starts_with("org.reflections")
                || i.module.starts_with("io.github.classgraph")
            {
                surface = true;
            }
        }
        surface |= facts
            .mentions
            .contains_key("ClassPathScanningCandidateComponentProvider")
            || facts.definitions.iter().any(|d| {
                d.decorations.iter().any(|x| {
                    x.args
                        .iter()
                        .any(|a| a.key.as_deref() == Some("includeFilters"))
                })
            });
    }
    // (d): the name in a file without mention facts (an unparsed file, a Python or Go file,
    // prose, a lock file), or anywhere when the word scan cannot tell.
    let unparsed_word = |name: &str| {
        inputs.unparsed.words_unread
            || inputs.unparsed.words.contains(name)
            || inputs.words.paths_of(name).is_none_or(|paths| {
                paths.iter().any(|p| {
                    inputs
                        .files
                        .get(p)
                        .is_none_or(|(language, _)| !parsed_by_rta(*language))
                })
            })
    };
    // Module objects taken anywhere (Y7): `(importing file, specifier)`.
    let module_objects: Vec<(&str, &str)> = inputs
        .files
        .iter()
        .filter(|(_, (language, _))| parsed_by_rta(*language))
        .flat_map(|(&path, (_, facts))| {
            facts.module_objects.iter().map(move |m| (path, m.as_str()))
        })
        .collect();

    let mut types: BTreeMap<Sym<'a>, Option<&'static str>> = BTreeMap::new();
    for &t in considered {
        work::tick(1);
        types.insert(
            t,
            own_rule(
                inputs,
                t,
                &Seen {
                    roles: &roles,
                    static_imported: &static_imported,
                    module_objects: &module_objects,
                    surface,
                    unparsed_word: &unparsed_word,
                },
            ),
        );
    }

    // (i): "instantiated" flows from a type to the types it names as supertypes, by simple
    // name, over every type of every parsed file; a breadth-first walk with a visited set, so a
    // cycle terminates and a chain of any length is kept whole.
    let mut by_name: BTreeMap<&str, Vec<Sym<'a>>> = BTreeMap::new();
    let mut supers: BTreeMap<Sym<'a>, Vec<&str>> = BTreeMap::new();
    for (&path, (_, facts)) in inputs.files {
        for (i, d) in facts.definitions.iter().enumerate() {
            if !is_type(d.kind) {
                continue;
            }
            work::tick(1);
            by_name.entry(d.name.as_str()).or_default().push((path, i));
            supers.insert(
                (path, i),
                d.supertypes.iter().map(|s| simple(&s.name)).collect(),
            );
        }
    }
    // Every type is instantiated unless considered and proven by its own rules.
    let mut queue: VecDeque<Sym<'a>> = supers
        .keys()
        .copied()
        .filter(|t| types.get(t).is_none_or(Option::is_some))
        .collect();
    let mut seen: BTreeSet<Sym<'a>> = queue.iter().copied().collect();
    while let Some(u) = queue.pop_front() {
        for name in supers.get(&u).into_iter().flatten() {
            for &t in by_name.get(name).into_iter().flatten() {
                work::tick(1);
                if seen.insert(t) {
                    if let Some(rule @ None) = types.get_mut(&t) {
                        *rule = Some("subtype");
                    }
                    queue.push_back(t);
                }
            }
        }
    }
    Proof { types }
}

/// Exact manifest resolution (contract M6c S4): `tsconfig.json` `paths`
/// aliases and `package.json` `exports`/`main` subpaths. Anything
/// unresolvable — a missing file, a multi-star or computed pattern, an
/// unknown package, `extends` chains, non-string targets — resolves to
/// nothing, and callers keep every type, as before.
#[derive(Debug, Default, Clone)]
pub(crate) struct AliasMaps {
    /// `(tsconfig dir, base dir, [(pattern, [targets])])`, longest dir first.
    tsconfigs: Vec<TsconfigEntry>,
    /// package name → `(dir, exports entries, main)`, longest dir first.
    packages: Vec<PackageEntry>,
}

/// One `tsconfig.json` `paths` scope: its dir, base dir and pattern targets.
type TsconfigEntry = (String, String, Vec<(String, Vec<String>)>);
/// One named package: name, dir, exports entries and `main` fallback.
type PackageEntry = (String, String, Vec<(String, String)>, Option<String>);

impl AliasMaps {
    /// Builds the maps from manifest texts by path (`tsconfig.json` and
    /// `package.json` entries; everything else ignored). Never fails: an
    /// unreadable manifest contributes nothing.
    pub fn build(manifests: &BTreeMap<&str, String>) -> Self {
        let mut tsconfigs = Vec::new();
        let mut packages = Vec::new();
        for (path, text) in manifests {
            let name = path.rsplit('/').next().unwrap_or(path);
            let dir = match path.rfind('/') {
                Some(i) => path[..i].to_owned(),
                None => String::new(),
            };
            let Ok(json) = serde_json::from_str::<serde_json::Value>(text) else {
                continue;
            };
            if name == "tsconfig.json" {
                let options = json.get("compilerOptions");
                let base = options
                    .and_then(|o| o.get("baseUrl"))
                    .and_then(|b| b.as_str())
                    .unwrap_or_default();
                let base = if base.is_empty() {
                    dir.clone()
                } else {
                    join_dir(&dir, base)
                };
                let mut paths = Vec::new();
                if let Some(map) = options
                    .and_then(|o| o.get("paths"))
                    .and_then(|p| p.as_object())
                {
                    for (pattern, targets) in map {
                        if pattern.matches('*').count() > 1 {
                            continue;
                        }
                        let targets: Vec<String> = targets
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|t| t.as_str())
                                    .map(|t| join_dir(&base, t))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if !targets.is_empty() {
                            paths.push((pattern.clone(), targets));
                        }
                    }
                }
                tsconfigs.push((dir, base, paths));
            } else if name == "package.json" {
                let pkg_name = json
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or_default()
                    .to_owned();
                if pkg_name.is_empty() {
                    continue;
                }
                let mut exports = Vec::new();
                if let Some(e) = json.get("exports") {
                    if let Some(s) = e.as_str() {
                        exports.push((".".to_owned(), join_dir(&dir, s)));
                    } else if let Some(map) = e.as_object() {
                        for (sub, target) in map {
                            let Some(target) = Self::export_target(target) else {
                                continue;
                            };
                            exports.push((sub.clone(), join_dir(&dir, &target)));
                        }
                    }
                }
                let main = json
                    .get("main")
                    .and_then(|m| m.as_str())
                    .map(|m| join_dir(&dir, m));
                packages.push((pkg_name, dir, exports, main));
            }
        }
        tsconfigs.sort_by_key(|a: &TsconfigEntry| std::cmp::Reverse(a.0.len()));
        packages.sort_by_key(|a: &PackageEntry| std::cmp::Reverse(a.1.len()));
        AliasMaps {
            tsconfigs,
            packages,
        }
    }

    /// A string export target: a plain string, or the `default` (else first
    /// string-valued) condition of a conditions object.
    fn export_target(target: &serde_json::Value) -> Option<String> {
        if let Some(s) = target.as_str() {
            return Some(s.to_owned());
        }
        target.as_object().and_then(|map| {
            map.get("default")
                .and_then(|d| d.as_str())
                .map(str::to_owned)
                .or_else(|| {
                    map.values()
                        .filter_map(|v| v.as_str())
                        .map(str::to_owned)
                        .next()
                })
        })
    }

    /// Resolves `spec` written in `from` to a repo-relative path, or `None`
    /// when no manifest resolves it exactly. Relative specifiers and `*` are
    /// not manifest business (the existing rules handle them). Precedence
    /// follows the tools: an exact key beats any pattern, and of the matching
    /// patterns the longest wins (TypeScript's longest-prefix rule, Node's
    /// `PATTERN_KEY_COMPARE`); a pattern with several targets is ambiguous, so
    /// it resolves nothing and the caller keeps every type (contract M6c D4).
    pub fn resolve(&self, from: &str, spec: &str) -> Option<String> {
        if spec.starts_with('.') || spec == "*" || spec.contains("://") {
            return None;
        }
        // `tsconfig.json` `paths`: nearest scope wins.
        if let Some((_, _, paths)) = self.tsconfigs.iter().find(|(dir, _, _)| {
            dir.is_empty() || from == dir.as_str() || from.starts_with(&format!("{dir}/"))
        }) {
            let mut best: Option<(usize, &str, String)> = None;
            for (pattern, targets) in paths {
                work::tick(1);
                let [target] = targets.as_slice() else {
                    continue;
                };
                if let Some(rest) = match_pattern(pattern, spec)
                    && best.as_ref().is_none_or(|(len, _, _)| pattern.len() > *len)
                {
                    best = Some((pattern.len(), target.as_str(), rest));
                }
            }
            if let Some((_, target, rest)) = best {
                return Some(substitute(target, &rest));
            }
        }
        // `package.json` `exports`/`main`: nearest package wins.
        let (name, sub) = split_package(spec)?;
        let (_, _dir, exports, main) = self.packages.iter().find(|(n, d, _, _)| {
            n == &name && (d.is_empty() || from == d.as_str() || from.starts_with(&format!("{d}/")))
        })?;
        if !exports.is_empty() {
            for (key, target) in exports {
                work::tick(1);
                if key.as_str() == sub {
                    return Some(target.clone());
                }
            }
            let mut best: Option<(usize, &str, String)> = None;
            for (key, target) in exports {
                work::tick(1);
                if let Some(rest) = match_subpath(key, &sub)
                    && best.as_ref().is_none_or(|(len, _, _)| key.len() > *len)
                {
                    best = Some((key.len(), target, rest));
                }
            }
            return best.map(|(_, target, rest)| substitute(target, &rest));
        }
        if sub == "." {
            return main.clone();
        }
        None
    }
}

/// Joins a manifest-relative path onto a repo-relative dir.
fn join_dir(dir: &str, path: &str) -> String {
    let path = path.trim_start_matches("./");
    if dir.is_empty() {
        path.trim_end_matches('/').to_owned()
    } else {
        format!(
            "{}/{}",
            dir.trim_end_matches('/'),
            path.trim_end_matches('/')
        )
    }
}

/// Splits a pattern with at most one `*` against text: the matched middle, or
/// `Some("")` for an exact key.
fn match_pattern(pattern: &str, text: &str) -> Option<String> {
    match pattern.split_once('*') {
        None => (pattern == text).then(String::new),
        Some((pre, post)) => text
            .strip_prefix(pre)
            .and_then(|t| t.strip_suffix(post))
            .map(str::to_owned),
    }
}

/// Matches an exports subpath key (`./x`, `./x/*`) against a `./`-less sub
/// (`./x`, `./x/y`): the matched middle for patterns.
fn match_subpath(key: &str, sub: &str) -> Option<String> {
    let key = key.strip_prefix("./").unwrap_or(key);
    let sub = sub.strip_prefix("./").unwrap_or(sub);
    match_pattern(key, sub)
}

/// Substitutes a single-`*` match into a target, or returns a `*`-less target
/// unchanged (exact keys carry an empty match).
fn substitute(target: &str, rest: &str) -> String {
    match target.split_once('*') {
        None => target.to_owned(),
        Some((pre, post)) => format!("{pre}{rest}{post}"),
    }
}

/// Splits a bare specifier into its package name and `./`-less subpath
/// (`@scope/pkg/sub` → (`@scope/pkg`, `./sub`); `pkg` → (`pkg`, `.`)).
fn split_package(spec: &str) -> Option<(String, String)> {
    let mut parts = spec.split('/');
    let first = parts.next()?;
    if first.is_empty() {
        return None;
    }
    let name = if let Some(scope) = first.strip_prefix('@') {
        format!("@{scope}/{}", parts.next()?)
    } else {
        first.to_owned()
    };
    let rest: Vec<&str> = parts.collect();
    let sub = if rest.is_empty() {
        ".".to_owned()
    } else {
        format!("./{}", rest.join("/"))
    };
    Some((name, sub))
}

/// Whether the module specifier `spec`, written in `from`, may name the file `path` (Y7,
/// narrowed by S4): an exactly manifest-resolved specifier names only its file; a specifier
/// resolved to another real file names that file, never this one; anything unresolvable falls
/// through to the rules below. A relative specifier is resolved as Node and TypeScript do,
/// extensions and `index` included; any other (a package, a path alias) may name a file whose
/// stem or any directory is its last segment; `*` (a computed `require`/`import()`) may name
/// any file.
fn module_may_name(
    from: &str,
    spec: &str,
    path: &str,
    aliases: &AliasMaps,
    files: &BTreeMap<&str, (Language, &FileFacts)>,
) -> bool {
    const EXTENSIONS: &[&str] = &[
        ".d.ts", ".ts", ".tsx", ".mts", ".cts", ".js", ".jsx", ".mjs", ".cjs",
    ];
    let strip = |p: &str| -> String {
        let p = p.trim_end_matches('/');
        let p = EXTENSIONS
            .iter()
            .find_map(|e| p.strip_suffix(e))
            .unwrap_or(p);
        p.strip_suffix("/index").unwrap_or(p).to_owned()
    };
    if spec == "*" {
        return true;
    }
    let target = strip(path);
    if spec == "." || spec == ".." || spec.starts_with("./") || spec.starts_with("../") {
        let mut parts: Vec<&str> = from.split('/').collect();
        parts.pop();
        for seg in spec.split('/') {
            match seg {
                "" | "." => {}
                ".." => {
                    if parts.pop().is_none() {
                        return false;
                    }
                }
                s => parts.push(s),
            }
        }
        return strip(&parts.join("/")) == target;
    }
    // Exact manifest resolution first (contract M6c S4): a resolved specifier
    // names only its file; resolved to another real file it names that file,
    // never this one; resolved nowhere it falls through to the approximate
    // rule below (like an unresolvable import: keep-all).
    if let Some(resolved) = aliases.resolve(from, spec) {
        let stated = strip(&resolved);
        if stated == target {
            return true;
        }
        let mut elsewhere = false;
        for p in files.keys() {
            work::tick(1);
            if strip(p) == stated {
                elsewhere = true;
                break;
            }
        }
        if elsewhere {
            return false;
        }
    }
    let spec = strip(spec);
    let last = spec.rsplit('/').next().unwrap_or(&spec);
    !last.is_empty() && target.split('/').any(|segment| segment == last)
}

/// What the repository-wide scans found, for [`own_rule`].
struct Seen<'s> {
    roles: &'s BTreeMap<&'s str, BTreeSet<MentionRole>>,
    static_imported: &'s BTreeSet<&'s str>,
    /// `(importing file, specifier)` of every module object taken (Y7).
    module_objects: &'s [(&'s str, &'s str)],
    surface: bool,
    unparsed_word: &'s dyn Fn(&str) -> bool,
}

/// The first of rules (a)–(h) that keeps `t`, or `None`.
fn own_rule(inputs: &Inputs<'_>, (path, i): Sym<'_>, seen: &Seen<'_>) -> Option<&'static str> {
    let Seen {
        roles,
        static_imported,
        module_objects,
        surface,
        unparsed_word,
    } = seen;
    let (language, facts) = inputs.files[path];
    let def = &facts.definitions[i];
    if !parsed_by_rta(language) {
        return Some("language");
    }
    if is_test_path(path) {
        return Some("test");
    }
    if !inputs.deployable.contains(&(inputs.service_of)(path)) {
        return Some("library");
    }
    if *surface {
        return Some("surface");
    }
    let name = def.name.as_str();
    let mentioned = roles.get(name);
    if mentioned.is_some_and(|r| {
        r.iter()
            .any(|x| *x != MentionRole::String && !ALLOWED.contains(x))
    }) || static_imported.contains(name)
        // A TS/JS module whose whole object is taken: its exports are used under no name
        // (`Object.values(ns)`), so every type it declares counts as mentioned (Y7).
        || (language != Language::Java
            && module_objects
                .iter()
                .any(|(from, spec)| module_may_name(from, spec, path, inputs.aliases, inputs.files)))
    {
        return Some("mention");
    }
    if mentioned.is_some_and(|r| r.contains(&MentionRole::String)) || unparsed_word(name) {
        return Some("string");
    }
    let inert = |d: &rb_syntax::Decoration| rb_frameworks::di::is_inert(facts, d);
    let members_decorated = facts
        .definitions
        .iter()
        .filter(|m| m.parent == Some(i))
        .flat_map(|m| &m.decorations)
        .chain(
            facts
                .bindings
                .iter()
                .filter(|b| b.member && b.scope == Some(i))
                .flat_map(|b| &b.decorations),
        )
        .any(|d| !inert(d));
    if def.decorations.iter().any(|d| !inert(d)) || members_decorated {
        return Some("annotation");
    }
    if def.parent.is_none() && facts.default_export.as_deref() == Some(name) {
        return Some("default_export");
    }
    if def
        .supertypes
        .iter()
        .any(|s| inputs.fragments.contains(simple(&s.name)))
    {
        return Some("fragment");
    }
    None
}

/// The types an `IMPLEMENTS` source key stands for: the type itself, or the type containing a
/// method.
pub(crate) fn implementing_types<'a>(
    sources: &BTreeSet<&NodeKey>,
    files: &BTreeMap<&'a str, (Language, &'a FileFacts)>,
    keys: &BTreeMap<&'a str, Vec<String>>,
) -> BTreeSet<Sym<'a>> {
    let mut by_key: BTreeMap<&str, Sym<'a>> = BTreeMap::new();
    for (&path, file_keys) in keys {
        for (i, k) in file_keys.iter().enumerate() {
            by_key.entry(k.as_str()).or_insert((path, i));
        }
    }
    sources
        .iter()
        .filter(|k| k.kind == NodeKind::Symbol)
        .filter_map(|k| by_key.get(k.key.as_str()).copied())
        .filter_map(|(path, i)| {
            let defs = &files.get(path)?.1.definitions;
            if is_type(defs[i].kind) {
                Some((path, i))
            } else {
                defs[i]
                    .parent
                    .filter(|&p| is_type(defs[p].kind))
                    .map(|p| (path, p))
            }
        })
        .collect()
}

/// Marks the `IMPLEMENTS` edges of every proven type and of the methods it contains with
/// `impl_instantiated: false`; returns the `rta_coverage` diagnostic.
pub(crate) fn apply(
    b: &mut GraphBuilder,
    proof: &Proof<'_>,
    files: &BTreeMap<&str, (Language, &FileFacts)>,
    keys: &BTreeMap<&str, Vec<String>>,
) -> Diagnostic {
    let mut sources: BTreeSet<NodeKey> = BTreeSet::new();
    for (&(path, t), rule) in &proof.types {
        if rule.is_some() {
            continue;
        }
        let defs = &files[path].1.definitions;
        sources.insert(NodeKey::new(NodeKind::Symbol, keys[path][t].clone()));
        for (m, d) in defs.iter().enumerate() {
            if d.parent == Some(t) && matches!(d.kind, DefKind::Method | DefKind::Function) {
                sources.insert(NodeKey::new(NodeKind::Symbol, keys[path][m].clone()));
            }
        }
    }
    let prunable = b.set_edge_attr(
        EdgeKind::Implements,
        &sources,
        "impl_instantiated",
        Value::Bool(false),
    );
    let mut kept: BTreeMap<&str, usize> = RULES.iter().map(|r| (*r, 0)).collect();
    for rule in proof.types.values().flatten() {
        *kept.get_mut(rule).expect("a listed rule") += 1;
    }
    let proven = proof.types.values().filter(|r| r.is_none()).count();
    Diagnostic {
        kind: "rta_coverage".into(),
        path: None,
        span: None,
        detail: json!({
            "types": proof.types.len(),
            "proven": proven,
            "kept": kept,
            "prunable_edges": prunable,
        }),
    }
}

#[cfg(test)]
mod tests {
    use rb_model::{Attrs, Graph};

    use super::*;
    use crate::resolve::Index;

    struct Run {
        /// `Type: rule` (or `proven`) per considered type, by name.
        types: Vec<String>,
        graph: Graph,
        coverage: Value,
    }

    fn language_of(path: &str) -> Language {
        match path.rsplit('.').next() {
            Some("java") => Language::Java,
            Some("ts") => Language::TypeScript,
            Some("js") => Language::JavaScript,
            Some("py") => Language::Python,
            Some("go") => Language::Go,
            other => panic!("{other:?}"),
        }
    }

    /// Indexes `files` (paths under `svc/` are a deployable service, under `lib/` a library),
    /// with `unparsed` as the raw scan of the files without facts.
    fn run_with(files: &[(&str, &str)], unparsed: RawScan) -> Run {
        run_full(files, &[], unparsed)
    }

    /// As [`run_with`], with `texts` as further files without facts for the word scan.
    fn run_full(files: &[(&str, &str)], texts: &[(&str, &str)], unparsed: RawScan) -> Run {
        let names: std::collections::HashSet<&str> = files
            .iter()
            .flat_map(|(_, src)| src.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')))
            .filter(|w| !w.is_empty())
            .collect();
        let words = crate::liveness::scan_words(
            files
                .iter()
                .chain(texts)
                .map(|(p, t)| (*p, Some(t.as_bytes()))),
            &names,
        );
        let facts: Vec<(String, Language, FileFacts)> = files
            .iter()
            .map(|(p, src)| {
                let l = language_of(p);
                (
                    (*p).to_owned(),
                    l,
                    rb_syntax::extract(l, src.as_bytes()).unwrap(),
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
        let index = Index::new(&by_path, &go, &keys);
        let mut b = GraphBuilder::new();
        for (path, (_, f)) in &by_path {
            b.add_node(NodeKey::new(NodeKind::File, *path), *path, Attrs::new())
                .unwrap();
            crate::syntax::add_symbols(&mut b, path, f, &keys[path]).unwrap();
        }
        crate::resolve::add_edges(&mut b, &index, &BTreeSet::new()).unwrap();
        let service_of = |p: &str| p.split('/').next().unwrap_or_default().to_owned();
        let deployable = BTreeSet::from(["svc".to_owned()]);
        let depends = BTreeMap::new();
        let empty_manifests: BTreeMap<&str, String> = BTreeMap::new();
        let resolution = crate::di::resolve(&crate::di::Inputs {
            files: &by_path,
            keys: &keys,
            index: &index,
            service_of: &service_of,
            deployable: &deployable,
            depends: &depends,
            unparsed: &unparsed,
            manifests: &empty_manifests,
        });
        let considered = implementing_types(&b.edge_sources(EdgeKind::Implements), &by_path, &keys);
        let manifests: BTreeMap<&str, String> =
            texts.iter().map(|(p, t)| (*p, (*t).to_owned())).collect();
        let aliases = AliasMaps::build(&manifests);

        let proof = prove(
            &Inputs {
                files: &by_path,
                service_of: &service_of,
                deployable: &deployable,
                unparsed: &unparsed,
                words: &words,
                fragments: &resolution.fragments,
                aliases: &aliases,
            },
            &considered,
        );
        let types = proof
            .types
            .iter()
            .map(|(&(p, i), rule)| {
                format!(
                    "{}: {}",
                    by_path[p].1.definitions[i].name,
                    rule.unwrap_or("proven")
                )
            })
            .collect();
        let coverage = apply(&mut b, &proof, &by_path, &keys).detail;
        Run {
            types,
            graph: b.finish().unwrap(),
            coverage,
        }
    }

    fn run(files: &[(&str, &str)]) -> Run {
        run_with(files, RawScan::default())
    }

    /// As [`run`], with `texts` as further files without facts (raw scan and word scan).
    fn run_texts(files: &[(&str, &str)], texts: &[(&str, &str)]) -> Run {
        let mut raw = RawScan::default();
        for (path, text) in texts {
            raw.merge(rb_frameworks::clients::raw_scan(path, Some(text)));
        }
        run_full(files, texts, raw)
    }

    /// `src -> dst` of the `IMPLEMENTS` edges flagged `impl_instantiated: false`.
    fn flagged(r: &Run) -> Vec<String> {
        let g = &r.graph;
        let short = |id: i64| {
            let k = &g.nodes().iter().find(|n| n.id == id).unwrap().key;
            k.rsplit_once("`/").map_or(k.clone(), |(_, d)| d.to_owned())
        };
        g.edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Implements)
            .filter(|e| e.attrs.get("impl_instantiated") == Some(&Value::Bool(false)))
            .map(|e| format!("{} -> {}", short(e.src), short(e.dst)))
            .collect()
    }

    const SINKS: &str = "package com.acme;\ninterface AuditSink { void record(String e); }\nclass LogAuditSink implements AuditSink { public void record(String e) {} }\nclass KafkaAuditSink implements AuditSink { public void record(String e) {} }\nclass Use { AuditSink s = new LogAuditSink(); }\n";

    /// The proving row (the fixture's `KafkaAuditSink`) and its kept neighbour.
    #[test]
    fn a_never_mentioned_implementation_is_proven() {
        let r = run(&[("svc/src/main/java/com/acme/Sinks.java", SINKS)]);
        assert_eq!(r.types, ["LogAuditSink: mention", "KafkaAuditSink: proven"]);
        assert_eq!(
            flagged(&r),
            [
                "KafkaAuditSink# -> AuditSink#",
                "KafkaAuditSink#record(). -> AuditSink#record()."
            ]
        );
        assert_eq!(r.coverage["proven"], 1);
        assert_eq!(r.coverage["prunable_edges"], 2);
    }

    /// One keeping row per rule (S9).
    #[test]
    fn one_keeping_row_per_rule() {
        let kafka = |extra: &str| format!("{SINKS}{extra}");
        let rule_of = |files: &[(&str, &str)], raw: RawScan| -> String {
            run_with(files, raw)
                .types
                .into_iter()
                .find(|t| t.starts_with("KafkaAuditSink"))
                .unwrap()
        };
        let main = "svc/src/main/java/com/acme/Sinks.java";
        for (extra, want) in [
            ("class U { Object o = new KafkaAuditSink(); }\n", "mention"),
            ("class U { Object o = KafkaAuditSink::new; }\n", "mention"),
            ("class U { Object o = KafkaAuditSink.class; }\n", "mention"),
            ("class U { KafkaAuditSink k; }\n", "mention"),
            ("class U { Object o = KafkaAuditSink.make(); }\n", "mention"),
            (
                "class U { String s = \"com.acme.KafkaAuditSink\"; }\n",
                "string",
            ),
            (
                "@org.springframework.stereotype.Component class KafkaAuditSink2 {}\n",
                "proven",
            ),
        ] {
            let src = kafka(extra);
            assert_eq!(
                rule_of(&[(main, &src)], RawScan::default()),
                format!("KafkaAuditSink: {want}"),
                "{extra}"
            );
        }
        // Annotations on the type or a member; inert ones do not keep it.
        let annotated = SINKS.replace(
            "class KafkaAuditSink",
            "@org.springframework.stereotype.Component class KafkaAuditSink",
        );
        assert_eq!(
            rule_of(&[(main, &annotated)], RawScan::default()),
            "KafkaAuditSink: annotation"
        );
        let member = SINKS.replace("class KafkaAuditSink implements AuditSink { public void record", "class KafkaAuditSink implements AuditSink { @org.junit.jupiter.api.Test public void record");
        assert_eq!(
            rule_of(&[(main, &member)], RawScan::default()),
            "KafkaAuditSink: annotation"
        );
        let inert = SINKS.replace(
            "class KafkaAuditSink implements AuditSink { public void record",
            "class KafkaAuditSink implements AuditSink { @Override public void record",
        );
        assert_eq!(
            rule_of(&[(main, &inert)], RawScan::default()),
            "KafkaAuditSink: proven"
        );
        // A name in a file without facts (a `.kt` file), or one over the byte cap.
        let raw = RawScan {
            words: BTreeSet::from(["KafkaAuditSink".to_owned()]),
            ..RawScan::default()
        };
        assert_eq!(rule_of(&[(main, SINKS)], raw), "KafkaAuditSink: string");
        let over = RawScan {
            words_unread: true,
            ..RawScan::default()
        };
        assert_eq!(rule_of(&[(main, SINKS)], over), "KafkaAuditSink: string");
        // A CDI `beans.xml`.
        let beans = RawScan {
            di_surface: vec![(
                "svc/src/main/resources/META-INF/beans.xml".into(),
                "beans.xml".into(),
            )],
            ..RawScan::default()
        };
        assert_eq!(rule_of(&[(main, SINKS)], beans), "KafkaAuditSink: surface");
        // `includeFilters` anywhere, a classpath scanner import.
        let filters =
            format!("{SINKS}@ComponentScan(includeFilters = @Filter(Foo.class)) class Cfg {{}}\n");
        assert_eq!(
            rule_of(&[(main, &filters)], RawScan::default()),
            "KafkaAuditSink: surface"
        );
        let scanner = format!(
            "import org.reflections.Reflections;\n{}",
            SINKS.replace("package com.acme;\n", "")
        );
        assert_eq!(
            rule_of(
                &[(main, &format!("package com.acme;\n{scanner}"))],
                RawScan::default()
            ),
            "KafkaAuditSink: surface"
        );
        let provider = format!(
            "{SINKS}class P {{ Object p = new ClassPathScanningCandidateComponentProvider(false); }}\n"
        );
        assert_eq!(
            rule_of(&[(main, &provider)], RawScan::default()),
            "KafkaAuditSink: surface"
        );
        // A test path; a library service.
        assert_eq!(
            rule_of(
                &[("svc/src/test/java/com/acme/Sinks.java", SINKS)],
                RawScan::default()
            ),
            "KafkaAuditSink: test"
        );
        assert_eq!(
            rule_of(
                &[("lib/src/main/java/com/acme/Sinks.java", SINKS)],
                RawScan::default()
            ),
            "KafkaAuditSink: library"
        );
        // A static import of the type (enum constants used unqualified).
        let other = "package com.other;\nimport static com.acme.KafkaAuditSink.A;\nclass V {}\n";
        assert_eq!(
            rule_of(
                &[(main, SINKS), ("svc/src/main/java/com/other/V.java", other)],
                RawScan::default()
            ),
            "KafkaAuditSink: mention"
        );
        // A Spring Data fragment implementation.
        let fragment = "package com.acme;\ninterface UserRepositoryCustom { void touch(long id); }\ninterface UserRepository extends org.springframework.data.jpa.repository.JpaRepository<User, Long>, UserRepositoryCustom {}\nclass UserRepositoryCustomImpl implements UserRepositoryCustom { public void touch(long id) {} }\n";
        let r = run(&[("svc/src/main/java/com/acme/R.java", fragment)]);
        assert!(
            r.types
                .contains(&"UserRepositoryCustomImpl: fragment".to_owned()),
            "{:?}",
            r.types
        );
        // TS: a default export; a language never proven.
        let ts = "interface Sink { record(): void }\nexport default class KafkaAuditSink implements Sink { record() {} }\n";
        assert_eq!(
            run(&[("svc/src/sink.ts", ts)]).types,
            ["KafkaAuditSink: default_export"]
        );
        // A named export used nowhere is proven: an importer would mention it (or alias it).
        let ts_plain = "interface Sink { record(): void }\nexport class KafkaAuditSink implements Sink { record() {} }\n";
        assert_eq!(
            run(&[("svc/src/sink.ts", ts_plain)]).types,
            ["KafkaAuditSink: proven"]
        );
        let importer = "import { KafkaAuditSink as K } from './sink';\nexport const k = new K();\n";
        assert_eq!(
            run(&[("svc/src/sink.ts", ts_plain), ("svc/src/use.ts", importer)]).types,
            ["KafkaAuditSink: mention"]
        );
        let ts_local = "interface Sink { record(): void }\nclass KafkaAuditSink implements Sink { record() {} }\n";
        assert_eq!(
            run(&[("svc/src/sink.ts", ts_local)]).types,
            ["KafkaAuditSink: proven"]
        );
        let py =
            "class Base:\n    def run(self): pass\nclass Impl(Base):\n    def run(self): pass\n";
        assert_eq!(run(&[("svc/a.py", py)]).types, ["Impl: language"]);
    }

    /// (i): an instantiated subtype keeps its supertypes at depth 1 and 2; the reverse keeps
    /// nothing; a supertype cycle terminates; a long chain is kept whole.
    #[test]
    fn instantiated_subtypes_keep_their_supertypes() {
        let main = "svc/src/main/java/com/acme/H.java";
        let chain = "package com.acme;\ninterface I { void m(); }\nclass A implements I { public void m() {} }\nclass B extends A { public void m() {} }\nclass C extends B { public void m() {} }\n";
        // Nothing instantiated: all proven.
        assert_eq!(
            run(&[(main, chain)]).types,
            ["A: proven", "B: proven", "C: proven"]
        );
        // `new B()` keeps B and A (depth 1), not C (reverse).
        let r = run(&[(main, &format!("{chain}class U {{ Object o = new B(); }}\n"))]);
        assert_eq!(r.types, ["A: subtype", "B: mention", "C: proven"]);
        // `new C()` keeps B (depth 1) and A (depth 2).
        let r = run(&[(main, &format!("{chain}class U {{ Object o = new C(); }}\n"))]);
        assert_eq!(r.types, ["A: subtype", "B: subtype", "C: mention"]);
        // A subtype in a test source is instantiated too.
        let t = "package com.acme;\nclass Fake extends C { }\n";
        let r = run(&[(main, chain), ("svc/src/test/java/com/acme/Fake.java", t)]);
        assert_eq!(
            r.types,
            ["A: subtype", "B: subtype", "C: subtype", "Fake: test"]
        );
        // A cycle (`A extends B`, `B extends A`) terminates.
        let cycle = "package com.acme;\ninterface I { void m(); }\nclass A extends B implements I { public void m() {} }\nclass B extends A { public void m() {} }\nclass U { Object o = new A(); }\n";
        let r = run(&[(main, cycle)]);
        assert_eq!(r.types, ["A: mention", "B: subtype"]);
        // A 5,000-deep chain with an instance at the bottom is kept whole.
        let mut deep = String::from(
            "package com.acme;\ninterface I { void m(); }\nclass T0 implements I { public void m() {} }\n",
        );
        for i in 1..5000 {
            deep += &format!("class T{i} extends T{} {{}}\n", i - 1);
        }
        deep += "class U { Object o = new T4999(); }\n";
        let r = run(&[(main, &deep)]);
        assert!(
            r.types.iter().all(|t| !t.ends_with("proven")),
            "{:?}",
            &r.types[..3]
        );
    }

    /// Evaluator pass 1, Y1 and Y14: a nested class's binary name in a string or in a
    /// `META-INF/services` file, and its name in any non-binary file that has no Java/TS/JS
    /// mention facts (Python, Go, `.txt`, a lock file), keep it (`string`).
    #[test]
    fn binary_names_and_every_text_file_keep_a_type() {
        let main = "svc/src/main/java/com/acme/Holder.java";
        let holder = "package com.acme;\ninterface Greeter { String greet(); }\nclass Holder { static class LoadedImpl implements Greeter { public String greet() { return \"l\"; } } }\n";
        let rule = |files: &[(&str, &str)], texts: &[(&str, &str)]| -> String {
            run_texts(files, texts)
                .types
                .into_iter()
                .find(|t| t.starts_with("LoadedImpl"))
                .unwrap()
        };
        assert_eq!(rule(&[(main, holder)], &[]), "LoadedImpl: proven");
        let forname =
            "package com.acme;\nclass App { Object o = load(\"com.acme.Holder$LoadedImpl\"); }\n";
        assert_eq!(
            rule(
                &[
                    (main, holder),
                    ("svc/src/main/java/com/acme/App.java", forname)
                ],
                &[]
            ),
            "LoadedImpl: string"
        );
        for (path, text) in [
            (
                "svc/src/main/resources/META-INF/services/com.acme.Greeter",
                "com.acme.Holder$LoadedImpl\n",
            ),
            ("svc/plugins.txt", "Holder.LoadedImpl\n"),
            ("svc/package-lock.json", "{\"LoadedImpl\": 1}"),
            ("svc/tools/run.py", "cls = 'LoadedImpl'\n"),
            ("svc/tools/main.go", "package main\n// LoadedImpl\n"),
        ] {
            assert_eq!(
                rule(&[(main, holder)], &[(path, text)]),
                "LoadedImpl: string",
                "{path}"
            );
        }
        // A Python or Go file is parsed, and still counts.
        assert_eq!(
            rule(
                &[(main, holder), ("svc/tools/run.py", "x = 'LoadedImpl'\n")],
                &[]
            ),
            "LoadedImpl: string"
        );
    }

    /// Evaluator pass 1, Y2: a base whose only subclass is no definition (a Java local class
    /// in a lambda or in an anonymous class body, a TS class expression) is kept.
    #[test]
    fn subclasses_that_are_no_definitions_keep_their_base() {
        let base = "package com.acme;\ninterface Greeter { String greet(); }\nclass LambdaBase implements Greeter { public String greet() { return \"b\"; } }\n";
        let main = "svc/src/main/java/com/acme/Base.java";
        assert_eq!(run(&[(main, base)]).types, ["LambdaBase: proven"]);
        for user in [
            "package com.acme;\nclass App { Object s = (java.util.function.Supplier<Greeter>) () -> { class Local extends LambdaBase {} return new Local(); }; }\n",
            "package com.acme;\nclass App { Object r = new Runnable() { public void run() { class Local extends LambdaBase {} new Local(); } }; }\n",
        ] {
            let r = run(&[(main, base), ("svc/src/main/java/com/acme/App.java", user)]);
            assert!(
                r.types.contains(&"LambdaBase: mention".to_owned()),
                "{user}: {:?}",
                r.types
            );
        }
        let ts_base = "export interface Greeter { greet(): string }\nexport class ExprBase implements Greeter { greet(): string { return 'e'; } }\n";
        assert_eq!(
            run(&[("svc/src/base.ts", ts_base)]).types,
            ["ExprBase: proven"]
        );
        let ts_main = "import { ExprBase } from './base';\nconst Derived = class extends ExprBase {};\nexport const d = new Derived();\n";
        assert_eq!(
            run(&[("svc/src/base.ts", ts_base), ("svc/src/main.ts", ts_main)]).types,
            ["ExprBase: mention"]
        );
    }

    /// Evaluator pass 1, Y7: a module whose whole object is taken (`import * as`, `export *`,
    /// `require`, `import()`) keeps every type it declares; a namespace import of another
    /// module does not.
    #[test]
    fn module_objects_keep_their_types() {
        let commands = "export interface Command { run(): string }\nexport class DeployCommand implements Command { run(): string { return 'deploy'; } }\n";
        let decl = "svc/src/commands/index.ts";
        let rule = |user: (&str, &str)| -> String {
            run(&[(decl, commands), user])
                .types
                .into_iter()
                .find(|t| t.starts_with("DeployCommand"))
                .unwrap()
        };
        for user in [
            (
                "svc/src/main.ts",
                "import * as commands from './commands/index';\nexport const all = Object.values(commands);\n",
            ),
            (
                "svc/src/main.ts",
                "import * as commands from './commands';\nexport const all = Object.values(commands);\n",
            ),
            (
                "svc/src/main.ts",
                "import * as commands from './commands/index.js';\nexport const all = commands;\n",
            ),
            ("svc/src/all.ts", "export * from './commands';\n"),
            (
                "svc/src/main.js",
                "const commands = require('./commands');\nmodule.exports = commands;\n",
            ),
            (
                "svc/src/main.ts",
                "export async function f(n: string) { return import('./commands/' + n); }\n",
            ),
            (
                "svc/src/main.ts",
                "import * as commands from '@app/commands';\nexport const all = commands;\n",
            ),
            // A package whose entry lives in a directory on the module's path.
            (
                "svc/src/main.ts",
                "import * as svc from '@acme/src';\nexport const all = svc;\n",
            ),
        ] {
            assert_eq!(rule(user), "DeployCommand: mention", "{}", user.1);
        }
        for user in [
            (
                "svc/src/main.ts",
                "import * as path from 'path';\nexport const p = path;\n",
            ),
            (
                "svc/src/main.ts",
                "import * as other from './other';\nexport const o = other;\n",
            ),
        ] {
            assert_eq!(rule(user), "DeployCommand: proven", "{}", user.1);
        }
    }

    /// Contract M6c S4: `tsconfig.json` `paths` aliases and `package.json`
    /// `exports` resolve module objects exactly — a type named only through an
    /// exactly-resolved alias is proven when otherwise clean; an unresolvable
    /// alias or unknown package keeps every type, as before.
    #[test]
    fn manifest_aliases_resolve_module_objects_exactly() {
        let user = "export interface User { id: string }\nexport class DbUser implements User { id: string = \"\"; }\n";
        let profile = "export interface Profile { id: string }\nexport class DbProfile implements Profile { id: string = \"\"; }\n";
        let find = |files: &[(&str, &str)], texts: &[(&str, &str)]| -> Vec<String> {
            let mut out: Vec<String> = run_full(files, texts, RawScan::default())
                .types
                .into_iter()
                .filter(|t| t.starts_with("DbUser") || t.starts_with("DbProfile"))
                .collect();
            out.sort();
            out
        };
        let decl_a = "svc/src/models/user.ts";
        let decl_b = "svc/src/other/user.ts";
        let user_file = (
            "svc/src/main.ts",
            "import * as m from '@models/user';\nexport const all = m;\n",
        );
        let tsconfig = (
            "svc/tsconfig.json",
            "{ \"compilerOptions\": { \"paths\": { \"@models/*\": [\"src/models/*\"] } } }",
        );
        // Exact: the resolved file's type is kept, the same-stemmed other is proven.
        assert_eq!(
            find(&[(decl_a, user), (decl_b, profile), user_file], &[tsconfig]),
            ["DbProfile: proven", "DbUser: mention"]
        );
        // An alias to nowhere resolves nothing: both kept, as before.
        let nowhere = (
            "svc/tsconfig.json",
            "{ \"compilerOptions\": { \"paths\": { \"@models/*\": [\"src/elsewhere/*\"] } } }",
        );
        assert_eq!(
            find(&[(decl_a, user), (decl_b, profile), user_file], &[nowhere]),
            ["DbProfile: mention", "DbUser: mention"]
        );
        // A pattern with several targets is ambiguous (D4): both kept, as before.
        let multi = (
            "svc/tsconfig.json",
            "{ \"compilerOptions\": { \"paths\": { \"@models/*\": [\"src/models/*\", \"src/other/*\"] } } }",
        );
        assert_eq!(
            find(&[(decl_a, user), (decl_b, profile), user_file], &[multi]),
            ["DbProfile: mention", "DbUser: mention"]
        );
        // An unknown package keeps every type, as before.
        let other_user = (
            "svc/src/main.ts",
            "import * as m from '@other/user';\nexport const all = m;\n",
        );
        assert_eq!(
            find(
                &[(decl_a, user), (decl_b, profile), other_user],
                &[tsconfig]
            ),
            ["DbProfile: mention", "DbUser: mention"]
        );
        // `package.json` `exports` resolve exactly too.
        let pkg = (
            "svc/package.json",
            "{ \"name\": \"@acme/models\", \"exports\": { \"./user\": \"./src/models/user.ts\" } }",
        );
        let pkg_user = (
            "svc/src/main.ts",
            "import * as m from '@acme/models/user';\nexport const all = m;\n",
        );
        assert_eq!(
            find(&[(decl_a, user), (decl_b, profile), pkg_user], &[pkg]),
            ["DbProfile: proven", "DbUser: mention"]
        );
    }

    /// Contract M6c S4/D4: precedence is exact-beats-pattern with longest-pattern
    /// winning, and a pattern with several targets resolves nothing (keep-all).
    #[test]
    fn alias_resolution_precedence_and_ambiguity() {
        let maps = |texts: &[(&str, &str)]| {
            AliasMaps::build(&texts.iter().map(|(p, t)| (*p, (*t).to_owned())).collect())
        };
        // A pattern with several targets is ambiguous: resolves nothing.
        let multi = maps(&[(
            "svc/tsconfig.json",
            "{ \"compilerOptions\": { \"paths\": { \"@m/*\": [\"src/a/*\", \"src/b/*\"] } } }",
        )]);
        assert_eq!(multi.resolve("svc/src/main.ts", "@m/user"), None);
        // Longest pattern wins over map order.
        let longest = maps(&[(
            "svc/tsconfig.json",
            "{ \"compilerOptions\": { \"paths\": {
                \"@m/*\": [\"src/generic/*\"],
                \"@m/special/*\": [\"src/special/*\"] } } }",
        )]);
        assert_eq!(
            longest.resolve("svc/src/main.ts", "@m/special/user"),
            Some("svc/src/special/user".to_owned())
        );
        assert_eq!(
            longest.resolve("svc/src/main.ts", "@m/other"),
            Some("svc/src/generic/other".to_owned())
        );
        // `exports`: an exact subpath beats a pattern even when the pattern
        // sorts first; of several patterns the longest wins.
        let exports = maps(&[(
            "svc/package.json",
            "{ \"name\": \"@acme/m\", \"exports\": {
                \"./*\": \"./src/fallback/*.ts\",
                \"./user\": \"./src/models/user.ts\",
                \"./a/*\": \"./src/a/*.ts\",
                \"./a\": \"./src/exact-a.ts\",
                \"./a*\": \"./src/star-a.ts\" } }",
        )]);
        assert_eq!(
            exports.resolve("svc/src/main.ts", "@acme/m/user"),
            Some("svc/src/models/user.ts".to_owned())
        );
        assert_eq!(
            exports.resolve("svc/src/main.ts", "@acme/m/a/x"),
            Some("svc/src/a/x.ts".to_owned())
        );
        assert_eq!(
            exports.resolve("svc/src/main.ts", "@acme/m/other"),
            Some("svc/src/fallback/other.ts".to_owned())
        );
        // The exact key beats even a longer pattern (`./a*` would win on
        // length alone through the pattern loop).
        assert_eq!(
            exports.resolve("svc/src/main.ts", "@acme/m/a"),
            Some("svc/src/exact-a.ts".to_owned())
        );
    }

    /// Invariant 9: alias resolution grows linearly (counted, 4× → ≤ 5×).
    #[test]
    fn alias_resolution_grows_linearly() {
        fn work_for(n: usize) -> u64 {
            let manifests = BTreeMap::from([(
                "svc/tsconfig.json",
                "{ \"compilerOptions\": { \"paths\": { \"@models/*\": [\"src/models/*\"] } } }"
                    .to_owned(),
            )]);
            let aliases = AliasMaps::build(&manifests);
            work::take();
            for i in 0..n {
                // One pattern per tsconfig (config-bounded); specifiers scale
                // with the repo, so per-resolve work is constant.
                let _ = aliases.resolve("svc/src/main.ts", &format!("@models/u{i}"));
            }
            work::take()
        }
        let (small, large) = (work_for(50), work_for(200));
        assert!(large <= small * 5, "{small} -> {large}");
    }

    /// Evaluator pass 1, Y12 (mutant R4): an instantiated subtype that is no `IMPLEMENTS`
    /// source itself (its head does not resolve) still keeps the type it names.
    #[test]
    fn an_unconsidered_instantiated_subtype_keeps_its_supertype() {
        let main = "svc/src/main/java/com/acme/H.java";
        let src = "package com.acme;\ninterface I { void m(); }\nclass A implements I { public void m() {} }\nclass B extends Lib.A {}\nclass U { Object o = new B(); }\n";
        assert_eq!(run(&[(main, src)]).types, ["A: subtype"]);
    }

    /// Invariant 11: every considered type is proven or kept by exactly one rule.
    #[test]
    fn rta_buckets_sum_to_the_types() {
        let r = run(&[
            ("svc/src/main/java/com/acme/Sinks.java", SINKS),
            (
                "lib/src/main/java/com/acme/L.java",
                "package com.acme;\ninterface Q {}\nclass LQ implements Q {}\n",
            ),
        ]);
        let kept: u64 = r.coverage["kept"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_u64().unwrap())
            .sum();
        assert_eq!(
            kept + r.coverage["proven"].as_u64().unwrap(),
            r.coverage["types"].as_u64().unwrap()
        );
        assert_eq!(r.coverage["types"], 3);
    }

    fn work_for(n: usize) -> u64 {
        let mut src = String::from("package com.acme;\n");
        for i in 0..n {
            src += &format!(
                "interface I{i} {{ void m(); }}\nclass A{i} implements I{i} {{ public void m() {{}} }}\nclass B{i} extends A{i} {{}}\nclass U{i} {{ Object o = new B{i}(); }}\n"
            );
        }
        let facts = rb_syntax::extract(Language::Java, src.as_bytes()).unwrap();
        let by_path: BTreeMap<&str, (Language, &FileFacts)> =
            BTreeMap::from([("svc/src/main/java/A.java", (Language::Java, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> = by_path
            .iter()
            .map(|(p, (_, f))| (*p, rb_syntax::symbol_keys(p, f)))
            .collect();
        let considered: BTreeSet<Sym<'_>> = facts
            .definitions
            .iter()
            .enumerate()
            .filter(|(_, d)| d.kind == DefKind::Class && !d.supertypes.is_empty())
            .map(|(i, _)| ("svc/src/main/java/A.java", i))
            .collect();
        let service_of = |_: &str| "svc".to_owned();
        let deployable = BTreeSet::from(["svc".to_owned()]);
        let (raw, fragments) = (RawScan::default(), BTreeSet::new());
        let _ = keys;
        let names: std::collections::HashSet<&str> =
            facts.definitions.iter().map(|d| d.name.as_str()).collect();
        let words = crate::liveness::scan_words(
            [("svc/src/main/java/A.java", Some(src.as_bytes()))],
            &names,
        );
        work::take();
        let no_aliases = AliasMaps::default();
        let proof = prove(
            &Inputs {
                files: &by_path,
                service_of: &service_of,
                deployable: &deployable,
                unparsed: &raw,
                words: &words,
                fragments: &fragments,
                aliases: &no_aliases,
            },
            &considered,
        );
        assert_eq!(proof.types.len(), 2 * n);
        work::take()
    }

    /// Invariant 9: the proof grows linearly in types and mentions (counted, 4× → ≤ 5×).
    #[test]
    fn the_proof_grows_linearly() {
        let (small, large) = (work_for(200), work_for(800));
        assert!(large <= small * 5, "{small} -> {large}");
    }
}
