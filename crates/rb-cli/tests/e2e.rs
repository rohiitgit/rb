//! End-to-end pipeline tests (design §12.2, §13): run the real `rb` binary against Postgres + AGE.
//!
//! Requires the compose database: `docker compose up -d --wait`, then
//! `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55432/rb cargo test`.
//! Without `RB_TEST_DATABASE_URL` these tests print a notice and pass vacuously.

use std::path::{Path, PathBuf};
use std::process::Command;

use std::collections::BTreeMap;

use rb_model::{EdgeKind, NodeKind, canonical_dump};
use rb_store::Store;

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn db_url() -> Option<String> {
    let url = std::env::var("RB_TEST_DATABASE_URL").ok();
    if url.is_none() {
        eprintln!("skipping: set RB_TEST_DATABASE_URL to run database tests");
    }
    url
}

/// Materialises `fixtures/<name>` as a git repository with a fixed identity and date.
fn fixture_repo(name: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    copy_dir(&workspace_root().join("fixtures").join(name), tmp.path());
    commit_all(tmp.path());
    tmp
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

fn copy_dir(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            std::fs::create_dir_all(&target).unwrap();
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn rb(url: &str, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_rb"))
        .args(args)
        .env("RB_DATABASE_URL", url)
        .env("RB_AGE_LOAD_DIR", workspace_root().join("var/age-load"))
        .output()
        .unwrap();
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
    rb(&url, &["index", path, "--repo", &a]);
    rb(&url, &["index", path, "--repo", &b]);
    let dump_a = rb(&url, &["dump", "--repo", &a]);
    let dump_b = rb(&url, &["dump", "--repo", &b]);
    assert_eq!(dump_a, dump_b, "same commit, different repos");

    assert!(rb(&url, &["index", path, "--repo", &a]).contains("up to date"));
    rb(&url, &["index", path, "--repo", &a, "--force"]);
    assert_eq!(rb(&url, &["dump", "--repo", &a]), dump_a, "forced rebuild");

    // The stored snapshot is faithful to the in-memory pipeline output.
    let indexed = rb_indexer::index_commit(repo.path(), "HEAD", &mut rb_indexer::NoCache).unwrap();
    assert_eq!(
        canonical_dump(&indexed.commit_sha, &indexed.graph, &indexed.diagnostics),
        dump_a,
        "store round trip"
    );
    let graph = &indexed.graph;
    let node = |id: i64| graph.nodes().iter().find(|n| n.id == id).unwrap();

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

/// Compares `actual` with `tests/golden/<fixture>/<name>`; `RB_BLESS=1` rewrites the file instead.
/// Blessed output must be reviewed by hand before it is committed.
fn golden(fixture: &str, name: &str, actual: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(fixture)
        .join(name);
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
        &["index", repo.path().to_str().unwrap(), "--repo", &name],
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
    // M1b extracts call references but must not emit unresolved CALLS edges (contract C7).
    assert!(!dump.contains("\"CALLS\""), "M1b must not emit CALLS edges");
    golden(fixture, "dump.jsonl", &dump);

    let err = rb_fails(&url, &["service", "no-such-service", "--repo", &name]);
    assert!(err.contains("no service matches"), "{err}");
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
        &["index", repo.path().to_str().unwrap(), "--repo", &name],
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
