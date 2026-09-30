//! The SCIP⋈Tree-sitter join (design §5.3): a project's decoded index, matched by range against
//! the same files' syntax facts, gives exact symbol keys and exact `CALLS`, `REFERENCES` and
//! `IMPLEMENTS` edges. Tree-sitter supplies the syntactic role (a call, a type use, an
//! implementation) and the enclosing definition; SCIP supplies which symbol a name refers to.

use std::collections::{BTreeMap, BTreeSet};

use rb_model::{Evidence, Span};
use rb_scip::ScipIndex;
use rb_syntax::{FileFacts, RefKind};

/// What one project's index contributes.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Joined {
    /// Per file, per definition index: the SCIP symbol that replaces its heuristic key.
    pub keys: BTreeMap<String, BTreeMap<usize, String>>,
    /// Exact edges; every target is a symbol whose definition matched, so the graph has its node.
    pub edges: Vec<ScipEdge>,
    pub coverage: Coverage,
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
    /// Tree-sitter definitions matched to a SCIP definition occurrence.
    pub definitions_matched: usize,
    pub definitions_total: usize,
    /// Tree-sitter references matched to an occurrence of a symbol defined in the index.
    pub references_matched: usize,
    /// Matched an occurrence, but the symbol is defined outside the index (a dependency).
    pub references_external: usize,
    /// Matched an occurrence that is a document-local symbol (never a node).
    pub references_local: usize,
    /// Matched an occurrence of a symbol the index defines, but at a range Tree-sitter does not
    /// treat as a definition (a field, a constant), so the graph has no node for it.
    pub references_unanchored: usize,
    /// No occurrence at that range: SCIP has nothing to say, and nothing is guessed.
    pub references_unmatched: usize,
}

/// A document with facts: repo path, the document, its facts and its occurrences by span.
type Doc<'a> = (
    String,
    &'a rb_scip::Document,
    &'a FileFacts,
    BTreeMap<Span, Vec<&'a rb_scip::Occurrence>>,
);

/// Joins `index` (paths relative to `root`) with the facts of the repository's files, keyed by
/// repo path, whose heuristic keys are `keys[path]`.
pub(crate) fn join(
    index: &ScipIndex,
    root: &str,
    files: &BTreeMap<&str, &FileFacts>,
    keys: &BTreeMap<&str, Vec<String>>,
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
    let mut out = Joined::default();
    // The documents with facts, each with its occurrences by span.
    let docs: Vec<Doc<'_>> = index
        .documents
        .iter()
        .filter_map(|doc| {
            let path = repo_path(&doc.path);
            let facts = *files.get(path.as_str())?;
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
    // Definitions first, over every document: the name's range is a definition occurrence of a
    // global symbol. The matched symbols are the only targets an edge may have, since only they
    // get `Symbol` nodes.
    for (path, _, facts, by_span) in &docs {
        let mut replaced: BTreeMap<usize, String> = BTreeMap::new();
        for (i, d) in facts.definitions.iter().enumerate() {
            out.coverage.definitions_total += 1;
            let hit = by_span.get(&d.name_span).and_then(|os| {
                os.iter()
                    .find(|o| o.is_definition() && !o.symbol.starts_with("local "))
            });
            if let Some(o) = hit {
                out.coverage.definitions_matched += 1;
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
    let mut implements: Vec<ScipEdge> = Vec::new();
    for (path, doc, facts, by_span) in &docs {
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
                out.coverage.references_external += 1;
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
        // Implementations come from symbol information, anchored at the type's name.
        let Some(replaced) = replaced else {
            continue;
        };
        for s in &doc.symbols {
            let Some((i, _)) = replaced.iter().find(|(_, sym)| **sym == s.symbol) else {
                continue;
            };
            for rel in s.relationships.iter().filter(|r| r.is_implementation) {
                if nodes.contains(rel.symbol.as_str()) {
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
    out
}

#[cfg(test)]
mod tests {
    use rb_scip::{Document, Occurrence, Relationship, SymbolInfo};
    use rb_syntax::Language;

    use super::*;

    const JAVA: &str = "package p;\nclass Animal { void sound() {} }\nclass Dog extends Animal {\n  void sound() { helper(); ext(); local(); }\n  void helper() {}\n}\n";

    /// Definition occurrences at every Tree-sitter definition name, plus references.
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
                "ext" => "scip-java maven org.ext:lib 1.0 e/Ext#ext().".to_owned(),
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
                symbols: vec![SymbolInfo {
                    symbol: sym("Dog#sound()."),
                    relationships: vec![Relationship {
                        symbol: sym("Animal#sound()."),
                        is_implementation: true,
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
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
        let files: BTreeMap<&str, &FileFacts> = BTreeMap::from([(path, &facts)]);
        let keys: BTreeMap<&str, Vec<String>> = BTreeMap::from([(path, heuristic)]);
        let joined = join(&index(&facts), "svc", &files, &keys);
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
        assert_eq!(
            edge_lines(&joined),
            [
                "scip-java maven . . p/Dog# References scip-java maven . . p/Animal#",
                "scip-java maven . . p/Dog#sound(). Calls scip-java maven . . p/Dog#helper().",
                "scip-java maven . . p/Dog#sound(). Implements scip-java maven . . p/Animal#sound().",
            ]
        );
        assert_eq!(
            joined.coverage,
            Coverage {
                definitions_matched: 5,
                definitions_total: 5,
                references_matched: 2,
                references_external: 1,
                references_local: 1,
                references_unanchored: 0,
                references_unmatched: 0,
            }
        );
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
        let files: BTreeMap<&str, &FileFacts> = BTreeMap::from([(path, &facts)]);
        let keys: BTreeMap<&str, Vec<String>> =
            BTreeMap::from([(path, rb_syntax::symbol_keys(path, &facts))]);
        let joined = join(&index, "", &files, &keys);
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
        let files: BTreeMap<&str, &FileFacts> = BTreeMap::from([(path, &facts)]);
        let keys: BTreeMap<&str, Vec<String>> = BTreeMap::from([(path, heuristic.clone())]);
        // A document for a file we have no facts for, and an index with offset ranges.
        let mut idx = index(&facts);
        idx.documents[0].path = "src/Other.java".into();
        let joined = join(&idx, "svc", &files, &keys);
        assert!(joined.keys.is_empty() && joined.edges.is_empty());
        let mut shifted = index(&facts);
        for o in &mut shifted.documents[0].occurrences {
            o.span[1] += 1;
        }
        let joined = join(&shifted, "svc", &files, &keys);
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
        let mut facts_by_path: BTreeMap<String, FileFacts> = BTreeMap::new();
        for d in &index.documents {
            let language = match d.path.rsplit('.').next() {
                Some("ts") => rb_syntax::Language::TypeScript,
                Some("tsx") => rb_syntax::Language::Tsx,
                Some("js") => rb_syntax::Language::JavaScript,
                Some("java") => rb_syntax::Language::Java,
                _ => continue,
            };
            let Ok(src) = std::fs::read(format!("{dir}/{}", d.path)) else {
                continue;
            };
            facts_by_path.insert(d.path.clone(), rb_syntax::extract(language, &src).unwrap());
            println!(
                "  {} occurrences={} symbols={} encoding_assumed={} unconverted={}",
                d.path,
                d.occurrences.len(),
                d.symbols.len(),
                d.encoding_assumed,
                d.unconverted
            );
        }
        let files: BTreeMap<&str, &FileFacts> =
            facts_by_path.iter().map(|(p, f)| (p.as_str(), f)).collect();
        let keys: BTreeMap<&str, Vec<String>> = facts_by_path
            .iter()
            .map(|(p, f)| (p.as_str(), rb_syntax::symbol_keys(p, f)))
            .collect();
        let joined = join(&index, "", &files, &keys);
        println!("coverage={:?}", joined.coverage);
        for (path, replaced) in &joined.keys {
            for (i, s) in replaced {
                println!(
                    "  key {path} {} -> {s}",
                    files[path.as_str()].definitions[*i].name
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
