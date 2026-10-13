//! Running one container (design §5.2): the command line is built here and executed by a
//! [`Runner`], so orchestration is unit-tested against a fake and only the e2e tests need Docker.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// What a container may reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// The fetch phase: the internal network [`crate::egress::FETCH_NETWORK`] (no route out),
    /// whose only other member is the run's egress proxy, and no resolver of its own
    /// (`--dns 127.0.0.1`; the network's embedded resolver answers no external name anyway;
    /// contract M5 D15, S21).
    Fetch,
    /// The index phase: nothing.
    None,
}

/// One container invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerRun {
    /// The container's name (`rb-scip-<input_hash16>-<pid>-<nanos>-<fetch|index>`, unique to
    /// one run of the job), so a run that exceeds its budget can be killed by name (contract
    /// M3b amendments 18 and 22); `None` leaves it to Docker.
    pub name: Option<String>,
    /// Image reference pinned by digest.
    pub image: String,
    /// `linux/amd64` when the image has no native variant (scip-typescript on arm64).
    pub platform: Option<String>,
    /// Host directory mounted read-write at `mount_at`.
    pub workdir: PathBuf,
    pub mount_at: String,
    /// The container's working directory, relative to `mount_at` (`""`: `mount_at` itself).
    /// A scip-typescript job's export is repository-shaped and starts in its root (contract
    /// M6a D7); a scip-java job's export is its root.
    pub cwd: String,
    /// Extra mounts `(host, container)`: dependency caches shared between the two phases.
    pub caches: Vec<(PathBuf, String)>,
    /// Read-only mounts `(host, container)`: the egress client configuration.
    pub readonly: Vec<(PathBuf, String)>,
    pub env: Vec<(String, String)>,
    pub network: Network,
    pub cpus: f32,
    pub memory: String,
    pub timeout: Duration,
    /// Command and arguments; `None` uses the image's entrypoint and passes only `args`.
    pub entrypoint: Option<String>,
    pub args: Vec<String>,
}

impl ContainerRun {
    /// The host directory the container starts in: `workdir` joined with `cwd`.
    pub fn host_cwd(&self) -> PathBuf {
        if self.cwd.is_empty() {
            self.workdir.clone()
        } else {
            self.workdir.join(&self.cwd)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// The last lines of stderr, kept for `scip_status` reporting.
    pub stderr_tail: String,
    pub elapsed: Duration,
}

impl Outcome {
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

pub trait Runner {
    fn run(&self, run: &ContainerRun) -> anyhow::Result<Outcome>;

    /// The content-addressed ID (`sha256:…`) of a locally present image, `None` when the runtime
    /// does not have it. A digest-pinned reference needs no lookup; a locally built image (the
    /// scip-java one) has no registry digest, so its ID is what identifies it in the cache key.
    fn image_id(&self, image: &str) -> Option<String>;

    /// Starts one fetch run's egress proxy (contract M5 S21): creates the fetch and egress
    /// networks when absent, starts `spec.name` from `spec.image` on the egress network with
    /// `spec.config_dir` mounted read-only, connects it to the fetch network, waits until it
    /// listens, and returns its address there (the fetch containers need no DNS to reach it).
    fn start_proxy(&self, spec: &ProxySpec) -> anyhow::Result<String>;

    /// Stops and removes the proxy `name`; returns what it logged (its refusals are reported
    /// with a failed fetch). Never fails: a proxy already gone logged nothing.
    fn stop_proxy(&self, name: &str) -> String;
}

/// One run's egress proxy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxySpec {
    /// `rb-egress-<run instance>`: unique to the run, like its containers.
    pub name: String,
    pub image: String,
    /// Holds `tinyproxy.conf` and `filter` ([`crate::egress::Egress::write_config`]).
    pub config_dir: PathBuf,
}

/// The `docker run` arguments for `run`: no credentials, no host toolchains, the network
/// disabled unless the phase fetches, a CPU and memory budget, and the work directory mounted.
pub fn docker_args(run: &ContainerRun) -> Vec<String> {
    let mut a: Vec<String> = vec!["run".into(), "--rm".into()];
    if let Some(name) = &run.name {
        a.extend(["--name".into(), name.clone()]);
    }
    if let Some(p) = &run.platform {
        a.extend(["--platform".into(), p.clone()]);
    }
    match run.network {
        Network::Fetch => a.extend([
            "--network".into(),
            crate::egress::FETCH_NETWORK.into(),
            "--dns".into(),
            "127.0.0.1".into(),
        ]),
        Network::None => a.extend(["--network".into(), "none".into()]),
    }
    a.extend([
        "--cpus".into(),
        format!("{}", run.cpus),
        "--memory".into(),
        run.memory.clone(),
        "-v".into(),
        format!("{}:{}", run.workdir.display(), run.mount_at),
        "-w".into(),
        if run.cwd.is_empty() {
            run.mount_at.clone()
        } else {
            format!("{}/{}", run.mount_at, run.cwd)
        },
    ]);
    for (host, inside) in &run.caches {
        a.extend(["-v".into(), format!("{}:{}", host.display(), inside)]);
    }
    for (host, inside) in &run.readonly {
        a.extend(["-v".into(), format!("{}:{}:ro", host.display(), inside)]);
    }
    for (k, v) in &run.env {
        a.extend(["-e".into(), format!("{k}={v}")]);
    }
    if let Some(e) = &run.entrypoint {
        a.extend(["--entrypoint".into(), e.clone()]);
    }
    a.push(run.image.clone());
    a.extend(run.args.iter().cloned());
    a
}

/// Runs containers with the `docker` CLI.
pub struct DockerRunner {
    /// The CLI program (`docker`; a fake in tests).
    pub docker: PathBuf,
    /// How long a timed-out run's client gets to exit after `docker kill` before it is killed
    /// itself.
    pub kill_grace: Duration,
    /// The bound on every other CLI call (inspect, kill, logs, network and proxy commands): a
    /// wedged daemon makes the CLI hang, never fail (contract M7b S2).
    pub cli_timeout: Duration,
}

impl Default for DockerRunner {
    fn default() -> Self {
        DockerRunner {
            docker: PathBuf::from("docker"),
            kill_grace: Duration::from_secs(10),
            cli_timeout: CLI_TIMEOUT,
        }
    }
}

/// How long `docker version` may take before the daemon counts as not responding.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// The default bound on Docker CLI calls other than container runs (see
/// [`DockerRunner::cli_timeout`]).
pub const CLI_TIMEOUT: Duration = Duration::from_secs(60);

/// Whether `docker version` succeeds (the CLI is installed and a daemon answers) within
/// [`PROBE_TIMEOUT`]; the error is the user-facing reason when it does not.
pub fn docker_available() -> Result<(), String> {
    docker_available_with(Path::new("docker"), PROBE_TIMEOUT)
}

/// [`docker_available`] for a given CLI program and bound. A CLI that does not answer in time
/// (a wedged daemon) is killed and reaped, and reported as not responding (contract M7b S2).
pub fn docker_available_with(docker: &Path, limit: Duration) -> Result<(), String> {
    let out = bounded_output(
        Command::new(docker).args(["version", "--format", "{{.Server.Version}}"]),
        limit,
    );
    match out {
        Ok(Some(o)) if o.status.success() => Ok(()),
        Ok(Some(o)) => Err(format!(
            "docker is not available: {}",
            tail(&String::from_utf8_lossy(&o.stderr), 1)
        )),
        Ok(None) => Err(format!(
            "docker is not responding (`docker version` gave no answer within {} s; restart \
             the Docker daemon)",
            limit.as_secs_f64()
        )),
        Err(e) => Err(format!("docker is not available: {e}")),
    }
}

/// Runs `cmd` (stdin closed, stdout and stderr captured) for at most `limit`: its output, or
/// `None` when it overran and was killed and reaped (contract M7b S2).
fn bounded_output(
    cmd: &mut Command,
    limit: Duration,
) -> std::io::Result<Option<std::process::Output>> {
    use std::io::Read;
    let started = std::time::Instant::now();
    let mut child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout = drain(
        child
            .stdout
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let stderr = drain(
        child
            .stderr
            .take()
            .map(|p| Box::new(p) as Box<dyn Read + Send>),
    );
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() >= limit {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stderr) = (
        stdout.join().unwrap_or_default(),
        stderr.join().unwrap_or_default(),
    );
    Ok(status.map(|status| std::process::Output {
        status,
        stdout,
        stderr,
    }))
}

impl DockerRunner {
    /// `docker kill <name>`: stops the container a timed-out client was attached to, since
    /// killing the client alone leaves the container running (contract M3b amendment 18).
    /// Errors (the container already gone) are ignored.
    fn kill_container(&self, name: &str) {
        let _ = bounded_output(
            Command::new(&self.docker).args(["kill", name]),
            self.cli_timeout,
        );
    }
}

impl Runner for DockerRunner {
    fn run(&self, run: &ContainerRun) -> anyhow::Result<Outcome> {
        let started = std::time::Instant::now();
        let mut child = Command::new(&self.docker)
            .args(docker_args(run))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("starting docker: {e}"))?;
        let stderr = child.stderr.take();
        let reader = std::thread::spawn(move || {
            use std::io::Read as _;
            let mut buf = String::new();
            if let Some(mut s) = stderr {
                let _ = s.read_to_string(&mut buf);
            }
            buf
        });
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if started.elapsed() > run.timeout {
                timed_out = true;
                // The container first, by name; the client exits once it is gone (with `--rm`,
                // once it is removed). A client that stays is killed after the grace period.
                if let Some(name) = &run.name {
                    self.kill_container(name);
                    let deadline = std::time::Instant::now() + self.kill_grace;
                    while child.try_wait()?.is_none() && std::time::Instant::now() < deadline {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        let stderr = reader.join().unwrap_or_default();
        Ok(Outcome {
            exit_code: status.and_then(|s| s.code()),
            timed_out,
            stderr_tail: tail(&stderr, 40),
            elapsed: started.elapsed(),
        })
    }

    fn image_id(&self, image: &str) -> Option<String> {
        let out = bounded_output(
            Command::new(&self.docker).args(["image", "inspect", "--format", "{{.Id}}", image]),
            self.cli_timeout,
        )
        .ok()??;
        let id = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (out.status.success() && id.starts_with("sha256:")).then_some(id)
    }

    fn start_proxy(&self, spec: &ProxySpec) -> anyhow::Result<String> {
        use crate::egress::{EGRESS_NETWORK, FETCH_NETWORK, LABEL, PROXY_CONFIG_AT, PROXY_PORT};
        self.ensure_network(FETCH_NETWORK, true)?;
        self.ensure_network(EGRESS_NETWORK, false)?;
        let config = format!(
            "{}:{PROXY_CONFIG_AT}:ro",
            absolute(&spec.config_dir)?.display()
        );
        let label = format!("{LABEL}=proxy");
        self.docker_ok(&[
            "run",
            "-d",
            "--rm",
            "--name",
            &spec.name,
            "--label",
            &label,
            "--network",
            EGRESS_NETWORK,
            "--cpus",
            "0.5",
            "--memory",
            "64m",
            "-v",
            &config,
            &spec.image,
        ])?;
        let started = (|| -> anyhow::Result<String> {
            self.docker_ok(&["network", "connect", FETCH_NETWORK, &spec.name])?;
            let format =
                format!("{{{{(index .NetworkSettings.Networks \"{FETCH_NETWORK}\").IPAddress}}}}");
            let address = self.docker_ok(&["inspect", "--format", &format, &spec.name])?;
            anyhow::ensure!(
                !address.is_empty(),
                "the egress proxy has no address on {FETCH_NETWORK}"
            );
            let port = PROXY_PORT.to_string();
            for _ in 0..100 {
                if self
                    .docker_ok(&["exec", &spec.name, "nc", "-z", "127.0.0.1", &port])
                    .is_ok()
                {
                    return Ok(address);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            anyhow::bail!("the egress proxy did not listen within 10 s")
        })();
        if started.is_err() {
            self.stop_proxy(&spec.name);
        }
        started
    }

    fn stop_proxy(&self, name: &str) -> String {
        let log = bounded_output(
            Command::new(&self.docker).args(["logs", name]),
            self.cli_timeout,
        )
        .ok()
        .flatten()
        .map(|o| {
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            text
        })
        .unwrap_or_default();
        let _ = self.docker_ok(&["rm", "-f", name]);
        log
    }
}

impl DockerRunner {
    /// Runs the CLI with `args`; its trimmed stdout, or an error with its stderr.
    fn docker_ok(&self, args: &[&str]) -> anyhow::Result<String> {
        let out = bounded_output(Command::new(&self.docker).args(args), self.cli_timeout)
            .map_err(|e| anyhow::anyhow!("starting docker: {e}"))?
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "docker {}: no answer within {} s (is the Docker daemon responding?)",
                    args.first().copied().unwrap_or_default(),
                    self.cli_timeout.as_secs_f64()
                )
            })?;
        anyhow::ensure!(
            out.status.success(),
            "docker {}: {}",
            args.first().copied().unwrap_or_default(),
            tail(&String::from_utf8_lossy(&out.stderr), 3)
        );
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
    }

    /// Creates the labelled network `name` (`--internal` when asked) unless it exists; a
    /// concurrent creation by another run is no error. An existing network of that name must be
    /// as asked (internal or not): a bridge with a route out is never used as the fetch network.
    fn ensure_network(&self, name: &str, internal: bool) -> anyhow::Result<()> {
        let inspect = |runner: &DockerRunner| {
            runner.docker_ok(&["network", "inspect", "--format", "{{.Internal}}", name])
        };
        let found = match inspect(self) {
            Ok(found) => found,
            Err(_) => {
                let label = format!("{}=network", crate::egress::LABEL);
                let mut args = vec!["network", "create", "--label", &label];
                if internal {
                    args.push("--internal");
                }
                args.push(name);
                // Another run may have created it in between: inspect again either way.
                let _ = self.docker_ok(&args);
                inspect(self)?
            }
        };
        anyhow::ensure!(
            found == internal.to_string(),
            "network {name} exists with internal={found}; expected internal={internal} \
             (remove it with `rb gc --egress`)"
        );
        Ok(())
    }

    /// The egress networks and proxy containers on this host (`rb gc --egress`); with
    /// `remove`, removes them (proxies first). Returns what was found, as `container <name>` or
    /// `network <name>`.
    pub fn egress_leftovers(&self, remove: bool) -> anyhow::Result<Vec<String>> {
        use crate::egress::{EGRESS_NETWORK, FETCH_NETWORK, LABEL};
        let filter = format!("label={LABEL}=proxy");
        let containers =
            self.docker_ok(&["ps", "-a", "--filter", &filter, "--format", "{{.Names}}"])?;
        let mut found = Vec::new();
        for name in containers.lines().filter(|l| !l.is_empty()) {
            if remove {
                self.docker_ok(&["rm", "-f", name])?;
            }
            found.push(format!("container {name}"));
        }
        for name in [FETCH_NETWORK, EGRESS_NETWORK] {
            if self.docker_ok(&["network", "inspect", name]).is_ok() {
                if remove {
                    self.docker_ok(&["network", "rm", name])?;
                }
                found.push(format!("network {name}"));
            }
        }
        Ok(found)
    }
}

/// The last `lines` lines of `text`.
pub fn tail(text: &str, lines: usize) -> String {
    let all: Vec<&str> = text.lines().collect();
    let start = all.len().saturating_sub(lines);
    all[start..].join("\n")
}

/// A host path for a `-v` mount must be absolute.
pub fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    Ok(std::fs::canonicalize(path)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docker_arguments_isolate_the_index_phase() {
        let run = ContainerRun {
            name: None,
            image: "sourcegraph/scip-typescript@sha256:0000".into(),
            platform: Some("linux/amd64".into()),
            workdir: PathBuf::from("/tmp/checkout"),
            mount_at: "/src".into(),
            cwd: String::new(),
            caches: vec![(PathBuf::from("/tmp/npm-cache"), "/root/.npm".into())],
            readonly: Vec::new(),
            env: vec![("NODE_OPTIONS".into(), "--max-old-space-size=4096".into())],
            network: Network::None,
            cpus: 2.0,
            memory: "4g".into(),
            timeout: Duration::from_secs(60),
            entrypoint: None,
            args: vec!["index".into(), "--output".into(), "index.scip".into()],
        };
        let args = docker_args(&run).join(" ");
        assert_eq!(
            args,
            "run --rm --platform linux/amd64 --network none --cpus 2 --memory 4g -v /tmp/checkout:/src -w /src -v /tmp/npm-cache:/root/.npm -e NODE_OPTIONS=--max-old-space-size=4096 sourcegraph/scip-typescript@sha256:0000 index --output index.scip"
        );
        // Contract M6a D7: a scip-typescript job's export is repository-shaped; the container
        // starts in its root, under the same mount.
        let in_root = ContainerRun {
            cwd: "apps/web".into(),
            ..run.clone()
        };
        assert!(
            docker_args(&in_root)
                .join(" ")
                .contains(" -v /tmp/checkout:/src -w /src/apps/web "),
            "{:?}",
            docker_args(&in_root)
        );
        assert_eq!(in_root.host_cwd(), PathBuf::from("/tmp/checkout/apps/web"));
        assert_eq!(run.host_cwd(), PathBuf::from("/tmp/checkout"));
        // Contract M5 S21: the fetch phase attaches only to the internal fetch network, with
        // no resolver of its own, and its egress client files mounted read-only.
        let fetch = ContainerRun {
            name: Some("rb-scip-0123456789abcdef-fetch".into()),
            network: Network::Fetch,
            platform: None,
            readonly: vec![(PathBuf::from("/tmp/egress"), "/rb-egress".into())],
            entrypoint: Some("npm".into()),
            args: vec!["ci".into()],
            ..run
        };
        let args = docker_args(&fetch).join(" ");
        assert!(
            args.starts_with(
                "run --rm --name rb-scip-0123456789abcdef-fetch --network rb-scip-fetch --dns 127.0.0.1 --cpus"
            ),
            "{args}"
        );
        assert!(args.contains(" -v /tmp/egress:/rb-egress:ro "), "{args}");
        assert!(!args.contains("bridge"), "{args}");
        assert!(!args.contains("--platform"), "{args}");
        assert!(
            args.contains("--entrypoint npm sourcegraph/scip-typescript@sha256:0000 ci"),
            "{args}"
        );
    }

    /// A fake `docker` whose `run` records its arguments and sleeps as the container would, and
    /// whose `kill <name>` ends that run (or, when `kills` is false, does nothing, as when the
    /// container is already gone). Returns the runner and the log the fake appends to.
    fn fake_docker(dir: &Path, kills: bool) -> (DockerRunner, PathBuf) {
        use std::os::unix::fs::PermissionsExt as _;
        let log = dir.join("log");
        let kill = if kills {
            "kill \"$(cat \"$DIR/$2.pid\")\""
        } else {
            ":"
        };
        let script = format!(
            "#!/bin/sh\nDIR={dir}\necho \"$@\" >> \"$DIR/log\"\ncase \"$1\" in\n  run)\n    while [ $# -gt 0 ]; do\n      if [ \"$1\" = --name ]; then echo $$ > \"$DIR/$2.pid\"; fi\n      shift\n    done\n    exec sleep 30 ;;\n  kill) {kill} ;;\nesac\n",
            dir = dir.display()
        );
        let docker = dir.join("docker");
        std::fs::write(&docker, script).unwrap();
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let runner = DockerRunner {
            docker,
            kill_grace: Duration::from_millis(500),
            cli_timeout: CLI_TIMEOUT,
        };
        (runner, log)
    }

    fn timed_run(name: &str, dir: &Path) -> ContainerRun {
        ContainerRun {
            name: Some(name.into()),
            image: "img".into(),
            platform: None,
            workdir: dir.to_path_buf(),
            mount_at: "/src".into(),
            cwd: String::new(),
            caches: Vec::new(),
            readonly: Vec::new(),
            env: Vec::new(),
            network: Network::None,
            cpus: 1.0,
            memory: "1g".into(),
            timeout: Duration::from_secs(1),
            entrypoint: None,
            args: Vec::new(),
        }
    }

    /// Contract M3b amendment 18: a run over its budget is killed by container name, and the
    /// client's exit is awaited; a client that outlives `docker kill` is killed itself after
    /// the grace period. The log's lines are checked for presence, not order: under load the
    /// fake's shell can start after the budget expires, so its `run` line may follow the
    /// `kill` line, and the run still ends promptly.
    #[test]
    fn a_timed_out_run_kills_the_container_by_name_then_the_client() {
        let tmp = tempfile::tempdir().unwrap();
        let (runner, log) = fake_docker(tmp.path(), true);
        let started = std::time::Instant::now();
        let outcome = runner
            .run(&timed_run("rb-scip-abc-index", tmp.path()))
            .unwrap();
        assert!(outcome.timed_out);
        assert!(!outcome.ok());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the run ended with the container, not the sleep: {:?}",
            started.elapsed()
        );
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(
            logged.lines().any(|l| l == "kill rb-scip-abc-index"),
            "{logged}"
        );
        assert!(
            logged
                .lines()
                .any(|l| l.starts_with("run --rm --name rb-scip-abc-index ")),
            "{logged}"
        );

        // `docker kill` did nothing (the container was already gone): the client is killed
        // after the grace period, and the run still ends.
        let tmp = tempfile::tempdir().unwrap();
        let (runner, log) = fake_docker(tmp.path(), false);
        let started = std::time::Instant::now();
        let outcome = runner
            .run(&timed_run("rb-scip-abc-fetch", tmp.path()))
            .unwrap();
        assert!(outcome.timed_out);
        let elapsed = started.elapsed();
        assert!(
            (Duration::from_millis(1500)..Duration::from_secs(5)).contains(&elapsed),
            "budget plus grace, not the sleep: {elapsed:?}"
        );
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(
            logged.lines().any(|l| l == "kill rb-scip-abc-fetch"),
            "{logged}"
        );
    }

    /// Contract M7b S2 (stress F2): a Docker CLI that never answers (a wedged daemon) is given
    /// up on within its bound, killed and reaped — no orphan — and the probe says so; the CLI
    /// calls behind `gc --egress` are bounded the same way.
    #[test]
    fn a_hung_docker_cli_is_bounded_killed_and_reaped() {
        use std::os::unix::fs::PermissionsExt as _;
        let tmp = tempfile::tempdir().unwrap();
        let docker = tmp.path().join("docker");
        let pids = tmp.path().join("pids");
        std::fs::write(
            &docker,
            format!("#!/bin/sh\necho $$ >> {}\nexec sleep 30\n", pids.display()),
        )
        .unwrap();
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
        let started = std::time::Instant::now();
        // Bounds an order of magnitude above a process start (lesson 2026-10-01), far below the
        // fake's 30 s hang.
        let err = docker_available_with(&docker, Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("not responding"), "{err}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        let runner = DockerRunner {
            docker,
            kill_grace: Duration::from_secs(1),
            cli_timeout: Duration::from_secs(1),
        };
        let started = std::time::Instant::now();
        assert!(runner.egress_leftovers(true).is_err());
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        // A fake killed before its shell wrote its pid leaves no line (lesson 2026-10-01: never
        // assume when a spawned process started); every pid written must be gone.
        let pids = std::fs::read_to_string(&pids).unwrap_or_default();
        assert!(pids.lines().count() >= 1, "{pids}");
        for pid in pids.lines() {
            let alive = Command::new("kill")
                .args(["-0", pid])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success();
            assert!(!alive, "docker fake {pid} outlived its call");
        }
    }

    /// A fake `docker` for the egress commands: logs every call; `network inspect` answers
    /// from `$DIR/net-<name>` (absent: fails, as for a missing network) and `network create`
    /// writes that file (`true` with `--internal`); `inspect` prints an address; `logs` prints
    /// a refusal.
    fn fake_egress_docker(dir: &Path) -> (DockerRunner, PathBuf) {
        use std::os::unix::fs::PermissionsExt as _;
        let log = dir.join("log");
        let script = format!(
            "#!/bin/sh\nDIR={dir}\necho \"$@\" >> \"$DIR/log\"\nlast() {{ for a; do :; done; echo \"$a\"; }}\ncase \"$1 $2\" in\n  'network inspect') f=\"$DIR/net-$(last \"$@\")\"; [ -f \"$f\" ] || exit 1; cat \"$f\" ;;\n  'network create') n=$(last \"$@\"); case \"$*\" in *--internal*) echo true ;; *) echo false ;; esac > \"$DIR/net-$n\" ;;\n  'network rm') rm -f \"$DIR/net-$3\" ;;\n  *) case \"$1\" in\n       inspect) echo 172.30.0.2 ;;\n       logs) echo 'NOTICE x [1]: Proxying refused on filtered url \"example.com:443\"' ;;\n       ps) echo rb-egress-left ;;\n     esac ;;\nesac\n",
            dir = dir.display()
        );
        let docker = dir.join("docker");
        std::fs::write(&docker, script).unwrap();
        std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
        (
            DockerRunner {
                docker,
                kill_grace: Duration::from_millis(500),
                cli_timeout: CLI_TIMEOUT,
            },
            log,
        )
    }

    /// Contract M5 S21: starting a proxy creates the fetch network `--internal` and the egress
    /// network when absent (labelled), starts the proxy on the egress network with its
    /// configuration read-only, connects it to the fetch network, waits for it to listen and
    /// returns its address there; stopping it returns its log and removes it. A fetch network
    /// that exists but is not internal is refused before any proxy starts; `rb gc --egress`
    /// lists and removes the proxies and both networks.
    #[test]
    fn egress_proxies_run_on_an_internal_network_only() {
        let tmp = tempfile::tempdir().unwrap();
        let (runner, log) = fake_egress_docker(tmp.path());
        let config = tmp.path().join("proxy");
        std::fs::create_dir_all(&config).unwrap();
        let spec = ProxySpec {
            name: "rb-egress-run1".into(),
            image: crate::egress::PROXY_IMAGE.into(),
            config_dir: config.clone(),
        };
        assert_eq!(runner.start_proxy(&spec).unwrap(), "172.30.0.2");
        assert!(
            runner
                .stop_proxy("rb-egress-run1")
                .contains("example.com:443")
        );
        let calls = std::fs::read_to_string(&log).unwrap();
        let config = absolute(&config).unwrap();
        for want in [
            "network create --label rb.egress=network --internal rb-scip-fetch".to_owned(),
            "network create --label rb.egress=network rb-scip-egress".to_owned(),
            format!(
                "run -d --rm --name rb-egress-run1 --label rb.egress=proxy --network rb-scip-egress --cpus 0.5 --memory 64m -v {}:/etc/tinyproxy:ro rb-egress-proxy:1.11.3",
                config.display()
            ),
            "network connect rb-scip-fetch rb-egress-run1".to_owned(),
            "exec rb-egress-run1 nc -z 127.0.0.1 8888".to_owned(),
            "logs rb-egress-run1".to_owned(),
            "rm -f rb-egress-run1".to_owned(),
        ] {
            assert!(calls.lines().any(|l| l == want), "{want}\n{calls}");
        }
        // A bridged network posing as the fetch network: refused, no proxy started.
        let tmp = tempfile::tempdir().unwrap();
        let (runner, log) = fake_egress_docker(tmp.path());
        std::fs::write(tmp.path().join("net-rb-scip-fetch"), "false\n").unwrap();
        let err = runner.start_proxy(&spec).unwrap_err().to_string();
        assert!(err.contains("internal=false"), "{err}");
        let calls = std::fs::read_to_string(&log).unwrap();
        assert!(!calls.lines().any(|l| l.starts_with("run ")), "{calls}");
        // `rb gc --egress`.
        std::fs::write(tmp.path().join("net-rb-scip-egress"), "false\n").unwrap();
        assert_eq!(
            runner.egress_leftovers(false).unwrap(),
            [
                "container rb-egress-left",
                "network rb-scip-fetch",
                "network rb-scip-egress"
            ]
        );
        runner.egress_leftovers(true).unwrap();
        let calls = std::fs::read_to_string(&log).unwrap();
        for want in [
            "rm -f rb-egress-left",
            "network rm rb-scip-fetch",
            "network rm rb-scip-egress",
        ] {
            assert!(calls.lines().any(|l| l == want), "{want}\n{calls}");
        }
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        assert_eq!(tail("a\nb\nc\n", 2), "b\nc");
        assert_eq!(tail("", 2), "");
    }
}
