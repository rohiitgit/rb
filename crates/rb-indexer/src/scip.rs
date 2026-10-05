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
    /// `definitions_total − definitions_matched − definitions_local` (contract M3b amendment
    /// 62): definitions the index gives no definition occurrence at their name that names
    /// them (S7's second exception), including any an occurrence was rejected for by the name
    /// guard (amendment 59).
    pub definitions_without_occurrence: usize,
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
    /// The subset of `relationships_dropped` from a member the syntax deliberately has no
    /// definition for (an anonymous class's method, design §8.2) into a target defined in the
    /// project: a document-local source symbol with a definition occurrence in the
    /// relationship's document that binds to no Tree-sitter definition (contract M3b amendment
    /// 62). Not a join gap, and no edge is possible, but the link exists and is counted.
    pub relationships_anonymous: usize,
    /// Documents with facts that the agreement check rejected (contract M3b amendment 64): the
    /// indexer and Tree-sitter may read the file differently, so the whole file stays Tier B
    /// and none of its definitions or references is counted above.
    pub documents_rejected: usize,
    /// `documents_rejected` by the first reason that applied.
    pub documents_rejected_reasons: BTreeMap<rb_scip::Rejection, usize>,
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
    let mut out = Joined::default();
    // The documents with facts, each with its occurrences by span, that pass the agreement
    // check (contract M3b amendment 64): a rejected document is left out entirely, as if the
    // index had none for its file except that it is counted as rejected, not as missing.
    let mut indexed: BTreeSet<String> = BTreeSet::new();
    let mut docs: Vec<Doc<'_>> = Vec::new();
    for doc in &index.documents {
        let path = repo_path(&doc.path);
        let Some(&(_, facts)) = files.get(path.as_str()) else {
            continue;
        };
        indexed.insert(path.clone());
        let rejected = doc.rejected.or_else(|| {
            definition_disagreement(facts, &doc.occurrences)
                .then_some(rb_scip::Rejection::DefinitionDisagreement)
        });
        if let Some(reason) = rejected {
            out.coverage.documents_rejected += 1;
            *out.coverage
                .documents_rejected_reasons
                .entry(reason)
                .or_default() += 1;
            continue;
        }
        let by_span = doc.occurrences.iter().fold(
            BTreeMap::<Span, Vec<&rb_scip::Occurrence>>::new(),
            |mut m, o| {
                m.entry(o.span).or_default().push(o);
                m
            },
        );
        docs.push((path, doc, facts, by_span));
    }
    // Per document, how its tokens may be named (contract M3b amendments 59 and 66), and its
    // definitions anchored to the index's definition occurrences.
    let naming: Vec<Naming<'_>> = docs
        .iter()
        .map(|(_, doc, facts, by_span)| Naming::new(doc, facts, by_span))
        .collect();
    let anchors: Vec<Anchors<'_>> = docs
        .iter()
        .zip(&naming)
        .map(|((_, _, facts, by_span), naming)| Anchors::new(facts, by_span, naming))
        .collect();
    // Contract M3b amendment 68 (`structure_disagreement`): a Java document in which a
    // reference that would become an edge has another source in Tree-sitter's structure than
    // in the index's own enclosing ranges is rejected too. Whether a reference would become an
    // edge is judged against the targets of every document that reached this point, a superset
    // of the final ones (a rejection only removes targets), so the outcome does not depend on
    // the documents' order.
    let candidate_nodes: BTreeSet<&str> = anchors
        .iter()
        .flat_map(|a| a.replaced.values().map(String::as_str))
        .collect();
    let agrees: Vec<bool> = docs
        .iter()
        .zip(naming.iter().zip(&anchors))
        .map(|((path, doc, facts, by_span), (naming, anchors))| {
            files[path.as_str()].0 != Language::Java
                || !structure_disagreement(
                    facts,
                    by_span,
                    &doc.occurrences,
                    naming,
                    anchors,
                    &candidate_nodes,
                )
        })
        .collect();
    let mut docs_kept = Vec::with_capacity(docs.len());
    let mut naming_kept = Vec::with_capacity(docs.len());
    let mut anchors_kept = Vec::with_capacity(docs.len());
    for ((doc, naming), (anchors, agrees)) in docs
        .into_iter()
        .zip(naming)
        .zip(anchors.into_iter().zip(agrees))
    {
        if agrees {
            docs_kept.push(doc);
            naming_kept.push(naming);
            anchors_kept.push(anchors);
        } else {
            out.coverage.documents_rejected += 1;
            *out.coverage
                .documents_rejected_reasons
                .entry(rb_scip::Rejection::StructureDisagreement)
                .or_default() += 1;
        }
    }
    let (docs, naming) = (docs_kept, naming_kept);
    // Every global symbol the accepted documents define, so references to dependencies are
    // told apart; a symbol defined only in a rejected document is the project's own but has no
    // node (`references_undocumented`).
    let defined: BTreeSet<&str> = docs
        .iter()
        .flat_map(|(_, d, ..)| &d.occurrences)
        .filter(|o| o.is_definition() && !o.symbol.starts_with("local "))
        .map(|o| o.symbol.as_str())
        .collect();
    // The packages the index itself belongs to (`None` when its symbols carry none): a symbol of
    // one of them that the index does not define (a file of the project without a document;
    // scip-typescript's `node_modules/.prisma/…` under the project's own package name) is not
    // a dependency.
    let own_packages: BTreeSet<Option<rb_scip::Package>> =
        defined.iter().map(|s| rb_scip::package(s)).collect();
    out.documented = docs.iter().map(|(path, ..)| path.clone()).collect();
    out.coverage.files_without_document = files
        .iter()
        .filter(|(path, (language, _))| languages.contains(language) && !indexed.contains(**path))
        .count();
    let mut locals: Vec<BTreeMap<&str, usize>> = Vec::with_capacity(docs.len());
    for ((path, ..), anchors) in docs.iter().zip(anchors_kept) {
        out.coverage.definitions_total += anchors.total;
        out.coverage.definitions_matched += anchors.matched;
        out.coverage.definitions_matched_by_name += anchors.matched_by_name;
        out.coverage.definitions_local += anchors.local;
        if !anchors.replaced.is_empty() {
            out.keys.insert(path.clone(), anchors.replaced);
        }
        locals.push(anchors.locals);
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
    // `Coverage::relationships_dropped_in_project`). A local symbol is when it is bound to a
    // definition of that document (amendment 54), which places its definition occurrence at a
    // Tree-sitter definition's name by construction; its display name is not consulted, since
    // scip-typescript leaves it empty (amendment 60).
    let in_project = |symbol: &str, di: usize| -> bool {
        if symbol.starts_with("local ") {
            locals[di].contains_key(symbol)
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
        // References: the name's range is a non-definition occurrence that names the token
        // (amendment 59); one that does not is counted unmatched.
        for r in &facts.references {
            let kind = match r.kind {
                RefKind::Call => EdgeKind::Calls,
                RefKind::Class | RefKind::Type => EdgeKind::References,
                RefKind::Implementation => continue,
            };
            let Some(o) = by_span
                .get(&r.span)
                .and_then(|os| os.iter().find(|o| !o.is_definition()))
                .filter(|o| {
                    let bound = locals[di]
                        .get(o.symbol.as_str())
                        .map(|&i| facts.definitions[i].name.as_str());
                    naming[di].names_token(o, bound, &r.name)
                })
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
            // node to start an edge from: its relationships are dropped and counted. A local
            // defined here that binds to no Tree-sitter definition is an anonymous class's
            // member (amendment 62).
            let Some((i, source_key)) = key_of(&s.symbol) else {
                let anonymous = s.symbol.starts_with("local ")
                    && local_sites[di].contains_key(s.symbol.as_str());
                for rel in implementations {
                    out.coverage.relationships_anonymous +=
                        usize::from(anonymous && in_project(&rel.symbol, di));
                    dropped(rel);
                }
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
    out.coverage.definitions_without_occurrence = out.coverage.definitions_total
        - out.coverage.definitions_matched
        - out.coverage.definitions_local;
    out.edges.sort_by(|a, b| {
        (&a.src, a.kind as u8, &a.dst, &a.at).cmp(&(&b.src, b.kind as u8, &b.dst, &b.at))
    });
    out.edges.dedup();
    out.packages.sort();
    out
}

/// One document's Tree-sitter definitions anchored to the index's definition occurrences: the
/// name's range is a definition occurrence of a global symbol. The matched symbols are the
/// only targets an edge may have, since only they get `Symbol` nodes. A definition occurrence
/// of a local symbol at the name binds that symbol, within its document, to the definition,
/// which keeps its heuristic key (contract M3b amendment 54). Exact anchors at the name come
/// before the by-name fallback.
struct Anchors<'a> {
    /// Per definition index: the SCIP symbol that replaces its heuristic key.
    replaced: BTreeMap<usize, String>,
    /// Local symbols bound to a definition (its index; the first one binding the symbol).
    locals: BTreeMap<&'a str, usize>,
    /// Per definition index: the definition occurrence that anchored it, global or local.
    occurrences: BTreeMap<usize, &'a rb_scip::Occurrence>,
    total: usize,
    matched: usize,
    matched_by_name: usize,
    /// Definitions anchored by a local symbol.
    local: usize,
}

impl<'a> Anchors<'a> {
    fn new(
        facts: &FileFacts,
        by_span: &BTreeMap<Span, Vec<&'a rb_scip::Occurrence>>,
        naming: &Naming<'_>,
    ) -> Self {
        let mut out = Anchors {
            replaced: BTreeMap::new(),
            locals: BTreeMap::new(),
            occurrences: BTreeMap::new(),
            total: facts.definitions.len(),
            matched: 0,
            matched_by_name: 0,
            local: 0,
        };
        for (i, d) in facts.definitions.iter().enumerate() {
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
                    && naming.local_names.get(o.symbol.as_str()) == Some(&d.name.as_str())
            };
            // The occurrence at the name must name the definition (contract M3b amendment 59):
            // one an indexer's position error moved there is unmatched, not another key. A
            // local symbol is not bound yet: its name, when known, decides.
            let names_d = |o: &&rb_scip::Occurrence| naming.names_token(o, None, &d.name);
            let global_at_name = at_name(is_global_definition).filter(names_d);
            let local_at_name = global_at_name
                .is_none()
                .then(|| at_name(is_local_definition))
                .flatten()
                .filter(names_d);
            let global_by_name = (global_at_name.is_none() && local_at_name.is_none())
                .then(|| definition_before_name(d, by_span, &global_named))
                .flatten();
            if let Some(o) = global_at_name.or(global_by_name) {
                out.matched += 1;
                out.matched_by_name += usize::from(global_by_name.is_some());
                out.replaced.insert(i, o.symbol.clone());
                out.occurrences.insert(i, o);
                continue;
            }
            let local = local_at_name.or_else(|| definition_before_name(d, by_span, &local_named));
            if let Some(o) = local {
                out.local += 1;
                out.locals.entry(o.symbol.as_str()).or_insert(i);
                out.occurrences.insert(i, o);
            }
        }
        out
    }
}

/// Whether Tree-sitter's structure contradicts the index's own enclosing ranges about where a
/// reference sits (contract M3b amendment 68, `structure_disagreement`; applied to Java only):
/// for a reference the join would turn into an edge (a call or type use whose occurrence names
/// its token and whose symbol is a target: a bound local or one of `nodes`), with Tree-sitter's
/// enclosing definition as its source,
/// 1. the reference lies outside the enclosing range of the definition occurrence that
///    anchored that source (when the indexer gave it one); or
/// 2. the innermost enclosing range around the reference that belongs to an anchored
///    definition's symbol is neither the source's nor one of its Tree-sitter ancestors' (a
///    reference Tree-sitter puts at file level has no source, so any such range disagrees).
///
/// Definitions Tree-sitter deliberately does not have (fields, locals, an anonymous class's
/// methods, lambdas) are not anchored, so their ranges are passed over: what they hold is
/// credited to the enclosing definition on both sides (design §8.2). javac translates every
/// Unicode escape before lexing (JLS §3.3), so `\u002a/` or `\u0022` can end a comment or a
/// string for javac only and move code into another member (the evaluator's K1).
fn structure_disagreement(
    facts: &FileFacts,
    by_span: &BTreeMap<Span, Vec<&rb_scip::Occurrence>>,
    occurrences: &[rb_scip::Occurrence],
    naming: &Naming<'_>,
    anchors: &Anchors<'_>,
    nodes: &BTreeSet<&str>,
) -> bool {
    let inside = |outer: Span, inner: Span| {
        (outer[0], outer[1]) <= (inner[0], inner[1]) && (inner[2], inner[3]) <= (outer[2], outer[3])
    };
    let anchored: BTreeSet<&str> = anchors
        .occurrences
        .values()
        .map(|o| o.symbol.as_str())
        .collect();
    let ranges: Vec<(Span, &str)> = occurrences
        .iter()
        .filter(|o| o.is_definition() && anchored.contains(o.symbol.as_str()))
        .filter_map(|o| Some((o.enclosing?, o.symbol.as_str())))
        .collect();
    let symbol_of = |i: usize| anchors.occurrences.get(&i).map(|o| o.symbol.as_str());
    facts.references.iter().any(|r| {
        if r.kind == RefKind::Implementation {
            return false;
        }
        let Some(o) = by_span
            .get(&r.span)
            .and_then(|os| os.iter().find(|o| !o.is_definition()))
        else {
            return false;
        };
        let bound = anchors
            .locals
            .get(o.symbol.as_str())
            .map(|&i| facts.definitions[i].name.as_str());
        let target = if o.symbol.starts_with("local ") {
            bound.is_some()
        } else {
            nodes.contains(o.symbol.as_str())
        };
        if !target || !naming.names_token(o, bound, &r.name) {
            return false;
        }
        if let Some(range) = r
            .enclosing
            .and_then(|i| anchors.occurrences.get(&i))
            .and_then(|o| o.enclosing)
            && !inside(range, r.span)
        {
            return true;
        }
        // Ranges nest, so the innermost one around the reference starts last.
        let innermost = ranges
            .iter()
            .filter(|(range, _)| inside(*range, r.span))
            .max_by_key(|(range, _)| {
                (
                    (range[0], range[1]),
                    std::cmp::Reverse((range[2], range[3])),
                )
            });
        let Some(&(_, symbol)) = innermost else {
            return false;
        };
        let mut at = r.enclosing;
        while let Some(i) = at {
            if symbol_of(i) == Some(symbol) {
                return false;
            }
            at = facts.definitions[i].parent;
        }
        true
    })
}

/// How a document's tokens may be named, beyond [`rb_scip::may_name`] (contract M3b amendments
/// 59 and 66): each local symbol's name, and each import alias with the symbol it stands for.
struct Naming<'a> {
    /// A local symbol's `SymbolInformation.display_name` (scip-java), else the text at its
    /// definition occurrence as the decoder read it (`Document::local_names`; scip-typescript
    /// gives no display names).
    local_names: BTreeMap<&'a str, &'a str>,
    /// A default import's local name and the symbol of the import's own occurrence there: the
    /// first occurrence inside the import statement that is not a module (its descriptor does
    /// not end with `/`). scip-typescript gives the local name the symbol the export resolves
    /// to (`h.ts/helper().`, `K#`, through a re-export `index.d.ts/validator/isLatLong().`),
    /// never a `default` descriptor (measured, amendment 66).
    default_imports: BTreeMap<&'a str, &'a str>,
    /// Named imports `{ name as local }`: `local` → `name`.
    named_imports: BTreeMap<&'a str, &'a str>,
}

impl<'a> Naming<'a> {
    fn new(
        doc: &'a rb_scip::Document,
        facts: &'a FileFacts,
        by_span: &BTreeMap<Span, Vec<&'a rb_scip::Occurrence>>,
    ) -> Self {
        let mut local_names: BTreeMap<&str, &str> = doc
            .local_names
            .iter()
            .map(|(symbol, name)| (symbol.as_str(), name.as_str()))
            .collect();
        for s in &doc.symbols {
            if s.symbol.starts_with("local ") && !s.display_name.is_empty() {
                local_names.insert(s.symbol.as_str(), s.display_name.as_str());
            }
        }
        let mut default_imports = BTreeMap::new();
        let mut named_imports = BTreeMap::new();
        for import in &facts.imports {
            for n in &import.names {
                if n.name != "default" {
                    named_imports.insert(n.local.as_str(), n.name.as_str());
                    continue;
                }
                let start: Span = [import.span[0], import.span[1], i32::MIN, i32::MIN];
                let end: Span = [import.span[2], import.span[3], i32::MAX, i32::MAX];
                let site = by_span
                    .range(start..=end)
                    .filter(|(span, _)| (span[2], span[3]) <= (import.span[2], import.span[3]))
                    .flat_map(|(_, os)| os.iter())
                    .find(|o| !o.symbol.ends_with('/'));
                if let Some(o) = site {
                    default_imports.insert(n.local.as_str(), o.symbol.as_str());
                }
            }
        }
        Naming {
            local_names,
            default_imports,
            named_imports,
        }
    }

    /// Whether occurrence `o`, matched at a token whose text is `text`, names that token
    /// (contract M3b amendments 59 and 66), so that an occurrence a position error moved onto
    /// another token is not taken for it:
    /// - a global symbol in one of the forms [`rb_scip::may_name`] accepts; at a default
    ///   import's local name, the symbol of that import's own occurrence; at a named import's
    ///   local name `{ name as text }`, a symbol that may name `name`;
    /// - a local symbol whose name is known when that name is the text; else, when it is
    ///   bound to a definition of this document (`bound`, that definition's name), when the
    ///   definition's name is the text; else (no name, no definition here: no edge can come
    ///   of it) it is accepted.
    fn names_token(&self, o: &rb_scip::Occurrence, bound: Option<&str>, text: &str) -> bool {
        if o.symbol.starts_with("local ") {
            return match self.local_names.get(o.symbol.as_str()) {
                Some(name) => *name == text,
                None => bound.is_none_or(|name| name == text),
            };
        }
        rb_scip::may_name(&o.symbol, text)
            || self.default_imports.get(text) == Some(&o.symbol.as_str())
            || self
                .named_imports
                .get(text)
                .is_some_and(|name| rb_scip::may_name(&o.symbol, name))
    }
}

/// Whether Tree-sitter contradicts the index about a document's definitions (contract M3b
/// amendment 64, `definition_disagreement`): a global definition occurrence of a method or a
/// type (its descriptor ends with `).` or `#`) that no Tree-sitter definition accounts for —
/// none has its name there with a name the symbol may name ([`rb_scip::may_name`]), and none
/// has it before its name naming it (the amendment-7 fallback) — and that lies where
/// Tree-sitter reads something else: on a definition's name or a reference, or inside a
/// function or method (whose body cannot hold a global method or type the syntax misses).
/// Outside those places Tree-sitter deliberately captures some forms not at all, and they are
/// not disagreements: Java annotation interfaces and their elements, scip-typescript's
/// `<constructor>` at the `constructor` keyword (measured on the four public repositories:
/// 52 such occurrences, 0 documents rejected).
fn definition_disagreement(facts: &FileFacts, occurrences: &[rb_scip::Occurrence]) -> bool {
    let tokens: BTreeSet<Span> = facts
        .definitions
        .iter()
        .map(|d| d.name_span)
        .chain(facts.references.iter().map(|r| r.span))
        .collect();
    let inside = |outer: Span, inner: Span| {
        (outer[0], outer[1]) <= (inner[0], inner[1]) && (inner[2], inner[3]) <= (outer[2], outer[3])
    };
    occurrences.iter().any(|o| {
        if !is_global_definition(o) || !(o.symbol.ends_with(").") || o.symbol.ends_with('#')) {
            return false;
        }
        let name = rb_scip::last_descriptor_name(&o.symbol);
        let accounted = facts.definitions.iter().any(|d| {
            (d.name_span == o.span && rb_scip::may_name(&o.symbol, &d.name))
                || (name.as_deref() == Some(d.name.as_str())
                    && inside(
                        [d.span[0], d.span[1], d.name_span[0], d.name_span[1]],
                        o.span,
                    ))
        });
        !accounted
            && (tokens.contains(&o.span)
                || facts.definitions.iter().any(|d| {
                    matches!(
                        d.kind,
                        rb_syntax::DefKind::Function | rb_syntax::DefKind::Method
                    ) && inside(d.span, o.span)
                }))
    })
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
                definitions_without_occurrence: 0,
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
                relationships_anonymous: 0,
                documents_rejected: 0,
                documents_rejected_reasons: BTreeMap::new(),
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
        // versa, so neither matches, and since each lies inside a method Tree-sitter has, the
        // document disagrees with the syntax and is rejected whole (contract M3b amendment 64).
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
        assert!(joined.keys.is_empty() && joined.documented.is_empty());
        assert_eq!(
            (
                joined.coverage.definitions_total,
                joined.coverage.documents_rejected,
                joined.coverage.documents_rejected_reasons
                    [&rb_scip::Rejection::DefinitionDisagreement]
            ),
            (0, 1, 1)
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
    /// heuristic key, counted as matched. The layout is scip-typescript's (a class declared
    /// inside a method is a local symbol); scip-java gives local classes global symbols
    /// (amendment 62).
    #[test]
    fn a_call_to_a_local_definition_is_exact() {
        let src = "export class C {\n  run() {\n    class Local { go() {} }\n    new Local().go();\n  }\n}\n";
        let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
        let path = "src/c.ts";
        let sym = |d: &str| format!("scip-typescript npm t 1.0.0 src/`c.ts`/C#{d}");
        let index = ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences: vec![
                    occ_at(at_nth(src, 0, "C", 0), &sym(""), 1),
                    occ_at(at_nth(src, 1, "run", 0), &sym("run()."), 1),
                    occ_at(at_nth(src, 2, "Local", 0), "local 6", 1),
                    occ_at(at_nth(src, 2, "go", 0), "local 7", 1),
                    occ_at(at_nth(src, 3, "Local", 0), "local 6", 0),
                    occ_at(at_nth(src, 3, "go", 0), "local 7", 0),
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
        let files = BTreeMap::from([(path, (Language::TypeScript, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> = BTreeMap::from([(path, heuristic.clone())]);
        let joined = join(&index, "", &files, &keys, &[Language::TypeScript]);
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
        let index = ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences: vec![
                    occ_at(at_nth(src, 1, "A", 0), &sym("A#"), 1),
                    // `A#m()`'s definition occurrence inside the `interface` keyword: unmatched
                    // (outside any method and on no token, so no disagreement either).
                    occ_at(at_nth(src, 1, "e", 0), &sym("A#m()."), 1),
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
        // Contract M3b amendment 62: the anonymous class's link into the project is counted
        // on its own; `A#m()`'s occurrence sits in `interface` and `B#m()`'s on the first `m`
        // of its line (in `implements`), so both are definitions without an occurrence (still
        // in project: each lies on its definition's name line).
        assert_eq!(joined.coverage.relationships_anonymous, 1, "local 5 → A#");
        assert_eq!(
            joined.coverage.definitions_without_occurrence, 2,
            "A#m() and B#m()"
        );
    }

    /// Contract M3b amendment 60 (the evaluator's I2): scip-typescript keys classes declared
    /// inside a function as locals and gives them no display name. `B extends A` inside `inner`
    /// relates `local 2 → local 0` and `local 3 → local 1`; both ends are bound locals, so the
    /// dropped links are counted in project, whatever their display names.
    #[test]
    fn typescript_local_to_local_relationships_are_counted_in_project() {
        let src = "export function inner() {\n  class A { m() { return 1; } }\n  class B extends A { m() { return 2; } }\n  return new B().m();\n}\n";
        let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
        let path = "src/mixin.ts";
        let index = ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences: vec![
                    occ_at(
                        at_nth(src, 0, "inner", 0),
                        "scip-typescript npm t 1.0.0 src/`mixin.ts`/inner().",
                        1,
                    ),
                    occ_at(at_nth(src, 1, "A", 0), "local 0", 1),
                    occ_at(at_nth(src, 1, "m", 0), "local 1", 1),
                    occ_at(at_nth(src, 2, "B", 0), "local 2", 1),
                    occ_at(at_nth(src, 2, "A", 0), "local 0", 0),
                    occ_at(at_nth(src, 2, "m", 0), "local 3", 1),
                ],
                symbols: vec![
                    implementation("local 2", "local 0"),
                    implementation("local 3", "local 1"),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(
            index.documents[0]
                .symbols
                .iter()
                .all(|s| s.display_name.is_empty())
        );
        let files = BTreeMap::from([(path, (Language::TypeScript, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> =
            BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
        let joined = join(&index, "", &files, &keys, &[Language::TypeScript]);
        assert_eq!(joined.coverage.definitions_local, 4);
        assert_eq!(joined.coverage.relationships_dropped, 2);
        assert_eq!(joined.coverage.relationships_dropped_in_project, 2);
        assert_eq!(joined.coverage.relationships_anonymous, 0);
    }

    /// Contract M3b amendment 59, second line of defence (the evaluator's I1), and amendment 64:
    /// the decoder of pass 6 read scip-java's lines as `\n`-separated, so after the lone CR in
    /// `LoneCr2.java` the definition occurrence of `m0` sat at `m2`'s name, `m2`'s at `m3`'s
    /// and `zeta`'s at `beta`'s, and the call occurrence of `zeta()` at the `beta()` call. These
    /// are those spans (the measured raw ranges, read as Tree-sitter rows): a method's
    /// occurrence on another method's name is a disagreement, and the document is rejected
    /// whole. Where the definitions agree, an occurrence must still name its token: a call
    /// occurrence moved onto another call, or a field's occurrence on a method's name, is not
    /// taken for it, and is counted (references unmatched, definitions without occurrence).
    #[test]
    fn an_occurrence_must_name_its_token() {
        let src = "package t;\n\npublic class LoneCr2 {\n\tint m1() { return 0; }\r\tint m0() { return 1; }\n\tint m2() { return zeta(); }\n\tint m3() { return beta(); }\n\tint zeta() { return 2; }\n\tint beta() { return 3; }\n}\n";
        let facts = rb_syntax::extract(Language::Java, src.as_bytes()).unwrap();
        let path = "src/main/java/t/LoneCr2.java";
        let sym = |d: &str| format!("scip-java maven . . t/LoneCr2#{d}");
        let raw = |line: i32, start: i32, end: i32| -> Span { [line, start, line, end] };
        let def_at = |name: &str| {
            facts
                .definitions
                .iter()
                .find(|d| d.name == name)
                .unwrap()
                .name_span
        };
        let call_at = |name: &str| {
            facts
                .references
                .iter()
                .find(|r| r.name == name)
                .unwrap()
                .span
        };
        // The spans are where the evaluator found them: `m0`'s occurrence on `m2`'s name and
        // `zeta`'s call occurrence on the `beta` call.
        assert_eq!(def_at("m2"), raw(4, 5, 7));
        assert_eq!(def_at("beta"), raw(7, 5, 9));
        assert_eq!(call_at("beta"), raw(5, 19, 23));
        let files = BTreeMap::from([(path, (Language::Java, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> =
            BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
        let joined_with = |occurrences: Vec<Occurrence>| {
            let index = ScipIndex {
                documents: vec![Document {
                    path: path.into(),
                    occurrences,
                    ..Default::default()
                }],
                ..Default::default()
            };
            join(&index, "", &files, &keys, &[Language::Java])
        };
        let joined = joined_with(vec![
            occ_at(raw(2, 13, 20), &sym(""), 1),
            occ_at(raw(2, 13, 20), &sym("`<init>`()."), 1),
            occ_at(raw(3, 5, 7), &sym("m1()."), 1),
            occ_at(raw(4, 5, 7), &sym("m0()."), 1),
            occ_at(raw(5, 5, 7), &sym("m2()."), 1),
            occ_at(raw(5, 19, 23), &sym("zeta()."), 0),
            occ_at(raw(6, 5, 7), &sym("m3()."), 1),
            occ_at(raw(6, 19, 23), &sym("beta()."), 0),
            occ_at(raw(7, 5, 9), &sym("zeta()."), 1),
            occ_at(raw(8, 5, 9), &sym("beta()."), 1),
        ]);
        assert!(joined.keys.is_empty(), "{:?}", joined.keys);
        assert!(joined.edges.is_empty(), "{:?}", edge_lines(&joined));
        assert_eq!(joined.coverage.definitions_total, 0);
        assert_eq!(
            joined.coverage.documents_rejected_reasons,
            BTreeMap::from([(rb_scip::Rejection::DefinitionDisagreement, 1)])
        );

        // The definitions at their names; a field's occurrence (not a method or a type) on
        // `m2`'s name, and `zeta()`'s call occurrence on the `beta()` call.
        let mut occurrences: Vec<Occurrence> = ["m1", "m0", "m3", "zeta", "beta"]
            .iter()
            .map(|name| occ_at(def_at(name), &sym(&format!("{name}().")), 1))
            .collect();
        occurrences.push(occ_at(raw(2, 13, 20), &sym(""), 1));
        occurrences.push(occ_at(def_at("m2"), &sym("m0."), 1));
        occurrences.push(occ_at(call_at("beta"), &sym("zeta()."), 0));
        let joined = joined_with(occurrences);
        let replaced: Vec<&str> = joined.keys[path]
            .keys()
            .map(|i| facts.definitions[*i].name.as_str())
            .collect();
        assert_eq!(replaced, ["LoneCr2", "m1", "m0", "m3", "zeta", "beta"]);
        assert!(joined.edges.is_empty(), "{:?}", edge_lines(&joined));
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(joined.coverage.definitions_without_occurrence, 1, "m2");
        assert_eq!(
            joined.coverage.references_unmatched, 2,
            "the zeta() call has no occurrence, the beta() call's names zeta"
        );
    }

    /// One document of `tool` (`scip-java` or `scip-typescript`) as an encoded `index.scip`,
    /// with occurrences `(symbol, roles, raw line, raw start, raw end)`.
    fn encoded_index(
        tool: &str,
        path: &str,
        occurrences: &[(String, i32, i32, i32, i32)],
    ) -> Vec<u8> {
        use protobuf::Message as _;
        scip::types::Index {
            metadata: Some(scip::types::Metadata {
                tool_info: Some(scip::types::ToolInfo {
                    name: tool.into(),
                    ..Default::default()
                })
                .into(),
                ..Default::default()
            })
            .into(),
            documents: vec![scip::types::Document {
                relative_path: path.into(),
                occurrences: occurrences
                    .iter()
                    .map(
                        |(symbol, roles, line, start, end)| scip::types::Occurrence {
                            symbol: symbol.clone(),
                            symbol_roles: *roles,
                            range: vec![*line, *start, *end],
                            ..Default::default()
                        },
                    )
                    .collect(),
                ..Default::default()
            }],
            ..Default::default()
        }
        .write_to_bytes()
        .unwrap()
    }

    /// Contract M3b amendment 59 (the evaluator's I1), through the decoder: the raw ranges
    /// `rb-scip-java:0.13.1` and scip-typescript 0.4.0 emitted for a lone CR (both) and a
    /// U+2028 inside a string literal (TypeScript), decoded from the files' text and joined.
    /// Every definition gets its own key, and each call is evidenced at its own token.
    #[test]
    fn indexer_line_terminators_join_on_the_tokens_the_indexer_meant() {
        // (language, tool, path, symbol prefix, text, measured occurrences, expected calls
        // `(caller, callee, the callee token's nth call)`).
        type Case<'a> = (
            Language,
            &'a str,
            &'a str,
            &'a str,
            &'a str,
            Vec<(&'a str, i32, i32, i32, i32)>,
            Vec<(&'a str, &'a str)>,
        );
        let cases: Vec<Case<'_>> = vec![
            (
                Language::Java,
                "scip-java",
                "t/LoneCr2.java",
                "scip-java maven . . t/LoneCr2#",
                "package t;\n\npublic class LoneCr2 {\n\tint m1() { return 0; }\r\tint m0() { return 1; }\n\tint m2() { return zeta(); }\n\tint m3() { return beta(); }\n\tint zeta() { return 2; }\n\tint beta() { return 3; }\n}\n",
                vec![
                    ("", 1, 2, 13, 20),
                    ("m1().", 1, 3, 5, 7),
                    ("m0().", 1, 4, 5, 7),
                    ("m2().", 1, 5, 5, 7),
                    ("zeta().", 0, 5, 19, 23),
                    ("m3().", 1, 6, 5, 7),
                    ("beta().", 0, 6, 19, 23),
                    ("zeta().", 1, 7, 5, 9),
                    ("beta().", 1, 8, 5, 9),
                ],
                vec![("m2", "zeta"), ("m3", "beta")],
            ),
            (
                Language::TypeScript,
                "scip-typescript",
                "src/cr.ts",
                "scip-typescript npm eol-ts 1.0.0 src/`cr.ts`/K#",
                "export class K {\n  m1() { return 0; }\r  m0() { return 1; }\n  m2() { return this.zeta(); }\n  m3() { return this.beta(); }\n  zeta() { return 2; }\n  beta() { return 3; }\n}\n",
                vec![
                    ("", 1, 0, 13, 14),
                    ("m1().", 1, 1, 2, 4),
                    ("m0().", 1, 2, 2, 4),
                    ("m2().", 1, 3, 2, 4),
                    ("zeta().", 0, 3, 21, 25),
                    ("m3().", 1, 4, 2, 4),
                    ("beta().", 0, 4, 21, 25),
                    ("zeta().", 1, 5, 2, 6),
                    ("beta().", 1, 6, 2, 6),
                ],
                vec![("m2", "zeta"), ("m3", "beta")],
            ),
            (
                Language::TypeScript,
                "scip-typescript",
                "src/ls.ts",
                "scip-typescript npm eol-ts 1.0.0 src/`ls.ts`/L#",
                "export class L {\n  m1() { return \"a\u{2028}b\"; }\n  m0() { return 1; }\n  m2() { return this.zeta(); }\n  // c \u{2029} d\n  m3() { return this.beta(); }\n  /* e \u{85} f */ m4() { return this.m3(); }\n  zeta() { return 2; }\n  beta() { return 3; }\n}\n",
                vec![
                    ("", 1, 0, 13, 14),
                    ("m1().", 1, 1, 2, 4),
                    ("m0().", 1, 3, 2, 4),
                    ("m2().", 1, 4, 2, 4),
                    ("zeta().", 0, 4, 21, 25),
                    // After U+2029 the line comment ends for TypeScript: `d` is a property.
                    ("d.", 1, 6, 1, 2),
                    ("m3().", 1, 7, 2, 4),
                    ("beta().", 0, 7, 21, 25),
                    ("m4().", 1, 8, 14, 16),
                    ("m3().", 0, 8, 33, 35),
                    ("zeta().", 1, 9, 2, 6),
                    ("beta().", 1, 10, 2, 6),
                ],
                vec![("m2", "zeta"), ("m3", "beta"), ("m4", "m3")],
            ),
        ];
        for (language, tool, path, prefix, src, measured, calls) in cases {
            let occurrences: Vec<(String, i32, i32, i32, i32)> = measured
                .iter()
                .map(|&(d, roles, line, start, end)| {
                    (format!("{prefix}{d}"), roles, line, start, end)
                })
                .collect();
            let bytes = encoded_index(tool, path, &occurrences);
            let mut index =
                rb_scip::decode(&bytes, &|p| (p == path).then(|| src.as_bytes().to_vec())).unwrap();
            let facts = rb_syntax::extract(language, src.as_bytes()).unwrap();
            let files = BTreeMap::from([(path, (language, &facts))]);
            let keys: BTreeMap<&str, Vec<String>> =
                BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
            // A lone CR in a Java file is a line terminator javac counts and Tree-sitter-java
            // does not: since pass 7 the file stays Tier B whole (contract M3b amendment 64).
            // The conversion is still exercised through the join with the rejection lifted.
            if tool == "scip-java" {
                assert_eq!(
                    index.documents[0].rejected,
                    Some(rb_scip::Rejection::CommentTerminator)
                );
                let joined = join(&index, "", &files, &keys, &[language]);
                assert!(joined.keys.is_empty() && joined.edges.is_empty(), "{path}");
                assert_eq!(joined.coverage.documents_rejected, 1);
                index.documents[0].rejected = None;
            } else {
                assert_eq!(index.documents[0].rejected, None, "{path}");
            }
            let joined = join(&index, "", &files, &keys, &[language]);
            // Every method keeps its own name in its key.
            for (i, d) in facts.definitions.iter().enumerate() {
                let expected = match d.kind {
                    rb_syntax::DefKind::Class => prefix.to_owned(),
                    _ => format!("{prefix}{}().", d.name),
                };
                assert_eq!(
                    joined.keys[path].get(&i),
                    Some(&expected),
                    "{path}: {}",
                    d.name
                );
            }
            assert_eq!(
                joined.coverage.definitions_matched, joined.coverage.definitions_total,
                "{path}"
            );
            // Each call is evidenced at its own callee token.
            let mut got: Vec<(String, String, Span)> = joined
                .edges
                .iter()
                .filter(|e| e.kind == EdgeKind::Calls)
                .map(|e| {
                    let Source::Symbol(src_key) = &e.src else {
                        panic!("a call outside a method")
                    };
                    (src_key.clone(), e.dst.clone(), e.at.span)
                })
                .collect();
            got.sort();
            let mut expected: Vec<(String, String, Span)> = calls
                .iter()
                .map(|&(caller, callee)| {
                    let token = format!(".{callee}()");
                    let token = if language == Language::Java {
                        format!(" {callee}()")
                    } else {
                        token
                    };
                    let offset = src.find(&token).unwrap() + 1;
                    let row = src[..offset].matches('\n').count() as i32;
                    let col = (offset - src[..offset].rfind('\n').map_or(0, |n| n + 1)) as i32;
                    (
                        format!("{prefix}{caller}()."),
                        format!("{prefix}{callee}()."),
                        [row, col, row, col + callee.len() as i32],
                    )
                })
                .collect();
            expected.sort();
            assert_eq!(got, expected, "{path}");
        }
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
                definitions_without_occurrence: 0,
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
                relationships_anonymous: 0,
                documents_rejected: 0,
                documents_rejected_reasons: BTreeMap::new(),
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
        // Every method's definition occurrence now lies inside the method, off its name: the
        // document disagrees with the syntax and the file is left to Tier B whole (contract M3b
        // amendment 64), counted as rejected and not as a file without a document.
        let joined = join(&shifted, "svc", &files, &keys, java);
        assert!(joined.keys.is_empty() && joined.edges.is_empty());
        assert!(joined.documented.is_empty());
        assert_eq!(joined.coverage.definitions_total, 0);
        assert_eq!(joined.coverage.references_unmatched, 0);
        assert_eq!(joined.coverage.files_without_document, 0);
        assert_eq!(joined.coverage.documents_rejected, 1);
    }

    /// The span Tree-sitter gives the `nth` match of `needle` in `src` (rows by `\n`, byte
    /// columns), cut to the identifier it starts with (`b()` stands for `b`).
    fn tok(src: &str, needle: &str, nth: usize) -> Span {
        let offset = src
            .match_indices(needle)
            .nth(nth)
            .unwrap_or_else(|| panic!("{needle:?} #{nth}"))
            .0;
        let len = needle
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .unwrap_or(needle.len());
        let row = src[..offset].matches('\n').count() as i32;
        let col = (offset - src[..offset].rfind('\n').map_or(0, |n| n + 1)) as i32;
        [row, col, row, col + len as i32]
    }

    /// Joins one index over Java or TypeScript files given as `(path, source)`.
    fn join_sources(language: Language, sources: &[(&str, &str)], index: &ScipIndex) -> Joined {
        let facts: Vec<(&str, FileFacts)> = sources
            .iter()
            .map(|(path, src)| (*path, rb_syntax::extract(language, src.as_bytes()).unwrap()))
            .collect();
        let files: BTreeMap<&str, (Language, &FileFacts)> = facts
            .iter()
            .map(|(path, f)| (*path, (language, f)))
            .collect();
        let keys: BTreeMap<&str, Vec<String>> = facts
            .iter()
            .map(|(path, f)| (*path, rb_syntax::symbol_keys(path, f)))
            .collect();
        join(index, "", &files, &keys, &[language])
    }

    const COMMENT_CR: &str = "package t;\n\npublic class CommentCr {\n\tint good() { // note\r return a(); } int evil() {\n\t\treturn b();\n\t}\n\tint a() { return 1; }\n\tint b() { return 2; }\n}\n";
    const PLAIN: &str = "package t;\n\npublic class Plain {\n    int a() { return b(); }\n    int b() { return 1; }\n}\n";

    /// `Plain.java` as scip-java indexes it: every definition and the call at its token.
    fn plain_document() -> Document {
        let sym = |d: &str| format!("scip-java maven . . t/Plain#{d}");
        Document {
            path: "t/Plain.java".into(),
            occurrences: vec![
                occ_at(tok(PLAIN, "Plain", 0), &sym(""), 1),
                occ_at(tok(PLAIN, "a()", 0), &sym("a()."), 1),
                occ_at(tok(PLAIN, "b()", 0), &sym("b()."), 0),
                occ_at(tok(PLAIN, "b()", 1), &sym("b()."), 1),
            ],
            ..Default::default()
        }
    }

    /// Contract M3b amendment 64 (the evaluator's J2): javac ends `good`'s `// note` comment at
    /// the lone CR, so it compiles `good() { return a(); }` and `evil() { return b(); }`;
    /// Tree-sitter-java's comment runs to `\n`, so its `good` holds `return b();` and there is
    /// no `evil`. The decoder rejects the document for the lone CR (`comment_terminator`);
    /// with that check bypassed (a hand-built document, the occurrences at the spans
    /// `rb-scip-java:0.13.1` gave them on `var/fix-pass7/scratch/t7`), `evil`'s definition
    /// occurrence inside Tree-sitter's `good` rejects it (`definition_disagreement`). Either way
    /// the whole file stays Tier B, `good → b` is never an exact edge, and a clean document of
    /// the same index keeps its exact edges.
    #[test]
    fn a_document_the_parsers_read_differently_stays_heuristic_whole() {
        let path = "t/CommentCr.java";
        let sym = |d: &str| format!("scip-java maven . . t/CommentCr#{d}");
        let hand_built = Document {
            path: path.into(),
            occurrences: vec![
                occ_at(tok(COMMENT_CR, "CommentCr", 0), &sym(""), 1),
                occ_at(tok(COMMENT_CR, "CommentCr", 0), &sym("`<init>`()."), 1),
                occ_at(tok(COMMENT_CR, "good", 0), &sym("good()."), 1),
                occ_at(tok(COMMENT_CR, "a()", 0), &sym("a()."), 0),
                occ_at(tok(COMMENT_CR, "evil", 0), &sym("evil()."), 1),
                occ_at(tok(COMMENT_CR, "b()", 0), &sym("b()."), 0),
                occ_at(tok(COMMENT_CR, "a()", 1), &sym("a()."), 1),
                occ_at(tok(COMMENT_CR, "b()", 1), &sym("b()."), 1),
            ],
            ..Default::default()
        };
        assert_eq!(
            tok(COMMENT_CR, "evil", 0),
            [3, 41, 3, 45],
            "the measured span"
        );
        assert_eq!(
            tok(COMMENT_CR, "b()", 0),
            [4, 9, 4, 10],
            "the measured span"
        );
        let bytes = encoded_index("scip-java", path, &[]);
        let decoded = rb_scip::decode(&bytes, &|p| {
            (p == path).then(|| COMMENT_CR.as_bytes().to_vec())
        })
        .unwrap();
        for (document, reason) in [
            (
                decoded.documents[0].clone(),
                rb_scip::Rejection::CommentTerminator,
            ),
            (hand_built, rb_scip::Rejection::DefinitionDisagreement),
        ] {
            // `User` calls into the rejected file: its symbols have no node, like those of a
            // file without a document (`references_undocumented`).
            let user = "package t;\nclass User { int u() { return new CommentCr().good(); } }\n";
            let user_document = Document {
                path: "t/User.java".into(),
                occurrences: vec![
                    occ_at(tok(user, "User", 0), "scip-java maven . . t/User#", 1),
                    occ_at(tok(user, "u()", 0), "scip-java maven . . t/User#u().", 1),
                    occ_at(tok(user, "CommentCr", 0), &sym(""), 0),
                    occ_at(tok(user, "good", 0), &sym("good()."), 0),
                ],
                ..Default::default()
            };
            let index = ScipIndex {
                documents: vec![document, plain_document(), user_document],
                ..Default::default()
            };
            let joined = join_sources(
                Language::Java,
                &[
                    (path, COMMENT_CR),
                    ("t/Plain.java", PLAIN),
                    ("t/User.java", user),
                ],
                &index,
            );
            assert_eq!(
                (
                    joined.coverage.references_undocumented,
                    joined.coverage.references_unanchored
                ),
                (2, 0)
            );
            assert_eq!(
                edge_lines(&joined),
                ["scip-java maven . . t/Plain#a(). Calls scip-java maven . . t/Plain#b()."],
                "{reason:?}"
            );
            assert_eq!(
                joined.documented,
                BTreeSet::from(["t/Plain.java".to_owned(), "t/User.java".to_owned()])
            );
            assert!(!joined.keys.contains_key(path));
            assert_eq!(
                joined.coverage.documents_rejected_reasons,
                BTreeMap::from([(reason, 1)])
            );
            assert_eq!(joined.coverage.documents_rejected, 1);
            assert_eq!(joined.coverage.files_without_document, 0);
            assert_eq!(
                joined.coverage.definitions_total, 5,
                "Plain's and User's only"
            );
        }
    }

    /// Contract M3b amendment 64 (the evaluator's J1): `Enc.java` is valid UTF-8 in a build
    /// declaring ISO-8859-1, so javac counts each `é` as two units and every later position of
    /// the line lands right of its token. The spans are the ones `rb-scip-java:0.13.1` gave on
    /// `var/fix-pass7/scratch/t7enc`, decoded as UTF-8: `p()`'s definition on `q`'s name, the
    /// `z()` call occurrence on the `z` of `z(1)`, `q()` and `z(+1)` at the line's end. The
    /// document disagrees with the syntax even without the pom's signal, so `q → z()` is never
    /// an exact edge; with the pom's signal the decoder's caller records `source_encoding`
    /// first. A Latin-1 byte (`Lat.java`) is `not_utf8`.
    #[test]
    fn a_document_read_in_another_encoding_is_rejected() {
        let src = format!(
            "package t;\n\npublic class Enc {{\n    int z() {{ return 0; }}\n    int z(int x) {{ return x; }}\n    /* {} */ int p() {{ return z(); }} int q() {{ return z(1); }}\n}}\n",
            "é".repeat(24)
        );
        let src = src.as_str();
        let path = "t/Enc.java";
        let sym = |d: &str| format!("scip-java maven . . t/Enc#{d}");
        let end = src.lines().nth(5).unwrap().len() as i32;
        let document = Document {
            path: path.into(),
            occurrences: vec![
                occ_at(tok(src, "Enc", 0), &sym(""), 1),
                occ_at(tok(src, "z()", 0), &sym("z()."), 1),
                occ_at(tok(src, "z(int", 0), &sym("z(+1)."), 1),
                occ_at(tok(src, "q()", 0), &sym("p()."), 1),
                occ_at(tok(src, "z(1)", 0), &sym("z()."), 0),
                occ_at([5, end, 5, end], &sym("q()."), 1),
                occ_at([5, end, 5, end], &sym("z(+1)."), 0),
            ],
            ..Default::default()
        };
        assert_eq!(tok(src, "q()", 0), [5, 87, 5, 88], "the measured span");
        assert_eq!(tok(src, "z(1)", 0), [5, 100, 5, 101], "the measured span");
        for (rejected, reason) in [
            (None, rb_scip::Rejection::DefinitionDisagreement),
            (
                Some(rb_scip::Rejection::SourceEncoding),
                rb_scip::Rejection::SourceEncoding,
            ),
        ] {
            let index = ScipIndex {
                documents: vec![Document {
                    rejected,
                    ..document.clone()
                }],
                ..Default::default()
            };
            let joined = join_sources(Language::Java, &[(path, src)], &index);
            assert!(joined.edges.is_empty(), "{:?}", edge_lines(&joined));
            assert!(joined.keys.is_empty());
            assert_eq!(
                joined.coverage.documents_rejected_reasons,
                BTreeMap::from([(reason, 1)])
            );
        }
        let latin1 = src.replace(&"é".repeat(24), &"\u{e9}".repeat(12));
        let latin1: Vec<u8> = latin1
            .chars()
            .map(|c| u8::try_from(u32::from(c)).unwrap())
            .collect();
        let bytes = encoded_index("scip-java", "t/Lat.java", &[]);
        let decoded = rb_scip::decode(&bytes, &|_| Some(latin1.clone())).unwrap();
        assert_eq!(
            decoded.documents[0].rejected,
            Some(rb_scip::Rejection::NotUtf8)
        );
    }

    /// Contract M3b amendment 64: forms Tree-sitter deliberately does not capture, outside
    /// any method, are not disagreements (measured on spring-retry, jsoup and class-validator):
    /// Java annotation interfaces and their elements, top-level or nested, and
    /// scip-typescript's `<constructor>` at the `constructor` keyword.
    #[test]
    fn forms_tree_sitter_does_not_capture_are_no_disagreement() {
        let java = "package p;\npublic @interface Backoff {\n  long delay() default 0;\n}\nclass Outer {\n  @interface Inner { int value(); }\n  void run() {}\n}\n";
        let sym = |d: &str| format!("scip-java maven . . p/{d}");
        let index = ScipIndex {
            documents: vec![Document {
                path: "p/Backoff.java".into(),
                occurrences: vec![
                    occ_at(tok(java, "Backoff", 0), &sym("Backoff#"), 1),
                    occ_at(tok(java, "delay", 0), &sym("Backoff#delay()."), 1),
                    occ_at(tok(java, "Outer", 0), &sym("Outer#"), 1),
                    occ_at(tok(java, "Inner", 0), &sym("Outer#Inner#"), 1),
                    occ_at(tok(java, "value", 0), &sym("Outer#Inner#value()."), 1),
                    occ_at(tok(java, "run", 0), &sym("Outer#run()."), 1),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let joined = join_sources(Language::Java, &[("p/Backoff.java", java)], &index);
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(
            joined.coverage.definitions_matched, joined.coverage.definitions_total,
            "Outer and run"
        );
        let ts = "export class K {\n  constructor() {}\n  m() { return 1; }\n}\n";
        let sym = |d: &str| format!("scip-typescript npm t 1.0.0 src/`k.ts`/K#{d}");
        let index = ScipIndex {
            documents: vec![Document {
                path: "src/k.ts".into(),
                occurrences: vec![
                    occ_at(tok(ts, "K", 0), &sym(""), 1),
                    occ_at(tok(ts, "constructor", 0), &sym("`<constructor>`()."), 1),
                    occ_at(tok(ts, "m()", 0), &sym("m()."), 1),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let joined = join_sources(Language::TypeScript, &[("src/k.ts", ts)], &index);
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(joined.coverage.definitions_without_occurrence, 0);
        // Outside a method, a method's definition on a token Tree-sitter reads otherwise (the
        // call in a field initializer) is a disagreement.
        let field =
            "package p;\nclass F {\n  int x = compute();\n  int compute() { return 1; }\n}\n";
        let index = ScipIndex {
            documents: vec![Document {
                path: "p/F.java".into(),
                occurrences: vec![
                    occ_at(tok(field, "F", 0), "scip-java maven . . p/F#", 1),
                    occ_at(
                        tok(field, "compute()", 0),
                        "scip-java maven . . p/F#compute().",
                        1,
                    ),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let joined = join_sources(Language::Java, &[("p/F.java", field)], &index);
        assert_eq!(joined.coverage.documents_rejected, 1);
    }

    /// Contract M3b amendment 66 (the evaluator's J4): a default import's local name stands
    /// only for the symbol of that import's own occurrence (scip-typescript gives it the
    /// symbol the export resolves to, `h.ts/helper().`). `zz()` is accepted through its import
    /// `zz`; a symbol named `zzzzzz` at the `helper` token is not (it was, through any default
    /// import named `helper`), nor is the imported symbol at a token that is no import's
    /// local name (`other`).
    #[test]
    fn a_default_import_alias_names_only_its_imports_symbol() {
        let h = "export default function helper() { return 1; }\n";
        let alias = "import helper from './h';\nimport zz from './h';\nexport class A2 { zzzzzz() { return 1; } }\nexport function q() { return helper(); }\nexport function r() { return zz(); }\nexport function s() { return other(); }\nfunction other() { return 2; }\n";
        let pkg = "scip-typescript npm t 1.0.0 src/";
        let helper = format!("{pkg}`h.ts`/helper().");
        let module = format!("{pkg}`h.ts`/");
        let sym = |d: &str| format!("{pkg}`alias.ts`/{d}");
        let index = ScipIndex {
            documents: vec![
                Document {
                    path: "src/h.ts".into(),
                    occurrences: vec![occ_at(tok(h, "helper", 0), &helper, 1)],
                    ..Default::default()
                },
                Document {
                    path: "src/alias.ts".into(),
                    occurrences: vec![
                        occ_at(tok(alias, "helper", 0), &helper, 0),
                        occ_at(tok(alias, "'./h'", 0), &module, 0),
                        occ_at(tok(alias, "zz", 0), &helper, 0),
                        occ_at(tok(alias, "'./h'", 1), &module, 0),
                        occ_at(tok(alias, "A2", 0), &sym("A2#"), 1),
                        occ_at(tok(alias, "zzzzzz", 0), &sym("A2#zzzzzz()."), 1),
                        occ_at(tok(alias, "q() {", 0), &sym("q()."), 1),
                        occ_at(tok(alias, "helper()", 0), &sym("A2#zzzzzz()."), 0),
                        occ_at(tok(alias, "r() {", 0), &sym("r()."), 1),
                        occ_at(tok(alias, "zz();", 0), &helper, 0),
                        occ_at(tok(alias, "s() {", 0), &sym("s()."), 1),
                        occ_at(tok(alias, "other()", 0), &helper, 0),
                        occ_at(tok(alias, "other()", 1), &sym("other()."), 1),
                    ],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let joined = join_sources(
            Language::TypeScript,
            &[("src/h.ts", h), ("src/alias.ts", alias)],
            &index,
        );
        assert_eq!(
            edge_lines(&joined),
            [format!("{} Calls {helper}", sym("r()."))]
        );
        assert_eq!(
            joined.coverage.references_unmatched, 2,
            "helper() and other()"
        );
        assert_eq!(joined.coverage.documents_rejected, 0);
    }

    /// Contract M3b amendment 66 (the evaluator's J4): a local symbol names a token only by
    /// its name (scip-java's display name, or the text at its definition occurrence) or, with
    /// none known (scip-typescript), by the definition it is bound to. A local whose name is
    /// `stop` does not bind to `go`, and `local 1` (`Local`) at the `go()` call is no call to
    /// `Local`.
    #[test]
    fn a_local_symbol_names_its_token_by_its_name_or_its_binding() {
        let src = "export function outer() {\n  class Local { go() { return 1; } }\n  return new Local().go();\n}\n";
        let path = "src/o.ts";
        let outer = "scip-typescript npm t 1.0.0 src/`o.ts`/outer().";
        let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
        let heuristic = rb_syntax::symbol_keys(path, &facts);
        let local = facts
            .definitions
            .iter()
            .position(|d| d.name == "Local")
            .unwrap();
        let named = |symbol: &str, name: &str| SymbolInfo {
            symbol: symbol.into(),
            display_name: name.into(),
            ..Default::default()
        };
        // With display names, as scip-java gives them; without, as scip-typescript does.
        for (go_definition, symbols, bound) in [
            (
                "local 9",
                vec![named("local 1", "Local"), named("local 9", "stop")],
                1,
            ),
            ("local 2", vec![], 2),
        ] {
            let index = ScipIndex {
                documents: vec![Document {
                    path: path.into(),
                    occurrences: vec![
                        occ_at(tok(src, "outer", 0), outer, 1),
                        occ_at(tok(src, "Local", 0), "local 1", 1),
                        occ_at(tok(src, "go()", 0), go_definition, 1),
                        occ_at(tok(src, "Local()", 0), "local 1", 0),
                        occ_at(tok(src, "go()", 1), "local 1", 0),
                    ],
                    symbols,
                    ..Default::default()
                }],
                ..Default::default()
            };
            let joined = join_sources(Language::TypeScript, &[(path, src)], &index);
            assert_eq!(
                edge_lines(&joined),
                [format!("{outer} References {}", heuristic[local])],
                "{go_definition}"
            );
            assert_eq!(joined.coverage.definitions_local, bound, "{go_definition}");
            assert_eq!(joined.coverage.references_unmatched, 1, "{go_definition}");
        }
    }

    /// `occurrence` with the enclosing range `enclosing`.
    fn enclosed(occurrence: Occurrence, enclosing: Span) -> Occurrence {
        Occurrence {
            enclosing: Some(enclosing),
            ..occurrence
        }
    }

    /// The span from the start of the `nth` match of `from` in `src` to the end of the first
    /// match of `to` at or after it (one row).
    fn between(src: &str, from: &str, nth: usize, to: &str) -> Span {
        let start = tok(src, from, nth);
        let row = src.lines().nth(start[0] as usize).unwrap();
        let end = start[1] as usize + row[start[1] as usize..].find(to).unwrap() + to.len();
        [start[0], start[1], start[0], end as i32]
    }

    /// The evaluator's K1 sources (`var/fix-pass8/esc`): line 3 holds a Unicode escape that
    /// javac translates before lexing (JLS §3.3), so `\u002a/` ends a block comment and
    /// `\u0022` a string literal for javac only.
    fn escape_source(class: &str, line: &str) -> String {
        format!(
            "package p;\n\npublic class {class} {{\n{line}\n  int a() {{ return 1; }}\n  int b() {{ return 2; }}\n}}\n"
        )
    }

    /// The occurrences `rb-scip-java:0.13.1` gave an escape source (`var/fix-pass8/esc`,
    /// decoded): the class, `good` with the enclosing range javac saw (it ends at the `}`
    /// after `a();`), the calls of `a` and `b`, and `a`, `b` with their ranges; `extra` are
    /// the line's other occurrences.
    fn escape_document(
        class: &str,
        src: &str,
        good_ends: &str,
        extra: Vec<Occurrence>,
    ) -> Document {
        let sym = |d: &str| format!("scip-java maven . . p/{class}#{d}");
        let last = src.lines().count() as i32 - 1;
        let mut occurrences = vec![
            enclosed(occ_at(tok(src, class, 0), &sym(""), 1), [2, 0, last, 1]),
            occ_at(tok(src, class, 0), &sym("`<init>`()."), 1),
            enclosed(
                occ_at(tok(src, "good", 0), &sym("good()."), 1),
                between(src, tok_text(src, 3), 0, good_ends),
            ),
            occ_at(tok(src, "a()", 0), &sym("a()."), 0),
            occ_at(tok(src, "b()", 0), &sym("b()."), 0),
            enclosed(
                occ_at(tok(src, "a()", 1), &sym("a()."), 1),
                between(src, "int a()", 0, "}"),
            ),
            enclosed(
                occ_at(tok(src, "b()", 1), &sym("b()."), 1),
                between(src, "int b()", 0, "}"),
            ),
        ];
        occurrences.extend(extra);
        occurrences.sort();
        Document {
            path: format!("p/{class}.java"),
            occurrences,
            ..Default::default()
        }
    }

    /// The first word of row `row` of `src` (the return type that starts `good`).
    fn tok_text(src: &str, row: usize) -> &str {
        src.lines()
            .nth(row)
            .unwrap()
            .split_whitespace()
            .next()
            .unwrap()
    }

    /// Contract M3b amendment 68 (the evaluator's K1): a Unicode escape other than a line
    /// terminator restructures the line for javac only. Tree-sitter-java reads `b()` inside
    /// `good`; scip-java's own enclosing range for `good` ends before it (`Esc1`: `\u002a/`
    /// ends the comment, so `{ b(); }` is an initializer; `Esc2`: `\u0022` ends the string;
    /// `Esc3`: `b()` is in field `r`'s lambda). Each document is rejected whole
    /// (`structure_disagreement`), so `good → b` is never an exact edge, while `Plain` (the
    /// ranges agree) keeps its exact edges. The spans are the ones `rb-scip-java:0.13.1`
    /// emitted on `var/fix-pass8/esc`.
    #[test]
    fn a_source_the_enclosing_ranges_contradict_is_rejected() {
        let esc1 = escape_source(
            "Esc1",
            "  int good() { /* note \\u002a/ return a(); } { /* */ b(); }",
        );
        let esc2 = escape_source(
            "Esc2",
            "  void good() { String s = \"\\u0022; a(); } { String t = \\u0022\"; b(); }",
        );
        let esc3 = escape_source(
            "Esc3",
            "  int good() { /* \\uuu002A/ return a(); } Runnable r = () -> { /* */ b(); };",
        );
        let plain = escape_source("Plain", "  int good() { return a() + b(); }");
        let jdk = "scip-java maven jdk 21 java/lang/";
        let documents = [
            escape_document("Esc1", &esc1, "a(); }", vec![]),
            escape_document(
                "Esc2",
                &esc2,
                "a(); }",
                vec![
                    occ_at(tok(&esc2, "String", 0), &format!("{jdk}String#"), 0),
                    enclosed(
                        occ_at(tok(&esc2, "s =", 0), "local 0", 1),
                        between(&esc2, "String s", 0, "0022"),
                    ),
                    occ_at(tok(&esc2, "String", 1), &format!("{jdk}String#"), 0),
                    enclosed(
                        occ_at(tok(&esc2, "t =", 0), "local 1", 1),
                        between(&esc2, "String t", 0, "\""),
                    ),
                ],
            ),
            escape_document(
                "Esc3",
                &esc3,
                "a(); }",
                vec![
                    occ_at(tok(&esc3, "Runnable", 0), &format!("{jdk}Runnable#"), 0),
                    enclosed(
                        occ_at(tok(&esc3, "r =", 0), "scip-java maven . . p/Esc3#r.", 1),
                        between(&esc3, "Runnable", 0, "};"),
                    ),
                ],
            ),
        ];
        // The measured spans (decoded columns of the real index).
        let good_range = |d: &Document| {
            d.occurrences
                .iter()
                .find(|o| o.symbol.ends_with("good()."))
                .and_then(|o| o.enclosing)
                .unwrap()
        };
        assert_eq!(good_range(&documents[0]), [3, 2, 3, 44]);
        assert_eq!(good_range(&documents[1]), [3, 2, 3, 42]);
        assert_eq!(good_range(&documents[2]), [3, 2, 3, 41]);
        assert_eq!(tok(&esc1, "b()", 0), [3, 53, 3, 54]);
        assert_eq!(tok(&esc2, "b()", 0), [3, 65, 3, 66]);
        assert_eq!(tok(&esc3, "b()", 0), [3, 69, 3, 70]);
        assert_eq!(between(&esc3, "Runnable", 0, "};"), [3, 42, 3, 76]);
        for (document, src) in documents.into_iter().zip([&esc1, &esc2, &esc3]) {
            let class = document.path.clone();
            let index = ScipIndex {
                documents: vec![document, escape_document("Plain", &plain, "}", vec![])],
                ..Default::default()
            };
            let joined = join_sources(
                Language::Java,
                &[(class.as_str(), src.as_str()), ("p/Plain.java", &plain)],
                &index,
            );
            assert_eq!(
                joined.coverage.documents_rejected_reasons,
                BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)]),
                "{class}"
            );
            assert_eq!(
                edge_lines(&joined),
                [
                    "scip-java maven . . p/Plain#good(). Calls scip-java maven . . p/Plain#a().",
                    "scip-java maven . . p/Plain#good(). Calls scip-java maven . . p/Plain#b().",
                ],
                "{class}"
            );
            assert!(!joined.documented.contains(&class), "{class}");
        }
    }

    /// Contract M3b amendment 68, the second clause: the innermost index range around a
    /// reference that belongs to an anchored definition must be the reference's Tree-sitter
    /// source or one of its ancestors. Tree-sitter reads `b()` in a field initializer (source
    /// `S`) while the index's range for `m1` holds it; and Tree-sitter reads `b()` in `m1`,
    /// which has no range here, while the index's range for `b` holds it. Both documents are
    /// rejected; with the ranges where Tree-sitter has them, neither is.
    #[test]
    fn a_reference_inside_another_definitions_range_is_rejected() {
        let src = "package t;\nclass S {\n  int m1() { return b(); }\n  int b() { return 1; }\n  int f = b();\n}\n";
        let sym = |d: &str| format!("scip-java maven . . t/S#{d}");
        let document = |m1: Option<Span>, b: Span| {
            let m1_def = occ_at(tok(src, "m1", 0), &sym("m1()."), 1);
            Document {
                path: "t/S.java".into(),
                occurrences: vec![
                    enclosed(occ_at(tok(src, "S", 0), &sym(""), 1), [1, 0, 5, 1]),
                    match m1 {
                        Some(range) => enclosed(m1_def, range),
                        None => m1_def,
                    },
                    occ_at(tok(src, "b()", 0), &sym("b()."), 0),
                    enclosed(occ_at(tok(src, "b()", 1), &sym("b()."), 1), b),
                    occ_at(tok(src, "b()", 2), &sym("b()."), 0),
                ],
                ..Default::default()
            }
        };
        let m1 = between(src, "int m1", 0, "}");
        let b = between(src, "int b", 0, "}");
        let rejected = |document: Document| {
            let index = ScipIndex {
                documents: vec![document],
                ..Default::default()
            };
            join_sources(Language::Java, &[("t/S.java", src)], &index)
                .coverage
                .documents_rejected_reasons
        };
        let structure = BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)]);
        // `m1`'s range stretched over the field (rows 2 to 4).
        assert_eq!(rejected(document(Some([2, 2, 4, 14]), b)), structure);
        // `m1` without a range, `b`'s range stretched back over `m1`'s call.
        assert_eq!(rejected(document(None, [2, 2, 3, 23])), structure);
        assert_eq!(rejected(document(Some(m1), b)), BTreeMap::new());
        assert_eq!(rejected(document(None, b)), BTreeMap::new());
        // A method of an enum constant's body is anchored by a local symbol (amendment 54);
        // its range is checked like a global one's.
        let en =
            "package t;\nenum E {\n  A { int v() { return b(); } };\n  int b() { return 1; }\n}\n";
        let sym = |d: &str| format!("scip-java maven . . t/E#{d}");
        let rejected = |v: Span| {
            let index = ScipIndex {
                documents: vec![Document {
                    path: "t/E.java".into(),
                    occurrences: vec![
                        enclosed(occ_at(tok(en, "E", 0), &sym(""), 1), [1, 0, 4, 1]),
                        enclosed(occ_at(tok(en, "v()", 0), "local 1", 1), v),
                        occ_at(tok(en, "b()", 0), &sym("b()."), 0),
                        enclosed(
                            occ_at(tok(en, "b()", 1), &sym("b()."), 1),
                            between(en, "int b", 0, "}"),
                        ),
                    ],
                    symbols: vec![SymbolInfo {
                        symbol: "local 1".into(),
                        display_name: "v".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            };
            join_sources(Language::Java, &[("t/E.java", en)], &index)
                .coverage
                .documents_rejected_reasons
        };
        let v = between(en, "int v", 0, "}");
        assert_eq!(rejected(v), BTreeMap::new());
        assert_eq!(rejected([v[0], v[1], v[2], v[1] + 9]), structure);
    }

    /// Contract M3b amendment 68: what scip-java's enclosing ranges cover, measured on
    /// `var/fix-pass8/esc/Kinds.java` with `rb-scip-java:0.13.1` (decoded), agrees with
    /// Tree-sitter's crediting everywhere: field initializers and initializer blocks to the
    /// class, an anonymous class's method, a lambda and a local variable to the enclosing
    /// method, a local class's method to itself, an enum constant body's method (a bound local)
    /// to itself, a record's and a nested class's methods to themselves. No rejection, and the
    /// exact edges are the ones before the check.
    #[test]
    fn the_measured_enclosing_ranges_agree_with_tree_sitter() {
        let src = "package p;\n\nimport java.util.function.Supplier;\n\npublic class Kinds implements Runnable {\n  static int counter = init();\n  int field = 3;\n  Supplier<Integer> lam = () -> init();\n  static { init(); }\n  { init(); }\n  public Kinds() { this(1); }\n  Kinds(int x) { init(); }\n  static int init() { return 1; }\n  public void run() {\n    int local = init();\n    Runnable anon = new Runnable() {\n      public void run() { init(); }\n    };\n    class LocalC { int m() { return init(); } }\n    Supplier<Integer> s = () -> { return init() + local; };\n  }\n  interface Iface { int z(); }\n  enum E { A { int v() { return init(); } }, B; int v() { return 0; } }\n  record R(int q) { int w() { return init(); } }\n  static class Nested { int n() { return init(); } }\n}\n";
        let p = "scip-java maven maven/t/esc 1.0 p/Kinds#";
        let init = format!("{p}init().");
        // (span, symbol, roles, enclosing) as decoded from the real index.
        let measured: &[(Span, &str, i32, Option<Span>)] = &[
            ([4, 13, 4, 18], "", 1, Some([4, 0, 25, 1])),
            ([5, 13, 5, 20], "counter.", 1, Some([5, 2, 5, 30])),
            ([5, 23, 5, 27], "init().", 0, None),
            ([6, 6, 6, 11], "field.", 1, Some([6, 2, 6, 16])),
            ([7, 20, 7, 23], "lam.", 1, Some([7, 2, 7, 39])),
            ([7, 32, 7, 36], "init().", 0, None),
            ([8, 11, 8, 15], "init().", 0, None),
            ([9, 4, 9, 8], "init().", 0, None),
            ([10, 9, 10, 14], "`<init>`().", 1, Some([10, 2, 10, 29])),
            ([10, 19, 10, 23], "`<init>`(+1).", 0, None),
            ([11, 2, 11, 7], "`<init>`(+1).", 1, Some([11, 2, 11, 26])),
            ([11, 12, 11, 13], "local 0", 1, Some([11, 8, 11, 13])),
            ([11, 17, 11, 21], "init().", 0, None),
            ([12, 13, 12, 17], "init().", 1, Some([12, 2, 12, 33])),
            ([13, 14, 13, 17], "run().", 1, Some([13, 2, 20, 3])),
            ([14, 8, 14, 13], "local 1", 1, Some([14, 4, 14, 23])),
            ([14, 16, 14, 20], "init().", 0, None),
            ([15, 13, 15, 17], "local 2", 1, Some([15, 4, 17, 6])),
            ([16, 18, 16, 21], "local 4", 1, Some([16, 6, 16, 35])),
            ([16, 26, 16, 30], "init().", 0, None),
            ([18, 10, 18, 16], "run().LocalC#", 1, Some([18, 4, 18, 47])),
            (
                [18, 23, 18, 24],
                "run().LocalC#m().",
                1,
                Some([18, 19, 18, 45]),
            ),
            ([18, 36, 18, 40], "init().", 0, None),
            ([19, 22, 19, 23], "local 5", 1, Some([19, 4, 19, 59])),
            ([19, 41, 19, 45], "init().", 0, None),
            ([19, 50, 19, 55], "local 1", 0, None),
            ([21, 12, 21, 17], "Iface#", 1, Some([21, 2, 21, 30])),
            ([21, 24, 21, 25], "Iface#z().", 1, Some([21, 20, 21, 28])),
            ([22, 7, 22, 8], "E#", 1, Some([22, 2, 22, 71])),
            ([22, 11, 22, 12], "E#A.", 1, Some([22, 11, 22, 43])),
            ([22, 19, 22, 20], "local 8", 1, Some([22, 15, 22, 41])),
            ([22, 32, 22, 36], "init().", 0, None),
            ([22, 45, 22, 46], "E#B.", 1, Some([22, 45, 22, 46])),
            ([22, 52, 22, 53], "E#v().", 1, Some([22, 48, 22, 69])),
            ([23, 9, 23, 10], "R#", 1, Some([23, 2, 23, 48])),
            ([23, 24, 23, 25], "R#w().", 1, Some([23, 20, 23, 46])),
            ([23, 37, 23, 41], "init().", 0, None),
            ([24, 15, 24, 21], "Nested#", 1, Some([24, 2, 24, 52])),
            ([24, 28, 24, 29], "Nested#n().", 1, Some([24, 24, 24, 50])),
            ([24, 41, 24, 45], "init().", 0, None),
        ];
        let rows: Vec<&str> = src.lines().collect();
        let occurrences = measured
            .iter()
            .map(|&(span, d, roles, enclosing)| {
                let symbol = if d.starts_with("local ") {
                    d.to_owned()
                } else {
                    format!("{p}{d}")
                };
                // Every named span is on its token.
                let text = &rows[span[0] as usize][span[1] as usize..span[3] as usize];
                assert!(
                    d.starts_with("local ") || rb_scip::may_name(&symbol, text) || text == "this",
                    "{d} at {text}"
                );
                Occurrence {
                    span,
                    symbol,
                    roles,
                    enclosing,
                }
            })
            .collect();
        let local_name = |symbol: &str, name: &str| SymbolInfo {
            symbol: symbol.into(),
            display_name: name.into(),
            ..Default::default()
        };
        let index = ScipIndex {
            documents: vec![Document {
                path: "p/Kinds.java".into(),
                occurrences,
                symbols: vec![
                    local_name("local 0", "x"),
                    local_name("local 1", "local"),
                    local_name("local 2", "anon"),
                    local_name("local 4", "run"),
                    local_name("local 5", "s"),
                    local_name("local 8", "v"),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let joined = join_sources(Language::Java, &[("p/Kinds.java", src)], &index);
        assert_eq!(joined.coverage.documents_rejected, 0);
        let mut calls: Vec<String> = edge_lines(&joined)
            .into_iter()
            .filter(|l| l.ends_with(&format!("Calls {init}")))
            .map(|l| l.replace(&format!(" Calls {init}"), ""))
            .collect();
        calls.dedup();
        assert_eq!(
            calls,
            [
                "rb-heuristic . . . `p/Kinds.java`/Kinds#E#v().".to_owned(),
                p.to_owned(),
                format!("{p}Nested#n()."),
                format!("{p}R#w()."),
                format!("{p}`<init>`(+1)."),
                format!("{p}run()."),
                format!("{p}run().LocalC#m()."),
            ]
        );
    }

    /// Contract M3b amendment 68: the check is scip-java's only. scip-typescript's ranges for
    /// a class and a method begin at their decorators, which Tree-sitter-typescript credits to
    /// the file (an exported class's decorator) or to the class (a method's); measured on
    /// `var/fix-pass8/tsd/src/dec.ts`. TypeScript has no escape translation before lexing, so
    /// nothing like K1 arises there; checking it would reject every decorated class.
    #[test]
    fn typescript_decorators_are_no_structure_disagreement() {
        let src = "export function Dec(x: unknown): any { return () => undefined; }\nexport class Other { static y(): number { return 2; } }\n\n/** doc */\n@Dec(Other)\nexport class D {\n  @Dec(Other.y())\n  m(): number { return Other.y(); }\n  @Dec(Other) p = Other.y();\n}\n";
        let p = "scip-typescript npm tsd 1.0.0 src/`dec.ts`/";
        let sym = |d: &str| format!("{p}{d}");
        let index = ScipIndex {
            documents: vec![Document {
                path: "src/dec.ts".into(),
                occurrences: vec![
                    enclosed(occ_at(tok(src, "Dec", 0), &sym("Dec()."), 1), [0, 0, 0, 64]),
                    enclosed(
                        occ_at(tok(src, "Other", 0), &sym("Other#"), 1),
                        [1, 0, 1, 55],
                    ),
                    enclosed(
                        occ_at(tok(src, "y()", 0), &sym("Other#y()."), 1),
                        [1, 21, 1, 53],
                    ),
                    occ_at(tok(src, "Dec(", 1), &sym("Dec()."), 0),
                    occ_at(tok(src, "Other)", 0), &sym("Other#"), 0),
                    enclosed(occ_at(tok(src, "D {", 0), &sym("D#"), 1), [4, 0, 9, 1]),
                    occ_at(tok(src, "Dec(", 2), &sym("Dec()."), 0),
                    occ_at(tok(src, "y()", 1), &sym("Other#y()."), 0),
                    enclosed(occ_at(tok(src, "m()", 0), &sym("D#m()."), 1), [6, 2, 7, 35]),
                    occ_at(tok(src, "y()", 2), &sym("Other#y()."), 0),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(tok(src, "Dec(", 1), [4, 1, 4, 4], "the measured span");
        assert_eq!(tok(src, "y()", 1), [6, 13, 6, 14], "the measured span");
        let joined = join_sources(Language::TypeScript, &[("src/dec.ts", src)], &index);
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert!(
            edge_lines(&joined).contains(&format!("file src/dec.ts Calls {}", sym("Dec()."))),
            "{:?}",
            edge_lines(&joined)
        );
    }

    /// Contract M3b amendment 70 (the evaluator's K3, mutant N24): a definition occurrence at
    /// another definition's name accounts for nothing unless its symbol may name it. Pass 7's
    /// `Lat`/`Enc` layout put `p()`'s definition on `q`'s name; here `p()` and `q()` swap
    /// names outside any method, so only the name check makes them a disagreement.
    #[test]
    fn a_definition_at_another_definitions_name_is_a_disagreement() {
        let src = "package t;\nclass W {\n  int p() { return 1; } int q() { return 2; }\n}\n";
        let sym = |d: &str| format!("scip-java maven . . t/W#{d}");
        let document = |p_at: Span, q_at: Span| Document {
            path: "t/W.java".into(),
            occurrences: vec![
                occ_at(tok(src, "W", 0), &sym(""), 1),
                occ_at(p_at, &sym("p()."), 1),
                occ_at(q_at, &sym("q()."), 1),
            ],
            ..Default::default()
        };
        let reasons = |document: Document| {
            let index = ScipIndex {
                documents: vec![document],
                ..Default::default()
            };
            join_sources(Language::Java, &[("t/W.java", src)], &index)
                .coverage
                .documents_rejected_reasons
        };
        let (p, q) = (tok(src, "p()", 0), tok(src, "q()", 0));
        assert_eq!(
            reasons(document(q, p)),
            BTreeMap::from([(rb_scip::Rejection::DefinitionDisagreement, 1)])
        );
        assert_eq!(reasons(document(p, q)), BTreeMap::new());
    }

    /// Contract M3b amendment 70 (mutant N25): a type's definition occurrence is checked as a
    /// method's is. The index places `t/Ghost#` on the `helper` call inside `run` (where
    /// Tree-sitter has no type): rejected; at a place Tree-sitter has nothing (the class body's
    /// blank row, like a nested annotation interface) it is not.
    #[test]
    fn a_type_definition_where_tree_sitter_reads_code_is_a_disagreement() {
        let src = "package t;\nclass G {\n  void run() { helper(); }\n  void helper() {}\n\n}\n";
        let sym = |d: &str| format!("scip-java maven . . t/G#{d}");
        let reasons = |ghost: Span| {
            let index = ScipIndex {
                documents: vec![Document {
                    path: "t/G.java".into(),
                    occurrences: vec![
                        occ_at(tok(src, "G", 0), &sym(""), 1),
                        occ_at(tok(src, "run", 0), &sym("run()."), 1),
                        occ_at(ghost, "scip-java maven . . t/Ghost#", 1),
                        occ_at(tok(src, "helper", 1), &sym("helper()."), 1),
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            };
            join_sources(Language::Java, &[("t/G.java", src)], &index)
                .coverage
                .documents_rejected_reasons
        };
        assert_eq!(
            reasons(tok(src, "helper", 0)),
            BTreeMap::from([(rb_scip::Rejection::DefinitionDisagreement, 1)])
        );
        assert_eq!(reasons([4, 0, 4, 0]), BTreeMap::new());
    }

    /// Contract M3b amendment 70 (mutant N27): a named import's alias `{ x as y }` names a
    /// token only for a symbol that may name `x`. `y()` carrying `m.ts/x().` is the call of
    /// `x`; carrying `m.ts/zz().` (a project function) it is unmatched, not a call of `zz`.
    #[test]
    fn a_named_import_alias_names_only_symbols_of_its_name() {
        let m = "export function x() { return 1; }\nexport function zz() { return 2; }\n";
        let user = "import { x as y } from './m';\nexport function u() { return y(); }\n";
        let pkg = "scip-typescript npm t 1.0.0 src/";
        let (x, zz) = (format!("{pkg}`m.ts`/x()."), format!("{pkg}`m.ts`/zz()."));
        let u = format!("{pkg}`user.ts`/u().");
        let joined = |at_call: &str| {
            let index = ScipIndex {
                documents: vec![
                    Document {
                        path: "src/m.ts".into(),
                        occurrences: vec![
                            occ_at(tok(m, "x()", 0), &x, 1),
                            occ_at(tok(m, "zz()", 0), &zz, 1),
                        ],
                        ..Default::default()
                    },
                    Document {
                        path: "src/user.ts".into(),
                        occurrences: vec![
                            occ_at(tok(user, "x as", 0), &x, 0),
                            occ_at(tok(user, "y }", 0), &x, 0),
                            occ_at(tok(user, "u()", 0), &u, 1),
                            occ_at(tok(user, "y()", 0), at_call, 0),
                        ],
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };
            join_sources(
                Language::TypeScript,
                &[("src/m.ts", m), ("src/user.ts", user)],
                &index,
            )
        };
        assert_eq!(edge_lines(&joined(&x)), [format!("{u} Calls {x}")]);
        let wrong = joined(&zz);
        assert!(wrong.edges.is_empty(), "{:?}", edge_lines(&wrong));
        assert_eq!(wrong.coverage.references_unmatched, 1);
    }
}
