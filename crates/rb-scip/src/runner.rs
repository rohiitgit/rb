//! Running one container (design §5.2): the command line is built here and executed by a
//! [`Runner`], so orchestration is unit-tested against a fake and only the e2e tests need Docker.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// What a container may reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// The fetch phase: Docker's default bridge network, unrestricted (registry allow-listing
    /// is M5; contract M3b amendment 27).
    Registries,
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
    /// Host directory mounted read-write at `mount_at`, and the container's working directory.
    pub workdir: PathBuf,
    pub mount_at: String,
    /// Extra mounts `(host, container)`: dependency caches shared between the two phases.
    pub caches: Vec<(PathBuf, String)>,
    pub env: Vec<(String, String)>,
    pub network: Network,
    pub cpus: f32,
    pub memory: String,
    pub timeout: Duration,
    /// Command and arguments; `None` uses the image's entrypoint and passes only `args`.
    pub entrypoint: Option<String>,
    pub args: Vec<String>,
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
    a.extend([
        "--network".into(),
        match run.network {
            Network::Registries => "bridge".into(),
            Network::None => "none".into(),
        },
        "--cpus".into(),
        format!("{}", run.cpus),
        "--memory".into(),
        run.memory.clone(),
        "-v".into(),
        format!("{}:{}", run.workdir.display(), run.mount_at),
        "-w".into(),
        run.mount_at.clone(),
    ]);
    for (host, inside) in &run.caches {
        a.extend(["-v".into(), format!("{}:{}", host.display(), inside)]);
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
}

impl Default for DockerRunner {
    fn default() -> Self {
        DockerRunner {
            docker: PathBuf::from("docker"),
            kill_grace: Duration::from_secs(10),
        }
    }
}

/// Whether `docker version` succeeds (the CLI is installed and a daemon answers); the error is
/// the user-facing reason when it does not.
pub fn docker_available() -> Result<(), String> {
    let out = Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(format!(
            "docker is not available: {}",
            tail(&String::from_utf8_lossy(&o.stderr), 1)
        )),
        Err(e) => Err(format!("docker is not available: {e}")),
    }
}

impl DockerRunner {
    /// `docker kill <name>`: stops the container a timed-out client was attached to, since
    /// killing the client alone leaves the container running (contract M3b amendment 18).
    /// Errors (the container already gone) are ignored.
    fn kill_container(&self, name: &str) {
        let _ = Command::new(&self.docker)
            .args(["kill", name])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
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
        let out = Command::new(&self.docker)
            .args(["image", "inspect", "--format", "{{.Id}}", image])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        let id = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        (out.status.success() && id.starts_with("sha256:")).then_some(id)
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
            caches: vec![(PathBuf::from("/tmp/npm-cache"), "/root/.npm".into())],
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
        let fetch = ContainerRun {
            name: Some("rb-scip-0123456789abcdef-fetch".into()),
            network: Network::Registries,
            platform: None,
            entrypoint: Some("npm".into()),
            args: vec!["ci".into()],
            ..run
        };
        let args = docker_args(&fetch).join(" ");
        assert!(
            args.starts_with("run --rm --name rb-scip-0123456789abcdef-fetch --network bridge"),
            "{args}"
        );
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
            caches: Vec::new(),
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

    #[test]
    fn tail_keeps_the_last_lines() {
        assert_eq!(tail("a\nb\nc\n", 2), "b\nc");
        assert_eq!(tail("", 2), "");
    }
}
