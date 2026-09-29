//! M0 end-to-end tests (design §12.2, §13): run the real `rb` binary against Postgres + AGE.
//!
//! Requires the compose database: `docker compose up -d --wait`, then
//! `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55432/rb cargo test`.
//! Without `RB_TEST_DATABASE_URL` these tests print a notice and pass vacuously.

use std::path::{Path, PathBuf};
use std::process::Command;

use rb_model::{canonical_dump, graph_from_manifest};
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
    for args in [
        &["init", "-q", "-b", "main"][..],
        &["add", "-A"],
        &["commit", "-q", "-m", "fixture"],
    ] {
        let out = Command::new("git")
            .args(args)
            .current_dir(tmp.path())
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
    tmp
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
    let manifest = rb_git::read_manifest(repo.path(), "HEAD").unwrap();
    let graph = graph_from_manifest(&manifest).unwrap();
    assert_eq!(
        canonical_dump(&manifest.commit_sha, &graph),
        dump_a,
        "store round trip"
    );
    let files = manifest.files.len();
    assert!(files >= 6, "fixture unexpectedly small: {files} files");

    // M0 exit criterion: `rb cypher` returns files per service.
    let per_service = rb(
        &url,
        &[
            "cypher",
            "MATCH (f:File)-[:IN_SERVICE]->(s:Service) RETURN s.name, count(f)",
            "--repo",
            &a,
            "--cols",
            "service,files",
        ],
    );
    assert_eq!(per_service, format!("service\tfiles\n(root)\t{files}\n"));

    // AGE parity (design §12.2): every relational IN_SERVICE edge exists in the projection.
    let age_edges = rb(
        &url,
        &[
            "cypher",
            "MATCH (f:File)-[e:IN_SERVICE]->(s:Service) RETURN f.key, s.key, e.certainty ORDER BY f.key",
            "--repo",
            &a,
            "--cols",
            "file,service,certainty",
        ],
    );
    let mut expected = String::from("file\tservice\tcertainty\n");
    for f in &manifest.files {
        expected.push_str(&format!("{}\t(root)\t5\n", f.path));
    }
    assert_eq!(age_edges, expected);

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

#[test]
fn ts_express_prisma_m0() {
    check_fixture("ts-express-prisma", "ts");
}

#[test]
fn java_spring_jpa_m0() {
    check_fixture("java-spring-jpa", "java");
}
