//! Running one job (design §5.2): export the build root's files into a work directory that the
//! container VM can see, fetch dependencies with the network on, index with it off, and collect
//! `index.scip`. Nothing here talks to the database; the caller owns the cache.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::egress::{CLIENT_CONFIG_AT, Egress, PROXY_IMAGE};
use crate::jobs::{
    Fetch, Indexer, Jdk, Job, NodeLock, PythonLock, SCIP_GO_IMAGE, SCIP_JAVA_IMAGE,
    SCIP_JAVA25_IMAGE, SCIP_PYTHON_IMAGE, SCIP_TYPESCRIPT_IMAGE,
};
use crate::runner::{ContainerRun, Network, Outcome, ProxySpec, Runner};

/// scip-java's fetch and index recipe, part of each of its jobs' cache keys (contract M3b
/// amendment 47; one per indexer since contract M6a D8). Bump it whenever scip-java's container
/// commands, its image's invocation or its export rules change: an index produced by another
/// recipe is then a cache miss, never served as "cached". `m5-egress-1`: the fetch phase behind
/// the egress proxy, per-repository dependency caches and the Develocity/Gradle Enterprise
/// build caches off in both Maven phases (contract M5 S21, S22). Unchanged by M6a, so cached
/// Java indexes stay valid (a unit pins the key of a fixed input). `m9d-java-1`: the index
/// phase writes scip-java's targetroot outside the sources (contract M9d D12b), so every
/// Maven job re-runs once.
pub const RECIPE_JAVA: &str = "m9d-java-1";

/// scip-typescript's recipe (contract M6a D8), bumped like [`RECIPE_JAVA`]. `m6a-ts-1`:
/// workspace roots fetch and index their members (D5, D6) and exports are repository-shaped
/// (D7). `m9d-ts-1`: npm fetches rewrite lockfile registry hosts and ignore `engine-strict`
/// (contract M9d D1). `m9d-ts-2`: the rewrite names the lockfile's one foreign registry host
/// instead of `always` (D1 narrowed by amendment 4).
pub const RECIPE_TS: &str = "m9d-ts-2";

/// scip-go's recipe (contract M6d S1/S3): `go mod download`, then `scip-go`
/// offline. Bumped like [`RECIPE_JAVA`].
pub const RECIPE_GO: &str = "m6d-go-1";

/// scip-go's recipe for a job with a workspace or a replace target outside its root
/// (contract M9d D5, D6): a `go.work` written by rb with `GOFLAGS` empty and a `go list -deps
/// -test` fetch, or a repository-shaped export starting in the root. Jobs with neither keep
/// [`RECIPE_GO`], their layout and their cache rows. `m9d-go-2`: the `go.work` also replaces
/// each workspace module at the version the root requires (amendment 4); its text is no input
/// file, so the recipe carries the change.
pub const RECIPE_GO_WORK: &str = "m9d-go-2";

/// scip-python's recipe (contract M6d S2/S4): environment resolution, then
/// `scip-python index` offline. Bumped like [`RECIPE_JAVA`]. `m6d-py-2`:
/// the fetch installs lockfile-driven environments (S4; `m6d-py-1`
/// fetched nothing). `m6d-py-3`: pip ignores `requires-python` (S11 — the
/// image's 3.10 predates some lockfiles' floors; versions stay pinned).
/// `m6d-py-4`: pip reads with a 60 s socket timeout (phase 12 — the 15 s
/// default trips on slow wheel reads through the emulated stack). `m9d-py-1`:
/// a root without a `pyproject.toml` names its project, Poetry's stdout reaches
/// the tail, and a failed fetch is followed by an index without the
/// environment (contract M9d D2–D4).
pub const RECIPE_PY: &str = "m9d-py-1";

/// scip-java's Gradle recipe (contract M6e S2): dependency resolution plus one
/// online build in the fetch phase, then `scip-java index --build-tool Gradle`
/// offline. Bumped like [`RECIPE_JAVA`]. Separate from it because the fetch
/// commands differ: a Maven-fetched index must never serve a Gradle job. `m9d-gradle-1`:
/// both phases run rb's init script, which marks the protobuf plugin's tool artifacts
/// executable (contract M9d D13).
pub const RECIPE_GRADLE: &str = "m9d-gradle-1";

/// The recipe a job's cache key carries: scip-java's gains `+T<n>` when its Maven builds run
/// with `n` > 1 threads (contract M6a D9), so no cached index crosses thread settings; the
/// other budget settings change no index and are not part of it. Gradle jobs carry
/// [`RECIPE_GRADLE`] (threads are a Maven-reactor setting; Gradle commands take none).
pub fn recipe(job: &Job, budget: &Budget) -> String {
    match (&job.indexer, &job.fetch) {
        (Indexer::ScipJava, Fetch::Gradle) => RECIPE_GRADLE.to_owned(),
        (Indexer::ScipJava, _) if budget.maven_threads > 1 => {
            format!("{RECIPE_JAVA}+T{}", budget.maven_threads)
        }
        (Indexer::ScipJava, _) => RECIPE_JAVA.to_owned(),
        (Indexer::ScipGo, _) if !job.workspace.is_empty() || !job.replace_targets.is_empty() => {
            RECIPE_GO_WORK.to_owned()
        }
        (Indexer::ScipGo, _) => RECIPE_GO.to_owned(),
        (Indexer::ScipPython, _) => RECIPE_PY.to_owned(),
        (Indexer::ScipTypescript, _) => RECIPE_TS.to_owned(),
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
    /// A scip-python job whose fetch failed and whose index phase, run anyway without the
    /// environment, wrote an index (contract M9d D4): the project's own code is indexed and
    /// third-party names stay unresolved. Stored as `ok` with phase `fetch`; `stderr_tail` is
    /// the fetch's.
    OkWithoutEnvironment,
    /// A scip-go workspace job (contract M9d D5) whose workspace run failed (a phase exited
    /// non-zero, or no index was written) and whose re-run as a plain job wrote an index
    /// (contract M9d D5b, amendment 2): the index is the plain one, as without the workspace.
    /// Stored as `ok` with phase `index` (the workspace's index is the part missing);
    /// `stderr_tail` is the workspace run's.
    OkWithoutWorkspace,
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
            RunStatus::Ok | RunStatus::OkWithoutEnvironment | RunStatus::OkWithoutWorkspace => "ok",
            RunStatus::Failed(_) => "failed",
            RunStatus::Timeout { .. } => "timeout",
            RunStatus::Unsupported(_) => "unsupported",
            RunStatus::Skipped(_) => "skipped",
            RunStatus::Environment(_) => "environment",
        }
    }

    /// Whether the job wrote an index: `ok`, with its environment or without it (D4), or
    /// without its workspace (D5b).
    pub fn is_ok(&self) -> bool {
        matches!(
            self,
            RunStatus::Ok | RunStatus::OkWithoutEnvironment | RunStatus::OkWithoutWorkspace
        )
    }

    /// Whether this outcome is written to the index cache: only a finished run of the project's
    /// own build (`ok` or `failed`). A timeout depends on the budget, and an unsupported or
    /// environment outcome on the host, so they are decided again on the next run.
    pub fn is_cacheable(&self) -> bool {
        self.is_ok() || matches!(self, RunStatus::Failed(_))
    }

    /// The phase a `failed` or `timeout` job stopped in; the `fetch` phase that failed before
    /// an index without the environment (D4); the `index` phase for an index without the
    /// workspace (D5b).
    pub fn phase(&self) -> Option<Phase> {
        match self {
            RunStatus::Failed(phase) | RunStatus::Timeout { phase, .. } => Some(*phase),
            RunStatus::OkWithoutEnvironment => Some(Phase::Fetch),
            RunStatus::OkWithoutWorkspace => Some(Phase::Index),
            _ => None,
        }
    }

    /// The status a cache row recorded: `ok` (in the `fetch` phase: an index without the
    /// environment, D4; in the `index` phase: an index without the workspace, D5b), or
    /// `failed` with its phase. `None` for any other value (a `timeout`, a `failed` without a
    /// phase written by an older binary), which the caller treats as a miss.
    pub fn from_cached(status: &str, phase: Option<&str>) -> Option<RunStatus> {
        match (status, phase) {
            ("ok", None) => Some(RunStatus::Ok),
            ("ok", Some("fetch")) => Some(RunStatus::OkWithoutEnvironment),
            ("ok", Some("index")) => Some(RunStatus::OkWithoutWorkspace),
            ("failed", phase) => phase.and_then(Phase::parse).map(RunStatus::Failed),
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
            RunStatus::Ok
            | RunStatus::OkWithoutEnvironment
            | RunStatus::OkWithoutWorkspace
            | RunStatus::Failed(_) => None,
        }
    }

    /// What an `ok` job's status line appends when its index is degraded: made without the
    /// environment after a failed fetch (contract M9d D4), or without the workspace after a
    /// failed workspace run (D5b).
    pub fn note(&self) -> Option<&'static str> {
        match self {
            RunStatus::OkWithoutEnvironment => Some("environment not installed: fetch failed"),
            RunStatus::OkWithoutWorkspace => Some("workspace failed: indexed without go.work"),
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

/// The image reference a job uses: scip-typescript's registry digest, scip-java's local tag
/// for the JDK its release needs (contract M9d D12; the JDK 25 one for a release above 25,
/// which [`Images::for_job`] refuses).
pub fn image_for(job: &Job) -> &'static str {
    match job.indexer {
        Indexer::ScipTypescript => SCIP_TYPESCRIPT_IMAGE,
        Indexer::ScipJava => crate::jobs::jdk(job).map_or(SCIP_JAVA25_IMAGE, Jdk::image),
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
    /// scip-java on JDK 25 (contract M9d D12): like `java`, for the Maven jobs whose release
    /// is above 21.
    pub java25: Result<String, String>,
    /// scip-go: like scip-java, a locally built image identified by ID.
    pub go: Result<String, String>,
    /// The egress proxy image (contract M5 S21): its ID, or the reason every job is
    /// `unsupported` (no fetch runs without the proxy; nothing builds it implicitly).
    pub proxy: Result<String, String>,
}

impl Images {
    /// The indexer images on this host, looked up once per `rb index` (contract M3b
    /// amendment 29). `jdk25` tells whether any planned job needs JDK 25 (contract
    /// D12-lazy, from the plan): without one the java25 image is not inspected and
    /// records as not looked up, saving the fifth `docker image inspect` per run.
    pub fn resolve(runner: &dyn Runner, jdk25: bool) -> Images {
        Images {
            java: runner.image_id(SCIP_JAVA_IMAGE).ok_or_else(|| {
                format!(
                    "scip-java: image {SCIP_JAVA_IMAGE} is not available locally; build it as \
                     docker/scip-java/README.md describes"
                )
            }),
            java25: if jdk25 {
                runner.image_id(SCIP_JAVA25_IMAGE).ok_or_else(|| {
                    format!(
                        "scip-java: image {SCIP_JAVA25_IMAGE} is not available locally; build it as \
                         docker/scip-java/README.md describes"
                    )
                })
            } else {
                Err(format!(
                    "scip-java: image {SCIP_JAVA25_IMAGE} was not looked up"
                ))
            },
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
            java25: Err(format!(
                "scip-java: image {SCIP_JAVA25_IMAGE} was not looked up"
            )),
            go: Err(format!("scip-go: image {SCIP_GO_IMAGE} was not looked up")),
            proxy: Err(format!(
                "egress proxy: image {PROXY_IMAGE} was not looked up"
            )),
        }
    }

    /// The image `job` runs in and its index is cached under: the digest reference itself for
    /// a pinned image, the image ID for the locally built scip-java one (on the JDK the job's
    /// release needs, contract M9d D12). `Err` is the reason the job is `unsupported`: the
    /// indexer image, or the egress proxy image, is absent, or the release is above 25.
    pub fn for_job(&self, job: &Job) -> Result<String, String> {
        let image = match job.indexer {
            Indexer::ScipTypescript => Ok(SCIP_TYPESCRIPT_IMAGE.to_owned()),
            Indexer::ScipPython => Ok(SCIP_PYTHON_IMAGE.to_owned()),
            Indexer::ScipJava => match crate::jobs::jdk(job) {
                Ok(Jdk::Jdk21) => self.java.clone(),
                Ok(Jdk::Jdk25) => self.java25.clone(),
                Err(release) => Err(format!(
                    "scip-java: the poms declare Java release {release}; the newest scip-java \
                     image runs JDK 25"
                )),
            },
            Indexer::ScipGo => self.go.clone(),
        }?;
        self.proxy.as_ref().map_err(Clone::clone)?;
        Ok(image)
    }

    /// The JDK 25 scip-java image's identity (contract M9d D12), recorded beside
    /// [`Images::identity`]'s: its image ID, or `absent`.
    pub fn java25_identity(&self) -> &str {
        self.java25.as_deref().unwrap_or("absent")
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
///
/// A scip-go workspace job (contract M9d D5) whose workspace run failed, a phase exiting
/// non-zero (137: an OOM kill) or no index written, runs again exactly as a plain job: a fresh
/// export without `go.work`, `GOFLAGS=-mod=mod`, `go mod download` (D5b, amendment 2). An
/// index from that re-run is [`RunStatus::OkWithoutWorkspace`] with the workspace run's tail;
/// a re-run that fails gives its own outcome. A timeout or a docker client failure in the
/// workspace run is returned as such (as D4), and a job with a replace target outside its root
/// (D6) is not re-run: a plain run cannot resolve the replace.
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
    let run = |job: &Job| {
        run_once(
            job, image, files, work_dir, cache_dir, budget, egress, runner,
        )
    };
    let attempt = run(job)?;
    let workspace = !job.workspace.is_empty() && job.replace_targets.is_empty();
    if !(workspace && matches!(attempt.status, RunStatus::Failed(_))) {
        return Ok(attempt);
    }
    let rerun = run(&Job {
        workspace: Vec::new(),
        ..job.clone()
    })?;
    let elapsed = started.elapsed();
    Ok(match rerun.status {
        // The workspace run's tail says why the workspace failed.
        RunStatus::Ok => ScipRun {
            status: RunStatus::OkWithoutWorkspace,
            stderr_tail: attempt.stderr_tail,
            elapsed,
            ..rerun
        },
        _ => ScipRun { elapsed, ..rerun },
    })
}

/// One run of `job` as [`run_job`] describes it, without D5b's re-run.
#[allow(clippy::too_many_arguments)]
fn run_once(
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
    if let Fetch::Npm { lockfile } = &job.fetch {
        // A Yarn Berry root never fetches: the image holds classic yarn 1.22.22
        // (measured in preflight), so a Berry install would fail on its lockfile;
        // versions still pin from it (contract M6e S5). Before anything is exported.
        if *lockfile == NodeLock::Yarn {
            let lock = if job.root.is_empty() {
                "yarn.lock".to_owned()
            } else {
                format!("{}/yarn.lock", job.root)
            };
            if files
                .iter()
                .find(|(p, _)| p == &lock)
                .is_some_and(|(_, b)| crate::jobs::is_yarn_berry(b))
            {
                return Ok(ScipRun {
                    status: RunStatus::Unsupported(
                        "Yarn Berry installs need a Berry binary; the image holds classic yarn 1.22.22"
                            .into(),
                    ),
                    index: None,
                    stderr_tail: String::new(),
                    elapsed: Duration::ZERO,
                });
            }
        }
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
            Indexer::ScipTypescript => {
                typescript_runs(job, image, work_dir, cache_dir, budget, egress)?
            }
            Indexer::ScipJava if job.fetch == Fetch::Gradle => gradle_runs(
                image,
                work_dir,
                cache_dir,
                &egress_dir.join("gradle"),
                budget,
            )?,
            Indexer::ScipJava => java_runs(image, work_dir, cache_dir, budget)?,
            Indexer::ScipGo => go_runs(job, image, work_dir, cache_dir, budget)?,
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
                if job.fetch == Fetch::Gradle {
                    // Gradle reads proxy configuration from JVM system properties, not the
                    // environment (contract M6e S2); `--no-daemon` keeps everything in the
                    // launcher JVM, so `GRADLE_OPTS` reaches dependency resolution.
                    fetch.env.push(crate::egress::gradle_proxy_opts(&address));
                }
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
        let mut fetch_failed = None;
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
            // Contract M9d D4: a scip-python job whose fetch failed indexes its own code
            // without the environment. A fetch over its budget or refused by the docker
            // client (`timeout`, `environment`) is decided again next run instead.
            let python = job.indexer == Indexer::ScipPython;
            if !(python && failed.status == RunStatus::Failed(Phase::Fetch)) {
                return Ok(failed);
            }
            fetch_failed = Some(failed);
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
        let indexed = finished(&outcome, index_bytes, Phase::Index, &index, started);
        Ok(match (fetch_failed, &indexed.status) {
            // An index without the environment keeps the fetch's tail (why it is missing);
            // a failed index leaves the fetch's failure, as before D4.
            (Some(failed), RunStatus::Ok) => ScipRun {
                status: RunStatus::OkWithoutEnvironment,
                stderr_tail: failed.stderr_tail,
                ..indexed
            },
            (Some(failed), RunStatus::Failed(_)) => ScipRun {
                elapsed: indexed.elapsed,
                ..failed
            },
            _ => indexed,
        })
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

/// Contract M9d D1, narrowed by amendment 4 (evaluator pass 1, B1): the host an npm fetch's
/// `replace-registry-host` names, read from the job's `package-lock.json`. Every `resolved` URL
/// counts, in the lockfile v1 shape (`dependencies`, nested), v2 (both sections) and v3
/// (`packages`). Of the registry-shaped tarball URLs' hosts ([`registry_tarball_host`]), those
/// `egress` allows (the built-in registries and the hosts opted in through
/// `RB_SCIP_REGISTRIES`) are dropped. Exactly one host left is the answer; none, or several (a
/// setting names one host), give `None`: no setting, npm's default. npm then rewrites only the
/// URLs on that host, so a URL dependency or a git host is fetched as written, and the
/// allow-list decides it.
fn npm_registry_rewrite(lockfile: &[u8], egress: &Egress) -> Option<String> {
    let lock: serde_json::Value = serde_json::from_slice(lockfile).ok()?;
    let mut resolved: Vec<&str> = lock
        .get("packages")
        .and_then(serde_json::Value::as_object)
        .into_iter()
        .flat_map(|packages| packages.values())
        .filter_map(|entry| entry.get("resolved")?.as_str())
        .collect();
    let mut nested: Vec<&serde_json::Value> = lock.get("dependencies").into_iter().collect();
    while let Some(dependencies) = nested.pop() {
        for entry in dependencies
            .as_object()
            .into_iter()
            .flat_map(|d| d.values())
        {
            resolved.extend(entry.get("resolved").and_then(serde_json::Value::as_str));
            nested.extend(entry.get("dependencies"));
        }
    }
    let foreign: std::collections::BTreeSet<String> = resolved
        .into_iter()
        .filter_map(registry_tarball_host)
        .filter(|host| !egress.hosts.contains(host))
        .collect();
    let mut hosts = foreign.into_iter();
    match (hosts.next(), hosts.next()) {
        (Some(host), None) => Some(host),
        _ => None, // none left, or several: no setting, npm's default
    }
}

/// The host of a registry-shaped tarball URL, `http(s)://<host>/…/<name>/-/<file>.tgz`, as npm
/// compares it (`URL.host`: lower case, with a port if one is written, without user
/// information). `<name>` is the last segment of the package name, so a scoped name
/// (`…/@scope/name/-/name-1.0.0.tgz`) and a registry under a path prefix match. Anything else
/// has none: a URL dependency's tarball (`https://cdn.sheetjs.com/xlsx-0.20.3/xlsx-0.20.3.tgz`),
/// a git URL, a local path.
fn registry_tarball_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (authority, path) = rest.split_once('/')?;
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let segments: Vec<&str> = path.split('/').collect();
    match segments.as_slice() {
        [.., name, "-", file] if !host.is_empty() && !name.is_empty() && file.ends_with(".tgz") => {
            Some(host.to_ascii_lowercase())
        }
        _ => None,
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
    egress: &Egress,
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
        Fetch::Go | Fetch::Python { .. } => {
            anyhow::bail!("a Go/Python fetch on a TypeScript job")
        }
    };
    let mut fetch = ContainerRun {
        entrypoint: Some(entrypoint.into()),
        args,
        ..base.clone()
    };
    // npm fetches only (contract M9d D1, narrowed by amendment 4). A lockfile may resolve from
    // a mirror outside the allow-list (nestjs_realworld: 194 entries from `npm.styque.de`, each
    // with the same integrity on `registry.npmjs.org`): `replace-registry-host=<that host>`
    // fetches its tarballs from the configured registry by the same path, and the lockfile's
    // integrity still checks them. Only that one host is named ([`npm_registry_rewrite`]); URL
    // dependencies, git hosts and opted-in hosts are fetched as written (`always` rewrote them
    // all, so an opted-in host could not be reached; evaluator pass 1, B1).
    // `engine-strict=false` stops a project `.npmrc` from refusing the image's node and npm
    // (mattermost webapp). Environment variables beat a project `.npmrc`. Yarn and pnpm runs,
    // and the index phase, are unchanged.
    if let Fetch::Npm {
        lockfile: lockfile @ (NodeLock::PackageLock | NodeLock::None),
    } = job.fetch
    {
        if lockfile == NodeLock::PackageLock
            && let Ok(text) = std::fs::read(fetch.host_cwd().join("package-lock.json"))
            && let Some(host) = npm_registry_rewrite(&text, egress)
        {
            fetch
                .env
                .push(("npm_config_replace_registry_host".into(), host));
        }
        fetch
            .env
            .push(("npm_config_engine_strict".into(), "false".into()));
    }
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

/// Where scip-java's Maven index phase collects its SemanticDB files (contract M9d D12b,
/// amendment 1): a container path outside the `/sources` mount. The default,
/// `target/scip-targetroot` under the sources, is deleted by an aggregator root's `clean` when
/// the reactor builds that root last (its modules inherit another parent), so no `index.scip`
/// was written (spring_boot_microservices; measured with and without it in P0).
const MAVEN_TARGETROOT: &str = "/scip-targetroot";

/// The fetch and index invocations for a scip-java Maven job (docker/scip-java/README.md,
/// verified on the Java fixture). The fetch phase resolves plugins and dependencies into the
/// repository's Maven cache (`go-offline`) and then does one online build, so that the
/// index phase can run the same build with `--offline` under `--network none`. Arguments after
/// `--` replace scip-java's default Maven arguments, so they are repeated plus `--offline`.
/// Maven logs to stdout by default; `MAVEN_OPTS` moves its logger to stderr so a failed build's
/// `[ERROR]` lines reach the stderr tail. scip-java prefers a repository's own `./mvnw` when
/// `access(X_OK)` says it is executable, which Docker Desktop's mounts say of every file, so
/// [`export`] leaves the wrapper out and the image's `mvn` is always the one used. The index
/// phase passes `--targetroot` [`MAVEN_TARGETROOT`], outside the mount (contract M9d D12b).
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
            "--targetroot",
            MAVEN_TARGETROOT,
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

/// Where every Gradle run of a scip-java job finds rb's init script (contract M9d D13), mounted
/// read-only.
const GRADLE_INIT_AT: &str = "/rb-gradle";

/// rb's Gradle init script (contract M9d D13; P0's measured `whenReady` draft). The protobuf
/// plugin downloads `protoc` and its plugins into `GRADLE_USER_HOME` with mode 0644, then runs
/// them only when `File.canExecute()` says no; on Docker Desktop's bind mounts it says yes for
/// root while `execve` fails EACCES (microservices_demo `adservice`: `error: 13`). So, once the
/// task graph is ready, before any task runs, the script resolves each project's
/// `protobufToolsLocator_*` configurations and sets 0755 on every resolved `*.exe` under
/// `GRADLE_USER_HOME`, without asking. A build without the plugin has no such configuration
/// and is untouched; other plugins' native tools are not covered (amendment 1, risk 5).
const GRADLE_INIT_SCRIPT: &str = r#"// rb (contract M9d D13): mark the protobuf plugin's tool artifacts executable.
import java.nio.file.Files
import java.nio.file.attribute.PosixFilePermissions
def rbMarkExec = { Project p ->
  def home = p.gradle.gradleUserHomeDir.toPath().toRealPath()
  p.configurations.findAll { it.name.startsWith('protobufToolsLocator_') }.each { c ->
    c.resolve().findAll { it.name.endsWith('.exe') }.each { f ->
      def path = f.toPath().toRealPath()
      if (path.startsWith(home)) {
        Files.setPosixFilePermissions(path, PosixFilePermissions.fromString('rwxr-xr-x'))
        println "rb-exec-bits: ${p.path} ${c.name} ${home.relativize(path)}"
      }
    }
  }
}
gradle.taskGraph.whenReady { graph ->
  graph.allTasks.findAll { it.class.name.contains('GenerateProtoTask') }*.project.unique().each { rbMarkExec(it) }
}
"#;

/// The fetch and index invocations for a scip-java Gradle job (docker/scip-java/README.md).
/// The fetch phase resolves dependencies (`dependencies`) and then does one online build, so
/// that the index phase can run the same build with `--offline` under `--network none`.
/// Arguments after `--` replace scip-java's default Gradle arguments, so they are repeated
/// plus `--offline`. `--console=plain` keeps progress spinners out of the stderr tail;
/// `--no-daemon` fits ephemeral containers (no daemon is ever reused); `--no-build-cache`
/// forces compilation to run, so javac with the SemanticDB plugin always emits (a cache hit
/// would skip it, the spring-retry lesson of contract M3b amendment 47). The repository's
/// wrapper stays unexported ([`export`]), so the image's `gradle` is always the one used.
/// Every run gets [`GRADLE_INIT_SCRIPT`], written to `init_dir` (the run's own side directory,
/// removed with it) and mounted read-only at [`GRADLE_INIT_AT`] (contract M9d D13).
fn gradle_runs(
    image: &str,
    work_dir: &Path,
    cache_dir: &Path,
    init_dir: &Path,
    budget: &Budget,
) -> anyhow::Result<(Vec<ContainerRun>, ContainerRun)> {
    let workdir = crate::runner::absolute(work_dir)?;
    let gradle_home = crate::runner::absolute(cache_dir)?.join("gradle");
    std::fs::create_dir_all(&gradle_home)?;
    std::fs::create_dir_all(init_dir)?;
    std::fs::write(init_dir.join("exec-bits.gradle"), GRADLE_INIT_SCRIPT)?;
    let init_dir = crate::runner::absolute(init_dir)?;
    let init_script = format!("{GRADLE_INIT_AT}/exec-bits.gradle");
    let init = ["--init-script", init_script.as_str()];
    let base = ContainerRun {
        name: None,
        image: image.into(),
        platform: None,
        workdir,
        mount_at: "/sources".into(),
        cwd: String::new(),
        caches: vec![(gradle_home, "/root/.gradle".into())],
        readonly: vec![(init_dir, GRADLE_INIT_AT.into())],
        // Explicit (the default is also `/root/.gradle` for root): the fetch
        // must never write where another repository's reads.
        env: vec![("GRADLE_USER_HOME".into(), "/root/.gradle".into())],
        network: Network::Fetch,
        cpus: budget.cpus,
        memory: budget.memory.clone(),
        timeout: budget.fetch_timeout,
        entrypoint: None,
        args: Vec::new(),
    };
    let flags = [
        "--console=plain",
        "--quiet",
        "--no-daemon",
        "--no-build-cache",
    ];
    let gradle = |tasks: &[&str]| ContainerRun {
        args: ["gradle"]
            .iter()
            .chain(flags.iter())
            .chain(init.iter())
            .chain(tasks.iter())
            .map(|s| (*s).to_owned())
            .collect(),
        ..base.clone()
    };
    let fetches = vec![
        gradle(&["dependencies"]),
        gradle(&["clean", "compileJava", "compileTestJava"]),
    ];
    let index = ContainerRun {
        network: Network::None,
        timeout: budget.index_timeout,
        args: [
            "scip-java",
            "index",
            "--build-tool",
            "Gradle",
            "--",
            "--offline",
            "--console=plain",
            "--no-daemon",
            "--no-build-cache",
        ]
        .iter()
        .chain(init.iter())
        .chain(["clean", "compileJava", "compileTestJava"].iter())
        .map(|s| (*s).to_owned())
        .collect(),
        ..base
    };
    Ok((fetches, index))
}
/// The fetch and index invocations for a scip-go job (docker/scip-go/README.md).
/// The fetch phase downloads the module graph into the repository's Go module
/// cache (`go mod download`); the index phase runs `scip-go` offline
/// (contract M6d S1; proxy settings and lockfile discipline harden in S3/S4).
///
/// Contract M9d D6: a job with a replace target outside its root has a repository-shaped
/// export and starts in its root, so the relative replace resolves. D5: a job with a
/// workspace gets rb's `go.work` in its root, written here, before either phase runs; both
/// phases run in workspace mode (`GOFLAGS` empty, since workspace mode refuses `-mod=mod`, and
/// `GOWORK` naming the file), and the fetch is `go list -deps -test` over `.` and each
/// workspace directory (amendment 1: it fetches what the workspace's packages load, where `go
/// mod download` fetches the whole workspace module graph, +1.69 GiB on mattermost `server`).
fn go_runs(
    job: &Job,
    image: &str,
    work_dir: &Path,
    cache_dir: &Path,
    budget: &Budget,
) -> anyhow::Result<(Vec<ContainerRun>, ContainerRun)> {
    let workdir = crate::runner::absolute(work_dir)?;
    let gomod = crate::runner::absolute(cache_dir)?.join("gomod");
    std::fs::create_dir_all(&gomod)?;
    let cwd = if job.replace_targets.is_empty() {
        String::new()
    } else {
        job.root.clone()
    };
    // Contract M6d S3: the checksum database lives outside the allow-list, so verification is
    // off; `go.sum` still verifies the modules it pins. The proxy is pinned (no `,direct` git
    // fallback outside the allow-list; S11: it would only fail closed, slowly). The toolchain is
    // pinned too (evaluator pass 1, E1-5): a newer-toolchain repo fails closed instead of
    // downloading one from outside the list.
    let mut env: Vec<(String, String)> = vec![
        ("GOSUMDB".into(), "off".into()),
        ("GOPROXY".into(), "https://proxy.golang.org".into()),
        ("GOTOOLCHAIN".into(), "local".into()),
    ];
    let mut fetch_args = ["go", "mod", "download"].map(String::from).to_vec();
    if job.workspace.is_empty() {
        env.push(("GOFLAGS".into(), "-mod=mod".into()));
    } else {
        let root = workdir.join(&cwd);
        std::fs::create_dir_all(&root)?;
        std::fs::write(root.join("go.work"), go_work(&root, &job.workspace))?;
        env.push(("GOFLAGS".into(), String::new()));
        let at = if cwd.is_empty() {
            "/sources/go.work".to_owned()
        } else {
            format!("/sources/{cwd}/go.work")
        };
        env.push(("GOWORK".into(), at));
        fetch_args = workspace_fetch(&job.workspace);
    }
    let base = ContainerRun {
        name: None,
        image: image.into(),
        platform: None,
        workdir,
        mount_at: "/sources".into(),
        cwd,
        caches: vec![(gomod, "/root/go/pkg/mod".into())],
        readonly: Vec::new(),
        env,
        network: Network::Fetch,
        cpus: budget.cpus,
        memory: budget.memory.clone(),
        timeout: budget.fetch_timeout,
        entrypoint: None,
        args: Vec::new(),
    };
    let fetch = ContainerRun {
        args: fetch_args,
        ..base.clone()
    };
    let index = ContainerRun {
        network: Network::None,
        timeout: budget.index_timeout,
        args: ["scip-go"].map(String::from).to_vec(),
        ..base
    };
    Ok((vec![fetch], index))
}

/// The `go.work` rb writes in a workspace job's root `root` (contract M9d D5): `go` with the
/// highest `go` line of the root's and each directory's `go.mod` in the export (Go's version
/// order; none when no module declares one), then `use` with `.` and each directory, as `go
/// work init` formats it. Written by rb, not by `go work init`, whose `go` line follows the
/// toolchain. Then (amendment 4) a `replace` block: for each directory whose module the root's
/// `go.mod` requires at a version, `<module path> <that version> => ./<dir>`, in the
/// directories' order. Without it, a root that also has a local `replace` makes Go ask the
/// proxy for the required version, which an in-repo module need not have published (the
/// evaluator's `wsrep` and `wsout`); a replace without the version is refused for a
/// workspace module.
fn go_work(root: &Path, workspace: &[String]) -> String {
    let read = |dir: &str| {
        std::fs::read(root.join(dir).join("go.mod"))
            .ok()
            .map(|text| crate::jobs::go_mod(&String::from_utf8_lossy(&text)))
    };
    let main = read(".");
    let modules: Vec<(&String, Option<crate::jobs::GoMod>)> =
        workspace.iter().map(|dir| (dir, read(dir))).collect();
    let version = main
        .iter()
        .chain(modules.iter().filter_map(|(_, module)| module.as_ref()))
        .filter_map(|module| module.go.as_ref())
        .max_by(|a, b| crate::jobs::go_version_key(a).cmp(&crate::jobs::go_version_key(b)));
    let mut text = version.map_or_else(String::new, |v| format!("go {v}\n\n"));
    text.push_str("use (\n\t.\n");
    for dir in workspace {
        text.push_str(&format!("\t./{dir}\n"));
    }
    text.push_str(")\n");
    let replaces: Vec<String> = modules
        .iter()
        .filter_map(|(dir, module)| {
            let path = module.as_ref()?.module.as_ref()?;
            let version = main.as_ref()?.requires.get(path)?;
            (!version.is_empty()).then(|| format!("\t{path} {version} => ./{dir}\n"))
        })
        .collect();
    if !replaces.is_empty() {
        text.push_str("\nreplace (\n");
        text.extend(replaces);
        text.push_str(")\n");
    }
    text
}

/// A workspace job's fetch (contract M9d D5, amendment 1): `go list -deps -test` over `.` and
/// each workspace directory, without `-e`, so a module that cannot be fetched fails the fetch.
fn workspace_fetch(workspace: &[String]) -> Vec<String> {
    ["go", "list", "-deps", "-test", "./..."]
        .map(String::from)
        .into_iter()
        .chain(workspace.iter().map(|dir| format!("./{dir}/...")))
        .collect()
}

/// The fetch and index invocations for a scip-python job (contract M6d S4).
/// The fetch phase resolves the environment behind the egress proxy, driven
/// only by what the job's root pins: a `poetry.lock` installs the locked
/// tree without the root project, a `requirements.txt` installs its pins
/// without transitive dependencies, anything else installs the project
/// itself without dependencies (so an unlisted host is never reached for an
/// unpinned tree, and third-party names stay unresolved — the safe
/// direction). The index runs `scip-python index` offline; the image exists
/// for linux/amd64 only, so the job is pinned there like scip-typescript's.
fn python_runs(
    job: &Job,
    image: &str,
    work_dir: &Path,
    cache_dir: &Path,
    budget: &Budget,
) -> anyhow::Result<(Vec<ContainerRun>, ContainerRun)> {
    let lockfile = match job.fetch {
        Fetch::Python { lockfile } => lockfile,
        _ => anyhow::bail!("a non-Python fetch on a Python job"),
    };
    let workdir = crate::runner::absolute(work_dir)?;
    let pip_cache = crate::runner::absolute(cache_dir)?.join("pip");
    std::fs::create_dir_all(&pip_cache)?;
    let mut caches = vec![(pip_cache, "/root/.cache/pip".into())];
    if lockfile == PythonLock::Poetry {
        let poetry_cache = crate::runner::absolute(cache_dir)?.join("pypoetry");
        std::fs::create_dir_all(&poetry_cache)?;
        caches.push((poetry_cache, "/root/.cache/pypoetry".into()));
    }
    let base = ContainerRun {
        name: None,
        image: image.into(),
        platform: Some("linux/amd64".into()),
        workdir,
        mount_at: "/src".into(),
        // Like scip-typescript's job roots (contract M6a D7): the container
        // starts in the job's root; revisited with evidence in S5.
        cwd: job.root.clone(),
        caches,
        readonly: Vec::new(),
        // pip's update notice depends on the cache's state (contract M5 S22).
        // `PIP_TIMEOUT` (phase 12: pip's 15 s default read timeout trips on
        // slow wheel reads through the emulated stack — measured 19 s ok /
        // 87 s ok / 33 s and 54 s read-timeout failures on one fixture fetch,
        // so 60 s of headroom without masking real outages, which the fetch
        // budget still bounds).
        env: vec![
            ("PIP_DISABLE_PIP_VERSION_CHECK".into(), "1".into()),
            ("PIP_TIMEOUT".into(), "60".into()),
        ],
        network: Network::Fetch,
        cpus: budget.cpus,
        memory: budget.memory.clone(),
        timeout: budget.fetch_timeout,
        entrypoint: None,
        args: Vec::new(),
    };
    let args: Vec<String> = match lockfile {
        // Contract M9d D3: Poetry 1.6.1 prints its errors on stdout, which the runner
        // discards, so stdout joins stderr; `exec` keeps Poetry's exit code.
        PythonLock::Poetry => [
            "sh",
            "-c",
            "exec poetry install --no-root --no-interaction 1>&2",
        ]
        .to_vec(),
        // `--ignore-requires-python`: the image's interpreter (3.10) predates
        // some lockfiles' floors (S11: a `>=3.14` floor fails the fetch
        // otherwise); versions stay pinned, and what the older runtime cannot
        // parse stays a counted gap. Never `--no-deps`-less: transitive trees
        // stay out.
        PythonLock::Requirements => [
            "pip",
            "install",
            "--no-deps",
            "--ignore-requires-python",
            "-r",
            "requirements.txt",
        ]
        .to_vec(),
        PythonLock::None => [
            "pip",
            "install",
            "--no-deps",
            "--ignore-requires-python",
            "-e",
            ".",
        ]
        .to_vec(),
    }
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let fetch = ContainerRun {
        args,
        ..base.clone()
    };
    // Contract M9d D2: scip-python reads the project's name and version from the root's
    // `pyproject.toml` and crashes on an undefined version without one (microblog), so such a
    // root names its project: its last segment, `root` at the repository root. The export is
    // repository-shaped, so the root's own file is `<export>/<root>/pyproject.toml`.
    let mut index_args = ["scip-python", "index"].map(String::from).to_vec();
    let pyproject = base.workdir.join(&job.root).join("pyproject.toml");
    if !pyproject.is_file() {
        let name = job.root.rsplit('/').next().filter(|s| !s.is_empty());
        let name = name.unwrap_or("root");
        index_args.extend(["--project-name", name, "--project-version", "."].map(String::from));
    }
    let index = ContainerRun {
        network: Network::None,
        timeout: budget.index_timeout,
        args: index_args,
        ..base
    };
    Ok((vec![fetch], index))
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

/// Removes a package cache tree (`<work dir>/cache/<repo>`, contract M5 S22). Go's module
/// cache is read-only (directories 0555, files 0444), and unlinking an entry needs write
/// permission on its directory, so every directory first gets owner `rwx` (contract M9d D15);
/// symbolic links are not followed. An absent path is not an error.
pub fn remove_cache_tree(path: &Path) -> std::io::Result<()> {
    fn writable(dir: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::symlink_metadata(dir)?.permissions().mode();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode | 0o700))?;
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                writable(&entry.path())?;
            }
        }
        Ok(())
    }
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
        Ok(m) if m.is_dir() => writable(path)?,
        Ok(_) => {}
    }
    std::fs::remove_dir_all(path)
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
        /// `<mount> <settings.xml>` per read-only mount holding one that a run saw, read while
        /// it ran.
        settings: RefCell<Vec<String>>,
        /// Per run, the Gradle init script mounted at `/rb-gradle`, read while it ran.
        inits: RefCell<Vec<Option<String>>>,
        /// Per run, the text of `go.work` in the directory it started in, read while it ran.
        go_works: RefCell<Vec<Option<String>>>,
        /// Outcomes the next runs take, in order, before `fail_fetch`/`fail_index` apply; an
        /// index run writes its `index.scip` when the flag is set.
        script: RefCell<std::collections::VecDeque<(Outcome, bool)>>,
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
                inits: RefCell::new(Vec::new()),
                go_works: RefCell::new(Vec::new()),
                script: RefCell::new(std::collections::VecDeque::new()),
            }
        }
    }

    const PROXY_AT: &str = "10.9.8.7";

    impl Runner for Fake {
        fn run(&self, run: &ContainerRun) -> anyhow::Result<Outcome> {
            self.runs.borrow_mut().push(run.clone());
            let mut init = None;
            for (host, inside) in &run.readonly {
                if inside == "/rb-gradle" {
                    init = std::fs::read_to_string(host.join("exec-bits.gradle")).ok();
                    continue;
                }
                let settings = std::fs::read_to_string(host.join("settings.xml"))?;
                self.settings
                    .borrow_mut()
                    .push(format!("{inside} {settings}"));
            }
            self.inits.borrow_mut().push(init);
            self.go_works
                .borrow_mut()
                .push(std::fs::read_to_string(run.host_cwd().join("go.work")).ok());
            let is_index = run.network == Network::None;
            if let Some((outcome, writes)) = self.script.borrow_mut().pop_front() {
                if is_index && writes {
                    std::fs::write(run.host_cwd().join("index.scip"), b"fake-index")?;
                }
                return Ok(outcome);
            }
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
            workspace: Vec::new(),
            replace_targets: Vec::new(),
            java_release: None,
        }
    }

    fn java_job(fetch: Fetch) -> Job {
        Job {
            root: "svc".into(),
            indexer: Indexer::ScipJava,
            fetch,
            projects: Vec::new(),
            unpinned: false,
            workspace: Vec::new(),
            replace_targets: Vec::new(),
            java_release: None,
        }
    }

    /// Contract M6e S6: a Yarn Berry root never fetches (the image holds
    /// classic yarn 1.22.22, measured in preflight): `unsupported` with the
    /// reason, before anything is exported or run.
    #[test]
    fn berry_jobs_are_unsupported_and_export_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(false, false);
        let files = vec![
            ("apps/web/package.json".to_owned(), b"{}".to_vec()),
            (
                "apps/web/yarn.lock".to_owned(),
                b"__metadata:\n  version: 8\n".to_vec(),
            ),
        ];
        let run = run_job(
            &ts_job(NodeLock::Yarn),
            "sha256:ts",
            &files,
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            &Budget::default(),
            &Egress::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(
            run.status,
            RunStatus::Unsupported(
                "Yarn Berry installs need a Berry binary; the image holds classic yarn 1.22.22"
                    .into()
            )
        );
        assert!(fake.runs.borrow().is_empty());
        assert!(!tmp.path().join("w").exists(), "nothing is exported");
        // A classic lockfile still runs.
        let files = vec![
            ("apps/web/package.json".to_owned(), b"{}".to_vec()),
            (
                "apps/web/yarn.lock".to_owned(),
                b"# yarn lockfile v1\n".to_vec(),
            ),
        ];
        let run = run_job(
            &ts_job(NodeLock::Yarn),
            "sha256:ts",
            &files,
            &tmp.path().join("w2"),
            &tmp.path().join("c2"),
            &Budget::default(),
            &Egress::default(),
            &fake,
        )
        .unwrap();
        assert!(!matches!(run.status, RunStatus::Unsupported(_)));
        assert!(!fake.runs.borrow().is_empty());
    }

    fn go_job() -> Job {
        Job {
            root: "svc".into(),
            indexer: Indexer::ScipGo,
            fetch: Fetch::Go,
            projects: Vec::new(),
            unpinned: false,
            workspace: Vec::new(),
            replace_targets: Vec::new(),
            java_release: None,
        }
    }

    fn python_job(lockfile: PythonLock) -> Job {
        Job {
            root: "svc".into(),
            indexer: Indexer::ScipPython,
            fetch: Fetch::Python { lockfile },
            projects: Vec::new(),
            unpinned: lockfile != PythonLock::Poetry,
            workspace: Vec::new(),
            replace_targets: Vec::new(),
            java_release: None,
        }
    }

    /// Contract M6d S1: a Go job fetches with `go mod download` on the fetch
    /// network (module cache mounted) and indexes with `scip-go` offline.
    /// Contract M6d S3: the fetch pins `GOSUMDB=off` (the checksum database
    /// is outside the allow-list; `go.sum` still verifies when present) and
    /// `GOPROXY` to the module proxy (S11: no `,direct` git fallback outside
    /// the allow-list).
    #[test]
    fn go_job_runs_download_then_scip_go() {
        let lines = argv_lines(&go_job(), "sha256:go", &Budget::default());
        assert_eq!(lines.len(), 2);
        assert!(
            lines[0].contains("--network rb-scip-fetch")
                && lines[0].contains("-v <work>-egress/client:/rb-egress:ro")
                && lines[0].contains("go mod download")
                && lines[0].contains("/root/go/pkg/mod")
                && lines[0].contains("-e GOSUMDB=off")
                && lines[0].contains("-e GOPROXY=https://proxy.golang.org")
                && lines[0].contains("-e GOTOOLCHAIN=local")
                && lines[0].contains("-e GOFLAGS=-mod=mod"),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].contains("--network none") && lines[1].ends_with("scip-go"),
            "{}",
            lines[1]
        );
    }

    /// A Go module at the export's root (`go 1.26.10`) requiring the in-repo module under
    /// `public/` (`go 1.26.9`, with a `toolchain` line).
    fn go_files() -> Vec<(String, Vec<u8>)> {
        [
            (
                "go.mod",
                "module example.com/server\n\ngo 1.26.10\n\nrequire example.com/server/public v0.4.3\n",
            ),
            (
                "public/go.mod",
                "module example.com/server/public\n\ngo 1.26.9\n\ntoolchain go1.27.2\n",
            ),
            ("main.go", "package main\n"),
        ]
        .map(|(p, t)| (p.to_owned(), t.as_bytes().to_vec()))
        .to_vec()
    }

    /// mattermost `server`'s shape (contract M9d D5).
    fn workspace_job() -> Job {
        Job {
            workspace: vec!["public".into()],
            ..go_job()
        }
    }

    /// Every value `run` sets for the environment variable `key`, in order.
    fn env_of<'a>(run: &'a ContainerRun, key: &str) -> Vec<&'a str> {
        run.env
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .collect()
    }

    /// The `go.work` rb writes for [`workspace_job`] over [`go_files`]: the highest `go` line
    /// of the modules (numerically, `1.26.10` above `1.26.9`), then `.` and each directory,
    /// then a versioned `replace` of each directory's module at the version the root requires
    /// (amendment 4).
    const GO_WORK: &str = "go 1.26.10\n\nuse (\n\t.\n\t./public\n)\n\nreplace (\n\texample.com/server/public v0.4.3 => ./public\n)\n";

    /// Contract M9d D5 (C5): a workspace job writes `<root>/go.work` (deterministic text: the
    /// highest `go` line, then `use` with `.` and each workspace directory, sorted) before the
    /// fetch, so both phases see it; both run with `GOFLAGS` empty (workspace mode refuses
    /// `-mod=mod`) and `GOWORK` naming that file; the fetch is `go list -deps -test` over `.`
    /// and each directory (amendment 1); the recipe is `RECIPE_GO_WORK`. A plain job keeps
    /// today's runs and `RECIPE_GO`. A D6 job (a replace target outside the root) starts in
    /// its root of a repository-shaped export and, without a workspace, keeps `-mod=mod` and
    /// `go mod download`. Amendment 4 (evaluator pass 1, S1): the `go.work` also replaces each
    /// workspace module the root's `go.mod` requires, at that version, with its directory, so
    /// an unpublished version is never asked of the proxy when the root also has a local
    /// `replace` (the evaluator's `wsrep` and `wsout`); a module the root does not require
    /// gets no replace.
    #[test]
    fn workspace_jobs_write_go_work_and_clear_goflags() {
        let tmp = tempfile::tempdir().unwrap();
        let run = |job: &Job, files: &[(String, Vec<u8>)], fake: &Fake| {
            run_job(
                job,
                "sha256:go",
                files,
                &tmp.path().join("w"),
                &tmp.path().join("c"),
                &Budget::default(),
                &Egress::default(),
                fake,
            )
            .unwrap()
        };
        let fake = Fake::new(false, false);
        let ran = run(&workspace_job(), &go_files(), &fake);
        assert_eq!(ran.status, RunStatus::Ok);
        {
            let runs = fake.runs.borrow();
            assert_eq!(runs.len(), 2);
            assert_eq!(runs[0].network, Network::Fetch);
            assert_eq!(runs[1].network, Network::None);
            assert_eq!(
                *fake.go_works.borrow(),
                [Some(GO_WORK.to_owned()), Some(GO_WORK.to_owned())],
                "both phases see the go.work"
            );
            assert_eq!(
                runs[0].args,
                ["go", "list", "-deps", "-test", "./...", "./public/..."]
            );
            assert_eq!(runs[1].args, ["scip-go"]);
            for r in runs.iter() {
                assert_eq!(r.cwd, "", "root-relative, as before");
                assert_eq!(env_of(r, "GOFLAGS"), [""], "{:?}", r.env);
                assert_eq!(env_of(r, "GOWORK"), ["/sources/go.work"], "{:?}", r.env);
                for (k, v) in [
                    ("GOSUMDB", "off"),
                    ("GOPROXY", "https://proxy.golang.org"),
                    ("GOTOOLCHAIN", "local"),
                ] {
                    assert_eq!(env_of(r, k), [v], "{:?}", r.env);
                }
            }
        }
        assert_eq!(recipe(&workspace_job(), &Budget::default()), RECIPE_GO_WORK);
        assert_ne!(RECIPE_GO_WORK, RECIPE_GO);
        // The go.work's text is no input file, so a change to it moves the recipe.
        assert_eq!(RECIPE_GO_WORK, "m9d-go-2");

        // A plain job: today's runs, no go.work, `RECIPE_GO`.
        let fake = Fake::new(false, false);
        run(&go_job(), &go_files(), &fake);
        {
            let runs = fake.runs.borrow();
            assert_eq!(runs[0].args, ["go", "mod", "download"]);
            for r in runs.iter() {
                assert_eq!(env_of(r, "GOFLAGS"), ["-mod=mod"]);
                assert!(env_of(r, "GOWORK").is_empty(), "{:?}", r.env);
            }
            assert_eq!(*fake.go_works.borrow(), [None, None]);
        }
        assert_eq!(recipe(&go_job(), &Budget::default()), RECIPE_GO);

        // Several directories, sorted; the go line is compared numerically (`1.21` above
        // `1.9`), and a module without one adds none.
        let files: Vec<(String, Vec<u8>)> = [
            ("go.mod", "module example.com/m\n\ngo 1.21\n"),
            ("a/go.mod", "module example.com/m/a\n"),
            ("z/y/go.mod", "module example.com/m/z/y\n\ngo 1.9\n"),
        ]
        .map(|(p, t)| (p.to_owned(), t.as_bytes().to_vec()))
        .to_vec();
        let job = Job {
            workspace: vec!["a".into(), "z/y".into()],
            ..go_job()
        };
        let fake = Fake::new(false, false);
        run(&job, &files, &fake);
        assert_eq!(
            fake.go_works.borrow()[0].as_deref(),
            Some("go 1.21\n\nuse (\n\t.\n\t./a\n\t./z/y\n)\n")
        );
        assert_eq!(
            fake.runs.borrow()[0].args,
            [
                "go",
                "list",
                "-deps",
                "-test",
                "./...",
                "./a/...",
                "./z/y/..."
            ]
        );

        // The evaluator's `wsrep`: the root requires the workspace module at an unpublished
        // version and replaces another module with a local directory (the planner leaves that
        // one out of the workspace). The required version is replaced; quotes and the
        // `// indirect` comment do not reach the go.work.
        let files: Vec<(String, Vec<u8>)> = [
            (
                "go.mod",
                "module example.com/wsrep\n\ngo 1.22\n\nrequire (\n\t\"example.com/wsrep/inner\" v0.0.0 // indirect\n\texample.com/wsrep/rep v0.0.0\n)\n\nreplace example.com/wsrep/rep => ./rep\n",
            ),
            ("inner/go.mod", "module example.com/wsrep/inner\n\ngo 1.23\n"),
            ("rep/go.mod", "module example.com/wsrep/rep\n\ngo 1.22\n"),
        ]
        .map(|(p, t)| (p.to_owned(), t.as_bytes().to_vec()))
        .to_vec();
        let job = Job {
            workspace: vec!["inner".into()],
            ..go_job()
        };
        let fake = Fake::new(false, false);
        run(&job, &files, &fake);
        let wsrep = Some(
            "go 1.23\n\nuse (\n\t.\n\t./inner\n)\n\nreplace (\n\texample.com/wsrep/inner v0.0.0 => ./inner\n)\n"
                .to_owned(),
        );
        assert_eq!(
            *fake.go_works.borrow(),
            [wsrep.clone(), wsrep],
            "both phases see the replace"
        );

        // D6: repository-shaped, the container in the root; no workspace keeps `-mod=mod`.
        let files: Vec<(String, Vec<u8>)> = [
            (
                "tools/sct/go.mod",
                "module example.com/sct\n\ngo 1.26.7\n\nrequire example.com/sct/sub v0.1.0\n",
            ),
            (
                "tools/sct/sub/go.mod",
                "module example.com/sct/sub\n\ngo 1.26.7\n",
            ),
            ("server/public/go.mod", "module example.com/public\n"),
        ]
        .map(|(p, t)| (p.to_owned(), t.as_bytes().to_vec()))
        .to_vec();
        let replaced = Job {
            root: "tools/sct".into(),
            replace_targets: vec!["../../server/public".into()],
            ..go_job()
        };
        let fake = Fake::new(false, false);
        run(&replaced, &files, &fake);
        {
            let runs = fake.runs.borrow();
            assert_eq!(runs[0].args, ["go", "mod", "download"]);
            for r in runs.iter() {
                assert_eq!(r.cwd, "tools/sct");
                assert_eq!(env_of(r, "GOFLAGS"), ["-mod=mod"]);
                assert!(env_of(r, "GOWORK").is_empty(), "{:?}", r.env);
            }
            assert_eq!(*fake.go_works.borrow(), [None, None]);
        }
        assert_eq!(recipe(&replaced, &Budget::default()), RECIPE_GO_WORK);
        // D5 with D6: the go.work is written in the root.
        let both = Job {
            workspace: vec!["sub".into()],
            ..replaced
        };
        let fake = Fake::new(false, false);
        run(&both, &files, &fake);
        let runs = fake.runs.borrow();
        assert_eq!(
            fake.go_works.borrow()[0].as_deref(),
            Some(
                "go 1.26.7\n\nuse (\n\t.\n\t./sub\n)\n\nreplace (\n\texample.com/sct/sub v0.1.0 => ./sub\n)\n"
            )
        );
        assert_eq!(
            runs[0].args,
            ["go", "list", "-deps", "-test", "./...", "./sub/..."]
        );
        for r in runs.iter() {
            assert_eq!(r.cwd, "tools/sct");
            assert_eq!(env_of(r, "GOFLAGS"), [""]);
            assert_eq!(env_of(r, "GOWORK"), ["/sources/tools/sct/go.work"]);
        }
    }

    /// Contract M9d D5b (amendment 2, C5b): a workspace run whose fetch or index exits non-zero
    /// (137: an OOM kill), or that writes no index, is run again exactly as a plain job (a
    /// fresh export without go.work, `GOFLAGS=-mod=mod`, `go mod download`). An index from the
    /// re-run makes the job `ok`, stored with phase `index`, with the workspace run's tail; the
    /// pair round-trips through [`RunStatus::from_cached`] to the variant whose `note()` is the
    /// status suffix. A re-run that fails gives its own failure. A timeout or a docker client
    /// failure in the workspace run is not re-run (as D4), and neither is a D6 job (a plain run
    /// cannot resolve its replace) nor a plain job.
    #[test]
    fn a_failed_workspace_run_reindexes_without_go_work() {
        let tmp = tempfile::tempdir().unwrap();
        let outcome = |exit_code: Option<i32>, timed_out: bool, tail: &str| Outcome {
            exit_code,
            timed_out,
            stderr_tail: tail.into(),
            elapsed: Duration::from_millis(1),
        };
        let ok = || (outcome(Some(0), false, ""), true);
        let scripted = |steps: Vec<(Outcome, bool)>| {
            let fake = Fake::new(false, false);
            *fake.script.borrow_mut() = steps.into();
            fake
        };
        let run = |job: &Job, fake: &Fake| {
            run_job(
                job,
                "sha256:go",
                &go_files(),
                &tmp.path().join("w"),
                &tmp.path().join("c"),
                &Budget::default(),
                &Egress::default(),
                fake,
            )
            .unwrap()
        };
        // Runs `from..` are the plain re-run; the runs before it saw the go.work.
        let plain_rerun = |fake: &Fake, from: usize| {
            let runs = fake.runs.borrow();
            assert_eq!(runs.len(), from + 2);
            assert_eq!(runs[from].args, ["go", "mod", "download"]);
            assert_eq!(runs[from + 1].args, ["scip-go"]);
            for r in &runs[from..] {
                assert_eq!(env_of(r, "GOFLAGS"), ["-mod=mod"]);
                assert!(env_of(r, "GOWORK").is_empty(), "{:?}", r.env);
                assert_eq!(r.cwd, "");
            }
            assert_ne!(
                runs[from].workdir, runs[0].workdir,
                "the re-run has an export of its own"
            );
            let works = fake.go_works.borrow();
            assert!(works[..from].iter().all(|w| w.as_deref() == Some(GO_WORK)));
            assert!(
                works[from..].iter().all(Option::is_none),
                "the re-run sees no go.work"
            );
        };
        let suffix = Some("workspace failed: indexed without go.work");

        // The workspace fetch fails: re-run plain.
        let fake = scripted(vec![
            (
                outcome(Some(1), false, "go: example.com/x@v1: not found"),
                false,
            ),
            ok(),
            ok(),
        ]);
        let ran = run(&workspace_job(), &fake);
        assert_eq!(
            (ran.status.as_str(), ran.status.phase()),
            ("ok", Some(Phase::Index))
        );
        assert!(ran.status.is_ok() && ran.status.is_cacheable());
        assert_ne!(ran.status, RunStatus::Ok);
        assert_ne!(ran.status, RunStatus::OkWithoutEnvironment);
        assert_eq!(ran.status.reason(), None);
        assert_eq!(ran.status.note(), suffix);
        assert_eq!(ran.index.as_deref(), Some(&b"fake-index"[..]));
        assert_eq!(
            ran.stderr_tail, "go: example.com/x@v1: not found",
            "the workspace run's tail"
        );
        plain_rerun(&fake, 1);
        // The cached pair round-trips to the variant and its suffix, apart from D4's.
        assert_eq!(
            RunStatus::from_cached("ok", Some("index")),
            Some(ran.status.clone())
        );
        assert_eq!(
            RunStatus::from_cached("ok", Some("index")).and_then(|s| s.note()),
            suffix
        );
        assert_eq!(
            RunStatus::from_cached("ok", Some("fetch")).and_then(|s| s.note()),
            Some("environment not installed: fetch failed")
        );
        assert_eq!(RunStatus::Ok.note(), None);

        // The workspace index is killed (137) or writes no index: re-run plain.
        for (index, tail) in [
            ((outcome(Some(137), false, "Killed"), false), "Killed"),
            (
                (outcome(Some(0), false, "done"), false),
                "done\n(no index.scip was produced)",
            ),
        ] {
            let fake = scripted(vec![ok(), index, ok(), ok()]);
            let ran = run(&workspace_job(), &fake);
            assert_eq!(ran.status.note(), suffix, "{tail}");
            assert_eq!(ran.status.phase(), Some(Phase::Index));
            assert_eq!(ran.index.as_deref(), Some(&b"fake-index"[..]));
            assert_eq!(ran.stderr_tail, tail);
            plain_rerun(&fake, 2);
        }

        // The re-run fails too: its own failure is the job's.
        let fake = scripted(vec![
            ok(),
            (outcome(Some(137), false, "Killed"), false),
            ok(),
            (outcome(Some(2), false, "plain boom"), false),
        ]);
        let ran = run(&workspace_job(), &fake);
        assert_eq!(ran.status, RunStatus::Failed(Phase::Index));
        assert_eq!(ran.stderr_tail, "plain boom");
        assert!(ran.index.is_none());
        plain_rerun(&fake, 2);

        // A timeout or a docker client failure in the workspace run is not re-run.
        for (steps, status) in [
            (
                vec![(outcome(None, true, "slow"), false)],
                RunStatus::Timeout {
                    phase: Phase::Fetch,
                    budget_secs: 600,
                },
            ),
            (
                vec![ok(), (outcome(None, true, "slow"), false)],
                RunStatus::Timeout {
                    phase: Phase::Index,
                    budget_secs: 900,
                },
            ),
            (
                vec![ok(), (outcome(Some(125), false, "docker: refused"), false)],
                RunStatus::Environment("docker: refused".into()),
            ),
        ] {
            let n = steps.len();
            let fake = scripted(steps);
            let ran = run(&workspace_job(), &fake);
            assert_eq!(ran.status, status);
            assert!(ran.index.is_none());
            assert_eq!(fake.runs.borrow().len(), n, "no re-run after {status:?}");
        }

        // A D6 job, with or without a workspace, and a plain job are not re-run.
        for job in [
            Job {
                replace_targets: vec!["../x".into()],
                ..workspace_job()
            },
            Job {
                replace_targets: vec!["../x".into()],
                ..go_job()
            },
            go_job(),
        ] {
            let fake = scripted(vec![ok(), (outcome(Some(137), false, "Killed"), false)]);
            let ran = run(&job, &fake);
            assert_eq!(ran.status, RunStatus::Failed(Phase::Index), "{job:?}");
            assert_eq!(fake.runs.borrow().len(), 2, "{job:?}");
        }
    }

    /// Contract M6d S4: a Python job's fetch resolves its environment behind
    /// the proxy — the locked tree for a poetry.lock, pins without transitive
    /// dependencies for a requirements.txt, the project only otherwise (a
    /// uv.lock has no installer in the image) — and the index stays an
    /// offline `scip-python index` on the emulated platform. pip's version
    /// check is off in every phase (contract M5 S22: its notice depends on
    /// the cache's state).
    #[test]
    fn python_jobs_fetch_lockfile_driven_environments_then_index_offline() {
        for (lockfile, fetch, cache) in [
            (
                PythonLock::Poetry,
                "poetry install --no-root --no-interaction",
                "/root/.cache/pypoetry",
            ),
            (
                PythonLock::Requirements,
                "pip install --no-deps --ignore-requires-python -r requirements.txt",
                "/root/.cache/pip",
            ),
            (
                PythonLock::None,
                "pip install --no-deps --ignore-requires-python -e .",
                "/root/.cache/pip",
            ),
        ] {
            let lines = argv_lines(&python_job(lockfile), "sha256:py", &Budget::default());
            assert_eq!(lines.len(), 2, "{lockfile:?}");
            assert!(
                lines[0].contains("--network rb-scip-fetch")
                    && lines[0].contains("--platform linux/amd64")
                    && lines[0].contains("-v <work>-egress/client:/rb-egress:ro")
                    && lines[0].contains(fetch)
                    && lines[0].contains(cache)
                    && lines[0].contains("-e PIP_DISABLE_PIP_VERSION_CHECK=1")
                    && lines[0].contains("-e PIP_TIMEOUT=60"),
                "{lockfile:?}: {}",
                lines[0]
            );
            // The export (`files()`) holds no `svc/pyproject.toml`, so the root names its
            // project (contract M9d D2).
            assert!(
                lines[1].contains("--network none")
                    && lines[1].contains("--platform linux/amd64")
                    && lines[1].contains("-v <work>:/src -w /src")
                    && lines[1]
                        .ends_with("scip-python index --project-name svc --project-version ."),
                "{lockfile:?}: {}",
                lines[1]
            );
        }
    }

    /// Contract M9d D2 (C3): scip-python reads its project's name and version from a
    /// `pyproject.toml`; without one it crashes on an undefined version (microblog, measured
    /// in planning), so a root without one indexes with `--project-name <its last segment>`
    /// (`root` at the repository root) and `--project-version .`. Only the root's own
    /// `pyproject.toml` counts. A root with one keeps today's arguments.
    #[test]
    fn python_roots_without_pyproject_name_their_project() {
        let index_args = |root: &str, files: &[&str]| -> Vec<String> {
            let tmp = tempfile::tempdir().unwrap();
            let fake = Fake::new(false, false);
            let files: Vec<(String, Vec<u8>)> = files
                .iter()
                .map(|f| ((*f).to_owned(), Vec::new()))
                .collect();
            let job = Job {
                root: root.into(),
                ..python_job(PythonLock::Requirements)
            };
            let run = run_job(
                &job,
                "sha256:py",
                &files,
                &tmp.path().join("w"),
                &tmp.path().join("c"),
                &Budget::default(),
                &Egress::default(),
                &fake,
            )
            .unwrap();
            assert_eq!(run.status, RunStatus::Ok, "{root}");
            let runs = fake.runs.borrow();
            assert_eq!(runs.len(), 2, "{root}");
            runs[1].args.clone()
        };
        let named = |name: &str| {
            [
                "scip-python",
                "index",
                "--project-name",
                name,
                "--project-version",
                ".",
            ]
            .map(String::from)
            .to_vec()
        };
        assert_eq!(
            index_args(
                "src/emailservice",
                &[
                    "src/emailservice/requirements.txt",
                    "src/emailservice/email_server.py"
                ]
            ),
            named("emailservice")
        );
        assert_eq!(
            index_args("", &["requirements.txt", "app.py"]),
            named("root")
        );
        assert_eq!(
            index_args("tools", &["tools/setup.py", "tools/requirements.txt"]),
            named("tools"),
            "a setup.py root has no pyproject.toml either"
        );
        assert_eq!(
            index_args(
                "svc",
                &[
                    "svc/requirements.txt",
                    "pyproject.toml",
                    "svc/lib/pyproject.toml"
                ]
            ),
            named("svc"),
            "another directory's pyproject.toml does not count"
        );
        assert_eq!(
            index_args("svc", &["svc/pyproject.toml", "svc/requirements.txt"]),
            ["scip-python", "index"]
        );
        assert_eq!(
            index_args("", &["pyproject.toml", "app.py"]),
            ["scip-python", "index"]
        );
    }

    /// Contract M9d D2 (C3; evaluator pass 1, e09): a root with a `pyproject.toml`, a
    /// `poetry.lock` and a `requirements.txt` (Poetry's exported pins, a common pair) is a
    /// Poetry root, not a requirements-only one: planned `Poetry` and pinned, fetched with
    /// `poetry install`, and indexed with today's arguments (no project name).
    #[test]
    fn a_poetry_root_with_a_requirements_txt_keeps_poetry_and_todays_arguments() {
        let paths = [
            "svc/pyproject.toml",
            "svc/poetry.lock",
            "svc/requirements.txt",
            "svc/app.py",
        ];
        let plan = crate::jobs::jobs(&paths, &|_| None);
        let [job] = plan.jobs.as_slice() else {
            panic!("{:?}", plan.jobs)
        };
        assert_eq!(
            (job.root.as_str(), &job.fetch, job.unpinned),
            (
                "svc",
                &Fetch::Python {
                    lockfile: PythonLock::Poetry
                },
                false
            )
        );
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(false, false);
        let files: Vec<(String, Vec<u8>)> = paths
            .iter()
            .map(|p| ((*p).to_owned(), Vec::new()))
            .collect();
        let run = run_job(
            job,
            "sha256:py",
            &files,
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            &Budget::default(),
            &Egress::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(run.status, RunStatus::Ok);
        let runs = fake.runs.borrow();
        assert_eq!(runs.len(), 2);
        assert_eq!(
            runs[0].args,
            [
                "sh",
                "-c",
                "exec poetry install --no-root --no-interaction 1>&2"
            ]
        );
        assert_eq!(runs[1].args, ["scip-python", "index"]);
    }

    /// Contract M9d D5b (amendment 2: "The elapsed time covers both runs"; evaluator pass 1,
    /// e07): a job run again without its workspace reports the time from the workspace run's
    /// start to the re-run's end, whether the re-run indexes or fails. The workspace run is held
    /// for 400 ms here, far longer than the plain re-run takes, so a time taken from the re-run
    /// alone falls short of it.
    #[test]
    fn a_workspace_fallbacks_elapsed_covers_both_runs() {
        const HOLD: Duration = Duration::from_millis(400);
        /// [`Fake`], holding its first run for [`HOLD`].
        struct Held<'a> {
            fake: &'a Fake,
            calls: std::cell::Cell<usize>,
        }
        impl Runner for Held<'_> {
            fn run(&self, run: &ContainerRun) -> anyhow::Result<Outcome> {
                if self.calls.replace(self.calls.get() + 1) == 0 {
                    std::thread::sleep(HOLD);
                }
                self.fake.run(run)
            }
            fn image_id(&self, image: &str) -> Option<String> {
                self.fake.image_id(image)
            }
            fn start_proxy(&self, spec: &ProxySpec) -> anyhow::Result<String> {
                self.fake.start_proxy(spec)
            }
            fn stop_proxy(&self, name: &str) -> String {
                self.fake.stop_proxy(name)
            }
        }
        let outcome = |exit_code: i32, tail: &str| Outcome {
            exit_code: Some(exit_code),
            timed_out: false,
            stderr_tail: tail.into(),
            elapsed: Duration::from_millis(1),
        };
        let failed = (outcome(1, "go: example.com/x@v1: not found"), false);
        let ok = (outcome(0, ""), true);
        for (script, status) in [
            (
                vec![failed.clone(), ok.clone(), ok.clone()],
                RunStatus::OkWithoutWorkspace,
            ),
            (
                vec![failed.clone(), failed.clone()],
                RunStatus::Failed(Phase::Fetch),
            ),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let fake = Fake::new(false, false);
            *fake.script.borrow_mut() = script.into();
            let held = Held {
                fake: &fake,
                calls: std::cell::Cell::new(0),
            };
            let ran = run_job(
                &workspace_job(),
                "sha256:go",
                &go_files(),
                &tmp.path().join("w"),
                &tmp.path().join("c"),
                &Budget::default(),
                &Egress::default(),
                &held,
            )
            .unwrap();
            assert_eq!(ran.status, status);
            assert!(
                ran.elapsed >= HOLD,
                "{status:?}: {:?} does not cover the workspace run",
                ran.elapsed
            );
        }
    }

    /// Contract M9d D13 (evaluator pass 1, e12): what rb's Gradle init script marks
    /// executable, read from the script's own predicates and evaluated on a model build, not
    /// from the presence of substrings. The script is Groovy, so its four predicates (the
    /// tasks that select a project, the locator configurations, the tool files, and the guard
    /// on the resolved path) are parsed from its text and evaluated as Groovy would: `String`
    /// `startsWith`/`endsWith`/`contains` on names, `Path.startsWith(Path)` on whole path
    /// elements. Marked must be exactly the protobuf tools under the Gradle home of a project
    /// that generates protos: not a jar, not a tool of another configuration, not one outside
    /// the home (a sibling directory sharing its text prefix included), not one of a project
    /// with no proto task. The mode set is `rwxr-xr-x`.
    #[test]
    fn the_gradle_init_script_marks_only_protobuf_tools_under_the_gradle_home() {
        use std::path::PathBuf;
        enum Val {
            Str(String),
            Path(PathBuf),
        }
        /// One predicate `[!]<subject>.<method>(<'literal'> | <name>)`.
        fn eval(expr: &str, env: &dyn Fn(&str) -> Val) -> bool {
            let expr = expr.trim();
            let (negated, expr) = match expr.strip_prefix('!') {
                Some(rest) => (true, rest.trim()),
                None => (false, expr),
            };
            let open = expr.find('(').unwrap();
            assert!(expr.ends_with(')'), "{expr}");
            let (call, arg) = (&expr[..open], &expr[open + 1..expr.len() - 1]);
            let dot = call.rfind('.').unwrap();
            let (subject, method) = (env(&call[..dot]), &call[dot + 1..]);
            let arg = match arg.strip_prefix('\'').and_then(|a| a.strip_suffix('\'')) {
                Some(literal) => Val::Str(literal.to_owned()),
                None => env(arg),
            };
            let holds = match (subject, method, arg) {
                (Val::Path(p), "startsWith", Val::Path(q)) => p.starts_with(q),
                (Val::Str(s), "startsWith", Val::Str(t)) => s.starts_with(&t),
                (Val::Str(s), "endsWith", Val::Str(t)) => s.ends_with(&t),
                (Val::Str(s), "contains", Val::Str(t)) => s.contains(&t),
                _ => panic!("not modelled: {expr}"),
            };
            holds != negated
        }
        let script = GRADLE_INIT_SCRIPT;
        // The body of the one `findAll { … }` after `receiver`.
        let find_all = |receiver: &str| -> &str {
            let at = format!("{receiver}.findAll {{ ");
            assert_eq!(script.matches(at.as_str()).count(), 1, "{at}");
            let rest = &script[script.find(at.as_str()).unwrap() + at.len()..];
            &rest[..rest.find(" }").unwrap()]
        };
        let tasks = find_all("graph.allTasks");
        let configurations = find_all("p.configurations");
        let tools = find_all("c.resolve()");
        assert_eq!(script.matches("findAll {").count(), 3);
        assert_eq!(script.matches("if (").count(), 1);
        let guard = {
            let rest = &script[script.find("if (").unwrap() + "if (".len()..];
            &rest[..rest.find(") {").unwrap()]
        };
        let mode = {
            let at = "PosixFilePermissions.fromString('";
            let rest = &script[script.find(at).unwrap() + at.len()..];
            &rest[..rest.find('\'').unwrap()]
        };
        assert_eq!(mode, "rwxr-xr-x");
        let home = PathBuf::from("/root/.gradle");
        let cache = "/root/.gradle/caches/modules-2/files-2.1";
        let protoc =
            format!("{cache}/com.google.protobuf/protoc/3.25.1/a1/protoc-3.25.1-linux-x86_64.exe");
        let grpc = format!(
            "{cache}/io.grpc/protoc-gen-grpc-java/1.62.2/b2/protoc-gen-grpc-java-1.62.2-linux-x86_64.exe"
        );
        /// A project, its tasks' classes, and its configurations with their resolved files.
        type Project<'a> = (&'a str, &'a [&'a str], Vec<(&'a str, Vec<String>)>);
        let build: [Project; 2] = [
            (
                ":ads",
                &[
                    "org.gradle.api.tasks.compile.JavaCompile_Decorated",
                    "com.google.protobuf.gradle.GenerateProtoTask_Decorated",
                ],
                vec![
                    ("protobufToolsLocator_protoc", vec![protoc.clone()]),
                    (
                        "protobufToolsLocator_grpc",
                        vec![
                            grpc.clone(),
                            format!("{cache}/io.grpc/grpc-api/1.62.2/c3/grpc-api-1.62.2.jar"),
                        ],
                    ),
                    (
                        "compileClasspath",
                        vec![format!("{cache}/x/tool/1.0/d4/tool-1.0.exe")],
                    ),
                    (
                        "protobufToolsLocator_local",
                        vec![
                            "/sources/tools/protoc.exe".to_owned(),
                            "/root/.gradle-other/protoc.exe".to_owned(),
                        ],
                    ),
                ],
            ),
            (
                ":web",
                &["org.gradle.api.tasks.compile.JavaCompile_Decorated"],
                vec![(
                    "protobufToolsLocator_protoc",
                    vec![format!(
                        "{cache}/com.google.protobuf/protoc/3.25.1/e5/web-protoc.exe"
                    )],
                )],
            ),
        ];
        /// The closure parameter `it` bound to something named `name`.
        fn named(name: &str) -> impl Fn(&str) -> Val + '_ {
            move |v: &str| match v {
                "it.name" | "it.class.name" => Val::Str(name.to_owned()),
                other => panic!("unbound {other}"),
            }
        }
        let mut marked = Vec::new();
        for (_, task_classes, configs) in &build {
            if !task_classes.iter().any(|class| eval(tasks, &named(class))) {
                continue;
            }
            for (config, files) in configs {
                if !eval(configurations, &named(config)) {
                    continue;
                }
                for file in files {
                    let name = file.rsplit('/').next().unwrap();
                    if !eval(tools, &named(name)) {
                        continue;
                    }
                    let env = |v: &str| match v {
                        "path" => Val::Path(PathBuf::from(file)),
                        "home" => Val::Path(home.clone()),
                        other => panic!("unbound {other}"),
                    };
                    if eval(guard, &env) {
                        marked.push(file.clone());
                    }
                }
            }
        }
        assert_eq!(marked, [protoc, grpc]);
    }

    /// Contract M9d D3 (C4): Poetry 1.6.1 prints its errors on stdout, which the runner
    /// discards (fastapi_realworld's stored tail held only `Creating virtualenv …`), so the
    /// Poetry fetch runs under `sh -c` with stdout sent to stderr; `exec` keeps Poetry's exit
    /// code. pip writes its errors to stderr already and runs as before.
    #[test]
    fn poetry_errors_reach_the_tail() {
        let fetch_args = |lockfile: PythonLock| {
            let tmp = tempfile::tempdir().unwrap();
            let fake = Fake::new(false, false);
            run_job(
                &python_job(lockfile),
                "sha256:py",
                &files(),
                &tmp.path().join("w"),
                &tmp.path().join("c"),
                &Budget::default(),
                &Egress::default(),
                &fake,
            )
            .unwrap();
            let runs = fake.runs.borrow();
            assert_eq!(runs[0].network, Network::Fetch);
            assert_eq!(runs[0].entrypoint, None, "{lockfile:?}");
            runs[0].args.clone()
        };
        assert_eq!(
            fetch_args(PythonLock::Poetry),
            [
                "sh",
                "-c",
                "exec poetry install --no-root --no-interaction 1>&2"
            ]
        );
        for lockfile in [PythonLock::Requirements, PythonLock::None] {
            assert_eq!(fetch_args(lockfile)[0], "pip", "{lockfile:?}");
        }
    }

    /// Contract M9d D4 (C4): when a scip-python job's fetch fails (fastapi_realworld: Poetry
    /// cannot build pyyaml 6.0 on musl), the index phase runs anyway, offline, without the
    /// environment. An index makes the job `ok` with phase `fetch` and the fetch's tail (with
    /// the proxy's refusals); it is cached, and the row round-trips through
    /// [`RunStatus::from_cached`]. A fetch timeout, a docker client failure, a failed index
    /// after a failed fetch, and any other indexer's failed fetch keep today's outcome.
    #[test]
    fn a_failed_python_fetch_indexes_without_the_environment() {
        let tmp = tempfile::tempdir().unwrap();
        let run = |job: &Job, fake: &Fake| {
            run_job(
                job,
                "sha256:py",
                &files(),
                &tmp.path().join("w"),
                &tmp.path().join("c"),
                &Budget::default(),
                &Egress::default(),
                fake,
            )
            .unwrap()
        };
        let fake = Fake {
            proxy_log: "Proxying refused on filtered url \"example.org:443\"".into(),
            ..Fake::new(true, false)
        };
        let fallback = run(&python_job(PythonLock::Poetry), &fake);
        assert_eq!(
            (fallback.status.as_str(), fallback.status.phase()),
            ("ok", Some(Phase::Fetch))
        );
        assert_ne!(fallback.status, RunStatus::Ok);
        assert_eq!(fallback.status.reason(), None);
        assert!(fallback.status.is_cacheable());
        assert_eq!(fallback.index.as_deref(), Some(&b"fake-index"[..]));
        assert_eq!(
            fallback.stderr_tail, "boom\negress proxy refused: example.org:443",
            "the fetch's tail"
        );
        {
            let runs = fake.runs.borrow();
            assert_eq!(runs.len(), 2, "the index ran after the failed fetch");
            assert_eq!(runs[0].network, Network::Fetch);
            assert_eq!(runs[1].network, Network::None);
            assert_eq!(runs[1].args[..2], ["scip-python", "index"]);
        }
        // The cache row round-trips; an ordinary `ok` row stays `Ok`, and `ok` in the index
        // phase is D5b's, not D4's (amendment 2; before it, a miss).
        assert_eq!(
            RunStatus::from_cached("ok", Some("fetch")),
            Some(fallback.status.clone())
        );
        assert_eq!(RunStatus::from_cached("ok", None), Some(RunStatus::Ok));
        assert_ne!(
            RunStatus::from_cached("ok", Some("index")),
            Some(fallback.status.clone())
        );

        // The index fails too: the fetch's failure, as before D4.
        let fake = Fake::new(true, true);
        let failed = run(&python_job(PythonLock::Requirements), &fake);
        assert_eq!(failed.status, RunStatus::Failed(Phase::Fetch));
        assert_eq!(failed.stderr_tail, "boom");
        assert!(failed.index.is_none());
        assert_eq!(fake.runs.borrow().len(), 2);

        // A fetch over its budget, or refused by the docker client: nothing indexes.
        let outcome = |exit_code: Option<i32>, timed_out: bool| Outcome {
            exit_code,
            timed_out,
            stderr_tail: "docker: refused".into(),
            elapsed: Duration::from_millis(1),
        };
        for (fetch_outcome, status) in [
            (
                outcome(None, true),
                RunStatus::Timeout {
                    phase: Phase::Fetch,
                    budget_secs: 600,
                },
            ),
            (
                outcome(Some(125), false),
                RunStatus::Environment("docker: refused".into()),
            ),
        ] {
            let fake = Fake {
                fetch_outcome: Some(Ok(fetch_outcome)),
                ..Fake::new(false, false)
            };
            let run = run(&python_job(PythonLock::Poetry), &fake);
            assert_eq!(run.status, status);
            assert!(run.index.is_none());
            assert_eq!(fake.runs.borrow().len(), 1, "no index after {status:?}");
        }

        // Any other indexer's failed fetch stops before indexing.
        for job in [
            ts_job(NodeLock::PackageLock),
            go_job(),
            java_job(Fetch::Maven),
            java_job(Fetch::Gradle),
        ] {
            let fake = Fake::new(true, false);
            let run = run(&job, &fake);
            assert_eq!(run.status, RunStatus::Failed(Phase::Fetch), "{job:?}");
            assert!(run.index.is_none(), "{job:?}");
            assert_eq!(fake.runs.borrow().len(), 1, "{job:?}");
        }
    }

    fn files() -> Vec<(String, Vec<u8>)> {
        vec![
            ("package.json".into(), b"{}".to_vec()),
            ("src/a.ts".into(), b"export const a = 1;".to_vec()),
        ]
    }

    /// Contract D12-lazy C1: resolving without a job that needs JDK 25 never asks the
    /// runtime for the java25 image; resolving with one records its identity like before.
    #[test]
    fn resolve_skips_the_jdk25_lookup_when_no_job_needs_it() {
        struct Counting {
            lookups: RefCell<Vec<String>>,
        }
        impl Runner for Counting {
            fn run(&self, _run: &ContainerRun) -> anyhow::Result<Outcome> {
                panic!("no containers run during resolve")
            }
            fn image_id(&self, image: &str) -> Option<String> {
                self.lookups.borrow_mut().push(image.to_owned());
                Some("sha256:test".to_owned())
            }
            fn start_proxy(&self, _spec: &ProxySpec) -> anyhow::Result<String> {
                panic!("no proxy during resolve")
            }
            fn stop_proxy(&self, _name: &str) -> String {
                panic!("no proxy during resolve")
            }
        }
        let counting = Counting {
            lookups: RefCell::new(Vec::new()),
        };
        let without = Images::resolve(&counting, false);
        assert!(
            !counting
                .lookups
                .borrow()
                .iter()
                .any(|i| i == SCIP_JAVA25_IMAGE),
            "no java25 lookup: {:?}",
            counting.lookups.borrow()
        );
        assert!(without.java25.is_err());
        let with = Images::resolve(&counting, true);
        assert!(
            counting
                .lookups
                .borrow()
                .iter()
                .any(|i| i == SCIP_JAVA25_IMAGE),
            "looked up when needed: {:?}",
            counting.lookups.borrow()
        );
        assert_eq!(with.java25.as_deref(), Ok("sha256:test"));
    }

    #[test]
    fn fetch_then_offline_index_and_the_work_directory_is_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let cache = tmp.path().join("cache");
        let fake = Fake::new(false, false);
        let job = ts_job(NodeLock::PackageLock);
        let images = Images::resolve(&fake, true);
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
        let image = Images::resolve(&fake, true).for_job(&job).unwrap();
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
                "--targetroot",
                "/scip-targetroot",
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
                    "start rb-egress-{instance} files.pythonhosted.org:443,plugins-artifacts.gradle.org:443,plugins.gradle.org:443,proxy.golang.org:443,pypi.org:443,registry.npmjs.org:443,registry.yarnpkg.com:443,repo.maven.apache.org:443,repo.spring.io:443,repo1.maven.org:443,storage.googleapis.com:443"
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
        let images = Images::resolve(&fake, true);
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

    /// Contract M9d D12b (amendment 1): every Maven index phase passes `--targetroot
    /// /scip-targetroot` to scip-java, before `--`, at a container path outside the `/sources`
    /// mount. Its default, `target/scip-targetroot` under the sources, is deleted by an
    /// aggregator root's `clean` when the reactor builds that root last (spring_boot_microservices
    /// wrote no index). The Java recipe is `m9d-java-1`, so every Maven job re-runs once; the
    /// Gradle index is unchanged.
    #[test]
    fn maven_index_phase_writes_the_targetroot_outside_the_sources() {
        let tmp = tempfile::tempdir().unwrap();
        for dir in ["w", "g", "c"] {
            std::fs::create_dir_all(tmp.path().join(dir)).unwrap();
        }
        let (_, index) = java_runs(
            JAVA_ID,
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            &Budget::default(),
        )
        .unwrap();
        let dash = index.args.iter().position(|a| a == "--").unwrap();
        let at = index.args[..dash].iter().position(|a| a == "--targetroot");
        let targetroot = at.map(|at| index.args[at + 1].as_str());
        assert_eq!(targetroot, Some("/scip-targetroot"), "{:?}", index.args);
        assert!(!Path::new("/scip-targetroot").starts_with(&index.mount_at));
        assert_eq!(index.mount_at, "/sources");
        let (_, gradle) = gradle_runs(
            "sha256:java",
            &tmp.path().join("g"),
            &tmp.path().join("c"),
            &tmp.path().join("i"),
            &Budget::default(),
        )
        .unwrap();
        assert!(
            !gradle.args.iter().any(|a| a == "--targetroot"),
            "{:?}",
            gradle.args
        );
        assert_eq!(RECIPE_JAVA, "m9d-java-1");
    }

    #[test]
    fn gradle_job_resolves_builds_then_indexes_offline() {
        let lines = argv_lines(&java_job(Fetch::Gradle), "sha256:java", &Budget::default());
        assert_eq!(lines.len(), 3);
        assert!(
            lines[0].contains("--network rb-scip-fetch")
                && lines[0].contains("-v <work>-egress/client:/rb-egress:ro")
                && lines[0].contains(
                    "gradle --console=plain --quiet --no-daemon --no-build-cache --init-script /rb-gradle/exec-bits.gradle dependencies"
                )
                && lines[0].contains("/root/.gradle")
                && lines[0].contains("-e GRADLE_USER_HOME=/root/.gradle")
                && lines[0].contains("-e GRADLE_OPTS=-Dhttps.proxyHost="),
            "{}",
            lines[0]
        );
        assert!(
            lines[1].contains("--network rb-scip-fetch")
                && lines[1].contains(
                    "gradle --console=plain --quiet --no-daemon --no-build-cache --init-script /rb-gradle/exec-bits.gradle clean compileJava compileTestJava"
                )
                && lines[1].contains("/root/.gradle"),
            "{}",
            lines[1]
        );
        assert!(
            lines[2].contains("--network none")
                && lines[2].contains("scip-java index --build-tool Gradle")
                && lines[2].contains("-- --offline --console=plain --no-daemon --no-build-cache")
                && lines[2].contains("clean compileJava compileTestJava"),
            "{}",
            lines[2]
        );
    }

    /// Contract M9d D13 (C12): both Gradle phases, the two fetches and the index, run with rb's
    /// init script, mounted read-only at `/rb-gradle` and passed as `--init-script` (after
    /// `--` in the index, where scip-java hands it to Gradle). The script, once the task graph
    /// is ready (`whenReady`, P0's measured hook), resolves the protobuf plugin's
    /// `protobufToolsLocator_*` configurations and sets 0755 on each `*.exe` under
    /// `GRADLE_USER_HOME` with `Files.setPosixFilePermissions`, without asking `canExecute`
    /// (which Docker Desktop's bind mounts answer yes for root while `execve` fails). The
    /// Gradle recipe is `m9d-gradle-1`; the run leaves nothing beside the cache.
    #[test]
    fn gradle_runs_mark_tool_artifacts_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let fake = Fake::new(false, false);
        let run = run_job(
            &java_job(Fetch::Gradle),
            "sha256:java",
            &files(),
            &tmp.path().join("w"),
            &tmp.path().join("c"),
            &Budget::default(),
            &Egress::default(),
            &fake,
        )
        .unwrap();
        assert_eq!(run.status, RunStatus::Ok);
        let runs = fake.runs.borrow();
        assert_eq!(runs.len(), 3);
        let flag = [
            "--init-script".to_owned(),
            "/rb-gradle/exec-bits.gradle".to_owned(),
        ];
        for (k, r) in runs.iter().enumerate() {
            assert!(
                r.readonly.iter().any(|(_, inside)| inside == "/rb-gradle"),
                "run {k}: {:?}",
                r.readonly
            );
            let at = r.args.windows(2).position(|w| w == flag);
            assert!(at.is_some(), "run {k}: {:?}", r.args);
            if r.network == Network::None {
                let dash = r.args.iter().position(|a| a == "--").unwrap();
                assert!(at.unwrap() > dash, "{:?}", r.args);
            }
        }
        let inits = fake.inits.borrow();
        assert_eq!(inits.len(), 3);
        for (k, text) in inits.iter().enumerate() {
            let text = text
                .as_deref()
                .unwrap_or_else(|| panic!("run {k} saw no init script"));
            for needle in [
                "gradle.taskGraph.whenReady",
                "protobufToolsLocator_",
                "endsWith('.exe')",
                "gradleUserHomeDir",
                "Files.setPosixFilePermissions",
                "rwxr-xr-x",
            ] {
                assert!(text.contains(needle), "run {k}: {needle}\n{text}");
            }
            let code: Vec<&str> = text
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .collect();
            assert!(!code.iter().any(|l| l.contains("canExecute")), "{text}");
        }
        let left: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(left, ["c"], "the run's export and side files are removed");
        assert_eq!(RECIPE_GRADLE, "m9d-gradle-1");
    }

    #[test]
    fn a_missing_java_image_is_unsupported_and_exports_refuse_unsafe_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let no_image = Fake {
            java_image: false,
            ..Fake::new(false, false)
        };
        let images = Images::resolve(&no_image, true);
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

    /// Contract M9d D1 (C2), narrowed by amendment 4 (evaluator pass 1, B1): an npm fetch
    /// names the lockfile's one foreign registry host in `replace-registry-host`
    /// (nestjs_realworld resolves 194 entries from `npm.styque.de`, outside the allow-list), and
    /// passes no such setting when that host is opted in or the job has no `package-lock.json`.
    /// It ignores a project `.npmrc`'s `engine-strict` (mattermost webapp). Environment variables
    /// beat the project's `.npmrc`. Yarn and pnpm runs, and every index phase, keep today's
    /// environment. The recipe is bumped, so every cached TypeScript row re-runs once.
    #[test]
    fn npm_fetches_rewrite_registry_hosts_and_ignore_engines() {
        assert_eq!(RECIPE_TS, "m9d-ts-2");
        let engines: (String, String) = ("npm_config_engine_strict".into(), "false".into());
        let today: Vec<(String, String)> = vec![
            ("NODE_OPTIONS".into(), "--max-old-space-size=4096".into()),
            ("npm_config_update_notifier".into(), "false".into()),
        ];
        // A v1 lockfile (nestjs_realworld's shape) with one entry from the mirror.
        let lock = br#"{"lockfileVersion": 1, "dependencies": {"rxjs": {"version": "6.5.0",
            "resolved": "https://npm.styque.de/rxjs/-/rxjs-6.5.0.tgz"}}}"#;
        let plain = files();
        let mut locked = files();
        locked.push(("apps/web/package-lock.json".into(), lock.to_vec()));
        let opted_in = Egress::with(&["npm.styque.de".to_owned()]).unwrap();
        let tmp = tempfile::tempdir().unwrap();
        for (lockfile, npm) in [
            (NodeLock::PackageLock, true),
            (NodeLock::None, true),
            (NodeLock::Yarn, false),
            (NodeLock::Pnpm, false),
        ] {
            let job = ts_job(lockfile);
            assert_eq!(recipe(&job, &Budget::default()), RECIPE_TS, "{lockfile:?}");
            for (files, egress, rewrite) in [
                (
                    &locked,
                    &Egress::default(),
                    lockfile == NodeLock::PackageLock,
                ),
                (&locked, &opted_in, false),
                (&plain, &Egress::default(), false),
            ] {
                let what = format!("{lockfile:?}, {} files, {:?}", files.len(), egress.hosts);
                let fake = Fake::new(false, false);
                run_job(
                    &job,
                    SCIP_TYPESCRIPT_IMAGE,
                    files,
                    &tmp.path().join("w"),
                    &tmp.path().join("c"),
                    &Budget::default(),
                    egress,
                    &fake,
                )
                .unwrap();
                let runs = fake.runs.borrow();
                assert_eq!(runs.len(), 2, "{what}");
                let (fetch, index) = (&runs[0], &runs[1]);
                let expected: Vec<&str> = if rewrite {
                    vec!["npm.styque.de"]
                } else {
                    Vec::new()
                };
                assert_eq!(
                    env_of(fetch, "npm_config_replace_registry_host"),
                    expected,
                    "{what}"
                );
                assert_eq!(fetch.env.contains(&engines), npm, "{what}: {:?}", fetch.env);
                // Apart from the two settings, every fetch starts with today's environment (the
                // egress proxy's variables follow it).
                let rest: Vec<(String, String)> = fetch
                    .env
                    .iter()
                    .filter(|(k, _)| {
                        k != "npm_config_replace_registry_host" && k != "npm_config_engine_strict"
                    })
                    .cloned()
                    .collect();
                assert_eq!(rest[..today.len()], today[..], "{what}");
                assert_eq!(index.env, today, "{what}");
            }
        }
    }

    /// Contract M9d D1, amendment 4 (evaluator pass 1, B1): the host `replace-registry-host`
    /// names is the lockfile's one foreign registry host. Every `resolved` URL is read, in the
    /// v1 shape (`dependencies`, nested), v2 (both) and v3 (`packages`); only registry-shaped
    /// tarball URLs (`…/<name>/-/<file>.tgz`, scoped names included) count, so a URL dependency
    /// (the evaluator's `cdn.sheetjs.com` tarball) and `git+ssh`/`git+https` hosts are never
    /// rewritten; hosts on the allow-list or opted in (`RB_SCIP_REGISTRIES`) are dropped; none
    /// or two left give no setting.
    #[test]
    fn npm_rewrite_names_the_lockfiles_one_foreign_registry_host() {
        let npmjs = "https://registry.npmjs.org/lodash/-/lodash-4.17.21.tgz";
        let yarnpkg = "https://registry.yarnpkg.com/left-pad/-/left-pad-1.3.0.tgz";
        let mirror = "https://npm.styque.de/rxjs/-/rxjs-6.5.0.tgz";
        let scoped = "https://npm.styque.de/@nestjs/core/-/core-7.0.0.tgz";
        let other = "https://registry.example.com/npm/left-pad/-/left-pad-1.3.0.tgz";
        let url_dep = "https://cdn.sheetjs.com/xlsx-0.20.3/xlsx-0.20.3.tgz";
        let git_ssh = "git+ssh://git@github.com/mattermost/react-bootstrap.git#c3fcb4d";
        let git_https = "git+https://github.com/larkox/js-client-oauth2.git#a1b2c3d";
        let v3 = |urls: &[&str]| -> Vec<u8> {
            let packages: serde_json::Map<String, serde_json::Value> = urls
                .iter()
                .enumerate()
                .map(|(i, u)| {
                    (
                        format!("node_modules/p{i}"),
                        serde_json::json!({"version": "1.0.0", "resolved": u}),
                    )
                })
                .chain([("".to_owned(), serde_json::json!({"name": "app"}))])
                .collect();
            serde_json::to_vec(&serde_json::json!({"lockfileVersion": 3, "packages": packages}))
                .unwrap()
        };
        // v1: `dependencies`, the second URL only nested under the first entry.
        let v1 = |top: &str, nested: &str| -> Vec<u8> {
            serde_json::to_vec(&serde_json::json!({
                "lockfileVersion": 1,
                "dependencies": {"a": {"version": "1.0.0", "resolved": top,
                    "dependencies": {"b": {"version": "1.0.0", "resolved": nested}}}}
            }))
            .unwrap()
        };
        let default = Egress::default();
        let opted_in = Egress::with(&["npm.styque.de".to_owned()]).unwrap();
        let sheetjs = Egress::with(&["cdn.sheetjs.com".to_owned()]).unwrap();
        let styque = Some("npm.styque.de".to_owned());
        let rows: Vec<(&str, Vec<u8>, &Egress, Option<String>)> = vec![
            (
                "one foreign host",
                v3(&[npmjs, mirror]),
                &default,
                styque.clone(),
            ),
            (
                "scoped name",
                v3(&[npmjs, scoped]),
                &default,
                styque.clone(),
            ),
            ("opted in", v3(&[npmjs, mirror, scoped]), &opted_in, None),
            (
                "opted in, another left",
                v3(&[mirror, other]),
                &opted_in,
                Some("registry.example.com".to_owned()),
            ),
            ("allow-listed only", v3(&[npmjs, yarnpkg]), &default, None),
            ("URL dependency", v3(&[npmjs, url_dep]), &default, None),
            ("URL dependency opted in", v3(&[url_dep]), &sheetjs, None),
            ("git", v3(&[npmjs, git_ssh, git_https]), &default, None),
            (
                "git beside a mirror",
                v3(&[git_ssh, mirror]),
                &default,
                styque.clone(),
            ),
            ("two foreign hosts", v3(&[mirror, other]), &default, None),
            ("v1 nested", v1(npmjs, scoped), &default, styque.clone()),
            ("v1 nested, two hosts", v1(mirror, other), &default, None),
            (
                "v2 both sections",
                serde_json::to_vec(&serde_json::json!({
                    "lockfileVersion": 2,
                    "packages": {"node_modules/rxjs": {"resolved": mirror}},
                    "dependencies": {"rxjs": {"resolved": mirror}}
                }))
                .unwrap(),
                &default,
                styque.clone(),
            ),
            ("not JSON", b"{".to_vec(), &default, None),
        ];
        for (what, lockfile, egress, expected) in rows {
            assert_eq!(npm_registry_rewrite(&lockfile, egress), expected, "{what}");
        }
    }

    /// Contract M6a D6, D8: a grouped workspace job fetches at its root with today's
    /// commands (`npm ci` for `package-lock.json`, `yarn install --frozen-lockfile` for
    /// `yarn.lock`, scripts off) and indexes every member's `tsconfig.json` directory after
    /// `--`, in the job's (sorted) order; the recipes are per indexer.
    #[test]
    fn a_workspace_job_fetches_at_its_root_and_indexes_every_member() {
        assert_eq!((RECIPE_JAVA, RECIPE_TS), ("m9d-java-1", "m9d-ts-2"));
        assert_eq!(RECIPE_PY, "m9d-py-1");
        assert_eq!(RECIPE_GRADLE, "m9d-gradle-1");
        assert_eq!(
            recipe(&java_job(Fetch::Maven), &Budget::default()),
            RECIPE_JAVA
        );
        assert_eq!(
            recipe(&java_job(Fetch::Gradle), &Budget::default()),
            RECIPE_GRADLE
        );
        assert_eq!(
            recipe(&ts_job(NodeLock::PackageLock), &Budget::default()),
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
                workspace: Vec::new(),
                replace_targets: Vec::new(),
                java_release: None,
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
    /// cache keys are unchanged. Since M9d D1 the npm fetch also carries
    /// `npm_config_engine_strict=false` (and `RECIPE_TS` moved, so cached TypeScript rows re-run
    /// once); since amendment 4, `npm_config_replace_registry_host` only for a lockfile with one
    /// foreign registry host, and this job's files hold no lockfile. Every other line is as
    /// recorded.
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
            "run --rm --name <name> --network none --cpus 2 --memory 4g -v <work>:/sources -w /sources -v <tmp>/c/m2:/root/.m2 -e MAVEN_OPTS=-Dorg.slf4j.simpleLogger.logFile=System.err sha256:66a0b395166fc65c49cc099b0b3e4d2cb61e16c177dbc3c022160bb0b4cd1cf2 scip-java index --build-tool Maven --targetroot /scip-targetroot -- --batch-mode --offline clean verify -DskipTests {OFF}",
            "run --rm --name <name> --platform linux/amd64 --network rb-scip-fetch --dns 127.0.0.1 --cpus 2 --memory 4g -v <work>:/src -w /src -v <tmp>/c/npm:/root/.npm -v <work>-egress/client:/rb-egress:ro -e NODE_OPTIONS=--max-old-space-size=4096 -e npm_config_update_notifier=false -e npm_config_engine_strict=false {PROXY} --entrypoint /usr/local/bin/actual-npm sourcegraph/scip-typescript@sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845 ci --no-audit --no-fund --loglevel=error --ignore-scripts --logs-max=0",
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
        assert_eq!(recipe(&java_job(Fetch::Maven), &threaded), "m9d-java-1+T4");
        assert_eq!(
            recipe(&java_job(Fetch::Maven), &Budget::default()),
            RECIPE_JAVA
        );
        assert_eq!(recipe(&java_job(Fetch::Gradle), &threaded), RECIPE_GRADLE);
        assert_eq!(recipe(&ts_job(NodeLock::PackageLock), &threaded), RECIPE_TS);
        let files = [("pom.xml", "1")];
        assert_ne!(
            crate::project_input_hash(
                &recipe(&java_job(Fetch::Maven), &threaded),
                files,
                &Egress::default()
            ),
            crate::project_input_hash(
                &recipe(&java_job(Fetch::Maven), &Budget::default()),
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

    /// Contract M9d D15 (C18): Go's module cache is read-only (directories 0555, files 0444),
    /// so a plain `remove_dir_all` fails on it with `PermissionDenied` and the e2e caches
    /// leaked. Such a tree is removed whole, as is a plain one; an absent path is no error; a
    /// symbolic link inside the tree is removed without touching its target's modes.
    #[test]
    fn cache_trees_with_read_only_modules_are_removed() {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |p: &Path, m: u32| {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap()
        };
        let mode_of = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        let tmp = tempfile::tempdir().unwrap();
        let outside = tmp.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        mode(&outside, 0o555);

        let go = tmp.path().join("t_go");
        let module = go.join("gomod/github.com/x/y@v1.0.0");
        std::fs::create_dir_all(module.join("sub")).unwrap();
        std::fs::write(module.join("go.mod"), "module github.com/x/y\n").unwrap();
        std::fs::write(module.join("sub/a.go"), "package sub\n").unwrap();
        std::os::unix::fs::symlink(&outside, go.join("gomod/link")).unwrap();
        for file in ["go.mod", "sub/a.go"] {
            mode(&module.join(file), 0o444);
        }
        for dir in [
            module.join("sub"),
            module.clone(),
            go.join("gomod/github.com/x"),
        ] {
            mode(&dir, 0o555);
        }
        assert!(
            std::fs::write(module.join("new"), "").is_err(),
            "the tree is read-only"
        );
        remove_cache_tree(&go).unwrap();
        assert!(!go.exists());
        assert!(outside.is_dir());
        assert_eq!(mode_of(&outside), 0o555, "a link's target keeps its modes");

        let plain = tmp.path().join("t_plain");
        std::fs::create_dir_all(plain.join("a")).unwrap();
        std::fs::write(plain.join("a/b.txt"), "x").unwrap();
        remove_cache_tree(&plain).unwrap();
        assert!(!plain.exists());

        remove_cache_tree(&tmp.path().join("absent")).unwrap();
        mode(&outside, 0o755);
    }
}
