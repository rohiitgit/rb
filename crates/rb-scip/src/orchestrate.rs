//! Running one job (design §5.2): export the build root's files into a work directory that the
//! container VM can see, fetch dependencies with the network on, index with it off, and collect
//! `index.scip`. Nothing here talks to the database; the caller owns the cache.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::jobs::{Fetch, Indexer, Job, NodeLock, SCIP_TYPESCRIPT_IMAGE};
use crate::runner::{ContainerRun, Network, Outcome, Runner};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Ok,
    /// The fetch or index phase failed; `stderr_tail` says why.
    Failed,
    Timeout,
    /// No indexer can run for this job on this host; the reason is user-facing.
    Unsupported(String),
    /// The job was not attempted (SCIP disabled, or Docker unavailable); the reason is user-facing.
    Skipped(String),
}

impl RunStatus {
    /// The value stored in `rb.scip_index.status` and reported per project.
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Ok => "ok",
            RunStatus::Failed => "failed",
            RunStatus::Timeout => "timeout",
            RunStatus::Unsupported(_) => "unsupported",
            RunStatus::Skipped(_) => "skipped",
        }
    }

    /// The status a cache row recorded (`ok`, `failed` or `timeout`; anything else is `failed`,
    /// since only finished runs are cached).
    pub fn from_cached(status: &str) -> RunStatus {
        match status {
            "ok" => RunStatus::Ok,
            "timeout" => RunStatus::Timeout,
            _ => RunStatus::Failed,
        }
    }

    /// Why the job did not run, for `unsupported` and `skipped`.
    pub fn reason(&self) -> Option<&str> {
        match self {
            RunStatus::Unsupported(r) | RunStatus::Skipped(r) => Some(r),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipRun {
    pub status: RunStatus,
    pub index: Option<Vec<u8>>,
    pub stderr_tail: String,
    pub elapsed: Duration,
}

/// Resource budget per phase.
#[derive(Debug, Clone, PartialEq)]
pub struct Budget {
    pub cpus: f32,
    pub memory: String,
    pub fetch_timeout: Duration,
    pub index_timeout: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            cpus: 2.0,
            memory: "4g".into(),
            fetch_timeout: Duration::from_secs(600),
            index_timeout: Duration::from_secs(900),
        }
    }
}

/// The image reference a job uses, pinned by digest: the cache key's second half.
pub fn image_for(job: &Job) -> &'static str {
    match job.indexer {
        Indexer::ScipTypescript => SCIP_TYPESCRIPT_IMAGE,
        Indexer::ScipJava => "unsupported",
    }
}

/// Runs `job` over `files` (`(root-relative path, bytes)`) in `work_dir` (recreated, removed
/// afterwards) with `cache_dir` holding package-manager caches shared across jobs.
pub fn run_job(
    job: &Job,
    files: &[(String, Vec<u8>)],
    work_dir: &Path,
    cache_dir: &Path,
    budget: &Budget,
    runner: &dyn Runner,
) -> anyhow::Result<ScipRun> {
    let started = std::time::Instant::now();
    let unsupported = |reason: &str| ScipRun {
        status: RunStatus::Unsupported(reason.to_owned()),
        index: None,
        stderr_tail: String::new(),
        elapsed: Duration::ZERO,
    };
    if job.indexer == Indexer::ScipJava {
        // The published scip-java image is 6.75 GB (contract M3b facts); a slim image is pending.
        return Ok(unsupported("scip-java: no usable indexer image yet"));
    }
    export(files, work_dir)?;
    std::fs::create_dir_all(cache_dir)?;
    let result = (|| -> anyhow::Result<ScipRun> {
        let (fetch, index) = typescript_runs(job, work_dir, cache_dir, budget)?;
        let outcome = runner.run(&fetch)?;
        if !outcome.ok() {
            return Ok(finished(&outcome, None, started));
        }
        let outcome = runner.run(&index)?;
        let index_bytes = if outcome.ok() {
            std::fs::read(work_dir.join("index.scip")).ok()
        } else {
            None
        };
        Ok(finished(&outcome, index_bytes, started))
    })();
    let _ = std::fs::remove_dir_all(work_dir);
    result
}

fn finished(outcome: &Outcome, index: Option<Vec<u8>>, started: std::time::Instant) -> ScipRun {
    let status = if outcome.timed_out {
        RunStatus::Timeout
    } else if outcome.ok() && index.is_some() {
        RunStatus::Ok
    } else {
        RunStatus::Failed
    };
    let mut stderr_tail = outcome.stderr_tail.clone();
    if outcome.ok() && index.is_none() {
        stderr_tail.push_str("\n(no index.scip was produced)");
    }
    ScipRun {
        status,
        index,
        stderr_tail: stderr_tail.trim().to_owned(),
        elapsed: started.elapsed(),
    }
}

/// The fetch and index invocations for a scip-typescript job (contract M3b facts: the image's
/// `npm` is a wrapper that hides failures, so the real binary is called; `--network none` for
/// the index; the image exists for linux/amd64 only).
fn typescript_runs(
    job: &Job,
    work_dir: &Path,
    cache_dir: &Path,
    budget: &Budget,
) -> anyhow::Result<(ContainerRun, ContainerRun)> {
    let workdir = crate::runner::absolute(work_dir)?;
    let npm_cache = crate::runner::absolute(cache_dir)?.join("npm");
    std::fs::create_dir_all(&npm_cache)?;
    let base = ContainerRun {
        image: SCIP_TYPESCRIPT_IMAGE.into(),
        platform: Some("linux/amd64".into()),
        workdir,
        mount_at: "/src".into(),
        caches: vec![(npm_cache, "/root/.npm".into())],
        env: vec![("NODE_OPTIONS".into(), "--max-old-space-size=4096".into())],
        network: Network::Registries,
        cpus: budget.cpus,
        memory: budget.memory.clone(),
        timeout: budget.fetch_timeout,
        entrypoint: None,
        args: Vec::new(),
    };
    let quiet = ["--no-audit", "--no-fund", "--loglevel=error"];
    let (entrypoint, args): (&str, Vec<String>) = match job.fetch {
        Fetch::Npm {
            lockfile: NodeLock::PackageLock,
        } => (
            "/usr/local/bin/actual-npm",
            once("ci").chain(quiet).map(String::from).collect(),
        ),
        Fetch::Npm {
            lockfile: NodeLock::Yarn,
        } => (
            "yarn",
            ["install", "--frozen-lockfile", "--non-interactive"]
                .map(String::from)
                .to_vec(),
        ),
        Fetch::Npm {
            lockfile: NodeLock::Pnpm,
        } => (
            "pnpm",
            ["install", "--frozen-lockfile"].map(String::from).to_vec(),
        ),
        Fetch::Npm {
            lockfile: NodeLock::None,
        } => (
            "/usr/local/bin/actual-npm",
            once("install").chain(quiet).map(String::from).collect(),
        ),
        Fetch::Maven { .. } | Fetch::Gradle { .. } => {
            anyhow::bail!("a Java fetch on a TypeScript job")
        }
    };
    let fetch = ContainerRun {
        entrypoint: Some(entrypoint.into()),
        args,
        ..base.clone()
    };
    let mut index_args: Vec<String> = ["index", "--output", "index.scip", "--no-progress-bar"]
        .map(String::from)
        .to_vec();
    index_args.extend(job.projects.iter().cloned());
    let index = ContainerRun {
        network: Network::None,
        timeout: budget.index_timeout,
        args: index_args,
        ..base
    };
    Ok((fetch, index))
}

fn once(s: &str) -> std::iter::Once<&str> {
    std::iter::once(s)
}

/// Writes `files` under `dir`, which is recreated empty first. Paths never escape `dir`.
fn export(files: &[(String, Vec<u8>)], dir: &Path) -> anyhow::Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir)?;
    }
    std::fs::create_dir_all(dir)?;
    for (path, bytes) in files {
        anyhow::ensure!(
            !path.split('/').any(|p| p == ".." || p.is_empty()) && !path.starts_with('/'),
            "unsafe path in export: {path}"
        );
        let dest: PathBuf = dir.join(path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// Records runs; the index phase writes a fake `index.scip` unless told to fail.
    struct Fake {
        runs: RefCell<Vec<ContainerRun>>,
        fail_fetch: bool,
        fail_index: bool,
    }

    impl Runner for Fake {
        fn run(&self, run: &ContainerRun) -> anyhow::Result<Outcome> {
            self.runs.borrow_mut().push(run.clone());
            let is_index = run.args.first().map(String::as_str) == Some("index");
            let fail = if is_index {
                self.fail_index
            } else {
                self.fail_fetch
            };
            if is_index && !fail {
                std::fs::write(run.workdir.join("index.scip"), b"fake-index")?;
            }
            Ok(Outcome {
                exit_code: Some(if fail { 1 } else { 0 }),
                timed_out: false,
                stderr_tail: if fail { "boom".into() } else { String::new() },
                elapsed: Duration::from_millis(1),
            })
        }
    }

    fn ts_job(lockfile: NodeLock) -> Job {
        Job {
            root: "apps/web".into(),
            indexer: Indexer::ScipTypescript,
            fetch: Fetch::Npm { lockfile },
            projects: vec![".".into(), "sub".into()],
            unpinned: lockfile == NodeLock::None,
        }
    }

    fn files() -> Vec<(String, Vec<u8>)> {
        vec![
            ("package.json".into(), b"{}".to_vec()),
            ("src/a.ts".into(), b"export const a = 1;".to_vec()),
        ]
    }

    #[test]
    fn fetch_then_offline_index_and_the_work_directory_is_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let cache = tmp.path().join("cache");
        let fake = Fake {
            runs: RefCell::new(Vec::new()),
            fail_fetch: false,
            fail_index: false,
        };
        let run = run_job(
            &ts_job(NodeLock::PackageLock),
            &files(),
            &work,
            &cache,
            &Budget::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(run.status, RunStatus::Ok);
        assert_eq!(run.index.as_deref(), Some(&b"fake-index"[..]));
        assert!(!work.exists(), "work directory is removed");
        let runs = fake.runs.borrow();
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].network, Network::Registries);
        assert_eq!(
            runs[0].entrypoint.as_deref(),
            Some("/usr/local/bin/actual-npm")
        );
        assert_eq!(runs[0].args[0], "ci");
        assert_eq!(runs[1].network, Network::None);
        assert_eq!(
            runs[1].args,
            [
                "index",
                "--output",
                "index.scip",
                "--no-progress-bar",
                ".",
                "sub"
            ]
        );
        assert_eq!(runs[1].platform.as_deref(), Some("linux/amd64"));
        assert!(runs[1].caches[0].1 == "/root/.npm");
        assert!(
            cache.join("npm").is_dir(),
            "the npm cache directory is created"
        );
    }

    #[test]
    fn a_failed_fetch_stops_before_indexing_and_a_failed_index_has_no_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake {
            runs: RefCell::new(Vec::new()),
            fail_fetch: true,
            fail_index: false,
        };
        let run = run_job(
            &ts_job(NodeLock::None),
            &files(),
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            &Budget::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.stderr_tail, "boom");
        assert_eq!(fake.runs.borrow().len(), 1);
        assert_eq!(
            fake.runs.borrow()[0].args[0],
            "install",
            "no lockfile: npm install"
        );
        let fake = Fake {
            runs: RefCell::new(Vec::new()),
            fail_fetch: false,
            fail_index: true,
        };
        let run = run_job(
            &ts_job(NodeLock::Yarn),
            &files(),
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            &Budget::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(run.status, RunStatus::Failed);
        assert!(run.index.is_none());
        assert_eq!(fake.runs.borrow()[0].entrypoint.as_deref(), Some("yarn"));
    }

    #[test]
    fn java_jobs_are_unsupported_for_now_and_exports_refuse_unsafe_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake {
            runs: RefCell::new(Vec::new()),
            fail_fetch: false,
            fail_index: false,
        };
        let job = Job {
            root: "svc".into(),
            indexer: Indexer::ScipJava,
            fetch: Fetch::Maven { wrapper: false },
            projects: Vec::new(),
            unpinned: false,
        };
        let run = run_job(
            &job,
            &files(),
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            &Budget::default(),
            &fake,
        )
        .unwrap();
        assert!(matches!(run.status, RunStatus::Unsupported(_)));
        assert!(fake.runs.borrow().is_empty());
        let bad = vec![("../escape".to_owned(), b"x".to_vec())];
        assert!(export(&bad, &tmp.path().join("e")).is_err());
    }
}
