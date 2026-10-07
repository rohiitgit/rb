//! Helpers shared by the end-to-end suites (`e2e.rs`, `api.rs`): the database lock and its
//! start-up clean-up, fixture repositories, running `rb`, and a running `rb serve`. Each suite
//! uses a subset of them.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use rb_store::Store;

pub fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The database URL, plus a lock that serializes the tests of this binary: they spawn `rb`
/// processes that create and drop AGE graphs concurrently, and AGE 1.8 on Postgres 18 then
/// intermittently kills a backend with `FATAL: failed to re-find shared lock object` (seen 11
/// times in one day of parallel runs, never serially). The tests themselves are independent.
pub struct Db {
    pub url: String,
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

pub fn db_url() -> Option<Db> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let url = std::env::var("RB_TEST_DATABASE_URL").ok();
    let Some(url) = url else {
        eprintln!("skipping: set RB_TEST_DATABASE_URL to run database tests");
        return None;
    };
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    static CLEAN: std::sync::Once = std::sync::Once::new();
    CLEAN.call_once(|| remove_leftover_test_repositories(&url));
    Some(Db { url, _guard: guard })
}

/// Contract M5 S8: once per run, before the first test touches the database, the `t_*`
/// repositories that killed runs left behind are deleted (one graph per transaction) and any
/// `rb_t_*` graph no live snapshot names is dropped, through `rb gc`'s code.
pub fn remove_leftover_test_repositories(url: &str) {
    let mut store = Store::connect(url).unwrap();
    store.migrate().unwrap();
    let leftover: Vec<String> = store
        .repo_names()
        .unwrap()
        .into_iter()
        .filter(|r| r.starts_with("t_"))
        .collect();
    for r in &leftover {
        store.delete_repo(r).unwrap();
    }
    let plan = store.gc_plan("t_").unwrap();
    let removed = store.gc_apply(&plan).unwrap();
    eprintln!(
        "e2e start-up: removed {} leftover test repositories, {} graphs and {} building rows",
        leftover.len(),
        removed.graphs.len(),
        removed.building.len()
    );
}

/// Materialises `fixtures/<name>` as a git repository with a fixed identity and date, from the
/// files the workspace tracks (contract M3b amendment 40): an IDE's build output under a
/// fixture, ignored by the workspace, would otherwise be committed into the fixture repository.
pub fn fixture_repo(name: &str) -> tempfile::TempDir {
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
pub fn tracked_files(name: &str) -> Vec<String> {
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
pub fn repo_from(files: &[(&str, &str)]) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let p = tmp.path().join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, contents).unwrap();
    }
    commit_all(tmp.path());
    tmp
}

pub fn commit_all(dir: &Path) {
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

pub fn rb(url: &str, args: &[&str]) -> String {
    rb_with_path(url, args, None)
}

/// `rb` with the child's `PATH` replaced (`Some("/usr/bin:/bin")` hides Docker from it).
pub fn rb_with_path(url: &str, args: &[&str], path: Option<&str>) -> String {
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
pub struct Cleanup<'a> {
    pub url: &'a str,
    pub repos: Vec<String>,
}

impl Drop for Cleanup<'_> {
    /// Deletes the repositories, their package caches (`var/scip/cache/<repo>`, contract M5
    /// S22) and, when a SCIP job ran for one of them, the egress networks it created (contract
    /// M5 S21; the tests of this binary run one at a time, so no other run is using them).
    fn drop(&mut self) {
        if let Ok(mut store) = Store::connect(self.url) {
            for r in &self.repos {
                let _ = store.delete_repo(r);
            }
        }
        let mut ran = false;
        for r in &self.repos {
            let cache = workspace_root().join("var/scip/cache").join(r);
            ran |= cache.exists();
            let _ = std::fs::remove_dir_all(cache);
        }
        if ran {
            let _ = rb_scip::DockerRunner::default().egress_leftovers(true);
        }
    }
}

/// The id of `repo`'s live snapshot.
pub fn live_id(url: &str, repo: &str) -> i64 {
    Store::connect(url)
        .unwrap()
        .live_snapshot(repo)
        .unwrap()
        .expect("a live snapshot")
        .id
}

/// `rb` with its exit code, stdout and stderr, whether it succeeds or not.
pub fn rb_exit(url: &str, args: &[&str]) -> (i32, String, String) {
    rb_exit_env(url, args, &[])
}

pub fn rb_exit_env(url: &str, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rb"));
    cmd.args(args).env("RB_DATABASE_URL", url);
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

/// A running `rb serve` on an ephemeral loopback port; stopped with SIGINT when dropped (and
/// killed if it does not stop), so a failed assertion leaves no server behind.
pub struct Served {
    pub child: std::process::Child,
    pub addr: String,
}

impl Served {
    pub fn start(url: &str, extra: &[&str]) -> Served {
        Served::start_env(url, extra, &[])
    }

    /// `rb serve` with extra environment variables (`RB_TEST_CAPS`, `RB_TEST_HOLD_MS`).
    pub fn start_env(url: &str, extra: &[&str], env: &[(&str, &str)]) -> Served {
        use std::io::BufRead;
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rb"));
        cmd.args([&["serve", "--bind", "127.0.0.1:0"][..], extra].concat())
            .env("RB_DATABASE_URL", url)
            .stdout(std::process::Stdio::piped());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().unwrap();
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let addr = line
            .trim()
            .strip_prefix("rb serve: listening on http://")
            .unwrap_or_else(|| panic!("unexpected first line {line:?}"))
            .to_owned();
        Served { child, addr }
    }

    /// One request with `Connection: close`; the status and the body.
    pub fn call(&self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        use std::io::{Read, Write};
        let mut s = std::net::TcpStream::connect(&self.addr).unwrap();
        let body = body.unwrap_or("");
        let ct = if method == "POST" {
            "Content-Type: application/json\r\n"
        } else {
            ""
        };
        write!(
            s,
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n{ct}Content-Length: {}\r\n\r\n{body}",
            self.addr,
            body.len()
        )
        .unwrap();
        let mut text = String::new();
        s.read_to_string(&mut text).unwrap();
        let (head, body) = text.split_once("\r\n\r\n").unwrap();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, body.to_owned())
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        let _ = Command::new("kill")
            .args(["-INT", &self.child.id().to_string()])
            .status();
        for _ in 0..50 {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
