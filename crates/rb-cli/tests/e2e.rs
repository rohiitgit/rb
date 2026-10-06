//! End-to-end pipeline tests (design §12.2, §13): run the real `rb` binary against Postgres + AGE.
//!
//! Requires the compose database: `docker compose up -d --wait`, then
//! `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55432/rb cargo test`.
//! Without `RB_TEST_DATABASE_URL` these tests print a notice and pass vacuously. Every index run
//! passes `--no-scip` so goldens stay heuristic and Docker-free, except the `scip_*` tests,
//! which run the real indexer containers and skip themselves when Docker (or the local
//! scip-java image) is unavailable.

use std::path::{Path, PathBuf};
use std::process::Command;

use std::collections::{BTreeMap, HashMap, HashSet};

use rb_model::{Certainty, EdgeKind, Graph, NodeKind, Provenance, canonical_dump};
use rb_store::Store;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The database URL, plus a lock that serializes the tests of this binary: they spawn `rb`
/// processes that create and drop AGE graphs concurrently, and AGE 1.8 on Postgres 18 then
/// intermittently kills a backend with `FATAL: failed to re-find shared lock object` (seen 11
/// times in one day of parallel runs, never serially). The tests themselves are independent.
struct Db {
    url: String,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl std::ops::Deref for Db {
    type Target = str;
    fn deref(&self) -> &str {
        &self.url
    }
}

impl AsRef<std::ffi::OsStr> for Db {
    fn as_ref(&self) -> &std::ffi::OsStr {
        self.url.as_ref()
    }
}

fn db_url() -> Option<Db> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let url = std::env::var("RB_TEST_DATABASE_URL").ok();
    let Some(url) = url else {
        eprintln!("skipping: set RB_TEST_DATABASE_URL to run database tests");
        return None;
    };
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    Some(Db { url, _guard: guard })
}

/// Materialises `fixtures/<name>` as a git repository with a fixed identity and date, from the
/// files the workspace tracks (contract M3b amendment 40): an IDE's build output under a
/// fixture, ignored by the workspace, would otherwise be committed into the fixture repository.
fn fixture_repo(name: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    let from = workspace_root().join("fixtures").join(name);
    for path in tracked_files(name) {
        let target = tmp.path().join(&path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::copy(from.join(&path), target).unwrap();
    }
    commit_all(tmp.path());
    tmp
}

/// The fixture-relative paths `git ls-files` lists for `fixtures/<name>`.
fn tracked_files(name: &str) -> Vec<String> {
    let prefix = format!("fixtures/{name}/");
    let out = Command::new("git")
        .args(["ls-files", "-z", "--", prefix.trim_end_matches('/')])
        .current_dir(workspace_root())
        .output()
        .unwrap();
    assert!(out.status.success(), "git ls-files failed for {name}");
    let files: Vec<String> = out
        .stdout
        .split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| {
            String::from_utf8(p.to_vec())
                .unwrap()
                .strip_prefix(&prefix)
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(!files.is_empty(), "no tracked files under {prefix}");
    files
}

/// A repository built from inline `(path, contents)` pairs.
fn repo_from(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let p = tmp.path().join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }
    commit_all(tmp.path());
    tmp
}

fn commit_all(dir: &Path) {
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["add", "-A"],
        &["commit", "-q", "-m", "fixture"],
    ] {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "rb")
            .env("GIT_AUTHOR_EMAIL", "rb@example.invalid")
            .env("GIT_COMMITTER_NAME", "rb")
            .env("GIT_COMMITTER_EMAIL", "rb@example.invalid")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

fn rb(url: &str, args: &[&str]) -> String {
    rb_with_path(url, args, None)
}

/// `rb` with the child's `PATH` replaced (`Some("/usr/bin:/bin")` hides Docker from it).
fn rb_with_path(url: &str, args: &[&str], path: Option<&str>) -> String {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rb"));
    cmd.args(args)
        .env("RB_DATABASE_URL", url)
        .env("RB_AGE_LOAD_DIR", workspace_root().join("var/age-load"))
        .env("RB_SCIP_WORK_DIR", workspace_root().join("var/scip"));
    if let Some(path) = path {
        cmd.env("PATH", path);
    }
    let out = cmd.output().unwrap();
    assert!(
        out.status.success(),
        "rb {args:?} failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Deletes the test's repositories (rows and AGE graphs) even when an assertion fails.
struct Cleanup<'a> {
    url: &'a str,
    repos: Vec<String>,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        if let Ok(mut store) = Store::connect(self.url) {
            for r in &self.repos {
                let _ = store.delete_repo(r);
            }
        }
    }
}

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

    // AGE parity (design §12.2): the projection holds exactly the relational edges.
    let age_edges = rb(
        &url,
        &[
            "cypher",
            "MATCH (a)-[e]->(b) RETURN a.key, label(e), b.key, e.certainty",
            "--repo",
            &a,
            "--cols",
            "src,kind,dst,certainty",
        ],
    );
    let mut expected: Vec<String> = graph
        .edges()
        .iter()
        .map(|e| {
            format!(
                "{}\t{}\t{}\t{}",
                node(e.src).key,
                e.kind,
                node(e.dst).key,
                e.certainty.rank()
            )
        })
        .collect();
    expected.sort();
    assert_eq!(sorted_rows(&age_edges), expected);

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
        sorted_rows(&rb(&url, &["cypher", query, "--repo", &a, "--cols", cols]))
    };
    assert_eq!(
        cypher("MATCH (e:Endpoint) RETURN e.key", "endpoint"),
        endpoints
    );
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
    golden(
        fixture,
        "card-root.json",
        &rb(&url, &["service", ".", "--repo", &name, "--json"]),
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
        &rb(&url, &["table-deps", "invoices", "--repo", &name, "--json"]),
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
    golden(fixture, "path-login.json", &json);
    // The login path spans three services and two languages (TypeScript → Java → TypeScript).
    let tree: serde_json::Value = serde_json::from_str(&json).unwrap();
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
    path_steps(&tree["upstream"], &mut steps);
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
    // Amendment 49: the stored reason names no host path.
    let dump = rb(&url, &["dump", "--repo", &name]);
    assert!(
        dump.contains("\"reason\":\"shim: docker run refused for <host path>\"")
            && !dump.contains("/Users/nobody"),
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
