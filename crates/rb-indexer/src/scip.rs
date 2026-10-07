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
    /// Resolved declared supertypes of globally anchored type definitions that the type's own
    /// symbol information does not relate it to (contract M3b amendment 78; a scip-typescript
    /// interface, which it never relates, excepted): the syntax declares the clause, the index
    /// does not confirm it. The `IMPLEMENTS` walk steps only onto supertypes its origin's
    /// symbol information relates.
    pub supertypes_unconfirmed: usize,
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
    /// Occurrences of the accepted scip-java documents removed as Lombok-generated before the
    /// agreement checks (contract M6a D1): those at a `lombok/Generated#` marker's span and the
    /// definitions it encloses exactly.
    pub lombok_removed: usize,
    /// Accepted documents with at least one such removal (contract M6a D3).
    pub documents_lombok: usize,
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
    // Contract M6a D1, D2: in a scip-java index, each document's Lombok-generated occurrences
    // are removed first, and every check below judges what remains, unchanged. A document
    // without a marker is used as it is.
    let without_lombok: Vec<Option<(rb_scip::Document, usize)>> = index
        .documents
        .iter()
        .map(|doc| {
            languages
                .contains(&Language::Java)
                .then(|| remove_lombok_generated(doc))
                .flatten()
        })
        .collect();
    // Per path, the occurrences removed from its document.
    let mut removed: BTreeMap<String, usize> = BTreeMap::new();
    // The documents with facts, each with its occurrences by span, that pass the agreement
    // check (contract M3b amendment 64): a rejected document is left out entirely, as if the
    // index had none for its file except that it is counted as rejected, not as missing.
    let mut indexed: BTreeSet<String> = BTreeSet::new();
    let mut docs: Vec<Doc<'_>> = Vec::new();
    for (doc, lombok) in index.documents.iter().zip(&without_lombok) {
        let path = repo_path(&doc.path);
        let Some(&(_, facts)) = files.get(path.as_str()) else {
            continue;
        };
        let doc = match lombok {
            Some((stripped, n)) => {
                removed.insert(path.clone(), *n);
                stripped
            }
            None => doc,
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
    // the documents' order. Of the checks, only the syntax-error rule applies to the other
    // languages (amendment 75): scip-typescript's compiler reads a merge-conflict marker as
    // trivia up to the next marker, Tree-sitter keeps the text between them with `ERROR`
    // nodes, so a method only Tree-sitter sees could source an exact edge.
    let candidate_nodes: BTreeSet<&str> = anchors
        .iter()
        .flat_map(|a| a.replaced.values().map(String::as_str))
        .collect();
    let agrees: Vec<bool> = docs
        .iter()
        .zip(naming.iter().zip(&anchors))
        .map(|((path, doc, facts, by_span), (naming, anchors))| {
            if files[path.as_str()].0 != Language::Java {
                return facts.syntax_error.is_none();
            }
            !(definitions_unpaired(facts, doc, anchors)
                || structure_disagreement(
                    facts,
                    by_span,
                    &doc.occurrences,
                    naming,
                    anchors,
                    &candidate_nodes,
                ))
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
    // Counted for accepted documents only, like every other coverage figure (contract M6a D3).
    for (path, ..) in &docs {
        if let Some(&n) = removed.get(path) {
            out.coverage.lombok_removed += n;
            out.coverage.documents_lombok += 1;
        }
    }
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
    // a walk's exact endpoint, and what a declared supertype resolves to), each one's declared
    // supertypes resolved through the index's own occurrences (contract M3b amendment 26), and
    // the supertypes its own symbol information relates it to (amendment 78). A walk steps
    // only onto supertypes its origin's symbol information relates it to: a clause only
    // Tree-sitter reads (a type parameter's bound behind an escaped `*/`) is not one the
    // compiled type has, and would turn an override's reverse twin into an exact edge. Both
    // indexers relate a class to every supertype, transitively; scip-java relates an interface
    // likewise, scip-typescript 0.4.0 relates an interface to nothing (measured on
    // `var/fix-pass11`), so a walk through a TypeScript interface is confirmed by its origin,
    // and a walk from one (an interface method's link) by its declared clauses alone
    // (contract M6a D4).
    let mut types_by_symbol: BTreeMap<&str, Vec<(usize, usize)>> = BTreeMap::new();
    let mut supertypes: BTreeMap<(usize, usize), Vec<String>> = BTreeMap::new();
    let mut related: BTreeMap<(usize, usize), BTreeSet<&str>> = BTreeMap::new();
    for (di, (path, doc, facts, by_span)) in docs.iter().enumerate() {
        out.coverage.occurrences_unconverted += doc.unconverted;
        let replaced = out.keys.get(path);
        let java = files[path.as_str()].0 == Language::Java;
        for (i, d) in facts.definitions.iter().enumerate() {
            if !is_type(d) {
                continue;
            }
            let global = replaced.and_then(|r| r.get(&i)).map(String::as_str);
            if let Some(symbol) = global {
                types_by_symbol.entry(symbol).or_default().push((di, i));
            }
            let declared = declared_supertypes(d, by_span);
            out.coverage.supertypes_unresolved += declared.iter().filter(|s| s.is_none()).count();
            let declared: Vec<String> = declared.into_iter().flatten().collect();
            // The supertypes the type's own symbol information relates it to, found by its
            // global anchor. A type bound to a local symbol holds local sources, whose links
            // take no walk (amendment 54); an unanchored type (counted as a definition without
            // occurrence) has no symbol information: neither gives a walk a step.
            let relates: BTreeSet<&str> = global
                .map(|own| {
                    doc.symbols
                        .iter()
                        .filter(|s| s.symbol == own)
                        .flat_map(|s| &s.relationships)
                        .map(|r| r.symbol.as_str())
                        .collect()
                })
                .unwrap_or_default();
            // Counted: a resolved clause of an anchored type its own symbol information does
            // not confirm, except a scip-typescript interface's, which it never relates.
            if global.is_some() && (java || d.kind != rb_syntax::DefKind::Interface) {
                out.coverage.supertypes_unconfirmed += declared
                    .iter()
                    .filter(|s| !relates.contains(s.as_str()))
                    .count();
            }
            supertypes.insert((di, i), declared);
            related.insert((di, i), relates);
        }
    }
    let mut implements: Vec<ScipEdge> = Vec::new();
    for (di, (path, doc, facts, by_span)) in docs.iter().enumerate() {
        let heuristic = &keys[path.as_str()];
        // Whether definition `j` of this document is a scip-typescript interface (contract
        // M6a D4); a Java interface's walk keeps amendment 78's confirmation.
        let typescript_interface = |j: usize| {
            matches!(
                files[path.as_str()].0,
                Language::TypeScript | Language::Tsx | Language::JavaScript
            ) && facts.definitions[j].kind == rb_syntax::DefKind::Interface
        };
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
                            // A holder that is no type (a top-level function) relates to nothing.
                            related.get(&holder).is_some_and(|related| {
                                // Contract M6a D4: a scip-typescript interface relates its type
                                // to nothing, so its declared clauses alone decide the
                                // direction of a link its method's own symbol information
                                // carries (sub → super only, measured in P1b).
                                let confirmed_by =
                                    (!typescript_interface(holder.1)).then_some(related);
                                reaches_supertype(
                                    &supertypes,
                                    &types_by_symbol,
                                    holder,
                                    targets,
                                    confirmed_by,
                                )
                            })
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

/// The project definition an accepted document's reference occurrence at `span` names, when the
/// token there is that definition's name (contract M4a S8): its SCIP symbol, which is its graph
/// key. `None` for a file the join did not accept, a local or external symbol, a symbol whose
/// definition has no node, or more than one such symbol at the span.
pub(crate) fn definition_at(
    index: &ScipIndex,
    root: &str,
    joined: &Joined,
    path: &str,
    span: Span,
    token: &str,
) -> Option<String> {
    if !joined.documented.contains(path) {
        return None;
    }
    let doc_path = if root.is_empty() {
        path
    } else {
        path.strip_prefix(root)?.strip_prefix('/')?
    };
    let doc = index.documents.iter().find(|d| d.path == doc_path)?;
    let nodes: BTreeSet<&str> = joined
        .keys
        .values()
        .flat_map(|r| r.values().map(String::as_str))
        .collect();
    let found: BTreeSet<&str> = doc
        .occurrences
        .iter()
        .filter(|o| o.span == span && !o.is_definition())
        .map(|o| o.symbol.as_str())
        .filter(|s| nodes.contains(s) && rb_scip::last_descriptor_name(s).as_deref() == Some(token))
        .collect();
    match found.into_iter().collect::<Vec<_>>().as_slice() {
        [one] => Some((*one).to_owned()),
        _ => None,
    }
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
            // A type is anchored by a global type symbol only: scip-java puts the implicit
            // constructor alone at a type's name when the type's own occurrence is at an earlier
            // mention of the name (a class annotation naming a nested type of the class), and
            // the amendment-7 fallback below finds that one (amendment 75).
            let global_at_name = if is_type(d) {
                at_name(|o| is_global_definition(o) && o.symbol.ends_with('#'))
            } else {
                at_name(is_global_definition)
            }
            .filter(names_d);
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
///    definition's symbol or to any global method, constructor or type definition, anchored or
///    not (amendment 72: a member only javac sees, an annotation interface), is neither the
///    source's nor one of its Tree-sitter ancestors' (a reference Tree-sitter puts at file
///    level has no source, so any such range disagrees).
///
/// Ranges of what Tree-sitter deliberately does not define and the index does not key as a
/// method or a type (fields, enum constants, an anonymous class's methods, which are unbound
/// locals; lambdas have none) are passed over: what they hold is credited to the enclosing
/// definition on both sides (design §8.2). javac translates every
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
        .filter(|o| {
            o.is_definition()
                && (anchored.contains(o.symbol.as_str()) || is_method_or_type_definition(o))
        })
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

/// Whether the index and Tree-sitter disagree about which methods, constructors and types a
/// Java document defines, or Tree-sitter could not parse it (contract M3b amendment 72;
/// recorded as `structure_disagreement`). javac compiled the file, so
/// - a Tree-sitter syntax error is a disagreement (an escape such as `\u002f\u002a` that opens
///   a comment for javac only leaves Tree-sitter `ERROR` nodes);
/// - every Tree-sitter definition (Java's are all methods, constructors and types) must be
///   paired (amendment 75): a type anchored by a global type symbol (descriptor `#`), a method
///   or constructor by a global method or constructor symbol (`).`), or a method by a bound
///   local of kind `Method` whose innermost enclosing definition range (any other definition
///   occurrence, global or local) is an enum constant of the method's Tree-sitter parent: a
///   member of the parent's anchored type (`rb_scip::type_symbol_of`) whose kind is
///   unspecified (scip-java 0.13.1's enum constants; its fields are `Field`, its methods
///   `Method`) or `EnumMember`. That is the enum-constant-body shape amendment 54 binds;
///   a field, a local variable, or an anonymous class's method elsewhere at a Tree-sitter
///   method's name is a member javac compiled differently;
/// - every global method, constructor or type definition occurrence with an enclosing range
///   must anchor a Tree-sitter definition, except an annotation interface (`kind` `Interface`,
///   related to `java/lang/annotation/Annotation#`) and its elements, which Tree-sitter-java
///   deliberately does not capture. Implicit constructors have no range (and no text); locals
///   (anonymous classes' members), fields, enum constants and record components are not
///   methods or types; lambdas have no definition.
///
/// Measured on spring-petclinic, jsoup, spring-retry, networknt/json-schema-validator and
/// spring-petclinic-rest: the only unpaired forms are those annotation forms (1, 48 and 4
/// occurrences on jsoup, spring-retry and spring-petclinic-rest) and implicit constructors.
/// Every bound local on them is an enum constant body's method (jsoup's 117, amendment 75).
fn definitions_unpaired(facts: &FileFacts, doc: &rb_scip::Document, anchors: &Anchors<'_>) -> bool {
    let inside = |outer: Span, inner: Span| {
        (outer[0], outer[1]) <= (inner[0], inner[1]) && (inner[2], inner[3]) <= (outer[2], outer[3])
    };
    let kinds: BTreeMap<&str, &str> = doc
        .symbols
        .iter()
        .map(|s| (s.symbol.as_str(), s.kind.as_str()))
        .collect();
    // The symbol of the innermost definition range around `o` other than its own symbol's.
    let innermost = |o: &rb_scip::Occurrence| {
        doc.occurrences
            .iter()
            .filter(|e| e.is_definition() && e.symbol != o.symbol)
            .filter_map(|e| Some((e.enclosing?, e.symbol.as_str())))
            .filter(|(range, _)| inside(*range, o.span))
            .max_by_key(|(range, _)| {
                (
                    (range[0], range[1]),
                    std::cmp::Reverse((range[2], range[3])),
                )
            })
            .map(|(_, symbol)| symbol)
    };
    let paired = |i: usize| {
        let d = &facts.definitions[i];
        let Some(o) = anchors.occurrences.get(&i) else {
            return false;
        };
        if !o.symbol.starts_with("local ") {
            return o.symbol.ends_with(if is_type(d) { "#" } else { ")." });
        }
        let parent = d
            .parent
            .and_then(|p| anchors.occurrences.get(&p))
            .map(|p| p.symbol.as_str());
        !is_type(d)
            && kinds.get(o.symbol.as_str()) == Some(&"Method")
            && innermost(o).is_some_and(|constant| {
                matches!(kinds.get(constant), Some(&("" | "EnumMember")))
                    && parent.is_some_and(|parent| {
                        rb_scip::type_symbol_of(constant).as_deref() == Some(parent)
                    })
            })
    };
    let anchored: BTreeSet<&str> = anchors
        .occurrences
        .values()
        .map(|o| o.symbol.as_str())
        .collect();
    let annotations: BTreeSet<&str> = doc
        .symbols
        .iter()
        .filter(|s| {
            s.kind == "Interface"
                && s.relationships
                    .iter()
                    .any(|r| r.symbol.ends_with(" java/lang/annotation/Annotation#"))
        })
        .map(|s| s.symbol.as_str())
        .collect();
    // An annotation interface is its own type symbol, and its elements' too.
    let uncaptured = |symbol: &str| {
        rb_scip::type_symbol_of(symbol).is_some_and(|t| annotations.contains(t.as_str()))
    };
    facts.syntax_error.is_some()
        || !(0..facts.definitions.len()).all(paired)
        || doc.occurrences.iter().any(|o| {
            is_method_or_type_definition(o)
                && o.enclosing.is_some()
                && !anchored.contains(o.symbol.as_str())
                && !uncaptured(&o.symbol)
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
        if !is_method_or_type_definition(o) {
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

/// Contract M6a D1: `doc` without the occurrences Lombok marks as generated, with how many were
/// removed; `None` when it holds no marker (it is then judged exactly as before M6a). The
/// markers are the spans of `doc`'s reference occurrences of `@lombok.Generated`
/// ([`rb_scip::symbol::is_lombok_generated_marker`]); removed are the occurrences at a marker's
/// span and the definitions (global or local) whose enclosing range is a marker's span, exactly.
/// Lombok annotates every member it generates with that marker, and scip-java gives the
/// member's definition, its parameters and its body's references that span (probe P2b). A real
/// member's range runs from its first modifier to its end and a real reference names one
/// token, so neither equals a marker span (an annotation, a declarator with its terminator, or
/// `class … }`); a reference merely inside a marker span (an anonymous class in a field
/// initializer) is kept. One pass collects the markers into a sorted set and one filters.
fn remove_lombok_generated(doc: &rb_scip::Document) -> Option<(rb_scip::Document, usize)> {
    rb_frameworks::work::tick(doc.occurrences.len());
    let markers: BTreeSet<Span> = doc
        .occurrences
        .iter()
        .filter(|o| !o.is_definition() && rb_scip::symbol::is_lombok_generated_marker(&o.symbol))
        .map(|o| o.span)
        .collect();
    if markers.is_empty() {
        return None;
    }
    rb_frameworks::work::tick(doc.occurrences.len());
    let generated = |o: &rb_scip::Occurrence| {
        markers.contains(&o.span)
            || (o.is_definition() && o.enclosing.is_some_and(|e| markers.contains(&e)))
    };
    let occurrences: Vec<rb_scip::Occurrence> = doc
        .occurrences
        .iter()
        .filter(|o| !generated(o))
        .cloned()
        .collect();
    let n = doc.occurrences.len() - occurrences.len();
    Some((
        rb_scip::Document {
            occurrences,
            ..doc.clone()
        },
        n,
    ))
}

fn is_global_definition(o: &rb_scip::Occurrence) -> bool {
    o.is_definition() && !o.symbol.starts_with("local ")
}

/// A global definition occurrence of a method, a constructor or a type: its descriptor ends with
/// `).` or `#`.
fn is_method_or_type_definition(o: &rb_scip::Occurrence) -> bool {
    is_global_definition(o) && (o.symbol.ends_with(").") || o.symbol.ends_with('#'))
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
/// external or unresolved supertype ends that path, and only when `from`'s own symbol
/// information relates it to that symbol (`related`, amendment 78), so a clause the compiled
/// program does not have ends it too. `related` is `None` for an origin whose indexer relates
/// it to nothing (a scip-typescript interface, contract M6a D4): its declared clauses, each
/// resolved through the index's own type occurrence, are the only steps. The walk accepts only
/// when it reaches one of `targets` itself; the index's relationship is exact, so the walk only
/// decides its direction. A visited set and a depth cap bound it.
fn reaches_supertype(
    supertypes: &BTreeMap<(usize, usize), Vec<String>>,
    types_by_symbol: &BTreeMap<&str, Vec<(usize, usize)>>,
    from: (usize, usize),
    targets: &[(usize, usize)],
    related: Option<&BTreeSet<&str>>,
) -> bool {
    const MAX_DEPTH: usize = 64;
    let mut visited = BTreeSet::from([from]);
    let mut frontier = vec![from];
    for _ in 0..MAX_DEPTH {
        let mut next = Vec::new();
        for at in frontier {
            for symbol in supertypes.get(&at).map_or(&[][..], Vec::as_slice) {
                if related.is_some_and(|related| !related.contains(symbol.as_str())) {
                    continue;
                }
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

    /// Contract M4a S8: a handler name in an accepted document resolves through the index's
    /// reference occurrence at its span, and only to a matched definition named like the token.
    #[test]
    fn a_handler_resolves_through_the_occurrence_at_its_span() {
        let login = "scip-typescript npm p 1.0.0 src/`auth.ts`/login().";
        let other = "scip-typescript npm p 1.0.0 src/`auth.ts`/other().";
        let occ = |span: Span, symbol: &str, roles: i32| Occurrence {
            span,
            symbol: symbol.into(),
            roles,
            enclosing: None,
        };
        let index = ScipIndex {
            documents: vec![Document {
                path: "src/app.ts".into(),
                occurrences: vec![
                    occ([3, 26, 3, 31], login, 0),
                    occ([4, 20, 4, 25], other, 0),
                    occ([5, 20, 5, 25], "local 1", 0),
                    occ(
                        [6, 20, 6, 25],
                        "scip-typescript npm express 4.0.0 `index.d.ts`/json().",
                        0,
                    ),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut joined = Joined::default();
        joined.documented.insert("svc/src/app.ts".into());
        joined.keys.insert(
            "svc/src/auth.ts".into(),
            BTreeMap::from([(0, login.to_owned()), (1, other.to_owned())]),
        );
        let at = |span, token| definition_at(&index, "svc", &joined, "svc/src/app.ts", span, token);
        assert_eq!(at([3, 26, 3, 31], "login").as_deref(), Some(login));
        // The token must be the definition's name; locals and externals give nothing.
        assert_eq!(at([4, 20, 4, 25], "login"), None);
        assert_eq!(at([5, 20, 5, 25], "login"), None);
        assert_eq!(at([6, 20, 6, 25], "json"), None);
        assert_eq!(at([9, 0, 9, 5], "login"), None);
        // A file the join did not accept is never exact.
        assert_eq!(
            definition_at(
                &index,
                "svc",
                &joined,
                "svc/src/other.ts",
                [3, 26, 3, 31],
                "login"
            ),
            None
        );
    }

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
                supertypes_unconfirmed: 0,
                occurrences_unconverted: 0,
                relationships_dropped: 0,
                relationships_dropped_in_project: 0,
                relationships_anonymous: 0,
                documents_rejected: 0,
                documents_rejected_reasons: BTreeMap::new(),
                lombok_removed: 0,
                documents_lombok: 0,
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

    /// scip-java 0.13.1's symbol information for a method of an enum constant's body: a
    /// `local` symbol of kind `Method` with its display name; the constant itself has symbol
    /// information without a kind (measured on jsoup and `var/fix-pass10/x10esc`, amendment 75).
    fn local_method(symbol: &str, name: &str) -> SymbolInfo {
        SymbolInfo {
            symbol: symbol.into(),
            kind: "Method".into(),
            display_name: name.into(),
            ..Default::default()
        }
    }

    fn enum_constant(symbol: &str) -> SymbolInfo {
        SymbolInfo {
            symbol: symbol.into(),
            ..Default::default()
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
                    enclosed(
                        occ_at(at_nth(src, 2, "A", 0), &sym("A."), 1),
                        between(src, "A {", 0, "} }"),
                    ),
                    occ_at(at_nth(src, 2, "process", 0), "local 3", 1),
                    occ_at(at_nth(src, 2, "helper", 0), &sym("helper()."), 0),
                    occ_at(at_nth(src, 3, "process", 0), &sym("process()."), 1),
                    occ_at(at_nth(src, 4, "helper", 0), &sym("helper()."), 1),
                ],
                symbols: vec![
                    SymbolInfo {
                        relationships: implementation("local 3", &sym("process().")).relationships,
                        ..local_method("local 3", "process")
                    },
                    enum_constant(&sym("A.")),
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
                        enclosed(
                            occ_at(at_nth(a, 2, "X", 0), "scip-java maven . . a/A#X.", 1),
                            between(a, "X {", 0, "} }"),
                        ),
                        occ_at(at_nth(a, 2, "go", 0), "local 3", 1),
                        occ_at(at_nth(a, 3, "go", 0), "scip-java maven . . a/A#go().", 1),
                    ],
                    symbols: vec![
                        enum_constant("scip-java maven . . a/A#X."),
                        local_method("local 3", "go"),
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
    /// local the syntax does not define (an anonymous class's method), is not. TypeScript:
    /// since amendment 72 a Java document with a Tree-sitter definition no occurrence anchors
    /// is rejected whole, so only a TypeScript document can show such a gap.
    #[test]
    fn relationships_dropped_in_project_are_counted() {
        let src = "interface A { m(): void; }\nclass B implements A { m() {} }\n";
        let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
        let path = "src/a.ts";
        let sym = |d: &str| format!("scip-typescript npm t 1.0.0 src/`a.ts`/{d}");
        let index = ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences: vec![
                    occ_at(at_nth(src, 0, "A", 0), &sym("A#"), 1),
                    // `A#m()`'s definition occurrence inside the `interface` keyword: unmatched
                    // (outside any method and on no token, so no disagreement either).
                    occ_at(at_nth(src, 0, "e", 0), &sym("A#m()."), 1),
                    occ_at(at_nth(src, 1, "B", 0), &sym("B#"), 1),
                    occ_at(at_nth(src, 1, "A", 0), &sym("A#"), 0),
                    occ_at(at_nth(src, 1, "m", 0), &sym("B#m()."), 1),
                    // An anonymous class's method, which Tree-sitter treats as a local.
                    occ_at(at_nth(src, 0, "f", 0), "local 5", 1),
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
                                symbol: "scip-typescript npm typescript 5.6.3 lib/`lib.d.ts`/Runnable#run().".into(),
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
        let files = BTreeMap::from([(path, (Language::TypeScript, &facts))]);
        let keys: BTreeMap<&str, Vec<String>> =
            BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
        let joined = join(&index, "", &files, &keys, &[Language::TypeScript]);
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

        // The definitions at their names, and `zeta()`'s call occurrence on the `beta()` call:
        // the call is not taken for `zeta`.
        let mut occurrences: Vec<Occurrence> = ["m1", "m0", "m2", "m3", "zeta", "beta"]
            .iter()
            .map(|name| occ_at(def_at(name), &sym(&format!("{name}().")), 1))
            .collect();
        occurrences.push(occ_at(raw(2, 13, 20), &sym(""), 1));
        occurrences.push(occ_at(call_at("beta"), &sym("zeta()."), 0));
        let joined = joined_with(occurrences.clone());
        let replaced: Vec<&str> = joined.keys[path]
            .keys()
            .map(|i| facts.definitions[*i].name.as_str())
            .collect();
        assert_eq!(
            replaced,
            ["LoneCr2", "m1", "m0", "m2", "m3", "zeta", "beta"]
        );
        assert!(joined.edges.is_empty(), "{:?}", edge_lines(&joined));
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(
            joined.coverage.references_unmatched, 2,
            "the zeta() call has no occurrence, the beta() call's names zeta"
        );
        // A field's occurrence (not a method or a type) on `m2`'s name instead of `m2`'s: it
        // does not name `m2`, so `m2` has no definition occurrence, which in Java rejects the
        // document (amendment 72).
        occurrences.retain(|o| o.span != def_at("m2"));
        occurrences.push(occ_at(def_at("m2"), &sym("m0."), 1));
        let joined = joined_with(occurrences);
        assert!(joined.keys.is_empty(), "{:?}", joined.keys);
        assert_eq!(
            joined.coverage.documents_rejected_reasons,
            BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)])
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
    /// level on the implementation's side only. Amendment 78: scip-typescript 0.4.0 relates an
    /// interface to nothing (measured, `var/fix-pass11/t11ts`), so `Mid`'s clause is confirmed
    /// by the walk's origin (`Sub#` and `Svc#` relate to `Base#`) and not counted unconfirmed.
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
        let joined = hierarchy_joined(Language::TypeScript, &[("base.ts", src)], &rels, &[]);
        assert_eq!(joined.coverage.supertypes_unconfirmed, 0);
        assert_eq!(joined.coverage.supertypes_unresolved, 0);
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
                supertypes_unconfirmed: 0,
                occurrences_unconverted: 0,
                relationships_dropped: 0,
                relationships_dropped_in_project: 0,
                relationships_anonymous: 0,
                documents_rejected: 0,
                documents_rejected_reasons: BTreeMap::new(),
                lombok_removed: 0,
                documents_lombok: 0,
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
        // `C0`'s symbol information relates it to every supertype, as scip-java's does
        // (measured, amendment 78: the walk steps only onto related supertypes).
        let up: Vec<(String, String)> = (1..=65)
            .map(|i| ("p/C0#".to_owned(), format!("p/C{i}#")))
            .collect();
        let rels: Vec<(&str, &str)> = [("p/C64#", "p/C0#")]
            .into_iter()
            .chain(up.iter().map(|(a, b)| (a.as_str(), b.as_str())))
            .collect();
        let edges = hierarchy(&files, &rels);
        assert_eq!(edges.len(), 64, "C1 to C64: {edges:?}");
        assert!(edges.contains(&"p/C0# Implements p/C64#".to_owned()));
        assert!(!edges.contains(&"p/C0# Implements p/C65#".to_owned()));
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
                        enclosed(
                            occ_at(tok(en, "A", 0), &sym("A."), 1),
                            between(en, "A {", 0, "} }"),
                        ),
                        enclosed(occ_at(tok(en, "v()", 0), "local 1", 1), v),
                        occ_at(tok(en, "b()", 0), &sym("b()."), 0),
                        enclosed(
                            occ_at(tok(en, "b()", 1), &sym("b()."), 1),
                            between(en, "int b", 0, "}"),
                        ),
                    ],
                    symbols: vec![enum_constant(&sym("A.")), local_method("local 1", "v")],
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

    /// The evaluator's L1 and L3 sources (`var/fix-pass9/x9esc`, each compiled by javac). A
    /// `\u002a/` ends a block comment for javac only, so javac compiles a member that
    /// Tree-sitter-java reads as comment text, followed by its body, which Tree-sitter reads as
    /// an initializer of the class: `Hid1` a method, `Hid2` a method whose local Tree-sitter
    /// reads as a field, `Hid3` a nested class, `Hid5` a constructor, `Hid6` a static method
    /// (Tree-sitter: a static initializer), `Hid7` a method of a nested class; in `Hid9` the
    /// nested class `W9`'s `extends T9` becomes `S9`'s for Tree-sitter. `\u002f\u002a …
    /// \u002a\u002f` opens and closes a comment for javac only, so Tree-sitter has a method `g`
    /// javac never compiled (`Hid10`, with `ERROR` nodes).
    fn hidden_member_source(file: &str) -> &'static str {
        match file {
            "Hid1" => {
                "package p;\n\nclass Hid1 {\n  /* \\u002a/ void m() /* */ { b(); }\n  void b() {}\n}\n"
            }
            "Hid2" => {
                "package p;\n\nclass Hid2 {\n  /* \\u002a/ int m() { /* */ int x = b(); /* \\u002a/ return x; } /* */\n  int b() { return 1; }\n}\n"
            }
            "Hid3" => {
                "package p;\n\nclass Hid3 {\n  /* \\u002a/ static class In { void m() /* */ { b(); } /* \\u002a/ } /* */\n  static void b() {}\n}\n"
            }
            "Hid5" => {
                "package p;\n\nclass Hid5 {\n  /* \\u002a/ Hid5() /* */ { b(); }\n  void b() {}\n}\n"
            }
            "Hid6" => {
                "package p;\n\nclass Hid6 {\n  static /* \\u002a/ void m() /* */ { b(); }\n  static void b() {}\n}\n"
            }
            "Hid7" => {
                "package p;\n\nclass Hid7 {\n  static class N {\n    /* \\u002a/ void m() /* */ { Hid7.b(); }\n  }\n  static void b() {}\n}\n"
            }
            "Hid9" => {
                "package p;\n\nclass S9 /* \\u002a/ { static class W9 /* */ extends T9 {\n  void b() {}\n} /* \\u002a/ } /* */\nclass T9 { void b() {} }\n"
            }
            "Hid10" => {
                "package p;\n\nclass Hid10 {\n  \\u002f\\u002a void g() \\u002a\\u002f { b(); }\n  void b() {}\n}\n"
            }
            _ => unreachable!("{file}"),
        }
    }

    /// The occurrences `rb-scip-java:0.13.1` gave `file` (`var/fix-pass9/x9esc`; ASCII, so the
    /// decoded spans are the raw ones): `(span, descriptor, roles, enclosing range)`, the
    /// descriptor after `p/`, `local 0` for `Hid2`'s `x`. Package and JDK occurrences are left
    /// out.
    fn hidden_member_occurrences(file: &str) -> Vec<(Span, &'static str, i32, Option<Span>)> {
        match file {
            "Hid1" => vec![
                ([2, 6, 2, 10], "Hid1#", 1, Some([2, 0, 5, 1])),
                ([2, 6, 2, 10], "Hid1#`<init>`().", 1, None),
                ([3, 18, 3, 19], "Hid1#m().", 1, Some([3, 13, 3, 36])),
                ([3, 30, 3, 31], "Hid1#b().", 0, None),
                ([4, 7, 4, 8], "Hid1#b().", 1, Some([4, 2, 4, 13])),
            ],
            "Hid2" => vec![
                ([2, 6, 2, 10], "Hid2#", 1, Some([2, 0, 5, 1])),
                ([2, 6, 2, 10], "Hid2#`<init>`().", 1, None),
                ([3, 17, 3, 18], "Hid2#m().", 1, Some([3, 13, 3, 64])),
                ([3, 33, 3, 34], "local 0", 1, Some([3, 29, 3, 41])),
                ([3, 37, 3, 38], "Hid2#b().", 0, None),
                ([3, 60, 3, 61], "local 0", 0, None),
                ([4, 6, 4, 7], "Hid2#b().", 1, Some([4, 2, 4, 23])),
            ],
            "Hid3" => vec![
                ([2, 6, 2, 10], "Hid3#", 1, Some([2, 0, 5, 1])),
                ([2, 6, 2, 10], "Hid3#`<init>`().", 1, None),
                ([3, 26, 3, 28], "Hid3#In#", 1, Some([3, 13, 3, 67])),
                ([3, 26, 3, 28], "Hid3#In#`<init>`().", 1, None),
                ([3, 36, 3, 37], "Hid3#In#m().", 1, Some([3, 31, 3, 54])),
                ([3, 48, 3, 49], "Hid3#b().", 0, None),
                ([4, 14, 4, 15], "Hid3#b().", 1, Some([4, 2, 4, 20])),
            ],
            "Hid5" => vec![
                ([2, 6, 2, 10], "Hid5#", 1, Some([2, 0, 5, 1])),
                ([3, 13, 3, 17], "Hid5#`<init>`().", 1, Some([3, 13, 3, 34])),
                ([3, 28, 3, 29], "Hid5#b().", 0, None),
                ([4, 7, 4, 8], "Hid5#b().", 1, Some([4, 2, 4, 13])),
            ],
            "Hid6" => vec![
                ([2, 6, 2, 10], "Hid6#", 1, Some([2, 0, 5, 1])),
                ([2, 6, 2, 10], "Hid6#`<init>`().", 1, None),
                ([3, 25, 3, 26], "Hid6#m().", 1, Some([3, 2, 3, 43])),
                ([3, 37, 3, 38], "Hid6#b().", 0, None),
                ([4, 14, 4, 15], "Hid6#b().", 1, Some([4, 2, 4, 20])),
            ],
            "Hid7" => vec![
                ([2, 6, 2, 10], "Hid7#", 1, Some([2, 0, 7, 1])),
                ([2, 6, 2, 10], "Hid7#`<init>`().", 1, None),
                ([3, 15, 3, 16], "Hid7#N#", 1, Some([3, 2, 5, 3])),
                ([3, 15, 3, 16], "Hid7#N#`<init>`().", 1, None),
                ([4, 20, 4, 21], "Hid7#N#m().", 1, Some([4, 15, 4, 43])),
                ([4, 32, 4, 36], "Hid7#", 0, None),
                ([4, 37, 4, 38], "Hid7#b().", 0, None),
                ([6, 14, 6, 15], "Hid7#b().", 1, Some([6, 2, 6, 20])),
            ],
            "Hid9" => vec![
                ([2, 6, 2, 8], "S9#", 1, Some([2, 0, 4, 14])),
                ([2, 6, 2, 8], "S9#`<init>`().", 1, None),
                ([2, 35, 2, 37], "S9#W9#", 1, Some([2, 22, 4, 1])),
                ([2, 35, 2, 37], "S9#W9#`<init>`().", 1, None),
                ([2, 52, 2, 54], "T9#", 0, None),
                ([3, 7, 3, 8], "S9#W9#b().", 1, Some([3, 2, 3, 13])),
                ([5, 6, 5, 8], "T9#", 1, Some([5, 0, 5, 24])),
                ([5, 6, 5, 8], "T9#`<init>`().", 1, None),
                ([5, 16, 5, 17], "T9#b().", 1, Some([5, 11, 5, 22])),
            ],
            "Hid10" => vec![
                ([2, 6, 2, 11], "Hid10#", 1, Some([2, 0, 5, 1])),
                ([2, 6, 2, 11], "Hid10#`<init>`().", 1, None),
                ([3, 39, 3, 40], "Hid10#b().", 0, None),
                ([4, 7, 4, 8], "Hid10#b().", 1, Some([4, 2, 4, 13])),
            ],
            _ => unreachable!("{file}"),
        }
    }

    /// `file`'s measured document; every named span is on a token its symbol may name, and
    /// every one-row enclosing range starts at a word and ends with `}`.
    fn hidden_member_document(file: &str) -> Document {
        let src = hidden_member_source(file);
        let rows: Vec<&str> = src.lines().collect();
        let text = |s: Span| &rows[s[0] as usize][s[1] as usize..s[3] as usize];
        let mut occurrences: Vec<Occurrence> = hidden_member_occurrences(file)
            .into_iter()
            .map(|(span, d, roles, enclosing)| {
                let symbol = if d.starts_with("local ") {
                    d.to_owned()
                } else {
                    format!("scip-java maven maven/t9/x9esc 1.0 p/{d}")
                };
                assert!(
                    d.starts_with("local ") || rb_scip::may_name(&symbol, text(span)),
                    "{file}: {d} at {:?}",
                    text(span)
                );
                if let Some(range) = enclosing.filter(|r| r[0] == r[2]) {
                    let t = text(range);
                    assert!(
                        t.starts_with(char::is_alphabetic) && t.ends_with(['}', ';']),
                        "{file}: {t}"
                    );
                }
                Occurrence {
                    span,
                    symbol,
                    roles,
                    enclosing,
                }
            })
            .collect();
        occurrences.sort();
        Document {
            path: format!("p/{file}.java"),
            occurrences,
            symbols: vec![SymbolInfo {
                symbol: "local 0".into(),
                display_name: "x".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// Contract M3b amendment 72 (the evaluator's L1 and L3): a member only javac sees has a
    /// definition occurrence no Tree-sitter definition accounts for, and a method only
    /// Tree-sitter sees has none; either rejects the document whole (`structure_disagreement`),
    /// so the calls the class's initializers seem to hold (`Hid1# → b()`, …, `S9# → T9#`) and
    /// `Hid10#g() → b()` are never exact edges, while `Plain` in the same index keeps its own.
    /// The `5502422` binary emitted each of those edges as exact on the real image.
    #[test]
    fn members_only_one_parser_sees_reject_the_document() {
        let plain = "package p;\n\nclass Plain {\n  void m() { b(); }\n  void b() {}\n}\n";
        let sym = |d: &str| format!("scip-java maven maven/t9/x9esc 1.0 p/Plain#{d}");
        let plain_document = Document {
            path: "p/Plain.java".into(),
            occurrences: vec![
                enclosed(occ_at(tok(plain, "Plain", 0), &sym(""), 1), [2, 0, 5, 1]),
                occ_at(tok(plain, "Plain", 0), &sym("`<init>`()."), 1),
                enclosed(
                    occ_at(tok(plain, "m()", 0), &sym("m()."), 1),
                    between(plain, "void m", 0, "}"),
                ),
                occ_at(tok(plain, "b()", 0), &sym("b()."), 0),
                enclosed(
                    occ_at(tok(plain, "b()", 1), &sym("b()."), 1),
                    between(plain, "void b", 0, "}"),
                ),
            ],
            ..Default::default()
        };
        for file in [
            "Hid1", "Hid2", "Hid3", "Hid5", "Hid6", "Hid7", "Hid9", "Hid10",
        ] {
            let src = hidden_member_source(file);
            let path = format!("p/{file}.java");
            let index = ScipIndex {
                documents: vec![hidden_member_document(file), plain_document.clone()],
                ..Default::default()
            };
            let joined = join_sources(
                Language::Java,
                &[(path.as_str(), src), ("p/Plain.java", plain)],
                &index,
            );
            assert_eq!(
                joined.coverage.documents_rejected_reasons,
                BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)]),
                "{file}"
            );
            assert_eq!(
                edge_lines(&joined),
                [format!("{} Calls {}", sym("m()."), sym("b()."))],
                "{file}"
            );
            assert!(!joined.documented.contains(&path), "{file}");
        }
    }

    /// Contract M3b amendment 72: each side of the two-way agreement rejects on its own, and so
    /// does a Tree-sitter syntax error; an annotation interface and its elements, which
    /// Tree-sitter-java does not capture (measured on jsoup, spring-retry and
    /// spring-petclinic-rest, here `Ann` of `var/fix-pass9/x9forms`), are no disagreement,
    /// but a reference inside one is checked against its range (clause 2 considers every
    /// global method, constructor or type definition, anchored or not).
    #[test]
    fn each_side_of_the_definition_agreement_rejects_alone() {
        let rejected = |path: &str, src: &str, document: Document| {
            let index = ScipIndex {
                documents: vec![document],
                ..Default::default()
            };
            join_sources(Language::Java, &[(path, src)], &index)
                .coverage
                .documents_rejected_reasons
        };
        let structure = BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)]);
        // Only javac's `m`: `Hid1` without the call's occurrence, so no reference is checked.
        let mut hid1 = hidden_member_document("Hid1");
        hid1.occurrences
            .retain(|o| !(o.roles == 0 && o.symbol.ends_with("b().")));
        assert_eq!(
            rejected("p/Hid1.java", hidden_member_source("Hid1"), hid1),
            structure
        );
        // Only Tree-sitter's `m`: `Plain` whose index has no definition occurrence for it.
        let plain = "package p;\n\nclass Plain {\n  void m() { b(); }\n  void b() {}\n}\n";
        let sym = |d: &str| format!("scip-java maven . . p/Plain#{d}");
        let plain_document = |with_m: bool| Document {
            path: "p/Plain.java".into(),
            occurrences: [
                Some(enclosed(
                    occ_at(tok(plain, "Plain", 0), &sym(""), 1),
                    [2, 0, 5, 1],
                )),
                with_m.then(|| {
                    enclosed(
                        occ_at(tok(plain, "m()", 0), &sym("m()."), 1),
                        between(plain, "void m", 0, "}"),
                    )
                }),
                Some(occ_at(tok(plain, "b()", 0), &sym("b()."), 0)),
                Some(enclosed(
                    occ_at(tok(plain, "b()", 1), &sym("b()."), 1),
                    between(plain, "void b", 0, "}"),
                )),
            ]
            .into_iter()
            .flatten()
            .collect(),
            ..Default::default()
        };
        assert_eq!(
            rejected("p/Plain.java", plain, plain_document(true)),
            BTreeMap::new()
        );
        assert_eq!(
            rejected("p/Plain.java", plain, plain_document(false)),
            structure
        );
        // Only a syntax error: `\u003b` is an empty statement (`;`) for javac and an `ERROR`
        // node for Tree-sitter; every definition agrees.
        let semi = plain.replace("{ b(); }", "{ b(); \\u003b }");
        let facts = rb_syntax::extract(Language::Java, semi.as_bytes()).unwrap();
        assert!(facts.syntax_error.is_some());
        assert_eq!(facts.definitions.len(), 3);
        assert_eq!(
            rejected("p/Plain.java", &semi, plain_document(true)),
            structure
        );

        // The annotation interface, its elements and the nested types, as measured; `n` calls
        // `k` inside `Nested` on both sides.
        let ann = "package f;\n\npublic @interface Ann {\n  int value() default 1;\n  class Nested { void n() { k(); } static int k() { return 1; } }\n}\n";
        let sym = |d: &str| format!("scip-java maven maven/t9/x9forms 1.0 f/Ann#{d}");
        let annotation = |kind: &str, related: bool| SymbolInfo {
            symbol: sym(""),
            kind: kind.into(),
            relationships: related
                .then(|| Relationship {
                    symbol: "scip-java maven jdk 21 java/lang/annotation/Annotation#".into(),
                    is_implementation: true,
                    ..Default::default()
                })
                .into_iter()
                .collect(),
            ..Default::default()
        };
        let ann_document = |info: SymbolInfo, extra: Vec<Occurrence>| {
            let mut occurrences = vec![
                enclosed(occ_at(tok(ann, "Ann", 0), &sym(""), 1), [2, 0, 5, 1]),
                enclosed(
                    occ_at(tok(ann, "value", 0), &sym("value()."), 1),
                    between(ann, "int value", 0, ";"),
                ),
                enclosed(
                    occ_at(tok(ann, "Nested", 0), &sym("Nested#"), 1),
                    between(ann, "class Nested", 0, "} }"),
                ),
                enclosed(
                    occ_at(tok(ann, "n()", 0), &sym("Nested#n()."), 1),
                    between(ann, "void n", 0, "}"),
                ),
                occ_at(tok(ann, "k()", 0), &sym("Nested#k()."), 0),
                enclosed(
                    occ_at(tok(ann, "k()", 1), &sym("Nested#k()."), 1),
                    between(ann, "static int k", 0, "}"),
                ),
            ];
            occurrences.extend(extra);
            occurrences.sort();
            Document {
                path: "f/Ann.java".into(),
                occurrences,
                symbols: vec![info],
                ..Default::default()
            }
        };
        let accepted = BTreeMap::new();
        assert_eq!(
            rejected(
                "f/Ann.java",
                ann,
                ann_document(annotation("Interface", true), vec![])
            ),
            accepted
        );
        // Not an annotation interface: no relation to `Annotation`, or not an interface.
        for info in [annotation("Interface", false), annotation("Class", true)] {
            assert_eq!(
                rejected("f/Ann.java", ann, ann_document(info, vec![])),
                structure
            );
        }
        // A call in a constant of the annotation interface: Tree-sitter puts it at file level
        // (no source), the index inside `Ann`'s range, so its source disagrees.
        let with_constant = ann.replace("default 1;", "default 1; int C = Nested.k();");
        let constant_call = tok(&with_constant, "k()", 0);
        assert_eq!(
            constant_call[0], 3,
            "the rows of the other occurrences do not move"
        );
        let document = ann_document(
            annotation("Interface", true),
            vec![occ_at(constant_call, &sym("Nested#k()."), 0)],
        );
        assert_eq!(rejected("f/Ann.java", &with_constant, document), structure);
    }

    /// Contract M3b amendment 74 (the evaluator's L5, mutants S9 and S10): a bound local is
    /// checked like a global definition, as a range and as a target. An enum constant body's
    /// methods are `local` symbols bound to Tree-sitter's definitions (amendment 54). Where the
    /// range of `v` (stretched to row 3) holds the field initializer's `b()`, which Tree-sitter
    /// credits to `E`, the innermost range is the local's and the document is rejected; where
    /// the call of the bound local `w` lies outside the range of `v`, its Tree-sitter source,
    /// it is rejected too. With the measured ranges neither is. `w` is declared before `v`, so
    /// the stretched range of `v` never holds `w`'s definition, which then stays paired
    /// (amendment 75: its innermost enclosing range is the constant `A`).
    #[test]
    fn bound_locals_are_checked_as_ranges_and_as_targets() {
        let en = "package t;\nenum E {\n  A { int w() { return 1; } int v() { return w(); } };\n  int f = b();\n  int b() { return 1; }\n}\n";
        let sym = |d: &str| format!("scip-java maven . . t/E#{d}");
        let rejected = |v: Span| {
            let mut occurrences = vec![
                enclosed(occ_at(tok(en, "E", 0), &sym(""), 1), [1, 0, 5, 1]),
                enclosed(
                    occ_at(tok(en, "A", 0), &sym("A."), 1),
                    between(en, "A {", 0, "} }"),
                ),
                enclosed(occ_at(tok(en, "v()", 0), "local 1", 1), v),
                enclosed(
                    occ_at(tok(en, "w()", 0), "local 2", 1),
                    between(en, "int w", 0, "}"),
                ),
                occ_at(tok(en, "w()", 1), "local 2", 0),
                occ_at(tok(en, "b()", 0), &sym("b()."), 0),
                enclosed(
                    occ_at(tok(en, "b()", 1), &sym("b()."), 1),
                    between(en, "int b", 0, "}"),
                ),
            ];
            occurrences.sort();
            let index = ScipIndex {
                documents: vec![Document {
                    path: "t/E.java".into(),
                    occurrences,
                    symbols: vec![
                        enum_constant(&sym("A.")),
                        local_method("local 1", "v"),
                        local_method("local 2", "w"),
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            };
            join_sources(Language::Java, &[("t/E.java", en)], &index)
                .coverage
                .documents_rejected_reasons
        };
        let v = between(en, "int v", 0, "}");
        let structure = BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)]);
        assert_eq!(rejected(v), BTreeMap::new());
        // `v`'s range over `int f = b();` (rows 2 to 3): `b()` is inside it.
        let f_end = between(en, "int f", 0, ";");
        assert_eq!(rejected([v[0], v[1], f_end[2], f_end[3]]), structure);
        // `v`'s range ending before `w()`.
        let w_call = tok(en, "w()", 1);
        assert_eq!(rejected([v[0], v[1], v[2], w_call[1] - 1]), structure);
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
                    local_method("local 8", "v"),
                    enum_constant(&format!("{p}E#A.")),
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

    /// Contract M3b amendment 75 (the evaluator's M1): `\u002a/` ends a block comment for
    /// javac only, so javac compiles a member named `n` that is not a method of the class while
    /// Tree-sitter-java reads a method `n` of the class: an anonymous class's method inside `m`
    /// (`Anon1`; `Anon2`'s implements `I2#n()`), a local variable holding a lambda (`Loc1`), a
    /// field holding one (`Fld1`), an anonymous class's method in a field initializer (`Anon3`),
    /// a local variable of an initializer block (`Init1`), and an anonymous class's method
    /// inside a method of an enum constant's body (`Enm2`). `Enm1` is the shape amendment 54
    /// binds: `n` is a method of the enum constant `A`'s body for javac, a method of the enum for
    /// Tree-sitter, and both credit its body to `n`.
    fn restructured_member_source(file: &str) -> &'static str {
        match file {
            "Anon1" => {
                "package p;\n\nclass Anon1 {\n  void m() { Object x = new Object() /* \\u002a/ { { /* */ ; } void n() { b(); } /* \\u002a/ }; } /* */\n  void b() {}\n}\n"
            }
            "Anon2" => {
                "package p;\n\nclass Anon2 {\n  void m() { I2 x = new I2() /* \\u002a/ { { /* */ ; } public void n() { b(); } /* \\u002a/ }; } /* */\n  void b() {}\n}\n\ninterface I2 { void n(); }\n"
            }
            "Loc1" => {
                "package p;\n\nclass Loc1 {\n  void m() { /* \\u002a/ { /* */ } Runnable n /* \\u002a/ = /* */ () /* \\u002a/ -> /* */ { b(); } /* \\u002a/ ; } /* */\n  void b() {}\n}\n"
            }
            "Fld1" => {
                "package p;\n\nclass Fld1 {\n  Runnable n /* \\u002a/ = /* */ () /* \\u002a/ -> /* */ { b(); } /* \\u002a/ ; /* */\n  void b() {}\n}\n"
            }
            "Anon3" => {
                "package p;\n\nclass Anon3 {\n  Object f = new Object() /* \\u002a/ { /* */ ; void n() { b(); } /* \\u002a/ } ; /* */\n  void b() {}\n}\n"
            }
            "Init1" => {
                "package p;\n\nclass Init1 {\n  /* \\u002a/ { /* */ Runnable n /* \\u002a/ = /* */ () /* \\u002a/ -> /* */ { b(); } /* \\u002a/ ; } /* */\n  void b() {}\n}\n"
            }
            "Enm1" => {
                "package p;\n\nenum Enm1 {\n  A /* \\u002a/ { /* */ ; void n() { b(); } /* \\u002a/ } ; /* */\n  void b() {}\n}\n"
            }
            "Enm2" => {
                "package p;\n\nenum Enm2 {\n  A /* \\u002a/ { void x() { new Object() { /* */ ; void n() { b(); } /* \\u002a/ }; } } /* */ ;\n  void b() {}\n}\n"
            }
            _ => unreachable!("{file}"),
        }
    }

    /// The occurrences and symbol information `rb-scip-java:0.13.1` gave `file`
    /// (`var/fix-pass10/x10esc`, decoded; ASCII, so the spans are the raw ones): occurrences as
    /// `(span, descriptor, roles, enclosing range)` with the descriptor after `p/`, and symbols
    /// as `(symbol, kind, display name, implemented symbol)`. JDK occurrences are left out.
    #[allow(clippy::type_complexity)]
    fn restructured_member_measured(
        file: &str,
    ) -> (
        Vec<(Span, &'static str, i32, Option<Span>)>,
        Vec<(
            &'static str,
            &'static str,
            &'static str,
            Option<&'static str>,
        )>,
    ) {
        let class = |name: &str, close: i32| {
            vec![
                (
                    [2, 6, 2, 6 + name.len() as i32],
                    "",
                    1,
                    Some([2, 0, close, 1]),
                ),
                ([2, 6, 2, 6 + name.len() as i32], "`<init>`().", 1, None),
            ]
        };
        let (mut occurrences, symbols) = match file {
            "Anon1" => (
                vec![
                    ([3, 7, 3, 8], "m().", 1, Some([3, 2, 3, 95])),
                    ([3, 20, 3, 21], "local 0", 1, Some([3, 13, 3, 93])),
                    ([3, 67, 3, 68], "local 2", 1, Some([3, 62, 3, 79])),
                    ([3, 73, 3, 74], "b().", 0, None),
                    ([4, 7, 4, 8], "b().", 1, Some([4, 2, 4, 13])),
                ],
                vec![
                    ("local 0", "Variable", "x", None),
                    ("local 2", "Method", "n", None),
                ],
            ),
            "Anon2" => (
                vec![
                    ([3, 7, 3, 8], "m().", 1, Some([3, 2, 3, 94])),
                    ([3, 13, 3, 15], "I2#", 0, None),
                    ([3, 16, 3, 17], "local 0", 1, Some([3, 13, 3, 92])),
                    ([3, 24, 3, 26], "I2#", 0, None),
                    ([3, 66, 3, 67], "local 2", 1, Some([3, 54, 3, 78])),
                    ([3, 72, 3, 73], "b().", 0, None),
                    ([4, 7, 4, 8], "b().", 1, Some([4, 2, 4, 13])),
                    ([7, 10, 7, 12], "I2#", 1, Some([7, 0, 7, 26])),
                    ([7, 20, 7, 21], "I2#n().", 1, Some([7, 15, 7, 24])),
                ],
                vec![
                    ("local 0", "Variable", "x", None),
                    ("local 2", "Method", "n", Some("I2#n().")),
                ],
            ),
            "Loc1" => (
                vec![
                    ([3, 7, 3, 8], "m().", 1, Some([3, 2, 3, 110])),
                    ([3, 43, 3, 44], "local 0", 1, Some([3, 34, 3, 108])),
                    ([3, 89, 3, 90], "b().", 0, None),
                    ([4, 7, 4, 8], "b().", 1, Some([4, 2, 4, 13])),
                ],
                vec![("local 0", "Variable", "n", None)],
            ),
            "Fld1" => (
                vec![
                    ([3, 11, 3, 12], "n.", 1, Some([3, 2, 3, 76])),
                    ([3, 57, 3, 58], "b().", 0, None),
                    ([4, 7, 4, 8], "b().", 1, Some([4, 2, 4, 13])),
                ],
                vec![("n.", "Field", "n", None)],
            ),
            "Anon3" => (
                vec![
                    ([3, 9, 3, 10], "f.", 1, Some([3, 2, 3, 79])),
                    ([3, 52, 3, 53], "local 1", 1, Some([3, 47, 3, 64])),
                    ([3, 58, 3, 59], "b().", 0, None),
                    ([4, 7, 4, 8], "b().", 1, Some([4, 2, 4, 13])),
                ],
                vec![("f.", "Field", "f", None), ("local 1", "Method", "n", None)],
            ),
            "Init1" => (
                vec![
                    ([3, 30, 3, 31], "local 0", 1, Some([3, 21, 3, 95])),
                    ([3, 76, 3, 77], "b().", 0, None),
                    ([4, 7, 4, 8], "b().", 1, Some([4, 2, 4, 13])),
                ],
                vec![("local 0", "Variable", "n", None)],
            ),
            "Enm1" => (
                vec![
                    ([3, 2, 3, 3], "local 1", 0, None),
                    ([3, 2, 3, 3], "A.", 1, Some([3, 2, 3, 55])),
                    ([3, 30, 3, 31], "local 2", 1, Some([3, 25, 3, 42])),
                    ([3, 36, 3, 37], "b().", 0, None),
                    ([4, 7, 4, 8], "b().", 1, Some([4, 2, 4, 13])),
                ],
                vec![("A.", "", "A", None), ("local 2", "Method", "n", None)],
            ),
            "Enm2" => (
                vec![
                    ([3, 2, 3, 3], "local 1", 0, None),
                    ([3, 2, 3, 3], "A.", 1, Some([3, 2, 3, 86])),
                    ([3, 22, 3, 23], "local 2", 1, Some([3, 17, 3, 84])),
                    ([3, 56, 3, 57], "local 4", 1, Some([3, 51, 3, 68])),
                    ([3, 62, 3, 63], "b().", 0, None),
                    ([4, 7, 4, 8], "b().", 1, Some([4, 2, 4, 13])),
                ],
                vec![
                    ("A.", "", "A", None),
                    ("local 2", "Method", "x", None),
                    ("local 4", "Method", "n", None),
                ],
            ),
            _ => unreachable!("{file}"),
        };
        occurrences.extend(if file.starts_with("Enm") {
            vec![
                ([2, 5, 2, 9], "", 1, Some([2, 0, 5, 1])),
                ([2, 5, 2, 9], "`<init>`().", 1, None),
            ]
        } else {
            class(file, 5)
        });
        (occurrences, symbols)
    }

    /// `file`'s measured document; every global span is on a token its symbol may name, and
    /// every local's on its display name.
    fn restructured_member_document(file: &str) -> Document {
        let src = restructured_member_source(file);
        let rows: Vec<&str> = src.lines().collect();
        let text = |s: Span| &rows[s[0] as usize][s[1] as usize..s[3] as usize];
        let (measured, symbols) = restructured_member_measured(file);
        let global = |d: &str| {
            if d.starts_with("local ") {
                d.to_owned()
            } else if d.starts_with("I2#") {
                format!("scip-java maven maven/t10/x10esc 1.0 p/{d}")
            } else {
                format!("scip-java maven maven/t10/x10esc 1.0 p/{file}#{d}")
            }
        };
        let symbols: Vec<SymbolInfo> = symbols
            .into_iter()
            .map(|(symbol, kind, name, implemented)| SymbolInfo {
                symbol: global(symbol),
                kind: kind.into(),
                display_name: name.into(),
                relationships: implemented
                    .map(|to| Relationship {
                        symbol: global(to),
                        is_implementation: true,
                        ..Default::default()
                    })
                    .into_iter()
                    .collect(),
            })
            .collect();
        let mut occurrences: Vec<Occurrence> = measured
            .into_iter()
            .map(|(span, d, roles, enclosing)| {
                let symbol = global(d);
                let named = match symbols.iter().find(|s| s.symbol == symbol) {
                    Some(s) if d.starts_with("local ") && roles == 1 => {
                        s.display_name == text(span)
                    }
                    _ => d.starts_with("local ") || rb_scip::may_name(&symbol, text(span)),
                };
                assert!(named, "{file}: {d} at {:?}", text(span));
                Occurrence {
                    span,
                    symbol,
                    roles,
                    enclosing,
                }
            })
            .collect();
        occurrences.sort();
        Document {
            path: format!("p/{file}.java"),
            occurrences,
            symbols,
            ..Default::default()
        }
    }

    /// Contract M3b amendment 75 (the evaluator's M1): a Tree-sitter method is paired only by
    /// a global method or constructor occurrence, or by a local method of an enum constant's
    /// body bound to a method of that enum; a local variable, an anonymous class's method or a
    /// field at its name leaves it unpaired and rejects the document whole
    /// (`structure_disagreement`), so `n → b` (and `Anon2`'s `n IMPLEMENTS I2#n()`) is never an
    /// exact edge, while `Plain` in the same index keeps its own. The `bf43351` binary emitted
    /// each of those edges as exact on the real image (`var/fix-pass10/x10esc`). `Enm1`, the
    /// enum-constant-body shape, is accepted: its edge is the one a constant body's method
    /// gives (amendment 54).
    #[test]
    fn a_method_only_tree_sitter_sees_is_not_paired_by_a_local_or_a_field() {
        let plain = "package p;\n\nclass Plain {\n  void m() { b(); }\n  void b() {}\n}\n";
        let p = "scip-java maven maven/t10/x10esc 1.0 p/";
        let sym = |d: &str| format!("{p}Plain#{d}");
        let plain_document = Document {
            path: "p/Plain.java".into(),
            occurrences: vec![
                enclosed(occ_at(tok(plain, "Plain", 0), &sym(""), 1), [2, 0, 5, 1]),
                occ_at(tok(plain, "Plain", 0), &sym("`<init>`()."), 1),
                enclosed(
                    occ_at(tok(plain, "m()", 0), &sym("m()."), 1),
                    between(plain, "void m", 0, "}"),
                ),
                occ_at(tok(plain, "b()", 0), &sym("b()."), 0),
                enclosed(
                    occ_at(tok(plain, "b()", 1), &sym("b()."), 1),
                    between(plain, "void b", 0, "}"),
                ),
            ],
            ..Default::default()
        };
        for file in ["Anon1", "Anon2", "Loc1", "Fld1", "Anon3", "Init1", "Enm2"] {
            let src = restructured_member_source(file);
            let facts = rb_syntax::extract(Language::Java, src.as_bytes()).unwrap();
            assert!(facts.syntax_error.is_none(), "{file}");
            assert!(
                facts.definitions.iter().any(|d| d.name == "n"),
                "{file}: Tree-sitter reads a method `n`"
            );
            let path = format!("p/{file}.java");
            let index = ScipIndex {
                documents: vec![restructured_member_document(file), plain_document.clone()],
                ..Default::default()
            };
            let joined = join_sources(
                Language::Java,
                &[(path.as_str(), src), ("p/Plain.java", plain)],
                &index,
            );
            assert_eq!(
                joined.coverage.documents_rejected_reasons,
                BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)]),
                "{file}"
            );
            assert_eq!(
                edge_lines(&joined),
                [format!("{} Calls {}", sym("m()."), sym("b()."))],
                "{file}"
            );
            assert!(!joined.documented.contains(&path), "{file}");
        }
        let enm1 = restructured_member_source("Enm1");
        let index = ScipIndex {
            documents: vec![restructured_member_document("Enm1")],
            ..Default::default()
        };
        let joined = join_sources(Language::Java, &[("p/Enm1.java", enm1)], &index);
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(joined.coverage.definitions_local, 1);
        assert_eq!(
            edge_lines(&joined),
            [format!(
                "rb-heuristic . . . `p/Enm1.java`/Enm1#n(). Calls {p}Enm1#b()."
            )]
        );
    }

    /// Contract M3b amendment 75 (the evaluator's M3): scip-java places a type's definition
    /// occurrence at the first mention of its name, which a class annotation naming one of
    /// the class's own nested types holds (`@Ext(Fut.Inner.class)`; vavr's `FutureTest` and
    /// `JavaConvertersTest`); at the class's name there is only the implicit constructor,
    /// without a range. The class anchors through its type symbol by the amendment-7 fallback,
    /// never through the constructor, so the document is accepted with the class keyed
    /// `Fut#`. Measured on `var/fix-pass10/x10esc` (`rb-scip-java:0.13.1`).
    #[test]
    fn a_type_anchors_through_its_type_symbol_not_its_implicit_constructor() {
        let src = "package p;\n\n@Deprecated\n@Ext(Fut.Inner.class)\npublic class Fut {\n  static class Inner {}\n  void m() { b(); }\n  void b() {}\n}\n";
        let p = "scip-java maven maven/t10/x10esc 1.0 p/";
        let measured: &[(Span, &str, i32, Option<Span>)] = &[
            ([3, 1, 3, 4], "Ext#", 0, None),
            ([3, 5, 3, 8], "Fut#", 0, None),
            ([3, 5, 3, 8], "Fut#", 1, Some([2, 0, 8, 1])),
            ([3, 9, 3, 14], "Fut#Inner#", 0, None),
            ([3, 15, 3, 20], "Fut#Inner#class.", 0, None),
            ([4, 13, 4, 16], "Fut#`<init>`().", 1, None),
            ([5, 15, 5, 20], "Fut#Inner#", 1, Some([5, 2, 5, 23])),
            ([5, 15, 5, 20], "Fut#Inner#`<init>`().", 1, None),
            ([6, 7, 6, 8], "Fut#m().", 1, Some([6, 2, 6, 19])),
            ([6, 13, 6, 14], "Fut#b().", 0, None),
            ([7, 7, 7, 8], "Fut#b().", 1, Some([7, 2, 7, 13])),
        ];
        let rows: Vec<&str> = src.lines().collect();
        let occurrences = measured
            .iter()
            .map(|&(span, d, roles, enclosing)| {
                let symbol = format!("{p}{d}");
                let text = &rows[span[0] as usize][span[1] as usize..span[3] as usize];
                assert!(rb_scip::may_name(&symbol, text), "{d} at {text}");
                Occurrence {
                    span,
                    symbol,
                    roles,
                    enclosing,
                }
            })
            .collect();
        let index = ScipIndex {
            documents: vec![Document {
                path: "p/Fut.java".into(),
                occurrences,
                ..Default::default()
            }],
            ..Default::default()
        };
        let facts = rb_syntax::extract(Language::Java, src.as_bytes()).unwrap();
        let class = facts
            .definitions
            .iter()
            .position(|d| d.name == "Fut")
            .unwrap();
        let joined = join_sources(Language::Java, &[("p/Fut.java", src)], &index);
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(joined.keys["p/Fut.java"][&class], format!("{p}Fut#"));
        assert_eq!(joined.coverage.definitions_matched_by_name, 1);
        assert_eq!(
            joined.coverage.definitions_matched,
            joined.coverage.definitions_total
        );
        assert_eq!(
            edge_lines(&joined),
            [format!("{p}Fut#m(). Calls {p}Fut#b().")]
        );
    }

    /// A record's compact canonical constructor is a Tree-sitter definition (contract M3b
    /// amendment 75; open item of amendment 72), paired with the constructor occurrence
    /// scip-java gives at its name: `Rec2(String)` is `` `<init>`(). `` and the compact one,
    /// declared second, `` `<init>`(+1). `` (source order; measured on
    /// `var/fix-pass10/x10esc`). The document is accepted and the compact constructor is the
    /// source of the call in its body, not the record.
    #[test]
    fn a_compact_constructor_pairs_with_its_constructor_occurrence() {
        let src = "package p;\n\nrecord Rec2(int x) {\n  Rec2(String s) { this(s.length()); }\n  Rec2 {\n    check(x);\n  }\n  static void check(int v) {}\n  static Rec2 make() { return new Rec2(1); }\n}\n";
        let p = "scip-java maven maven/t10/x10esc 1.0 p/Rec2#";
        let measured: &[(Span, &str, i32, Option<Span>)] = &[
            ([2, 7, 2, 11], "", 1, Some([2, 0, 9, 1])),
            ([2, 16, 2, 17], "local 1", 1, None),
            ([2, 16, 2, 17], "x.", 1, Some([2, 12, 2, 17])),
            ([3, 2, 3, 6], "`<init>`().", 1, Some([3, 2, 3, 38])),
            ([3, 14, 3, 15], "local 0", 1, Some([3, 7, 3, 15])),
            ([3, 19, 3, 23], "`<init>`(+1).", 0, None),
            ([3, 24, 3, 25], "local 0", 0, None),
            ([4, 2, 4, 6], "`<init>`(+1).", 1, Some([4, 2, 6, 3])),
            ([5, 4, 5, 9], "check().", 0, None),
            ([5, 10, 5, 11], "local 1", 0, None),
            ([7, 14, 7, 19], "check().", 1, Some([7, 2, 7, 29])),
            ([7, 24, 7, 25], "local 2", 1, Some([7, 20, 7, 25])),
            ([8, 9, 8, 13], "", 0, None),
            ([8, 14, 8, 18], "make().", 1, Some([8, 2, 8, 44])),
            ([8, 34, 8, 38], "`<init>`(+1).", 0, None),
        ];
        let occurrences = measured
            .iter()
            .map(|&(span, d, roles, enclosing)| Occurrence {
                span,
                symbol: if d.starts_with("local ") {
                    d.to_owned()
                } else {
                    format!("{p}{d}")
                },
                roles,
                enclosing,
            })
            .collect();
        let local = |symbol: &str, name: &str| SymbolInfo {
            symbol: symbol.into(),
            display_name: name.into(),
            ..Default::default()
        };
        let index = ScipIndex {
            documents: vec![Document {
                path: "p/Rec2.java".into(),
                occurrences,
                symbols: vec![
                    local("local 0", "s"),
                    local("local 1", "x"),
                    local("local 2", "v"),
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let facts = rb_syntax::extract(Language::Java, src.as_bytes()).unwrap();
        let compact = facts
            .definitions
            .iter()
            .position(|d| d.name == "Rec2" && d.name_span == [4, 2, 4, 6])
            .expect("the compact constructor is a definition");
        let joined = join_sources(Language::Java, &[("p/Rec2.java", src)], &index);
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(
            joined.keys["p/Rec2.java"][&compact],
            format!("{p}`<init>`(+1).")
        );
        let edges = edge_lines(&joined);
        assert!(
            edges.contains(&format!("{p}`<init>`(+1). Calls {p}check().")),
            "{edges:?}"
        );
        assert!(
            !edges.iter().any(|e| e.starts_with(&format!("{p} "))),
            "the record is no source: {edges:?}"
        );
    }

    /// Contract M3b amendment 75 (the evaluator's M2): the TypeScript scanner treats a
    /// merge-conflict marker's `=======` line as trivia up to the next `>>>>>>> ` line, while
    /// Tree-sitter-typescript keeps what lies between them (with `ERROR` nodes): scip-typescript
    /// sees `m1` holding `helper()`, Tree-sitter a method `m9` the compiler never saw. A
    /// TypeScript document with a Tree-sitter syntax error is rejected
    /// (`structure_disagreement`), so `C#m9() → helper()` is never exact. Measured on
    /// `var/fix-pass10/x10ts` (scip-typescript 0.4.0).
    #[test]
    fn a_typescript_document_tree_sitter_cannot_parse_is_rejected() {
        let conflict = "export function helper(): number { return 1; }\nexport class C {\n  m1(): void {\n=======\n  }\n  m9(): void {\n>>>>>>> x\n    helper();\n  }\n}\n";
        let plain = "import { helper } from './conflict';\nexport class P {\n  run(): number { return helper(); }\n}\n";
        let pkg = "scip-typescript npm x10ts 1.0.0 src/";
        let (c, pl) = (format!("{pkg}`conflict.ts`/"), format!("{pkg}`plain.ts`/"));
        let index = ScipIndex {
            documents: vec![
                Document {
                    path: "src/conflict.ts".into(),
                    occurrences: vec![
                        enclosed(occ_at([0, 0, 0, 0], &c, 1), [0, 0, 10, 0]),
                        enclosed(
                            occ_at([0, 16, 0, 22], &format!("{c}helper()."), 1),
                            [0, 0, 0, 46],
                        ),
                        enclosed(occ_at([1, 13, 1, 14], &format!("{c}C#"), 1), [1, 0, 9, 1]),
                        enclosed(
                            occ_at([2, 2, 2, 4], &format!("{c}C#m1()."), 1),
                            [2, 2, 8, 3],
                        ),
                        occ_at([7, 4, 7, 10], &format!("{c}helper()."), 0),
                    ],
                    ..Default::default()
                },
                Document {
                    path: "src/plain.ts".into(),
                    occurrences: vec![
                        enclosed(occ_at([0, 0, 0, 0], &pl, 1), [0, 0, 4, 0]),
                        occ_at([0, 9, 0, 15], &format!("{c}helper()."), 0),
                        occ_at([0, 23, 0, 35], &c, 0),
                        enclosed(occ_at([1, 13, 1, 14], &format!("{pl}P#"), 1), [1, 0, 3, 1]),
                        enclosed(
                            occ_at([2, 2, 2, 5], &format!("{pl}P#run()."), 1),
                            [2, 2, 2, 36],
                        ),
                        occ_at([2, 25, 2, 31], &format!("{c}helper()."), 0),
                    ],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let facts = rb_syntax::extract(Language::TypeScript, conflict.as_bytes()).unwrap();
        assert!(facts.syntax_error.is_some());
        assert!(facts.definitions.iter().any(|d| d.name == "m9"));
        let joined = join_sources(
            Language::TypeScript,
            &[("src/conflict.ts", conflict), ("src/plain.ts", plain)],
            &index,
        );
        assert_eq!(
            joined.coverage.documents_rejected_reasons,
            BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)])
        );
        assert!(!joined.documented.contains("src/conflict.ts"));
        assert!(joined.documented.contains("src/plain.ts"));
        // `helper` is defined only in the rejected document: no node, no edge (amendment 64).
        assert!(joined.edges.is_empty(), "{:?}", edge_lines(&joined));
        assert_eq!(joined.coverage.references_undocumented, 1);
    }

    /// Contract M3b amendment 75: a bound local pairs a Tree-sitter definition only in the
    /// enum-constant-body shape, each part of it needed: the local is a method (kind
    /// `Method`), the Tree-sitter definition is a method, and the innermost definition range
    /// around the local is an enum constant (kind unspecified, as scip-java 0.13.1 gives it)
    /// of the Tree-sitter parent's type. `E`'s occurrences follow `Enm1`'s measured shape.
    #[test]
    fn the_enum_constant_body_shape_needs_every_part() {
        let rejected =
            |src: &str, n: &str, local_kind: &str, constant: &str, constant_kind: &str| {
                let sym = |d: &str| format!("scip-java maven . . p/{d}");
                let mut occurrences = vec![
                    enclosed(occ_at(tok(src, "E", 0), &sym("E#"), 1), [2, 0, 5, 1]),
                    enclosed(
                        occ_at(tok(src, "A", 0), &sym(constant), 1),
                        between(src, "A {", 0, "} }"),
                    ),
                    enclosed(
                        occ_at(tok(src, n, 0), "local 2", 1),
                        between(src, n, 0, "}"),
                    ),
                ];
                // `b`'s definition, after the call in the constant's body when there is one.
                let calls = src.matches("b()").count() - 1;
                if calls == 1 {
                    occurrences.push(occ_at(tok(src, "b()", 0), &sym("E#b()."), 0));
                }
                occurrences.push(enclosed(
                    occ_at(tok(src, "b()", calls), &sym("E#b()."), 1),
                    between(src, "void b", 0, "}"),
                ));
                occurrences.sort();
                let index = ScipIndex {
                    documents: vec![Document {
                        path: "p/E.java".into(),
                        occurrences,
                        symbols: vec![
                            SymbolInfo {
                                kind: constant_kind.into(),
                                ..enum_constant(&sym(constant))
                            },
                            SymbolInfo {
                                kind: local_kind.into(),
                                ..local_method("local 2", "n")
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                };
                join_sources(Language::Java, &[("p/E.java", src)], &index)
                    .coverage
                    .documents_rejected
            };
        let src = "package p;\n\nenum E {\n  A { void n() { b(); } };\n  void b() {}\n}\n";
        assert_eq!(rejected(src, "n()", "Method", "E#A.", ""), 0);
        assert_eq!(rejected(src, "n()", "Method", "E#A.", "EnumMember"), 0);
        // A field of the constant's body, or of another kind than a method.
        assert_eq!(rejected(src, "n()", "Field", "E#A.", ""), 1);
        // The innermost range is a field's, not an enum constant's.
        assert_eq!(rejected(src, "n()", "Method", "E#A.", "Field"), 1);
        // A constant of another type than the Tree-sitter parent's.
        assert_eq!(rejected(src, "n()", "Method", "F#A.", ""), 1);
        // A type Tree-sitter reads where the index has the local method.
        let class = "package p;\n\nenum E {\n  A { class n {} };\n  void b() {}\n}\n";
        let facts = rb_syntax::extract(Language::Java, class.as_bytes()).unwrap();
        assert!(
            facts
                .definitions
                .iter()
                .any(|d| d.name == "n" && is_type(d))
        );
        assert_eq!(rejected(class, "n {", "Method", "E#A.", ""), 1);
    }

    /// Contract M3b amendment 78 (the evaluator's N2): a declared supertype leads the
    /// `IMPLEMENTS` walk on only when the type's own symbol information relates it to the
    /// supertype's symbol. `Sup1.java` (`§` stands for a backslash) is `class Sup1<T extends
    /// Sup2>` for javac and `class Sup1 extends Sup2` for Tree-sitter; scip-java relates
    /// `Sup2#` to `Sup1#` and each `n()` to the other (amendment 16), `Sup1#` to nothing.
    /// Without the confirmation the reverse twin `Sup1#n() IMPLEMENTS Sup2#n()` was exact (the
    /// `2406e57` binary on `var/fix-pass11/t11esc`; occurrences and symbols as measured there,
    /// the decoder bypassed: it rejects the file for its escapes, amendment 77).
    #[test]
    fn a_supertype_the_index_does_not_relate_is_not_walked() {
        let src = "package p;\n\nclass Sup1 /* §u002a/ <T /* */ extends Sup2 /* §u002a/ > /* */ {\n  void n() {}\n}\n\nclass Sup2 extends Sup1<Sup2> {\n  void n() {}\n}\n"
            .replace('§', "\\");
        let p = "scip-java maven maven/t11/t11esc 1.0 p/";
        let measured: &[(Span, &str, i32, Option<Span>)] = &[
            ([2, 6, 2, 10], "Sup1#", 1, Some([2, 0, 4, 1])),
            ([2, 6, 2, 10], "Sup1#`<init>`().", 1, None),
            ([2, 23, 2, 24], "Sup1#[T]", 1, Some([2, 0, 4, 1])),
            ([2, 39, 2, 43], "Sup2#", 0, None),
            ([3, 7, 3, 8], "Sup1#n().", 1, Some([3, 2, 3, 13])),
            ([6, 6, 6, 10], "Sup2#", 1, Some([6, 0, 8, 1])),
            ([6, 6, 6, 10], "Sup2#`<init>`().", 1, None),
            ([6, 19, 6, 23], "Sup1#", 0, None),
            ([6, 24, 6, 28], "Sup2#", 0, None),
            ([7, 7, 7, 8], "Sup2#n().", 1, Some([7, 2, 7, 13])),
        ];
        let occurrences = measured
            .iter()
            .map(|&(span, d, roles, enclosing)| Occurrence {
                span,
                symbol: format!("{p}{d}"),
                roles,
                enclosing,
            })
            .collect();
        let related = |from: &str, kind: &str, to: Option<&str>| SymbolInfo {
            kind: kind.into(),
            ..match to {
                Some(to) => implementation(&format!("{p}{from}"), &format!("{p}{to}")),
                None => enum_constant(&format!("{p}{from}")),
            }
        };
        let symbols = |sup1: Option<&str>| {
            vec![
                related("Sup1#", "Class", sup1),
                related("Sup1#n().", "Method", Some("Sup2#n().")),
                related("Sup2#", "Class", Some("Sup1#")),
                related("Sup2#n().", "Method", Some("Sup1#n().")),
            ]
        };
        let join_with = |sup1: Option<&str>| {
            let index = ScipIndex {
                documents: vec![Document {
                    path: "p/Sup1.java".into(),
                    occurrences: Vec::clone(&occurrences),
                    symbols: symbols(sup1),
                    ..Default::default()
                }],
                ..Default::default()
            };
            join_sources(Language::Java, &[("p/Sup1.java", &src)], &index)
        };
        let joined = join_with(None);
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(joined.coverage.supertypes_unresolved, 0);
        assert_eq!(joined.coverage.supertypes_unconfirmed, 1, "Sup1 → Sup2");
        let implements: Vec<String> = edge_lines(&joined)
            .into_iter()
            .filter(|e| e.contains(" Implements "))
            .collect();
        assert_eq!(
            implements,
            [
                format!("{p}Sup2# Implements {p}Sup1#"),
                format!("{p}Sup2#n(). Implements {p}Sup1#n()."),
            ]
        );
        // Had the index related `Sup1#` to `Sup2#`, the clause would lead on (both directions
        // then reach: the cycle javac would refuse to compile).
        let confirmed = join_with(Some("Sup2#"));
        assert_eq!(confirmed.coverage.supertypes_unconfirmed, 0);
        assert!(
            edge_lines(&confirmed).contains(&format!("{p}Sup1#n(). Implements {p}Sup2#n().")),
            "{:?}",
            edge_lines(&confirmed)
        );
    }

    /// Contract M3b amendment 79 (the evaluator's N4, mutant MK2): a type reaches the pairing
    /// rule with a method anchor only through the amendment-7 fallback, when a method symbol
    /// whose last descriptor is the type's name stands before the type's name. Such an
    /// anchor pairs nothing: the document is rejected, while the type symbol at the same
    /// place (the `Fut` shape, amendment 75) is accepted. Synthetic layout.
    #[test]
    fn a_type_is_not_paired_by_a_method_named_like_it() {
        let src = "package p;\n\n@Ann(Foo.Foo)\nclass Foo {\n  void m() {}\n}\n";
        let p = "scip-java maven . . p/";
        let joined = |anchor: &str| {
            let mut occurrences = vec![
                enclosed(
                    occ_at(tok(src, "Foo", 1), &format!("{p}{anchor}"), 1),
                    [2, 0, 5, 1],
                ),
                occ_at(tok(src, "Foo", 2), &format!("{p}Foo#`<init>`()."), 1),
                enclosed(
                    occ_at(tok(src, "m()", 0), &format!("{p}Foo#m()."), 1),
                    between(src, "void m", 0, "}"),
                ),
            ];
            occurrences.sort();
            let index = ScipIndex {
                documents: vec![Document {
                    path: "p/Foo.java".into(),
                    occurrences,
                    ..Default::default()
                }],
                ..Default::default()
            };
            join_sources(Language::Java, &[("p/Foo.java", src)], &index)
        };
        let method = joined("Foo#Foo().");
        assert_eq!(
            method.coverage.documents_rejected_reasons,
            BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)])
        );
        let class = joined("Foo#");
        assert_eq!(class.coverage.documents_rejected, 0);
        assert_eq!(class.coverage.definitions_matched_by_name, 1);
    }

    /// Contract M3b amendments 75 and 79 (the evaluator's N4, mutant MK13): the syntax-error
    /// rule covers every document of a non-Java language scip-typescript indexes, TSX and
    /// JavaScript as well as TypeScript: a merge-conflict marker leaves Tree-sitter `ERROR`
    /// nodes in each, and the document is rejected; without the marker it is accepted.
    #[test]
    fn tsx_and_javascript_documents_tree_sitter_cannot_parse_are_rejected() {
        let conflict = "export function helper() { return 1; }\nexport class C {\n  m1() {\n=======\n  }\n  m9() {\n>>>>>>> x\n    helper();\n  }\n}\n";
        let clean = "export function helper() { return 1; }\nexport class C {\n  m1() {\n  }\n  m9() {\n    helper();\n  }\n}\n";
        for (language, path) in [
            (Language::Tsx, "src/c.tsx"),
            (Language::JavaScript, "src/c.js"),
            (Language::TypeScript, "src/c.ts"),
        ] {
            let index = ScipIndex {
                documents: vec![Document {
                    path: path.into(),
                    ..Default::default()
                }],
                ..Default::default()
            };
            let facts = rb_syntax::extract(language, conflict.as_bytes()).unwrap();
            assert!(facts.syntax_error.is_some(), "{path}");
            let rejected = join_sources(language, &[(path, conflict)], &index);
            assert_eq!(
                rejected.coverage.documents_rejected_reasons,
                BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)]),
                "{path}"
            );
            let accepted = join_sources(language, &[(path, clean)], &index);
            assert_eq!(accepted.coverage.documents_rejected, 0, "{path}");
            assert!(accepted.documented.contains(path), "{path}");
        }
    }

    /// A relationship whose source has no type holder (a top-level function) gives no
    /// `IMPLEMENTS` edge and no panic: its holder relates to nothing (amendment 78).
    #[test]
    fn a_source_without_a_type_holder_takes_no_walk() {
        let src = "export interface I { m(): void }\nexport function m() {}\n";
        let p = "scip-typescript npm t1 1.0.0 src/`a.ts`/";
        let index = ScipIndex {
            documents: vec![Document {
                path: "src/a.ts".into(),
                occurrences: vec![
                    occ_at(tok(src, "I", 0), &format!("{p}I#"), 1),
                    occ_at(tok(src, "m()", 0), &format!("{p}I#m()."), 1),
                    occ_at(tok(src, "m()", 1), &format!("{p}m()."), 1),
                ],
                symbols: vec![implementation(&format!("{p}m()."), &format!("{p}I#m()."))],
                ..Default::default()
            }],
            ..Default::default()
        };
        let joined = join_sources(Language::TypeScript, &[("src/a.ts", src)], &index);
        assert_eq!(joined.coverage.definitions_matched, 3);
        assert!(edge_lines(&joined).is_empty(), "{:?}", edge_lines(&joined));
    }

    // Contract M6a D1-D3 (S1, S2): Lombok-generated occurrences are removed before the
    // agreement checks. The synthetic documents below have the shapes `rb-scip-java:0.13.1`
    // gave Lombok 1.18.34 in probe P2b (`var/m6a/phase0/p2b`): a `lombok/Generated#` reference
    // at the marker span, every generated definition enclosed by exactly that span, generated
    // bodies' references at it, and generated definitions' own ranges where javac's positions
    // put them (after a class annotation, or on the first later text match of their name).
    // Every position is derived from the source.

    const LOMBOK: &str = "scip-java maven maven/org.projectlombok/lombok 1.18.34 lombok/";

    /// The span of the `nth` match of `needle` in `src`, all of it (one row).
    fn text_span(src: &str, needle: &str, nth: usize) -> Span {
        let at = tok(src, needle, nth);
        [at[0], at[1], at[0], at[1] + needle.len() as i32]
    }

    /// Lombok's marker reference at `span`.
    fn marker(span: Span) -> Occurrence {
        occ_at(span, &format!("{LOMBOK}Generated#"), 0)
    }

    /// A definition Lombok generated: its own range `span`, its enclosing range the marker's.
    fn generated(span: Span, symbol: &str, marker: Span) -> Occurrence {
        enclosed(occ_at(span, symbol, 1), marker)
    }

    /// A multi-line range from the end of `after` to column `col` of the next row: where
    /// scip-java puts the members a class annotation generates (P2b: `[6:5–7:6]` after `@Data`).
    fn after(after: Span, col: i32) -> Span {
        [after[0], after[3], after[0] + 1, col]
    }

    /// The index of one Java document.
    fn java_index(
        path: &str,
        mut occurrences: Vec<Occurrence>,
        symbols: Vec<SymbolInfo>,
    ) -> ScipIndex {
        occurrences.sort();
        ScipIndex {
            documents: vec![Document {
                path: path.into(),
                occurrences,
                symbols,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    /// `joined`'s coverage without the Lombok counters, for comparing with a twin index that
    /// has no Lombok occurrences.
    fn without_lombok(joined: &Joined) -> Coverage {
        Coverage {
            lombok_removed: 0,
            documents_lombok: 0,
            ..joined.coverage.clone()
        }
    }

    const USER: &str = "package p;\n\nimport lombok.Data;\n\n@Data\npublic class User {\n    private String name;\n\n    public String greet() { return name + helper(); }\n    private String helper() { return \"!\"; }\n}\n";

    /// `User.java` with `@Data`: the hand-written occurrences, and Lombok's (when `lombok`):
    /// the marker at `@Data`, the generated bodies' references there, the setter, `equals`,
    /// `canEqual`, `hashCode`, `toString`, the getter and the constructor enclosed by it, and
    /// a generated parameter (a local).
    fn user_occurrences(marker_symbol: &str, lombok: bool) -> (Vec<Occurrence>, usize) {
        let sym = |d: &str| format!("scip-java maven maven/com.acme/demo 1.0 p/User#{d}");
        let mut occurrences = vec![
            enclosed(occ_at(tok(USER, "User", 0), &sym(""), 1), [4, 0, 10, 1]),
            enclosed(
                occ_at(tok(USER, "name", 0), &sym("name."), 1),
                between(USER, "private String name", 0, ";"),
            ),
            enclosed(
                occ_at(tok(USER, "greet", 0), &sym("greet()."), 1),
                between(USER, "public String greet", 0, "}"),
            ),
            occ_at(tok(USER, "name", 1), &sym("name."), 0),
            occ_at(tok(USER, "helper", 0), &sym("helper()."), 0),
            enclosed(
                occ_at(tok(USER, "helper", 1), &sym("helper()."), 1),
                between(USER, "private String helper", 0, "}"),
            ),
        ];
        if !lombok {
            // Without Lombok, javac's implicit constructor has no range.
            occurrences.push(occ_at(tok(USER, "User", 0), &sym("`<init>`()."), 1));
            return (occurrences, 0);
        }
        let data = text_span(USER, "@Data", 0);
        let lombok = vec![
            occ_at(data, marker_symbol, 0),
            occ_at(data, &sym("name."), 0),
            occ_at(data, "scip-java maven jdk 21 java/lang/Object#equals().", 0),
            occ_at(data, &sym("canEqual()."), 0),
            generated(tok(USER, "User", 0), &sym("`<init>`()."), data),
            generated(after(data, 6), &sym("setName()."), data),
            generated(after(data, 5), &sym("equals()."), data),
            generated(after(data, 7), &sym("canEqual()."), data),
            generated(after(data, 7), &sym("hashCode()."), data),
            generated(after(data, 7), &sym("toString()."), data),
            generated(after(data, 6), &sym("getName()."), data),
            generated(after(data, 0), "local 0", data),
        ];
        let n = lombok.len();
        occurrences.extend(lombok);
        (occurrences, n)
    }

    /// S1 (a), (c), (d): a `@Data` class. At `cfa8ab9` the generated members, which only javac
    /// sees, make the document a `structure_disagreement`; with them removed it is accepted,
    /// `greet() → helper()` is exact, and the result equals the twin index without Lombok's
    /// occurrences except for the counters: the generated bodies' references and parameters
    /// leave no trace but `lombok_removed`.
    #[test]
    fn a_data_class_is_accepted_once_its_generated_members_are_removed() {
        let path = "p/User.java";
        let (occurrences, removed) = user_occurrences(&format!("{LOMBOK}Generated#"), true);
        let joined = join_sources(
            Language::Java,
            &[(path, USER)],
            &java_index(path, occurrences, Vec::new()),
        );
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert!(joined.documented.contains(path));
        let sym = |d: &str| format!("scip-java maven maven/com.acme/demo 1.0 p/User#{d}");
        assert_eq!(
            edge_lines(&joined),
            [format!("{} Calls {}", sym("greet()."), sym("helper()."))]
        );
        assert_eq!(
            (
                joined.coverage.lombok_removed,
                joined.coverage.documents_lombok
            ),
            (removed, 1)
        );
        let (plain, _) = user_occurrences("", false);
        let twin = join_sources(
            Language::Java,
            &[(path, USER)],
            &java_index(path, plain, Vec::new()),
        );
        assert_eq!(
            (
                &joined.keys,
                &joined.edges,
                &joined.documented,
                &joined.packages
            ),
            (&twin.keys, &twin.edges, &twin.documented, &twin.packages)
        );
        assert_eq!(without_lombok(&joined), twin.coverage);
        assert_eq!(
            (twin.coverage.lombok_removed, twin.coverage.documents_lombok),
            (0, 0)
        );
    }

    /// S1 (j), (k): D1 never clears a verdict it did not cause. A Lombok document the decoder
    /// rejected (`structural_escape`) stays rejected for that reason; one that also holds a
    /// member only javac sees without a marker stays a `structure_disagreement`; a
    /// `lombok/Generated#` of another package is no marker, so nothing is removed.
    #[test]
    fn lombok_removal_never_clears_another_verdict() {
        let path = "p/User.java";
        let sym = |d: &str| format!("scip-java maven maven/com.acme/demo 1.0 p/User#{d}");
        let marker_symbol = format!("{LOMBOK}Generated#");
        let joined_with = |occurrences: Vec<Occurrence>, rejected: Option<rb_scip::Rejection>| {
            let mut index = java_index(path, occurrences, Vec::new());
            index.documents[0].rejected = rejected;
            join_sources(Language::Java, &[(path, USER)], &index)
        };
        let (occurrences, _) = user_occurrences(&marker_symbol, true);
        let escaped = joined_with(
            occurrences.clone(),
            Some(rb_scip::Rejection::StructuralEscape),
        );
        assert_eq!(
            escaped.coverage.documents_rejected_reasons,
            BTreeMap::from([(rb_scip::Rejection::StructuralEscape, 1)])
        );
        let mut hidden = occurrences;
        hidden.push(generated(
            after(text_span(USER, "private String name;", 0), 4),
            &sym("hidden()."),
            between(USER, "private String name", 0, ";"),
        ));
        let unmarked = joined_with(hidden, None);
        assert_eq!(
            unmarked.coverage.documents_rejected_reasons,
            BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)])
        );
        let (fake, _) = user_occurrences(
            "scip-java maven maven/com.example/fake 1.0 lombok/Generated#",
            true,
        );
        let fake = joined_with(fake, None);
        assert_eq!(
            fake.coverage.documents_rejected_reasons,
            BTreeMap::from([(rb_scip::Rejection::StructureDisagreement, 1)])
        );
        for joined in [&escaped, &unmarked, &fake] {
            assert!(joined.edges.is_empty());
            assert_eq!(
                (
                    joined.coverage.lombok_removed,
                    joined.coverage.documents_lombok
                ),
                (0, 0),
                "a rejected document counts no removal"
            );
        }
    }

    /// S1 (b): a getter a class-level `@Getter` generates from a field. scip-java places its
    /// definition at the first later text match of its name, the `getName()` call inside
    /// `greet()` beside the real reference there, with the field's declarator (`name;`) as its
    /// enclosing range and the marker; at `cfa8ab9` that is a `definition_disagreement` (a
    /// method defined on a reference inside a method). Removed, the document is accepted, the
    /// real reference stays and counts as `undocumented` (the project's own member without a
    /// definition, like a record's accessors; amendment 2), and no edge targets the getter.
    #[test]
    fn a_field_getter_defined_at_a_call_is_removed_and_its_call_is_undocumented() {
        let src = "package p;\n\nimport lombok.Getter;\n\n@Getter\npublic class Acct {\n    private String name;\n\n    public String greet() { return getName() + helper(); }\n    private String helper() { return \"!\"; }\n}\n";
        let path = "p/Acct.java";
        let sym = |d: &str| format!("scip-java maven maven/com.acme/demo 1.0 p/Acct#{d}");
        let field = text_span(src, "name;", 0);
        let mut occurrences = vec![
            enclosed(occ_at(tok(src, "Acct", 0), &sym(""), 1), [4, 0, 10, 1]),
            occ_at(tok(src, "Acct", 0), &sym("`<init>`()."), 1),
            enclosed(
                occ_at(tok(src, "name", 0), &sym("name."), 1),
                between(src, "private String name", 0, ";"),
            ),
            enclosed(
                occ_at(tok(src, "greet", 0), &sym("greet()."), 1),
                between(src, "public String greet", 0, "}"),
            ),
            occ_at(tok(src, "getName", 0), &sym("getName()."), 0),
            occ_at(tok(src, "helper", 0), &sym("helper()."), 0),
            enclosed(
                occ_at(tok(src, "helper", 1), &sym("helper()."), 1),
                between(src, "private String helper", 0, "}"),
            ),
        ];
        let lombok = vec![
            marker(field),
            occ_at(field, &sym("name."), 0),
            occ_at(field, "scip-java maven jdk 21 java/lang/String#", 0),
            generated(tok(src, "getName", 0), &sym("getName()."), field),
        ];
        let removed = lombok.len();
        occurrences.extend(lombok);
        let joined = join_sources(
            Language::Java,
            &[(path, src)],
            &java_index(path, occurrences, Vec::new()),
        );
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(
            edge_lines(&joined),
            [format!("{} Calls {}", sym("greet()."), sym("helper()."))]
        );
        assert_eq!(joined.coverage.references_undocumented, 1);
        assert_eq!(
            (
                joined.coverage.lombok_removed,
                joined.coverage.documents_lombok
            ),
            (removed, 1)
        );
    }

    /// S1 (e): `@Builder`'s nested type, its members and `builder()`, all enclosed by the
    /// `@Builder` marker: at `cfa8ab9` the nested type, which only javac sees, makes the
    /// document a `structure_disagreement`. Removed, it is accepted; the calls into the
    /// builder are the project's own members without definitions (`undocumented`), never edges.
    #[test]
    fn a_builder_and_its_nested_type_are_removed() {
        let src = "package p;\n\nimport lombok.Builder;\n\n@Builder\npublic class Built {\n    private final String id;\n\n    public static Built make() { return Built.builder().id(\"x\").build(); }\n}\n";
        let path = "p/Built.java";
        let sym = |d: &str| format!("scip-java maven maven/com.acme/demo 1.0 p/Built#{d}");
        let builder = text_span(src, "@Builder", 0);
        let mut occurrences = vec![
            enclosed(occ_at(tok(src, "Built", 0), &sym(""), 1), [4, 0, 9, 1]),
            enclosed(
                occ_at(tok(src, "id", 0), &sym("id."), 1),
                between(src, "private final String id", 0, ";"),
            ),
            enclosed(
                occ_at(tok(src, "make", 0), &sym("make()."), 1),
                between(src, "public static Built make", 0, "}"),
            ),
            occ_at(tok(src, "Built", 1), &sym(""), 0),
            occ_at(tok(src, "Built", 2), &sym(""), 0),
            occ_at(tok(src, "builder()", 0), &sym("builder()."), 0),
            occ_at(tok(src, "id(", 0), &sym("BuiltBuilder#id()."), 0),
            occ_at(tok(src, "build()", 0), &sym("BuiltBuilder#build()."), 0),
        ];
        let lombok = vec![
            marker(builder),
            occ_at(builder, &sym("id."), 0),
            generated(tok(src, "Built", 0), &sym("`<init>`()."), builder),
            generated(after(builder, 9), &sym("BuiltBuilder#"), builder),
            generated(after(builder, 9), &sym("BuiltBuilder#`<init>`()."), builder),
            generated(after(builder, 4), &sym("BuiltBuilder#build()."), builder),
            generated(after(builder, 7), &sym("BuiltBuilder#toString()."), builder),
            generated(tok(src, "id", 0), &sym("BuiltBuilder#id."), builder),
            generated(tok(src, "id", 0), &sym("BuiltBuilder#id()."), builder),
            generated(after(builder, 6), &sym("builder()."), builder),
            generated(tok(src, "id", 0), "local 0", builder),
        ];
        let removed = lombok.len();
        occurrences.extend(lombok);
        let joined = join_sources(
            Language::Java,
            &[(path, src)],
            &java_index(path, occurrences, Vec::new()),
        );
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert!(joined.documented.contains(path));
        assert!(
            joined.edges.iter().all(|e| e.kind == EdgeKind::References),
            "{:?}",
            edge_lines(&joined)
        );
        assert_eq!(joined.coverage.references_undocumented, 3);
        assert_eq!(
            (
                joined.coverage.lombok_removed,
                joined.coverage.documents_lombok
            ),
            (removed, 1)
        );
    }

    /// S1 (f): a hand-written method annotated `@lombok.Generated` keeps its definition (its
    /// enclosing range runs from the annotation to its closing brace, never the marker's span),
    /// is paired and keyed, and its call is exact; only the marker reference itself is removed.
    #[test]
    fn a_hand_written_generated_method_is_kept() {
        let src = "package p;\n\npublic class Marked {\n    @lombok.Generated\n    public int value() { return 1; }\n\n    public int use() { return value(); }\n}\n";
        let path = "p/Marked.java";
        let sym = |d: &str| format!("scip-java maven maven/com.acme/demo 1.0 p/Marked#{d}");
        let annotation = text_span(src, "@lombok.Generated", 0);
        let value_end = text_span(src, "public int value() { return 1; }", 0);
        let occurrences = vec![
            enclosed(occ_at(tok(src, "Marked", 0), &sym(""), 1), [2, 0, 7, 1]),
            occ_at(tok(src, "Marked", 0), &sym("`<init>`()."), 1),
            occ_at(tok(src, "lombok", 0), "scip-java maven . . lombok/", 0),
            marker(tok(src, "Generated", 0)),
            enclosed(
                occ_at(tok(src, "value", 0), &sym("value()."), 1),
                [annotation[0], annotation[1], value_end[2], value_end[3]],
            ),
            enclosed(
                occ_at(tok(src, "use", 0), &sym("use()."), 1),
                between(src, "public int use", 0, "}"),
            ),
            occ_at(tok(src, "value", 1), &sym("value()."), 0),
        ];
        let joined = join_sources(
            Language::Java,
            &[(path, src)],
            &java_index(path, occurrences, Vec::new()),
        );
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(
            edge_lines(&joined),
            [format!("{} Calls {}", sym("use()."), sym("value()."))]
        );
        assert!(joined.keys[path].values().any(|k| *k == sym("value().")));
        assert_eq!(
            (
                joined.coverage.lombok_removed,
                joined.coverage.documents_lombok
            ),
            (1, 1)
        );
    }

    /// S1 (g): with a class-level `@Getter`, a field's marker spans its whole declarator, here
    /// an anonymous class's body. Only occurrences at the marker's span and definitions
    /// enclosed by exactly that span are removed: the anonymous class's method (a local with
    /// its own range) and the call inside it are kept and judged as without Lombok (the call is
    /// credited to the class, design §8.2), equal to the twin index without Lombok.
    #[test]
    fn an_anonymous_class_inside_a_marker_span_is_kept() {
        let src = "package p;\n\nimport lombok.Getter;\n\n@Getter\npublic class Anon {\n    private final Runnable task = new Runnable() { public void run() { helper(); } };\n\n    static void helper() {}\n}\n";
        let path = "p/Anon.java";
        let sym = |d: &str| format!("scip-java maven maven/com.acme/demo 1.0 p/Anon#{d}");
        let declarator = between(src, "task = new", 0, "} };");
        let plain = vec![
            enclosed(occ_at(tok(src, "Anon", 0), &sym(""), 1), [4, 0, 9, 1]),
            occ_at(tok(src, "Anon", 0), &sym("`<init>`()."), 1),
            enclosed(
                occ_at(tok(src, "task", 0), &sym("task."), 1),
                between(src, "private final Runnable task", 0, "} };"),
            ),
            occ_at(tok(src, "Runnable", 1), "local 0", 1),
            enclosed(
                occ_at(tok(src, "run", 0), "local 1", 1),
                between(src, "public void run", 0, "}"),
            ),
            occ_at(tok(src, "helper", 0), &sym("helper()."), 0),
            enclosed(
                occ_at(tok(src, "helper", 1), &sym("helper()."), 1),
                between(src, "static void helper", 0, "}"),
            ),
        ];
        let mut occurrences = plain.clone();
        occurrences.extend([
            marker(declarator),
            occ_at(declarator, &sym("task."), 0),
            generated(after(declarator, 4), &sym("getTask()."), declarator),
        ]);
        let joined = join_sources(
            Language::Java,
            &[(path, src)],
            &java_index(path, occurrences, Vec::new()),
        );
        let twin = join_sources(
            Language::Java,
            &[(path, src)],
            &java_index(path, plain, Vec::new()),
        );
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(
            edge_lines(&joined),
            [format!("{} Calls {}", sym(""), sym("helper()."))]
        );
        assert_eq!((&joined.edges, &joined.keys), (&twin.edges, &twin.keys));
        assert_eq!(without_lombok(&joined), twin.coverage);
        assert_eq!(joined.coverage.lombok_removed, 3);
    }

    /// S1 (h): an enum with `@Getter` whose constant body defines a method: the generated
    /// getter, defined by scip-java at the `getCode()` call inside `show()` (a
    /// `definition_disagreement` at `cfa8ab9`), is removed; the constant body's method still
    /// binds through its local symbol (amendment 54).
    #[test]
    fn an_enum_constant_body_still_binds_in_a_lombok_enum() {
        let src = "package p;\n\nimport lombok.Getter;\n\n@Getter\npublic enum Color {\n    RED { public String show() { return \"R\"; } },\n    GREEN;\n\n    private final String code = \"c\";\n\n    public String show() { return getCode(); }\n}\n";
        let path = "p/Color.java";
        let sym = |d: &str| format!("scip-java maven maven/com.acme/demo 1.0 p/Color#{d}");
        let field = text_span(src, "code = \"c\";", 0);
        let occurrences = vec![
            occ_at(tok(src, "Color", 0), &sym(""), 1),
            enclosed(
                occ_at(tok(src, "RED", 0), &sym("RED."), 1),
                between(src, "RED {", 0, "} }"),
            ),
            occ_at(tok(src, "show", 0), "local 3", 1),
            occ_at(tok(src, "GREEN", 0), &sym("GREEN."), 1),
            occ_at(tok(src, "code", 0), &sym("code."), 1),
            occ_at(tok(src, "show", 1), &sym("show()."), 1),
            occ_at(tok(src, "getCode", 0), &sym("getCode()."), 0),
            marker(field),
            occ_at(field, &sym("code."), 0),
            generated(tok(src, "getCode", 0), &sym("getCode()."), field),
        ];
        let symbols = vec![
            local_method("local 3", "show"),
            enum_constant(&sym("RED.")),
            enum_constant(&sym("GREEN.")),
        ];
        let joined = join_sources(
            Language::Java,
            &[(path, src)],
            &java_index(path, occurrences, symbols),
        );
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(joined.coverage.definitions_local, 1);
        assert_eq!(joined.coverage.references_undocumented, 1);
        assert_eq!(
            (
                joined.coverage.lombok_removed,
                joined.coverage.documents_lombok
            ),
            (3, 1)
        );
    }

    const SHADOW: &str = "package p;\n\nimport lombok.Getter;\n\npublic class Shadow {\n    public String run() { return new Named().getName() + new Explicit().getName(); }\n}\n\nclass Named {\n    @Getter String name;\n}\n\nclass Explicit {\n    String getName() { return \"e\"; }\n}\n";

    /// S2 with P2b's real positions (`Shadow.java`): scip-java defines the getter `Named`'s
    /// field-level `@Getter` generates at `Explicit`'s declaration name, enclosed by the
    /// `@Getter` marker. `Explicit`'s definition keeps `Explicit#getName().`, the call through
    /// `new Explicit()` is exact to it, the call through `new Named()` is `undocumented`, and no
    /// edge targets `Named#getName().`. At `cfa8ab9` the unpaired generated method rejects the
    /// document (`structure_disagreement`).
    #[test]
    fn a_hand_written_method_at_a_generated_definitions_position_keeps_its_own_key() {
        let path = "p/Shadow.java";
        let sym = |d: &str| format!("scip-java maven maven/com.acme/demo 1.0 p/{d}");
        let getter = text_span(SHADOW, "@Getter", 0);
        let occurrences = vec![
            enclosed(
                occ_at(tok(SHADOW, "Shadow", 0), &sym("Shadow#"), 1),
                [4, 0, 6, 1],
            ),
            occ_at(tok(SHADOW, "Shadow", 0), &sym("Shadow#`<init>`()."), 1),
            enclosed(
                occ_at(tok(SHADOW, "run", 0), &sym("Shadow#run()."), 1),
                between(SHADOW, "public String run", 0, "}"),
            ),
            occ_at(tok(SHADOW, "Named", 0), &sym("Named#`<init>`()."), 0),
            occ_at(tok(SHADOW, "getName", 0), &sym("Named#getName()."), 0),
            occ_at(tok(SHADOW, "Explicit", 0), &sym("Explicit#`<init>`()."), 0),
            occ_at(tok(SHADOW, "getName", 1), &sym("Explicit#getName()."), 0),
            enclosed(
                occ_at(tok(SHADOW, "Named", 1), &sym("Named#"), 1),
                [8, 0, 10, 1],
            ),
            occ_at(tok(SHADOW, "Named", 1), &sym("Named#`<init>`()."), 1),
            marker(getter),
            occ_at(getter, &sym("Named#name."), 0),
            occ_at(getter, "scip-java maven jdk 21 java/lang/String#", 0),
            enclosed(
                occ_at(tok(SHADOW, "name", 0), &sym("Named#name."), 1),
                between(SHADOW, "@Getter String name", 0, ";"),
            ),
            enclosed(
                occ_at(tok(SHADOW, "Explicit", 1), &sym("Explicit#"), 1),
                [12, 0, 14, 1],
            ),
            occ_at(tok(SHADOW, "Explicit", 1), &sym("Explicit#`<init>`()."), 1),
            enclosed(
                occ_at(tok(SHADOW, "getName", 2), &sym("Explicit#getName()."), 1),
                between(SHADOW, "String getName()", 0, "}"),
            ),
            generated(tok(SHADOW, "getName", 2), &sym("Named#getName()."), getter),
        ];
        let joined = join_sources(
            Language::Java,
            &[(path, SHADOW)],
            &java_index(path, occurrences, Vec::new()),
        );
        assert_eq!(joined.coverage.documents_rejected, 0);
        let keys: Vec<&String> = joined.keys[path].values().collect();
        assert!(keys.contains(&&sym("Explicit#getName().")), "{keys:?}");
        assert!(!keys.contains(&&sym("Named#getName().")), "{keys:?}");
        let edges = edge_lines(&joined);
        assert!(
            edges.contains(&format!(
                "{} Calls {}",
                sym("Shadow#run()."),
                sym("Explicit#getName().")
            )),
            "{edges:?}"
        );
        assert!(
            edges.iter().all(|e| !e.ends_with(&sym("Named#getName()."))),
            "{edges:?}"
        );
        assert_eq!(joined.coverage.references_undocumented, 1);
    }

    /// A document of `n` generated members, each a marker with a definition it encloses and a
    /// body reference at it, beside `n` real definitions.
    fn lombok_work_for(n: usize) -> u64 {
        let sym = |d: String| format!("scip-java maven maven/com.acme/demo 1.0 p/Big#{d}");
        let mut occurrences = Vec::new();
        for i in 0..n as i32 {
            let at: Span = [i, 0, i, 5];
            occurrences.extend([
                marker(at),
                occ_at(at, &sym(format!("f{i}.")), 0),
                generated([i, 5, i + 1, 3], &sym(format!("getF{i}().")), at),
                enclosed(
                    occ_at([i, 10, i, 12], &sym(format!("m{i}().")), 1),
                    [i, 6, i, 30],
                ),
            ]);
        }
        occurrences.sort();
        let doc = Document {
            path: "p/Big.java".into(),
            occurrences,
            ..Default::default()
        };
        rb_frameworks::work::take();
        let (_, removed) = remove_lombok_generated(&doc).unwrap();
        assert_eq!(removed, 3 * n);
        rb_frameworks::work::take()
    }

    /// Contract M6a S19, invariants 9 and 10: D1's removal is one pass into a sorted set and
    /// one filtering pass (counted: 4× the occurrences and markers → at most 5× the work).
    #[test]
    fn lombok_removal_grows_linearly() {
        let (small, large) = (lombok_work_for(100), lombok_work_for(400));
        assert!(large <= small * 5, "{small} -> {large}");
    }

    // Contract M6a D4 (S5): scip-typescript relates an interface method to the methods it
    // overrides (sub → super only, at every depth) and an interface type to nothing (probes P1,
    // P1b). The walk from an interface method's holder steps onto its declared supertypes,
    // resolved through the index's own type occurrences, without the origin's (empty) relations.

    const SHAPES: &str = "export interface Base { m(): void; n(x: number): number }\nexport interface Mid extends Base { m(): void; extra(): void }\nexport interface Deep extends Mid { m(): void }\nexport interface Other extends Mid, Base { n(x: number): number }\nexport interface GBase<T> { g(x: T): T }\nexport interface GSub extends GBase<string> { g(x: string): string }\nexport interface Unrelated { m(): void }\nexport interface Prop extends Base { m: () => void }\n";

    /// S5's positive rows: one level, two clauses, depth 3 (both overridden members) and a
    /// generic super-interface, from the relationships P1b recorded; each needs D4 (at
    /// `cfa8ab9` every one is refused for want of the interface's own relations).
    #[test]
    fn typescript_interface_methods_implement_what_they_override() {
        let f = "src/`shapes.ts`/";
        let rels = [
            (format!("{f}Mid#m()."), format!("{f}Base#m().")),
            (format!("{f}Deep#m()."), format!("{f}Mid#m().")),
            (format!("{f}Deep#m()."), format!("{f}Base#m().")),
            (format!("{f}Other#n()."), format!("{f}Base#n().")),
            (format!("{f}GSub#g()."), format!("{f}GBase#g().")),
        ];
        let rels: Vec<(&str, &str)> = rels.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let joined = hierarchy_joined(Language::TypeScript, &[("shapes.ts", SHAPES)], &rels, &[]);
        assert_eq!(
            implements_lines(Language::TypeScript, &joined),
            [
                format!("{f}Deep#m(). Implements {f}Base#m()."),
                format!("{f}Deep#m(). Implements {f}Mid#m()."),
                format!("{f}GSub#g(). Implements {f}GBase#g()."),
                format!("{f}Mid#m(). Implements {f}Base#m()."),
                format!("{f}Other#n(). Implements {f}Base#n()."),
            ]
        );
        assert_eq!(joined.coverage.relationships_dropped, 0);
        assert_eq!(joined.coverage.supertypes_unresolved, 0);
    }

    /// S5's negative rows: a relationship whose target's type the source's declared clauses do
    /// not reach (`Mid#m() → Unrelated#m()`, fabricated), the reverse of an override (`Base#m()
    /// → Mid#m()`, fabricated: `Base` declares no clause reaching `Mid`), a member with no super
    /// member (`Mid#extra()`, no relationship), and a property signature (`Prop#m`, not a
    /// method: no definition, its link dropped) give no edge.
    #[test]
    fn typescript_interface_links_the_clauses_do_not_confirm_give_no_edge() {
        let f = "src/`shapes.ts`/";
        let rels = [
            (format!("{f}Mid#m()."), format!("{f}Unrelated#m().")),
            (format!("{f}Base#m()."), format!("{f}Mid#m().")),
            (format!("{f}Prop#m."), format!("{f}Base#m().")),
        ];
        let rels: Vec<(&str, &str)> = rels.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let joined = hierarchy_joined(Language::TypeScript, &[("shapes.ts", SHAPES)], &rels, &[]);
        assert!(
            implements_lines(Language::TypeScript, &joined).is_empty(),
            "{:?}",
            implements_lines(Language::TypeScript, &joined)
        );
    }

    /// S5: a supertype imported from another file and a namespace-qualified one
    /// (`extends ns.Base`), each resolved through the index's type occurrence at the written
    /// name (amendment 26), as scip-typescript emits it (P1b: the imported module's symbol).
    #[test]
    fn typescript_interface_methods_implement_imported_and_qualified_supertypes() {
        let base = "export interface Base { m(): void }\n";
        let forms = "import { Base } from \"./base\";\nimport * as ns from \"./base\";\nexport interface Imp extends Base { m(): void }\nexport interface Q extends ns.Base { m(): void }\n";
        let b = "src/`base.ts`/";
        let f = "src/`forms.ts`/";
        let rels = [
            (format!("{f}Imp#m()."), format!("{b}Base#m().")),
            (format!("{f}Q#m()."), format!("{b}Base#m().")),
        ];
        let rels: Vec<(&str, &str)> = rels.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let base_type = format!("{b}Base#");
        let extra = [
            ("forms.ts", at_nth(forms, 2, "Base", 0), base_type.as_str()),
            ("forms.ts", at_nth(forms, 3, "Base", 0), base_type.as_str()),
        ];
        let joined = hierarchy_joined(
            Language::TypeScript,
            &[("base.ts", base), ("forms.ts", forms)],
            &rels,
            &extra,
        );
        assert_eq!(
            implements_lines(Language::TypeScript, &joined),
            [
                format!("{f}Imp#m(). Implements {b}Base#m()."),
                format!("{f}Q#m(). Implements {b}Base#m()."),
            ]
        );
    }

    /// Contract M6a D4's scope: the waiver is for scip-typescript interfaces only. Amendment
    /// 78's shape with Java interfaces (`§` stands for a backslash): javac reads `interface
    /// Sup1<T extends Sup2>`, Tree-sitter `interface Sup1 extends Sup2`; scip-java relates
    /// `Sup2#` to `Sup1#`, each `n()` to the other, `Sup1#` to nothing (as it does for the
    /// classes of `a_supertype_the_index_does_not_relate_is_not_walked`; positions derived from
    /// the source). The reverse twin `Sup1#n() → Sup2#n()` stays unconfirmed: no edge.
    #[test]
    fn a_java_interface_keeps_its_origin_confirmation() {
        let src = "package p;\n\ninterface Sup1 /* §u002a/ <T /* */ extends Sup2 /* §u002a/ > /* */ {\n  void n();\n}\n\ninterface Sup2 extends Sup1<Sup2> {\n  void n();\n}\n"
            .replace('§', "\\");
        let p = "scip-java maven maven/t11/t11esc 1.0 p/";
        let occurrences: Vec<Occurrence> = [
            (at_nth(&src, 2, "Sup1", 0), "Sup1#", 1, Some([2, 0, 4, 1])),
            (at_nth(&src, 2, "T", 0), "Sup1#[T]", 1, Some([2, 0, 4, 1])),
            (at_nth(&src, 2, "Sup2", 0), "Sup2#", 0, None),
            (
                at_nth(&src, 3, "n", 0),
                "Sup1#n().",
                1,
                Some(between(&src, "void n", 0, ";")),
            ),
            (at_nth(&src, 6, "Sup2", 0), "Sup2#", 1, Some([6, 0, 8, 1])),
            (at_nth(&src, 6, "Sup1", 0), "Sup1#", 0, None),
            (at_nth(&src, 6, "Sup2", 1), "Sup2#", 0, None),
            (
                at_nth(&src, 7, "n", 0),
                "Sup2#n().",
                1,
                Some(between(&src, "void n", 1, ";")),
            ),
        ]
        .into_iter()
        .map(|(span, d, roles, enclosing)| Occurrence {
            span,
            symbol: format!("{p}{d}"),
            roles,
            enclosing,
        })
        .collect();
        let kind = |info: SymbolInfo, kind: &str| SymbolInfo {
            kind: kind.into(),
            ..info
        };
        let symbols = vec![
            kind(enum_constant(&format!("{p}Sup1#")), "Interface"),
            kind(
                implementation(&format!("{p}Sup1#n()."), &format!("{p}Sup2#n().")),
                "Method",
            ),
            kind(
                implementation(&format!("{p}Sup2#"), &format!("{p}Sup1#")),
                "Interface",
            ),
            kind(
                implementation(&format!("{p}Sup2#n()."), &format!("{p}Sup1#n().")),
                "Method",
            ),
        ];
        let index = ScipIndex {
            documents: vec![Document {
                path: "p/Sup1.java".into(),
                occurrences,
                symbols,
                ..Default::default()
            }],
            ..Default::default()
        };
        let joined = join_sources(Language::Java, &[("p/Sup1.java", &src)], &index);
        assert_eq!(joined.coverage.documents_rejected, 0);
        assert_eq!(joined.coverage.supertypes_unconfirmed, 1, "Sup1 → Sup2");
        let implements: Vec<String> = edge_lines(&joined)
            .into_iter()
            .filter(|e| e.contains(" Implements "))
            .collect();
        assert_eq!(
            implements,
            [
                format!("{p}Sup2# Implements {p}Sup1#"),
                format!("{p}Sup2#n(). Implements {p}Sup1#n()."),
            ]
        );
    }

    /// Contract M6a D4's scope: a scip-typescript class keeps amendment 78's confirmation (it
    /// relates its type to every supertype, P1). A clause its symbol information does not
    /// relate (here `A#` relates to nothing, as for a clause only Tree-sitter reads) does not
    /// lead the walk on, so `A#m() → B#m()` gives no edge and the clause is counted.
    #[test]
    fn a_typescript_class_keeps_its_origin_confirmation() {
        let src = "export class B { m(): void {} }\nexport class A extends B { m(): void {} }\n";
        let f = "src/`a.ts`/";
        let rels = [(format!("{f}A#m()."), format!("{f}B#m()."))];
        let rels: Vec<(&str, &str)> = rels.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let joined = hierarchy_joined(Language::TypeScript, &[("a.ts", src)], &rels, &[]);
        assert_eq!(joined.coverage.supertypes_unconfirmed, 1);
        assert!(implements_lines(Language::TypeScript, &joined).is_empty());
    }
}
