//! Running one job (design §5.2): export the build root's files into a work directory that the
//! container VM can see, fetch dependencies with the network on, index with it off, and collect
//! `index.scip`. Nothing here talks to the database; the caller owns the cache.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::egress::{CLIENT_CONFIG_AT, Egress, PROXY_IMAGE};
use crate::jobs::{Fetch, Indexer, Job, NodeLock, SCIP_GO_IMAGE, SCIP_JAVA_IMAGE, SCIP_PYTHON_IMAGE, SCIP_TYPESCRIPT_IMAGE};
use crate::runner::{ContainerRun, Network, Outcome, ProxySpec, Runner};

/// scip-java's fetch and index recipe, part of each of its jobs' cache keys (contract M3b
/// amendment 47; one per indexer since contract M6a D8). Bump it whenever scip-java's container
/// commands, its image's invocation or its export rules change: an index produced by another
/// recipe is then a cache miss, never served as "cached". `m5-egress-1`: the fetch phase behind
/// the egress proxy, per-repository dependency caches and the Develocity/Gradle Enterprise
/// build caches off in both Maven phases (contract M5 S21, S22). Unchanged by M6a, so cached
/// Java indexes stay valid (a unit pins the key of a fixed input).
pub const RECIPE_JAVA: &str = "m5-egress-1";

/// scip-typescript's recipe (contract M6a D8), bumped like [`RECIPE_JAVA`]. `m6a-ts-1`:
/// workspace roots fetch and index their members (D5, D6) and exports are repository-shaped
/// (D7).
pub const RECIPE_TS: &str = "m6a-ts-1";

/// scip-go's recipe (contract M6d S1/S3): `go mod download`, then `scip-go`
/// offline. Bumped like [`RECIPE_JAVA`].
pub const RECIPE_GO: &str = "m6d-go-1";

/// scip-python's recipe (contract M6d S2/S4): environment resolution, then
/// `scip-python index` offline. Bumped like [`RECIPE_JAVA`].
pub const RECIPE_PY: &str = "m6d-py-1";

/// The recipe a job's cache key carries: scip-java's gains `+T<n>` when its Maven builds run
/// with `n` > 1 threads (contract M6a D9), so no cached index crosses thread settings; the
/// other budget settings change no index and are not part of it.
pub fn recipe(indexer: Indexer, budget: &Budget) -> String {
    match indexer {
        Indexer::ScipJava if budget.maven_threads > 1 => {
            format!("{RECIPE_JAVA}+T{}", budget.maven_threads)
        }
        Indexer::ScipJava => RECIPE_JAVA.to_owned(),
        Indexer::ScipGo => RECIPE_GO.to_owned(),
        Indexer::ScipPython => RECIPE_PY.to_owned(),
        Indexer::ScipTypescript => RECIPE_TS.to_owned(),
    }
}

/// The Develocity and Gradle Enterprise Maven extensions' build caches, off in both Maven
/// phases (contract M5 S22): a cache hit skips `javac`, so its sources get no SCIP document
/// (spring-retry's test tree, contract M3b open items), and the remote cache is a host outside
/// the allow-list anyway.
const BUILD_CACHES_OFF: [&str; 4] = [
    "-Ddevelocity.cache.local.enabled=false",
    "-Ddevelocity.cache.remote.enabled=false",
    "-Dgradle.cache.local.enabled=false",
    "-Dgradle.cache.remote.enabled=false",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunStatus {
    Ok,
    /// The fetch or index phase failed (contract M3b amendment 56: which one is recorded, as
    /// for a timeout; an index that was not produced or could not be decoded is the index
    /// phase's); `stderr_tail` says why.
    Failed(Phase),
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
    /// contract M3b amendment 30). Not cached. The reason is the first line of the client's
    /// text with absolute paths redacted (amendment 49); the run's `stderr_tail` keeps the
    /// whole text for the terminal.
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

    /// The phase [`Phase::as_str`] names; `None` for any other text.
    pub fn parse(text: &str) -> Option<Phase> {
        match text {
            "fetch" => Some(Phase::Fetch),
            "index" => Some(Phase::Index),
            _ => None,
        }
    }
}

impl RunStatus {
    /// The value stored in `rb.scip_index.status` and reported per project.
    pub fn as_str(&self) -> &'static str {
        match self {
            RunStatus::Ok => "ok",
            RunStatus::Failed(_) => "failed",
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
        matches!(self, RunStatus::Ok | RunStatus::Failed(_))
    }

    /// The phase a `failed` or `timeout` job stopped in.
    pub fn phase(&self) -> Option<Phase> {
        match self {
            RunStatus::Failed(phase) | RunStatus::Timeout { phase, .. } => Some(*phase),
            _ => None,
        }
    }

    /// The status a cache row recorded: `ok`, or `failed` with its phase. `None` for any other
    /// value (a `timeout`, or a `failed` without a phase, written by an older binary), which
    /// the caller treats as a miss.
    pub fn from_cached(status: &str, phase: Option<&str>) -> Option<RunStatus> {
        match status {
            "ok" => Some(RunStatus::Ok),
            "failed" => phase.and_then(Phase::parse).map(RunStatus::Failed),
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
            RunStatus::Ok | RunStatus::Failed(_) => None,
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

/// Resource budget per phase, set on the indexing host only (`rb index --scip-cpus`,
/// `--scip-memory`, `--scip-maven-threads`, `--scip-timeout-secs`; contract M6a D9): the
/// indexed repository cannot raise its own budget.
#[derive(Debug, Clone, PartialEq)]
pub struct Budget {
    pub cpus: f32,
    pub memory: String,
    pub fetch_timeout: Duration,
    pub index_timeout: Duration,
    /// Maven's reactor threads (`-T`) in both scip-java phases; 1 adds no flag.
    pub maven_threads: u8,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            cpus: 2.0,
            memory: "4g".into(),
            fetch_timeout: Duration::from_secs(600),
            index_timeout: Duration::from_secs(900),
            maven_threads: 1,
        }
    }
}

/// The image reference a job uses: scip-typescript's registry digest, scip-java's local tag.
pub fn image_for(job: &Job) -> &'static str {
    match job.indexer {
        Indexer::ScipTypescript => SCIP_TYPESCRIPT_IMAGE,
        Indexer::ScipJava => SCIP_JAVA_IMAGE,
        Indexer::ScipGo => SCIP_GO_IMAGE,
        Indexer::ScipPython => SCIP_PYTHON_IMAGE,
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
    /// scip-typescript needs no field: its jobs run by, cache under and are identified by the
    /// pinned digest reference (Docker pulls it when absent; contract M3b amendment 48).
    pub java: Result<String, String>,
    /// scip-go: like scip-java, a locally built image identified by ID.
    pub go: Result<String, String>,
    /// The egress proxy image (contract M5 S21): its ID, or the reason every job is
    /// `unsupported` (no fetch runs without the proxy; nothing builds it implicitly).
    pub proxy: Result<String, String>,
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
            go: runner.image_id(SCIP_GO_IMAGE).ok_or_else(|| {
                format!(
                    "scip-go: image {SCIP_GO_IMAGE} is not available locally; build it as \
                     docker/scip-go/README.md describes"
                )
            }),
            proxy: runner.image_id(PROXY_IMAGE).ok_or_else(|| {
                format!(
                    "egress proxy: image {PROXY_IMAGE} is not available locally; build it as \
                     docker/egress-proxy/README.md describes"
                )
            }),
        }
    }

    /// No lookup was made (SCIP off, Docker unavailable): nothing is known to be present.
    pub fn unresolved() -> Images {
        Images {
            java: Err(format!(
                "scip-java: image {SCIP_JAVA_IMAGE} was not looked up"
            )),
            go: Err(format!(
                "scip-go: image {SCIP_GO_IMAGE} was not looked up"
            )),
            proxy: Err(format!(
                "egress proxy: image {PROXY_IMAGE} was not looked up"
            )),
        }
    }

    /// The image `job` runs in and its index is cached under: the digest reference itself for
    /// a pinned image, the image ID for the locally built scip-java one. `Err` is the reason
    /// the job is `unsupported`: the indexer image, or the egress proxy image, is absent.
    pub fn for_job(&self, job: &Job) -> Result<String, String> {
        let image = match job.indexer {
            Indexer::ScipTypescript => Ok(SCIP_TYPESCRIPT_IMAGE.to_owned()),
            Indexer::ScipPython => Ok(SCIP_PYTHON_IMAGE.to_owned()),
            Indexer::ScipJava => self.java.clone(),
            Indexer::ScipGo => self.go.clone(),
        }?;
        self.proxy.as_ref().map_err(Clone::clone)?;
        Ok(image)
    }

    /// The identity recorded per indexer: the digest reference of a pinned image, the image ID
    /// of the locally built one, or `absent`.
    pub fn identity(&self, indexer: Indexer) -> &str {
        match indexer {
            Indexer::ScipJava => self.java.as_deref().unwrap_or("absent"),
            Indexer::ScipGo => self.go.as_deref().unwrap_or("absent"),
            Indexer::ScipTypescript => SCIP_TYPESCRIPT_IMAGE,
            Indexer::ScipPython => SCIP_PYTHON_IMAGE,
        }
    }
}

/// Runs `job` in `image` (from [`Images::for_job`]) over `files` (`(path, bytes)` as the
/// container sees them under its mount: root-relative for a scip-java job, repository paths for
/// a scip-typescript job, whose container starts in its root; contract M6a D7)
/// with `cache_dir` holding the repository's package-manager caches (contract M5 S22: the
/// caller keys it by repository). `work_dir` names the job (the caller names it after the
/// job's input hash); this run's export lives next to it in a directory of its own,
/// `<work_dir>-<pid>-<nanos>` (created, removed afterwards), and its containers are named
/// `rb-scip-<that name>-<phase>`, so two runs of the same job at once share nothing (contract
/// M3b amendment 22) and a run over its budget can be killed by name (amendment 18). The fetch
/// phase runs behind its own egress proxy, `rb-egress-<that name>`, allowing `egress`'s hosts
/// (contract M5 S21); its configuration lives in `<that directory>-egress` while it runs.
#[allow(clippy::too_many_arguments)]
pub fn run_job(
    job: &Job,
    image: &str,
    files: &[(String, Vec<u8>)],
    work_dir: &Path,
    cache_dir: &Path,
    budget: &Budget,
    egress: &Egress,
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
    let egress_dir = work_dir.with_file_name(format!("{instance}-egress"));
    let work_dir = &work_dir.with_file_name(&instance);
    export(files, work_dir)?;
    std::fs::create_dir_all(cache_dir)?;
    let name = format!("rb-scip-{instance}");
    let result = (|| -> anyhow::Result<ScipRun> {
        let (mut fetches, mut index) = match job.indexer {
            Indexer::ScipTypescript => typescript_runs(job, image, work_dir, cache_dir, budget)?,
            Indexer::ScipJava => java_runs(image, work_dir, cache_dir, budget)?,
            Indexer::ScipGo => go_runs(image, work_dir, cache_dir, budget)?,
            Indexer::ScipPython => python_runs(job, image, work_dir, cache_dir, budget)?,
        };
        for fetch in &mut fetches {
            fetch.name = Some(format!("{name}-fetch"));
        }
        index.name = Some(format!("{name}-index"));
        // The root's directory exists in a real export; created so the phases can start there.
        std::fs::create_dir_all(index.host_cwd())?;
        // The fetch phase behind this run's own proxy (contract M5 S21).
        let proxy = ProxySpec {
            name: format!("rb-egress-{instance}"),
            image: PROXY_IMAGE.to_owned(),
            config_dir: egress_dir.join("proxy"),
        };
        egress.write_config(&proxy.config_dir)?;
        let address = match runner.start_proxy(&proxy) {
            Ok(address) => address,
            Err(e) => return Ok(environment(format!("egress proxy: {e:#}"), started)),
        };
        let fetched = (|| -> anyhow::Result<Option<ScipRun>> {
            let client = crate::runner::absolute(&{
                let dir = egress_dir.join("client");
                std::fs::create_dir_all(&dir)?;
                std::fs::write(
                    dir.join("settings.xml"),
                    crate::egress::maven_settings(&address),
                )?;
                dir
            })?;
            for fetch in &mut fetches {
                fetch.env.extend(crate::egress::proxy_env(&address));
                fetch
                    .readonly
                    .push((client.clone(), CLIENT_CONFIG_AT.to_owned()));
                let outcome = match runner.run(fetch) {
                    Ok(outcome) => outcome,
                    Err(e) => return Ok(Some(environment(format!("{e:#}"), started))),
                };
                if !outcome.ok() {
                    return Ok(Some(finished(&outcome, None, Phase::Fetch, fetch, started)));
                }
            }
            Ok(None)
        })();
        let log = runner.stop_proxy(&proxy.name);
        if let Some(mut failed) = fetched? {
            // What the proxy refused explains most failed fetches; the log's times are not
            // kept (they are no function of the commit).
            let refused = crate::egress::refusals(&log);
            if !refused.is_empty() {
                failed.stderr_tail = format!(
                    "{}\negress proxy refused: {}",
                    failed.stderr_tail,
                    refused.join(", ")
                )
                .trim()
                .to_owned();
            }
            return Ok(failed);
        }
        let outcome = match runner.run(&index) {
            Ok(outcome) => outcome,
            Err(e) => return Ok(environment(format!("{e:#}"), started)),
        };
        let index_bytes = if outcome.ok() {
            std::fs::read(index.host_cwd().join("index.scip")).ok()
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
    let _ = std::fs::remove_dir_all(&egress_dir);
    result
}

/// Exit codes the `docker` client reserves for its own failures (the daemon refused the run,
/// the command could not be invoked, the command was not found).
const DOCKER_CLIENT_EXIT_CODES: [i32; 3] = [125, 126, 127];

fn environment(text: String, started: std::time::Instant) -> ScipRun {
    ScipRun {
        status: RunStatus::Environment(redact_host_paths(&text)),
        index: None,
        stderr_tail: text.trim().to_owned(),
        elapsed: started.elapsed(),
    }
}

/// The first non-empty line of a docker client's text with every absolute path replaced by
/// `<host path>`, so the stored reason names no host (contract M3b amendment 49). A path is a
/// `/` that starts a token or follows a `scheme://` (`unix:///Users/…/docker.sock`), a quote,
/// `=` or `(`, up to the token's end; closing punctuation after it is kept.
pub fn redact_host_paths(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default();
    line.split(' ')
        .map(|token| {
            let start = token.match_indices('/').map(|(i, _)| i).find(|&i| {
                i == 0
                    || token[..i].ends_with("://")
                    || matches!(token.as_bytes()[i - 1], b'\'' | b'"' | b'=' | b'(')
            });
            match start {
                Some(i) => {
                    let kept = token.trim_end_matches(['.', ',', ':', ';', ')', '\'', '"']);
                    format!("{}<host path>{}", &token[..i], &token[kept.len().max(i)..])
                }
                None => token.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
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
        RunStatus::Failed(phase)
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
        // The export is repository-shaped and the container starts in the job's root
        // (contract M6a D7), so `file:` links outside the root but inside the repository
        // resolve; scip-typescript's document paths stay relative to the root.
        cwd: job.root.clone(),
        caches: vec![(npm_cache, "/root/.npm".into())],
        readonly: Vec::new(),
        // npm's update notifier checks the registry and prints a notice unless its cache says it
        // checked recently, so a failed fetch's stderr (part of the dump) would depend on the
        // cache's state, which per-repository caches made visible (contract M5 S22).
        env: vec![
            ("NODE_OPTIONS".into(), "--max-old-space-size=4096".into()),
            ("npm_config_update_notifier".into(), "false".into()),
        ],
        network: Network::Fetch,
        cpus: budget.cpus,
        memory: budget.memory.clone(),
        timeout: budget.fetch_timeout,
        entrypoint: None,
        args: Vec::new(),
    };
    // Type indexing needs no lifecycle scripts, and the fetch phase runs the project's own code
    // otherwise (contract M3b amendment 27). No log file: a failed install would otherwise end
    // its stderr with a timestamped log path, which is not a function of the commit
    // (amendment 46).
    let quiet = [
        "--no-audit",
        "--no-fund",
        "--loglevel=error",
        "--ignore-scripts",
        "--logs-max=0",
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
        Fetch::Go | Fetch::Python => {
            anyhow::bail!("a Go/Python fetch on a TypeScript job")
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
/// repository's Maven cache (`go-offline`) and then does one online build, so that the
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
        cwd: String::new(),
        caches: vec![(m2, "/root/.m2".into())],
        readonly: Vec::new(),
        env: vec![(
            "MAVEN_OPTS".into(),
            "-Dorg.slf4j.simpleLogger.logFile=System.err".into(),
        )],
        network: Network::Fetch,
        cpus: budget.cpus,
        memory: budget.memory.clone(),
        timeout: budget.fetch_timeout,
        entrypoint: None,
        args: Vec::new(),
    };
    // The fetch phase reads the egress proxy from the settings `run_job` mounts (Maven ignores
    // the proxy environment variables; contract M5 S21).
    let settings = format!("{CLIENT_CONFIG_AT}/settings.xml");
    // Maven's reactor threads (contract M6a D9), appended to every Maven invocation of both
    // phases only above 1, so the default runs are `cfa8ab9`'s.
    let threads: Vec<String> = if budget.maven_threads > 1 {
        vec!["-T".into(), budget.maven_threads.to_string()]
    } else {
        Vec::new()
    };
    let mvn = |goals: &[&str]| ContainerRun {
        args: ["mvn", "-B", "-q", "-s", settings.as_str()]
            .iter()
            .chain(goals)
            .chain(BUILD_CACHES_OFF.iter())
            .map(|s| (*s).to_owned())
            .chain(threads.iter().cloned())
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
        .iter()
        .chain(BUILD_CACHES_OFF.iter())
        .map(|s| (*s).to_owned())
        .chain(threads.iter().cloned())
        .collect(),
        ..base
    };
    Ok((fetches, index))
}

/// The fetch and index invocations for a scip-go job (docker/scip-go/README.md).
/// The fetch phase downloads the module graph into the repository's Go module
/// cache (`go mod download`); the index phase runs `scip-go` offline
/// (contract M6d S1; proxy settings and lockfile discipline harden in S3/S4).
fn go_runs(
    image: &str,
    work_dir: &Path,
    cache_dir: &Path,
    budget: &Budget,
) -> anyhow::Result<(Vec<ContainerRun>, ContainerRun)> {
    let workdir = crate::runner::absolute(work_dir)?;
    let gomod = crate::runner::absolute(cache_dir)?.join("gomod");
    std::fs::create_dir_all(&gomod)?;
    let base = ContainerRun {
        name: None,
        image: image.into(),
        platform: None,
        workdir,
        mount_at: "/sources".into(),
        cwd: String::new(),
        caches: vec![(gomod, "/root/go/pkg/mod".into())],
        readonly: Vec::new(),
        env: Vec::new(),
        network: Network::Fetch,
        cpus: budget.cpus,
        memory: budget.memory.clone(),
        timeout: budget.fetch_timeout,
        entrypoint: None,
        args: Vec::new(),
    };
    let mut fetch = base.clone();
    fetch.args = ["go", "mod", "download"].map(String::from).to_vec();
    let index = ContainerRun {
        network: Network::None,
        timeout: budget.index_timeout,
        args: ["scip-go"].map(String::from).to_vec(),
        ..base
    };
    Ok((vec![fetch], index))
}

/// The fetch and index invocations for a scip-python job (contract M6d S2).
/// S2 fetches nothing: the environment resolution lands with the lockfile
/// discipline (S4/S6). The index runs `scip-python index` offline; the image
/// exists for linux/amd64 only, so the job is pinned there like
/// scip-typescript's.
fn python_runs(
    job: &Job,
    image: &str,
    work_dir: &Path,
    cache_dir: &Path,
    budget: &Budget,
) -> anyhow::Result<(Vec<ContainerRun>, ContainerRun)> {
    let workdir = crate::runner::absolute(work_dir)?;
    let base = ContainerRun {
        name: None,
        image: image.into(),
        platform: Some("linux/amd64".into()),
        workdir,
        mount_at: "/src".into(),
        // Like scip-typescript's job roots (contract M6a D7): the container
        // starts in the job's root; revisited with evidence in S5.
        cwd: job.root.clone(),
        caches: Vec::new(),
        readonly: Vec::new(),
        env: Vec::new(),
        network: Network::Fetch,
        cpus: budget.cpus,
        memory: budget.memory.clone(),
        timeout: budget.fetch_timeout,
        entrypoint: None,
        args: Vec::new(),
    };
    let index = ContainerRun {
        network: Network::None,
        timeout: budget.index_timeout,
        args: ["scip-python", "index"].map(String::from).to_vec(),
        ..base
    };
    Ok((Vec::new(), index))
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
        /// `start <name> <filter>` and `stop <name>` per proxy call, in order.
        proxies: RefCell<Vec<String>>,
        /// The proxy cannot start (an `Err` with this text).
        proxy_fails: Option<String>,
        /// What a stopped proxy logged.
        proxy_log: String,
        proxy_image: bool,
        /// `<mount> <settings.xml>` per read-only mount a run saw, read while it ran.
        settings: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(fail_fetch: bool, fail_index: bool) -> Fake {
            Fake {
                runs: RefCell::new(Vec::new()),
                fail_fetch,
                fail_index,
                java_image: true,
                fetch_outcome: None,
                proxies: RefCell::new(Vec::new()),
                proxy_fails: None,
                proxy_log: String::new(),
                proxy_image: true,
                settings: RefCell::new(Vec::new()),
            }
        }
    }

    const PROXY_AT: &str = "10.9.8.7";

    impl Runner for Fake {
        fn run(&self, run: &ContainerRun) -> anyhow::Result<Outcome> {
            self.runs.borrow_mut().push(run.clone());
            for (host, inside) in &run.readonly {
                let settings = std::fs::read_to_string(host.join("settings.xml"))?;
                self.settings
                    .borrow_mut()
                    .push(format!("{inside} {settings}"));
            }
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
                std::fs::write(run.host_cwd().join("index.scip"), b"fake-index")?;
            }
            Ok(Outcome {
                exit_code: Some(if fail { 1 } else { 0 }),
                timed_out: false,
                stderr_tail: if fail { "boom".into() } else { String::new() },
                elapsed: Duration::from_millis(1),
            })
        }

        fn image_id(&self, image: &str) -> Option<String> {
            match image {
                SCIP_JAVA_IMAGE => self.java_image.then(|| JAVA_ID.to_owned()),
                PROXY_IMAGE => self.proxy_image.then(|| "sha256:proxy".to_owned()),
                _ => None,
            }
        }

        fn start_proxy(&self, spec: &ProxySpec) -> anyhow::Result<String> {
            let filter = std::fs::read_to_string(spec.config_dir.join("filter"))?;
            self.proxies.borrow_mut().push(format!(
                "start {} {}",
                spec.name,
                filter.lines().collect::<Vec<_>>().join(",")
            ));
            match &self.proxy_fails {
                Some(e) => Err(anyhow::anyhow!("{e}")),
                None => Ok(PROXY_AT.to_owned()),
            }
        }

        fn stop_proxy(&self, name: &str) -> String {
            self.proxies.borrow_mut().push(format!("stop {name}"));
            self.proxy_log.clone()
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

    fn go_job() -> Job {
        Job {
            root: "svc".into(),
            indexer: Indexer::ScipGo,
            fetch: Fetch::Go,
            projects: Vec::new(),
            unpinned: false,
        }
    }

    fn python_job() -> Job {
        Job {
            root: "svc".into(),
            indexer: Indexer::ScipPython,
            fetch: Fetch::Python,
            projects: Vec::new(),
            unpinned: true,
        }
    }

    /// Contract M6d S1: a Go job fetches with `go mod download` on the fetch
    /// network (module cache mounted) and indexes with `scip-go` offline.
    #[test]
    fn go_job_runs_download_then_scip_go() {
        let lines = argv_lines(&go_job(), "sha256:go", &Budget::default());
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].contains("--network rb-scip-fetch")
                && lines[0].contains("-v <work>-egress/client:/rb-egress:ro")
                && lines[0].contains("go mod download")
                && lines[0].contains("/root/go/pkg/mod"),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].contains("--network none")
                && lines[1].ends_with("scip-go"),
            "{}",
            lines[1]
        );
    }

    /// Contract M6d S2: a Python job runs no fetch yet (S4) and indexes with
    /// `scip-python index` offline, emulated on arm64.
    #[test]
    fn python_job_indexes_offline_with_no_fetch() {
        let lines = argv_lines(&python_job(), "sha256:py", &Budget::default());
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("--network none")
                && lines[0].contains("--platform linux/amd64")
                && lines[0].contains("-v <work>:/src -w /src")
                && lines[0].ends_with("scip-python index"),
            "{}",
            lines[0]
        );
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
            SCIP_TYPESCRIPT_IMAGE,
            "a pinned image is identified by its digest reference, without a lookup"
        );
        assert_eq!(images.identity(Indexer::ScipJava), JAVA_ID);
        let run = run_job(
            &job,
            &image,
            &files(),
            &work,
            &cache,
            &Budget::default(),
            &Egress::default(),
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
        assert_eq!(runs[0].network, Network::Fetch);
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

    /// Contract M3b amendment 56: a `failed` job names the phase that failed, and the phase
    /// round-trips through the cache row; a `failed` row without a phase (an older binary's)
    /// is a miss, like any status this binary does not write.
    #[test]
    fn a_failed_job_records_its_phase() {
        for phase in [Phase::Fetch, Phase::Index] {
            let status = RunStatus::Failed(phase);
            assert_eq!(status.as_str(), "failed");
            assert_eq!(status.phase(), Some(phase));
            assert!(status.is_cacheable());
            assert_eq!(
                RunStatus::from_cached("failed", Some(phase.as_str())),
                Some(status)
            );
        }
        assert_eq!(RunStatus::from_cached("failed", None), None);
        assert_eq!(RunStatus::from_cached("failed", Some("build")), None);
        assert_eq!(RunStatus::from_cached("ok", None), Some(RunStatus::Ok));
        assert_eq!(RunStatus::from_cached("timeout", Some("fetch")), None);
        assert_eq!(RunStatus::Ok.phase(), None);
        assert_eq!(
            RunStatus::Timeout {
                phase: Phase::Index,
                budget_secs: 1
            }
            .phase(),
            Some(Phase::Index)
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
            &Egress::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(run.status, RunStatus::Failed(Phase::Fetch));
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
            &Egress::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(run.status, RunStatus::Failed(Phase::Index));
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
            maven_threads: 1,
        };
        let run = run_job(
            &job,
            &image,
            &files(),
            &work,
            &cache,
            &budget,
            &Egress::default(),
            &fake,
        )
        .unwrap();
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
        // Contract M5 S21, S22: the fetch phase reads the egress proxy from the mounted
        // settings, and both phases turn the Develocity/Gradle Enterprise build caches off.
        let caches_off = [
            "-Ddevelocity.cache.local.enabled=false",
            "-Ddevelocity.cache.remote.enabled=false",
            "-Dgradle.cache.local.enabled=false",
            "-Dgradle.cache.remote.enabled=false",
        ];
        let with_off = |args: &[&str]| -> Vec<String> {
            args.iter()
                .chain(caches_off.iter())
                .map(|s| (*s).to_owned())
                .collect()
        };
        assert_eq!(runs[0].network, Network::Fetch);
        assert_eq!(
            runs[0].args,
            with_off(&[
                "mvn",
                "-B",
                "-q",
                "-s",
                "/rb-egress/settings.xml",
                "org.apache.maven.plugins:maven-dependency-plugin:3.11.0:go-offline"
            ])
        );
        assert_eq!(runs[0].timeout, Duration::from_secs(10));
        assert_eq!(runs[1].network, Network::Fetch);
        assert_eq!(
            runs[1].args,
            with_off(&[
                "mvn",
                "-B",
                "-q",
                "-s",
                "/rb-egress/settings.xml",
                "clean",
                "verify",
                "-DskipTests"
            ])
        );
        assert_eq!(runs[2].network, Network::None);
        assert_eq!(runs[2].timeout, Duration::from_secs(20));
        assert_eq!(
            runs[2].args,
            with_off(&[
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
            ])
        );
        assert!(
            cache.join("m2").is_dir(),
            "the Maven cache directory is created"
        );
    }

    /// Contract M5 S21: each run's fetch phase runs behind its own egress proxy, started
    /// before the first fetch with the allow-list as its filter and stopped before the index
    /// phase; the fetch containers get the proxy's address (environment, Maven settings mounted
    /// read-only), the index phase none of it; the run's egress files are removed.
    #[test]
    fn the_fetch_phase_runs_behind_its_own_egress_proxy() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let cache = tmp.path().join("cache");
        let fake = Fake::new(false, false);
        let egress = Egress::with(&["repo.spring.io".to_owned()]).unwrap();
        let run = run_job(
            &java_job(Fetch::Maven),
            JAVA_ID,
            &files(),
            &work,
            &cache,
            &Budget::default(),
            &egress,
            &fake,
        )
        .unwrap();
        assert_eq!(run.status, RunStatus::Ok);
        let runs = fake.runs.borrow();
        let instance = runs[0]
            .workdir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            *fake.proxies.borrow(),
            [
                format!(
                    "start rb-egress-{instance} registry.npmjs.org:443,registry.yarnpkg.com:443,repo.maven.apache.org:443,repo.spring.io:443,repo1.maven.org:443"
                ),
                format!("stop rb-egress-{instance}"),
            ]
        );
        let url = format!("http://{PROXY_AT}:8888");
        for fetch in &runs[..2] {
            assert_eq!(fetch.network, Network::Fetch);
            assert!(
                fetch.env.contains(&("HTTPS_PROXY".to_owned(), url.clone())),
                "{:?}",
                fetch.env
            );
            assert_eq!(fetch.readonly.len(), 1);
            assert_eq!(fetch.readonly[0].1, "/rb-egress");
        }
        let settings = fake.settings.borrow();
        assert_eq!(settings.len(), 2);
        assert!(
            settings[0].starts_with("/rb-egress <settings>")
                && settings[0].contains(&format!("<host>{PROXY_AT}</host>")),
            "{}",
            settings[0]
        );
        let index = &runs[2];
        assert_eq!(index.network, Network::None);
        assert!(index.readonly.is_empty());
        assert!(
            !index
                .env
                .iter()
                .any(|(k, _)| k.contains("PROXY") || k.contains("proxy"))
        );
        assert!(!tmp.path().join(format!("{instance}-egress")).exists());
    }

    /// Contract M5 S21: a failed fetch stops the proxy too and names what it refused (without
    /// the log's times); a proxy that cannot start is an `environment` outcome (not cached)
    /// and no container runs; without the proxy image every job is `unsupported`.
    #[test]
    fn egress_failures_are_reported_and_never_run_unguarded() {
        let tmp = tempfile::tempdir().unwrap();
        let (work, cache) = (tmp.path().join("work"), tmp.path().join("cache"));
        let mut fake = Fake::new(true, false);
        fake.proxy_log = "NOTICE    Oct 06 23:33:11.069 [1]: Proxying refused on filtered url \"repo.spring.io:443\"\nNOTICE    Oct 06 23:33:12.069 [1]: Proxying refused on filtered url \"repo.spring.io:443\"\n".into();
        let run = |fake: &Fake| {
            run_job(
                &java_job(Fetch::Maven),
                JAVA_ID,
                &files(),
                &work,
                &cache,
                &Budget::default(),
                &Egress::default(),
                fake,
            )
            .unwrap()
        };
        let failed = run(&fake);
        assert_eq!(failed.status, RunStatus::Failed(Phase::Fetch));
        assert!(
            failed
                .stderr_tail
                .ends_with("boom\negress proxy refused: repo.spring.io:443"),
            "{}",
            failed.stderr_tail
        );
        assert_eq!(fake.proxies.borrow().len(), 2, "started and stopped");
        assert_eq!(fake.runs.borrow().len(), 1, "no index after a failed fetch");

        let mut fake = Fake::new(false, false);
        fake.proxy_fails = Some("network rb-scip-fetch exists with internal=false".into());
        let down = run(&fake);
        assert!(
            matches!(&down.status, RunStatus::Environment(r) if r.starts_with("egress proxy:")),
            "{:?}",
            down.status
        );
        assert!(!down.status.is_cacheable());
        assert!(fake.runs.borrow().is_empty(), "nothing ran unguarded");

        let mut fake = Fake::new(false, false);
        fake.proxy_image = false;
        let images = Images::resolve(&fake);
        for job in [java_job(Fetch::Maven), ts_job(NodeLock::PackageLock)] {
            let reason = images.for_job(&job).unwrap_err();
            assert!(reason.contains("rb-egress-proxy:1.11.3"), "{reason}");
        }
        assert_eq!(images.identity(Indexer::ScipJava), JAVA_ID);
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
                &Egress::default(),
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
            &Egress::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(run.status, RunStatus::Failed(Phase::Fetch));
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
            &Egress::default(),
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

    /// Contract M3b amendments 27 and 46: the fetch phase installs packages without their
    /// lifecycle scripts, for every lockfile kind, and npm writes no log file (its path is
    /// timestamped and would end a failed job's stderr tail).
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
                &Egress::default(),
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
            assert_eq!(
                fetch.args.iter().any(|a| a == "--logs-max=0"),
                entrypoint == "/usr/local/bin/actual-npm",
                "{lockfile:?}: {:?}",
                fetch.args
            );
            // Contract M5 S22: no update notice depends on the cache's state.
            assert!(
                fetch
                    .env
                    .contains(&("npm_config_update_notifier".into(), "false".into())),
                "{lockfile:?}: {:?}",
                fetch.env
            );
        }
    }

    /// Contract M6a D6, D8: a grouped workspace job fetches at its root with today's
    /// commands (`npm ci` for `package-lock.json`, `yarn install --frozen-lockfile` for
    /// `yarn.lock`, scripts off) and indexes every member's `tsconfig.json` directory after
    /// `--`, in the job's (sorted) order; the recipes are per indexer.
    #[test]
    fn a_workspace_job_fetches_at_its_root_and_indexes_every_member() {
        assert_eq!((RECIPE_JAVA, RECIPE_TS), ("m5-egress-1", "m6a-ts-1"));
        assert_eq!(recipe(Indexer::ScipJava, &Budget::default()), RECIPE_JAVA);
        assert_eq!(
            recipe(Indexer::ScipTypescript, &Budget::default()),
            RECIPE_TS
        );
        let tmp = tempfile::tempdir().unwrap();
        for (lockfile, entrypoint, fetch_args) in [
            (
                NodeLock::PackageLock,
                "/usr/local/bin/actual-npm",
                vec![
                    "ci",
                    "--no-audit",
                    "--no-fund",
                    "--loglevel=error",
                    "--ignore-scripts",
                    "--logs-max=0",
                ],
            ),
            (
                NodeLock::Yarn,
                "yarn",
                vec![
                    "install",
                    "--frozen-lockfile",
                    "--non-interactive",
                    "--ignore-scripts",
                ],
            ),
        ] {
            let job = Job {
                root: String::new(),
                indexer: Indexer::ScipTypescript,
                fetch: Fetch::Npm { lockfile },
                projects: vec!["packages/app".into(), "packages/core".into()],
                unpinned: false,
            };
            let fake = Fake::new(false, false);
            run_job(
                &job,
                SCIP_TYPESCRIPT_IMAGE,
                &files(),
                &tmp.path().join("w"),
                &tmp.path().join("c"),
                &Budget::default(),
                &Egress::default(),
                &fake,
            )
            .unwrap();
            let runs = fake.runs.borrow();
            assert_eq!(runs.len(), 2, "one fetch at the root, one index");
            assert_eq!(runs[0].entrypoint.as_deref(), Some(entrypoint));
            assert_eq!(runs[0].args, fetch_args, "{lockfile:?}");
            assert_eq!(
                runs[1].args,
                [
                    "index",
                    "--output",
                    "index.scip",
                    "--no-progress-bar",
                    "--",
                    "packages/app",
                    "packages/core"
                ]
            );
        }
    }

    /// The full `docker run` argument line of every phase of `job` at `budget`, with this run's
    /// host paths and container names replaced by placeholders.
    fn argv_lines(job: &Job, image: &str, budget: &Budget) -> Vec<String> {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(false, false);
        run_job(
            job,
            image,
            &files(),
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            budget,
            &Egress::default(),
            &fake,
        )
        .unwrap();
        let tmp = tmp.path().canonicalize().unwrap().display().to_string();
        fake.runs
            .borrow()
            .iter()
            .map(|r| {
                let mut a = crate::runner::docker_args(r);
                if let Some(i) = a.iter().position(|x| x == "--name") {
                    a[i + 1] = "<name>".into();
                }
                a.join(" ")
                    .replace(&r.workdir.display().to_string(), "<work>")
                    .replace(&tmp, "<tmp>")
            })
            .collect()
    }

    /// Contract M6a D9, S12: at the default budget every phase's full argument line is the one
    /// `cfa8ab9`'s code built (recorded from it: the Java runs and a TypeScript root job, whose
    /// construction differs since only in `cwd`, empty for both), so default runs and their
    /// cache keys are unchanged.
    #[test]
    fn default_budget_argv_is_cfa8ab9s() {
        let proxy = [
            "HTTPS_PROXY",
            "HTTP_PROXY",
            "https_proxy",
            "http_proxy",
            "npm_config_https_proxy",
            "npm_config_proxy",
            "YARN_HTTPS_PROXY",
            "YARN_HTTP_PROXY",
        ]
        .map(|k| format!("-e {k}=http://10.9.8.7:8888"))
        .join(" ");
        let off = BUILD_CACHES_OFF.join(" ");
        let expected: Vec<String> = [
            "run --rm --name <name> --network rb-scip-fetch --dns 127.0.0.1 --cpus 2 --memory 4g -v <work>:/sources -w /sources -v <tmp>/c/m2:/root/.m2 -v <work>-egress/client:/rb-egress:ro -e MAVEN_OPTS=-Dorg.slf4j.simpleLogger.logFile=System.err {PROXY} sha256:66a0b395166fc65c49cc099b0b3e4d2cb61e16c177dbc3c022160bb0b4cd1cf2 mvn -B -q -s /rb-egress/settings.xml org.apache.maven.plugins:maven-dependency-plugin:3.11.0:go-offline {OFF}",
            "run --rm --name <name> --network rb-scip-fetch --dns 127.0.0.1 --cpus 2 --memory 4g -v <work>:/sources -w /sources -v <tmp>/c/m2:/root/.m2 -v <work>-egress/client:/rb-egress:ro -e MAVEN_OPTS=-Dorg.slf4j.simpleLogger.logFile=System.err {PROXY} sha256:66a0b395166fc65c49cc099b0b3e4d2cb61e16c177dbc3c022160bb0b4cd1cf2 mvn -B -q -s /rb-egress/settings.xml clean verify -DskipTests {OFF}",
            "run --rm --name <name> --network none --cpus 2 --memory 4g -v <work>:/sources -w /sources -v <tmp>/c/m2:/root/.m2 -e MAVEN_OPTS=-Dorg.slf4j.simpleLogger.logFile=System.err sha256:66a0b395166fc65c49cc099b0b3e4d2cb61e16c177dbc3c022160bb0b4cd1cf2 scip-java index --build-tool Maven -- --batch-mode --offline clean verify -DskipTests {OFF}",
            "run --rm --name <name> --platform linux/amd64 --network rb-scip-fetch --dns 127.0.0.1 --cpus 2 --memory 4g -v <work>:/src -w /src -v <tmp>/c/npm:/root/.npm -v <work>-egress/client:/rb-egress:ro -e NODE_OPTIONS=--max-old-space-size=4096 -e npm_config_update_notifier=false {PROXY} --entrypoint /usr/local/bin/actual-npm sourcegraph/scip-typescript@sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845 ci --no-audit --no-fund --loglevel=error --ignore-scripts --logs-max=0",
            "run --rm --name <name> --platform linux/amd64 --network none --cpus 2 --memory 4g -v <work>:/src -w /src -v <tmp>/c/npm:/root/.npm -e NODE_OPTIONS=--max-old-space-size=4096 -e npm_config_update_notifier=false sourcegraph/scip-typescript@sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845 index --output index.scip --no-progress-bar -- . sub",
        ]
        .iter()
        .map(|l| l.replace("{PROXY}", &proxy).replace("{OFF}", &off))
        .collect();
        let mut actual = argv_lines(&java_job(Fetch::Maven), JAVA_ID, &Budget::default());
        let ts_root = Job {
            root: String::new(),
            ..ts_job(NodeLock::PackageLock)
        };
        actual.extend(argv_lines(
            &ts_root,
            SCIP_TYPESCRIPT_IMAGE,
            &Budget::default(),
        ));
        assert_eq!(actual, expected);
    }

    /// Contract M6a D9, S12: `--scip-maven-threads n` > 1 appends `-T n` to every Maven
    /// invocation of both phases and gives the Java recipe `+T<n>` (another cache key); 1 does
    /// neither; the CPU and memory budget reach `docker run`; scip-typescript is untouched.
    #[test]
    fn maven_threads_change_the_java_runs_and_their_recipe_only() {
        let threaded = Budget {
            cpus: 8.0,
            memory: "6g".into(),
            maven_threads: 4,
            ..Budget::default()
        };
        let lines = argv_lines(&java_job(Fetch::Maven), JAVA_ID, &threaded);
        assert_eq!(lines.len(), 3);
        for line in &lines {
            assert!(line.ends_with(" -T 4"), "{line}");
            assert!(line.contains(" --cpus 8 --memory 6g "), "{line}");
        }
        assert_eq!(recipe(Indexer::ScipJava, &threaded), "m5-egress-1+T4");
        assert_eq!(recipe(Indexer::ScipJava, &Budget::default()), RECIPE_JAVA);
        assert_eq!(recipe(Indexer::ScipTypescript, &threaded), RECIPE_TS);
        let files = [("pom.xml", "1")];
        assert_ne!(
            crate::project_input_hash(
                &recipe(Indexer::ScipJava, &threaded),
                files,
                &Egress::default()
            ),
            crate::project_input_hash(
                &recipe(Indexer::ScipJava, &Budget::default()),
                files,
                &Egress::default()
            ),
            "the threaded Java key differs"
        );
        let one = Budget {
            maven_threads: 1,
            ..threaded.clone()
        };
        assert!(
            argv_lines(&java_job(Fetch::Maven), JAVA_ID, &one)
                .iter()
                .all(|l| !l.contains(" -T "))
        );
        let ts = argv_lines(
            &ts_job(NodeLock::PackageLock),
            SCIP_TYPESCRIPT_IMAGE,
            &threaded,
        );
        assert!(ts.iter().all(|l| !l.contains(" -T ")), "{ts:?}");
    }

    /// Contract M6a D7: a scip-typescript job's export is repository-shaped (a link target
    /// outside the root keeps its repository path) and both phases start in the job's root,
    /// where scip-typescript writes `index.scip`; a scip-java job's export is its root.
    #[test]
    fn typescript_exports_are_repository_shaped_and_start_in_the_root() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("export");
        export(
            &[
                ("apps/web/package.json".into(), b"{}".to_vec()),
                ("libs/core/package.json".into(), b"{}".to_vec()),
            ],
            &dir,
        )
        .unwrap();
        assert!(dir.join("apps/web/package.json").is_file());
        assert!(dir.join("libs/core/package.json").is_file());
        let fake = Fake::new(false, false);
        let run = run_job(
            &ts_job(NodeLock::PackageLock),
            SCIP_TYPESCRIPT_IMAGE,
            &[
                ("apps/web/package.json".into(), b"{}".to_vec()),
                ("apps/web/src/a.ts".into(), b"export const a = 1;".to_vec()),
                ("libs/core/package.json".into(), b"{}".to_vec()),
            ],
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            &Budget::default(),
            &Egress::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(run.status, RunStatus::Ok, "the index is read in the root");
        for r in fake.runs.borrow().iter() {
            assert_eq!((r.mount_at.as_str(), r.cwd.as_str()), ("/src", "apps/web"));
            assert!(
                crate::runner::docker_args(r)
                    .join(" ")
                    .contains(" -w /src/apps/web "),
                "{:?}",
                crate::runner::docker_args(r)
            );
        }
        let fake = Fake::new(false, false);
        run_job(
            &java_job(Fetch::Maven),
            JAVA_ID,
            &[("pom.xml".into(), b"<project/>".to_vec())],
            &tmp.path().join("j"),
            &tmp.path().join("c"),
            &Budget::default(),
            &Egress::default(),
            &fake,
        )
        .unwrap();
        assert!(fake.runs.borrow().iter().all(|r| r.cwd.is_empty()));
    }

    /// Contract M3b amendment 49: an `environment` reason is the first line of the docker
    /// client's text with absolute paths redacted, so the dump names no host; the run's
    /// `stderr_tail` keeps the whole text for the terminal.
    #[test]
    fn environment_reasons_name_no_host_path() {
        let tmp = tempfile::tempdir().unwrap();
        let text = "Cannot connect to the Docker daemon at unix:///Users/rohit/.colima/default/docker.sock. Is the docker daemon running?\nSee '/usr/local/bin/docker --help'.";
        let fake = Fake {
            fetch_outcome: Some(Ok(Outcome {
                exit_code: Some(125),
                timed_out: false,
                stderr_tail: text.into(),
                elapsed: Duration::from_millis(1),
            })),
            ..Fake::new(false, false)
        };
        let run = run_job(
            &ts_job(NodeLock::PackageLock),
            SCIP_TYPESCRIPT_IMAGE,
            &files(),
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            &Budget::default(),
            &Egress::default(),
            &fake,
        )
        .unwrap();
        let reason = run.status.reason().unwrap();
        assert_eq!(
            reason,
            "Cannot connect to the Docker daemon at unix://<host path>. Is the docker daemon running?"
        );
        assert!(!reason.contains("/Users") && !reason.contains("unix:///"));
        assert_eq!(run.stderr_tail, text, "the terminal gets the whole text");
        assert_eq!(
            redact_host_paths(
                "\n docker: Error response from daemon: No such image: sha256:0000\n\nRun 'docker run --help' for more information"
            ),
            "docker: Error response from daemon: No such image: sha256:0000"
        );
        assert_eq!(
            redact_host_paths(
                "starting docker: /Users/x/bin/docker: No such file or directory (os error 2)"
            ),
            "starting docker: <host path>: No such file or directory (os error 2)"
        );
        assert_eq!(
            redact_host_paths(
                "See '/usr/local/bin/docker --help' (DOCKER_HOST=unix:///var/run/docker.sock)."
            ),
            "See '<host path> --help' (DOCKER_HOST=unix://<host path>)."
        );
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
                &Egress::default(),
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
            RunStatus::Failed(Phase::Fetch)
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
