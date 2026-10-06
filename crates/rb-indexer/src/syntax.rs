//! Syntax stage (design §3.1 stage 2, §4.4): choose files to parse, reuse cached facts, parse the
//! rest in parallel, and turn definitions into `Symbol` nodes.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rb_frameworks::clients::{RawScan, raw_scan};
use rb_model::{
    Certainty, Diagnostic, EdgeKind, Evidence, FileEntry, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance,
};
use rb_syntax::{FileFacts, Language};
use serde_json::json;

/// Identifies one cached extraction: the same bytes parse differently per language.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FactsKey {
    pub blob: String,
    pub language: Language,
}

impl FactsKey {
    /// The cache's `extractor` column: extractor version plus language.
    pub fn extractor(&self) -> String {
        format!("{};{}", rb_syntax::extractor_id(), self.language.as_str())
    }
}

/// Where facts from earlier runs live. The indexer never does I/O itself; the caller plugs in
/// the store (or [`NoCache`]).
pub trait FactsCache {
    fn load(&mut self, keys: &[FactsKey]) -> anyhow::Result<HashMap<FactsKey, FileFacts>>;
    fn save(&mut self, facts: &[(FactsKey, FileFacts)]) -> anyhow::Result<()>;
}

pub struct NoCache;

impl FactsCache for NoCache {
    fn load(&mut self, _: &[FactsKey]) -> anyhow::Result<HashMap<FactsKey, FileFacts>> {
        Ok(HashMap::new())
    }

    fn save(&mut self, _: &[(FactsKey, FileFacts)]) -> anyhow::Result<()> {
        Ok(())
    }
}

/// Files worth parsing, with their language; skipped files that deserve a diagnostic.
///
/// Binary and generated files and languages without a grammar are skipped silently (their `File`
/// node already says why). Oversized files are reported, since they are expected to be source.
pub(crate) fn parse_plan(files: &[FileEntry]) -> (Vec<(&FileEntry, Language)>, Vec<Diagnostic>) {
    let mut plan = Vec::new();
    let mut diagnostics = Vec::new();
    for f in files {
        let Some(language) = f.lang.and_then(Language::from_lang) else {
            continue;
        };
        if f.binary || f.generated {
            continue;
        }
        if f.size > rb_syntax::MAX_PARSE_BYTES {
            diagnostics.push(Diagnostic {
                kind: "extraction_skipped".into(),
                path: Some(f.path.clone()),
                span: None,
                detail: json!({"reason": "too_large", "size": f.size}),
            });
            continue;
        }
        plan.push((f, language));
    }
    (plan, diagnostics)
}

/// Bytes of a file without facts that the raw-text scan reads (contract M4a amendment 17): a
/// larger file is not read and taints every kind it would be scanned for, so the cap can only
/// over-taint.
pub(crate) const RAW_SCAN_BYTES: u64 = 4 * 1024 * 1024;

/// The raw-text scan ([`rb_frameworks::clients::raw_scan`]) of every file the plan does not
/// parse — a language with no grammar, a generated file, one too large to parse — except
/// binary blobs (a NUL byte in the first 8000, as Git decides); the blob is read as lossy
/// UTF-8. Returns the merged findings and the number of files scanned.
pub(crate) fn scan_unparsed(
    files: &[FileEntry],
    plan: &[(&FileEntry, Language)],
    blob: &dyn Fn(&str) -> anyhow::Result<Vec<u8>>,
) -> anyhow::Result<(RawScan, usize)> {
    let parsed: BTreeSet<&str> = plan.iter().map(|(f, _)| f.path.as_str()).collect();
    let mut out = RawScan::default();
    let mut scanned = 0;
    for f in files
        .iter()
        .filter(|f| !f.binary && !parsed.contains(f.path.as_str()))
    {
        scanned += 1;
        let found = if f.size > RAW_SCAN_BYTES {
            raw_scan(&f.path, None)
        } else {
            raw_scan(&f.path, Some(&String::from_utf8_lossy(&blob(&f.blob_sha)?)))
        };
        out.merge(found);
    }
    Ok((out, scanned))
}

/// Parses every job, spreading work over `threads`. Output order equals input order.
pub(crate) fn parse_parallel(
    jobs: &[(Language, Vec<u8>)],
    threads: usize,
) -> anyhow::Result<Vec<FileFacts>> {
    if jobs.is_empty() {
        return Ok(Vec::new());
    }
    let chunk = jobs.len().div_ceil(threads.max(1));
    std::thread::scope(|scope| {
        let workers: Vec<_> = jobs
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|(language, source)| rb_syntax::extract(*language, source))
                        .collect::<anyhow::Result<Vec<_>>>()
                })
            })
            .collect();
        let mut out = Vec::with_capacity(jobs.len());
        for worker in workers {
            let facts = worker
                .join()
                .map_err(|_| anyhow::anyhow!("a parser thread panicked"))??;
            out.extend(facts);
        }
        Ok(out)
    })
}

/// Adds the file's symbols: `Symbol` nodes, `DEFINED_IN` (evidence: name span) and `CONTAINS`.
/// `keys` is parallel to `facts.definitions` (heuristic keys, or SCIP keys where the join matched).
///
/// Declarations that share a key (e.g. TypeScript declaration merging) are one symbol: the first
/// declaration's kind wins and every site becomes `DEFINED_IN` evidence.
pub(crate) fn add_symbols(
    b: &mut GraphBuilder,
    path: &str,
    facts: &FileFacts,
    keys: &[String],
) -> anyhow::Result<()> {
    let file = NodeKey::new(NodeKind::File, path);
    let keys: Vec<NodeKey> = keys
        .iter()
        .map(|k| NodeKey::new(NodeKind::Symbol, k.clone()))
        .collect();
    for (def, key) in facts.definitions.iter().zip(&keys) {
        if !b.has_node(key) {
            b.add_node(
                key.clone(),
                &def.name,
                object(json!({"kind": def.kind.as_str()})),
            )?;
        }
        let at = Evidence {
            path: path.to_owned(),
            span: def.name_span,
        };
        b.add_edge(
            NewEdge::new(
                key.clone(),
                EdgeKind::DefinedIn,
                file.clone(),
                Provenance::Treesitter,
                Certainty::Exact,
            )
            .evidence(at.clone()),
        )?;
        if let Some(parent) = def.parent {
            b.add_edge(
                NewEdge::new(
                    keys[parent].clone(),
                    EdgeKind::Contains,
                    key.clone(),
                    Provenance::Treesitter,
                    Certainty::Exact,
                )
                .evidence(at),
            )?;
        }
    }
    Ok(())
}

/// Facts for every planned file: cache hits first, then the misses parsed (in batches, so memory
/// stays bounded) and saved. Returns the facts and how many blobs were actually parsed.
pub(crate) fn load_or_parse(
    read_blob: &dyn Fn(&str) -> anyhow::Result<Vec<u8>>,
    plan: &[(&FileEntry, Language)],
    cache: &mut dyn FactsCache,
    threads: usize,
) -> anyhow::Result<(HashMap<FactsKey, FileFacts>, usize)> {
    let keys: Vec<FactsKey> = plan
        .iter()
        .map(|(f, language)| FactsKey {
            blob: f.blob_sha.clone(),
            language: *language,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut facts = cache.load(&keys)?;
    let missing: Vec<&FactsKey> = keys.iter().filter(|k| !facts.contains_key(k)).collect();
    for batch in missing.chunks(PARSE_BATCH) {
        let jobs = batch
            .iter()
            .map(|k| Ok((k.language, read_blob(&k.blob)?)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        let parsed: Vec<(FactsKey, FileFacts)> = batch
            .iter()
            .map(|k| (*k).clone())
            .zip(parse_parallel(&jobs, threads)?)
            .collect();
        cache.save(&parsed)?;
        facts.extend(parsed);
    }
    Ok((facts, missing.len()))
}

/// Blobs read and parsed per round; bounds memory on large repositories.
const PARSE_BATCH: usize = 512;

/// Per parsed file: `syntax_error` if the tree had errors, `definitions_too_deep` if nesting was
/// truncated at `rb_syntax::MAX_NESTING`.
pub(crate) fn syntax_diagnostics(
    facts_by_path: &BTreeMap<&str, (Language, &FileFacts)>,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for (path, (_, facts)) in facts_by_path {
        if let Some(span) = facts.syntax_error {
            out.push(Diagnostic {
                kind: "syntax_error".into(),
                path: Some((*path).to_owned()),
                span: Some(span),
                detail: json!({}),
            });
        }
        if facts.nesting_truncated {
            out.push(Diagnostic {
                kind: "definitions_too_deep".into(),
                path: Some((*path).to_owned()),
                span: None,
                detail: json!({"max_nesting": rb_syntax::MAX_NESTING}),
            });
        }
    }
    out
}

fn object(v: serde_json::Value) -> rb_model::Attrs {
    match v {
        serde_json::Value::Object(m) => m,
        _ => unreachable!("object() is only called with json!({{..}}) literals"),
    }
}

#[cfg(test)]
mod tests {
    use rb_frameworks::clients::ClientKind;

    use super::*;

    fn entry(path: &str, size: u64) -> FileEntry {
        FileEntry {
            path: path.into(),
            blob_sha: format!("{:0>40}", path.len()),
            size,
            executable: false,
            lang: rb_git::language_for_path(path),
            binary: false,
            generated: false,
        }
    }

    #[test]
    fn plan_skips_unsupported_binary_generated_and_oversized_files() {
        let mut binary = entry("img.ts", 10);
        binary.binary = true;
        let mut generated = entry("gen.ts", 10);
        generated.generated = true;
        let files = vec![
            entry("a.ts", 10),
            entry("README.md", 10),
            binary,
            generated,
            entry("huge.java", rb_syntax::MAX_PARSE_BYTES + 1),
            entry("b.py", 10),
        ];
        let (plan, diagnostics) = parse_plan(&files);
        let planned: Vec<(&str, Language)> =
            plan.iter().map(|(f, l)| (f.path.as_str(), *l)).collect();
        assert_eq!(
            planned,
            [("a.ts", Language::TypeScript), ("b.py", Language::Python)]
        );
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, "extraction_skipped");
        assert_eq!(diagnostics[0].path.as_deref(), Some("huge.java"));
        assert_eq!(diagnostics[0].detail["reason"], "too_large");
    }

    /// Contract M4a amendment 17 (evaluator pass 7, W1, W2): every non-binary file the plan
    /// does not parse (no grammar, generated, too large to parse) is scanned as text; parsed
    /// and binary files are not; a file over the scan's byte cap is not read and taints every
    /// kind it is scanned for, and discovery.
    #[test]
    fn files_without_facts_are_scanned_as_text() {
        let file = |path: &str, size: u64| FileEntry {
            blob_sha: path.into(),
            ..entry(path, size)
        };
        let mut binary = file("img.png", 10);
        binary.binary = true;
        let mut generated = file("gen.go", 10);
        generated.generated = true;
        let mut files = vec![
            file("a.ts", 10),
            file("App.vue", 10),
            binary,
            file("big.ts", rb_syntax::MAX_PARSE_BYTES + 1),
            generated,
            file("x.py", 10),
        ];
        let blobs: HashMap<&str, &[u8]> = HashMap::from([
            (
                "a.ts",
                &b"export const label = \"RequestInterceptor\";\n"[..],
            ),
            ("App.vue", b"<script>import axios from 'axios'</script>\n"),
            ("img.png", b"\0RequestInterceptor fetch"),
            ("big.ts", b"globalThis.fetch = f;\n"),
            (
                "gen.go",
                b"package main\nfunc init() { http.DefaultTransport = nil }\n",
            ),
            ("x.py", b"RequestInterceptor = 1\n"),
        ]);
        let blob = |sha: &str| {
            blobs
                .get(sha)
                .map(|b| b.to_vec())
                .ok_or_else(|| anyhow::anyhow!("{sha} must not be read"))
        };
        let scan = |files: &[FileEntry]| {
            let (plan, _) = parse_plan(files);
            scan_unparsed(files, &plan, &blob).unwrap()
        };
        let (found, scanned) = scan(&files);
        assert_eq!(
            found.hooks,
            BTreeSet::from([ClientKind::Fetch, ClientKind::Axios, ClientKind::GoHttp])
        );
        assert!(!found.discovery);
        assert_eq!(scanned, 3);
        files.push(file("vendor.js", RAW_SCAN_BYTES + 1));
        let (found, scanned) = scan(&files);
        assert_eq!(
            found.hooks,
            BTreeSet::from([
                ClientKind::Fetch,
                ClientKind::Axios,
                ClientKind::Feign,
                ClientKind::GoHttp
            ])
        );
        assert!(found.discovery);
        assert_eq!(scanned, 4);
    }

    #[test]
    fn parallel_parsing_equals_serial_and_keeps_order() {
        let jobs: Vec<(Language, Vec<u8>)> = (0..40)
            .map(|i| {
                let lang = [Language::Python, Language::Go, Language::Java][i % 3];
                let src = match lang {
                    Language::Python => format!("def f{i}():\n    g{i}()\n"),
                    Language::Go => format!("package p\nfunc F{i}() {{ G{i}() }}\n"),
                    _ => format!("class C{i} {{ void m() {{ n{i}(); }} }}\n"),
                };
                (lang, src.into_bytes())
            })
            .collect();
        let serial = parse_parallel(&jobs, 1).unwrap();
        assert_eq!(serial.len(), 40);
        assert_eq!(serial[3].definitions[0].name, "f3");
        assert_eq!(parse_parallel(&jobs, 4).unwrap(), serial);
        assert_eq!(parse_parallel(&jobs, 64).unwrap(), serial);
    }

    #[test]
    fn symbols_defined_in_and_contains() {
        let src = "class A:\n    def m(self):\n        pass\n";
        let facts = rb_syntax::extract(Language::Python, src.as_bytes()).unwrap();
        let mut b = GraphBuilder::new();
        b.add_node(
            rb_model::NodeKey::new(NodeKind::File, "a.py"),
            "a.py",
            rb_model::Attrs::new(),
        )
        .unwrap();
        add_symbols(
            &mut b,
            "a.py",
            &facts,
            &rb_syntax::symbol_keys("a.py", &facts),
        )
        .unwrap();
        let g = b.finish().unwrap();

        let key = |id: i64| g.nodes().iter().find(|n| n.id == id).unwrap().key.clone();
        let edges: Vec<(String, EdgeKind, String, Provenance, Certainty)> = g
            .edges()
            .iter()
            .map(|e| (key(e.src), e.kind, key(e.dst), e.provenance, e.certainty))
            .collect();
        let class = "rb-heuristic . . . `a.py`/A#".to_owned();
        let method = "rb-heuristic . . . `a.py`/A#m().".to_owned();
        assert_eq!(
            edges,
            [
                (
                    class.clone(),
                    EdgeKind::DefinedIn,
                    "a.py".into(),
                    Provenance::Treesitter,
                    Certainty::Exact
                ),
                (
                    class.clone(),
                    EdgeKind::Contains,
                    method.clone(),
                    Provenance::Treesitter,
                    Certainty::Exact
                ),
                (
                    method.clone(),
                    EdgeKind::DefinedIn,
                    "a.py".into(),
                    Provenance::Treesitter,
                    Certainty::Exact
                ),
            ]
        );
        let m = g.nodes().iter().find(|n| n.key == method).unwrap();
        assert_eq!((m.name.as_str(), &m.attrs["kind"]), ("m", &"method".into()));
        let defined_in = g
            .edges()
            .iter()
            .find(|e| e.kind == EdgeKind::DefinedIn && key(e.src) == method)
            .unwrap();
        assert_eq!(defined_in.evidence[0].span, [1, 8, 1, 9]);
    }

    #[test]
    fn merged_declarations_become_one_symbol_with_two_sites() {
        let src = "class X {}\ninterface X { a: string }\n";
        let facts = rb_syntax::extract(Language::TypeScript, src.as_bytes()).unwrap();
        let mut b = GraphBuilder::new();
        b.add_node(
            rb_model::NodeKey::new(NodeKind::File, "x.ts"),
            "x.ts",
            rb_model::Attrs::new(),
        )
        .unwrap();
        add_symbols(
            &mut b,
            "x.ts",
            &facts,
            &rb_syntax::symbol_keys("x.ts", &facts),
        )
        .unwrap();
        let g = b.finish().unwrap();
        let symbols: Vec<&rb_model::Node> = g
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::Symbol)
            .collect();
        assert_eq!(symbols.len(), 1);
        assert_eq!(
            symbols[0].attrs["kind"], "class",
            "first declaration's kind wins"
        );
        let defined_in = g
            .edges()
            .iter()
            .find(|e| e.kind == EdgeKind::DefinedIn)
            .unwrap();
        assert_eq!(defined_in.evidence.len(), 2);
    }

    /// An in-memory cache that records how it was used.
    #[derive(Default)]
    struct MemCache {
        rows: HashMap<FactsKey, FileFacts>,
        saves: usize,
    }

    impl FactsCache for MemCache {
        fn load(&mut self, keys: &[FactsKey]) -> anyhow::Result<HashMap<FactsKey, FileFacts>> {
            Ok(keys
                .iter()
                .filter_map(|k| self.rows.get(k).map(|f| (k.clone(), f.clone())))
                .collect())
        }

        fn save(&mut self, facts: &[(FactsKey, FileFacts)]) -> anyhow::Result<()> {
            self.saves += facts.len();
            self.rows.extend(facts.iter().cloned());
            Ok(())
        }
    }

    #[test]
    fn warm_cache_parses_nothing_and_yields_identical_facts() {
        let mut same_as_a = entry("copy.py", 10);
        let a = entry("a.py", 10);
        same_as_a.blob_sha = a.blob_sha.clone(); // identical content at two paths
        let files = vec![a, same_as_a, entry("b.go", 10), entry("b.js", 10)];
        let sources: HashMap<String, Vec<u8>> = files
            .iter()
            .map(|f| (f.blob_sha.clone(), b"def f():\n    pass\n".to_vec()))
            .collect();
        let read = |blob: &str| Ok(sources[blob].clone());
        let (plan, _) = parse_plan(&files);

        let mut cache = MemCache::default();
        let (cold, parsed) = load_or_parse(&read, &plan, &mut cache, 2).unwrap();
        // The helper gives all four files one blob sha. a.py and copy.py are the same
        // (blob, language) and parse once; b.go and b.js add two more languages.
        assert_eq!(parsed, 3);
        assert_eq!(cache.saves, 3);

        let (warm, parsed) = load_or_parse(&read, &plan, &mut cache, 2).unwrap();
        assert_eq!(parsed, 0);
        assert_eq!(warm, cold);
    }

    #[test]
    fn cache_keys_carry_extractor_version_and_language() {
        let key = FactsKey {
            blob: "b".repeat(40),
            language: Language::Tsx,
        };
        assert_eq!(
            key.extractor(),
            format!("{};tsx", rb_syntax::extractor_id())
        );
        let other = FactsKey {
            language: Language::TypeScript,
            ..key.clone()
        };
        assert_ne!(key.extractor(), other.extractor());
    }

    #[test]
    fn syntax_errors_become_diagnostics() {
        let broken =
            rb_syntax::extract(Language::Python, b"def ok():\n    pass\ndef bad(:\n").unwrap();
        let clean = rb_syntax::extract(Language::Python, b"x = 1\n").unwrap();
        let by_path: BTreeMap<&str, (Language, &FileFacts)> = [
            ("bad.py", (Language::Python, &broken)),
            ("ok.py", (Language::Python, &clean)),
        ]
        .into_iter()
        .collect();
        let diagnostics = syntax_diagnostics(&by_path);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, "syntax_error");
        assert_eq!(diagnostics[0].path.as_deref(), Some("bad.py"));
        assert_eq!(diagnostics[0].span.map(|s| s[0]), Some(2));

        let depth = rb_syntax::MAX_NESTING + 1;
        let src = format!("{}{}", "function a(){".repeat(depth), "}".repeat(depth));
        let truncated = rb_syntax::extract(Language::JavaScript, src.as_bytes()).unwrap();
        let by_path: BTreeMap<&str, (Language, &FileFacts)> =
            [("deep.js", (Language::JavaScript, &truncated))]
                .into_iter()
                .collect();
        let kinds: Vec<String> = syntax_diagnostics(&by_path)
            .into_iter()
            .map(|d| d.kind)
            .collect();
        assert_eq!(kinds, ["definitions_too_deep"]);
    }
}
