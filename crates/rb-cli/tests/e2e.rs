//! End-to-end pipeline tests (design §12.2, §13): run the real `rb` binary against Postgres + AGE.
//!
//! Requires the compose database: `docker compose up -d --wait`, then
//! `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55432/rb cargo test`.
//! Without `RB_TEST_DATABASE_URL` these tests print a notice and pass vacuously. Every index run
//! passes `--no-scip` so goldens stay heuristic and Docker-free, except the `scip_*` tests,
//! which run the real indexer containers and skip themselves when Docker (or the local
//! scip-java image) is unavailable.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use std::collections::{BTreeMap, HashMap, HashSet};

use common::*;

use rb_api::Settings;
use rb_model::{Certainty, EdgeKind, Graph, NodeKind, Provenance, canonical_dump};
use rb_store::Store;

fn check_fixture(fixture: &str, tag: &str) {
    let Some(url) = db_url() else { return };
    let repo = fixture_repo(fixture);
    let path = repo.path().to_str().unwrap();
    let pid = std::process::id();
    let (a, b) = (format!("t_{tag}_{pid}_a"), format!("t_{tag}_{pid}_b"));
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![a.clone(), b.clone()],
    };

    // Determinism: the same commit indexed into two repos, and rebuilt, dumps byte-identically.
    rb(&url, &["index", path, "--repo", &a, "--no-scip"]);
    rb(&url, &["index", path, "--repo", &b, "--no-scip"]);
    let dump_a = rb(&url, &["dump", "--repo", &a]);
    let dump_b = rb(&url, &["dump", "--repo", &b]);
    assert_eq!(dump_a, dump_b, "same commit, different repos");

    assert!(rb(&url, &["index", path, "--repo", &a, "--no-scip"]).contains("up to date"));
    rb(&url, &["index", path, "--repo", &a, "--no-scip", "--force"]);
    assert_eq!(rb(&url, &["dump", "--repo", &a]), dump_a, "forced rebuild");

    // The stored snapshot is faithful to the in-memory pipeline output.
    let indexed = rb_indexer::index_commit(
        repo.path(),
        "HEAD",
        &mut rb_indexer::NoCache,
        &rb_indexer::ScipOptions::disabled(),
        &mut rb_indexer::NoScipCache,
    )
    .unwrap();
    assert_eq!(
        canonical_dump(&indexed.commit_sha, &indexed.graph, &indexed.diagnostics),
        dump_a,
        "store round trip"
    );
    let graph = &indexed.graph;
    let node = |id: i64| graph.nodes().iter().find(|n| n.id == id).unwrap();

    // Contract M4a S23: the fixture's precision guards, and the HTTP invariants every dump keeps.
    // Both are collected before asserting, so a regression shows every line and rule it breaks.
    let mut failures = guard_failures(fixture, &dump_a);
    failures.extend(http_invariant_failures(graph));
    failures.extend(m4b_invariant_failures(graph));
    assert!(
        failures.is_empty(),
        "precision guards and invariants of {fixture}:\n{}",
        failures.join("\n")
    );

    // The fixture commit holds the tracked files and nothing else (amendment 40).
    let mut files: Vec<String> = graph
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .map(|n| n.key.clone())
        .collect();
    files.sort();
    let mut tracked = tracked_files(fixture);
    tracked.sort();
    assert_eq!(files, tracked, "the fixture repository's files");

    // Invariant: every File has exactly one IN_SERVICE edge.
    for f in graph.nodes().iter().filter(|n| n.kind == NodeKind::File) {
        let memberships = graph
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::InService && e.src == f.id)
            .count();
        assert_eq!(
            memberships, 1,
            "{} has {memberships} IN_SERVICE edges",
            f.key
        );
    }

    // M0 exit criterion: `rb cypher` returns files per service.
    let per_service = rb(
        &url,
        &[
            "cypher",
            "MATCH (f:File)-[:IN_SERVICE]->(s:Service) RETURN s.key, count(f)",
            "--repo",
            &a,
            "--cols",
            "service,files",
            "--timeout-ms",
            "30000",
        ],
    );
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for e in graph
        .edges()
        .iter()
        .filter(|e| e.kind == EdgeKind::InService)
    {
        *counts.entry(node(e.dst).key.as_str()).or_default() += 1;
    }
    assert!(!counts.is_empty());
    assert_eq!(
        sorted_rows(&per_service),
        counts
            .iter()
            .map(|(k, n)| format!("{k}\t{n}"))
            .collect::<Vec<_>>()
    );

    // Contract M5 S6 (design §12.2): the projection holds exactly the snapshot's vertices and,
    // as a multiset, exactly its edges with their provenance, certainty and `ops`.
    let failures = age_parity_failures(&url, &a, graph);
    assert!(failures.is_empty(), "AGE parity:\n{}", failures.join("\n"));

    // Contract M4a S26: `Endpoint` vertices, and every endpoint's 1-hop neighbours in each
    // direction (`EXPOSES` in, `HANDLED_BY` out, `CONSUMES` in), equal the relational ones.
    let mut endpoints: Vec<String> = graph
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::Endpoint)
        .map(|n| n.key.clone())
        .collect();
    endpoints.sort();
    let cypher = |query: &str, cols: &str| {
        sorted_rows(&rb(
            &url,
            &[
                "cypher",
                query,
                "--repo",
                &a,
                "--cols",
                cols,
                "--timeout-ms",
                "30000",
            ],
        ))
    };
    assert!(!endpoints.is_empty());
    for (outbound, query) in [
        (
            true,
            "MATCH (e:Endpoint)-[r]->(n) RETURN e.key, label(r), n.key",
        ),
        (
            false,
            "MATCH (n)-[r]->(e:Endpoint) RETURN e.key, label(r), n.key",
        ),
    ] {
        let mut expected: Vec<String> = graph
            .edges()
            .iter()
            .filter_map(|e| {
                let (end, other) = if outbound {
                    (node(e.src), node(e.dst))
                } else {
                    (node(e.dst), node(e.src))
                };
                (end.kind == NodeKind::Endpoint)
                    .then(|| format!("{}\t{}\t{}", end.key, e.kind, other.key))
            })
            .collect();
        expected.sort();
        assert_eq!(
            cypher(query, "endpoint,kind,neighbour"),
            expected,
            "{query}"
        );
    }

    // Contract M4b S26: the projection's `INJECTS` edges and Service -> Service `DEPENDS_ON`
    // edges, read through their labels, equal the relational ones (each end's 1-hop neighbours
    // over that label). The Java fixtures have injections, the monorepo a `via=package` edge.
    for (kind, src_kind, dst_kind, query) in [
        (
            EdgeKind::Injects,
            NodeKind::Symbol,
            NodeKind::Symbol,
            "MATCH (a:Symbol)-[r:INJECTS]->(b:Symbol) RETURN a.key, b.key, r.provenance, r.certainty",
        ),
        (
            EdgeKind::DependsOn,
            NodeKind::Service,
            NodeKind::Service,
            "MATCH (a:Service)-[r:DEPENDS_ON]->(b:Service) RETURN a.key, b.key, r.provenance, r.certainty",
        ),
    ] {
        let mut expected: Vec<String> = graph
            .edges()
            .iter()
            .filter(|e| {
                e.kind == kind && node(e.src).kind == src_kind && node(e.dst).kind == dst_kind
            })
            .map(|e| {
                format!(
                    "{}\t{}\t{}\t{}",
                    node(e.src).key,
                    node(e.dst).key,
                    e.provenance,
                    e.certainty.rank()
                )
            })
            .collect();
        expected.sort();
        let produced = match kind {
            EdgeKind::Injects => fixture != "ts-express-prisma",
            _ => fixture == "polyglot-monorepo",
        };
        assert_eq!(!expected.is_empty(), produced, "{kind} edges in {fixture}");
        assert_eq!(
            cypher(query, "src,dst,provenance,certainty"),
            expected,
            "{query}"
        );
    }

    // Cypher runs read-only: writes are rejected.
    let out = Command::new(env!("CARGO_BIN_EXE_rb"))
        .args([
            "cypher",
            "CREATE (n:File {key: 'x'}) RETURN n",
            "--repo",
            &a,
        ])
        .env("RB_DATABASE_URL", &url)
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "a CREATE through rb cypher must fail"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("read-only"));
}

/// Contract M5 S6: the differences between the repository's live AGE graph and `graph`, the
/// snapshot it projects: the whole vertex set `(label, properties)` and the whole edge multiset
/// `(label, start nid, end nid, properties)`, each property map compared as JSON, types included
/// (`nid` and `certainty` numbers, `ops` the relational array, the loader's `__id__` equal to
/// `nid`, and no other property). Empty when they agree.
fn age_parity_failures(url: &str, repo: &str, graph: &Graph) -> Vec<String> {
    use serde_json::{Value, json};
    let query = |cypher: &str, cols: &str| -> Vec<Vec<String>> {
        let out = rb(
            url,
            &[
                "cypher",
                cypher,
                "--repo",
                repo,
                "--cols",
                cols,
                "--timeout-ms",
                "30000",
            ],
        );
        out.lines()
            .skip(1)
            .map(|l| {
                let mut row: Vec<String> = l.split('\t').map(str::to_owned).collect();
                // The property map, normalised (sorted keys, compact).
                if let Some(props) = row.last_mut() {
                    *props = serde_json::from_str::<Value>(props)
                        .map_or_else(|_| format!("unparsed {props}"), |v| v.to_string());
                }
                row
            })
            .collect()
    };
    let mut failures = Vec::new();
    let mut compare = |what: &str, mut actual: Vec<Vec<String>>, mut expected: Vec<Vec<String>>| {
        actual.sort();
        expected.sort();
        if actual != expected {
            let missing: Vec<_> = expected.iter().filter(|r| !actual.contains(r)).collect();
            let extra: Vec<_> = actual.iter().filter(|r| !expected.contains(r)).collect();
            failures.push(format!(
                "{what}: {} projected, {} relational; missing {missing:?}; extra {extra:?}",
                actual.len(),
                expected.len()
            ));
        }
    };
    let expected: Vec<Vec<String>> = graph
        .nodes()
        .iter()
        .map(|n| {
            vec![
                n.kind.age_label().to_owned(),
                // `__id__`: AGE 1.8's CSV loader writes the id column's value there too.
                json!({"nid": n.id, "key": n.key, "name": n.name, "__id__": n.id}).to_string(),
            ]
        })
        .collect();
    compare(
        "vertices",
        query(
            "MATCH (n) RETURN label(n), properties(n)",
            "label,properties",
        ),
        expected,
    );
    let expected: Vec<Vec<String>> = graph
        .edges()
        .iter()
        .map(|e| {
            let mut props = json!({
                "provenance": e.provenance.as_str(),
                "certainty": e.certainty.rank(),
            });
            if let Some(ops) = e.attrs.get("ops") {
                props["ops"] = ops.clone();
            }
            vec![
                e.kind.to_string(),
                e.src.to_string(),
                e.dst.to_string(),
                props.to_string(),
            ]
        })
        .collect();
    compare(
        "edges",
        query(
            "MATCH (a)-[e]->(b) RETURN label(e), a.nid, b.nid, properties(e)",
            "label,src,dst,properties",
        ),
        expected,
    );
    failures
}

/// The certainty names, weakest first (`rb_model::Certainty` ranks 1–5).
const CERTAINTIES: [&str; 5] = ["ambiguous", "heuristic", "synthetic", "dispatch", "exact"];

/// One precision-guard line (contract M4a D10, S23): `<KIND> <src key> -> <dst key>
/// [<certainty>] {<attributes>}`, with `*` for any key and the last two parts optional.
#[derive(Debug)]
struct Guard {
    line: String,
    kind: String,
    src: String,
    dst: String,
    certainty: Option<String>,
    attrs: serde_json::Map<String, serde_json::Value>,
}

/// An edge row of a dump: kind, source key, target key, certainty name and attributes.
struct DumpEdge {
    kind: String,
    src: String,
    dst: String,
    certainty: String,
    attrs: serde_json::Value,
}

impl Guard {
    fn matches(&self, e: &DumpEdge) -> bool {
        self.kind == e.kind
            && (self.src == "*" || self.src == e.src)
            && (self.dst == "*" || self.dst == e.dst)
            && self.certainty.as_ref().is_none_or(|c| *c == e.certainty)
            && self.attrs.iter().all(|(k, v)| e.attrs.get(k) == Some(v))
    }
}

fn parse_guards(fixture: &str, file: &str) -> Vec<Guard> {
    let path = golden_path(fixture, file);
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("precision guards {}: {e}", path.display()));
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .map(|line| {
            let bad = || panic!("{}: unreadable guard {line:?}", path.display());
            let (kind, rest) = line.split_once(' ').unwrap_or_else(bad);
            let (src, mut dst) = rest.split_once(" -> ").unwrap_or_else(bad);
            let mut attrs = serde_json::Map::new();
            // Keys never contain `{"` (an endpoint path's parameter is `{}`).
            if let Some(i) = dst.find(" {\"") {
                attrs = serde_json::from_str(&dst[i + 1..]).unwrap_or_else(|e| {
                    panic!("{}: unreadable attributes in {line:?}: {e}", path.display())
                });
                dst = &dst[..i];
            }
            let mut certainty = None;
            for c in CERTAINTIES {
                if let Some(d) = dst.strip_suffix(&format!(" [{c}]")) {
                    certainty = Some(c.to_owned());
                    dst = d;
                }
            }
            Guard {
                line: line.to_owned(),
                kind: kind.to_owned(),
                src: src.to_owned(),
                dst: dst.to_owned(),
                certainty,
                attrs,
            }
        })
        .collect()
}

/// The guard lines `dump` fails: every `must-exist.txt` line must match an edge of `dump`, and
/// no `must-not-exist.txt` line may; a must-not-exist line's source key (unless `*`) must name a
/// node of the dump, so a misspelt key cannot make a line pass vacuously.
fn guard_failures(fixture: &str, dump: &str) -> Vec<String> {
    let mut nodes = HashSet::new();
    let mut edges = Vec::new();
    for line in dump.lines().skip(1) {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        let text = |i: usize| row[i].as_str().unwrap_or_default().to_owned();
        match row[0].as_str() {
            Some("N") => {
                nodes.insert(text(2));
            }
            Some("E") => {
                let rank = row[7].as_i64().unwrap();
                edges.push(DumpEdge {
                    kind: text(3),
                    src: text(2),
                    dst: text(5),
                    certainty: CERTAINTIES[usize::try_from(rank - 1).unwrap()].to_owned(),
                    attrs: row[8].clone(),
                });
            }
            _ => {}
        }
    }
    let mut failures = Vec::new();
    for g in parse_guards(fixture, "must-exist.txt") {
        if !edges.iter().any(|e| g.matches(e)) {
            failures.push(format!("missing: {}", g.line));
        }
    }
    for g in parse_guards(fixture, "must-not-exist.txt") {
        if g.src != "*" && !nodes.contains(&g.src) {
            failures.push(format!("vacuous (no node {:?}): {}", g.src, g.line));
        }
        for e in edges.iter().filter(|e| g.matches(e)) {
            failures.push(format!(
                "forbidden: {}\n    found {} {} -> {} [{}] {}",
                g.line, e.kind, e.src, e.dst, e.certainty, e.attrs
            ));
        }
    }
    failures
}

/// Contract M4a invariants 1, 3, 5 and 12 over a fixture's graph: `http_stitch` gives only
/// `CONSUMES`, never above `synthetic`; a `CONSUMES` source lies in another service than the
/// endpoint; every endpoint has exactly one `EXPOSES`, from the service its key names; every
/// `EXPOSES`, `HANDLED_BY` and `CONSUMES` edge has evidence at a `File`. Returns the violations.
fn http_invariant_failures(graph: &Graph) -> Vec<String> {
    let mut failures = Vec::new();
    let by_id: HashMap<i64, &rb_model::Node> = graph.nodes().iter().map(|n| (n.id, n)).collect();
    let files: HashSet<&str> = graph
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .map(|n| n.key.as_str())
        .collect();
    let out_edge = |src: i64, kind: EdgeKind| {
        graph
            .edges()
            .iter()
            .find(|e| e.src == src && e.kind == kind)
            .map(|e| e.dst)
    };
    // The service of a symbol (through its file) or of a file.
    let service_of = |id: i64| {
        let file = if by_id[&id].kind == NodeKind::File {
            Some(id)
        } else {
            out_edge(id, EdgeKind::DefinedIn)
        };
        file.and_then(|f| out_edge(f, EdgeKind::InService))
            .map(|s| by_id[&s].key.as_str())
    };
    let mut exposes: HashMap<i64, Vec<i64>> = HashMap::new();
    for e in graph.edges() {
        let (src, dst) = (by_id[&e.src], by_id[&e.dst]);
        let what = format!("{} {} -> {}", e.kind, src.key, dst.key);
        if e.provenance == Provenance::HttpStitch || e.kind == EdgeKind::Consumes {
            if !(e.kind == EdgeKind::Consumes
                && e.provenance == Provenance::HttpStitch
                && e.certainty <= Certainty::Synthetic)
            {
                failures.push(format!(
                    "invariant 3: {what}: {:?} {:?}",
                    e.provenance, e.certainty
                ));
            }
            let own = service_of(e.src);
            if own.is_none() {
                failures.push(format!("invariant 5: {what}: the client has no service"));
            } else if own == dst.attrs["service"].as_str() {
                failures.push(format!(
                    "invariant 5: {what}: a client consumes its own service"
                ));
            }
        }
        if matches!(
            e.kind,
            EdgeKind::Exposes | EdgeKind::HandledBy | EdgeKind::Consumes
        ) && (e.evidence.is_empty()
            || !e.evidence.iter().all(|ev| files.contains(ev.path.as_str())))
        {
            failures.push(format!("invariant 12: {what}: evidence {:?}", e.evidence));
        }
        if e.kind == EdgeKind::Exposes {
            exposes.entry(e.dst).or_default().push(e.src);
        }
    }
    for n in graph
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::Endpoint)
    {
        let from: Vec<&str> = exposes
            .get(&n.id)
            .map(|v| v.iter().map(|s| by_id[s].key.as_str()).collect())
            .unwrap_or_default();
        if from != [n.attrs["service"].as_str().unwrap_or_default()] {
            failures.push(format!("invariant 1: {}: EXPOSES from {from:?}", n.key));
        }
    }
    failures
}

/// Contract M4b invariants 2, 10 and S24 over a fixture's graph: every `INJECTS` has provenance
/// `di`, joins two `Symbol`s, is `heuristic` or `ambiguous` (F2: never `synthetic`, so never
/// above it) and has evidence at a `File`; every `manifest` `DEPENDS_ON` has evidence at a
/// `File`; `impl_instantiated` appears only on `IMPLEMENTS` and only as `false`. Returns the
/// violations.
fn m4b_invariant_failures(graph: &Graph) -> Vec<String> {
    let by_id: HashMap<i64, &rb_model::Node> = graph.nodes().iter().map(|n| (n.id, n)).collect();
    let files: HashSet<&str> = graph
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::File)
        .map(|n| n.key.as_str())
        .collect();
    let mut failures = Vec::new();
    for e in graph.edges() {
        let (src, dst) = (by_id[&e.src], by_id[&e.dst]);
        let what = format!("{} {} -> {}", e.kind, src.key, dst.key);
        let evidenced =
            !e.evidence.is_empty() && e.evidence.iter().all(|ev| files.contains(ev.path.as_str()));
        if e.kind == EdgeKind::Injects {
            if e.provenance != Provenance::Di || e.certainty > Certainty::Heuristic {
                failures.push(format!(
                    "M4b invariant 2: {what}: {:?} {:?}",
                    e.provenance, e.certainty
                ));
            }
            if src.kind != NodeKind::Symbol || dst.kind != NodeKind::Symbol {
                failures.push(format!("M4b invariant 2: {what}: not Symbol to Symbol"));
            }
        }
        if (e.kind == EdgeKind::Injects
            || (e.kind == EdgeKind::DependsOn && e.provenance == Provenance::Manifest))
            && !evidenced
        {
            failures.push(format!(
                "M4b invariant 10: {what}: evidence {:?}",
                e.evidence
            ));
        }
        if let Some(flag) = e.attrs.get("impl_instantiated")
            && (e.kind != EdgeKind::Implements || flag != &serde_json::Value::Bool(false))
        {
            failures.push(format!("M4b S24: {what}: impl_instantiated {flag}"));
        }
    }
    failures
}

/// Contract M4a S23 and M4b S24 (producer coverage, design §8.5): each fixture has both guard
/// files, they parse, and together their must-exist lines assert every edge kind M4a and M4b
/// produce: `INJECTS`, both `DEPENDS_ON` forms (to a `Package` and, `via=package`, to a
/// `Service`) and an `IMPLEMENTS` flagged `impl_instantiated: false`.
#[test]
fn precision_guards_assert_every_produced_edge_kind() {
    let mut kinds = HashSet::new();
    let (mut to_service, mut to_package, mut flagged) = (false, false, false);
    for fixture in ["ts-express-prisma", "java-spring-jpa", "polyglot-monorepo"] {
        for g in parse_guards(fixture, "must-exist.txt") {
            let via_package = g.attrs.get("via").and_then(|v| v.as_str()) == Some("package");
            if g.kind == EdgeKind::DependsOn.to_string() {
                to_service |= via_package;
                to_package |= g.attrs.contains_key("version_from");
            }
            flagged |= g.kind == EdgeKind::Implements.to_string()
                && g.attrs.get("impl_instantiated") == Some(&serde_json::Value::Bool(false));
            kinds.insert(g.kind);
        }
        assert!(!parse_guards(fixture, "must-not-exist.txt").is_empty());
    }
    for kind in [
        EdgeKind::Exposes,
        EdgeKind::HandledBy,
        EdgeKind::Consumes,
        EdgeKind::Injects,
    ] {
        assert!(kinds.contains(&kind.to_string()), "no must-exist {kind}");
    }
    assert!(to_service, "no must-exist DEPENDS_ON via=package");
    assert!(to_package, "no must-exist DEPENDS_ON to a package");
    assert!(
        flagged,
        "no must-exist IMPLEMENTS with impl_instantiated: false"
    );
}

/// Data rows of `rb cypher` output (header dropped), sorted bytewise; agtype strings print unquoted.
fn sorted_rows(out: &str) -> Vec<String> {
    let mut rows: Vec<String> = out.lines().skip(1).map(str::to_owned).collect();
    rows.sort();
    rows
}

#[test]
fn ts_express_prisma() {
    check_fixture("ts-express-prisma", "ts");
}

#[test]
fn java_spring_jpa() {
    check_fixture("java-spring-jpa", "java");
}

#[test]
fn polyglot_monorepo() {
    check_fixture("polyglot-monorepo", "poly");
}

fn golden_path(fixture: &str, name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(fixture)
        .join(name)
}

/// Compares `actual` with `tests/golden/<fixture>/<name>`; `RB_BLESS=1` rewrites the file instead.
/// Blessed output must be reviewed by hand before it is committed.
fn golden(fixture: &str, name: &str, actual: &str) {
    let path = golden_path(fixture, name);
    if std::env::var("RB_BLESS").is_ok_and(|v| v == "1") {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "missing golden file {}; run with RB_BLESS=1 and review it",
            path.display()
        )
    });
    assert_eq!(actual, expected, "golden mismatch: {}", path.display());
}

/// A `--json` body made stable for a golden (contract M5 S11): each run indexes a fresh
/// `t_*_<pid>` repository, so its name and the snapshot id the database assigns differ per run.
/// The envelope must name exactly `repo` and its snapshot `live`; then every quoted occurrence of
/// the name, the snapshot's AGE graph name and every `"id": <live>` are replaced by placeholders.
fn stable(body: &str, repo: &str, live: i64) -> String {
    let head = format!(
        "{{\n  \"api\": 1,\n  \"repo\": \"{repo}\",\n  \"snapshot\": {{\n    \"id\": {live},\n"
    );
    assert!(
        body.starts_with(&head),
        "envelope of {repo} @ {live}:\n{body}"
    );
    body.replace(&format!("rb_{repo}_{live}"), "rb_<repo>_<live>")
        .replace(&format!("\"{repo}\""), "\"<repo>\"")
        .replace(&format!("\"id\": {live},"), "\"id\": \"<live>\",")
}

fn rb_fails(url: &str, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_rb"))
        .args(args)
        .env("RB_DATABASE_URL", url)
        .output()
        .unwrap();
    assert!(!out.status.success(), "rb {args:?} should have failed");
    String::from_utf8(out.stderr).unwrap()
}

/// M1a structure goldens (design §12.1): services, cards, owners, search and the full dump.
#[test]
fn polyglot_monorepo_structure_goldens() {
    let Some(url) = db_url() else { return };
    let fixture = "polyglot-monorepo";
    let repo = fixture_repo(fixture);
    let name = format!("t_golden_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    rb(
        &url,
        &[
            "index",
            repo.path().to_str().unwrap(),
            "--repo",
            &name,
            "--no-scip",
        ],
    );

    golden(
        fixture,
        "services.txt",
        &rb(&url, &["service", "--repo", &name]),
    );
    golden(
        fixture,
        "card-auth.txt",
        &rb(&url, &["service", "services/auth", "--repo", &name]),
    );
    // Resolution by name, for a service that only exists through rb.toml.
    golden(
        fixture,
        "card-gateway.txt",
        &rb(&url, &["service", "edge-gateway", "--repo", &name]),
    );
    // Contract M4a S20: a service with two inline-handled APIs called by three services.
    golden(
        fixture,
        "card-notification.txt",
        &rb(&url, &["service", "notification", "--repo", &name]),
    );
    let live = live_id(&url, &name);
    golden(
        fixture,
        "card-root.json",
        &stable(
            &rb(&url, &["service", ".", "--repo", &name, "--json"]),
            &name,
            live,
        ),
    );
    golden(
        fixture,
        "find-auth.txt",
        &rb(&url, &["find", "auth", "--repo", &name, "--limit", "8"]),
    );
    golden(
        fixture,
        "find-owners.txt",
        &rb(&url, &["find", "acme", "--kind", "owner", "--repo", &name]),
    );
    golden(
        fixture,
        "find-login-symbols.txt",
        &rb(
            &url,
            &["find", "login", "--kind", "symbol", "--repo", &name],
        ),
    );
    let dump = rb(&url, &["dump", "--repo", &name]);
    // M3a (contract R1, invariants 1-2): resolved code edges are heuristic guesses, never exact.
    for line in dump.lines() {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        let kind = row
            .get(3)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if matches!(kind, "CALLS" | "REFERENCES" | "IMPLEMENTS") {
            assert!(
                matches!(row[6].as_str(), Some("heuristic" | "ambiguous")),
                "{line}"
            );
        }
    }
    golden(fixture, "dump.jsonl", &dump);

    let err = rb_fails(&url, &["service", "no-such-service", "--repo", &name]);
    assert!(err.contains("no service matches"), "{err}");

    // M2a (contract S13, S14): table dependencies and SQL coverage.
    golden(
        fixture,
        "table-deps-public-users.txt",
        &rb(&url, &["table-deps", "public.users", "--repo", &name]),
    );
    golden(
        fixture,
        "table-deps-invoices.json",
        &stable(
            &rb(&url, &["table-deps", "invoices", "--repo", &name, "--json"]),
            &name,
            live,
        ),
    );
    golden(
        fixture,
        "table-deps-email-log.txt",
        &rb(&url, &["table-deps", "email_log", "--repo", &name]),
    );
    let err = rb_fails(&url, &["table-deps", "users", "--repo", &name]);
    assert!(
        err.contains("matches several tables")
            && err.contains("audit.users")
            && err.contains("public.users"),
        "{err}"
    );
    // Unquoted names fold to lowercase, as in Postgres.
    assert!(
        rb(&url, &["table-deps", "Email_Log", "--repo", &name]).starts_with("public.email_log")
    );
    let status = rb(&url, &["status", "--repo", &name]);
    let sql_line = status
        .lines()
        .find(|l| l.trim_start().starts_with("sql:"))
        .unwrap_or_default();
    golden(fixture, "status-sql.txt", &format!("{sql_line}\n"));
    // Contract M3a R8: the resolution coverage line.
    let code_line = status
        .lines()
        .find(|l| l.trim_start().starts_with("code:"))
        .unwrap_or_default();
    golden(fixture, "status-code.txt", &format!("{code_line}\n"));
    // Contract M4a S14: routes, and one bucket per client call site.
    let http_line = status
        .lines()
        .find(|l| l.trim_start().starts_with("http:"))
        .unwrap_or_default();
    golden(fixture, "status-http.txt", &format!("{http_line}\n"));
    // Contract M4b S7: one bucket per injection point.
    let di_line = status
        .lines()
        .find(|l| l.trim_start().starts_with("di:"))
        .unwrap_or_default();
    golden(fixture, "status-di.txt", &format!("{di_line}\n"));
    // Contract M4b S9: implementing types proven never instantiated.
    let rta_line = status
        .lines()
        .find(|l| l.trim_start().starts_with("rta:"))
        .unwrap_or_default();
    golden(fixture, "status-rta.txt", &format!("{rta_line}\n"));
}

/// Every `(service, path)` of a `rb path --format json` tree, depth first.
fn path_steps(nodes: &serde_json::Value, out: &mut Vec<(String, String)>) {
    for n in nodes.as_array().unwrap() {
        out.push((
            n["service"].as_str().unwrap().to_owned(),
            n["path"].as_str().unwrap().to_owned(),
        ));
        path_steps(&n["children"], out);
    }
}

/// Contract M4a S22 (Q4 on `polyglot-monorepo`, `--no-scip`): the login request path from the
/// web client through auth's controller and its Feign client into notification's inline
/// handler and its table, and the MFA email endpoint's callers in three services.
#[test]
fn request_path_goldens() {
    let Some(url) = db_url() else { return };
    let fixture = "polyglot-monorepo";
    let repo = fixture_repo(fixture);
    let name = format!("t_path_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    rb(
        &url,
        &[
            "index",
            repo.path().to_str().unwrap(),
            "--repo",
            &name,
            "--no-scip",
        ],
    );
    // S15: a name matching two endpoints lists both and picks neither.
    golden(
        fixture,
        "path-login-candidates.txt",
        &rb_fails(&url, &["path", "login", "--repo", &name]),
    );
    let login = rb(&url, &["path", "POST /api/v1/login", "--repo", &name]);
    golden(fixture, "path-login.txt", &login);
    // The same endpoint by its key and by a differently written method and path.
    assert_eq!(
        rb(
            &url,
            &[
                "path",
                "HTTP POST /api/v1/login @services/auth",
                "--repo",
                &name
            ]
        ),
        login
    );
    assert_eq!(
        rb(&url, &["path", "post /api/v1//login/", "--repo", &name]),
        login
    );
    let json = rb(
        &url,
        &[
            "path",
            "POST /api/v1/login",
            "--format",
            "json",
            "--repo",
            &name,
        ],
    );
    golden(
        fixture,
        "path-login.json",
        &stable(&json, &name, live_id(&url, &name)),
    );
    // The login path spans three services and two languages (TypeScript → Java → TypeScript).
    let tree: serde_json::Value = serde_json::from_str(&json).unwrap();
    let tree = &tree["result"];
    let mut steps = Vec::new();
    path_steps(&tree["upstream"], &mut steps);
    path_steps(&tree["downstream"], &mut steps);
    let services: HashSet<&str> = steps.iter().map(|(s, _)| s.as_str()).collect();
    assert!(
        ["@acme/web", "auth-service", "notification"]
            .iter()
            .all(|s| services.contains(s)),
        "{services:?}"
    );
    let languages = |steps: &[(String, String)]| -> HashSet<String> {
        steps
            .iter()
            .filter_map(|(_, p)| Path::new(p).extension())
            .map(|e| e.to_string_lossy().replace("tsx", "ts"))
            .collect()
    };
    assert_eq!(
        languages(&steps),
        HashSet::from(["ts".to_owned(), "java".to_owned()])
    );

    // Two services expose `POST /internal/mfa/email`; billing's is listed first (key order).
    let err = rb_fails(&url, &["path", "POST /internal/mfa/email", "--repo", &name]);
    assert!(
        err.contains("1. POST /internal/mfa/email  [billing]")
            && err.contains("2. POST /internal/mfa/email  [notification]"),
        "{err}"
    );
    let email = rb(
        &url,
        &[
            "path",
            "POST /internal/mfa/email",
            "--pick",
            "2",
            "--repo",
            &name,
        ],
    );
    golden(fixture, "path-mfa-email.txt", &email);
    // With the email path, Go joins: TypeScript, Java and Go callers.
    let json = rb(
        &url,
        &[
            "path",
            "POST /internal/mfa/email",
            "--pick",
            "2",
            "--format",
            "json",
            "--repo",
            &name,
        ],
    );
    let tree: serde_json::Value = serde_json::from_str(&json).unwrap();
    let mut steps = Vec::new();
    path_steps(&tree["result"]["upstream"], &mut steps);
    assert_eq!(
        languages(&steps),
        HashSet::from(["ts".to_owned(), "java".to_owned(), "go".to_owned()])
    );
}

/// Output order is bytewise, independent of the database's locale collation.
#[test]
fn listing_order_is_bytewise() {
    let Some(url) = db_url() else { return };
    let repo = repo_from(&[
        ("alpha/package.json", "{}"),
        ("Zeta/package.json", "{}"),
        ("_unde/package.json", "{}"),
        ("README.md", "x"),
    ]);
    let name = format!("t_order_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    rb(
        &url,
        &[
            "index",
            repo.path().to_str().unwrap(),
            "--repo",
            &name,
            "--no-scip",
        ],
    );
    let keys: Vec<String> = rb(&url, &["service", "--repo", &name])
        .lines()
        .map(|l| l.split('\t').next().unwrap().to_owned())
        .collect();
    assert_eq!(keys, [".", "Zeta", "_unde", "alpha"]);
    let found: Vec<String> = rb(&url, &["find", "package", "--repo", &name])
        .lines()
        .map(|l| l.split('\t').nth(1).unwrap().to_owned())
        .collect();
    assert_eq!(
        found,
        [
            "Zeta/package.json",
            "_unde/package.json",
            "alpha/package.json"
        ]
    );
}

/// Number from the `parsed N source files` part of `rb index` output.
fn parsed_count(index_output: &str) -> usize {
    let (_, rest) = index_output
        .split_once("parsed ")
        .unwrap_or_else(|| panic!("no parse count in {index_output:?}"));
    rest.split_whitespace().next().unwrap().parse().unwrap()
}

/// Contract C10/C12: a warm facts cache parses nothing and yields the same snapshot as a cold run.
#[test]
fn warm_facts_cache_equals_cold_run() {
    let Some(url) = db_url() else { return };
    let repo = fixture_repo("polyglot-monorepo");
    let path = repo.path().to_str().unwrap();
    let pid = std::process::id();
    let (cold, warm) = (format!("t_cold_{pid}"), format!("t_warm_{pid}"));
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![cold.clone(), warm.clone()],
    };

    let cold_run = rb(
        &url,
        &["index", path, "--repo", &cold, "--no-cache", "--no-scip"],
    );
    assert!(parsed_count(&cold_run) >= 8, "{cold_run}");
    rb(&url, &["index", path, "--repo", &warm, "--no-scip"]); // fills the cache (may already be warm)
    let warm_run = rb(
        &url,
        &["index", path, "--repo", &warm, "--no-scip", "--force"],
    );
    assert_eq!(parsed_count(&warm_run), 0, "{warm_run}");
    assert_eq!(
        rb(&url, &["dump", "--repo", &cold]),
        rb(&url, &["dump", "--repo", &warm])
    );
}

/// The stderr of a failing `rb` run with extra environment variables.
fn rb_fails_env(url: &str, args: &[&str], env: &[(&str, &str)]) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rb"));
    cmd.args(args).env("RB_DATABASE_URL", url);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    assert!(!out.status.success(), "rb {args:?} should have failed");
    String::from_utf8(out.stderr).unwrap()
}

/// Contract M5 S7: Cypher's guard rails. A statement over its timeout is cancelled and the error
/// names the limit (`--timeout-ms`, or `RB_CYPHER_TIMEOUT_MS`, 1 to 30,000); a write is refused
/// by the read-only transaction; the role `rb_reader` cannot call superuser-only functions
/// (`pg_read_file`, `pg_current_logfile`), which the compose superuser can; more than 10,000
/// rows print the first 10,000 and `truncated: rows`, exactly 10,000 print no such line.
#[test]
fn cypher_guard_rails() {
    let Some(url) = db_url() else { return };
    let repo = fixture_repo("polyglot-monorepo");
    let name = format!("t_cypher_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    rb(
        &url,
        &[
            "index",
            repo.path().to_str().unwrap(),
            "--repo",
            &name,
            "--no-scip",
        ],
    );
    // 105^5 rows on this fixture: the 4-way product counts in 0.9 s (phase 4), so this one
    // cannot finish within 100 ms on any machine the suite runs on.
    let product = "MATCH (a), (b), (c), (d), (e) RETURN count(*)";
    // A coarse bound on each run: the 100 ms timeout, not the 30 s maximum, cancelled it.
    let started = std::time::Instant::now();
    let err = rb_fails(
        &url,
        &["cypher", product, "--repo", &name, "--timeout-ms", "100"],
    );
    assert!(
        err.contains("statement timeout of 100 ms") && err.contains("--timeout-ms"),
        "{err}"
    );
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    let started = std::time::Instant::now();
    let err = rb_fails_env(
        &url,
        &["cypher", product, "--repo", &name],
        &[("RB_CYPHER_TIMEOUT_MS", "100")],
    );
    assert!(err.contains("statement timeout of 100 ms"), "{err}");
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    for bad in ["0", "30001"] {
        let err = rb_fails(
            &url,
            &["cypher", "RETURN 1", "--repo", &name, "--timeout-ms", bad],
        );
        assert!(err.contains("--timeout-ms"), "{bad}: {err}");
    }
    let err = rb_fails(
        &url,
        &[
            "cypher",
            "CREATE (n:File {key: 'x'}) RETURN n",
            "--repo",
            &name,
        ],
    );
    assert!(err.contains("read-only"), "{err}");
    for (query, function) in [
        (
            "RETURN pg_catalog.pg_read_file('/etc/hostname'::pg_text)",
            "pg_read_file",
        ),
        (
            "RETURN pg_catalog.pg_current_logfile()",
            "pg_current_logfile",
        ),
    ] {
        let err = rb_fails(&url, &["cypher", query, "--repo", &name]);
        assert!(
            err.contains(&format!("permission denied for function {function}")),
            "{err}"
        );
    }
    let rows = |n: u32| {
        rb(
            &url,
            &[
                "cypher",
                &format!("UNWIND range(1, {n}) AS x RETURN x"),
                "--repo",
                &name,
                "--cols",
                "x",
            ],
        )
    };
    let over = rows(10_001);
    let lines: Vec<&str> = over.lines().collect();
    assert_eq!(lines.len(), 1 + 10_000 + 1, "header, rows, truncated");
    assert_eq!(lines[0], "x");
    assert_eq!(*lines.last().unwrap(), "truncated: rows");
    let at = rows(10_000);
    assert_eq!(at.lines().count(), 1 + 10_000);
    assert!(!at.contains("truncated"));
}

/// Contract M5 D6, amendment 4: the `work` cap's calibration. Every default-parameter question
/// on the three fixtures (Q1 depth 1 and Q2 on every symbol and table, Q4 on every endpoint, Q3
/// on every table; `cha`) spends under 1/10,000 of the cap and is never cut by it. Prints the
/// largest count per question. No database: the fixtures are indexed in memory.
#[test]
fn work_cap_is_far_above_every_fixture_question() {
    use rb_store::code::{CallerOptions, Precision, SnapshotIndex, callers, impact};
    use rb_store::limits::{Caps, Limit, WORK};
    let mut max = [0u64; 4];
    for fixture in ["ts-express-prisma", "java-spring-jpa", "polyglot-monorepo"] {
        let repo = fixture_repo(fixture);
        let indexed = rb_indexer::index_commit(
            repo.path(),
            "HEAD",
            &mut rb_indexer::NoCache,
            &rb_indexer::ScipOptions::disabled(),
            &mut rb_indexer::NoScipCache,
        )
        .unwrap();
        let ix = SnapshotIndex::new(indexed.graph);
        let ids = |kind: NodeKind| -> Vec<i64> {
            ix.graph()
                .nodes()
                .iter()
                .filter(|n| n.kind == kind)
                .map(|n| n.id)
                .collect()
        };
        for t in ids(NodeKind::Symbol)
            .into_iter()
            .chain(ids(NodeKind::Table))
        {
            let q1 = callers(
                &ix,
                t,
                CallerOptions {
                    depth: rb_store::limits::CALLER_DEPTH_DEFAULT,
                    include_references: false,
                    min_certainty: Certainty::Ambiguous,
                    precision: Precision::Cha,
                    caps: Caps::default(),
                },
            );
            let q2 = impact(&ix, t, Certainty::Ambiguous, Precision::Cha);
            assert!(!q1.truncated.contains(&Limit::Work) && !q2.truncated.contains(&Limit::Work));
            max[0] = max[0].max(q1.work);
            max[1] = max[1].max(q2.work);
        }
        for e in ids(NodeKind::Endpoint) {
            let opts = rb_store::path::PathOptions {
                precision: Precision::Cha,
                ..Default::default()
            };
            let q4 = rb_store::path::request_path(&ix, e, opts);
            assert!(!q4.truncated.contains(&Limit::Work));
            max[2] = max[2].max(q4.work);
        }
        for t in ids(NodeKind::Table) {
            let opts = rb_store::rollup::RollupOptions {
                precision: Precision::Cha,
                hops: rb_store::limits::HOPS_DEFAULT,
                caps: Caps::default(),
            };
            let q3 = rb_store::rollup::table_rollup(&ix, t, opts);
            assert!(!q3.truncated.iter().any(|l| l == "work"));
            max[3] = max[3].max(q3.work);
        }
    }
    eprintln!(
        "largest work on the fixtures: Q1 depth 1 {}, Q2 {}, Q4 {}, Q3 {} (cap {WORK})",
        max[0], max[1], max[2], max[3]
    );
    assert!(max.iter().all(|&w| w > 0 && w * 10_000 <= WORK), "{max:?}");
}

/// Contract M5 S17: `rb diff`'s summaries use the Service Card's rule. On every fixture, for
/// every service, the card's dependencies are exactly `service_dependencies` from that service,
/// and every table operation of the card is in `table_access` for it (which adds migrations).
/// No database: the fixtures are indexed in memory.
#[test]
fn diff_summaries_agree_with_the_service_card() {
    use rb_store::code::SnapshotIndex;
    for fixture in ["ts-express-prisma", "java-spring-jpa", "polyglot-monorepo"] {
        let repo = fixture_repo(fixture);
        let indexed = rb_indexer::index_commit(
            repo.path(),
            "HEAD",
            &mut rb_indexer::NoCache,
            &rb_indexer::ScipOptions::disabled(),
            &mut rb_indexer::NoScipCache,
        )
        .unwrap();
        let ix = SnapshotIndex::new(indexed.graph);
        let deps = rb_store::diff::service_dependencies(&ix);
        let access = rb_store::diff::table_access(&ix);
        let services: Vec<String> = ix
            .graph()
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::Service)
            .map(|n| n.key.clone())
            .collect();
        let mut compared = 0;
        for key in &services {
            let card = rb_store::card::card_sections(&ix, key);
            let from_card: std::collections::BTreeSet<(String, String)> = card
                .dependencies
                .iter()
                .map(|d| (d.key.clone(), d.via.clone()))
                .collect();
            let from_summary: std::collections::BTreeSet<(String, String)> = deps
                .iter()
                .filter(|d| &d.from == key)
                .map(|d| (d.to.clone(), d.via.clone()))
                .collect();
            assert_eq!(from_card, from_summary, "{fixture} {key}");
            for t in &card.tables {
                let ops: Vec<String> = if t.ops.is_empty() {
                    vec!["mapped".to_owned()]
                } else {
                    t.ops.clone()
                };
                for op in ops {
                    assert!(
                        access
                            .iter()
                            .any(|a| &a.service == key && a.table == t.key && a.op == op),
                        "{fixture} {key} {} {op}",
                        t.key
                    );
                }
            }
            compared += from_card.len() + card.tables.len();
        }
        assert!(compared > 0, "{fixture}: nothing compared");
    }
}

/// Contract M5 S5 (M4a open item 4): `rb index --no-cache` parses every file and still writes the
/// facts it extracted, so a plain run afterwards parses nothing; both dump identically. The
/// sources carry a per-run token so that no earlier run has cached their blobs.
#[test]
fn no_cache_writes_the_facts_it_extracted() {
    let Some(url) = db_url() else { return };
    let token = format!(
        "{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let ts = format!("export function a_{token}(): number {{ return 1; }}\n");
    let py = format!("def b_{token}():\n    return 2\n");
    let java = format!("class C {{ int c_{token}() {{ return 3; }} }}\n");
    let repo = repo_from(&[("src/a.ts", &ts), ("src/b.py", &py), ("src/C.java", &java)]);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_nocache_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    let fresh = rb(
        &url,
        &["index", path, "--repo", &name, "--no-scip", "--no-cache"],
    );
    assert_eq!(parsed_count(&fresh), 3, "{fresh}");
    let dump = rb(&url, &["dump", "--repo", &name]);
    let plain = rb(
        &url,
        &["index", path, "--repo", &name, "--no-scip", "--force"],
    );
    assert_eq!(parsed_count(&plain), 0, "{plain}");
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
}

/// Contract M5 S4: `rb gc --dry-run` and `rb gc`, scoped to this test's repositories, list
/// nothing for a repository whose only graph is its live snapshot's; `rb cache prune --dry-run`
/// reports rows and bytes and removes nothing; `rb cache prune` needs `--facts` or `--scip`.
#[test]
fn gc_and_cache_prune_commands() {
    let Some(url) = db_url() else { return };
    let repo = fixture_repo("ts-express-prisma");
    let path = repo.path().to_str().unwrap();
    let prefix = format!("t_gccli_{}_", std::process::id());
    let name = format!("{prefix}a");
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    rb(
        &url,
        &["index", path, "--repo", &name, "--no-scip", "--force"],
    );
    for args in [
        &["gc", "--dry-run", "--prefix", &prefix][..],
        &["gc", "--prefix", &prefix],
    ] {
        assert_eq!(rb(&url, args), "gc: nothing to remove\n", "{args:?}");
    }
    let out = rb(&url, &["cache", "prune", "--facts", "--dry-run"]);
    assert!(
        out.starts_with("facts: would remove ") && out.ends_with(" bytes\n"),
        "{out}"
    );
    let err = rb_fails(&url, &["cache", "prune"]);
    assert!(err.contains("--facts") && err.contains("--scip"), "{err}");
}

/// Contract M5 amendment 16 (evaluator pass 1, Z2), the evaluator's repro end to end: a session
/// holds `ACCESS SHARE` on the live graph's vertex table, so the next `rb index`'s retention
/// waits to drop it; the publisher is killed (SIGKILL) once its flip has committed. The retired
/// snapshot still names its graph, yet `rb cypher --snapshot` on it is `invalid_param` (D17)
/// and `rb snapshots` names no graph for it; `rb serve`'s startup sweep then drops the leftover,
/// leaving one graph, the live snapshot's.
#[test]
fn a_publish_killed_after_its_flip_self_heals() {
    use serde_json::Value;
    let Some(url) = db_url() else { return };
    let name = format!("t_flipkill_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    let dir = repo_from(&[("src/a.ts", "export function a(): number { return 1; }\n")]);
    let path = dir.path().to_str().unwrap();
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    let old = live_id(&url, &name);
    std::fs::write(
        dir.path().join("src/b.ts"),
        "export function b(): number { return 2; }\n",
    )
    .unwrap();
    commit_all(dir.path());
    let graphs = |c: &mut postgres::Client| -> Vec<String> {
        let mut g: Vec<String> = c
            .query(
                "SELECT name::text FROM ag_catalog.ag_graph WHERE name::text ~ $1",
                &[&format!("^rb_{name}_[0-9]+$")],
            )
            .unwrap()
            .iter()
            .map(|r| r.get(0))
            .collect();
        g.sort();
        g
    };

    let mut holder = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    let mut held = holder.transaction().unwrap();
    held.batch_execute(&format!(
        "LOCK TABLE rb_{name}_{old}._ag_label_vertex IN ACCESS SHARE MODE"
    ))
    .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_rb"))
        .args(["index", path, "--repo", &name, "--no-scip"])
        .env("RB_DATABASE_URL", &*url)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let started = std::time::Instant::now();
    let new = loop {
        let live = Store::connect(&url)
            .unwrap()
            .live_snapshot(&name)
            .unwrap()
            .map(|s| s.id);
        if let Some(id) = live.filter(|id| *id != old) {
            break id;
        }
        assert!(
            child.try_wait().unwrap().is_none(),
            "rb index ended before it was killed"
        );
        assert!(started.elapsed() < std::time::Duration::from_secs(120));
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    child.kill().unwrap();
    child.wait().unwrap();
    held.rollback().unwrap();
    let mut probe = postgres::Client::connect(&url, postgres::NoTls).unwrap();
    let both = vec![format!("rb_{name}_{old}"), format!("rb_{name}_{new}")];
    assert_eq!(graphs(&mut probe), both, "the leftover the kill leaves");

    let (code, out, _) = rb_exit(
        &url,
        &[
            "cypher",
            "MATCH (n) RETURN count(n)",
            "--repo",
            &name,
            "--snapshot",
            &old.to_string(),
            "--json",
        ],
    );
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (code, v["error"]["kind"].as_str()),
        (2, Some("invalid_param")),
        "{out}"
    );
    let listed: Value =
        serde_json::from_str(&rb(&url, &["snapshots", "--repo", &name, "--json"])).unwrap();
    let rows: Vec<(i64, Value)> = listed["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_i64().unwrap(), s["age_graph"].clone()))
        .collect();
    assert_eq!(
        rows,
        vec![
            (new, Value::String(format!("rb_{name}_{new}"))),
            (old, Value::Null)
        ]
    );

    // Contract M6a S14 (D10): a server that refuses its bind address writes nothing, so the
    // leftover survives it; the one that starts on loopback sweeps it.
    let (code, _, err) = rb_exit(&url, &["serve", "--bind", "0.0.0.0:0"]);
    assert_ne!(code, 0, "a non-loopback bind is refused: {err}");
    assert!(err.contains("refusing to bind"), "{err}");
    assert_eq!(
        graphs(&mut probe),
        both,
        "a refused start leaves the leftover"
    );
    let server = Served::start(&url, &[]);
    assert_eq!(graphs(&mut probe), vec![format!("rb_{name}_{new}")]);
    drop(server);
    assert_eq!(
        rb(&url, &["gc", "--dry-run", "--prefix", &name]),
        "gc: nothing to remove\n"
    );
}

/// Contract C11: facts cached under one extractor version are invisible to another.
#[test]
fn facts_cache_is_keyed_by_extractor_version() {
    let Some(url) = db_url() else { return };
    let mut store = Store::connect(&url).unwrap();
    store.migrate().unwrap();
    let blob = format!("{:0>40x}", std::process::id());
    let row = |extractor: &str| (blob.clone(), extractor.to_owned());
    store
        .save_facts(&[(blob.clone(), "test-v1;go".into(), b"{}".to_vec())])
        .unwrap();
    assert_eq!(store.load_facts(&[row("test-v1;go")]).unwrap().len(), 1);
    assert!(store.load_facts(&[row("test-v2;go")]).unwrap().is_empty());
    assert!(store.load_facts(&[row("test-v1;java")]).unwrap().is_empty());
}

/// M2b (contract O8): ORM mappings and accesses in `rb table-deps`, per ORM fixture.
#[test]
fn orm_table_deps_goldens() {
    let Some(url) = db_url() else { return };
    for (fixture, table) in [("java-spring-jpa", "users"), ("ts-express-prisma", "users")] {
        let repo = fixture_repo(fixture);
        let name = format!("t_orm_{}_{}", fixture.replace('-', "_"), std::process::id());
        let _cleanup = Cleanup {
            url: &url,
            repos: vec![name.clone()],
        };
        rb(
            &url,
            &[
                "index",
                repo.path().to_str().unwrap(),
                "--repo",
                &name,
                "--no-scip",
            ],
        );
        golden(
            fixture,
            "table-deps-users.txt",
            &rb(&url, &["table-deps", table, "--repo", &name]),
        );
    }
}

/// Contract M5 S11, D17, D5 (polyglot, `--no-scip`): the new JSON answers (`callers` with its
/// entry marks, `find`, `status`) in the envelope; `rb snapshots`; `--snapshot` on a retired
/// snapshot answers byte-for-byte as that snapshot did while live, Cypher on it is
/// `invalid_param` (no projection), and a deleted, foreign or never-issued snapshot is
/// `snapshot_not_retained` or `not_found`; each error kind exits with D5's code and prints its body
/// with `--json` (its message on stderr without).
#[test]
fn json_answers_snapshots_and_error_codes() {
    let Some(url) = db_url() else { return };
    let fixture = "polyglot-monorepo";
    let repo = fixture_repo(fixture);
    let path = repo.path().to_str().unwrap();
    let pid = std::process::id();
    let (name, other) = (format!("t_api_{pid}"), format!("t_api_other_{pid}"));
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone(), other.clone()],
    };
    let json = |s: &str| -> serde_json::Value { serde_json::from_str(s).unwrap() };
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    let first = live_id(&url, &name);

    let callers_args = [
        "callers",
        "MfaClient.sendCode",
        "--depth",
        "2",
        "--repo",
        &name,
    ];
    let callers = rb(&url, &[&callers_args[..], &["--json"]].concat());
    golden(
        fixture,
        "callers-send-code.json",
        &stable(&callers, &name, first),
    );
    let find = rb(
        &url,
        &["find", "auth", "--limit", "8", "--repo", &name, "--json"],
    );
    golden(fixture, "find-auth.json", &stable(&find, &name, first));
    // The JSON lists what the text lists.
    let listed: Vec<String> = json(&find)["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            format!(
                "{}\t{}\t{}",
                n["kind"].as_str().unwrap(),
                n["key"].as_str().unwrap(),
                n["name"].as_str().unwrap()
            )
        })
        .collect();
    let text = rb(&url, &["find", "auth", "--limit", "8", "--repo", &name]);
    assert_eq!(listed, text.lines().collect::<Vec<_>>());
    golden(
        fixture,
        "status.json",
        &stable(
            &rb(&url, &["status", "--repo", &name, "--json"]),
            &name,
            first,
        ),
    );

    // A second snapshot of the commit: the first is retired and kept, without a graph.
    let live_text = rb(&url, &callers_args);
    rb(
        &url,
        &["index", path, "--repo", &name, "--no-scip", "--force"],
    );
    let second = live_id(&url, &name);
    let rows: Vec<(i64, String, Option<i64>, bool)> =
        json(&rb(&url, &["snapshots", "--repo", &name, "--json"]))["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r["id"].as_i64().unwrap(),
                    r["status"].as_str().unwrap().to_owned(),
                    r["parent_id"].as_i64(),
                    r["age_graph"].is_string(),
                )
            })
            .collect();
    assert_eq!(
        rows,
        [
            (second, "live".to_owned(), Some(first), true),
            (first, "retired".to_owned(), None, false)
        ]
    );
    let old = first.to_string();
    let on_old = rb(
        &url,
        &[&callers_args[..], &["--json", "--snapshot", &old]].concat(),
    );
    assert_eq!(
        on_old, callers,
        "the retired snapshot answers as it did while live"
    );
    assert_eq!(
        rb(&url, &[&callers_args[..], &["--snapshot", &old]].concat()),
        live_text
    );
    let (code, out, _) = rb_exit(
        &url,
        &[
            "cypher",
            "RETURN 1",
            "--repo",
            &name,
            "--snapshot",
            &old,
            "--json",
        ],
    );
    let e = json(&out);
    assert_eq!(
        (
            code,
            e["error"]["kind"].as_str(),
            e["error"]["param"].as_str()
        ),
        (2, Some("invalid_param"), Some("snapshot")),
        "{out}"
    );

    // Retention deletes it: no longer kept.
    rb(
        &url,
        &[
            "index",
            path,
            "--repo",
            &name,
            "--no-scip",
            "--force",
            "--keep",
            "0",
        ],
    );
    let (code, out, _) = rb_exit(
        &url,
        &[&callers_args[..], &["--json", "--snapshot", &old]].concat(),
    );
    assert_eq!(
        (code, json(&out)["error"]["kind"].as_str()),
        (3, Some("snapshot_not_retained")),
        "{out}"
    );
    let (code, out, err) = rb_exit(&url, &[&callers_args[..], &["--snapshot", &old]].concat());
    assert_eq!((code, out.as_str()), (3, ""));
    assert!(
        err.starts_with("Error: snapshot ") && err.contains("is no longer kept"),
        "{err}"
    );
    // Another repository's snapshot, and an id never issued.
    let small = repo_from(&[("README.md", "x")]);
    rb(
        &url,
        &[
            "index",
            small.path().to_str().unwrap(),
            "--repo",
            &other,
            "--no-scip",
        ],
    );
    for (snapshot, message) in [
        (live_id(&url, &other).to_string(), "is not a snapshot of"),
        ("9000000000000000000".to_owned(), "does not exist"),
    ] {
        let (code, out, _) = rb_exit(
            &url,
            &[&callers_args[..], &["--json", "--snapshot", &snapshot]].concat(),
        );
        let e = json(&out);
        assert_eq!(
            (code, e["error"]["kind"].as_str()),
            (3, Some("not_found")),
            "{out}"
        );
        assert!(
            e["error"]["message"].as_str().unwrap().contains(message),
            "{out}"
        );
    }

    // Every repository's status: no single repository or snapshot in the envelope.
    let all = json(&rb(&url, &["status", "--json"]));
    assert!(all["repo"].is_null() && all["snapshot"].is_null(), "{all}");
    let names: Vec<&str> = all["result"]["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&name.as_str()) && names.contains(&other.as_str()),
        "{names:?}"
    );

    // The card's derived sections run under the work budget (contract M5 amendment 4): cut by
    // the test-only override, the answer names `work`, in JSON and in text; under the default
    // cap it is not cut.
    let caps = Settings::TEST_CAPS_VAR;
    let card = |work: &str, json_flag: bool| {
        let mut args = vec!["service", ".", "--repo", name.as_str()];
        if json_flag {
            args.push("--json");
        }
        let (code, out, err) = rb_exit_env(&url, &args, &[(caps, &format!("work={work}"))]);
        assert_eq!(code, 0, "{err}");
        out
    };
    let cut = json(&card("1", true));
    assert_eq!(
        (cut["truncated"].clone(), cut["limits"]["work"].clone()),
        (serde_json::json!(["work"]), 1.into())
    );
    assert!(card("1", false).ends_with("truncated: work\n"));
    let full = json(&rb(&url, &["service", ".", "--repo", &name, "--json"]));
    assert_eq!(full["truncated"], serde_json::json!([]));

    // D5: each kind's exit code, in both modes; JSON prints the error body on stdout.
    let nope = format!("t_api_nope_{pid}");
    for (args, code, kind) in [
        (
            vec![
                "callers",
                "MfaClient.sendCode",
                "--depth",
                "26",
                "--repo",
                &name,
            ],
            2,
            "invalid_param",
        ),
        (
            vec!["callers", "noSuchSymbolAnywhere", "--repo", &name],
            3,
            "not_found",
        ),
        (vec!["callers", "x", "--repo", &nope], 3, "not_found"),
        (vec!["impact", "login", "--repo", &name], 4, "ambiguous"),
        (
            vec!["cypher", "MATCH (n RETURN n", "--repo", &name],
            5,
            "cypher",
        ),
    ] {
        let (got, out, err) = rb_exit(&url, &[&args[..], &["--json"]].concat());
        let e = json(&out);
        assert_eq!(
            (got, e["error"]["kind"].as_str()),
            (code, Some(kind)),
            "{args:?}: {out}{err}"
        );
        assert_eq!(err, "", "{args:?}");
        let (got, out, err) = rb_exit(&url, &args);
        assert_eq!((got, out.as_str()), (code, ""), "{args:?}");
        assert!(err.starts_with("Error: "), "{args:?}: {err}");
        if kind == "ambiguous" {
            let keys: Vec<&str> = e["error"]["candidates"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| c["key"].as_str().unwrap())
                .collect();
            assert_eq!(keys.len(), 2, "{out}");
            assert!(
                keys[0].ends_with("login().") && keys[1].ends_with("AuthController#login()."),
                "{keys:?}"
            );
        }
    }
}

/// Contract M5 S15, D18 (polyglot, `--no-scip`): `rb find`, `rb service` and `rb snapshots`
/// page with `--limit`/`--offset`; at limits 1, 7 and the maximum the pages concatenate to the
/// whole list, `total` is its length, `next_offset` is null exactly on the last page, an offset
/// past the end is an empty page; text prints the page's rows on stdout and the next offset on
/// stderr.
#[test]
fn flat_lists_page_by_limit_and_offset() {
    let Some(url) = db_url() else { return };
    let repo = fixture_repo("polyglot-monorepo");
    let path = repo.path().to_str().unwrap();
    let name = format!("t_pages_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    rb(
        &url,
        &["index", path, "--repo", &name, "--no-scip", "--force"],
    );
    rb(
        &url,
        &["index", path, "--repo", &name, "--no-scip", "--force"],
    );
    let json = |args: &[&str]| -> serde_json::Value {
        serde_json::from_str(&rb(&url, &[args, &["--repo", &name, "--json"]].concat())).unwrap()
    };
    for (list, max) in [
        (&["find", "a"][..], "1000"),
        (&["service"][..], "1000"),
        (&["snapshots"][..], "1000"),
    ] {
        let whole = json(&[list, &["--limit", max]].concat());
        let total = whole["result"]["total"].as_u64().unwrap();
        let items = whole["result"]["items"].as_array().unwrap().clone();
        assert_eq!(items.len() as u64, total, "{list:?}");
        assert!(total >= 3, "{list:?}: {total}");
        assert!(whole["result"]["next_offset"].is_null());
        for limit in ["1", "7", max] {
            let mut joined = Vec::new();
            let mut offset = 0u64;
            // Terminates: each page starts where asked, the next one strictly later, and there
            // are at most `total` pages.
            for _ in 0..=total {
                let off = offset.to_string();
                let page = json(&[list, &["--limit", limit, "--offset", &off]].concat());
                let r = &page["result"];
                assert_eq!(
                    (r["total"].as_u64(), r["offset"].as_u64()),
                    (Some(total), Some(offset))
                );
                joined.extend(r["items"].as_array().unwrap().iter().cloned());
                match r["next_offset"].as_u64() {
                    Some(next) => {
                        assert!(next > offset, "{list:?}: {next} after {offset}");
                        offset = next;
                    }
                    None => break,
                }
            }
            assert_eq!(joined, items, "{list:?} limit {limit}");
        }
        let past = (total + 2).to_string();
        let r = &json(&[list, &["--offset", &past]].concat())["result"];
        assert!(r["items"].as_array().unwrap().is_empty() && r["next_offset"].is_null());
    }
    // Text: the rows on stdout, the next page on stderr.
    let (code, out, err) = rb_exit(
        &url,
        &[
            "find", "a", "--limit", "2", "--offset", "1", "--repo", &name,
        ],
    );
    assert_eq!((code, out.lines().count()), (0, 2), "{out}");
    let total = json(&["find", "a"])["result"]["total"].as_u64().unwrap();
    assert_eq!(
        err,
        format!("rows 2-3 of {total} listed; next page: --offset 3\n")
    );
    let (_, _, err) = rb_exit(&url, &["service", "--repo", &name]);
    assert_eq!(err, "", "the whole list fits one page");
}

/// Contract M5 S13 against the database: `rb serve` answers with the body `rb … --json` prints;
/// when the live pointer moves between two requests, the second is answered from the new
/// snapshot; a write through Cypher is refused (read-only); SIGINT stops it cleanly.
#[test]
fn serve_answers_from_the_live_snapshot() {
    let Some(url) = db_url() else { return };
    let repo = repo_from(&[(
        "src/a.ts",
        "export function b(): number { return 1; }\nexport function a(): number { return b(); }\n",
    )]);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_serve_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    let mut server = Served::start(&url, &[]);
    let callers = format!("/v1/repos/{name}/callers?target=b");
    let (status, body) = server.call("GET", &callers, None);
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, rb(&url, &["callers", "b", "--repo", &name, "--json"]));
    let first: serde_json::Value = serde_json::from_str(&body).unwrap();

    // A new commit with a second caller; the pointer moves under the running server.
    std::fs::write(
        repo.path().join("src/c.ts"),
        "import { b } from \"./a\";\nexport function c(): number { return b(); }\n",
    )
    .unwrap();
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", "second"]] {
        let out = Command::new("git")
            .args(args)
            .current_dir(repo.path())
            .env("GIT_AUTHOR_NAME", "rb")
            .env("GIT_AUTHOR_EMAIL", "rb@example.invalid")
            .env("GIT_COMMITTER_NAME", "rb")
            .env("GIT_COMMITTER_EMAIL", "rb@example.invalid")
            .env("GIT_AUTHOR_DATE", "2026-01-02T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-02T00:00:00Z")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success());
    }
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    let (status, body) = server.call("GET", &callers, None);
    assert_eq!(status, 200, "{body}");
    let second: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        second["snapshot"]["id"].as_i64(),
        Some(live_id(&url, &name))
    );
    assert_ne!(second["snapshot"]["id"], first["snapshot"]["id"]);
    let caller_names = |v: &serde_json::Value| -> Vec<String> {
        v["result"]["callers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["display"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(caller_names(&first).len(), 1);
    assert_eq!(caller_names(&second).len(), 2, "{body}");
    assert_eq!(body, rb(&url, &["callers", "b", "--repo", &name, "--json"]));

    // Writes are refused: the Cypher transaction is read-only (and runs as `rb_reader`).
    let (status, body) = server.call(
        "POST",
        &format!("/v1/repos/{name}/cypher"),
        Some(r#"{"query": "CREATE (n:File {key: 'x'}) RETURN n"}"#),
    );
    assert_eq!(status, 422, "{body}");
    assert!(
        body.contains("\"kind\": \"cypher\"") && body.contains("read-only"),
        "{body}"
    );
    let (status, body) = server.call("GET", "/v1/repos", None);
    assert_eq!(status, 200);
    assert!(body.contains(&format!("\"name\": \"{name}\"")), "{body}");

    // SIGINT: a clean exit.
    let _ = Command::new("kill")
        .args(["-INT", &server.child.id().to_string()])
        .status();
    let exit = server.child.wait().unwrap();
    assert!(exit.success(), "{exit:?}");
}

/// Commits the working tree of `dir` with the e2e's fixed identity at `date`.
fn commit_at(dir: &Path, message: &str, date: &str) {
    for args in [&["add", "-A"][..], &["commit", "-q", "-m", message]] {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "rb")
            .env("GIT_AUTHOR_EMAIL", "rb@example.invalid")
            .env("GIT_COMMITTER_NAME", "rb")
            .env("GIT_COMMITTER_EMAIL", "rb@example.invalid")
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// Replaces `old` (present exactly once) by `new` in `path`.
fn edit(path: &Path, old: &str, new: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    assert_eq!(text.matches(old).count(), 1, "{}: {old}", path.display());
    std::fs::write(path, text.replace(old, new)).unwrap();
}

/// Contract M5 S17 (polyglot, `--no-scip`): a two-commit history — commit 2 adds the endpoint
/// `POST /internal/mfa/push`, removes the call `users.touch(1L)` from `AuthController.login`,
/// turns `UserDao.touch`'s `INSERT INTO audit.users` into a `DELETE`, and adds a call from
/// notification to auth's `GET /api/v1/login` — indexed in order; `rb diff` (from the live
/// snapshot's parent to it) names exactly the predicted summary rows, in text and JSON (goldens).
#[test]
fn diff_of_a_two_commit_history() {
    let Some(url) = db_url() else { return };
    let fixture = "polyglot-monorepo";
    let repo = fixture_repo(fixture);
    let root = repo.path();
    edit(
        &root.join("services/notification/src/server.ts"),
        "// app.post(\"/internal/mfa/push\"",
        "app.post(\"/internal/mfa/push\"",
    );
    edit(
        &root.join("services/auth/src/main/java/com/acme/auth/AuthController.java"),
        "        users.touch(1L);\n",
        "",
    );
    edit(
        &root.join("services/auth/src/main/java/com/acme/auth/UserDao.java"),
        "jdbc.update(\"INSERT INTO audit.users (user_id, event) VALUES (?, 'touch')\", id);",
        "jdbc.update(\"DELETE FROM audit.users WHERE user_id = ?\", id);",
    );
    let selfcheck = root.join("services/notification/src/selfcheck.ts");
    let mut text = std::fs::read_to_string(&selfcheck).unwrap();
    text.push_str("\n// Checks that auth's login page answers.\nexport async function loginCheck(): Promise<boolean> {\n  const res = await fetch(\"http://auth-service/api/v1/login\");\n  return res.ok;\n}\n");
    std::fs::write(&selfcheck, text).unwrap();
    commit_at(root, "second", "2026-01-02T00:00:00Z");
    let path = root.to_str().unwrap();
    let name = format!("t_diff_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    rb(
        &url,
        &[
            "index",
            path,
            "--repo",
            &name,
            "--no-scip",
            "--rev",
            "HEAD~1",
        ],
    );
    let first = live_id(&url, &name);
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    let second = live_id(&url, &name);

    let json = rb(&url, &["diff", "--repo", &name, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let r = &v["result"];
    assert_eq!(
        (r["from"]["id"].as_i64(), r["to"]["id"].as_i64()),
        (Some(first), Some(second))
    );
    // The rows predicted before blessing (contract M5 S17).
    assert_eq!(
        r["summary"],
        serde_json::json!({
            "endpoints": {
                "added": [{"key": "HTTP POST /internal/mfa/push @services/notification",
                           "display": "POST /internal/mfa/push", "service": "notification"}],
                "removed": []
            },
            "table_access": {
                "added": [{"service": "services/auth", "table": "audit.users", "op": "delete"}],
                "removed": [{"service": "services/auth", "table": "audit.users", "op": "insert"}]
            },
            "service_dependencies": {
                "added": [{"from": "services/notification", "to": "services/auth", "via": "http"}],
                "removed": []
            },
            "certainty": {}
        }),
        "{json}"
    );
    assert_eq!(r["tools_differ"], serde_json::json!([]));
    // The same request by commit prefix and by id gives the same body.
    let commit = |id: i64| -> String {
        Store::connect(&url)
            .unwrap()
            .snapshot(id)
            .unwrap()
            .commit_sha[..8]
            .to_owned()
    };
    assert_eq!(
        rb(
            &url,
            &[
                "diff",
                &commit(first),
                &commit(second),
                "--repo",
                &name,
                "--json"
            ]
        ),
        json
    );
    assert_eq!(
        rb(
            &url,
            &[
                "diff",
                &first.to_string(),
                &second.to_string(),
                "--repo",
                &name,
                "--json"
            ]
        ),
        json
    );
    let text = rb(&url, &["diff", "--repo", &name]);
    let stable_text = |t: &str| {
        let head = format!("diff snapshot {first} of {name} @ ");
        assert!(t.starts_with(&head), "{t}");
        t.replace(
            &format!("snapshot {first} of {name}"),
            "snapshot <from> of <repo>",
        )
        .replace(
            &format!("snapshot {second} of {name}"),
            "snapshot <live> of <repo>",
        )
    };
    golden(fixture, "diff-polyglot.txt", &stable_text(&text));
    golden(
        fixture,
        "diff-polyglot.json",
        &stable(&json, &name, second)
            .replace(&format!("\"id\": {first},"), "\"id\": \"<from>\",")
            .replace(&format!("\"from\": {first},"), "\"from\": \"<from>\",")
            .replace(&format!("\"to\": {second}\n"), "\"to\": \"<live>\"\n"),
    );
    // The reverse diff is the inverse: added and removed swap.
    let back: serde_json::Value = serde_json::from_str(&rb(
        &url,
        &[
            "diff",
            &second.to_string(),
            &first.to_string(),
            "--repo",
            &name,
            "--json",
        ],
    ))
    .unwrap();
    assert_eq!(
        back["result"]["summary"]["endpoints"]["removed"],
        r["summary"]["endpoints"]["added"]
    );
    assert_eq!(
        back["result"]["nodes"]["added"]["total"],
        r["nodes"]["removed"]["total"]
    );
    // Selector errors: a commit prefix shared by both snapshots of one commit is ambiguous.
    rb(
        &url,
        &["index", path, "--repo", &name, "--no-scip", "--force"],
    );
    let (code, out, _) = rb_exit(&url, &["diff", &commit(second), "--repo", &name, "--json"]);
    let e: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(
        (code, e["error"]["kind"].as_str()),
        (4, Some("ambiguous")),
        "{out}"
    );
    assert_eq!(e["error"]["candidates"].as_array().unwrap().len(), 2);
    let (code, _, err) = rb_exit(&url, &["diff", "zz12", "--repo", &name]);
    assert_eq!(code, 2, "{err}");
}

/// The `N` and `E` lines of a dump.
fn row_lines(dump: &str) -> Vec<String> {
    dump.lines()
        .filter(|l| l.starts_with("[\"N\"") || l.starts_with("[\"E\""))
        .map(str::to_owned)
        .collect()
}

/// Contract M5 S16 on real snapshots: for every ordered pair of the three fixtures,
/// `apply(dump lines of A, diff(A, B))` is B's dump lines; `rb diff A B --json` counts the same
/// rows; and each fixture against its own forced rebuild is an empty diff.
#[test]
fn diffs_of_fixtures_apply_and_rebuilds_are_empty() {
    let Some(url) = db_url() else { return };
    let pid = std::process::id();
    let fixtures = ["ts-express-prisma", "java-spring-jpa", "polyglot-monorepo"];
    let repos: Vec<String> = fixtures
        .iter()
        .map(|f| format!("t_dapply_{}_{pid}", f.replace('-', "_")))
        .collect();
    let _cleanup = Cleanup {
        url: &url,
        repos: repos.clone(),
    };
    let mut ids = Vec::new();
    for (f, r) in fixtures.iter().zip(&repos) {
        let dir = fixture_repo(f);
        let path = dir.path().to_str().unwrap();
        rb(&url, &["index", path, "--repo", r, "--no-scip"]);
        rb(&url, &["index", path, "--repo", r, "--no-scip", "--force"]);
        ids.push(live_id(&url, r));
        // The rebuild's diff with its parent (the first build) is empty.
        let v: serde_json::Value =
            serde_json::from_str(&rb(&url, &["diff", "--repo", r, "--json"])).unwrap();
        let res = &v["result"];
        for (section, parts) in [
            ("nodes", &["added", "removed", "changed"][..]),
            ("edges", &["added", "removed", "changed", "evidence_only"]),
        ] {
            for part in parts {
                assert_eq!(res[section][part]["total"], 0, "{f} {section}.{part}");
            }
        }
        for summary in ["endpoints", "table_access", "service_dependencies"] {
            assert_eq!(
                res["summary"][summary]["added"],
                serde_json::json!([]),
                "{f}"
            );
            assert_eq!(
                res["summary"][summary]["removed"],
                serde_json::json!([]),
                "{f}"
            );
        }
    }
    let mut store = Store::connect(&url).unwrap();
    let graphs: Vec<rb_model::Graph> = ids
        .iter()
        .map(|&id| store.load_graph(id).unwrap())
        .collect();
    let dumps: Vec<String> = ids
        .iter()
        .map(|id| rb(&url, &["dump", "--snapshot", &id.to_string()]))
        .collect();
    for a in 0..3 {
        for b in 0..3 {
            let d = rb_model::diff::diff(&graphs[a], &graphs[b]);
            let lines_a = row_lines(&dumps[a]);
            let refs: Vec<&str> = lines_a.iter().map(String::as_str).collect();
            assert_eq!(
                rb_model::diff::apply(&refs, &d).unwrap(),
                row_lines(&dumps[b]),
                "{} -> {}",
                fixtures[a],
                fixtures[b]
            );
            assert_eq!(a == b, d.is_empty());
            let v: serde_json::Value = serde_json::from_str(&rb(
                &url,
                &[
                    "diff",
                    &ids[a].to_string(),
                    &ids[b].to_string(),
                    "--repo",
                    &repos[b],
                    "--json",
                ],
            ))
            .unwrap();
            let total = |s: &str, p: &str| v["result"][s][p]["total"].as_u64().unwrap() as usize;
            assert_eq!(
                (
                    total("nodes", "added"),
                    total("nodes", "removed"),
                    total("nodes", "changed"),
                    total("edges", "added"),
                    total("edges", "removed"),
                    total("edges", "changed"),
                    total("edges", "evidence_only"),
                ),
                (
                    d.nodes_added.len(),
                    d.nodes_removed.len(),
                    d.nodes_changed.len(),
                    d.edges_added.len(),
                    d.edges_removed.len(),
                    d.edges_changed.len(),
                    d.edges_evidence_only.len(),
                ),
                "{} -> {}",
                fixtures[a],
                fixtures[b]
            );
        }
    }
}

/// Contract M3a R9-R11: `rb callers` and `rb impact` goldens on every fixture, including the
/// must-not-exist cases (a same-named method in an unrelated class).
#[test]
fn code_question_goldens() {
    let Some(url) = db_url() else { return };
    // (fixture, [(golden file, rb arguments)])
    type Questions<'a> = &'a [(&'a str, &'a [&'a str])];
    let cases: &[(&str, Questions)] = &[
        (
            "java-spring-jpa",
            &[
                (
                    "callers-authenticate.txt",
                    &["callers", "DefaultAuthService.authenticate", "--depth", "2"],
                ),
                (
                    "callers-legacy.txt",
                    &["callers", "LegacyAuth.authenticate"],
                ),
                (
                    "impact-authservice.txt",
                    &["impact", "AuthService#authenticate()."],
                ),
                ("impact-users.txt", &["impact", "public.users"]),
                // An implementation: its dispatch callers are affected (CHA).
                (
                    "impact-default-authenticate.txt",
                    &["impact", "DefaultAuthService.authenticate"],
                ),
            ],
        ),
        (
            "ts-express-prisma",
            &[
                (
                    "callers-find-user.txt",
                    &["callers", "findUserByEmail().", "--depth", "3"],
                ),
                ("callers-cache.txt", &["callers", "Cache.findUserByEmail"]),
                ("impact-create-session.txt", &["impact", "createSession"]),
            ],
        ),
        (
            "polyglot-monorepo",
            &[
                (
                    "callers-send-code.txt",
                    &["callers", "MfaClient.sendCode", "--depth", "2"],
                ),
                ("impact-login-pick.txt", &["impact", "login", "--pick", "1"]),
                // Contract M4a S21: the Feign method's caller handles the login endpoint.
                ("impact-send-code.txt", &["impact", "MfaClient.sendCode"]),
            ],
        ),
    ];
    for (fixture, questions) in cases {
        let repo = fixture_repo(fixture);
        let name = format!(
            "t_code_{}_{}",
            fixture.replace('-', "_"),
            std::process::id()
        );
        let _cleanup = Cleanup {
            url: &url,
            repos: vec![name.clone()],
        };
        rb(
            &url,
            &[
                "index",
                repo.path().to_str().unwrap(),
                "--repo",
                &name,
                "--no-scip",
            ],
        );
        for (golden_name, args) in *questions {
            let mut args: Vec<&str> = args.to_vec();
            args.extend(["--repo", &name]);
            golden(fixture, golden_name, &rb(&url, &args));
        }
    }
    // An ambiguous name lists candidates and exits with an error; it never picks one.
    let repo = fixture_repo("polyglot-monorepo");
    let name = format!("t_code_pick_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    rb(
        &url,
        &[
            "index",
            repo.path().to_str().unwrap(),
            "--repo",
            &name,
            "--no-scip",
        ],
    );
    let err = rb_fails(&url, &["impact", "login", "--repo", &name]);
    assert!(err.contains("matches 2 symbols; pass --pick N"), "{err}");
    assert!(
        err.contains("1. login()") && err.contains("2. AuthController#login()"),
        "{err}"
    );
}

/// The edges of `dump` that `keep(kind, attributes)` selects, one per line in dump order:
/// `<KIND> <src key> -> <dst key> [<certainty>] <attributes>`, the precision-guard grammar.
fn edge_lines(dump: &str, keep: impl Fn(&str, &serde_json::Value) -> bool) -> String {
    let mut out = String::new();
    for line in dump.lines().skip(1) {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        if row[0] != "E" || !keep(row[3].as_str().unwrap_or_default(), &row[8]) {
            continue;
        }
        let rank = usize::try_from(row[7].as_i64().unwrap() - 1).unwrap();
        out.push_str(&format!(
            "{} {} -> {} [{}] {}\n",
            row[3].as_str().unwrap_or_default(),
            row[2].as_str().unwrap_or_default(),
            row[5].as_str().unwrap_or_default(),
            CERTAINTIES[rank],
            row[8]
        ));
    }
    out
}

/// The line of `rb status` that starts with `prefix` (indentation ignored), newline-terminated.
fn status_line(status: &str, prefix: &str) -> String {
    let line = status
        .lines()
        .find(|l| l.trim_start().starts_with(prefix))
        .unwrap_or_default();
    format!("{line}\n")
}

/// Contract M4b S7, S9, S10, S11, S13, S18 and S24 on `java-spring-jpa` (`--no-scip`): the DI
/// and RTA status lines, the full card, `KafkaAuditSink#record()`'s dispatch caller pruned
/// under `rta` and shown under `cha`, and the orphan `LegacyAuth#authenticate()` leaves behind,
/// in text and JSON.
#[test]
fn spring_di_rta_and_orphan_goldens() {
    let Some(url) = db_url() else { return };
    let fixture = "java-spring-jpa";
    let repo = fixture_repo(fixture);
    let name = format!("t_di_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    rb(
        &url,
        &[
            "index",
            repo.path().to_str().unwrap(),
            "--repo",
            &name,
            "--no-scip",
        ],
    );
    let status = rb(&url, &["status", "--repo", &name]);
    golden(fixture, "status-di.txt", &status_line(&status, "di:"));
    golden(fixture, "status-rta.txt", &status_line(&status, "rta:"));
    golden(
        fixture,
        "card.txt",
        &rb(&url, &["service", ".", "--repo", &name]),
    );

    // The only caller of `KafkaAuditSink#record()` is a dispatch step from `AuditSink#record()`,
    // whose implementation is never instantiated: pruned and counted under `rta` (opt-in,
    // amendment 21), listed under `cha` (the default).
    let kafka = rb(
        &url,
        &[
            "callers",
            "KafkaAuditSink.record",
            "--precision",
            "rta",
            "--repo",
            &name,
        ],
    );
    assert!(
        !kafka.contains("DefaultAuthService#authenticate()")
            && kafka.contains("rta: 1 dispatch step")
            && kafka.contains("precision=rta (opt-in"),
        "{kafka}"
    );
    golden(fixture, "callers-kafka-record.txt", &kafka);
    let kafka_cha = rb(
        &url,
        &[
            "callers",
            "KafkaAuditSink.record",
            "--precision",
            "cha",
            "--repo",
            &name,
        ],
    );
    assert!(
        kafka_cha.contains("DefaultAuthService#authenticate()"),
        "{kafka_cha}"
    );
    golden(fixture, "callers-kafka-record-cha.txt", &kafka_cha);
    // The default is `cha`.
    assert_eq!(
        rb(&url, &["callers", "KafkaAuditSink.record", "--repo", &name]),
        kafka_cha
    );
    // `LogAuditSink` is instantiated by the `@Bean` method: its dispatch caller stays under both.
    for precision in ["rta", "cha"] {
        let log = rb(
            &url,
            &[
                "callers",
                "LogAuditSink.record",
                "--precision",
                precision,
                "--repo",
                &name,
            ],
        );
        assert!(
            log.contains("DefaultAuthService#authenticate()") && !log.contains("pruned"),
            "{precision}: {log}"
        );
    }

    // Removing the legacy endpoint method leaves its digest helper without a use.
    golden(
        fixture,
        "impact-legacy-orphans.txt",
        &rb(
            &url,
            &["impact", "LegacyAuth.authenticate", "--repo", &name],
        ),
    );
    golden(
        fixture,
        "impact-legacy-orphans.json",
        &stable(
            &rb(
                &url,
                &[
                    "impact",
                    "LegacyAuth.authenticate",
                    "--json",
                    "--repo",
                    &name,
                ],
            ),
            &name,
            live_id(&url, &name),
        ),
    );
}

/// Contract M3b S3, S4, S6, S7, S9, S12: the TypeScript fixture indexed with scip-typescript in
/// its container. Needs Docker (and the network for the fetch phase); skipped otherwise.
#[test]
fn scip_ts_express_prisma() {
    let Some(url) = db_url() else { return };
    if let Err(reason) = rb_scip::docker_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let fixture = "ts-express-prisma";
    let repo = fixture_repo(fixture);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_scip_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    // The index cache lives in the database and outlives test runs, so the first run may
    // already be served from it; the second run must be.
    let first = rb(&url, &["index", path, "--repo", &name]);
    assert!(first.contains("scip: . scip-typescript ok"), "{first}");
    // Amendment 33 (S1): scip-typescript emits UTF-16 columns and no document text, so the
    // decoder converts them with the blob. The call to `createSession` sits after a BMP and an
    // astral character on its line; its exact edge must land on the byte span Tree-sitter
    // reports for the call, which the join only matches when the conversion happened.
    let auth_ts = std::fs::read_to_string(
        workspace_root().join("fixtures/ts-express-prisma/src/routes/auth.ts"),
    )
    .unwrap();
    let (line, text) = auth_ts
        .lines()
        .enumerate()
        .find(|(_, l)| l.contains("createSession(user.id)"))
        .expect("the non-ASCII call line");
    assert!(
        text.contains("héllo 😀"),
        "the fixture line lost its non-ASCII text: {text}"
    );
    let col = text.find("createSession(").unwrap() as i64;
    let span = serde_json::json!([
        line as i64,
        col,
        line as i64,
        col + "createSession".len() as i64
    ]);
    let evidence = serde_json::json!(["src/routes/auth.ts", span]);
    let create_session = "scip-typescript npm auth-api 1.0.0 src/db/`users.ts`/createSession().";

    // S6, S7: exact edges between SCIP keys, and no heuristic code edge in the project.
    golden(
        fixture,
        "scip-callers-find-user.txt",
        &rb(
            &url,
            &[
                "callers",
                "findUserByEmail().",
                "--depth",
                "3",
                "--repo",
                &name,
            ],
        ),
    );
    golden(
        fixture,
        "scip-impact-create-session.txt",
        &rb(&url, &["impact", "createSession", "--repo", &name]),
    );
    let dump = rb(&url, &["dump", "--repo", &name]);
    let mut exact_calls = 0;
    let mut non_ascii_call = false;
    for line in dump.lines() {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        let kind = row
            .get(3)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if matches!(kind, "CALLS" | "REFERENCES" | "IMPLEMENTS") {
            assert_eq!(row[6].as_str(), Some("scip"), "{line}");
            assert_eq!(
                row[7].as_i64(),
                Some(rb_model::Certainty::Exact.rank().into()),
                "{line}"
            );
            exact_calls += usize::from(kind == "CALLS");
            if kind == "CALLS"
                && row[5] == create_session
                && row
                    .as_array()
                    .unwrap()
                    .last()
                    .unwrap()
                    .as_array()
                    .is_some_and(|evs| evs.contains(&evidence))
            {
                non_ascii_call = true;
            }
        }
    }
    assert!(exact_calls >= 2, "{dump}");
    assert!(
        non_ascii_call,
        "no exact call into createSession() at {evidence} in {dump}"
    );
    let status = rb(&url, &["status", "--repo", &name]);
    assert!(status.contains("unconverted 0"), "{status}");
    // S6: the SCIP key itself and the `path:line` form name the same symbol as the suffix; the
    // bare name still lists both `findUserByEmail` methods rather than picking one.
    let key = "scip-typescript npm auth-api 1.0.0 src/db/`users.ts`/findUserByEmail().";
    let by_key = rb(&url, &["callers", key, "--repo", &name]);
    assert_eq!(
        by_key,
        rb(&url, &["callers", "findUserByEmail().", "--repo", &name])
    );
    assert_eq!(
        by_key,
        rb(&url, &["callers", "src/db/users.ts:8", "--repo", &name])
    );
    let err = rb_fails(&url, &["callers", "findUserByEmail", "--repo", &name]);
    assert!(err.contains("matches 2 symbols"), "{err}");
    assert!(
        rb(
            &url,
            &[
                "find",
                "findUserByEmail",
                "--kind",
                "symbol",
                "--repo",
                &name
            ]
        )
        .contains(key)
    );

    // S9: coverage per project and the exact share.
    golden(fixture, "scip-status.txt", &scip_status_lines(&url, &name));
    // S8: the packages the service's exact references name, from the index's symbols.
    golden(
        fixture,
        "scip-card.txt",
        &rb(&url, &["service", ".", "--repo", &name]),
    );
    assert!(
        dump.contains("\"package\",\"npm/@prisma/client@5.22.0\""),
        "{dump}"
    );

    // S4, S12: unchanged inputs are served from the cache, and the dump is byte-identical.
    let second = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(
        second.contains("scip: . scip-typescript ok (cached"),
        "{second}"
    );
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    // Amendments 20 and 32 (S12): `--no-cache` bypasses the index cache's reads, so the
    // container runs again and its outcome replaces the row; two fresh runs of one commit dump
    // byte-identically, since no run time enters the graph.
    for _ in 0..2 {
        let fresh = rb(
            &url,
            &["index", path, "--repo", &name, "--force", "--no-cache"],
        );
        assert!(
            fresh.contains("scip: . scip-typescript ok ") && !fresh.contains("(cached"),
            "{fresh}"
        );
        assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    }
}

/// Contract M3b amendment 17: whether SCIP ran is part of the snapshot's tool configuration.
/// A snapshot indexed while Docker was hidden from `rb` (`PATH=/usr/bin:/bin`) is not "up to
/// date" for a `--no-scip` run, nor for a run with Docker; each configuration is up to date
/// only with itself.
#[test]
fn scip_availability_is_part_of_the_tool_configuration() {
    let Some(url) = db_url() else { return };
    // No SCIP job in this repository: only the configuration differs between the runs.
    let repo = repo_from(&[("tool.py", "def f():\n    pass\n"), ("README.md", "x\n")]);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_scipmode_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    let hidden = Some("/usr/bin:/bin");
    let first = rb_with_path(&url, &["index", path, "--repo", &name], hidden);
    assert!(first.contains("snapshot"), "{first}");
    let again = rb_with_path(&url, &["index", path, "--repo", &name], hidden);
    assert!(again.contains("up to date"), "{again}");

    let off = rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    assert!(!off.contains("up to date"), "unavailable ≠ off: {off}");
    let off_again = rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    assert!(off_again.contains("up to date"), "{off_again}");

    if rb_scip::docker_available().is_ok() {
        let on = rb(&url, &["index", path, "--repo", &name]);
        assert!(!on.contains("up to date"), "off ≠ on: {on}");
        let unavailable = rb_with_path(&url, &["index", path, "--repo", &name], hidden);
        assert!(
            !unavailable.contains("up to date"),
            "on ≠ unavailable: {unavailable}"
        );
    } else {
        eprintln!("skipping the on/unavailable half: docker is not available");
    }
}

/// The `code:` and `scip:` lines of `rb status`, as printed: nothing in them depends on the
/// run (the container's time is printed by `rb index` only; amendment 32).
fn scip_status_lines(url: &str, name: &str) -> String {
    let status = rb(url, &["status", "--repo", name]);
    let lines: Vec<&str> = status
        .lines()
        .filter(|l| {
            let l = l.trim_start();
            l.starts_with("code:") || l.starts_with("scip:")
        })
        .collect();
    format!("{}\n", lines.join("\n"))
}

/// A `PATH` whose first `docker` is a shell shim: `refuse` is a shell condition over the
/// client's arguments (`$1`, `$2`, …, `$*`); when it holds the shim prints `message` on stderr
/// and exits with `code`, otherwise the real client runs.
fn docker_shim(dir: &Path, refuse: &str, message: &str, code: i32) -> String {
    let inherited = std::env::var("PATH").unwrap();
    let real_docker = inherited
        .split(':')
        .map(|dir| Path::new(dir).join("docker"))
        .find(|p| p.is_file())
        .expect("docker on PATH");
    let shim = dir.join("docker");
    std::fs::write(
        &shim,
        format!(
            "#!/bin/sh\nif {refuse}; then\n  echo '{message}' >&2\n  exit {code}\nfi\nexec '{}' \"$@\"\n",
            real_docker.display()
        ),
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    format!("{}:{inherited}", dir.display())
}

/// Contract M3b amendments 29, 44 and 48: the scip-java image's identity is part of the
/// snapshot's tool configuration. With a `docker` shim on `PATH` whose `image inspect` fails
/// for `rb-scip-java:0.13.1` only (everything else reaches the real client), the Java fixture
/// indexes with scip-java `unsupported` and a heuristic graph, and `tool_versions.scip_java`
/// is `absent`; the same commit with the real `docker` is not "up to date". Deleting
/// `scip_java` from `tool_versions` makes this test fail: `scip_typescript` is the pinned
/// digest in both runs, so nothing else distinguishes them.
#[test]
fn scip_image_identity_is_part_of_the_tool_configuration() {
    let Some(url) = db_url() else { return };
    if let Err(reason) = scip_java_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let repo = fixture_repo("java-spring-jpa");
    let path = repo.path().to_str().unwrap();
    let name = format!("t_scipimg_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    let shim_dir = tempfile::tempdir().unwrap();
    let shimmed = docker_shim(
        shim_dir.path(),
        &format!(
            "[ \"$1\" = image ] && [ \"$2\" = inspect ] && case \" $* \" in *\" {} \"*) true ;; *) false ;; esac",
            rb_scip::jobs::SCIP_JAVA_IMAGE
        ),
        "shim: image inspect refused",
        1,
    );
    let absent = rb_with_path(&url, &["index", path, "--repo", &name], Some(&shimmed));
    assert!(absent.contains("scip: . scip-java unsupported"), "{absent}");
    let mut store = Store::connect(&url).unwrap();
    let tools = store.live_snapshot(&name).unwrap().unwrap().tool_versions;
    assert_eq!(tools["scip_java"], "absent", "{tools}");
    assert_eq!(
        tools["scip_typescript"],
        rb_scip::jobs::SCIP_TYPESCRIPT_IMAGE,
        "{tools}"
    );
    // Contract M6d S1 (evaluator pass 1, E1-1): the go and python identities
    // ride the same record (the java fixture has neither job).
    assert_eq!(
        tools["scip_python"],
        rb_scip::jobs::SCIP_PYTHON_IMAGE,
        "{tools}"
    );
    // Amendment 44: an `unsupported` job did not finish, so with the image still absent the
    // snapshot is rebuilt (cheaply: facts and index caches are warm), not "up to date".
    let again = rb_with_path(&url, &["index", path, "--repo", &name], Some(&shimmed));
    assert!(
        again.contains("rebuilding: 1 SCIP job did not finish last time")
            && again.contains("scip: . scip-java unsupported"),
        "{again}"
    );
    let present = rb(&url, &["index", path, "--repo", &name]);
    assert!(
        !present.contains("up to date") && present.contains("scip: . scip-java ok"),
        "{present}"
    );
    let tools = store.live_snapshot(&name).unwrap().unwrap().tool_versions;
    assert!(
        tools["scip_java"]
            .as_str()
            .is_some_and(|id| id.starts_with("sha256:")),
        "{tools}"
    );
    assert!(
        tools["scip_go"]
            .as_str()
            .is_some_and(|id| id.starts_with("sha256:")),
        "{tools}"
    );
    assert!(
        rb(&url, &["index", path, "--repo", &name]).contains("up to date"),
        "every job finished"
    );
}

/// Contract M3b amendment 44: a snapshot whose SCIP jobs did not all finish is never "up to
/// date". With a `docker` shim that refuses `run` (exit 125), the Java fixture's job (run with
/// `--no-cache`, since the index cache outlives test runs) is `environment` and nothing is
/// cached; the next plain run with the real `docker` rebuilds and the job is `ok`. A
/// `--scip-timeout-secs 1` run leaves a `timeout`; the next default-budget run rebuilds and
/// serves the index from the cache.
#[test]
fn scip_unfinished_jobs_are_not_up_to_date() {
    let Some(url) = db_url() else { return };
    if let Err(reason) = scip_java_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let repo = fixture_repo("java-spring-jpa");
    let path = repo.path().to_str().unwrap();
    let name = format!("t_scipunf_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    let shim_dir = tempfile::tempdir().unwrap();
    let shimmed = docker_shim(
        shim_dir.path(),
        "[ \"$1\" = run ]",
        "shim: docker run refused for /Users/nobody/sock",
        125,
    );
    let refused = rb_with_path(
        &url,
        &["index", path, "--repo", &name, "--no-cache"],
        Some(&shimmed),
    );
    assert!(
        refused.contains("scip: . scip-java environment")
            && refused.contains(": shim: docker run refused for <host path>"),
        "{refused}"
    );
    // Amendment 49: the stored reason names no host path. Since contract M5 S21 the first
    // `docker run` a job makes starts its egress proxy, so that is the run refused.
    let dump = rb(&url, &["dump", "--repo", &name]);
    assert!(
        dump.contains(
            "\"reason\":\"egress proxy: docker run: shim: docker run refused for <host path>\""
        ) && !dump.contains("/Users/nobody"),
        "{dump}"
    );
    let rebuilt = rb(&url, &["index", path, "--repo", &name]);
    assert!(
        rebuilt.contains("rebuilding: 1 SCIP job did not finish last time")
            && rebuilt.contains("scip: . scip-java ok"),
        "{rebuilt}"
    );
    assert!(
        rb(&url, &["index", path, "--repo", &name]).contains("up to date"),
        "every job finished"
    );

    let timed_out = rb(
        &url,
        &[
            "index",
            path,
            "--repo",
            &name,
            "--force",
            "--no-cache",
            "--scip-timeout-secs",
            "1",
        ],
    );
    assert!(
        timed_out.contains("scip: . scip-java timeout") && timed_out.contains("1 s budget)"),
        "{timed_out}"
    );
    let rebuilt = rb(&url, &["index", path, "--repo", &name]);
    assert!(
        rebuilt.contains("rebuilding: 1 SCIP job did not finish last time")
            && rebuilt.contains("scip: . scip-java ok (cached"),
        "{rebuilt}"
    );
}

/// Contract M5 S21, the probe suite: from a container started exactly as a fetch job is
/// (`docker_args` of a `Network::Fetch` run with the proxy's environment, in the scip-java image,
/// which has `curl`), behind a proxy started as `run_job` starts one, with the built-in
/// allow-list: an allow-listed registry answers over `CONNECT`; a host off the list, a direct IP
/// through the proxy and plain HTTP to an allow-listed host are refused by the proxy (its log
/// names each); direct TCP to an outside address and to the Docker host's gateway fails, DNS
/// resolves no external name, and UDP to a public resolver has no route. The fetch network is
/// `--internal`. Needs Docker, both local images and the network; skipped otherwise.
#[test]
fn scip_egress_probes_from_a_fetch_container() {
    use rb_scip::Runner as _;
    let Some(_url) = db_url() else { return };
    if let Err(reason) = scip_java_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let runner = rb_scip::DockerRunner::default();
    if runner.image_id(rb_scip::egress::PROXY_IMAGE).is_none() {
        eprintln!(
            "skipping: {} is not built; see docker/egress-proxy/README.md",
            rb_scip::egress::PROXY_IMAGE
        );
        return;
    }
    let pid = std::process::id();
    let dir = workspace_root().join(format!("var/scip/probe-{pid}"));
    struct Removed<'a>(&'a Path, &'a rb_scip::DockerRunner);
    impl Drop for Removed<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0);
            let _ = self.1.egress_leftovers(true);
        }
    }
    let _removed = Removed(&dir, &runner);
    let egress = rb_scip::Egress::default();
    egress.write_config(&dir.join("proxy")).unwrap();
    std::fs::create_dir_all(dir.join("work")).unwrap();
    let spec = rb_scip::ProxySpec {
        name: format!("rb-egress-probe-{pid}"),
        image: rb_scip::egress::PROXY_IMAGE.into(),
        config_dir: dir.join("proxy"),
    };
    let address = runner.start_proxy(&spec).unwrap();
    let script = r#"exec 1>&2
code() { curl -sS -o /dev/null -w '%{http_code}' --max-time 30 "$@" 2>/dev/null; echo " exit=$?"; }
echo "allowed $(code https://repo.maven.apache.org/maven2/)"
echo "gomod $(code https://proxy.golang.org/github.com/go-chi/chi/v5/@latest)"
echo "gosum $(code https://sum.golang.org/)"
echo "pypi $(code https://pypi.org/pypi/requests/json)"
echo "files $(code https://files.pythonhosted.org/)"
echo "testpypi $(code https://test.pypi.org/simple/)"
echo "gcs $(code https://storage.googleapis.com/)"
echo "plugins $(code https://plugins.gradle.org/)"
echo "distrib $(code https://services.gradle.org/)"
echo "host $(code https://example.com/)"
echo "plain $(code -x "$http_proxy" http://repo.maven.apache.org/maven2/)"
echo "ip $(code https://1.1.1.1/)"
echo "direct $(code --noproxy '*' --connect-timeout 5 https://1.1.1.1/)"
echo "gateway $(code --noproxy '*' --connect-timeout 5 http://192.168.65.254:55433/)"
getent hosts example.com > /dev/null; echo "dns exit=$?"
timeout 5 bash -c 'echo x > /dev/udp/8.8.8.8/53' 2> /dev/null; echo "udp exit=$?"
"#;
    let probe = rb_scip::ContainerRun {
        name: Some(format!("rb-scip-probe-{pid}-fetch")),
        image: rb_scip::jobs::SCIP_JAVA_IMAGE.into(),
        platform: None,
        workdir: dir.join("work"),
        mount_at: "/sources".into(),
        cwd: String::new(),
        caches: Vec::new(),
        readonly: Vec::new(),
        env: rb_scip::egress::proxy_env(&address),
        network: rb_scip::Network::Fetch,
        cpus: 1.0,
        memory: "1g".into(),
        timeout: std::time::Duration::from_secs(300),
        entrypoint: Some("bash".into()),
        args: vec!["-c".into(), script.into()],
    };
    let args = rb_scip::runner::docker_args(&probe).join(" ");
    assert!(
        args.contains("--network rb-scip-fetch --dns 127.0.0.1"),
        "{args}"
    );
    let outcome = runner.run(&probe).unwrap();
    let log = runner.stop_proxy(&spec.name);
    let internal = Command::new("docker")
        .args([
            "network",
            "inspect",
            "--format",
            "{{.Internal}}",
            "rb-scip-fetch",
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&internal.stdout).trim(), "true");
    let result = |probe: &str| -> String {
        outcome
            .stderr_tail
            .lines()
            .find_map(|l| l.strip_prefix(&format!("{probe} ")))
            .unwrap_or_else(|| panic!("no {probe} line:\n{}", outcome.stderr_tail))
            .to_owned()
    };
    eprintln!("egress probes:\n{}", outcome.stderr_tail);
    assert!(outcome.ok(), "{}", outcome.stderr_tail);
    assert_eq!(result("allowed"), "200 exit=0");
    assert_eq!(result("gomod"), "200 exit=0");
    assert!(
        result("gosum").starts_with("000 exit=") && result("gosum") != "000 exit=0",
        "gosum: {}",
        result("gosum")
    );
    assert_eq!(result("pypi"), "200 exit=0");
    // Contract M6d S4: the file host answers (its root is a 404), while the
    // same-org test index stays refused. S11: the Go proxy's storage bucket
    // answers (its root is a 400).
    assert_eq!(result("files"), "404 exit=0");
    assert_eq!(result("gcs"), "400 exit=0");
    // Contract M6e S2: the Gradle plugin portal answers through the proxy.
    // services.gradle.org (the distribution host) stays refused: nothing at
    // runtime dials it (the wrapper is never used), and a build that did
    // would fail closed with a named refusal.
    assert_eq!(result("plugins"), "200 exit=0");
    assert!(
        result("distrib").starts_with("000 exit=") && result("distrib") != "000 exit=0",
        "distrib: {}",
        result("distrib")
    );
    assert!(
        result("testpypi").starts_with("000 exit=") && result("testpypi") != "000 exit=0",
        "testpypi: {}",
        result("testpypi")
    );
    for refused in ["host", "ip", "direct", "gateway"] {
        let r = result(refused);
        assert!(
            r.starts_with("000 exit=") && r != "000 exit=0",
            "{refused}: {r}"
        );
    }
    assert_eq!(result("plain"), "403 exit=0");
    assert_ne!(result("dns"), "exit=0");
    assert_ne!(result("udp"), "exit=0");
    assert_eq!(
        rb_scip::egress::refusals(&log),
        [
            "1.1.1.1:443",
            "example.com:443",
            "http://repo.maven.apache.org/maven2/",
            // Contract M6e S2: the distribution host stays refused.
            "services.gradle.org:443",
            // Contract M6d S3: the checksum database stays refused.
            "sum.golang.org:443",
            // Contract M6d S4: the same-org test index stays refused.
            "test.pypi.org:443"
        ],
        "{log}"
    );
    assert!(log.contains("CONNECT repo.maven.apache.org:443"), "{log}");
    assert!(!log.contains("CONNECT 192.168.65.254"), "{log}");
}

/// Docker answering and the local scip-go image built (docker/scip-go/README.md); the
/// reason to skip otherwise. The image is never built implicitly.
fn scip_go_available() -> Result<(), String> {
    use rb_scip::Runner as _;
    rb_scip::docker_available()?;
    let image = rb_scip::jobs::SCIP_GO_IMAGE;
    rb_scip::DockerRunner::default()
        .image_id(image)
        .map(|_| ())
        .ok_or_else(|| format!("{image} is not built; see docker/scip-go/README.md"))
}

/// Contract M6d S9: the chi/SQL fixture. Nested `Route` prefixes, `Group`,
/// `With` chains and verbs become endpoints; `database/sql` sites become
/// `QUERIES`; the two implicit `Payer` implementations get `IMPLEMENTS`
/// edges (method and type) without any declared heritage (guards); call
/// dispatch through the interface stays unmatched (RTA through Go interfaces
/// is future work, recorded as residue). The `go.sum`-pinned fetch makes the
/// job reproducible.
#[test]
fn scip_go_chi_sqlx() {
    let Some(url) = db_url() else { return };
    if let Err(reason) = scip_go_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let fixture = "go-chi-sqlx";
    let repo = fixture_repo(fixture);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_scipgo_{}", std::process::id());
    let other = format!("t_scipgo2_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone(), other.clone()],
    };
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    golden(fixture, "dump.jsonl", &rb(&url, &["dump", "--repo", &name]));

    let started = std::time::Instant::now();
    let first = rb(&url, &["index", path, "--repo", &name, "--force"]);
    eprintln!("scip_go_chi_sqlx: first index {:.1?}", started.elapsed());
    assert!(
        first.contains("scip: . scip-go ok") && !first.contains("unpinned"),
        "{first}"
    );
    let dump = rb(&url, &["dump", "--repo", &name]);
    let failures = guard_failures(fixture, &dump);
    if !failures.is_empty() {
        eprintln!("scip_go_chi_sqlx guard failures:\n{}", failures.join("\n"));
    }
    golden(fixture, "scip-status.txt", &scip_status_lines(&url, &name));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    golden(
        fixture,
        "scip-callers-checkout.txt",
        &rb(&url, &["callers", "Checkout", "--repo", &name]),
    );
    let package = "scip-go gomod github.com/acme/shop . ";
    assert!(
        assert_exact_code_edges(&dump, package, "", ".") >= 7,
        "{dump}"
    );
    // The manifest and SCIP chi edges share one `go/` node (contract M6d S5).
    assert_eq!(
        dump.lines()
            .filter(|l| l.contains("[\"N\",\"package\",\"go/github.com/go-chi/chi/v5@v5.3.2\""))
            .count(),
        1,
        "{dump}"
    );
    assert!(!dump.contains("gomod/github.com/go-chi"), "{dump}");

    let second = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(second.contains("scip: . scip-go ok (cached"), "{second}");
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    let fresh = rb(
        &url,
        &["index", path, "--repo", &name, "--force", "--no-cache"],
    );
    assert!(
        fresh.contains("scip: . scip-go ok ") && !fresh.contains("(cached"),
        "{fresh}"
    );
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    rb(&url, &["index", path, "--repo", &other]);
    assert_eq!(rb(&url, &["dump", "--repo", &other]), dump);
}

/// Contract M6d S9: the FastAPI/SQLAlchemy fixture. Decorators, prefixes
/// and includes become endpoints; `__tablename__` models and f-string/literal
/// SQL become `QUERIES`; intra-repo imports resolve exact both ways;
/// `uv.lock` pins versions while the non-member client service stays `?`;
/// session/plain `requests`/`httpx` sites stitch cross-service (guards).
#[test]
fn scip_python_fastapi_sqlalchemy() {
    let Some(url) = db_url() else { return };
    if let Err(reason) = rb_scip::docker_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let fixture = "python-fastapi-sqlalchemy";
    let repo = fixture_repo(fixture);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_scippy_{}", std::process::id());
    let other = format!("t_scippy2_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone(), other.clone()],
    };
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    golden(fixture, "dump.jsonl", &rb(&url, &["dump", "--repo", &name]));

    let started = std::time::Instant::now();
    let first = rb(&url, &["index", path, "--repo", &name, "--force"]);
    eprintln!(
        "scip_python_fastapi_sqlalchemy: first index {:.1?}",
        started.elapsed()
    );
    for line in [
        "scip: client scip-python ok",
        "scip: services/shop scip-python ok",
    ] {
        assert!(first.contains(line), "{line} in {first}");
    }
    assert!(first.contains("unpinned"), "{first}");
    let dump = rb(&url, &["dump", "--repo", &name]);
    let failures = guard_failures(fixture, &dump);
    if !failures.is_empty() {
        eprintln!(
            "scip_python_fastapi_sqlalchemy guard failures:\n{}",
            failures.join("\n")
        );
    }
    golden(fixture, "scip-status.txt", &scip_status_lines(&url, &name));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    golden(
        fixture,
        "scip-callers-active-users.txt",
        &rb(&url, &["callers", "active_users", "--repo", &name]),
    );
    let package = "scip-python python shop 0.1.0 ";
    assert!(
        assert_exact_code_edges(&dump, package, "services/shop/", "services/shop") >= 2,
        "{dump}"
    );

    let second = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(
        second.contains("scip: services/shop scip-python ok (cached"),
        "{second}"
    );
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    let fresh = rb(
        &url,
        &["index", path, "--repo", &name, "--force", "--no-cache"],
    );
    assert!(
        fresh.contains("scip: services/shop scip-python ok ") && !fresh.contains("(cached"),
        "{fresh}"
    );
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    rb(&url, &["index", path, "--repo", &other]);
    assert_eq!(rb(&url, &["dump", "--repo", &other]), dump);
}
/// Docker answering and the local scip-java image built (docker/scip-java/README.md); the
/// reason to skip otherwise. The image is never built implicitly.
fn scip_java_available() -> Result<(), String> {
    use rb_scip::Runner as _;
    rb_scip::docker_available()?;
    let image = rb_scip::jobs::SCIP_JAVA_IMAGE;
    rb_scip::DockerRunner::default()
        .image_id(image)
        .map(|_| ())
        .ok_or_else(|| format!("{image} is not built; see docker/scip-java/README.md"))
}

/// Contract M3b S7 (as restated by amendment 54) for the SCIP-ok project at `root`: every
/// `CALLS`, `REFERENCES` and `IMPLEMENTS` edge of `dump` whose source or target key starts with
/// `key_prefix` (the project's SCIP package, `""` for every edge) or names a file under
/// `documented` (a directory the index has documents for) through a heuristic key
/// (`` `<documented> ``) is `scip`/`exact`, and the heuristic `Symbol` keys naming a file under
/// `documented` number exactly the project's `definitions_local` (the definitions scip-java
/// keys as document-local symbols). Returns the number of exact `CALLS`.
fn assert_exact_code_edges(dump: &str, key_prefix: &str, documented: &str, root: &str) -> usize {
    let mut exact_calls = 0;
    let heuristic_in_root = format!("rb-heuristic . . . `{documented}");
    let mut heuristic_symbols = 0;
    let mut definitions_local = None;
    for line in dump.lines() {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        if row[0] == "N" && row[1] == "symbol" {
            heuristic_symbols += usize::from(
                row[2]
                    .as_str()
                    .is_some_and(|k| k.starts_with(&heuristic_in_root)),
            );
            continue;
        }
        if row[0] == "D" && row[1] == "scip_status" && row[2] == root {
            definitions_local = row[4]["coverage"]["definitions_local"].as_u64();
            continue;
        }
        let kind = row
            .get(3)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !matches!(kind, "CALLS" | "REFERENCES" | "IMPLEMENTS") {
            continue;
        }
        let keys = [2, 5].map(|i| row[i].as_str().unwrap_or_default());
        if keys.iter().any(|k| k.starts_with(&heuristic_in_root)) {
            assert_eq!(
                row[6].as_str(),
                Some("scip"),
                "heuristic code edge in a SCIP-ok project: {line}"
            );
        }
        if keys.iter().any(|k| k.starts_with(key_prefix)) {
            assert_eq!(row[6].as_str(), Some("scip"), "{line}");
            assert_eq!(
                row[7].as_i64(),
                Some(rb_model::Certainty::Exact.rank().into()),
                "{line}"
            );
            exact_calls += usize::from(kind == "CALLS");
        }
    }
    let definitions_local =
        definitions_local.unwrap_or_else(|| panic!("no ok scip_status coverage for {root:?}"));
    assert_eq!(
        heuristic_symbols as u64, definitions_local,
        "heuristic keys in documented files other than local definitions"
    );
    exact_calls
}

/// Contract M3b S3–S7, S10, S12 for scip-java: the Java fixture indexed with Maven in the slim
/// local image. Needs Docker, the image and (on a cold Maven cache) the network; skipped with a
/// notice otherwise.
#[test]
fn scip_java_spring_jpa() {
    let Some(url) = db_url() else { return };
    if let Err(reason) = scip_java_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let fixture = "java-spring-jpa";
    let repo = fixture_repo(fixture);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_scipj_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    let started = std::time::Instant::now();
    let first = rb(&url, &["index", path, "--repo", &name]);
    eprintln!(
        "scip_java_spring_jpa: first index {:.1?}",
        started.elapsed()
    );
    assert!(first.contains("scip: . scip-java ok"), "{first}");

    // S6: the §9.1 forms name SCIP symbols; the interface call resolves through `IMPLEMENTS`.
    golden(
        fixture,
        "scip-callers-authenticate.txt",
        &rb(
            &url,
            &[
                "callers",
                "DefaultAuthService.authenticate",
                "--depth",
                "2",
                "--repo",
                &name,
            ],
        ),
    );
    golden(
        fixture,
        "scip-impact-authservice.txt",
        &rb(
            &url,
            &["impact", "AuthService#authenticate().", "--repo", &name],
        ),
    );
    // Amendment 7: `login` sits under `@PostMapping("/login")`, where scip-java anchors its
    // definition; the by-name fallback still gives it its SCIP key.
    let login_key =
        "scip-java maven maven/com.acme/auth-service 1.4.0 com/acme/auth/AuthController#login().";
    assert!(
        rb(
            &url,
            &["find", "login", "--kind", "symbol", "--repo", &name]
        )
        .contains(login_key)
    );
    assert_eq!(
        rb(&url, &["callers", login_key, "--repo", &name]),
        rb(&url, &["callers", "AuthController.login", "--repo", &name])
    );

    // Amendment 16: `IMPLEMENTS` runs from the implementation to the interface only, so the
    // interface method is not listed as an implementer of the class's.
    golden(
        fixture,
        "scip-impact-default-authenticate.txt",
        &rb(
            &url,
            &["impact", "DefaultAuthService.authenticate", "--repo", &name],
        ),
    );

    // S7: no heuristic code edge remains in the documented files (the Java sources), and every
    // edge of the project's package is exact.
    let dump = rb(&url, &["dump", "--repo", &name]);
    let package = "scip-java maven maven/com.acme/auth-service 1.4.0 ";
    assert!(
        assert_exact_code_edges(&dump, package, "src/", ".") >= 2,
        "{dump}"
    );
    // Amendment 15: ownership is by document. `scripts/tool.py` lies under the Maven root but
    // scip-java has no document for it, so it keeps its heuristic `main → helper` call and is
    // not counted as a file without a document (see the status golden).
    let python_call = dump.lines().any(|line| {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        row[0] == "E"
            && row[2] == "rb-heuristic . . . `scripts/tool.py`/main()."
            && row[3] == "CALLS"
            && row[5] == "rb-heuristic . . . `scripts/tool.py`/helper()."
            && row[6] == "heuristic"
    });
    assert!(python_call, "no heuristic main → helper call in {dump}");
    golden(fixture, "scip-status.txt", &scip_status_lines(&url, &name));
    // Contract M4b S25 (with F2): the injections end to end with scip-java's types. The plain
    // single-bean rows (`AuthController → DefaultAuthService`, the `@Primary` hasher) are
    // `heuristic` with `resolution` `unique`/`primary`, the profile issuers `ambiguous` and
    // `conditional`; the DI and RTA coverage lines and the flagged `IMPLEMENTS` follow.
    let status = rb(&url, &["status", "--repo", &name]);
    let injects = format!(
        "{}{}{}",
        status_line(&status, "di:"),
        status_line(&status, "rta:"),
        edge_lines(&dump, |kind, attrs| {
            kind == "INJECTS"
                || (kind == "IMPLEMENTS"
                    && attrs.get("impl_instantiated") == Some(&serde_json::Value::Bool(false)))
        })
    );
    let package = "scip-java maven maven/com.acme/auth-service 1.4.0 com/acme/auth/";
    for line in [
        format!(
            "INJECTS {package}AuthController# -> {package}DefaultAuthService# [heuristic] {{\"origin\":\"stereotype\",\"resolution\":\"unique\",\"sites\":1,\"via\":\"constructor\"}}"
        ),
        format!(
            "INJECTS {package}DefaultAuthService# -> {package}BcryptHasher# [heuristic] {{\"origin\":\"stereotype\",\"primary\":true,\"resolution\":\"primary\",\"sites\":1,\"via\":\"constructor\"}}"
        ),
    ] {
        assert!(injects.lines().any(|l| l == line), "{line}\n{injects}");
    }
    for issuer in ["JwtIssuer#", "DevIssuer#"] {
        assert!(
            injects
                .lines()
                .any(|l| l.contains(&format!("-> {package}{issuer} [ambiguous]"))
                    && l.contains("\"conditional\":true")),
            "{issuer}\n{injects}"
        );
    }
    assert!(!injects.contains("[synthetic]"), "{injects}");
    golden(fixture, "scip-injects.txt", &injects);
    // S8: Maven artifacts and the JDK, with the versions scip-java resolved.
    golden(
        fixture,
        "scip-card.txt",
        &rb(&url, &["service", ".", "--repo", &name]),
    );

    // S4, S12: unchanged inputs are served from the cache, and the dump is byte-identical.
    let started = std::time::Instant::now();
    let second = rb(&url, &["index", path, "--repo", &name, "--force"]);
    eprintln!(
        "scip_java_spring_jpa: cached index {:.1?}",
        started.elapsed()
    );
    assert!(second.contains("scip: . scip-java ok (cached"), "{second}");
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    // Amendment 34: the container runs again on `--no-cache` (a broken runner cannot pass this
    // test from the cache) and the fresh index gives the same graph.
    let started = std::time::Instant::now();
    let fresh = rb(
        &url,
        &["index", path, "--repo", &name, "--force", "--no-cache"],
    );
    eprintln!(
        "scip_java_spring_jpa: fresh index {:.1?}",
        started.elapsed()
    );
    assert!(
        fresh.contains("scip: . scip-java ok ") && !fresh.contains("(cached"),
        "{fresh}"
    );
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
}

/// Contract M6a S2, S4: the Lombok fixture, built by scip-java with Lombok 1.18.34 fetched from
/// Maven Central through the egress proxy. Every document is accepted once D1 removes the
/// generated occurrences (the status golden carries `documents rejected 0` and the
/// `lombok-generated` count); the hand-written calls are exact (must-exist), no edge targets a
/// generated member (must-not-exist), every code edge of the project is exact, and the call
/// through `new Explicit()` resolves to `Explicit`'s own definition although scip-java defines
/// `Named`'s generated getter at the same name (S2). Fresh, cached and `--no-cache` runs dump
/// identically. Needs Docker, the image and the network on a cold Maven cache.
#[test]
fn scip_java_lombok() {
    let Some(url) = db_url() else { return };
    if let Err(reason) = scip_java_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let fixture = "java-lombok";
    let repo = fixture_repo(fixture);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_sciplombok_{}", std::process::id());
    let other = format!("t_sciplombok2_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone(), other.clone()],
    };
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    golden(fixture, "dump.jsonl", &rb(&url, &["dump", "--repo", &name]));

    let started = std::time::Instant::now();
    let first = rb(&url, &["index", path, "--repo", &name, "--force"]);
    eprintln!("scip_java_lombok: first index {:.1?}", started.elapsed());
    assert!(first.contains("scip: . scip-java ok"), "{first}");
    // The guards are evaluated before the status golden and printed, so a run without D1
    // shows both the rejections and the missing edges.
    let dump = rb(&url, &["dump", "--repo", &name]);
    let failures = guard_failures(fixture, &dump);
    if !failures.is_empty() {
        eprintln!("scip_java_lombok guard failures:\n{}", failures.join("\n"));
    }
    golden(fixture, "scip-status.txt", &scip_status_lines(&url, &name));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    golden(
        fixture,
        "scip-callers-greet.txt",
        &rb(&url, &["callers", "User.greet", "--repo", &name]),
    );
    let package = "scip-java maven maven/com.acme/lombok-demo 1.0.0 ";
    assert!(
        assert_exact_code_edges(&dump, package, "src/", ".") >= 5,
        "{dump}"
    );

    let second = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(second.contains("scip: . scip-java ok (cached"), "{second}");
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    let fresh = rb(
        &url,
        &["index", path, "--repo", &name, "--force", "--no-cache"],
    );
    assert!(
        fresh.contains("scip: . scip-java ok ") && !fresh.contains("(cached"),
        "{fresh}"
    );
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    // Another repository name for the same commit dumps identically (contract M6a S18).
    rb(&url, &["index", path, "--repo", &other]);
    assert_eq!(rb(&url, &["dump", "--repo", &other]), dump);
}

/// Contract M6e S4: the Gradle fixture. The `java-gradle-catalog` root job resolves its
/// environment (Kotlin DSL + version catalog) behind the proxy and indexes offline with
/// `scip-java index --build-tool Gradle`, so `Main.run → Service.process → Helper.exclaim`
/// is exact and the catalog/literal pins are exact `DEPENDS_ON` rows (guards); cached, fresh
/// from an emptied per-repository cache and a second repository name dump identically.
#[test]
fn scip_java_gradle_catalog() {
    let Some(url) = db_url() else { return };
    if let Err(reason) = scip_java_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let fixture = "java-gradle-catalog";
    let repo = fixture_repo(fixture);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_scipgradle_{}", std::process::id());
    let other = format!("t_scipgradle2_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone(), other.clone()],
    };
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    golden(fixture, "dump.jsonl", &rb(&url, &["dump", "--repo", &name]));

    let started = std::time::Instant::now();
    let first = rb(&url, &["index", path, "--repo", &name, "--force"]);
    eprintln!(
        "scip_java_gradle_catalog: first index {:.1?}",
        started.elapsed()
    );
    assert!(first.contains("scip: . scip-java ok"), "{first}");
    let dump = rb(&url, &["dump", "--repo", &name]);
    let failures = guard_failures(fixture, &dump);
    if !failures.is_empty() {
        eprintln!(
            "scip_java_gradle_catalog guard failures:\n{}",
            failures.join("\n")
        );
    }
    golden(fixture, "scip-status.txt", &scip_status_lines(&url, &name));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    golden(
        fixture,
        "scip-callers-process.txt",
        &rb(&url, &["callers", "Service.process", "--repo", &name]),
    );
    let package = "scip-java maven . . ";
    assert!(
        assert_exact_code_edges(&dump, package, "src/", ".") >= 2,
        "{dump}"
    );

    let second = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(second.contains("scip: . scip-java ok (cached"), "{second}");
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    let fresh = rb(
        &url,
        &["index", path, "--repo", &name, "--force", "--no-cache"],
    );
    assert!(
        fresh.contains("scip: . scip-java ok ") && !fresh.contains("(cached"),
        "{fresh}"
    );
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    rb(&url, &["index", path, "--repo", &other]);
    assert_eq!(rb(&url, &["dump", "--repo", &other]), dump);
}

/// Contract M6a S11: the npm-workspaces fixture. The root's job is grouped (`workspace 2
/// members`, fetched at the root from its lockfile, offline: every package is local) and indexes
/// both members, so `packages/app`'s call into `packages/core` is exact through the workspace
/// link and D4's interface-method `IMPLEMENTS` edges are present (guards); `tools/e2e`'s job
/// exports its `file:` link target (`1 link targets`); `legacy/`, not a member, keeps its own
/// unpinned job. The fetch runs twice from an emptied per-repository cache with the same stored
/// output; cached, fresh and a second repository name dump identically.
#[test]
fn scip_ts_workspaces() {
    let Some(url) = db_url() else { return };
    if let Err(reason) = rb_scip::docker_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let fixture = "ts-workspaces";
    let repo = fixture_repo(fixture);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_scipws_{}", std::process::id());
    let other = format!("t_scipws2_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone(), other.clone()],
    };
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    golden(fixture, "dump.jsonl", &rb(&url, &["dump", "--repo", &name]));

    let first = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(first.contains("scip: . scip-typescript ok"), "{first}");
    let dump = rb(&url, &["dump", "--repo", &name]);
    let failures = guard_failures(fixture, &dump);
    if !failures.is_empty() {
        eprintln!(
            "scip_ts_workspaces guard failures:\n{}",
            failures.join("\n")
        );
    }
    golden(fixture, "scip-status.txt", &scip_status_lines(&url, &name));
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(
        assert_exact_code_edges(&dump, "scip-typescript npm @ws/", "packages/", ".") >= 2,
        "{dump}"
    );

    let cached = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(
        cached.contains("scip: . scip-typescript ok (cached"),
        "{cached}"
    );
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    // A cold and a warm per-repository package cache store the same output (lesson
    // 2026-10-07, "splitting a cache re-exposes the tools' own bookkeeping").
    let cache = workspace_root().join("var/scip/cache").join(&name);
    for run in ["cold", "warm"] {
        // The first run may have been served from the index cache (it outlives test runs), in
        // which case no fetch created the package cache: emptied either way.
        if run == "cold" && cache.exists() {
            std::fs::remove_dir_all(&cache).unwrap();
        }
        let fresh = rb(
            &url,
            &["index", path, "--repo", &name, "--force", "--no-cache"],
        );
        assert!(
            fresh.contains("scip: . scip-typescript ok ") && !fresh.contains("(cached"),
            "{run}: {fresh}"
        );
        assert_eq!(rb(&url, &["dump", "--repo", &name]), dump, "{run}");
    }
    rb(&url, &["index", path, "--repo", &other]);
    assert_eq!(rb(&url, &["dump", "--repo", &other]), dump);
}

/// Contract M6e S7: the pnpm/yarn lockfile fixture. The `pnpm` root groups two members
/// (`workspace 2 members`, fetched at the root with pnpm) and the `yarn` service keeps its
/// own classic job; ranges pin to exact `DEPENDS_ON` rows per format (guards); cached, fresh
/// and a second repository name dump identically.
#[test]
fn scip_ts_pnpm_yarn() {
    let Some(url) = db_url() else { return };
    if let Err(reason) = rb_scip::docker_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let fixture = "ts-pnpm-yarn";
    let repo = fixture_repo(fixture);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_scippnpy_{}", std::process::id());
    let other = format!("t_scippnpy2_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone(), other.clone()],
    };
    rb(&url, &["index", path, "--repo", &name, "--no-scip"]);
    golden(fixture, "dump.jsonl", &rb(&url, &["dump", "--repo", &name]));

    let first = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(first.contains("scip-typescript ok"), "{first}");
    let dump = rb(&url, &["dump", "--repo", &name]);
    let failures = guard_failures(fixture, &dump);
    if !failures.is_empty() {
        eprintln!("scip_ts_pnpm_yarn guard failures:\n{}", failures.join("\n"));
    }
    golden(fixture, "scip-status.txt", &scip_status_lines(&url, &name));
    assert!(failures.is_empty(), "{}", failures.join("\n"));

    let cached = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(cached.contains("(cached"), "{cached}");
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    let fresh = rb(
        &url,
        &["index", path, "--repo", &name, "--force", "--no-cache"],
    );
    assert!(!fresh.contains("(cached"), "{fresh}");
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump, "fresh");
    rb(&url, &["index", path, "--repo", &other]);
    assert_eq!(rb(&url, &["dump", "--repo", &other]), dump);
}

/// Contract M3b S10 and invariant 4: a monorepo mixing a Maven project that indexes, one that
/// fails to build on purpose (`services/broken`), and TypeScript packages. `rb index` succeeds,
/// the failed project keeps its stderr tail and stays heuristic, the others are exact.
#[test]
fn scip_polyglot_monorepo() {
    let Some(url) = db_url() else { return };
    if let Err(reason) = scip_java_available() {
        eprintln!("skipping: {reason}");
        return;
    }
    let fixture = "polyglot-monorepo";
    let repo = fixture_repo(fixture);
    let path = repo.path().to_str().unwrap();
    let name = format!("t_scipp_{}", std::process::id());
    let _cleanup = Cleanup {
        url: &url,
        repos: vec![name.clone()],
    };
    let started = std::time::Instant::now();
    let first = rb(&url, &["index", path, "--repo", &name]);
    eprintln!(
        "scip_polyglot_monorepo: first index {:.1?}",
        started.elapsed()
    );
    assert!(
        first.contains("scip: services/auth scip-java ok"),
        "{first}"
    );
    assert!(
        first.contains("scip: services/broken scip-java failed"),
        "{first}"
    );
    golden(fixture, "scip-status.txt", &scip_status_lines(&url, &name));

    // The failed project's diagnostic carries Maven's error, and its files stay heuristic; the
    // ok project's edges are exact.
    let dump = rb(&url, &["dump", "--repo", &name]);
    let broken = dump
        .lines()
        .find(|l| l.starts_with("[\"D\",\"scip_status\",\"services/broken\""))
        .unwrap_or_else(|| panic!("no scip_status for services/broken in {dump}"));
    // Maven's wording differs once the miss is cached in the shared `.m2` ("was not found …
    // during a previous attempt"); the resolution summary is the same either way.
    // Dependency resolution fails before scip-java runs: the fetch phase (amendment 56).
    assert!(
        broken.contains("\"status\":\"failed\"")
            && broken.contains("\"phase\":\"fetch\"")
            && broken
                .contains("The following artifacts could not be resolved: com.acme:nope:jar:0.0.0"),
        "{broken}"
    );
    assert!(
        first.lines().any(
            |l| l.starts_with("  scip: services/broken scip-java failed")
                && l.ends_with(" (fetch phase)")
        ),
        "{first}"
    );
    assert!(
        dump.contains(
            "rb-heuristic . . . `services/broken/src/main/java/com/acme/broken/Broken.java`/Broken#"
        ),
        "{dump}"
    );
    let auth_package = "scip-java maven maven/com.acme/auth-service 1.4.0 ";
    assert!(
        assert_exact_code_edges(&dump, auth_package, "services/auth/", "services/auth") >= 1,
        "{dump}"
    );
    // Contract M6d S5: the manifest and SCIP gin edges share one
    // `go/<module>@<version>` node (no `gomod/` row survives); the stdlib
    // pseudo-module keys under the same manager.
    let gin = "go/github.com/gin-gonic/gin@v1.10.0";
    assert_eq!(
        dump.lines()
            .filter(|l| l.contains(&format!("[\"N\",\"package\",\"{gin}\"")))
            .count(),
        1,
        "{dump}"
    );
    assert!(!dump.contains("gomod/github.com/gin-gonic/gin"), "{dump}");
    for provenance in ["\"manifest\"", "\"scip\""] {
        assert!(
            dump.lines().any(|l| l.contains(&format!(
                "\"DEPENDS_ON\",\"package\",\"{gin}\",{provenance}"
            ))),
            "{provenance} gin edge in {dump}"
        );
    }
    assert!(
        dump.contains("[\"N\",\"package\",\"go/github.com/golang/go/src@go1.23\""),
        "{dump}"
    );
    // S8: the ok Maven service lists its packages; the failed one has no `scip` package, only
    // its pom's literal dependency (contract M4b S15, provenance `manifest`).
    golden(
        fixture,
        "scip-card-auth.txt",
        &rb(&url, &["service", "services/auth", "--repo", &name]),
    );
    let broken_card = rb(&url, &["service", "services/broken", "--repo", &name]);
    assert!(
        broken_card.contains("├─ Packages (1)\n│    com.acme/nope 0.0.0  [literal]\n"),
        "{broken_card}"
    );
    golden(
        fixture,
        "scip-callers-send-code.txt",
        &rb(
            &url,
            &[
                "callers",
                "MfaClient.sendCode",
                "--depth",
                "2",
                "--repo",
                &name,
            ],
        ),
    );

    // Contract M4a S24: in SCIP mode the controller's call into the Feign client is exact, and
    // an Express handler named by an identifier in a SCIP-owned file resolves exactly.
    let login = rb(&url, &["path", "POST /api/v1/login", "--repo", &name]);
    assert!(
        login.lines().any(|l| l
            .trim_start()
            .starts_with("→ com/acme/auth/MfaClient#sendCode().")
            && l.ends_with("[exact]")),
        "{login}"
    );
    golden(fixture, "scip-path-login.txt", &login);
    let health = dump.lines().any(|line| {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        row[0] == "E"
            && row[2] == "HTTP GET /health @services/notification"
            && row[3] == "HANDLED_BY"
            && row[5] == "scip-typescript npm notification HEAD src/`server.ts`/health()."
            && row[7] == i64::from(rb_model::Certainty::Exact.rank())
    });
    assert!(health, "no exact HANDLED_BY to health() in {dump}");

    let second = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(
        second.contains("scip: services/auth scip-java ok (cached")
            && second.contains("scip: services/broken scip-java failed (cached"),
        "{second}"
    );
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
    // Amendment 34: every job runs again on `--no-cache`, and the graph is the same.
    let started = std::time::Instant::now();
    let fresh = rb(
        &url,
        &["index", path, "--repo", &name, "--force", "--no-cache"],
    );
    eprintln!(
        "scip_polyglot_monorepo: fresh index {:.1?}",
        started.elapsed()
    );
    assert!(
        fresh.contains("scip: services/auth scip-java ok ")
            && fresh.contains("scip: services/broken scip-java failed ")
            && !fresh.contains("(cached"),
        "{fresh}"
    );
    // A failed job's `stderr_tail` is the build tool's text, not a function of the commit
    // (amendment 46): Maven's wording depends on the shared `.m2` negative cache ("was not
    // found … during a previous attempt" against "Could not find artifact", whose entries
    // expire daily). Everything else in the two dumps is byte-identical, both Maven tails carry
    // the same resolution summary, and npm's tail, which `--logs-max=0` keeps free of its
    // timestamped log path, is byte-identical.
    let fresh_dump = rb(&url, &["dump", "--repo", &name]);
    let (fresh_rows, fresh_tails) = without_failed_stderr(&fresh_dump);
    let (rows, tails) = without_failed_stderr(&dump);
    assert_eq!(fresh_rows, rows);
    assert_eq!(fresh_tails.len(), 2, "{fresh_tails:?}");
    assert!(
        fresh_tails["services/broken"]
            .contains("The following artifacts could not be resolved: com.acme:nope:jar:0.0.0")
            && tails["services/broken"]
                .contains("The following artifacts could not be resolved: com.acme:nope:jar:0.0.0"),
        "{fresh_tails:?}"
    );
    assert_eq!(fresh_tails["apps/web"], tails["apps/web"]);
    assert!(
        !fresh_tails["apps/web"].contains("_logs"),
        "{}",
        fresh_tails["apps/web"]
    );
}

/// `dump`'s rows with the `stderr_tail` removed from every failed `scip_status` diagnostic,
/// plus those tails by build root.
fn without_failed_stderr(dump: &str) -> (Vec<String>, BTreeMap<String, String>) {
    let mut tails = BTreeMap::new();
    let rows = dump
        .lines()
        .map(|line| {
            let mut row: serde_json::Value = serde_json::from_str(line).unwrap();
            if row[0] == "D"
                && row[1] == "scip_status"
                && row[4]["status"] == "failed"
                && let Some(tail) = row[4].as_object_mut().unwrap().remove("stderr_tail")
            {
                tails.insert(
                    row[2].as_str().unwrap().to_owned(),
                    tail.as_str().unwrap().to_owned(),
                );
                return row.to_string();
            }
            line.to_owned()
        })
        .collect();
    (rows, tails)
}
