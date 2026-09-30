//! The SCIP⋈Tree-sitter join (design §5.3): a project's decoded index, matched by range against
//! the same files' syntax facts, gives exact symbol keys and exact `CALLS`, `REFERENCES` and
//! `IMPLEMENTS` edges. Tree-sitter supplies the syntactic role (a call, a type use, an
//! implementation) and the enclosing definition; SCIP supplies which symbol a name refers to.

use std::collections::{BTreeMap, BTreeSet};

use rb_model::{Evidence, Span};
use rb_scip::ScipIndex;
use rb_syntax::{FileFacts, Language, RefKind};

/// The languages an indexer covers: a file of one of them that its index has no document for
/// is a gap (`Coverage::files_without_document`); a file of any other language under the same
/// root was never the indexer's to cover.
pub(crate) fn indexed_languages(indexer: &str) -> &'static [Language] {
    match indexer {
        "scip-java" => &[Language::Java],
        "scip-typescript" => &[Language::TypeScript, Language::Tsx, Language::JavaScript],
        _ => &[],
    }
}

/// What one project's index contributes.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Joined {
    /// The files (repo paths) the index has a document for: the ones whose code edges are the
    /// exact ones below and nothing else (contract M3b invariant 1, amendment 15).
    pub documented: BTreeSet<String>,
    /// Per file, per definition index: the SCIP symbol that replaces its heuristic key.
    pub keys: BTreeMap<String, BTreeMap<usize, String>>,
    /// Exact edges; every target is a symbol whose definition matched, so the graph has its node.
    pub edges: Vec<ScipEdge>,
    /// External packages (design §5.5): per file and package, the first occurrence of a symbol
    /// of that package; sorted by file, then site.
    pub packages: Vec<PackageUse>,
    pub coverage: Coverage,
}

/// A file's first use of a symbol from a package the index does not define.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PackageUse {
    pub at: Evidence,
    pub package: rb_scip::Package,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ScipEdge {
    pub src: Source,
    pub kind: EdgeKind,
    pub dst: String,
    /// The reference site; for `Source::File` also the file that is the source.
    pub at: Evidence,
}

/// Where a reference sits: inside a definition (its key, SCIP when the definition matched, else
/// heuristic) or in top-level code, where the `File` node is the source.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Source {
    Symbol(String),
    File,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EdgeKind {
    Calls,
    References,
    Implements,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Coverage {
    /// Tree-sitter definitions matched to a SCIP definition occurrence (at their name, or
    /// through the by-name fallback below).
    pub definitions_matched: usize,
    /// The subset matched by [`definition_before_name`]: scip-java anchors a member's definition
    /// occurrence at the first text match of its name, which lands inside an annotation above
    /// the member when that text contains the name (contract M3b amendment 7).
    pub definitions_matched_by_name: usize,
    pub definitions_total: usize,
    /// Tree-sitter references matched to an occurrence of a symbol defined in the index.
    pub references_matched: usize,
    /// Matched an occurrence, but the symbol is defined outside the index (a dependency).
    pub references_external: usize,
    /// Matched an occurrence of a symbol of the project's own package that the index does not
    /// define (contract M3b amendment 23): a file of the project the indexer emitted no
    /// document for, or a member the compiler generates (a Java record's accessors), so there
    /// is nothing to point an edge at, and it is not a dependency.
    pub references_undocumented: usize,
    /// Matched an occurrence that is a document-local symbol (never a node).
    pub references_local: usize,
    /// Matched an occurrence of a symbol the index defines, but at a range Tree-sitter does not
    /// treat as a definition (a field, a constant), so the graph has no node for it.
    pub references_unanchored: usize,
    /// No occurrence at that range: SCIP has nothing to say, and nothing is guessed.
    pub references_unmatched: usize,
    /// Files of the indexer's languages the project owns that the indexer emitted no document
    /// for (a `tsconfig.json` that excludes `test/**`): they fall back to Tier B (contract M3b
    /// amendment 15), and none of their references is counted above.
    pub files_without_document: usize,
}

/// A document with facts: repo path, the document, its facts and its occurrences by span.
type Doc<'a> = (
    String,
    &'a rb_scip::Document,
    &'a FileFacts,
    BTreeMap<Span, Vec<&'a rb_scip::Occurrence>>,
);

/// Joins `index` (paths relative to `root`) with the facts of the files the project owns, keyed
/// by repo path, whose heuristic keys are `keys[path]`; `languages` are the ones the indexer
/// covers ([`indexed_languages`]).
pub(crate) fn join(
    index: &ScipIndex,
    root: &str,
    files: &BTreeMap<&str, (Language, &FileFacts)>,
    keys: &BTreeMap<&str, Vec<String>>,
    languages: &[Language],
) -> Joined {
    let repo_path = |doc: &str| {
        if root.is_empty() {
            doc.to_owned()
        } else {
            format!("{root}/{doc}")
        }
    };
    // Every global symbol the index defines, so references to dependencies are told apart.
    let defined: BTreeSet<&str> = index
        .documents
        .iter()
        .flat_map(|d| &d.occurrences)
        .filter(|o| o.is_definition() && !o.symbol.starts_with("local "))
        .map(|o| o.symbol.as_str())
        .collect();
    // The packages the index itself belongs to (`None` when its symbols carry none): a symbol of
    // one of them that the index does not define (a file of the project without a document;
    // scip-typescript's `node_modules/.prisma/…` under the project's own package name) is not
    // a dependency.
    let own_packages: BTreeSet<Option<rb_scip::Package>> =
        defined.iter().map(|s| rb_scip::package(s)).collect();
    let mut out = Joined::default();
    // The documents with facts, each with its occurrences by span.
    let docs: Vec<Doc<'_>> = index
        .documents
        .iter()
        .filter_map(|doc| {
            let path = repo_path(&doc.path);
            let (_, facts) = *files.get(path.as_str())?;
            let by_span = doc.occurrences.iter().fold(
                BTreeMap::<Span, Vec<&rb_scip::Occurrence>>::new(),
                |mut m, o| {
                    m.entry(o.span).or_default().push(o);
                    m
                },
            );
            Some((path, doc, facts, by_span))
        })
        .collect();
    out.documented = docs.iter().map(|(path, ..)| path.clone()).collect();
    out.coverage.files_without_document = files
        .iter()
        .filter(|(path, (language, _))| {
            languages.contains(language) && !out.documented.contains(**path)
        })
        .count();
    // Definitions first, over every document: the name's range is a definition occurrence of a
    // global symbol. The matched symbols are the only targets an edge may have, since only they
    // get `Symbol` nodes.
    for (path, _, facts, by_span) in &docs {
        let mut replaced: BTreeMap<usize, String> = BTreeMap::new();
        for (i, d) in facts.definitions.iter().enumerate() {
            out.coverage.definitions_total += 1;
            let at_name = by_span
                .get(&d.name_span)
                .and_then(|os| os.iter().find(|o| is_global_definition(o)));
            let (hit, by_name) = match at_name {
                Some(o) => (Some(*o), false),
                None => (definition_before_name(d, by_span), true),
            };
            if let Some(o) = hit {
                out.coverage.definitions_matched += 1;
                out.coverage.definitions_matched_by_name += usize::from(by_name);
                replaced.insert(i, o.symbol.clone());
            }
        }
        if !replaced.is_empty() {
            out.keys.insert(path.clone(), replaced);
        }
    }
    let nodes: BTreeSet<&str> = out
        .keys
        .values()
        .flat_map(|r| r.values().map(String::as_str))
        .collect();
    // The project's type definitions, `(document, index)`, for the `IMPLEMENTS` direction rule
    // below: by SCIP symbol (the matched ones; a walk's exact endpoint) and by simple name (all
    // of them; what a declared supertype resolves to).
    let mut types_by_symbol: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
    let mut types_by_name: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
    for (di, (path, _, facts, _)) in docs.iter().enumerate() {
        let replaced = out.keys.get(path);
        for (i, d) in facts.definitions.iter().enumerate() {
            if !is_type(d) {
                continue;
            }
            types_by_name
                .entry(d.name.as_str())
                .or_default()
                .push((di, i));
            if let Some(symbol) = replaced.and_then(|r| r.get(&i)) {
                types_by_symbol
                    .entry(symbol.as_str())
                    .or_default()
                    .push((di, i));
            }
        }
    }
    let mut implements: Vec<ScipEdge> = Vec::new();
    for (di, (path, doc, facts, by_span)) in docs.iter().enumerate() {
        let heuristic = &keys[path.as_str()];
        let replaced = out.keys.get(path);
        let source = |enclosing: Option<usize>| match enclosing {
            Some(i) => Source::Symbol(
                replaced
                    .and_then(|r| r.get(&i))
                    .cloned()
                    .unwrap_or_else(|| heuristic[i].clone()),
            ),
            None => Source::File,
        };
        // References: the name's range is a non-definition occurrence.
        for r in &facts.references {
            let kind = match r.kind {
                RefKind::Call => EdgeKind::Calls,
                RefKind::Class | RefKind::Type => EdgeKind::References,
                RefKind::Implementation => continue,
            };
            let Some(o) = by_span
                .get(&r.span)
                .and_then(|os| os.iter().find(|o| !o.is_definition()))
            else {
                out.coverage.references_unmatched += 1;
                continue;
            };
            let symbol = o.symbol.as_str();
            if symbol.starts_with("local ") {
                out.coverage.references_local += 1;
                continue;
            }
            if !defined.contains(symbol) {
                if own_packages.contains(&rb_scip::package(symbol)) {
                    out.coverage.references_undocumented += 1;
                } else {
                    out.coverage.references_external += 1;
                }
                continue;
            }
            if !nodes.contains(symbol) {
                out.coverage.references_unanchored += 1;
                continue;
            }
            out.coverage.references_matched += 1;
            out.edges.push(ScipEdge {
                src: source(r.enclosing),
                kind,
                dst: symbol.to_owned(),
                at: Evidence {
                    path: path.clone(),
                    span: r.span,
                },
            });
        }
        // Packages: every occurrence of a global symbol the index does not define, whether or
        // not Tree-sitter captured it (imports and annotations are not references); the first
        // site per package is the evidence.
        let mut first: BTreeMap<rb_scip::Package, Span> = BTreeMap::new();
        for (span, os) in by_span {
            for o in os {
                if o.symbol.starts_with("local ") || defined.contains(o.symbol.as_str()) {
                    continue;
                }
                let package = rb_scip::package(&o.symbol);
                if own_packages.contains(&package) {
                    continue;
                }
                if let Some(package) = package {
                    first.entry(package).or_insert(*span);
                }
            }
        }
        out.packages
            .extend(first.into_iter().map(|(package, span)| PackageUse {
                at: Evidence {
                    path: path.clone(),
                    span,
                },
                package,
            }));
        // Implementations come from symbol information, anchored at the definition's name, in
        // the one direction the syntax supports (contract M3b amendments 16 and 21): scip-java
        // 0.13.1 emits `is_implementation` on both sides of an override inside the workspace,
        // and relates an override to the method it implements however many levels up, so the
        // source's type holder must reach the target's type through its declared supertypes.
        let Some(replaced) = replaced else {
            continue;
        };
        for s in &doc.symbols {
            let Some((i, _)) = replaced.iter().find(|(_, sym)| **sym == s.symbol) else {
                continue;
            };
            let holder = (di, type_holder(facts, *i));
            for rel in s.relationships.iter().filter(|r| r.is_implementation) {
                if !nodes.contains(rel.symbol.as_str()) {
                    continue;
                }
                let targets = rb_scip::type_symbol_of(&rel.symbol)
                    .and_then(|type_symbol| types_by_symbol.get(type_symbol.as_str()));
                let supported = targets.is_some_and(|targets| {
                    reaches_supertype(&docs, &types_by_name, holder, targets)
                });
                if supported {
                    implements.push(ScipEdge {
                        src: Source::Symbol(s.symbol.clone()),
                        kind: EdgeKind::Implements,
                        dst: rel.symbol.clone(),
                        at: Evidence {
                            path: path.clone(),
                            span: facts.definitions[*i].name_span,
                        },
                    });
                }
            }
        }
    }
    out.edges.extend(implements);
    out.edges.sort_by(|a, b| {
        (&a.src, a.kind as u8, &a.dst, &a.at).cmp(&(&b.src, b.kind as u8, &b.dst, &b.at))
    });
    out.edges.dedup();
    out.packages.sort();
    out
}

fn is_global_definition(o: &rb_scip::Occurrence) -> bool {
    o.is_definition() && !o.symbol.starts_with("local ")
}

/// The definition whose supertypes decide an `IMPLEMENTS` direction: the definition itself when
/// it is a type, else (a method, a constant) the nearest enclosing type; as an index into
/// `facts.definitions`.
fn type_holder(facts: &FileFacts, mut i: usize) -> usize {
    while matches!(
        facts.definitions[i].kind,
        rb_syntax::DefKind::Function | rb_syntax::DefKind::Method | rb_syntax::DefKind::Constant
    ) && let Some(parent) = facts.definitions[i].parent
    {
        i = parent;
    }
    i
}

/// A definition a declared supertype can name.
fn is_type(d: &rb_syntax::Definition) -> bool {
    matches!(
        d.kind,
        rb_syntax::DefKind::Class | rb_syntax::DefKind::Interface | rb_syntax::DefKind::Type
    )
}

/// Whether `from` (a definition, `(document, index)` into `docs`) declares one of `targets` as
/// a supertype, directly or through the supertypes of its supertypes (contract M3b amendment
/// 21). A declared name resolves by its simple form (`a.b.Foo` and `Foo<T>` both name `Foo`) to
/// every type definition of the project with that name, and the walk accepts only when it
/// reaches one of `targets` itself: a type that has no definition in the project (an external
/// supertype) satisfies nothing, however it is named. Which of two same-named project types a
/// declaration means is not resolved here; the index's relationship is exact, so the walk only
/// decides its direction. A visited set and a depth cap bound the walk, since same-named types
/// can make the declared graph cyclic.
fn reaches_supertype(
    docs: &[Doc<'_>],
    types_by_name: &BTreeMap<&str, Vec<(usize, usize)>>,
    from: (usize, usize),
    targets: &[(usize, usize)],
) -> bool {
    const MAX_DEPTH: usize = 64;
    let mut visited = BTreeSet::from([from]);
    let mut frontier = vec![from];
    for _ in 0..MAX_DEPTH {
        let mut next = Vec::new();
        for (di, i) in frontier {
            for s in &docs[di].2.definitions[i].supertypes {
                let name = s.name.rsplit('.').next().unwrap_or(&s.name);
                for &t in types_by_name.get(name).map_or(&[][..], Vec::as_slice) {
                    if targets.contains(&t) {
                        return true;
                    }
                    if visited.insert(t) {
                        next.push(t);
                    }
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    false
}

/// The fallback for a definition with no definition occurrence at its name (contract M3b
/// amendment 7): scip-java anchors a member's definition occurrence at the first text match of
/// the member's name from the top of the member, so `@PostMapping("/login") … String login(…)`
/// gets its occurrence inside the annotation's string. Accepted is a definition occurrence of
/// a global symbol whose last descriptor is the definition's name and whose span lies inside
/// the definition, before its name (its modifiers and annotations); nothing outside those
/// bounds is considered, so the join still invents no range (invariant 6).
fn definition_before_name<'a>(
    d: &rb_syntax::Definition,
    by_span: &BTreeMap<Span, Vec<&'a rb_scip::Occurrence>>,
) -> Option<&'a rb_scip::Occurrence> {
    let start: Span = [d.span[0], d.span[1], i32::MIN, i32::MIN];
    by_span
        .range(start..d.name_span)
        .filter(|(span, _)| (span[2], span[3]) <= (d.name_span[0], d.name_span[1]))
        .flat_map(|(_, os)| os.iter().copied())
        .find(|o| {
            is_global_definition(o)
                && rb_scip::last_descriptor_name(&o.symbol).as_deref() == Some(d.name.as_str())
        })
}

#[cfg(test)]
mod tests {
    use rb_scip::{Document, Occurrence, Relationship, SymbolInfo};
    use rb_syntax::Language;

    use super::*;

    const JAVA: &str = "package p;\nclass Animal { void sound() {} }\nclass Dog extends Animal {\n  void sound() { helper(); ext(); local(); }\n  void helper() {}\n}\n";

    /// Definition occurrences at every Tree-sitter definition name, plus references. The
    /// `is_implementation` relationships are symmetric, as scip-java 0.13.1 emits them for an
    /// override inside the workspace (both the method and the type level).
    fn index(facts: &FileFacts) -> ScipIndex {
        let sym = |name: &str| format!("scip-java maven . . p/{name}");
        let mut occurrences = Vec::new();
        for d in &facts.definitions {
            let symbol = match d.name.as_str() {
                "Animal" => sym("Animal#"),
                "Dog" => sym("Dog#"),
                "sound" if d.span[0] == 1 => sym("Animal#sound()."),
                "sound" => sym("Dog#sound()."),
                "helper" => sym("Dog#helper()."),
                _ => continue,
            };
            occurrences.push(Occurrence {
                span: d.name_span,
                symbol,
                roles: 1,
                enclosing: None,
            });
        }
        for r in &facts.references {
            let symbol = match r.name.as_str() {
                "helper" => sym("Dog#helper()."),
                "ext" => "scip-java maven maven/org.ext/lib 1.0 e/Ext#ext().".to_owned(),
                "local" => "local 3".to_owned(),
                "Animal" => sym("Animal#"),
                _ => continue,
            };
            occurrences.push(Occurrence {
                span: r.span,
                symbol,
                roles: 0,
                enclosing: None,
            });
        }
        ScipIndex {
            documents: vec![Document {
                path: "src/Dog.java".into(),
                occurrences,
                symbols: [
                    ("Dog#sound().", "Animal#sound()."),
                    ("Animal#sound().", "Dog#sound()."),
                    ("Dog#", "Animal#"),
                    ("Animal#", "Dog#"),
                ]
                .map(|(from, to)| SymbolInfo {
                    symbol: sym(from),
                    relationships: vec![Relationship {
                        symbol: sym(to),
                        is_implementation: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                })
                .to_vec(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn matched_definitions_get_scip_keys_and_references_become_exact_edges() {
        let facts = rb_syntax::extract(Language::Java, JAVA.as_bytes()).unwrap();
        let path = "svc/src/Dog.java";
        let heuristic = rb_syntax::symbol_keys(path, &facts);
        let files = BTreeMap::from([(path, (Language::Java, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> = BTreeMap::from([(path, heuristic)]);
        let joined = join(&index(&facts), "svc", &files, &keys, &[Language::Java]);
        assert_eq!(joined.documented, BTreeSet::from([path.to_owned()]));
        let replaced: Vec<(String, String)> = joined.keys[path]
            .iter()
            .map(|(i, s)| (facts.definitions[*i].name.clone(), s.clone()))
            .collect();
        assert_eq!(
            replaced,
            [
                ("Animal".into(), "scip-java maven . . p/Animal#".into()),
                (
                    "sound".into(),
                    "scip-java maven . . p/Animal#sound().".into()
                ),
                ("Dog".into(), "scip-java maven . . p/Dog#".into()),
                ("sound".into(), "scip-java maven . . p/Dog#sound().".into()),
                (
                    "helper".into(),
                    "scip-java maven . . p/Dog#helper().".into()
                ),
            ]
        );
        // Only the direction the syntax supports (`Dog extends Animal`) gives `IMPLEMENTS`; the
        // relationships the index carries the other way round (contract M3b amendment 16) do not.
        assert_eq!(
            edge_lines(&joined),
            [
                "scip-java maven . . p/Dog# References scip-java maven . . p/Animal#",
                "scip-java maven . . p/Dog# Implements scip-java maven . . p/Animal#",
                "scip-java maven . . p/Dog#sound(). Calls scip-java maven . . p/Dog#helper().",
                "scip-java maven . . p/Dog#sound(). Implements scip-java maven . . p/Animal#sound().",
            ]
        );
        assert_eq!(
            joined.coverage,
            Coverage {
                definitions_matched: 5,
                definitions_matched_by_name: 0,
                definitions_total: 5,
                references_matched: 2,
                references_external: 1,
                references_undocumented: 0,
                references_local: 1,
                references_unanchored: 0,
                references_unmatched: 0,
                files_without_document: 0,
            }
        );
        // The external call names a package (design §5.5), at its site.
        let ext = facts.references.iter().find(|r| r.name == "ext").unwrap();
        assert_eq!(
            joined.packages,
            [PackageUse {
                at: Evidence {
                    path: path.into(),
                    span: ext.span,
                },
                package: rb_scip::package("scip-java maven maven/org.ext/lib 1.0 e/Ext#ext().")
                    .unwrap(),
            }]
        );
    }

    /// Packages come from every occurrence of an undefined global symbol, captured by
    /// Tree-sitter or not (an import, an annotation): one use per file and package, at the first
    /// site. Symbols of the index's own package, namespace-only symbols and locals name none.
    #[test]
    fn packages_are_the_first_use_per_file_of_each_undefined_global_symbols_package() {
        let src =
            "import { Client } from 'lib';\nexport function f(): Client { return other(); }\n";
        let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
        let path = "src/a.ts";
        let at = |token: &str| -> Span {
            let line = src.lines().position(|l| l.contains(token)).unwrap();
            let col = src.lines().nth(line).unwrap().find(token).unwrap();
            let (line, col) = (line as i32, col as i32);
            [line, col, line, col + token.len() as i32]
        };
        let occ = |span: Span, symbol: &str, roles: i32| Occurrence {
            span,
            symbol: symbol.into(),
            roles,
            enclosing: None,
        };
        let f = facts.definitions.iter().find(|d| d.name == "f").unwrap();
        let lib = "scip-typescript npm lib 2.0.0 `index.d.ts`/";
        let index = ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences: vec![
                    // The import binding (no Tree-sitter reference) is the first site.
                    occ(at("Client }"), &format!("{lib}Client#"), 2),
                    occ(at("'lib'"), lib, 0),
                    occ(
                        f.name_span,
                        "scip-typescript npm p 1.0.0 src/`a.ts`/f().",
                        1,
                    ),
                    occ(at("Client {"), &format!("{lib}Client#"), 0),
                    // The project's own package, undefined here: not a dependency.
                    occ(
                        at("other"),
                        "scip-typescript npm p 1.0.0 node_modules/`x.d.ts`/other().",
                        0,
                    ),
                    occ(at("return"), "scip-java maven . . org/springframework/", 0),
                    occ(at("import"), "local 1", 0),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let files = BTreeMap::from([(path, (Language::TypeScript, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> =
            BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
        let joined = join(&index, "", &files, &keys, &[Language::TypeScript]);
        assert_eq!(
            joined.packages,
            [PackageUse {
                at: Evidence {
                    path: path.into(),
                    span: at("Client }"),
                },
                package: rb_scip::Package {
                    manager: "npm".into(),
                    name: "lib".into(),
                    version: "2.0.0".into(),
                },
            }]
        );
    }

    /// scip-java's annotation-anchored definitions (contract M3b amendment 7): the occurrence
    /// inside `@Mapping("/login")` matches the `login` method below it, and is counted as a
    /// by-name match; a same-named occurrence outside the method, or one for another name
    /// inside its annotations, matches nothing.
    #[test]
    fn a_definition_occurrence_inside_the_annotations_matches_by_name() {
        let src = "package p;\nclass C {\n  @Mapping(\"/login\")\n  public String login() { return \"\"; }\n  @Mapping(\"/logout\")\n  public String logout() { return \"\"; }\n}\n";
        let facts = rb_syntax::extract(Language::Java, src.as_bytes()).unwrap();
        let path = "svc/src/C.java";
        let login = facts
            .definitions
            .iter()
            .find(|d| d.name == "login")
            .unwrap();
        assert!(
            login.span[0] < login.name_span[0],
            "the method's span starts at its annotation: {:?} vs {:?}",
            login.span,
            login.name_span
        );
        let sym = |d: &str| format!("scip-java maven maven/p/svc 1.0 p/C#{d}");
        let at = |line: usize, token: &str| -> Span {
            let col = src.lines().nth(line).unwrap().find(token).unwrap() as i32;
            let line = line as i32;
            [line, col, line, col + token.len() as i32]
        };
        let def = |span: Span, symbol: String| Occurrence {
            span,
            symbol,
            roles: 1,
            enclosing: None,
        };
        let class_name = facts.definitions.iter().find(|d| d.name == "C").unwrap();
        let in_login_annotation = at(2, "login");
        let in_logout_annotation = at(4, "logout");
        let index = |occurrences: Vec<Occurrence>| ScipIndex {
            documents: vec![Document {
                path: "src/C.java".into(),
                occurrences,
                ..Default::default()
            }],
            ..Default::default()
        };
        let files = BTreeMap::from([(path, (Language::Java, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> =
            BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
        let java = &[Language::Java];

        // The occurrences as scip-java emits them: `login` anchored in its annotation, `logout`
        // as well (its annotation's text contains it too); a same-named occurrence at the top
        // of the file lies outside both methods.
        let joined = join(
            &index(vec![
                def([0, 8, 0, 9], sym("login().")),
                def(class_name.name_span, sym("")),
                def(in_login_annotation, sym("login().")),
                def(in_logout_annotation, sym("logout().")),
            ]),
            "svc",
            &files,
            &keys,
            java,
        );
        let replaced: Vec<(&str, &str)> = joined.keys[path]
            .iter()
            .map(|(i, s)| (facts.definitions[*i].name.as_str(), s.as_str()))
            .collect();
        assert_eq!(
            replaced,
            [
                ("C", "scip-java maven maven/p/svc 1.0 p/C#"),
                ("login", "scip-java maven maven/p/svc 1.0 p/C#login()."),
                ("logout", "scip-java maven maven/p/svc 1.0 p/C#logout()."),
            ]
        );
        assert_eq!(
            (
                joined.coverage.definitions_matched,
                joined.coverage.definitions_matched_by_name,
                joined.coverage.definitions_total
            ),
            (3, 2, 3)
        );

        // Swapped names: the occurrence inside `login`'s annotation names `logout` and vice
        // versa, so neither matches; the class still does, at its name.
        let joined = join(
            &index(vec![
                def(class_name.name_span, sym("")),
                def(in_login_annotation, sym("logout().")),
                def(in_logout_annotation, sym("login().")),
            ]),
            "svc",
            &files,
            &keys,
            java,
        );
        assert_eq!(joined.keys[path].len(), 1);
        assert_eq!(
            (
                joined.coverage.definitions_matched,
                joined.coverage.definitions_matched_by_name
            ),
            (1, 0)
        );
        // A local symbol inside the annotations is never a match.
        let joined = join(
            &index(vec![def(in_login_annotation, "local 1".into())]),
            "svc",
            &files,
            &keys,
            java,
        );
        assert!(joined.keys.is_empty());
    }

    /// A synthetic scip-java index over several Java files: every class or interface gets
    /// `<package>/<Name>#` and every method `<package>/<Type>#<name>().`, each with a definition
    /// occurrence at its name; `relationships` are `is_implementation` links between those
    /// symbols, listed in the document that defines the source (scip-java's own layout; the
    /// tests list both directions where it would). Returns the `Implements` edges, without
    /// the symbol prefix.
    fn hierarchy(files: &[(&str, &str)], relationships: &[(&str, &str)]) -> Vec<String> {
        let prefix = "scip-java maven . . ";
        let facts: Vec<(String, FileFacts)> = files
            .iter()
            .map(|(path, src)| {
                (
                    (*path).to_owned(),
                    rb_syntax::extract(Language::Java, src.as_bytes()).unwrap(),
                )
            })
            .collect();
        let symbol_of = |f: &FileFacts, d: &rb_syntax::Definition| -> Option<String> {
            let package = f.package.as_deref().unwrap_or("p").replace('.', "/");
            let suffix = match d.kind {
                rb_syntax::DefKind::Class | rb_syntax::DefKind::Interface => {
                    format!("{package}/{}#", d.name)
                }
                rb_syntax::DefKind::Method => {
                    let parent = &f.definitions[d.parent?];
                    format!("{package}/{}#{}().", parent.name, d.name)
                }
                _ => return None,
            };
            Some(format!("{prefix}{suffix}"))
        };
        let documents = facts
            .iter()
            .map(|(path, f)| Document {
                path: path.clone(),
                occurrences: f
                    .definitions
                    .iter()
                    .filter_map(|d| {
                        Some(Occurrence {
                            span: d.name_span,
                            symbol: symbol_of(f, d)?,
                            roles: 1,
                            enclosing: None,
                        })
                    })
                    .collect(),
                symbols: relationships
                    .iter()
                    .filter(|(from, _)| {
                        f.definitions
                            .iter()
                            .any(|d| symbol_of(f, d).as_deref() == Some(&format!("{prefix}{from}")))
                    })
                    .map(|(from, to)| SymbolInfo {
                        symbol: format!("{prefix}{from}"),
                        relationships: vec![Relationship {
                            symbol: format!("{prefix}{to}"),
                            is_implementation: true,
                            ..Default::default()
                        }],
                        ..Default::default()
                    })
                    .collect(),
                ..Default::default()
            })
            .collect();
        let index = ScipIndex {
            documents,
            ..Default::default()
        };
        let by_path: BTreeMap<&str, (Language, &FileFacts)> = facts
            .iter()
            .map(|(p, f)| (p.as_str(), (Language::Java, f)))
            .collect();
        let keys: BTreeMap<&str, Vec<String>> = facts
            .iter()
            .map(|(p, f)| (p.as_str(), rb_syntax::symbol_keys(p, f)))
            .collect();
        let joined = join(&index, "", &by_path, &keys, &[Language::Java]);
        edge_lines(&joined)
            .into_iter()
            .filter(|l| l.contains(" Implements "))
            .map(|l| l.replace(prefix, ""))
            .collect()
    }

    /// Contract M3b amendment 21: `IMPLEMENTS` is accepted through the hierarchy the syntax
    /// declares, not only through a direct supertype. scip-java relates an override to the
    /// method it implements however many levels up (`Svc#m()` to `Base#m()` through the
    /// interface `Mid` and the abstract class `AbstractSvc`; `Bottom#run()` to `Top#run()`
    /// through `Middle`), on both sides, so the reverse is still rejected at every level.
    #[test]
    fn implements_is_accepted_through_declared_supertypes_transitively() {
        let files = [
            ("Base.java", "package p;\ninterface Base { void m(); }\n"),
            ("Mid.java", "package p;\ninterface Mid extends Base {}\n"),
            (
                "AbstractSvc.java",
                "package p;\nabstract class AbstractSvc {}\n",
            ),
            (
                "Svc.java",
                "package p;\nclass Svc extends AbstractSvc implements Mid { public void m() {} }\n",
            ),
        ];
        let both = |a: &'static str, b: &'static str| [(a, b), (b, a)];
        let rels: Vec<(&str, &str)> = [
            both("p/Svc#m().", "p/Base#m()."),
            both("p/Svc#", "p/Base#"),
            both("p/Svc#", "p/Mid#"),
            both("p/Svc#", "p/AbstractSvc#"),
            both("p/Mid#", "p/Base#"),
        ]
        .concat();
        assert_eq!(
            hierarchy(&files, &rels),
            [
                "p/Mid# Implements p/Base#",
                "p/Svc# Implements p/AbstractSvc#",
                "p/Svc# Implements p/Base#",
                "p/Svc# Implements p/Mid#",
                "p/Svc#m(). Implements p/Base#m().",
            ]
        );

        let files = [
            ("Top.java", "package p;\nclass Top { void run() {} }\n"),
            ("Middle.java", "package p;\nclass Middle extends Top {}\n"),
            (
                "Bottom.java",
                "package p;\nclass Bottom extends Middle { void run() {} }\n",
            ),
        ];
        let rels: Vec<(&str, &str)> = [
            both("p/Bottom#run().", "p/Top#run()."),
            both("p/Bottom#", "p/Top#"),
            both("p/Bottom#", "p/Middle#"),
            both("p/Middle#", "p/Top#"),
        ]
        .concat();
        assert_eq!(
            hierarchy(&files, &rels),
            [
                "p/Bottom# Implements p/Middle#",
                "p/Bottom# Implements p/Top#",
                "p/Bottom#run(). Implements p/Top#run().",
                "p/Middle# Implements p/Top#",
            ]
        );
    }

    /// The walk resolves a declared supertype by simple name and accepts only when it reaches
    /// the target's own type definition: with an unrelated `p.h.Base` next to the `p.other.Base`
    /// that `Svc` implements, the index's links to `p.other.Base` are accepted, and links the
    /// index would carry on `p.h.Base`'s side (whose declarations lead nowhere) are rejected.
    /// Which of two same-named types the syntax means is not decided here: the index is exact,
    /// so it never relates `Svc` to `p.h.Base`. A generic supertype (`Handler<String>`) names
    /// `Handler`.
    #[test]
    fn implements_walk_ends_at_the_targets_own_type_and_reads_generic_supertypes() {
        let files = [
            (
                "h/Base.java",
                "package p.h;\npublic interface Base { void m(); }\n",
            ),
            (
                "other/Base.java",
                "package p.other;\npublic interface Base { void m(); }\n",
            ),
            (
                "Svc.java",
                "package p;\nimport p.other.Base;\nclass Svc implements Base { public void m() {} }\n",
            ),
        ];
        let rels = [
            ("p/Svc#m().", "p/other/Base#m()."),
            ("p/other/Base#m().", "p/Svc#m()."),
            ("p/Svc#", "p/other/Base#"),
            ("p/other/Base#", "p/Svc#"),
            ("p/h/Base#m().", "p/Svc#m()."),
            ("p/h/Base#", "p/Svc#"),
        ];
        assert_eq!(
            hierarchy(&files, &rels),
            [
                "p/Svc# Implements p/other/Base#",
                "p/Svc#m(). Implements p/other/Base#m().",
            ]
        );

        let files = [
            (
                "Handler.java",
                "package p;\ninterface Handler<T> { void handle(T t); }\n",
            ),
            (
                "StringHandler.java",
                "package p;\nclass StringHandler implements Handler<String> { public void handle(String s) {} }\n",
            ),
        ];
        let rels = [
            ("p/StringHandler#handle().", "p/Handler#handle()."),
            ("p/Handler#handle().", "p/StringHandler#handle()."),
            ("p/StringHandler#", "p/Handler#"),
            ("p/Handler#", "p/StringHandler#"),
        ];
        assert_eq!(
            hierarchy(&files, &rels),
            [
                "p/StringHandler# Implements p/Handler#",
                "p/StringHandler#handle(). Implements p/Handler#handle().",
            ]
        );
    }

    /// Contract M3b amendment 23: a reference to a symbol of the project's own package that the
    /// index does not define (a file of the project the indexer emitted no document for) counts
    /// as `references_undocumented`, not as external; a dependency's symbol stays external and
    /// still names its package.
    #[test]
    fn a_reference_into_an_undocumented_file_of_the_project_is_not_external() {
        let src = "import { g } from './b';\nimport { ext } from 'lib';\nexport function f() { g(); ext(); }\n";
        let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
        let path = "src/a.ts";
        let at = |token: &str, len: usize| -> Span {
            let line = src.lines().position(|l| l.contains(token)).unwrap();
            let col = src.lines().nth(line).unwrap().find(token).unwrap();
            let (line, col) = (line as i32, col as i32);
            [line, col, line, col + len as i32]
        };
        let occ = |span: Span, symbol: &str, roles: i32| Occurrence {
            span,
            symbol: symbol.into(),
            roles,
            enclosing: None,
        };
        let f = facts.definitions.iter().find(|d| d.name == "f").unwrap();
        let index = ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences: vec![
                    occ(
                        f.name_span,
                        "scip-typescript npm p 1.0.0 src/`a.ts`/f().",
                        1,
                    ),
                    occ(
                        at("g();", 1),
                        "scip-typescript npm p 1.0.0 src/`b.ts`/g().",
                        0,
                    ),
                    occ(
                        at("ext();", 3),
                        "scip-typescript npm lib 2.0.0 `index.d.ts`/ext().",
                        0,
                    ),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let files = BTreeMap::from([(path, (Language::TypeScript, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> =
            BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
        let joined = join(&index, "", &files, &keys, &[Language::TypeScript]);
        assert!(joined.edges.is_empty());
        assert_eq!(
            joined.coverage,
            Coverage {
                definitions_matched: 1,
                definitions_matched_by_name: 0,
                definitions_total: 1,
                references_matched: 0,
                references_external: 1,
                references_undocumented: 1,
                references_local: 0,
                references_unanchored: 0,
                references_unmatched: 0,
                files_without_document: 0,
            }
        );
        let packages: Vec<String> = joined.packages.iter().map(|p| p.package.key()).collect();
        assert_eq!(packages, ["npm/lib@2.0.0"]);
    }

    /// The walk is bounded. A declared cycle (`A extends B`, `B extends A`: the compiler rejects
    /// it, the parser does not) ends without reaching an unrelated `C`; a chain is followed
    /// for 64 levels and no further.
    #[test]
    fn implements_walk_terminates_on_a_cycle_and_stops_at_its_depth_cap() {
        let files = [
            ("A.java", "package p;\nclass A extends B {}\n"),
            ("B.java", "package p;\nclass B extends A {}\n"),
            ("C.java", "package p;\nclass C {}\n"),
        ];
        let rels = [("p/A#", "p/C#"), ("p/A#", "p/B#"), ("p/B#", "p/A#")];
        assert_eq!(
            hierarchy(&files, &rels),
            ["p/A# Implements p/B#", "p/B# Implements p/A#"]
        );

        // `C0 extends C1 extends … extends C65`: `C64` is 64 levels above `C0`, `C65` is 65.
        let chain: Vec<(String, String)> = (0..=65)
            .map(|i| {
                let extends = if i < 65 {
                    format!(" extends C{}", i + 1)
                } else {
                    String::new()
                };
                (
                    format!("C{i}.java"),
                    format!("package p;\nclass C{i}{extends} {{}}\n"),
                )
            })
            .collect();
        let files: Vec<(&str, &str)> = chain
            .iter()
            .map(|(p, s)| (p.as_str(), s.as_str()))
            .collect();
        let rels = [
            ("p/C0#", "p/C64#"),
            ("p/C64#", "p/C0#"),
            ("p/C0#", "p/C65#"),
        ];
        assert_eq!(hierarchy(&files, &rels), ["p/C0# Implements p/C64#"]);
    }

    fn edge_lines(joined: &Joined) -> Vec<String> {
        joined
            .edges
            .iter()
            .map(|e| {
                let src = match &e.src {
                    Source::Symbol(k) => k.clone(),
                    Source::File => format!("file {}", e.at.path),
                };
                format!("{src} {:?} {}", e.kind, e.dst)
            })
            .collect()
    }

    /// Top-level code has the file as its source; a symbol the index defines at a range
    /// Tree-sitter does not (a constant) is no target, and counts as unanchored.
    #[test]
    fn top_level_calls_come_from_the_file_and_unanchored_symbols_get_no_edge() {
        let src = "export function f() {}\nexport const k = 1;\nf();\nk();\n";
        let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
        let path = "src/a.ts";
        let sym = |d: &str| format!("scip-typescript npm p 1.0.0 src/`a.ts`/{d}");
        let at = |token: &str| -> Span {
            let line = src.lines().position(|l| l.contains(token)).unwrap();
            let col = src.lines().nth(line).unwrap().find(token).unwrap();
            let (line, col) = (line as i32, col as i32);
            [line, col, line, col + 1]
        };
        let occ = |span: Span, symbol: String, roles: i32| Occurrence {
            span,
            symbol,
            roles,
            enclosing: None,
        };
        let index = ScipIndex {
            documents: vec![Document {
                path: "src/a.ts".into(),
                occurrences: vec![
                    occ(at("f() {}"), sym("f()."), 1),
                    occ(at("k = 1"), sym("k."), 1),
                    occ(at("f();"), sym("f()."), 0),
                    occ(at("k();"), sym("k."), 0),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let files = BTreeMap::from([(path, (Language::TypeScript, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> =
            BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
        let joined = join(&index, "", &files, &keys, &[Language::TypeScript]);
        assert_eq!(
            joined.keys[path],
            BTreeMap::from([(0, sym("f()."))]),
            "only the function is a Tree-sitter definition"
        );
        assert_eq!(
            edge_lines(&joined),
            [format!("file src/a.ts Calls {}", sym("f()."))]
        );
        assert_eq!(joined.coverage.references_matched, 1);
        assert_eq!(joined.coverage.references_unanchored, 1);
        assert_eq!(joined.coverage.references_unmatched, 0);
    }

    #[test]
    fn files_outside_the_facts_and_unmatched_ranges_are_skipped() {
        let facts = rb_syntax::extract(Language::Java, JAVA.as_bytes()).unwrap();
        let path = "svc/src/Dog.java";
        let heuristic = rb_syntax::symbol_keys(path, &facts);
        let files = BTreeMap::from([(path, (Language::Java, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> = BTreeMap::from([(path, heuristic.clone())]);
        let java = &[Language::Java];
        // A document for a file we have no facts for, and an index with offset ranges.
        let mut idx = index(&facts);
        idx.documents[0].path = "src/Other.java".into();
        let joined = join(&idx, "svc", &files, &keys, java);
        assert!(joined.keys.is_empty() && joined.edges.is_empty());
        assert!(
            joined.documented.is_empty(),
            "no document for a file with facts"
        );
        // The file we do have facts for has no document: counted, and nothing else is. A file
        // of a language the indexer does not cover is not a gap (contract M3b amendment 15).
        assert_eq!(joined.coverage.files_without_document, 1);
        assert_eq!(joined.coverage.definitions_total, 0);
        assert_eq!(
            join(&idx, "svc", &files, &keys, &[Language::Python])
                .coverage
                .files_without_document,
            0
        );
        assert_eq!(
            join(&index(&facts), "svc", &files, &keys, java)
                .coverage
                .files_without_document,
            0
        );
        let mut shifted = index(&facts);
        for o in &mut shifted.documents[0].occurrences {
            o.span[1] += 1;
        }
        let joined = join(&shifted, "svc", &files, &keys, java);
        assert!(joined.keys.is_empty());
        assert_eq!(joined.coverage.definitions_matched, 0);
        assert_eq!(joined.coverage.references_unmatched, 4);
    }
}

/// Debugging aid, not an assertion: with `RB_SCIP_SAMPLE_DIR=<dir>` holding an `index.scip` and
/// the sources it was built from, prints the join's coverage and a sample of keys and edges.
#[cfg(test)]
mod sample {
    use std::collections::BTreeMap;

    use super::*;

    #[test]
    fn print_real_index_join() {
        let Ok(dir) = std::env::var("RB_SCIP_SAMPLE_DIR") else {
            return;
        };
        let bytes = std::fs::read(format!("{dir}/index.scip")).unwrap();
        let index = rb_scip::decode(&bytes, &|p| std::fs::read(format!("{dir}/{p}")).ok()).unwrap();
        println!(
            "tool={} root={} documents={}",
            index.tool,
            index.project_root,
            index.documents.len()
        );
        let mut facts_by_path: BTreeMap<String, (Language, FileFacts)> = BTreeMap::new();
        for d in &index.documents {
            let language = match d.path.rsplit('.').next() {
                Some("ts") => Language::TypeScript,
                Some("tsx") => Language::Tsx,
                Some("js") => Language::JavaScript,
                Some("java") => Language::Java,
                _ => continue,
            };
            let Ok(src) = std::fs::read(format!("{dir}/{}", d.path)) else {
                continue;
            };
            facts_by_path.insert(
                d.path.clone(),
                (language, rb_syntax::extract(language, &src).unwrap()),
            );
            println!(
                "  {} occurrences={} symbols={} encoding_assumed={} unconverted={}",
                d.path,
                d.occurrences.len(),
                d.symbols.len(),
                d.encoding_assumed,
                d.unconverted
            );
        }
        let files: BTreeMap<&str, (Language, &FileFacts)> = facts_by_path
            .iter()
            .map(|(p, (l, f))| (p.as_str(), (*l, f)))
            .collect();
        let keys: BTreeMap<&str, Vec<String>> = facts_by_path
            .iter()
            .map(|(p, (_, f))| (p.as_str(), rb_syntax::symbol_keys(p, f)))
            .collect();
        let languages: Vec<Language> = files.values().map(|(l, _)| *l).collect();
        let joined = join(&index, "", &files, &keys, &languages);
        println!("coverage={:?}", joined.coverage);
        for (path, replaced) in &joined.keys {
            for (i, s) in replaced {
                println!(
                    "  key {path} {} -> {s}",
                    files[path.as_str()].1.definitions[*i].name
                );
            }
        }
        for e in joined.edges.iter().take(20) {
            println!("  edge {:?} {:?} {}", e.src, e.kind, e.dst);
        }
        for d in &index.documents {
            for o in d.occurrences.iter().take(6) {
                println!(
                    "  occ {} {:?} roles={} {}",
                    d.path, o.span, o.roles, o.symbol
                );
            }
        }
    }
}
