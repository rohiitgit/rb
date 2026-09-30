//! End-to-end pipeline tests (design §12.2, §13): run the real `rb` binary against Postgres + AGE.
//!
//! Requires the compose database: `docker compose up -d --wait`, then
//! `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55432/rb cargo test`.
//! Without `RB_TEST_DATABASE_URL` these tests print a notice and pass vacuously. Every index run
//! passes `--no-scip` so goldens stay heuristic and Docker-free, except `scip_ts_express_prisma`,
//! which runs the real scip-typescript container and skips itself when Docker is unavailable.

use std::path::{Path, PathBuf};
use std::process::Command;

use std::collections::BTreeMap;

use rb_model::{EdgeKind, NodeKind, canonical_dump};
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
        .env("RB_SCIP_WORK_DIR", workspace_root().join("var/scip"))
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
        }
    }
    assert!(exact_calls >= 2, "{dump}");
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

    // S4, S12: unchanged inputs are served from the cache, and the dump is byte-identical.
    let second = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(
        second.contains("scip: . scip-typescript ok (cached"),
        "{second}"
    );
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
}

/// The `code:` and `scip:` lines of `rb status`, with each job's run time masked: it is the
/// container's (or the cached run's), not a function of the commit.
fn scip_status_lines(url: &str, name: &str) -> String {
    let status = rb(url, &["status", "--repo", name]);
    let masked: Vec<String> = status
        .lines()
        .filter(|l| {
            let l = l.trim_start();
            l.starts_with("code:") || l.starts_with("scip:")
        })
        .map(|l| {
            l.split(' ')
                .map(|token| {
                    let t = token.trim_end_matches(',');
                    match t.strip_suffix('s') {
                        Some(n) if n.parse::<f64>().is_ok() => token.replace(t, "<elapsed>"),
                        _ => token.to_owned(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    format!("{}\n", masked.join("\n"))
}

/// Docker answering and the local scip-java image built (docker/scip-java/README.md); the
/// reason to skip otherwise. The image is never built implicitly.
fn scip_java_available() -> Result<(), String> {
    use rb_scip::Runner as _;
    rb_scip::docker_available()?;
    let image = rb_scip::jobs::SCIP_JAVA_IMAGE;
    rb_scip::DockerRunner
        .image_id(image)
        .map(|_| ())
        .ok_or_else(|| format!("{image} is not built; see docker/scip-java/README.md"))
}

/// Contract M3b S7 for one SCIP-ok project: every `CALLS`, `REFERENCES` and `IMPLEMENTS` edge
/// of `dump` whose source or target key starts with `key_prefix` (the project's SCIP package,
/// `""` for every edge) is `scip`/`exact`, and no such edge names one of the project's files
/// through a heuristic key (`` `<root>/ ``). Returns the number of exact `CALLS`.
fn assert_exact_code_edges(dump: &str, key_prefix: &str, root: &str) -> usize {
    let mut exact_calls = 0;
    let heuristic_in_root = format!("rb-heuristic . . . `{root}");
    for line in dump.lines() {
        let row: serde_json::Value = serde_json::from_str(line).unwrap();
        let kind = row
            .get(3)
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        if !matches!(kind, "CALLS" | "REFERENCES" | "IMPLEMENTS") {
            continue;
        }
        let keys = [2, 5].map(|i| row[i].as_str().unwrap_or_default());
        assert!(
            !keys.iter().any(|k| k.starts_with(&heuristic_in_root)),
            "heuristic code edge in a SCIP-ok project: {line}"
        );
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

    // S7: no heuristic code edge remains anywhere in the project.
    let dump = rb(&url, &["dump", "--repo", &name]);
    assert!(assert_exact_code_edges(&dump, "", "") >= 2, "{dump}");
    golden(fixture, "scip-status.txt", &scip_status_lines(&url, &name));

    // S4, S12: unchanged inputs are served from the cache, and the dump is byte-identical.
    let started = std::time::Instant::now();
    let second = rb(&url, &["index", path, "--repo", &name, "--force"]);
    eprintln!(
        "scip_java_spring_jpa: cached index {:.1?}",
        started.elapsed()
    );
    assert!(second.contains("scip: . scip-java ok (cached"), "{second}");
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
    assert!(
        broken.contains("\"status\":\"failed\"")
            && broken
                .contains("The following artifacts could not be resolved: com.acme:nope:jar:0.0.0"),
        "{broken}"
    );
    assert!(
        dump.contains(
            "rb-heuristic . . . `services/broken/src/main/java/com/acme/broken/Broken.java`/Broken#"
        ),
        "{dump}"
    );
    let auth_package = "scip-java maven maven/com.acme/auth-service 1.4.0 ";
    assert!(
        assert_exact_code_edges(&dump, auth_package, "services/auth/") >= 1,
        "{dump}"
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

    let second = rb(&url, &["index", path, "--repo", &name, "--force"]);
    assert!(
        second.contains("scip: services/auth scip-java ok (cached")
            && second.contains("scip: services/broken scip-java failed (cached"),
        "{second}"
    );
    assert_eq!(rb(&url, &["dump", "--repo", &name]), dump);
}
