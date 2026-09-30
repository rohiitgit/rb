//! Running one job (design §5.2): export the build root's files into a work directory that the
//! container VM can see, fetch dependencies with the network on, index with it off, and collect
//! `index.scip`. Nothing here talks to the database; the caller owns the cache.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::jobs::{Fetch, Indexer, Job, NodeLock, SCIP_JAVA_IMAGE, SCIP_TYPESCRIPT_IMAGE};
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

/// The image reference a job uses: scip-typescript's registry digest, scip-java's local tag.
pub fn image_for(job: &Job) -> &'static str {
    match job.indexer {
        Indexer::ScipTypescript => SCIP_TYPESCRIPT_IMAGE,
        Indexer::ScipJava => SCIP_JAVA_IMAGE,
    }
}

/// The image `job` runs in and its index is cached under: the digest reference itself for a
/// pinned image, the image ID for the locally built scip-java one. `Err` is the user-facing
/// reason the job is `unsupported`: the local image is absent, and nothing builds it implicitly.
pub fn resolve_image(job: &Job, runner: &dyn Runner) -> Result<String, String> {
    let image = image_for(job);
    match job.indexer {
        Indexer::ScipTypescript => Ok(image.to_owned()),
        Indexer::ScipJava => runner.image_id(image).ok_or_else(|| {
            format!(
                "scip-java: image {image} is not available locally; build it as \
                 docker/scip-java/README.md describes"
            )
        }),
    }
}

/// Runs `job` in `image` (from [`resolve_image`]) over `files` (`(root-relative path, bytes)`)
/// in `work_dir` (recreated, removed afterwards) with `cache_dir` holding package-manager
/// caches shared across jobs. The containers are named `rb-scip-<work dir name>-<phase>` (the
/// caller names the work directory after the job's input hash), so a run over its budget can
/// be killed by name (contract M3b amendment 18).
pub fn run_job(
    job: &Job,
    image: &str,
    files: &[(String, Vec<u8>)],
    work_dir: &Path,
    cache_dir: &Path,
    budget: &Budget,
    runner: &dyn Runner,
) -> anyhow::Result<ScipRun> {
    let started = std::time::Instant::now();
    if let Fetch::Gradle { .. } = job.fetch {
        // Only the Maven path is verified (contract M3b amendment 8); Gradle is M6.
        return Ok(ScipRun {
            status: RunStatus::Unsupported("scip-java Gradle builds are not verified yet".into()),
            index: None,
            stderr_tail: String::new(),
            elapsed: Duration::ZERO,
        });
    }
    export(files, work_dir)?;
    std::fs::create_dir_all(cache_dir)?;
    let name = format!(
        "rb-scip-{}",
        work_dir
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default()
    );
    let result = (|| -> anyhow::Result<ScipRun> {
        let (mut fetches, mut index) = match job.indexer {
            Indexer::ScipTypescript => typescript_runs(job, image, work_dir, cache_dir, budget)?,
            Indexer::ScipJava => java_runs(image, work_dir, cache_dir, budget)?,
        };
        for fetch in &mut fetches {
            fetch.name = Some(format!("{name}-fetch"));
        }
        index.name = Some(format!("{name}-index"));
        for fetch in &fetches {
            let outcome = runner.run(fetch)?;
            if !outcome.ok() {
                return Ok(finished(&outcome, None, started));
            }
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
    image: &str,
    work_dir: &Path,
    cache_dir: &Path,
    budget: &Budget,
) -> anyhow::Result<(Vec<ContainerRun>, ContainerRun)> {
    let workdir = crate::runner::absolute(work_dir)?;
    let npm_cache = crate::runner::absolute(cache_dir)?.join("npm");
    std::fs::create_dir_all(&npm_cache)?;
    let base = ContainerRun {
        name: None,
        image: image.into(),
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
    Ok((vec![fetch], index))
}

/// The fetch and index invocations for a scip-java Maven job (docker/scip-java/README.md,
/// verified on the Java fixture). The fetch phase resolves plugins and dependencies into the
/// Maven cache shared across jobs (`go-offline`) and then does one online build, so that the
/// index phase can run the same build with `--offline` under `--network none`. Arguments after
/// `--` replace scip-java's default Maven arguments, so they are repeated plus `--offline`.
/// Maven logs to stdout by default; `MAVEN_OPTS` moves its logger to stderr so a failed build's
/// `[ERROR]` lines reach the stderr tail. scip-java prefers a repository's own `./mvnw` when it
/// is executable, which would download Maven; exported files are never executable, so the
/// image's `mvn` is always the one used.
fn java_runs(
    image: &str,
    work_dir: &Path,
    cache_dir: &Path,
    budget: &Budget,
) -> anyhow::Result<(Vec<ContainerRun>, ContainerRun)> {
    let workdir = crate::runner::absolute(work_dir)?;
    let m2 = crate::runner::absolute(cache_dir)?.join("m2");
    std::fs::create_dir_all(&m2)?;
    let base = ContainerRun {
        name: None,
        image: image.into(),
        platform: None,
        workdir,
        mount_at: "/sources".into(),
        caches: vec![(m2, "/root/.m2".into())],
        env: vec![(
            "MAVEN_OPTS".into(),
            "-Dorg.slf4j.simpleLogger.logFile=System.err".into(),
        )],
        network: Network::Registries,
        cpus: budget.cpus,
        memory: budget.memory.clone(),
        timeout: budget.fetch_timeout,
        entrypoint: None,
        args: Vec::new(),
    };
    let mvn = |goals: &[&str]| ContainerRun {
        args: ["mvn", "-B", "-q"]
            .iter()
            .chain(goals)
            .map(|s| (*s).to_owned())
            .collect(),
        ..base.clone()
    };
    let fetches = vec![
        mvn(&["org.apache.maven.plugins:maven-dependency-plugin:3.11.0:go-offline"]),
        mvn(&["clean", "verify", "-DskipTests"]),
    ];
    let index = ContainerRun {
        network: Network::None,
        timeout: budget.index_timeout,
        args: [
            "scip-java",
            "index",
            "--build-tool",
            "Maven",
            "--",
            "--batch-mode",
            "--offline",
            "clean",
            "verify",
            "-DskipTests",
        ]
        .map(String::from)
        .to_vec(),
        ..base
    };
    Ok((fetches, index))
}

fn once(s: &str) -> std::iter::Once<&str> {
    std::iter::once(s)
}

/// Writes `files` under `dir`, which is recreated empty first. Paths never escape `dir`. Files
/// are written with default permissions, never executable.
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

    const JAVA_ID: &str = "sha256:66a0b395166fc65c49cc099b0b3e4d2cb61e16c177dbc3c022160bb0b4cd1cf2";

    /// Records runs; the index phase (the one without network) writes a fake `index.scip`
    /// unless told to fail. Knows the scip-java image when `java_image` is set.
    struct Fake {
        runs: RefCell<Vec<ContainerRun>>,
        fail_fetch: bool,
        fail_index: bool,
        java_image: bool,
    }

    impl Fake {
        fn new(fail_fetch: bool, fail_index: bool) -> Fake {
            Fake {
                runs: RefCell::new(Vec::new()),
                fail_fetch,
                fail_index,
                java_image: true,
            }
        }
    }

    impl Runner for Fake {
        fn run(&self, run: &ContainerRun) -> anyhow::Result<Outcome> {
            self.runs.borrow_mut().push(run.clone());
            let is_index = run.network == Network::None;
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

        fn image_id(&self, image: &str) -> Option<String> {
            (self.java_image && image == SCIP_JAVA_IMAGE).then(|| JAVA_ID.to_owned())
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

    fn java_job(fetch: Fetch) -> Job {
        Job {
            root: "svc".into(),
            indexer: Indexer::ScipJava,
            fetch,
            projects: Vec::new(),
            unpinned: false,
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
        let fake = Fake::new(false, false);
        let job = ts_job(NodeLock::PackageLock);
        let image = resolve_image(&job, &fake).unwrap();
        assert_eq!(
            image, SCIP_TYPESCRIPT_IMAGE,
            "a pinned image is its own key"
        );
        let run = run_job(
            &job,
            &image,
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
        assert_eq!(runs[0].name.as_deref(), Some("rb-scip-work-fetch"));
        assert_eq!(runs[1].name.as_deref(), Some("rb-scip-work-index"));
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
        let fake = Fake::new(true, false);
        let run = run_job(
            &ts_job(NodeLock::None),
            SCIP_TYPESCRIPT_IMAGE,
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
        let fake = Fake::new(false, true);
        let run = run_job(
            &ts_job(NodeLock::Yarn),
            SCIP_TYPESCRIPT_IMAGE,
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

    /// A Maven job: two online Maven runs into the shared `.m2`, then scip-java offline in the
    /// same image, all under `/sources`; the image ID resolved at run time is what runs.
    #[test]
    fn maven_jobs_fetch_online_then_index_offline_in_the_local_image() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let cache = tmp.path().join("cache");
        let fake = Fake::new(false, false);
        let job = java_job(Fetch::Maven { wrapper: false });
        let image = resolve_image(&job, &fake).unwrap();
        assert_eq!(image, JAVA_ID);
        let budget = Budget {
            cpus: 3.0,
            memory: "6g".into(),
            fetch_timeout: Duration::from_secs(10),
            index_timeout: Duration::from_secs(20),
        };
        let run = run_job(&job, &image, &files(), &work, &cache, &budget, &fake).unwrap();
        assert_eq!(run.status, RunStatus::Ok);
        assert_eq!(run.index.as_deref(), Some(&b"fake-index"[..]));
        assert!(!work.exists());
        let runs = fake.runs.borrow();
        assert_eq!(runs.len(), 3);
        let names: Vec<Option<&str>> = runs.iter().map(|r| r.name.as_deref()).collect();
        assert_eq!(
            names,
            [
                Some("rb-scip-work-fetch"),
                Some("rb-scip-work-fetch"),
                Some("rb-scip-work-index")
            ],
            "sequential fetch runs reuse the name; `--rm` removes each before the next"
        );
        for r in runs.iter() {
            assert_eq!(r.image, JAVA_ID);
            assert_eq!(r.platform, None, "scip-java runs natively");
            assert_eq!(r.mount_at, "/sources");
            assert_eq!(r.caches[0].1, "/root/.m2");
            assert!(r.caches[0].0.ends_with("cache/m2"));
            assert_eq!(r.entrypoint, None);
            assert_eq!(r.env[0].0, "MAVEN_OPTS");
            assert!(r.env[0].1.contains("logFile=System.err"));
            assert_eq!((r.cpus, r.memory.as_str()), (3.0, "6g"));
        }
        assert_eq!(runs[0].network, Network::Registries);
        assert_eq!(
            runs[0].args,
            [
                "mvn",
                "-B",
                "-q",
                "org.apache.maven.plugins:maven-dependency-plugin:3.11.0:go-offline"
            ]
        );
        assert_eq!(runs[0].timeout, Duration::from_secs(10));
        assert_eq!(runs[1].network, Network::Registries);
        assert_eq!(
            runs[1].args,
            ["mvn", "-B", "-q", "clean", "verify", "-DskipTests"]
        );
        assert_eq!(runs[2].network, Network::None);
        assert_eq!(runs[2].timeout, Duration::from_secs(20));
        assert_eq!(
            runs[2].args,
            [
                "scip-java",
                "index",
                "--build-tool",
                "Maven",
                "--",
                "--batch-mode",
                "--offline",
                "clean",
                "verify",
                "-DskipTests"
            ]
        );
        assert!(
            cache.join("m2").is_dir(),
            "the Maven cache directory is created"
        );
    }

    /// A build failure in the second fetch run stops before the index; nothing runs offline.
    #[test]
    fn a_failed_maven_build_stops_before_indexing() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(true, false);
        let run = run_job(
            &java_job(Fetch::Maven { wrapper: true }),
            JAVA_ID,
            &files(),
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            &Budget::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(run.status, RunStatus::Failed);
        assert_eq!(run.stderr_tail, "boom");
        assert_eq!(
            fake.runs.borrow().len(),
            1,
            "go-offline failed: nothing else runs"
        );
    }

    #[test]
    fn gradle_jobs_and_a_missing_java_image_are_unsupported_and_exports_refuse_unsafe_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(false, false);
        let run = run_job(
            &java_job(Fetch::Gradle { wrapper: true }),
            JAVA_ID,
            &files(),
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            &Budget::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(
            run.status,
            RunStatus::Unsupported("scip-java Gradle builds are not verified yet".into())
        );
        assert!(fake.runs.borrow().is_empty());
        assert!(!tmp.path().join("w").exists(), "nothing is exported");

        let no_image = Fake {
            java_image: false,
            ..Fake::new(false, false)
        };
        let reason =
            resolve_image(&java_job(Fetch::Maven { wrapper: false }), &no_image).unwrap_err();
        assert!(
            reason.contains("rb-scip-java:0.13.1") && reason.contains("docker/scip-java/README.md"),
            "{reason}"
        );
        assert_eq!(
            resolve_image(&ts_job(NodeLock::None), &no_image).as_deref(),
            Ok(SCIP_TYPESCRIPT_IMAGE),
            "a pinned image needs no lookup"
        );

        let bad = vec![("../escape".to_owned(), b"x".to_vec())];
        assert!(export(&bad, &tmp.path().join("e")).is_err());
    }
}
