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
    /// A phase ran over its budget (contract M3b amendment 31). Not cached: the budget is not
    /// part of the cache key.
    Timeout {
        phase: Phase,
        budget_secs: u64,
    },
    /// No indexer can run for this job on this host; the reason is user-facing.
    Unsupported(String),
    /// The job was not attempted (SCIP disabled, or Docker unavailable); the reason is user-facing.
    Skipped(String),
    /// The container runtime failed rather than the project's build (the `docker` client could
    /// not start, or exited 125, 126 or 127: daemon error, command not invokable or not found;
    /// contract M3b amendment 30). Not cached; the reason is the stderr tail.
    Environment(String),
}

/// The two container phases of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Fetch,
    Index,
}

impl Phase {
    pub fn as_str(self) -> &'static str {
        match self {
            Phase::Fetch => "fetch",
            Phase::Index => "index",
        }
    }
}

impl RunStatus {
    /// The value stored in `rb.scip_index.status` and reported per project.
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Ok => "ok",
            RunStatus::Failed => "failed",
            RunStatus::Timeout { .. } => "timeout",
            RunStatus::Unsupported(_) => "unsupported",
            RunStatus::Skipped(_) => "skipped",
            RunStatus::Environment(_) => "environment",
        }
    }

    /// Whether this outcome is written to the index cache: only a finished run of the project's
    /// own build (`ok` or `failed`). A timeout depends on the budget, and an unsupported or
    /// environment outcome on the host, so they are decided again on the next run.
    pub fn is_cacheable(&self) -> bool {
        matches!(self, RunStatus::Ok | RunStatus::Failed)
    }

    /// The status a cache row recorded: `ok` or `failed`. `None` for any other value (a
    /// `timeout` written by an older binary), which the caller treats as a miss.
    pub fn from_cached(status: &str) -> Option<RunStatus> {
        match status {
            "ok" => Some(RunStatus::Ok),
            "failed" => Some(RunStatus::Failed),
            _ => None,
        }
    }

    /// Why the job did not finish, for every status but `ok` and `failed`.
    pub fn reason(&self) -> Option<String> {
        match self {
            RunStatus::Unsupported(r) | RunStatus::Skipped(r) | RunStatus::Environment(r) => {
                Some(r.clone())
            }
            RunStatus::Timeout { phase, budget_secs } => Some(format!(
                "{} phase over its {budget_secs} s budget",
                phase.as_str()
            )),
            RunStatus::Ok | RunStatus::Failed => None,
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

/// The indexer images on this host, looked up once per `rb index` through
/// [`Runner::image_id`] (contract M3b amendment 29): what each job runs in and caches under,
/// and the identity a snapshot's `tool_versions` records, so a graph indexed without an image
/// is not "up to date" once the image exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Images {
    /// scip-java: the local image's ID (it has no registry digest), or the user-facing reason
    /// its jobs are `unsupported`: the image is absent, and nothing builds it implicitly.
    pub java: Result<String, String>,
    /// scip-typescript: the pinned image's ID when it is present. Its jobs run by and cache
    /// under the digest reference either way (Docker pulls it when absent).
    pub typescript: Option<String>,
}

impl Images {
    pub fn resolve(runner: &dyn Runner) -> Images {
        Images {
            java: runner.image_id(SCIP_JAVA_IMAGE).ok_or_else(|| {
                format!(
                    "scip-java: image {SCIP_JAVA_IMAGE} is not available locally; build it as \
                     docker/scip-java/README.md describes"
                )
            }),
            typescript: runner.image_id(SCIP_TYPESCRIPT_IMAGE),
        }
    }

    /// No lookup was made (SCIP off, Docker unavailable): nothing is known to be present.
    pub fn unresolved() -> Images {
        Images {
            java: Err(format!(
                "scip-java: image {SCIP_JAVA_IMAGE} was not looked up"
            )),
            typescript: None,
        }
    }

    /// The image `job` runs in and its index is cached under: the digest reference itself for
    /// a pinned image, the image ID for the locally built scip-java one. `Err` is the reason
    /// the job is `unsupported`.
    pub fn for_job(&self, job: &Job) -> Result<String, String> {
        match job.indexer {
            Indexer::ScipTypescript => Ok(SCIP_TYPESCRIPT_IMAGE.to_owned()),
            Indexer::ScipJava => self.java.clone(),
        }
    }

    /// The identity recorded per indexer: the image ID, or `absent`.
    pub fn identity(&self, indexer: Indexer) -> &str {
        let id = match indexer {
            Indexer::ScipJava => self.java.as_deref().ok(),
            Indexer::ScipTypescript => self.typescript.as_deref(),
        };
        id.unwrap_or("absent")
    }
}

/// Runs `job` in `image` (from [`Images::for_job`]) over `files` (`(root-relative path, bytes)`)
/// with `cache_dir` holding package-manager caches shared across jobs. `work_dir` names the
/// job (the caller names it after the job's input hash); this run's export lives next to it in
/// a directory of its own, `<work_dir>-<pid>-<nanos>` (created, removed afterwards), and its
/// containers are named `rb-scip-<that name>-<phase>`, so two runs of the same job at once
/// share nothing (contract M3b amendment 22) and a run over its budget can be killed by name
/// (amendment 18).
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
    if job.fetch == Fetch::Gradle {
        // Only the Maven path is verified (contract M3b amendment 8); Gradle is M6.
        return Ok(ScipRun {
            status: RunStatus::Unsupported("scip-java Gradle builds are not verified yet".into()),
            index: None,
            stderr_tail: String::new(),
            elapsed: Duration::ZERO,
        });
    }
    let instance = format!(
        "{}-{}-{}",
        work_dir
            .file_name()
            .map(|n| n.to_string_lossy())
            .unwrap_or_default(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos())
    );
    let work_dir = &work_dir.with_file_name(&instance);
    export(files, work_dir)?;
    std::fs::create_dir_all(cache_dir)?;
    let name = format!("rb-scip-{instance}");
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
            let outcome = match runner.run(fetch) {
                Ok(outcome) => outcome,
                Err(e) => return Ok(environment(format!("{e:#}"), started)),
            };
            if !outcome.ok() {
                return Ok(finished(&outcome, None, Phase::Fetch, fetch, started));
            }
        }
        let outcome = match runner.run(&index) {
            Ok(outcome) => outcome,
            Err(e) => return Ok(environment(format!("{e:#}"), started)),
        };
        let index_bytes = if outcome.ok() {
            std::fs::read(work_dir.join("index.scip")).ok()
        } else {
            None
        };
        Ok(finished(
            &outcome,
            index_bytes,
            Phase::Index,
            &index,
            started,
        ))
    })();
    let _ = std::fs::remove_dir_all(work_dir);
    result
}

/// Exit codes the `docker` client reserves for its own failures (the daemon refused the run,
/// the command could not be invoked, the command was not found).
const DOCKER_CLIENT_EXIT_CODES: [i32; 3] = [125, 126, 127];

fn environment(reason: String, started: std::time::Instant) -> ScipRun {
    ScipRun {
        status: RunStatus::Environment(reason.trim().to_owned()),
        index: None,
        stderr_tail: String::new(),
        elapsed: started.elapsed(),
    }
}

fn finished(
    outcome: &Outcome,
    index: Option<Vec<u8>>,
    phase: Phase,
    run: &ContainerRun,
    started: std::time::Instant,
) -> ScipRun {
    if outcome.timed_out {
        return ScipRun {
            status: RunStatus::Timeout {
                phase,
                budget_secs: run.timeout.as_secs(),
            },
            index: None,
            stderr_tail: outcome.stderr_tail.trim().to_owned(),
            elapsed: started.elapsed(),
        };
    }
    if outcome
        .exit_code
        .is_some_and(|code| DOCKER_CLIENT_EXIT_CODES.contains(&code))
    {
        return environment(outcome.stderr_tail.clone(), started);
    }
    let status = if outcome.ok() && index.is_some() {
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
    // Type indexing needs no lifecycle scripts, and the fetch phase runs the project's own code
    // otherwise (contract M3b amendment 27).
    let quiet = [
        "--no-audit",
        "--no-fund",
        "--loglevel=error",
        "--ignore-scripts",
    ];
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
            [
                "install",
                "--frozen-lockfile",
                "--non-interactive",
                "--ignore-scripts",
            ]
            .map(String::from)
            .to_vec(),
        ),
        Fetch::Npm {
            lockfile: NodeLock::Pnpm,
        } => (
            "pnpm",
            ["install", "--frozen-lockfile", "--ignore-scripts"]
                .map(String::from)
                .to_vec(),
        ),
        Fetch::Npm {
            lockfile: NodeLock::None,
        } => (
            "/usr/local/bin/actual-npm",
            once("install").chain(quiet).map(String::from).collect(),
        ),
        Fetch::Maven | Fetch::Gradle => {
            anyhow::bail!("a Java fetch on a TypeScript job")
        }
    };
    let fetch = ContainerRun {
        entrypoint: Some(entrypoint.into()),
        args,
        ..base.clone()
    };
    // Projects follow `--`, so a directory named like an option (`--help`) is a project.
    let mut index_args: Vec<String> =
        ["index", "--output", "index.scip", "--no-progress-bar", "--"]
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
/// `[ERROR]` lines reach the stderr tail. scip-java prefers a repository's own `./mvnw` when
/// `access(X_OK)` says it is executable, which Docker Desktop's mounts say of every file, so
/// [`export`] leaves the wrapper out and the image's `mvn` is always the one used.
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

/// Build wrappers scip-java would run instead of the image's tools when they sit in the build
/// root (contract M3b amendment 25); they download a build tool and are not exported.
const BUILD_WRAPPERS: [&str; 4] = ["mvnw", "mvnw.cmd", "gradlew", "gradlew.bat"];

/// Writes `files` under `dir`, which is recreated empty first. Paths never escape `dir`. Files
/// are written with default permissions; the build wrappers at the root are skipped.
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
        if BUILD_WRAPPERS.contains(&path.as_str()) {
            continue;
        }
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
    /// unless told to fail. Knows the scip-java image when `java_image` is set. `fetch_outcome`
    /// replaces the fetch phase's outcome (a runtime failure, a timeout) when set.
    struct Fake {
        runs: RefCell<Vec<ContainerRun>>,
        fail_fetch: bool,
        fail_index: bool,
        java_image: bool,
        fetch_outcome: Option<anyhow::Result<Outcome>>,
    }

    impl Fake {
        fn new(fail_fetch: bool, fail_index: bool) -> Fake {
            Fake {
                runs: RefCell::new(Vec::new()),
                fail_fetch,
                fail_index,
                java_image: true,
                fetch_outcome: None,
            }
        }
    }

    impl Runner for Fake {
        fn run(&self, run: &ContainerRun) -> anyhow::Result<Outcome> {
            self.runs.borrow_mut().push(run.clone());
            let is_index = run.network == Network::None;
            if !is_index && let Some(outcome) = &self.fetch_outcome {
                return match outcome {
                    Ok(o) => Ok(o.clone()),
                    Err(e) => Err(anyhow::anyhow!("{e}")),
                };
            }
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
        let images = Images::resolve(&fake);
        let image = images.for_job(&job).unwrap();
        assert_eq!(
            image, SCIP_TYPESCRIPT_IMAGE,
            "a pinned image is its own key"
        );
        assert_eq!(
            images.identity(Indexer::ScipTypescript),
            "absent",
            "the fake has no scip-typescript image"
        );
        assert_eq!(images.identity(Indexer::ScipJava), JAVA_ID);
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
        let runs = fake.runs.borrow();
        assert_eq!(runs.len(), 2);
        // The export lived in `work-<pid>-<nanos>` next to `work`, and is gone.
        let exported = runs[0].workdir.file_name().unwrap().to_string_lossy();
        assert!(exported.starts_with("work-"), "{exported}");
        assert!(!work.exists() && !runs[0].workdir.exists());
        assert_eq!(
            runs[0].name.as_deref(),
            Some(format!("rb-scip-{exported}-fetch").as_str())
        );
        assert_eq!(
            runs[1].name.as_deref(),
            Some(format!("rb-scip-{exported}-index").as_str())
        );
        assert_eq!(runs[0].network, Network::Registries);
        assert_eq!(
            runs[0].entrypoint.as_deref(),
            Some("/usr/local/bin/actual-npm")
        );
        assert_eq!(runs[0].args[0], "ci");
        assert_eq!(runs[1].network, Network::None);
        // Contract M3b amendment 36: a project directory named like an option (`--help`) is
        // not taken for one.
        assert_eq!(
            runs[1].args,
            [
                "index",
                "--output",
                "index.scip",
                "--no-progress-bar",
                "--",
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
        let job = java_job(Fetch::Maven);
        let image = Images::resolve(&fake).for_job(&job).unwrap();
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
        let names: Vec<&str> = runs.iter().map(|r| r.name.as_deref().unwrap()).collect();
        assert_eq!(
            names[0], names[1],
            "sequential fetch runs reuse the name; `--rm` removes each before the next"
        );
        assert!(names[0].starts_with("rb-scip-work-") && names[0].ends_with("-fetch"));
        assert_eq!(
            names[2],
            names[0].replace("-fetch", "-index"),
            "the index phase runs under the same instance"
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

    /// Contract M3b amendment 22: two runs of the same job (the same input hash, so the same
    /// `work_dir` argument) export into distinct directories and name their containers
    /// distinctly, so concurrent runs of one commit neither clobber each other's export nor
    /// collide on `--name`; the name the runner kills on a budget overrun is the instance's.
    #[test]
    fn each_run_of_a_job_has_its_own_work_directory_and_container_names() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("0123456789abcdef");
        let cache = tmp.path().join("cache");
        let fake = Fake::new(false, false);
        let job = ts_job(NodeLock::PackageLock);
        for _ in 0..2 {
            let run = run_job(
                &job,
                SCIP_TYPESCRIPT_IMAGE,
                &files(),
                &work,
                &cache,
                &Budget::default(),
                &fake,
            )
            .unwrap();
            assert_eq!(run.status, RunStatus::Ok);
        }
        let runs = fake.runs.borrow();
        assert_eq!(runs.len(), 4, "fetch and index, twice");
        let instances: Vec<String> = runs
            .iter()
            .map(|r| {
                r.workdir
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(instances[0], instances[1], "one instance per run_job");
        assert_eq!(instances[2], instances[3]);
        assert_ne!(instances[0], instances[2], "a new instance per run_job");
        let pid = std::process::id().to_string();
        for (r, instance) in runs.iter().zip(&instances) {
            let rest = instance
                .strip_prefix("0123456789abcdef-")
                .unwrap_or_else(|| panic!("{instance}"));
            let (p, nanos) = rest.split_once('-').unwrap_or_else(|| panic!("{instance}"));
            assert_eq!(p, pid, "{instance}");
            assert!(
                !nanos.is_empty() && nanos.chars().all(|c| c.is_ascii_digit()),
                "{instance}"
            );
            let phase = if r.network == Network::None {
                "index"
            } else {
                "fetch"
            };
            assert_eq!(
                r.name.as_deref(),
                Some(format!("rb-scip-{instance}-{phase}").as_str())
            );
        }
        let left: Vec<String> = tmp
            .path()
            .read_dir()
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n != "cache")
            .collect();
        assert!(
            left.is_empty(),
            "instance directories left behind: {left:?}"
        );
    }

    /// A build failure in the second fetch run stops before the index; nothing runs offline.
    #[test]
    fn a_failed_maven_build_stops_before_indexing() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(true, false);
        let run = run_job(
            &java_job(Fetch::Maven),
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
            &java_job(Fetch::Gradle),
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
        let images = Images::resolve(&no_image);
        let reason = images.for_job(&java_job(Fetch::Maven)).unwrap_err();
        assert!(
            reason.contains("rb-scip-java:0.13.1") && reason.contains("docker/scip-java/README.md"),
            "{reason}"
        );
        assert_eq!(images.identity(Indexer::ScipJava), "absent");
        assert_eq!(
            images.for_job(&ts_job(NodeLock::None)).as_deref(),
            Ok(SCIP_TYPESCRIPT_IMAGE),
            "a pinned image runs by its digest reference whether or not it is present"
        );

        let bad = vec![("../escape".to_owned(), b"x".to_vec())];
        assert!(export(&bad, &tmp.path().join("e")).is_err());
    }

    /// Contract M3b amendment 25: a repository's Maven or Gradle wrapper at the build root is
    /// not exported, so scip-java can only use the image's `mvn`; the same names deeper in the
    /// tree are ordinary files.
    #[test]
    fn export_skips_the_build_wrappers_at_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let files: Vec<(String, Vec<u8>)> = [
            "pom.xml",
            "mvnw",
            "mvnw.cmd",
            "gradlew",
            "gradlew.bat",
            "src/Main.java",
            "sub/mvnw",
            ".mvn/wrapper/maven-wrapper.properties",
        ]
        .map(|p| (p.to_owned(), b"x".to_vec()))
        .to_vec();
        let dir = tmp.path().join("e");
        export(&files, &dir).unwrap();
        let mut written: Vec<String> = walkdir(&dir)
            .into_iter()
            .map(|p| p.strip_prefix(&dir).unwrap().to_string_lossy().into_owned())
            .collect();
        written.sort();
        assert_eq!(
            written,
            [
                ".mvn/wrapper/maven-wrapper.properties",
                "pom.xml",
                "src/Main.java",
                "sub/mvnw"
            ]
        );
    }

    fn walkdir(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.extend(walkdir(&path));
            } else {
                out.push(path);
            }
        }
        out
    }

    /// Contract M3b amendment 27: the fetch phase installs packages without their lifecycle
    /// scripts, for every lockfile kind.
    #[test]
    fn npm_fetches_run_without_lifecycle_scripts() {
        let tmp = tempfile::tempdir().unwrap();
        for (lockfile, entrypoint, first) in [
            (NodeLock::PackageLock, "/usr/local/bin/actual-npm", "ci"),
            (NodeLock::None, "/usr/local/bin/actual-npm", "install"),
            (NodeLock::Yarn, "yarn", "install"),
            (NodeLock::Pnpm, "pnpm", "install"),
        ] {
            let fake = Fake::new(false, false);
            run_job(
                &ts_job(lockfile),
                SCIP_TYPESCRIPT_IMAGE,
                &files(),
                &tmp.path().join("w"),
                &tmp.path().join("c"),
                &Budget::default(),
                &fake,
            )
            .unwrap();
            let runs = fake.runs.borrow();
            let fetch = &runs[0];
            assert_eq!(
                fetch.entrypoint.as_deref(),
                Some(entrypoint),
                "{lockfile:?}"
            );
            assert_eq!(fetch.args[0], first, "{lockfile:?}");
            assert!(
                fetch.args.iter().any(|a| a == "--ignore-scripts"),
                "{lockfile:?}: {:?}",
                fetch.args
            );
        }
    }

    /// Contract M3b amendments 30 and 31: a `docker run` that the client or daemon refused
    /// (exit 125/126/127), or a client that could not start, is an `environment` outcome with
    /// the stderr tail as its reason; a phase over its budget is a `timeout` that names the
    /// phase and the budget. Neither is the project's fault (the caller does not cache them).
    #[test]
    fn runtime_failures_are_environment_and_timeouts_name_their_phase() {
        let tmp = tempfile::tempdir().unwrap();
        let outcome = |exit_code: Option<i32>, timed_out: bool, stderr: &str| Outcome {
            exit_code,
            timed_out,
            stderr_tail: stderr.into(),
            elapsed: Duration::from_millis(1),
        };
        let run_with = |fetch_outcome: anyhow::Result<Outcome>| {
            let fake = Fake {
                fetch_outcome: Some(fetch_outcome),
                ..Fake::new(false, false)
            };
            let budget = Budget {
                fetch_timeout: Duration::from_secs(7),
                ..Budget::default()
            };
            let run = run_job(
                &ts_job(NodeLock::PackageLock),
                SCIP_TYPESCRIPT_IMAGE,
                &files(),
                &tmp.path().join("w"),
                &tmp.path().join("c"),
                &budget,
                &fake,
            )
            .unwrap();
            assert_eq!(fake.runs.borrow().len(), 1, "nothing runs after the fetch");
            run.status
        };
        for code in [125, 126, 127] {
            assert_eq!(
                run_with(Ok(outcome(Some(code), false, "shim: docker run refused"))),
                RunStatus::Environment("shim: docker run refused".into()),
                "exit {code}"
            );
        }
        assert_eq!(
            run_with(Err(anyhow::anyhow!("starting docker: No such file"))),
            RunStatus::Environment("starting docker: No such file".into())
        );
        assert_eq!(
            run_with(Ok(outcome(None, true, ""))),
            RunStatus::Timeout {
                phase: Phase::Fetch,
                budget_secs: 7
            }
        );
        // A build that fails with any other code is the project's (cached by the caller).
        assert_eq!(
            run_with(Ok(outcome(Some(1), false, "[ERROR] boom"))),
            RunStatus::Failed
        );
        assert_eq!(
            RunStatus::Timeout {
                phase: Phase::Index,
                budget_secs: 900
            }
            .reason()
            .as_deref(),
            Some("index phase over its 900 s budget")
        );
    }
}
