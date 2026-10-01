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
    /// Definitions whose definition occurrence is a document-local symbol (scip-java's enum
    /// constant bodies, scip-typescript's nested functions and classes; contract M3b amendment
    /// 54): they keep their heuristic keys, and occurrences and relationships naming the local
    /// symbol in the same document resolve to them. Not part of `definitions_matched`.
    pub definitions_local: usize,
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
    /// Declared supertypes of the documented type definitions with no type occurrence inside
    /// their clause item (contract M3b amendment 26): the `IMPLEMENTS` walk stops there, and a
    /// lost true edge is preferred to a false exact one.
    pub supertypes_unresolved: usize,
    /// Occurrences of the documented files whose columns stayed in code units because the
    /// decoder had no line text to convert them with (contract M3b amendment 33); non-zero
    /// means the spans above may be off on non-ASCII lines.
    pub occurrences_unconverted: usize,
    /// `is_implementation` relationships that gave no edge because their source or target
    /// symbol has no matched project definition (a local class expression, a field, an
    /// external type; contract M3b amendment 52). Relationships the direction rule rejects
    /// (the reverse side of an override) are not dropped: their forward twin is the edge.
    pub relationships_dropped: usize,
    /// The subset of `relationships_dropped` whose source and target are both defined in this
    /// project: each has a definition occurrence in the index's documents (a local one in the
    /// relationship's own document) on the name line of a same-named Tree-sitter definition
    /// (contract M3b amendment 54). Non-zero means the join failed to anchor a definition both
    /// sides know, not that the link leaves the project.
    pub relationships_dropped_in_project: usize,
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
    // Each document's local symbols by name (`SymbolInformation.display_name`): a local
    // symbol's string carries none.
    let local_names: Vec<BTreeMap<&str, &str>> = docs
        .iter()
        .map(|(_, doc, ..)| {
            doc.symbols
                .iter()
                .filter(|s| s.symbol.starts_with("local "))
                .map(|s| (s.symbol.as_str(), s.display_name.as_str()))
                .collect()
        })
        .collect();
    // Definitions first, over every document: the name's range is a definition occurrence of a
    // global symbol. The matched symbols are the only targets an edge may have, since only they
    // get `Symbol` nodes. A definition occurrence of a local symbol at the name binds that
    // symbol, within its document, to the definition, which keeps its heuristic key (contract
    // M3b amendment 54). Exact anchors at the name come before the by-name fallback.
    let mut locals: Vec<BTreeMap<&str, usize>> = vec![BTreeMap::new(); docs.len()];
    for (di, (path, _, facts, by_span)) in docs.iter().enumerate() {
        let mut replaced: BTreeMap<usize, String> = BTreeMap::new();
        for (i, d) in facts.definitions.iter().enumerate() {
            out.coverage.definitions_total += 1;
            let at_name = |accept: fn(&rb_scip::Occurrence) -> bool| {
                by_span
                    .get(&d.name_span)
                    .and_then(|os| os.iter().copied().find(|o| accept(o)))
            };
            let global_named = |o: &rb_scip::Occurrence| {
                is_global_definition(o)
                    && rb_scip::last_descriptor_name(&o.symbol).as_deref() == Some(d.name.as_str())
            };
            let local_named = |o: &rb_scip::Occurrence| {
                is_local_definition(o)
                    && local_names[di].get(o.symbol.as_str()) == Some(&d.name.as_str())
            };
            let global_at_name = at_name(is_global_definition);
            let local_at_name = global_at_name
                .is_none()
                .then(|| at_name(is_local_definition))
                .flatten();
            let global_by_name = (global_at_name.is_none() && local_at_name.is_none())
                .then(|| definition_before_name(d, by_span, &global_named))
                .flatten();
            if let Some(o) = global_at_name.or(global_by_name) {
                out.coverage.definitions_matched += 1;
                out.coverage.definitions_matched_by_name += usize::from(global_by_name.is_some());
                replaced.insert(i, o.symbol.clone());
                continue;
            }
            let local = local_at_name.or_else(|| definition_before_name(d, by_span, &local_named));
            if let Some(o) = local {
                out.coverage.definitions_local += 1;
                locals[di].entry(o.symbol.as_str()).or_insert(i);
            }
        }
        if !replaced.is_empty() {
            out.keys.insert(path.clone(), replaced);
        }
    }
    // Where each symbol is defined, for telling a dropped relationship inside the project from
    // one that leaves it: per document, the Tree-sitter definitions' `(name line, name)`; the
    // global definition occurrences' `(document, line)`; each document's local ones' lines.
    let name_lines: Vec<BTreeSet<(i32, &str)>> = docs
        .iter()
        .map(|(_, _, facts, _)| {
            facts
                .definitions
                .iter()
                .map(|d| (d.name_span[0], d.name.as_str()))
                .collect()
        })
        .collect();
    let mut global_sites: BTreeMap<&str, Vec<(usize, i32)>> = BTreeMap::new();
    let mut local_sites: Vec<BTreeMap<&str, Vec<i32>>> = vec![BTreeMap::new(); docs.len()];
    for (di, (_, doc, ..)) in docs.iter().enumerate() {
        for o in doc.occurrences.iter().filter(|o| o.is_definition()) {
            if o.symbol.starts_with("local ") {
                local_sites[di]
                    .entry(o.symbol.as_str())
                    .or_default()
                    .push(o.span[0]);
            } else {
                global_sites
                    .entry(o.symbol.as_str())
                    .or_default()
                    .push((di, o.span[0]));
            }
        }
    }
    // Whether `symbol`, named in document `di`, is defined in the project (see
    // `Coverage::relationships_dropped_in_project`).
    let in_project = |symbol: &str, di: usize| -> bool {
        if symbol.starts_with("local ") {
            let Some(name) = local_names[di].get(symbol) else {
                return false;
            };
            local_sites[di]
                .get(symbol)
                .is_some_and(|lines| lines.iter().any(|&l| name_lines[di].contains(&(l, *name))))
        } else {
            let Some(name) = rb_scip::last_descriptor_name(symbol) else {
                return false;
            };
            global_sites.get(symbol).is_some_and(|sites| {
                sites
                    .iter()
                    .any(|&(dj, l)| name_lines[dj].contains(&(l, name.as_str())))
            })
        }
    };
    let nodes: BTreeSet<&str> = out
        .keys
        .values()
        .flat_map(|r| r.values().map(String::as_str))
        .collect();
    // The project's type definitions, `(document, index)`, by SCIP symbol (the matched ones:
    // a walk's exact endpoint, and what a declared supertype resolves to), and each one's
    // declared supertypes resolved through the index's own occurrences (contract M3b
    // amendment 26).
    let mut types_by_symbol: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
    let mut supertypes: BTreeMap<(usize, usize), Vec<String>> = BTreeMap::new();
    for (di, (path, doc, facts, by_span)) in docs.iter().enumerate() {
        out.coverage.occurrences_unconverted += doc.unconverted;
        let replaced = out.keys.get(path);
        for (i, d) in facts.definitions.iter().enumerate() {
            if !is_type(d) {
                continue;
            }
            if let Some(symbol) = replaced.and_then(|r| r.get(&i)) {
                types_by_symbol
                    .entry(symbol.as_str())
                    .or_default()
                    .push((di, i));
            }
            let declared = declared_supertypes(d, by_span);
            out.coverage.supertypes_unresolved += declared.iter().filter(|s| s.is_none()).count();
            supertypes.insert((di, i), declared.into_iter().flatten().collect());
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
                // A local symbol bound to a definition of this document (amendment 54) is an
                // exact target with that definition's heuristic key.
                match locals[di].get(symbol) {
                    Some(&i) => {
                        out.coverage.references_matched += 1;
                        out.edges.push(ScipEdge {
                            src: source(r.enclosing),
                            kind,
                            dst: heuristic[i].clone(),
                            at: Evidence {
                                path: path.clone(),
                                span: r.span,
                            },
                        });
                    }
                    None => out.coverage.references_local += 1,
                }
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
        // the one direction the syntax supports (contract M3b amendments 16, 21 and 26):
        // scip-java 0.13.1 emits `is_implementation` on both sides of an override inside the
        // workspace, and relates an override to the method it implements however many levels
        // up, so the source's type holder must reach the target's type through its declared
        // supertypes.
        // Several declarations can share one symbol (TypeScript overload signatures): the
        // first one is the definition the edge is anchored at. A local symbol bound in this
        // document stands for its definition, under that definition's heuristic key.
        let mut definition_of: BTreeMap<&str, usize> = BTreeMap::new();
        for (i, symbol) in replaced.into_iter().flatten() {
            definition_of.entry(symbol.as_str()).or_insert(*i);
        }
        let key_of = |symbol: &str| -> Option<(usize, String)> {
            match locals[di].get(symbol) {
                Some(&i) => Some((i, heuristic[i].clone())),
                None => definition_of.get(symbol).map(|&i| (i, symbol.to_owned())),
            }
        };
        for s in &doc.symbols {
            let implementations = s.relationships.iter().filter(|r| r.is_implementation);
            let mut dropped = |rel: &rb_scip::Relationship| {
                out.coverage.relationships_dropped += 1;
                out.coverage.relationships_dropped_in_project +=
                    usize::from(in_project(&s.symbol, di) && in_project(&rel.symbol, di));
            };
            // A source without a matched definition (a local class expression, a field) has no
            // node to start an edge from: its relationships are dropped and counted.
            let Some((i, source_key)) = key_of(&s.symbol) else {
                implementations.for_each(&mut dropped);
                continue;
            };
            let source_local = s.symbol.starts_with("local ");
            let holder = (di, type_holder(facts, i));
            for rel in implementations {
                let target_local = rel.symbol.starts_with("local ");
                let target = if target_local {
                    locals[di]
                        .get(rel.symbol.as_str())
                        .map(|&j| heuristic[j].clone())
                } else {
                    nodes
                        .contains(rel.symbol.as_str())
                        .then(|| rel.symbol.clone())
                };
                let Some(target) = target else {
                    dropped(rel);
                    continue;
                };
                // A local type is never a supertype of a global one (an enum constant body, an
                // anonymous or a local class cannot be named outside its scope), so `local →
                // global` is the forward side and `global → local` its reverse twin; between two
                // locals the syntax cannot be consulted (no type symbol), so the link is dropped
                // (amendment 54). Between globals the source's declared supertypes decide.
                let supported = match (source_local, target_local) {
                    (true, false) => true,
                    (false, true) => false,
                    (true, true) => {
                        dropped(rel);
                        continue;
                    }
                    (false, false) => rb_scip::type_symbol_of(&rel.symbol)
                        .and_then(|type_symbol| types_by_symbol.get(type_symbol.as_str()))
                        .is_some_and(|targets| {
                            reaches_supertype(&supertypes, &types_by_symbol, holder, targets)
                        }),
                };
                if supported {
                    implements.push(ScipEdge {
                        src: Source::Symbol(source_key.clone()),
                        kind: EdgeKind::Implements,
                        dst: target,
                        at: Evidence {
                            path: path.clone(),
                            span: facts.definitions[i].name_span,
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

fn is_local_definition(o: &rb_scip::Occurrence) -> bool {
    o.is_definition() && o.symbol.starts_with("local ")
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

/// The SCIP symbol each declared supertype of `d` names, resolved through the index's own
/// occurrences (contract M3b amendments 26 and 43): the last reference occurrence inside the
/// supertype's name span whose symbol is a type (its descriptor ends with `#`). The name span
/// holds the qualified name only, so a qualifier's namespace occurrence (`a/` in `a.Circle`) is
/// passed over as not a type, an outer type's occurrence gives way to the nested type after it
/// (`Outer#` then `Outer#Inner#` for `Outer.Inner`; scip-typescript's `NS#`, `NS/` then
/// `NS/Inner#` for a namespace-merged class), and type arguments (`Marker.Disc` in
/// `Marker<Marker.Disc>`) and type-use annotations lie outside the span. The chosen symbol
/// must name the written type: its last descriptor is the supertype's simple name (`Inner` for
/// `a.b.Outer.Inner`; contract M3b amendment 53), so an occurrence that an indexer's column
/// error moved into the span (scip-java's tab expansion put `Doer#` inside `Yyyyyyyy`) is not
/// taken for it. `None` when the span holds no such type occurrence: the indexer did not say
/// which type is meant, and nothing is guessed.
fn declared_supertypes(
    d: &rb_syntax::Definition,
    by_span: &BTreeMap<Span, Vec<&rb_scip::Occurrence>>,
) -> Vec<Option<String>> {
    d.supertypes
        .iter()
        .map(|s| {
            let start: Span = [s.span[0], s.span[1], i32::MIN, i32::MIN];
            let end: Span = [s.span[2], s.span[3], i32::MAX, i32::MAX];
            let simple_name = s.name.rsplit('.').next().unwrap_or(&s.name);
            by_span
                .range(start..=end)
                .filter(|(span, _)| (span[2], span[3]) <= (s.span[2], s.span[3]))
                .flat_map(|(_, os)| os.iter().copied())
                .rfind(|o| {
                    !o.is_definition() && !o.symbol.starts_with("local ") && o.symbol.ends_with('#')
                })
                .filter(|o| {
                    rb_scip::last_descriptor_name(&o.symbol).as_deref() == Some(simple_name)
                })
                .map(|o| o.symbol.clone())
        })
        .collect()
}

/// Whether `from` (a type definition, `(document, index)`) declares one of `targets` as a
/// supertype, directly or through the supertypes of its supertypes (contract M3b amendments 21
/// and 26). `supertypes` holds each type definition's declared supertypes as the SCIP symbols
/// the index's occurrences resolved them to ([`declared_supertypes`]); a symbol leads on only
/// when it is one of the project's matched type definitions (`types_by_symbol`), so an
/// external or unresolved supertype ends that path. The walk accepts only when it reaches one
/// of `targets` itself; the index's relationship is exact, so the walk only decides its
/// direction. A visited set and a depth cap bound it.
fn reaches_supertype(
    supertypes: &BTreeMap<(usize, usize), Vec<String>>,
    types_by_symbol: &BTreeMap<&str, Vec<(usize, usize)>>,
    from: (usize, usize),
    targets: &[(usize, usize)],
) -> bool {
    const MAX_DEPTH: usize = 64;
    let mut visited = BTreeSet::from([from]);
    let mut frontier = vec![from];
    for _ in 0..MAX_DEPTH {
        let mut next = Vec::new();
        for at in frontier {
            for symbol in supertypes.get(&at).map_or(&[][..], Vec::as_slice) {
                for &t in types_by_symbol
                    .get(symbol.as_str())
                    .map_or(&[][..], Vec::as_slice)
                {
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
/// gets its occurrence inside the annotation's string. Accepted is a definition occurrence that
/// `names_d` accepts as naming the definition (a global symbol whose last descriptor is its
/// name; a local symbol whose display name is; amendment 54) and whose span lies inside the
/// definition, before its name (its modifiers and annotations); nothing outside those bounds is
/// considered, so the join still invents no range (invariant 6).
fn definition_before_name<'a>(
    d: &rb_syntax::Definition,
    by_span: &BTreeMap<Span, Vec<&'a rb_scip::Occurrence>>,
    names_d: &dyn Fn(&rb_scip::Occurrence) -> bool,
) -> Option<&'a rb_scip::Occurrence> {
    let start: Span = [d.span[0], d.span[1], i32::MIN, i32::MIN];
    by_span
        .range(start..d.name_span)
        .filter(|(span, _)| (span[2], span[3]) <= (d.name_span[0], d.name_span[1]))
        .flat_map(|(_, os)| os.iter().copied())
        .find(|o| names_d(o))
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
                definitions_local: 0,
                definitions_total: 5,
                references_matched: 2,
                references_external: 1,
                references_undocumented: 0,
                references_local: 1,
                references_unanchored: 0,
                references_unmatched: 0,
                files_without_document: 0,
                supertypes_unresolved: 0,
                occurrences_unconverted: 0,
                relationships_dropped: 0,
                relationships_dropped_in_project: 0,
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

    /// A synthetic index over several files of one language: every class or interface gets a
    /// type symbol (`<package>/<Name>#` for Java, `src/\`<file>\`/<Name>#` for TypeScript) and
    /// every method `<Type>#<name>().`, each with a definition occurrence at its name; each
    /// declared supertype gets the reference occurrences the real indexers emit inside its
    /// clause item (contract M3b amendment 26): the type symbol at the type's name, preceded by
    /// a namespace occurrence at each qualifier (`a/` at `a` in `a.Circle`). A supertype name
    /// resolves as the compiler would: qualified names by their package, simple names in the
    /// file's own package, then through its imports, then to the one type of that name.
    /// `relationships` are `is_implementation` links between those symbols, listed in the
    /// document that defines the source (scip-java's own layout; the tests list both directions
    /// where it would). Returns the `Implements` edges, without the symbol prefix.
    fn hierarchy(files: &[(&str, &str)], relationships: &[(&str, &str)]) -> Vec<String> {
        hierarchy_in(Language::Java, files, relationships)
    }

    fn hierarchy_in(
        language: Language,
        files: &[(&str, &str)],
        relationships: &[(&str, &str)],
    ) -> Vec<String> {
        implements_lines(
            language,
            &hierarchy_joined(language, files, relationships, &[]),
        )
    }

    /// The `Implements` edges of a join, without the symbol prefix.
    fn implements_lines(language: Language, joined: &Joined) -> Vec<String> {
        edge_lines(joined)
            .into_iter()
            .filter(|l| l.contains(" Implements "))
            .map(|l| l.replace(symbol_prefix(language), ""))
            .collect()
    }

    fn symbol_prefix(language: Language) -> &'static str {
        match language {
            Language::Java => "scip-java maven . . ",
            _ => "scip-typescript npm t1 1.0.0 ",
        }
    }

    /// A type's symbol: nested types carry their outer type (`b/Marker#Disc#`), methods their
    /// type (`b/Marker#Disc#area().`).
    fn symbol_in(
        prefix: &str,
        scope: &str,
        f: &FileFacts,
        d: &rb_syntax::Definition,
    ) -> Option<String> {
        let outer = d.parent.map(|p| &f.definitions[p]);
        match d.kind {
            rb_syntax::DefKind::Class | rb_syntax::DefKind::Interface => {
                match outer.and_then(|o| symbol_in(prefix, scope, f, o)) {
                    Some(outer) => Some(format!("{outer}{}#", d.name)),
                    None => Some(format!("{prefix}{scope}/{}#", d.name)),
                }
            }
            rb_syntax::DefKind::Method => {
                let outer = symbol_in(prefix, scope, f, outer?)?;
                Some(format!("{outer}{}().", d.name))
            }
            _ => None,
        }
    }

    /// The join of the synthetic index described at [`hierarchy`], plus `extra` reference
    /// occurrences `(path, span, symbol without the prefix)` for what the helper does not
    /// emit (type arguments, annotations).
    fn hierarchy_joined(
        language: Language,
        files: &[(&str, &str)],
        relationships: &[(&str, &str)],
        extra: &[(&str, Span, &str)],
    ) -> Joined {
        let (facts, index) = hierarchy_index(language, files, relationships, extra);
        join_facts(language, &facts, &index)
    }

    /// The join of `index` with `facts` (repo paths at the root), each file's heuristic keys
    /// from its facts.
    fn join_facts(language: Language, facts: &[(String, FileFacts)], index: &ScipIndex) -> Joined {
        let by_path: BTreeMap<&str, (Language, &FileFacts)> = facts
            .iter()
            .map(|(p, f)| (p.as_str(), (language, f)))
            .collect();
        let keys: BTreeMap<&str, Vec<String>> = facts
            .iter()
            .map(|(p, f)| (p.as_str(), rb_syntax::symbol_keys(p, f)))
            .collect();
        join(index, "", &by_path, &keys, &[language])
    }

    /// The facts and the synthetic index of [`hierarchy_joined`], before the join.
    fn hierarchy_index(
        language: Language,
        files: &[(&str, &str)],
        relationships: &[(&str, &str)],
        extra: &[(&str, Span, &str)],
    ) -> (Vec<(String, FileFacts)>, ScipIndex) {
        let prefix = symbol_prefix(language);
        let facts: Vec<(String, FileFacts)> = files
            .iter()
            .map(|(path, src)| {
                (
                    (*path).to_owned(),
                    rb_syntax::extract(language, src.as_bytes()).unwrap(),
                )
            })
            .collect();
        // The symbol of a type by (package, simple name): a Java package is the file's clause,
        // a TypeScript "package" is the file.
        let scope_of = |path: &str, f: &FileFacts| -> String {
            match language {
                Language::Java => f.package.as_deref().unwrap_or("p").replace('.', "/"),
                _ => format!("src/`{path}`"),
            }
        };
        let type_symbol = |scope: &str, name: &str| format!("{prefix}{scope}/{name}#");
        let symbol_of = |path: &str, f: &FileFacts, d: &rb_syntax::Definition| -> Option<String> {
            symbol_in(prefix, &scope_of(path, f), f, d)
        };
        let types: Vec<(String, String)> = facts
            .iter()
            .flat_map(|(path, f)| {
                f.definitions
                    .iter()
                    .filter(|d| is_type(d))
                    .map(move |d| (scope_of(path, f), d.name.clone()))
            })
            .collect();
        let resolve = |f: &FileFacts, path: &str, name: &str| -> Option<String> {
            if let Some((package, simple)) = name.rsplit_once('.') {
                return Some(type_symbol(&package.replace('.', "/"), simple));
            }
            let own = scope_of(path, f);
            if types.iter().any(|(s, n)| *s == own && n == name) {
                return Some(type_symbol(&own, name));
            }
            for import in &f.imports {
                let module = import.module.replace('.', "/");
                if let Some(package) = module.strip_suffix(&format!("/{name}")) {
                    return Some(type_symbol(package, name));
                }
                if import.names.iter().any(|n| n.local == name) {
                    return Some(type_symbol(&module, name));
                }
            }
            let mut found = types.iter().filter(|(_, n)| n == name);
            let (scope, _) = found.next()?;
            assert!(found.next().is_none(), "{name} is ambiguous in the test");
            Some(type_symbol(scope, name))
        };
        let documents = facts
            .iter()
            .map(|(path, f)| {
                let mut occurrences: Vec<Occurrence> = f
                    .definitions
                    .iter()
                    .filter_map(|d| {
                        Some(Occurrence {
                            span: d.name_span,
                            symbol: symbol_of(path, f, d)?,
                            roles: 1,
                            enclosing: None,
                        })
                    })
                    .collect();
                for d in &f.definitions {
                    for s in &d.supertypes {
                        let Some(symbol) = resolve(f, path, &s.name) else {
                            continue;
                        };
                        // `a.b.Circle`: `a/` at `a`, `a/b/` at `b`, the type at `Circle`.
                        let [line, mut col, ..] = s.span;
                        let parts: Vec<&str> = s.name.split('.').collect();
                        for (k, part) in parts.iter().enumerate() {
                            let len = part.len() as i32;
                            let sym = if k + 1 == parts.len() {
                                symbol.clone()
                            } else {
                                format!("{prefix}{}/", parts[..=k].join("/"))
                            };
                            occurrences.push(Occurrence {
                                span: [line, col, line, col + len],
                                symbol: sym,
                                roles: 0,
                                enclosing: None,
                            });
                            col += len + 1;
                        }
                    }
                }
                occurrences.extend(extra.iter().filter(|(p, ..)| p == path).map(
                    |(_, span, symbol)| Occurrence {
                        span: *span,
                        symbol: format!("{prefix}{symbol}"),
                        roles: 0,
                        enclosing: None,
                    },
                ));
                Document {
                    path: path.clone(),
                    occurrences,
                    symbols: relationships
                        .iter()
                        .filter(|(from, _)| {
                            f.definitions.iter().any(|d| {
                                symbol_of(path, f, d).as_deref() == Some(&format!("{prefix}{from}"))
                            })
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
                }
            })
            .collect();
        let index = ScipIndex {
            documents,
            ..Default::default()
        };
        (facts, index)
    }

    /// `line`'s `token` (its `nth` occurrence on the line, 0-based) as a span.
    fn at_nth(src: &str, line: usize, token: &str, nth: usize) -> Span {
        let text = src.lines().nth(line).unwrap();
        let col = text
            .match_indices(token)
            .nth(nth)
            .unwrap_or_else(|| panic!("{token:?} #{nth} in {text:?}"))
            .0 as i32;
        let line = line as i32;
        [line, col, line, col + token.len() as i32]
    }

    /// Contract M3b amendment 43 (the evaluator's G1): `interface Face extends
    /// Marker<Marker.Disc>` has three type occurrences in its clause, `Marker#` at the raw
    /// type, `Marker#` and `Marker#Disc#` inside the argument; only the first lies in the
    /// name's span, so `Face`'s supertype is `Marker#`, scip-java's reverse link
    /// `Face#area() → Marker#Disc#area()` is rejected, and the forward edges (`Face# →
    /// Marker#`, `Disc# → Face#`, `Disc#area() → Face#area()`) are emitted; `Cfg extends
    /// Base<Base.Options>` keeps `Cfg#run() IMPLEMENTS Base#run()`. Nothing is unresolved.
    #[test]
    fn a_nested_type_argument_is_not_the_supertype() {
        let marker = "package b;\npublic interface Marker<T> { class Disc implements Face { public double area() { return 1; } } }\n";
        let face =
            "package b;\npublic interface Face extends Marker<Marker.Disc> { double area(); }\n";
        let both = |a: &'static str, b: &'static str| [(a, b), (b, a)];
        let rels: Vec<(&str, &str)> = [
            both("b/Face#", "b/Marker#"),
            both("b/Marker#Disc#", "b/Face#"),
            both("b/Marker#Disc#area().", "b/Face#area()."),
        ]
        .concat();
        let joined = hierarchy_joined(
            Language::Java,
            &[("b/Marker.java", marker), ("b/Face.java", face)],
            &rels,
            &[
                ("b/Face.java", at_nth(face, 1, "Marker", 1), "b/Marker#"),
                ("b/Face.java", at_nth(face, 1, "Disc", 0), "b/Marker#Disc#"),
            ],
        );
        assert_eq!(
            implements_lines(Language::Java, &joined),
            [
                "b/Face# Implements b/Marker#",
                "b/Marker#Disc# Implements b/Face#",
                "b/Marker#Disc#area(). Implements b/Face#area().",
            ]
        );
        assert_eq!(joined.coverage.supertypes_unresolved, 0);
        assert_eq!(joined.coverage.relationships_dropped, 0);

        let base =
            "package a;\npublic class Base<T> { public static class Options {} void run() {} }\n";
        let cfg = "package a;\npublic class Cfg extends Base<Base.Options> { void run() {} }\n";
        let rels: Vec<(&str, &str)> = [
            both("a/Cfg#", "a/Base#"),
            both("a/Cfg#run().", "a/Base#run()."),
        ]
        .concat();
        let joined = hierarchy_joined(
            Language::Java,
            &[("a/Base.java", base), ("a/Cfg.java", cfg)],
            &rels,
            &[
                ("a/Cfg.java", at_nth(cfg, 1, "Base", 1), "a/Base#"),
                (
                    "a/Cfg.java",
                    at_nth(cfg, 1, "Options", 0),
                    "a/Base#Options#",
                ),
            ],
        );
        assert_eq!(
            implements_lines(Language::Java, &joined),
            [
                "a/Cfg# Implements a/Base#",
                "a/Cfg#run(). Implements a/Base#run().",
            ]
        );
        assert_eq!(joined.coverage.supertypes_unresolved, 0);
    }

    /// Contract M3b amendment 53 (the evaluator's H1): `Wide extends<TAB><TAB>Box<Doer>,<TAB>
    /// Yyyyyyyy` as a decoder without javac's tab expansion read scip-java's occurrences:
    /// `Box#` [40..43], `Doer#` [44..48] and `Yyyyyyyy#` clamped to [52..52] (measured), against
    /// Tree-sitter's name spans `Box` [31..34] and `Yyyyyyyy` [42..50]. The last type
    /// occurrence inside `Yyyyyyyy`'s span is `Doer#`, which does not name the written type, so
    /// both supertypes are unresolved and scip-java's reverse link `Wide#act() → Doer#act()`
    /// gives no edge. With the columns converted (the occurrences at the names), every forward
    /// edge is there.
    #[test]
    fn a_supertype_occurrence_must_name_the_written_type() {
        let wide = "package h;\npublic interface Wide extends\t\tBox<Doer>,\tYyyyyyyy {\n\tvoid act();\n}\n";
        let files = [
            ("h/Box.java", "package h;\npublic interface Box<T> {}\n"),
            (
                "h/Yyyyyyyy.java",
                "package h;\npublic interface Yyyyyyyy { void why(); }\n",
            ),
            ("h/Wide.java", wide),
            (
                "h/Doer.java",
                "package h;\npublic class Doer implements Wide {\n\t@Override public void act() {}\n    \t@Override public void why() {}\n}\n",
            ),
        ];
        let both = |a: &'static str, b: &'static str| [(a, b), (b, a)];
        let rels: Vec<(&str, &str)> = [
            both("h/Doer#", "h/Wide#"),
            both("h/Doer#act().", "h/Wide#act()."),
            both("h/Doer#why().", "h/Yyyyyyyy#why()."),
            both("h/Wide#", "h/Box#"),
            both("h/Wide#", "h/Yyyyyyyy#"),
            both("h/Doer#", "h/Box#"),
            both("h/Doer#", "h/Yyyyyyyy#"),
        ]
        .concat();
        let (facts, mut index) = hierarchy_index(Language::Java, &files, &rels, &[]);
        let converted = join_facts(Language::Java, &facts, &index);
        assert_eq!(
            implements_lines(Language::Java, &converted),
            [
                "h/Doer# Implements h/Box#",
                "h/Doer# Implements h/Wide#",
                "h/Doer# Implements h/Yyyyyyyy#",
                "h/Doer#act(). Implements h/Wide#act().",
                "h/Doer#why(). Implements h/Yyyyyyyy#why().",
                "h/Wide# Implements h/Box#",
                "h/Wide# Implements h/Yyyyyyyy#",
            ]
        );
        assert_eq!(converted.coverage.supertypes_unresolved, 0);

        // The heritage occurrences at scip-java's raw columns.
        let doc = index
            .documents
            .iter_mut()
            .find(|d| d.path == "h/Wide.java")
            .unwrap();
        doc.occurrences.retain(|o| o.is_definition());
        let prefix = symbol_prefix(Language::Java);
        for (span, name) in [
            ([1, 40, 1, 43], "Box"),
            ([1, 44, 1, 48], "Doer"),
            ([1, 52, 1, 52], "Yyyyyyyy"),
        ] {
            doc.occurrences.push(Occurrence {
                span,
                symbol: format!("{prefix}h/{name}#"),
                roles: 0,
                enclosing: None,
            });
        }
        doc.occurrences.sort();
        let shifted = join_facts(Language::Java, &facts, &index);
        let lines = implements_lines(Language::Java, &shifted);
        assert!(
            !lines.iter().any(|l| l.starts_with("h/Wide#")),
            "no edge from Wide through the shifted occurrences: {lines:?}"
        );
        assert_eq!(
            lines,
            [
                "h/Doer# Implements h/Wide#",
                "h/Doer#act(). Implements h/Wide#act().",
            ]
        );
        assert_eq!(shifted.coverage.supertypes_unresolved, 2);
    }

    /// Contract M3b amendment 45 (the evaluator's G3): `implements @Tag Shape` has the
    /// annotation's `Tag#` before `Shape#`; the name span excludes the annotation, so the
    /// supertype is `Shape#` and both forward edges are emitted.
    #[test]
    fn an_annotated_supertype_resolves_to_the_type() {
        let shape = "package a;\npublic interface Shape { double area(); }\n";
        let sq = "package a;\npublic class Sq implements @Tag Shape { public double area() { return 1; } }\n";
        let both = |a: &'static str, b: &'static str| [(a, b), (b, a)];
        let rels: Vec<(&str, &str)> = [
            both("a/Sq#", "a/Shape#"),
            both("a/Sq#area().", "a/Shape#area()."),
        ]
        .concat();
        let joined = hierarchy_joined(
            Language::Java,
            &[("a/Shape.java", shape), ("a/Sq.java", sq)],
            &rels,
            &[("a/Sq.java", at_nth(sq, 1, "Tag", 0), "a/Tag#")],
        );
        assert_eq!(
            implements_lines(Language::Java, &joined),
            [
                "a/Sq# Implements a/Shape#",
                "a/Sq#area(). Implements a/Shape#area().",
            ]
        );
        assert_eq!(joined.coverage.supertypes_unresolved, 0);
    }

    /// Contract M3b amendment 52: an `is_implementation` relationship whose source (a local
    /// class expression) or target (a dependency's type) has no matched project definition
    /// gives no edge and is counted; the relationships that give edges, and the reverse links
    /// the direction rule rejects, are not.
    #[test]
    fn dropped_relationships_are_counted() {
        let src = "export interface I { m(): void }\nexport class A implements I { m() {} }\n";
        let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
        let path = "src/a.ts";
        let sym = |d: &str| format!("scip-typescript npm p 1.0.0 src/`a.ts`/{d}");
        let def = |name: &str| {
            let d = facts.definitions.iter().find(|d| d.name == name).unwrap();
            Occurrence {
                span: d.name_span,
                symbol: sym(&format!(
                    "{}{}",
                    d.parent
                        .map_or(String::new(), |p| format!("{}#", facts.definitions[p].name)),
                    if d.kind == rb_syntax::DefKind::Method {
                        format!("{name}().")
                    } else {
                        format!("{name}#")
                    }
                )),
                roles: 1,
                enclosing: None,
            }
        };
        let rel = |to: &str| Relationship {
            symbol: to.into(),
            is_implementation: true,
            ..Default::default()
        };
        let i_ref = at_nth(src, 1, "I", 0);
        let index = ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences: vec![
                    def("I"),
                    def("m"),
                    def("A"),
                    Occurrence {
                        span: i_ref,
                        symbol: sym("I#"),
                        roles: 0,
                        enclosing: None,
                    },
                ],
                symbols: vec![
                    SymbolInfo {
                        symbol: sym("A#"),
                        relationships: vec![
                            rel(&sym("I#")),
                            rel("scip-typescript npm lib 2.0.0 `index.d.ts`/Ext#"),
                        ],
                        ..Default::default()
                    },
                    SymbolInfo {
                        symbol: sym("I#"),
                        relationships: vec![rel(&sym("A#"))],
                        ..Default::default()
                    },
                    SymbolInfo {
                        symbol: "local 12".into(),
                        relationships: vec![rel(&sym("I#"))],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let files = BTreeMap::from([(path, (Language::TypeScript, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> =
            BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
        let joined = join(&index, "", &files, &keys, &[Language::TypeScript]);
        let implements: Vec<String> = edge_lines(&joined)
            .into_iter()
            .filter(|l| l.contains(" Implements "))
            .collect();
        assert_eq!(
            implements,
            [format!("{} Implements {}", sym("A#"), sym("I#"))]
        );
        assert_eq!(
            joined.coverage.relationships_dropped, 2,
            "the local class expression's and the external target's"
        );
    }

    /// A definition occurrence (`roles` 1) or a reference (0) at `span`.
    fn occ_at(span: Span, symbol: &str, roles: i32) -> Occurrence {
        Occurrence {
            span,
            symbol: symbol.into(),
            roles,
            enclosing: None,
        }
    }

    fn implementation(from: &str, to: &str) -> SymbolInfo {
        SymbolInfo {
            symbol: from.into(),
            relationships: vec![Relationship {
                symbol: to.into(),
                is_implementation: true,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// Contract M3b amendment 54 (the evaluator's H3): scip-java keys the method of an enum
    /// constant's body as `local 3`. Its definition occurrence at the method's name binds the
    /// local symbol to that definition, which keeps its heuristic key and is counted; the
    /// relationship `local 3 → Base#process()` gives `IMPLEMENTS` from it, the call inside it
    /// stays exact, and a reverse link `Base#process() → local 3` (not seen from scip-java
    /// 0.13.1, which lists none on jsoup) is rejected.
    #[test]
    fn an_enum_constant_body_method_implements_through_its_local_symbol() {
        let src = "package p;\npublic enum Base {\n  A { boolean process() { return helper(); } };\n  abstract boolean process();\n  boolean helper() { return true; }\n}\n";
        let facts = rb_syntax::extract(Language::Java, src.as_bytes()).unwrap();
        let path = "p/Base.java";
        let sym = |d: &str| format!("scip-java maven . . p/Base#{d}");
        let index = ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences: vec![
                    occ_at(at_nth(src, 1, "Base", 0), &sym(""), 1),
                    occ_at(at_nth(src, 2, "A", 0), &sym("A."), 1),
                    occ_at(at_nth(src, 2, "process", 0), "local 3", 1),
                    occ_at(at_nth(src, 2, "helper", 0), &sym("helper()."), 0),
                    occ_at(at_nth(src, 3, "process", 0), &sym("process()."), 1),
                    occ_at(at_nth(src, 4, "helper", 0), &sym("helper()."), 1),
                ],
                symbols: vec![
                    SymbolInfo {
                        display_name: "process".into(),
                        ..implementation("local 3", &sym("process()."))
                    },
                    implementation(&sym("process()."), "local 3"),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let heuristic = rb_syntax::symbol_keys(path, &facts);
        let body = facts
            .definitions
            .iter()
            .position(|d| d.name == "process" && d.name_span[0] == 2)
            .expect("the constant body's method is a Tree-sitter definition");
        let files = BTreeMap::from([(path, (Language::Java, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> = BTreeMap::from([(path, heuristic.clone())]);
        let joined = join(&index, "", &files, &keys, &[Language::Java]);
        assert!(
            !joined.keys[path].contains_key(&body),
            "the local definition keeps its heuristic key"
        );
        assert_eq!(
            edge_lines(&joined),
            [
                format!("{} Calls {}", heuristic[body], sym("helper().")),
                format!("{} Implements {}", heuristic[body], sym("process().")),
            ]
        );
        assert_eq!(joined.coverage.definitions_local, 1);
        assert_eq!(
            joined.coverage.definitions_matched + joined.coverage.definitions_local,
            joined.coverage.definitions_total
        );
        assert_eq!(joined.coverage.relationships_dropped, 0);
    }

    /// Contract M3b amendment 54: a local class's method (`local 7`) is called from the
    /// enclosing global method; the call is an exact edge to the local definition's
    /// heuristic key, counted as matched.
    #[test]
    fn a_call_to_a_local_definition_is_exact() {
        let src = "package p;\nclass C {\n  void run() {\n    class Local { void go() {} }\n    new Local().go();\n  }\n}\n";
        let facts = rb_syntax::extract(Language::Java, src.as_bytes()).unwrap();
        let path = "p/C.java";
        let sym = |d: &str| format!("scip-java maven . . p/C#{d}");
        let index = ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences: vec![
                    occ_at(at_nth(src, 1, "C", 0), &sym(""), 1),
                    occ_at(at_nth(src, 2, "run", 0), &sym("run()."), 1),
                    occ_at(at_nth(src, 3, "Local", 0), "local 6", 1),
                    occ_at(at_nth(src, 3, "go", 0), "local 7", 1),
                    occ_at(at_nth(src, 4, "Local", 0), "local 6", 0),
                    occ_at(at_nth(src, 4, "go", 0), "local 7", 0),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let heuristic = rb_syntax::symbol_keys(path, &facts);
        let go = facts
            .definitions
            .iter()
            .position(|d| d.name == "go")
            .expect("the local class's method is a Tree-sitter definition");
        let files = BTreeMap::from([(path, (Language::Java, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> = BTreeMap::from([(path, heuristic.clone())]);
        let joined = join(&index, "", &files, &keys, &[Language::Java]);
        assert!(
            edge_lines(&joined).contains(&format!("{} Calls {}", sym("run()."), heuristic[go])),
            "{:?}",
            edge_lines(&joined)
        );
        assert_eq!(joined.coverage.definitions_local, 2, "Local and go");
        assert_eq!(joined.coverage.references_local, 0);
    }

    /// Contract M3b amendment 54: local symbols are unique per document only. `local 3` is
    /// bound in `a/A.java`; the same string in `b/B.java` names nothing there, so its call
    /// gives no edge and counts as a local reference.
    #[test]
    fn a_local_symbol_never_crosses_documents() {
        let a = "package a;\nenum A {\n  X { void go() {} };\n  void go() {}\n}\n";
        let b = "package b;\nclass B {\n  void run() { go(); }\n}\n";
        let fa = rb_syntax::extract(Language::Java, a.as_bytes()).unwrap();
        let fb = rb_syntax::extract(Language::Java, b.as_bytes()).unwrap();
        let index = ScipIndex {
            documents: vec![
                Document {
                    path: "a/A.java".into(),
                    occurrences: vec![
                        occ_at(at_nth(a, 1, "A", 0), "scip-java maven . . a/A#", 1),
                        occ_at(at_nth(a, 2, "go", 0), "local 3", 1),
                        occ_at(at_nth(a, 3, "go", 0), "scip-java maven . . a/A#go().", 1),
                    ],
                    ..Default::default()
                },
                Document {
                    path: "b/B.java".into(),
                    occurrences: vec![
                        occ_at(at_nth(b, 1, "B", 0), "scip-java maven . . b/B#", 1),
                        occ_at(at_nth(b, 2, "run", 0), "scip-java maven . . b/B#run().", 1),
                        occ_at(at_nth(b, 2, "go", 0), "local 3", 0),
                    ],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let files = BTreeMap::from([
            ("a/A.java", (Language::Java, &fa)),
            ("b/B.java", (Language::Java, &fb)),
        ]);
        let keys: BTreeMap<&str, Vec<String>> = BTreeMap::from([
            ("a/A.java", rb_syntax::symbol_keys("a/A.java", &fa)),
            ("b/B.java", rb_syntax::symbol_keys("b/B.java", &fb)),
        ]);
        let joined = join(&index, "", &files, &keys, &[Language::Java]);
        assert!(joined.edges.is_empty(), "{:?}", edge_lines(&joined));
        assert_eq!(joined.coverage.definitions_local, 1);
        assert_eq!(joined.coverage.references_local, 1);
        assert_eq!(joined.coverage.references_matched, 0);
    }

    /// Contract M3b amendment 54: a dropped relationship whose two ends are both defined in the
    /// project (a definition occurrence on the name line of a same-named Tree-sitter
    /// definition) is a join gap and counted in project; one into a dependency, or from a
    /// local the syntax does not define (an anonymous class's method), is not.
    #[test]
    fn relationships_dropped_in_project_are_counted() {
        let src =
            "package p;\ninterface A { void m(); }\nclass B implements A { public void m() {} }\n";
        let facts = rb_syntax::extract(Language::Java, src.as_bytes()).unwrap();
        let path = "p/A.java";
        let sym = |d: &str| format!("scip-java maven . . p/{d}");
        let a_m = at_nth(src, 1, "m", 0);
        let index = ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences: vec![
                    occ_at(at_nth(src, 1, "A", 0), &sym("A#"), 1),
                    // `A#m()`'s definition occurrence two columns off its name: unmatched.
                    occ_at([a_m[0], a_m[1] + 2, a_m[2], a_m[3] + 2], &sym("A#m()."), 1),
                    occ_at(at_nth(src, 2, "B", 0), &sym("B#"), 1),
                    occ_at(at_nth(src, 2, "A", 0), &sym("A#"), 0),
                    occ_at(at_nth(src, 2, "m", 0), &sym("B#m()."), 1),
                    // An anonymous class's method, which Tree-sitter treats as a local.
                    occ_at(at_nth(src, 0, "p", 0), "local 5", 1),
                ],
                symbols: vec![
                    SymbolInfo {
                        relationships: vec![
                            Relationship {
                                symbol: sym("A#m()."),
                                is_implementation: true,
                                ..Default::default()
                            },
                            Relationship {
                                symbol: "scip-java maven jdk 21 java/lang/Runnable#run().".into(),
                                is_implementation: true,
                                ..Default::default()
                            },
                        ],
                        ..implementation(&sym("B#m()."), "")
                    },
                    implementation(&sym("B#"), &sym("A#")),
                    SymbolInfo {
                        display_name: "m".into(),
                        ..implementation("local 5", &sym("A#"))
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let files = BTreeMap::from([(path, (Language::Java, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> =
            BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
        let joined = join(&index, "", &files, &keys, &[Language::Java]);
        assert_eq!(
            edge_lines(&joined)
                .into_iter()
                .filter(|l| l.contains(" Implements "))
                .collect::<Vec<_>>(),
            [format!("{} Implements {}", sym("B#"), sym("A#"))]
        );
        assert_eq!(joined.coverage.relationships_dropped, 3);
        assert_eq!(
            joined.coverage.relationships_dropped_in_project, 1,
            "B#m() → A#m()"
        );
    }

    /// Contract M3b amendment 26 (the evaluator's J1): two project types named `Common`. With
    /// declared supertypes resolved by simple name, `a.Shape extends Common` reached `b.Common`,
    /// whose `extends a.Circle` reached `Circle`, and scip-java's reverse override link
    /// `Shape#area() → Circle#area()` became an exact edge. Resolved through the occurrences
    /// in the heritage clauses, `Shape`'s `Common` is `a.Common`, which leads nowhere, so only
    /// the forward edges remain.
    #[test]
    fn a_same_named_type_does_not_make_the_reverse_link_an_edge() {
        let files = [
            ("a/Common.java", "package a;\npublic interface Common {}\n"),
            (
                "a/Shape.java",
                "package a;\npublic interface Shape extends Common { double area(); }\n",
            ),
            (
                "a/Circle.java",
                "package a;\npublic class Circle implements Shape { public double area() { return 1; } }\n",
            ),
            (
                "b/Common.java",
                "package b;\npublic class Common extends a.Circle {}\n",
            ),
        ];
        let both = |a: &'static str, b: &'static str| [(a, b), (b, a)];
        let rels: Vec<(&str, &str)> = [
            both("a/Circle#area().", "a/Shape#area()."),
            both("a/Circle#", "a/Shape#"),
            both("a/Circle#", "a/Common#"),
            both("a/Shape#", "a/Common#"),
            both("b/Common#", "a/Circle#"),
            both("b/Common#", "a/Shape#"),
            both("b/Common#", "a/Common#"),
        ]
        .concat();
        assert_eq!(
            hierarchy(&files, &rels),
            [
                "a/Circle# Implements a/Common#",
                "a/Circle# Implements a/Shape#",
                "a/Circle#area(). Implements a/Shape#area().",
                "a/Shape# Implements a/Common#",
                "b/Common# Implements a/Circle#",
                "b/Common# Implements a/Common#",
                "b/Common# Implements a/Shape#",
            ]
        );
    }

    /// Contract M3b amendment 28 (the evaluator's T1): a TypeScript `interface Mid extends
    /// Base` declares a supertype, so `class Svc implements Mid` and `class Sub extends Svc`
    /// both implement `Base#m()` through the interface chain; scip-typescript lists every
    /// level on the implementation's side only.
    #[test]
    fn typescript_implements_runs_through_an_interface_chain() {
        let src = "export interface Base { m(): void }\nexport interface Mid extends Base { extra(): void }\nexport class Svc implements Mid { m() {} extra() {} }\nexport class Sub extends Svc { m() {} }\nexport function use(b: Base) { b.m(); }\n";
        let rels = [
            ("src/`base.ts`/Sub#", "src/`base.ts`/Svc#"),
            ("src/`base.ts`/Sub#", "src/`base.ts`/Mid#"),
            ("src/`base.ts`/Sub#", "src/`base.ts`/Base#"),
            ("src/`base.ts`/Sub#m().", "src/`base.ts`/Svc#m()."),
            ("src/`base.ts`/Sub#m().", "src/`base.ts`/Base#m()."),
            ("src/`base.ts`/Svc#", "src/`base.ts`/Mid#"),
            ("src/`base.ts`/Svc#", "src/`base.ts`/Base#"),
            ("src/`base.ts`/Svc#extra().", "src/`base.ts`/Mid#extra()."),
            ("src/`base.ts`/Svc#m().", "src/`base.ts`/Base#m()."),
        ];
        let edges = hierarchy_in(Language::TypeScript, &[("base.ts", src)], &rels);
        assert_eq!(
            edges,
            [
                "src/`base.ts`/Sub# Implements src/`base.ts`/Base#",
                "src/`base.ts`/Sub# Implements src/`base.ts`/Mid#",
                "src/`base.ts`/Sub# Implements src/`base.ts`/Svc#",
                "src/`base.ts`/Sub#m(). Implements src/`base.ts`/Base#m().",
                "src/`base.ts`/Sub#m(). Implements src/`base.ts`/Svc#m().",
                "src/`base.ts`/Svc# Implements src/`base.ts`/Base#",
                "src/`base.ts`/Svc# Implements src/`base.ts`/Mid#",
                "src/`base.ts`/Svc#extra(). Implements src/`base.ts`/Mid#extra().",
                "src/`base.ts`/Svc#m(). Implements src/`base.ts`/Base#m().",
            ]
        );
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
                definitions_local: 0,
                definitions_total: 1,
                references_matched: 0,
                references_external: 1,
                references_undocumented: 1,
                references_local: 0,
                references_unanchored: 0,
                references_unmatched: 0,
                files_without_document: 0,
                supertypes_unresolved: 0,
                occurrences_unconverted: 0,
                relationships_dropped: 0,
                relationships_dropped_in_project: 0,
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

    /// Contract M3b amendment 26: a declared supertype resolves to the type occurrence inside
    /// its clause item as the indexers emit them: the raw type before its arguments
    /// (`Repo<User>` → `Repo#`, not `User#`), the type after its qualifier's namespace
    /// occurrence (`a.Circle` → `a/Circle#`, not `a/`), the nested type over its outer one
    /// (`Outer.Inner` → `Outer#Inner#`), and nothing when the span holds no type occurrence,
    /// which the coverage counts.
    #[test]
    fn declared_supertypes_resolve_generic_qualified_and_nested_names() {
        let src =
            "package p;\nclass Impl extends a.Circle implements Repo<User>, Outer.Inner, Gone {}\n";
        let facts = rb_syntax::extract(Language::Java, src.as_bytes()).unwrap();
        let path = "src/Impl.java";
        let at = |token: &str| -> Span {
            let col = src.lines().nth(1).unwrap().find(token).unwrap() as i32;
            [1, col, 1, col + token.len() as i32]
        };
        let sym = |d: &str| format!("scip-java maven . . {d}");
        let occ = |span: Span, symbol: String, roles: i32| Occurrence {
            span,
            symbol,
            roles,
            enclosing: None,
        };
        let impl_def = facts.definitions.iter().find(|d| d.name == "Impl").unwrap();
        let occurrences = vec![
            occ(impl_def.name_span, sym("p/Impl#"), 1),
            occ(at("a."), sym("a/"), 0),
            occ(at("Circle"), sym("a/Circle#"), 0),
            occ(at("Repo"), sym("p/Repo#"), 0),
            occ(at("User"), sym("p/User#"), 0),
            occ(at("Outer"), sym("p/Outer#"), 0),
            occ(at("Inner"), sym("p/Outer#Inner#"), 0),
            // `Gone` has an occurrence, but of a local symbol: not a type the index names.
            occ(at("Gone"), "local 1".into(), 0),
        ];
        let by_span =
            occurrences
                .iter()
                .fold(BTreeMap::<Span, Vec<&Occurrence>>::new(), |mut m, o| {
                    m.entry(o.span).or_default().push(o);
                    m
                });
        assert_eq!(
            declared_supertypes(impl_def, &by_span),
            [
                Some(sym("a/Circle#")),
                Some(sym("p/Repo#")),
                Some(sym("p/Outer#Inner#")),
                None,
            ]
        );
        // scip-typescript on a class merged with a namespace (`class NS {}; namespace NS {
        // class Inner {} }`) emits `NS#` and `NS/` at `NS` and `NS/Inner#` at `Inner`: the
        // last type occurrence in the name is the nested type, although its symbol does not
        // extend the outer type's.
        let ts = "class A extends NS.Inner {}\n";
        let ts_facts = rb_syntax::extract(Language::TypeScript, ts.as_bytes()).unwrap();
        let ts_sym = |d: &str| format!("scip-typescript npm p 1.0.0 src/`a.ts`/{d}");
        let ts_occurrences = [
            occ([0, 16, 0, 18], ts_sym("NS#"), 0),
            occ([0, 16, 0, 18], ts_sym("NS/"), 0),
            occ([0, 19, 0, 24], ts_sym("NS/Inner#"), 0),
        ];
        assert_eq!(ts.find("NS.Inner"), Some(16));
        let ts_by_span =
            ts_occurrences
                .iter()
                .fold(BTreeMap::<Span, Vec<&Occurrence>>::new(), |mut m, o| {
                    m.entry(o.span).or_default().push(o);
                    m
                });
        assert_eq!(
            declared_supertypes(&ts_facts.definitions[0], &ts_by_span),
            [Some(ts_sym("NS/Inner#"))]
        );
        let index = ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences,
                ..Default::default()
            }],
            ..Default::default()
        };
        let files = BTreeMap::from([(path, (Language::Java, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> =
            BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
        let joined = join(&index, "", &files, &keys, &[Language::Java]);
        assert_eq!(joined.coverage.supertypes_unresolved, 1);
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
