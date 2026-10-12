//! Entry points and orphan eligibility at index time (contract M4b S11 stored kinds, S12, D10,
//! D11). Each definition gets its entry kinds from its annotations, modifiers, names and file
//! (import-gated, invariant 5); a symbol is `orphan_eligible` only when every rule below holds,
//! the last being a whole-word scan of every non-binary file of the repository: each occurrence
//! of its name must be its own declaration or lie in the evidence of an edge into it. Every
//! rule that fails keeps the symbol alive (invariant 4), and the first one is counted.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rb_frameworks::routes::is_annotation;
use rb_frameworks::work;
use rb_model::{Diagnostic, EdgeKind, Evidence, GraphBuilder, NodeKey, NodeKind, Span};
use rb_syntax::{DefKind, Definition, FileFacts, Language};
use serde_json::{Value, json};

use crate::di::is_test_path;
use crate::resolve::Index;

/// Bytes of one file the word scan reads; a larger file is not read, and then every name
/// counts as mentioned somewhere unaccounted (the scan can only over-protect).
pub(crate) const WORD_SCAN_BYTES: u64 = 32 * 1024 * 1024;

/// Occurrences of one name kept for the check; a name seen more often is `crowded` and keeps
/// every symbol of that name alive (a bound on memory, which can only over-protect).
pub(crate) const CROWDED: usize = 256;

/// The first failing rule of an ineligible symbol (S12), in the order they are checked.
pub(crate) const RULES: &[&str] = &[
    "library",
    "test",
    "entry",
    "annotation",
    "convention",
    "default_export",
    "supertype",
    "mention",
];

/// Where the names of the snapshot's definitions occur, word by word, in every non-binary
/// file. The default is "not scanned", under which every name counts as mentioned.
#[derive(Debug)]
pub struct Words {
    paths: Vec<String>,
    /// Occurrences per name: `(index into paths, [row, col, row, col])`, rows split at `\n` and
    /// columns in bytes, as Tree-sitter's.
    by_name: HashMap<String, Vec<(usize, Span)>>,
    crowded: HashSet<String>,
    /// A file was over [`WORD_SCAN_BYTES`], or nothing was scanned.
    unread: bool,
}

impl Default for Words {
    fn default() -> Self {
        Words {
            paths: Vec::new(),
            by_name: HashMap::new(),
            crowded: HashSet::new(),
            unread: true,
        }
    }
}

fn word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Whether the scan can find every occurrence of `name`: only ASCII letters, digits and `_`.
/// Any other character (a `$`, a non-ASCII letter) splits words, so such a name is never
/// matched whole and its symbol stays alive.
fn scannable(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(word_byte)
}

/// Scans `files` (path, bytes; `None` for a file over the cap) for the whole-word occurrences
/// of `names`.
#[cfg(test)]
pub(crate) fn scan_words<'a>(
    files: impl IntoIterator<Item = (&'a str, Option<&'a [u8]>)>,
    names: &HashSet<&str>,
) -> Words {
    let mut words = Words::scanned();
    for (path, bytes) in files {
        words.scan(path, bytes, names);
    }
    words
}

impl Words {
    /// The files in which `name` occurs as a whole word, or `None` when that is unknown: a file
    /// was not read, the name is crowded, or it cannot be matched whole (RTA's S9 (d)).
    pub(crate) fn paths_of(&self, name: &str) -> Option<BTreeSet<&str>> {
        if self.unread || !scannable(name) || self.crowded.contains(name) {
            return None;
        }
        Some(
            self.by_name
                .get(name)
                .into_iter()
                .flatten()
                .map(|(file, _)| self.paths[*file].as_str())
                .collect(),
        )
    }

    /// An empty scan, to which [`Words::scan`] adds files.
    pub(crate) fn scanned() -> Self {
        Words {
            unread: false,
            ..Words::default()
        }
    }

    /// Adds the whole-word occurrences of `names` in one file (`None`: over the cap, not
    /// read). A word is a maximal run of ASCII letters, digits and `_`, so `$name`, `name$`
    /// and `naméx` all contain the word `name` (more occurrences, never fewer).
    pub(crate) fn scan(&mut self, path: &str, bytes: Option<&[u8]>, names: &HashSet<&str>) {
        let Some(bytes) = bytes else {
            self.unread = true;
            return;
        };
        let file = self.paths.len();
        self.paths.push(path.to_owned());
        let (mut row, mut line_start, mut i) = (0i32, 0usize, 0usize);
        while i < bytes.len() {
            work::tick(1);
            let b = bytes[i];
            if b == b'\n' {
                row += 1;
                line_start = i + 1;
                i += 1;
                continue;
            }
            if !word_byte(b) {
                i += 1;
                continue;
            }
            let start = i;
            while i < bytes.len() && word_byte(bytes[i]) {
                i += 1;
            }
            // Word bytes are ASCII, so the slice is valid UTF-8.
            let word = std::str::from_utf8(&bytes[start..i]).unwrap_or_default();
            if !names.contains(word) || self.crowded.contains(word) {
                continue;
            }
            let at = self.by_name.entry(word.to_owned()).or_default();
            if at.len() == CROWDED {
                self.by_name.remove(word);
                self.crowded.insert(word.to_owned());
                continue;
            }
            let col = |x: usize| i32::try_from(x - line_start).unwrap_or(i32::MAX);
            at.push((file, [row, col(start), row, col(i)]));
        }
    }
}

/// The entry kinds of definition `i` (S11's stored kinds), sorted; empty for none.
pub(crate) fn entry_kinds(
    path: &str,
    language: Language,
    facts: &FileFacts,
    i: usize,
) -> Vec<&'static str> {
    let def = &facts.definitions[i];
    let mut kinds = BTreeSet::new();
    let annotated = |package: &str, name: &str| {
        def.decorations
            .iter()
            .any(|d| is_annotation(facts, d, package, name))
    };
    match language {
        Language::Java if def.kind == DefKind::Method => {
            if java_main(facts, i) {
                kinds.insert("main");
            }
            let junit = [
                ("org.junit", "Test"),
                ("org.junit.jupiter.api", "Test"),
                ("org.junit.jupiter.api", "RepeatedTest"),
                ("org.junit.jupiter.api", "TestFactory"),
                ("org.junit.jupiter.api", "TestTemplate"),
                ("org.junit.jupiter.params", "ParameterizedTest"),
            ];
            if junit.iter().any(|(p, n)| annotated(p, n)) {
                kinds.insert("test");
            }
            if annotated("org.springframework.scheduling.annotation", "Scheduled") {
                kinds.insert("scheduled");
            }
            let listeners = [
                ("org.springframework.context.event", "EventListener"),
                (
                    "org.springframework.transaction.event",
                    "TransactionalEventListener",
                ),
                ("org.springframework.kafka.annotation", "KafkaListener"),
                (
                    "org.springframework.amqp.rabbit.annotation",
                    "RabbitListener",
                ),
                ("org.springframework.jms.annotation", "JmsListener"),
            ];
            if listeners.iter().any(|(p, n)| annotated(p, n)) {
                kinds.insert("listener");
            }
            let lifecycle = [
                ("jakarta.annotation", "PostConstruct"),
                ("javax.annotation", "PostConstruct"),
                ("jakarta.annotation", "PreDestroy"),
                ("javax.annotation", "PreDestroy"),
                ("org.springframework.context.annotation", "Bean"),
            ];
            let runner = def.name == "run"
                && def.parent.is_some_and(|p| {
                    facts.definitions[p].supertypes.iter().any(|s| {
                        ["CommandLineRunner", "ApplicationRunner"]
                            .iter()
                            .any(|n| names_type(facts, &s.name, "org.springframework.boot", n))
                    })
                });
            if runner || lifecycle.iter().any(|(p, n)| annotated(p, n)) {
                kinds.insert("lifecycle");
            }
        }
        Language::Go if def.kind == DefKind::Function && def.parent.is_none() => {
            if def.name == "main" && facts.package.as_deref() == Some("main") {
                kinds.insert("main");
            }
            let tested = ["Test", "Benchmark", "Fuzz", "Example"].iter().any(|p| {
                def.name
                    .strip_prefix(p)
                    .is_some_and(|rest| !rest.starts_with(|c: char| c.is_ascii_lowercase()))
            });
            if tested && path.ends_with("_test.go") {
                kinds.insert("test");
            }
        }
        Language::Python
            if matches!(def.kind, DefKind::Function | DefKind::Method)
                && def.name.starts_with("test_")
                && is_test_path(path) =>
        {
            kinds.insert("test");
        }
        _ => {}
    }
    kinds.into_iter().collect()
}

/// A Java `static void main` with one `String[]` parameter, or with no parameter binding: a
/// varargs `String...` parameter, and a scalar one, are no bindings (amendment 4), so the facts
/// cannot tell `main(String... a)` from `main()` (a launch entry since Java 25) or `main(int x)`;
/// marking those too can only keep code alive (D10).
fn java_main(facts: &FileFacts, i: usize) -> bool {
    let def = &facts.definitions[i];
    if def.name != "main"
        || !def.modifiers.iter().any(|m| m == "static")
        || def.return_type.as_deref() != Some("void")
    {
        return false;
    }
    let params: Vec<_> = facts
        .bindings
        .iter()
        .filter(|b| b.scope == Some(i) && b.parameter.is_some())
        .collect();
    match params.as_slice() {
        [] => true,
        [p] => {
            p.parameter == Some(0)
                && p.array
                && matches!(
                    p.type_args.as_slice(),
                    [t] if t == "String" || t == "java.lang.String"
                )
        }
        _ => false,
    }
}

/// Whether a type name written in `facts` is `package.simple`: qualified, or imported by name
/// or by a wildcard of its package.
fn names_type(facts: &FileFacts, written: &str, package: &str, simple: &str) -> bool {
    written == format!("{package}.{simple}")
        || (written == simple
            && facts.imports.iter().any(|i| {
                !i.is_static
                    && i.module == package
                    && (i.wildcard
                        || i.names
                            .iter()
                            .any(|n| n.name == simple && n.local == simple))
            }))
}

/// Methods called by convention, through a library or a property name (S12 (d)).
const CONVENTION: &[&str] = &[
    "toString",
    "equals",
    "hashCode",
    "compareTo",
    "compare",
    "close",
    "iterator",
    "run",
    "call",
    "get",
    "accept",
    "apply",
    "test",
    "finalize",
    "readObject",
    "writeObject",
    "readResolve",
    "writeReplace",
    "valueOf",
    "values",
    "constructor",
];

/// S12 (d): a convention-invoked name. The method list applies to methods of every language
/// (the contract names Java and TS; wider can only keep code alive; a TS `constructor` is run
/// by `new`, never called by its name: amendment 10), bean accessors
/// (`get`/`is`/`set` and an upper-case letter) too, Python's `__*__` to any definition, and
/// every Go method.
fn convention_invoked(language: Language, def: &Definition) -> bool {
    let name = def.name.as_str();
    if language == Language::Go && def.kind == DefKind::Method {
        return true;
    }
    if name.len() > 4 && name.starts_with("__") && name.ends_with("__") {
        return true;
    }
    if def.kind != DefKind::Method {
        return false;
    }
    let accessor = ["get", "is", "set"].iter().any(|p| {
        name.strip_prefix(p)
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_uppercase()))
    });
    accessor || CONVENTION.contains(&name)
}

/// What eligibility reads.
pub(crate) struct Inputs<'a> {
    pub files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
    pub keys: &'a BTreeMap<&'a str, Vec<String>>,
    pub index: &'a Index<'a>,
    pub service_of: &'a dyn Fn(&str) -> String,
    /// Deployable services (`role = service`).
    pub deployable: &'a BTreeSet<String>,
    pub words: &'a Words,
    /// The interfaces Spring Data repositories extend, by simple name (fragments, contract
    /// M4b D3; the set RTA's rule (h) reads).
    pub fragments: &'a BTreeSet<String>,
}

/// Rules (a)–(e) for definition `i` of `path`: the first that fails, or `None`.
fn own_rule<'a>(inputs: &Inputs<'a>, path: &'a str, i: usize, entry: bool) -> Option<&'static str> {
    let (language, facts) = inputs.files[path];
    let def = &facts.definitions[i];
    if !inputs.deployable.contains(&(inputs.service_of)(path)) {
        return Some("library");
    }
    if is_test_path(path) {
        return Some("test");
    }
    if entry {
        return Some("entry");
    }
    // The definition and its enclosing definitions.
    let mut chain = vec![i];
    let mut cur = def.parent;
    while let Some(p) = cur {
        work::tick(1);
        chain.push(p);
        cur = facts.definitions[p].parent;
    }
    let inert = |d: &rb_syntax::Decoration| rb_frameworks::di::is_inert(facts, d);
    let annotated = chain.iter().any(|&j| {
        let d = &facts.definitions[j];
        (j == i || is_type_kind(d.kind)) && d.decorations.iter().any(|x| !inert(x))
    });
    if annotated {
        return Some("annotation");
    }
    if convention_invoked(language, def) {
        return Some("convention");
    }
    // A Spring Data fragment implementation (a type in the chain implementing, by name, an
    // interface a repository extends) is instantiated by its name's convention, members
    // included (contract M5 S19 (d), M4b open item 3; RTA's rule (h)).
    let fragment = chain.iter().any(|&j| {
        let d = &facts.definitions[j];
        is_type_kind(d.kind)
            && d.supertypes.iter().any(|s| {
                let simple = s.name.rsplit('.').next().unwrap_or(&s.name);
                inputs.fragments.contains(simple)
            })
    });
    if fragment {
        return Some("convention");
    }
    // A TS/JS default export is imported under any name, which no word scan sees (amendment
    // 10).
    if def.parent.is_none() && facts.default_export.as_deref() == Some(def.name.as_str()) {
        return Some("default_export");
    }
    // (e): a supertype of the definition (a type) or of an enclosing type, at any depth
    // through the repository's types, that does not resolve to a type of the repository: an
    // override of a library method (evaluator pass 1, Y3).
    let types: Vec<(&str, usize)> = chain
        .iter()
        .filter(|&&j| is_type_kind(facts.definitions[j].kind))
        .map(|&j| (path, j))
        .collect();
    if external_supertype(inputs, &types) {
        return Some("supertype");
    }
    None
}

/// Whether a declared supertype of `start`, or of any repository type they reach through
/// their supertypes, may be no repository type (contract M5 S19 (e), narrowed by M6c S5/D5,
/// M4b open item 21): an ambiguous head, an unresolvable one, or one whose resolution
/// actually reached an on-demand import that names no repository type. A head uniquely
/// resolved at an earlier level (lexical, same file, single-type import, same package) counts
/// in-repo even beside an outside binding of the same name; dotted names keep the old
/// presence rule (their container path has no level).
/// Breadth-first with a visited set, at most [`crate::di::MAX_SUPERTYPE_DEPTH`] levels; a chain
/// longer than that counts as reaching an external type (conservative).
fn external_supertype<'a>(inputs: &Inputs<'a>, start: &[(&'a str, usize)]) -> bool {
    use crate::resolve::TypeLevel;
    let mut seen: BTreeSet<(&'a str, usize)> = start.iter().copied().collect();
    let mut level: Vec<(&'a str, usize)> = start.to_vec();
    for _ in 0..crate::di::MAX_SUPERTYPE_DEPTH {
        let mut next = Vec::new();
        for (path, j) in level {
            let d = &inputs.files[path].1.definitions[j];
            for s in &d.supertypes {
                work::tick(1);
                let found = inputs.index.type_candidates(path, d.parent, &s.name);
                if found.len() != 1 {
                    return true;
                }
                let head = s.name.split('.').next().unwrap_or(&s.name);
                let outside = match inputs.index.type_level(path, d.parent, &s.name) {
                    // Dotted names keep the old presence rule.
                    None => inputs.index.imported_from_outside(path, head),
                    // Only an on-demand resolution can still be external, and only
                    // while the file has an outside wildcard binding.
                    Some(TypeLevel::ImportOnDemand) => inputs.index.on_demand_from_outside(path),
                    // Local, single-import and same-package resolutions — and, by
                    // construction unreachable here, a miss beside a unique hit —
                    // count in-repo.
                    Some(_) => false,
                };
                if outside {
                    return true;
                }
                next.extend(found.into_iter().filter(|t| seen.insert(*t)));
            }
        }
        if next.is_empty() {
            return false;
        }
        level = next;
    }
    true
}

fn is_type_kind(kind: DefKind) -> bool {
    matches!(kind, DefKind::Class | DefKind::Interface | DefKind::Type)
}

/// One symbol key: its definitions, entry kinds and first failing rule.
struct Symbol<'a> {
    name: &'a str,
    entry: BTreeSet<&'static str>,
    rule: Option<&'static str>,
    /// Its declarations' name spans.
    declared: Vec<(&'a str, Span)>,
}

fn within(inner: Span, outer: Span) -> bool {
    (outer[0], outer[1]) <= (inner[0], inner[1]) && (inner[2], inner[3]) <= (outer[2], outer[3])
}

/// Sets `entry` (sorted kinds, when any) and `orphan_eligible: true` on the snapshot's symbols;
/// returns the `orphan_coverage` diagnostic. Runs after every edge producer.
pub(crate) fn apply(b: &mut GraphBuilder, inputs: &Inputs<'_>) -> Diagnostic {
    let mut symbols: BTreeMap<&str, Symbol<'_>> = BTreeMap::new();
    for (&path, (language, facts)) in inputs.files {
        for (i, def) in facts.definitions.iter().enumerate() {
            work::tick(1);
            let entry = entry_kinds(path, *language, facts, i);
            let rule = own_rule(inputs, path, i, !entry.is_empty());
            let s = symbols
                .entry(inputs.keys[path][i].as_str())
                .or_insert_with(|| Symbol {
                    name: &def.name,
                    entry: BTreeSet::new(),
                    rule: None,
                    declared: Vec::new(),
                });
            s.entry.extend(entry);
            s.rule = s.rule.or(rule);
            s.declared.push((path, def.name_span));
        }
    }
    // (b) for the query-time `endpoint` kind and (f): the edges into the symbols still eligible.
    let open: BTreeSet<NodeKey> = symbols
        .iter()
        .filter(|(_, s)| s.rule.is_none())
        .map(|(k, _)| NodeKey::new(NodeKind::Symbol, *k))
        .collect();
    let inbound = b.inbound(&open);
    let empty = Vec::new();
    for (key, s) in symbols.iter_mut().filter(|(_, s)| s.rule.is_none()) {
        let edges = inbound
            .get(&NodeKey::new(NodeKind::Symbol, *key))
            .unwrap_or(&empty);
        if edges.iter().any(|(k, _)| *k == EdgeKind::HandledBy) {
            s.rule = Some("entry");
            continue;
        }
        if !mentions_accounted(inputs.words, s, edges) {
            s.rule = Some("mention");
        }
    }

    let mut kept: BTreeMap<&str, usize> = RULES.iter().map(|r| (*r, 0)).collect();
    let mut eligible = 0;
    for (key, s) in &symbols {
        let node = NodeKey::new(NodeKind::Symbol, *key);
        if !s.entry.is_empty() {
            let kinds: Vec<Value> = s.entry.iter().map(|k| Value::from(*k)).collect();
            b.set_node_attr(&node, "entry", Value::Array(kinds));
        }
        match s.rule {
            Some(rule) => *kept.get_mut(rule).expect("a listed rule") += 1,
            None => {
                eligible += 1;
                b.set_node_attr(&node, "orphan_eligible", Value::Bool(true));
            }
        }
    }
    Diagnostic {
        kind: "orphan_coverage".into(),
        path: None,
        span: None,
        detail: json!({
            "symbols": symbols.len(),
            "eligible": eligible,
            "kept": kept,
        }),
    }
}

/// S12 (f): every occurrence of the symbol's name is one of its declarations or lies in the
/// evidence of an edge into it.
fn mentions_accounted(words: &Words, s: &Symbol<'_>, edges: &[(EdgeKind, Vec<Evidence>)]) -> bool {
    if words.unread || !scannable(s.name) || words.crowded.contains(s.name) {
        return false;
    }
    words
        .by_name
        .get(s.name)
        .into_iter()
        .flatten()
        .all(|&(file, at)| {
            work::tick(1);
            let path = words.paths[file].as_str();
            s.declared.iter().any(|&(p, span)| p == path && span == at)
                || edges
                    .iter()
                    .flat_map(|(_, ev)| ev)
                    .any(|ev| ev.path == path && within(at, ev.span))
        })
}

#[cfg(test)]
mod tests {
    use rb_model::{Attrs, Graph};

    use super::*;

    struct Run {
        graph: Graph,
        coverage: Value,
    }

    fn language_of(path: &str) -> Option<Language> {
        match path.rsplit('.').next() {
            Some("java") => Some(Language::Java),
            Some("ts") => Some(Language::TypeScript),
            Some("js") => Some(Language::JavaScript),
            Some("py") => Some(Language::Python),
            Some("go") => Some(Language::Go),
            _ => None,
        }
    }

    /// Indexes `files` (paths under `svc/` are a deployable service, under `lib/` a library);
    /// files of other extensions are only scanned as text. `over` names files over the cap.
    fn run_with(files: &[(&str, &str)], over: &[&str]) -> Run {
        run_hooked(files, over, &|_| {})
    }

    /// As [`run_with`], with `hook` adding edges before eligibility is decided.
    fn run_hooked(files: &[(&str, &str)], over: &[&str], hook: &dyn Fn(&mut GraphBuilder)) -> Run {
        let facts: Vec<(String, Language, FileFacts)> = files
            .iter()
            .filter_map(|(p, src)| {
                let l = language_of(p)?;
                Some((
                    (*p).to_owned(),
                    l,
                    rb_syntax::extract(l, src.as_bytes()).unwrap(),
                ))
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
        for (path, _) in files {
            b.add_node(NodeKey::new(NodeKind::File, *path), *path, Attrs::new())
                .unwrap();
        }
        for (path, (_, f)) in &by_path {
            crate::syntax::add_symbols(&mut b, path, f, &keys[path]).unwrap();
        }
        crate::resolve::add_edges(&mut b, &index, &BTreeSet::new()).unwrap();
        hook(&mut b);
        let names: HashSet<&str> = by_path
            .values()
            .flat_map(|(_, f)| f.definitions.iter().map(|d| d.name.as_str()))
            .collect();
        let words = scan_words(
            files.iter().map(|(p, src)| {
                let bytes = (!over.contains(p)).then_some(src.as_bytes());
                (*p, bytes)
            }),
            &names,
        );
        let service_of = |p: &str| p.split('/').next().unwrap_or_default().to_owned();
        let deployable = BTreeSet::from(["svc".to_owned()]);
        // The fragment interfaces as the pipeline computes them (DI's Spring Data beans).
        let (depends, unparsed) = (BTreeMap::new(), rb_frameworks::clients::RawScan::default());
        let empty_manifests: BTreeMap<&str, String> = BTreeMap::new();
        let empty_xml: BTreeMap<&str, String> = BTreeMap::new();
        let fragments = crate::di::resolve(&crate::di::Inputs {
            files: &by_path,
            keys: &keys,
            index: &index,
            service_of: &service_of,
            deployable: &deployable,
            depends: &depends,
            unparsed: &unparsed,
            manifests: &empty_manifests,
            xml: &empty_xml,
        })
        .fragments;
        let coverage = apply(
            &mut b,
            &Inputs {
                files: &by_path,
                keys: &keys,
                index: &index,
                service_of: &service_of,
                deployable: &deployable,
                words: &words,
                fragments: &fragments,
            },
        )
        .detail;
        Run {
            graph: b.finish().unwrap(),
            coverage,
        }
    }

    fn run(files: &[(&str, &str)]) -> Run {
        run_with(files, &[])
    }

    /// The attributes of the symbol whose key ends with `` `/<desc> ``: its entry kinds, and
    /// `eligible` when `orphan_eligible` is set.
    fn attrs(r: &Run, desc: &str) -> String {
        let n = r
            .graph
            .nodes()
            .iter()
            .find(|n| n.kind == NodeKind::Symbol && n.key.ends_with(&format!("`/{desc}")))
            .unwrap_or_else(|| panic!("no symbol {desc}"));
        let mut out: Vec<String> = n
            .attrs
            .get("entry")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|k| k.as_str().unwrap().to_owned())
            .collect();
        if n.attrs.get("orphan_eligible") == Some(&Value::Bool(true)) {
            out.push("eligible".into());
        }
        out.join(",")
    }

    const JOBS: &str = "package com.acme;
import org.springframework.scheduling.annotation.Scheduled;
import org.springframework.context.event.EventListener;
import org.springframework.kafka.annotation.KafkaListener;
import org.springframework.context.annotation.Bean;
import jakarta.annotation.PostConstruct;
import org.springframework.boot.CommandLineRunner;
public class Jobs {
  public static void main(String[] args) {}
  @Scheduled(fixedRate = 5) void tick() {}
  @EventListener void on(Object e) {}
  @KafkaListener(topics = \"t\") void consume(String m) {}
  @PostConstruct void init() {}
  @Bean Object factory() { return null; }
}
class Runner implements CommandLineRunner { public void run(String... a) {} }
class Varargs { public static void main(String... a) {} }
class NotMain { public void main(String[] a) {} }
class TwoArgs { public static void main(String[] a, int b) {} }
class ListMain { public static void main(java.util.List<String> a) {} }
class Decoys { @Scheduledx void a() {} @other.Scheduled void b() {} }
";

    /// S11: each stored kind, import-gated, with its decoys (a non-static `main`, a `@Test` of
    /// another package, an unimported `@Scheduled`, a `run` outside a runner).
    #[test]
    fn entry_kinds_are_import_gated() {
        let tests = "package com.acme;
import org.junit.jupiter.api.Test;
import org.junit.jupiter.params.ParameterizedTest;
class ATest { @Test void a() {} @ParameterizedTest void b() {} }
";
        let other = "package com.acme;
import org.other.Test;
class BTest { @Test void c() {} void run() {} }
class NotRunner implements CommandLineRunner { public void run(String... a) {} }
";
        let go_main = "package main\nfunc main() {}\nfunc helper() {}\n";
        let go_lib = "package util\nfunc main() {}\n";
        let go_plain = "package x\nfunc TestB(t *testing.T) {}\n";
        let go_test = "package x\nfunc TestA(t *testing.T) {}\nfunc Testable() {}\nfunc BenchmarkB(b *testing.B) {}\nfunc Example() {}\nfunc FuzzF(f *testing.F) {}\n";
        let py = "def test_one():\n    pass\ndef pyhelper():\n    pass\n";
        let r = run(&[
            ("svc/src/main/java/com/acme/Jobs.java", JOBS),
            ("svc/src/test/java/com/acme/ATest.java", tests),
            ("svc/src/test/java/com/acme/BTest.java", other),
            ("svc/cmd/main.go", go_main),
            ("svc/util/u.go", go_lib),
            ("svc/x/x_test.go", go_test),
            ("svc/x/x.go", go_plain),
            ("svc/tests/test_a.py", py),
        ]);
        for (desc, want) in [
            ("Jobs#main().", "main"),
            ("Jobs#tick().", "scheduled"),
            ("Jobs#on().", "listener"),
            ("Jobs#consume().", "listener"),
            ("Jobs#init().", "lifecycle"),
            ("Jobs#factory().", "lifecycle"),
            ("Runner#run().", "lifecycle"),
            ("Varargs#main().", "main"),
            ("NotMain#main().", ""),
            ("TwoArgs#main().", ""),
            ("ListMain#main().", ""),
            ("NotRunner#run().", ""),
            ("TestB().", ""),
            ("pyhelper().", ""),
            ("Decoys#a().", ""),
            ("Decoys#b().", ""),
            ("ATest#a().", "test"),
            ("ATest#b().", "test"),
            ("BTest#c().", ""),
            ("BTest#run().", ""),
            ("main().", "main"),
            ("helper().", ""),
            ("TestA().", "test"),
            ("Testable().", ""),
            ("BenchmarkB().", "test"),
            ("Example().", "test"),
            ("FuzzF().", "test"),
            ("test_one().", "test"),
        ] {
            let got = attrs(&r, desc);
            let kinds = got.trim_end_matches("eligible").trim_end_matches(',');
            assert_eq!(kinds, want, "{desc}: {got}");
        }
        // A Go `main` outside package `main` is no entry.
        let lib_main = r
            .graph
            .nodes()
            .iter()
            .find(|n| n.key.contains("`svc/util/u.go`/main()."))
            .unwrap();
        assert!(!lib_main.attrs.contains_key("entry"));
    }

    const SVC: &str = "package com.acme;
public class Svc {
  public void go() { legacyHash(\"x\"); }
  private String legacyHash(String s) { return s; }
}
";
    const MAIN: &str = "svc/src/main/java/com/acme/Svc.java";

    /// S12: the eligible row (a private helper called once) and one keeping row per rule.
    #[test]
    fn one_keeping_row_per_rule() {
        let r = run(&[(MAIN, SVC)]);
        assert_eq!(attrs(&r, "Svc#legacyHash()."), "eligible");
        assert_eq!(attrs(&r, "Svc#go()."), "eligible");
        assert_eq!(attrs(&r, "Svc#"), "eligible");
        assert_eq!(r.coverage["eligible"], 3);

        let with = |old: &str, new: &str| SVC.replace(old, new);
        let row = |files: &[(&str, &str)]| -> String {
            let r = run(files);
            attrs(&r, "Svc#legacyHash().")
        };
        // (f): a comment, a string, a same-named method elsewhere, a template.
        let comment = with(
            "public class Svc {",
            "// legacyHash is old\npublic class Svc {",
        );
        assert_eq!(row(&[(MAIN, &comment)]), "");
        let string = with("go() {", "go() { String n = \"legacyHash\";");
        assert_eq!(row(&[(MAIN, &string)]), "");
        let other = format!("{SVC}class Other {{ void legacyHash() {{}} }}\n");
        assert_eq!(row(&[(MAIN, &other)]), "");
        let template = "<p th:text=\"${svc.legacyHash('x')}\"></p>\n";
        assert_eq!(
            row(&[
                (MAIN, SVC),
                ("svc/src/main/resources/templates/a.html", template)
            ]),
            ""
        );
        // An occurrence in another file at the position of the call's evidence, or of the
        // declaration, is not accounted for by them.
        for token in ["legacyHash(\"x\")", "legacyHash(String"] {
            let at = SVC.find(token).unwrap();
            let line = SVC[..at].matches('\n').count();
            let col = at - SVC[..at].rfind('\n').map_or(0, |n| n + 1);
            let text = format!("{}{}legacyHash\n", "\n".repeat(line), " ".repeat(col));
            assert_eq!(row(&[(MAIN, SVC), ("svc/notes.txt", &text)]), "", "{token}");
        }
        // A file over the cap, a name seen too often.
        assert_eq!(
            row(&[(MAIN, SVC), ("svc/big.txt", "x")]),
            "eligible",
            "control"
        );
        let r = run_with(&[(MAIN, SVC), ("svc/big.txt", "x")], &["svc/big.txt"]);
        assert_eq!(r.coverage["kept"]["mention"], 3);
        let crowd = "legacyHash\n".repeat(CROWDED + 1);
        assert_eq!(row(&[(MAIN, SVC), ("svc/c.txt", &crowd)]), "");
        // (b): an entry kind keeps it.
        let scheduled = with(
            "public class Svc {",
            "import org.springframework.scheduling.annotation.Scheduled;\npublic class Svc {",
        )
        .replace(
            "private String legacyHash",
            "@Scheduled(fixedRate = 1) String legacyHash",
        );
        assert_eq!(row(&[(MAIN, &scheduled)]), "scheduled");
        // (c): its own annotation, its class's; an inert one keeps nothing.
        let cached = with(
            "private String legacyHash",
            "@Cacheable private String legacyHash",
        );
        let r = run(&[(MAIN, &cached)]);
        assert_eq!(attrs(&r, "Svc#legacyHash()."), "");
        assert_eq!(r.coverage["kept"]["annotation"], 1);
        let component = with(
            "public class Svc {",
            "import org.springframework.stereotype.Component;\n@Component public class Svc {",
        );
        assert_eq!(row(&[(MAIN, &component)]), "");
        let deprecated = with(
            "private String legacyHash",
            "@Deprecated private String legacyHash",
        );
        assert_eq!(row(&[(MAIN, &deprecated)]), "eligible");
        // (d): an accessor name; a Go method.
        let accessor = SVC.replace("legacyHash", "getEmail");
        let r = run(&[(MAIN, &accessor)]);
        assert_eq!(attrs(&r, "Svc#getEmail()."), "");
        assert_eq!(r.coverage["kept"]["convention"], 1);
        let go =
            "package svc\ntype S struct{}\nfunc (s *S) Do() {}\nfunc Use() { var s S; s.Do() }\n";
        let r = run(&[("svc/s.go", go)]);
        assert_eq!(r.coverage["kept"]["convention"], 1);
        // (e): a class implementing a library interface.
        let runnable = with(
            "public class Svc {",
            "public class Svc implements Runnable {",
        );
        let r = run(&[(MAIN, &runnable)]);
        assert_eq!(attrs(&r, "Svc#legacyHash()."), "");
        assert_eq!(r.coverage["kept"]["supertype"], 3);
        // (a): a library service; a test path.
        let r = run(&[("lib/src/main/java/com/acme/Svc.java", SVC)]);
        assert_eq!(r.coverage["kept"]["library"], 3);
        let r = run(&[("svc/src/test/java/com/acme/Svc.java", SVC)]);
        assert_eq!(r.coverage["kept"]["test"], 3);
        // A handler of an endpoint (the query-time `endpoint` kind).
        let r = run_hooked(&[(MAIN, SVC)], &[], &|b| {
            let ep = NodeKey::new(NodeKind::Endpoint, "HTTP GET /x @svc");
            b.add_node(ep.clone(), "GET /x", Attrs::new()).unwrap();
            let go = r#"rb-heuristic . . . `svc/src/main/java/com/acme/Svc.java`/Svc#go()."#;
            b.add_edge(rb_model::NewEdge::new(
                ep,
                EdgeKind::HandledBy,
                NodeKey::new(NodeKind::Symbol, go),
                rb_model::Provenance::Treesitter,
                rb_model::Certainty::Heuristic,
            ))
            .unwrap();
        });
        assert_eq!(attrs(&r, "Svc#go()."), "");
        assert_eq!(r.coverage["kept"]["entry"], 1);
        // A TS/JS default export; a TS constructor.
        let ts = "export default function helper() { return 1; }\nfunction local() { return 2; }\n";
        let r = run(&[("svc/a.ts", ts)]);
        assert_eq!(attrs(&r, "helper()."), "");
        assert_eq!(r.coverage["kept"]["default_export"], 1);
        assert_eq!(attrs(&r, "local()."), "eligible");
        let ctor = "export class K {\n  constructor() {}\n}\n";
        let r = run(&[("svc/k.ts", ctor)]);
        assert_eq!(attrs(&r, "K#"), "eligible");
        assert_eq!(
            r.coverage["kept"]["convention"].as_u64(),
            Some(r.coverage["symbols"].as_u64().unwrap() - 1)
        );
        // A name the scan cannot match whole.
        let js = "function $helper() { return 1; }\nexport function go() { return $helper(); }\n";
        let r = run(&[("svc/a.js", js)]);
        assert_eq!(attrs(&r, "$helper()."), "");
    }

    /// Evaluator pass 1, Y3: rule (e) follows the supertype chain through repository types,
    /// so an override of an external method through an in-repository base (depth 1 and 2), or
    /// through an in-repository interface extending an external one, is kept; a chain that
    /// stays in the repository keeps nothing.
    #[test]
    fn external_supertypes_through_repository_bases_keep_members() {
        const PROBE: &str = "package com.acme;\nclass Probe { boolean probe(AuthInterceptor i) { return i.preHandle(null, null, null); } }\n";
        let base = "package com.acme;\npublic abstract class BaseInterceptor implements org.springframework.web.servlet.HandlerInterceptor {}\n";
        let auth = "package com.acme;\npublic class AuthInterceptor extends BaseInterceptor {\n  public boolean preHandle(Object a, Object b, Object c) { return tokenPresent(a); }\n  private boolean tokenPresent(Object r) { return r != null; }\n}\n";
        fn files<'a>(base: &'a str, auth: &'a str) -> [(&'static str, &'a str); 3] {
            [
                ("svc/src/main/java/com/acme/BaseInterceptor.java", base),
                ("svc/src/main/java/com/acme/AuthInterceptor.java", auth),
                ("svc/src/main/java/com/acme/Probe.java", PROBE),
            ]
        }
        let r = run(&files(base, auth));
        assert_eq!(attrs(&r, "AuthInterceptor#preHandle()."), "");
        assert_eq!(attrs(&r, "AuthInterceptor#tokenPresent()."), "");
        // Depth 2: a middle class between.
        let mid = "package com.acme;\npublic abstract class BaseInterceptor extends Middle {}\nabstract class Middle implements org.springframework.web.servlet.HandlerInterceptor {}\n";
        let r = run(&files(mid, auth));
        assert_eq!(attrs(&r, "AuthInterceptor#preHandle()."), "");
        // An in-repository interface extending an external one.
        let iface = "package com.acme;\npublic interface BaseInterceptor extends org.springframework.context.ApplicationListener<Object> {}\n";
        let impl_ = "package com.acme;\npublic class AuthInterceptor implements BaseInterceptor {\n  public boolean preHandle(Object a, Object b, Object c) { return tokenPresent(a); }\n  private boolean tokenPresent(Object r) { return r != null; }\n}\n";
        let r = run(&files(iface, impl_));
        assert_eq!(attrs(&r, "AuthInterceptor#preHandle()."), "");
        // Control: a chain inside the repository.
        let local = "package com.acme;\npublic abstract class BaseInterceptor implements Local {}\ninterface Local {}\n";
        let r = run(&files(local, auth));
        assert_eq!(attrs(&r, "AuthInterceptor#tokenPresent()."), "eligible");
        // A supertype cycle inside the repository terminates (and keeps nothing).
        let cycle =
            "package com.acme;\npublic abstract class BaseInterceptor extends AuthInterceptor {}\n";
        let r = run(&files(cycle, auth));
        assert_eq!(attrs(&r, "AuthInterceptor#tokenPresent()."), "eligible");
        // A repository chain longer than the depth cap counts as external (conservative);
        // one within it does not.
        for (depth, want) in [(60, "eligible"), (70, "")] {
            let mut chain = String::from(
                "package com.acme;\npublic abstract class BaseInterceptor extends C1 {}\n",
            );
            for i in 1..depth {
                chain += &format!("abstract class C{i} extends C{} {{}}\n", i + 1);
            }
            chain += &format!("abstract class C{depth} {{}}\n");
            let r = run(&files(&chain, auth));
            assert_eq!(
                attrs(&r, "AuthInterceptor#tokenPresent()."),
                want,
                "{depth}"
            );
        }
    }

    /// Contract M5 S19 (d) (M4b open item 3): a class implementing, by name, an interface a
    /// Spring Data repository extends (a fragment, which Spring Data instantiates by its name)
    /// is not eligible, nor are its members; counted under `convention`. A class implementing
    /// another interface stays eligible.
    #[test]
    fn spring_data_fragment_implementations_are_not_orphans() {
        let repo = "package com.acme;\ninterface UserRepositoryCustom { void touch(long id); }\ninterface UserRepository extends org.springframework.data.jpa.repository.JpaRepository<User, Long>, UserRepositoryCustom {}\nclass User {}\n";
        let fragment = "package com.acme;\nclass UserRepositoryCustomImpl implements UserRepositoryCustom {\n  public void touch(long id) { audit(); }\n  private void audit() {}\n}\n";
        let other = "package com.acme;\ninterface Plain { void touch(long id); }\nclass PlainImpl implements Plain {\n  public void touch(long id) { record(); }\n  private void record() {}\n}\n";
        let r = run(&[
            ("svc/src/main/java/com/acme/R.java", repo),
            ("svc/src/main/java/com/acme/F.java", fragment),
            ("svc/src/main/java/com/acme/P.java", other),
        ]);
        assert_eq!(attrs(&r, "UserRepositoryCustomImpl#"), "");
        assert_eq!(attrs(&r, "UserRepositoryCustomImpl#audit()."), "");
        assert_eq!(attrs(&r, "PlainImpl#"), "eligible");
        assert_eq!(attrs(&r, "PlainImpl#record()."), "eligible");
        // Without the repository, the fragment interface is a plain one.
        let r = run(&[
            (
                "svc/src/main/java/com/acme/R.java",
                "package com.acme;\ninterface UserRepositoryCustom { void touch(long id); }\n",
            ),
            ("svc/src/main/java/com/acme/F.java", fragment),
        ]);
        assert_eq!(attrs(&r, "UserRepositoryCustomImpl#audit()."), "eligible");
    }

    /// Contract M5 S19 (e) (M4b open item 21): rule (e) trusts a supertype head's Tier B
    /// resolution only when it is unique and the declaring file neither imports that name from
    /// outside the repository (a library type the name may be) nor has an on-demand import of
    /// a package or module outside it; otherwise the head counts as external, at any depth of
    /// the chain. Imports from inside the repository, and a cycle, keep nothing.
    #[test]
    fn supertype_heads_resolved_by_name_are_external_unless_certain() {
        const PROBE: &str = "package com.acme;\nclass Probe { boolean probe(AuthInterceptor i) { return i.preHandle(null); } }\n";
        let local = "package com.acme;\npublic abstract class BaseInterceptor implements Local {}\ninterface Local {}\n";
        let auth = |imports: &str| {
            format!(
                "package com.acme;\n{imports}public class AuthInterceptor extends BaseInterceptor {{\n  public boolean preHandle(Object a) {{ return tokenPresent(a); }}\n  private boolean tokenPresent(Object r) {{ return r != null; }}\n}}\n"
            )
        };
        let row = |extra: &[(&str, &str)], auth: &str| -> String {
            let mut files = vec![
                ("svc/src/main/java/com/acme/BaseInterceptor.java", local),
                ("svc/src/main/java/com/acme/AuthInterceptor.java", auth),
                ("svc/src/main/java/com/acme/Probe.java", PROBE),
            ];
            files.extend_from_slice(extra);
            attrs(&run(&files), "AuthInterceptor#tokenPresent().")
        };
        let sub = (
            "svc/src/main/java/com/acme/sub/Helper.java",
            "package com.acme.sub;\npublic class Helper {}\n",
        );
        // Control: the head resolves to the same package, nothing imported.
        assert_eq!(row(&[], &auth("")), "eligible");
        // Predicted update (M6c S5/D5): the head resolves to the same package,
        // so an outside single import of the same name no longer counts.
        assert_eq!(
            row(
                &[],
                &auth("import org.springframework.web.servlet.BaseInterceptor;\n")
            ),
            "eligible"
        );
        // Predicted update (M6c S5/D5): same-package resolution beside an
        // on-demand import of a library package counts in-repo.
        assert_eq!(
            row(&[], &auth("import org.springframework.web.servlet.*;\n")),
            "eligible"
        );
        // Predicted update (M6c S5/D5): the single import misses, the
        // same-package level hits, so the head counts in-repo.
        assert_eq!(
            row(&[sub], &auth("import com.acme.sub.BaseInterceptor;\n")),
            "eligible"
        );
        // On-demand and single imports of repository packages are no doubt.
        assert_eq!(row(&[sub], &auth("import com.acme.sub.*;\n")), "eligible");
        assert_eq!(
            row(&[sub], &auth("import com.acme.sub.Helper;\n")),
            "eligible"
        );
        // Two repository types of that name: not unique.
        let twin = (
            "svc/src/main/java/com/acme/Twin.java",
            "package com.acme;\nabstract class BaseInterceptor implements Local {}\n",
        );
        assert_eq!(row(&[twin], &auth("")), "");
        // Depth 2: the middle head resolves to the same package, so the
        // predicted update (M6c S5/D5) keeps nothing doubtful: eligible.
        let mid = "package com.acme;\nimport org.springframework.web.servlet.Middle;\npublic abstract class BaseInterceptor extends Middle {}\n";
        let middle = (
            "svc/src/main/java/com/acme/Middle.java",
            "package com.acme;\nabstract class Middle implements Local {}\ninterface Local {}\n",
        );
        let mut files = vec![
            ("svc/src/main/java/com/acme/BaseInterceptor.java", mid),
            middle,
            ("svc/src/main/java/com/acme/Probe.java", PROBE),
        ];
        let plain = auth("");
        files.push(("svc/src/main/java/com/acme/AuthInterceptor.java", &plain));
        assert_eq!(
            attrs(&run(&files), "AuthInterceptor#tokenPresent()."),
            "eligible"
        );
        // A cycle with no doubtful head terminates and keeps nothing.
        let cycle =
            "package com.acme;\npublic abstract class BaseInterceptor extends AuthInterceptor {}\n";
        let files = [
            ("svc/src/main/java/com/acme/BaseInterceptor.java", cycle),
            (
                "svc/src/main/java/com/acme/AuthInterceptor.java",
                plain.as_str(),
            ),
            ("svc/src/main/java/com/acme/Probe.java", PROBE),
        ];
        assert_eq!(
            attrs(&run(&files), "AuthInterceptor#tokenPresent()."),
            "eligible"
        );
    }

    /// Contract M6c S5 (D5): a supertype head counts external only when its
    /// resolution actually reached an on-demand import. A unique same-package
    /// or single-import resolution counts in-repo even beside an outside
    /// binding of the same head; a unique on-demand resolution still counts
    /// external while an outside wildcard binds the head, and a miss stays
    /// external.
    #[test]
    fn supertype_heads_resolved_in_repo_count_in_repo() {
        const PROBE: &str = "package com.acme;\nclass Probe { boolean probe(AuthInterceptor i) { return i.preHandle(null); } }\n";
        let local = "package com.acme;\npublic abstract class BaseInterceptor implements Local {}\ninterface Local {}\n";
        let auth = |imports: &str| {
            format!(
                "package com.acme;\n{imports}public class AuthInterceptor extends BaseInterceptor {{\n  public boolean preHandle(Object a) {{ return tokenPresent(a); }}\n  private boolean tokenPresent(Object r) {{ return r != null; }}\n}}\n"
            )
        };
        let row = |extra: &[(&str, &str)], auth: &str| -> String {
            let mut files = vec![
                ("svc/src/main/java/com/acme/BaseInterceptor.java", local),
                ("svc/src/main/java/com/acme/AuthInterceptor.java", auth),
                ("svc/src/main/java/com/acme/Probe.java", PROBE),
            ];
            files.extend_from_slice(extra);
            attrs(&run(&files), "AuthInterceptor#tokenPresent().")
        };
        // Same-package resolution beside an outside single import: in-repo.
        assert_eq!(
            row(
                &[],
                &auth("import org.springframework.web.servlet.BaseInterceptor;\n")
            ),
            "eligible"
        );
        // Same-package resolution beside an outside on-demand import: in-repo.
        assert_eq!(
            row(&[], &auth("import org.springframework.web.servlet.*;\n")),
            "eligible"
        );
        // Single-import resolution beside an outside on-demand import: in-repo.
        // (`sub` holds a BaseInterceptor, so the single import wins.)
        let single = (
            "svc/src/main/java/com/acme/sub/Single.java",
            "package com.acme.sub;\npublic abstract class BaseInterceptor {}\n",
        );
        assert_eq!(
            row(
                &[single],
                &auth("import com.acme.sub.BaseInterceptor;\nimport org.foo.*;\n")
            ),
            "eligible"
        );
        // Unique on-demand resolution with an outside wildcard binding: external.
        // (No same-package BaseInterceptor here, so resolution reaches imports.)
        let sub = (
            "svc/src/main/java/com/acme/sub/Base.java",
            "package com.acme.sub;\npublic abstract class BaseInterceptor {}\n",
        );
        let r = run(&[
            (
                "svc/src/main/java/com/acme/AuthInterceptor.java",
                &auth("import com.acme.sub.*;\nimport org.foo.*;\n"),
            ),
            sub,
            ("svc/src/main/java/com/acme/Probe.java", PROBE),
        ]);
        assert_eq!(attrs(&r, "AuthInterceptor#tokenPresent()."), "");
        // Unique on-demand resolution beside an outside single binding (which
        // misses): in-repo — the single import names nothing.
        let r = run(&[
            (
                "svc/src/main/java/com/acme/AuthInterceptor.java",
                &auth("import com.acme.sub.*;\nimport org.foo.BaseInterceptor;\n"),
            ),
            sub,
            ("svc/src/main/java/com/acme/Probe.java", PROBE),
        ]);
        assert_eq!(attrs(&r, "AuthInterceptor#tokenPresent()."), "eligible");
        // A miss with an outside single binding stays external.
        assert_eq!(
            row(
                &[],
                &auth("import org.foo.Missing;\n")
                    .replace("extends BaseInterceptor", "extends Missing")
            ),
            ""
        );
    }

    /// Evaluator pass 1, Y1 and Y14: a nested class's binary name (`Outer$Inner`) in a
    /// `META-INF/services` file, a string or a `.txt` file mentions the nested class.
    #[test]
    fn binary_names_and_text_files_are_mentions() {
        let src =
            "package com.acme;\npublic class Holder {\n  public static class LoadedImpl { }\n}\n";
        let main = "svc/src/main/java/com/acme/Holder.java";
        let r = run(&[(main, src)]);
        assert_eq!(attrs(&r, "Holder#LoadedImpl#"), "eligible");
        for (path, text) in [
            (
                "svc/src/main/resources/META-INF/services/com.acme.Greeter",
                "com.acme.Holder$LoadedImpl\n",
            ),
            (
                "svc/src/main/java/com/acme/App.java",
                "package com.acme;\nclass App { Object o = load(\"com.acme.Holder$LoadedImpl\"); }\n",
            ),
            ("svc/plugins.txt", "Holder$LoadedImpl\n"),
            ("svc/run.py", "x = 'LoadedImpl'\n"),
        ] {
            let r = run(&[(main, src), (path, text)]);
            assert_eq!(attrs(&r, "Holder#LoadedImpl#"), "", "{path}");
        }
    }

    /// Invariant 11: every symbol is eligible or kept by exactly one rule.
    #[test]
    fn orphan_buckets_sum_to_the_symbols() {
        let r = run(&[
            ("svc/src/main/java/com/acme/Jobs.java", JOBS),
            (MAIN, SVC),
            (
                "lib/src/main/java/com/acme/L.java",
                "package com.acme;\nclass L {}\n",
            ),
        ]);
        let kept: u64 = r.coverage["kept"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_u64().unwrap())
            .sum();
        assert_eq!(
            kept + r.coverage["eligible"].as_u64().unwrap(),
            r.coverage["symbols"].as_u64().unwrap()
        );
        assert!(r.coverage["kept"]["entry"].as_u64() > Some(0));
    }

    fn work_for(n: usize) -> u64 {
        let mut src = String::from("package com.acme;\npublic class Svc {\n");
        for i in 0..n {
            src += &format!("  void go{i}() {{ helper{i}(); }}\n  private void helper{i}() {{}}\n");
        }
        src += "}\n";
        let text = format!("{}\n", "word ".repeat(n));
        let files = [(MAIN, src.as_str()), ("svc/notes.txt", text.as_str())];
        work::take();
        let r = run(&files);
        assert_eq!(r.coverage["eligible"].as_u64(), Some(2 * n as u64 + 1));
        work::take()
    }

    /// Invariant 9: the word scan and the eligibility check grow linearly (counted, 4× → ≤ 5×).
    #[test]
    fn eligibility_grows_linearly() {
        let (small, large) = (work_for(100), work_for(400));
        assert!(large <= small * 5, "{small} -> {large}");
    }

    /// Invariant 9: the level-tracked rule-(e) walk grows linearly (counted,
    /// 4× → ≤ 5×). Each head resolves twice (candidates + level), both walks
    /// first-win over the same levels.
    #[test]
    fn rule_e_level_walk_grows_linearly() {
        fn work_for(n: usize) -> u64 {
            let mut bases = String::from("package com.acme;\n");
            for i in 0..n {
                bases += &format!("class B{i} {{}}\n");
            }
            let mut owned: Vec<(String, String)> =
                vec![("svc/src/main/java/com/acme/Bases.java".to_owned(), bases)];
            for i in 0..n {
                owned.push((
                    format!("svc/src/main/java/com/acme/C{i}.java"),
                    format!(
                        "package com.acme;\nimport org.foo.B{i};\npublic class C{i} extends B{i} {{\n  private void h{i}() {{}}\n}}\n"
                    ),
                ));
            }
            let refs: Vec<(&str, &str)> = owned
                .iter()
                .map(|(p, s)| (p.as_str(), s.as_str()))
                .collect();
            work::take();
            let r = run(&refs);
            // Every head resolves in-repo, so the helper stays eligible: the
            // walk ran, the ratio below is not vacuous.
            assert_eq!(attrs(&r, "C0#h0()."), "eligible", "{n}");
            work::take()
        }
        let (small, large) = (work_for(25), work_for(100));
        assert!(large <= small * 5, "{small} -> {large}");
    }
}
