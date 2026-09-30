//! Running one container (design §5.2): the command line is built here and executed by a
//! [`Runner`], so orchestration is unit-tested against a fake and only the e2e tests need Docker.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// What a container may reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Network {
    /// The fetch phase: package registries (the host network; registry allow-listing is M5).
    Registries,
    /// The index phase: nothing.
    None,
}

/// One container invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerRun {
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
}

/// The `docker run` arguments for `run`: no credentials, no host toolchains, the network
/// disabled unless the phase fetches, a CPU and memory budget, and the work directory mounted.
pub fn docker_args(run: &ContainerRun) -> Vec<String> {
    let mut a: Vec<String> = vec!["run".into(), "--rm".into()];
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
pub struct DockerRunner;

impl Runner for DockerRunner {
    fn run(&self, run: &ContainerRun) -> anyhow::Result<Outcome> {
        let started = std::time::Instant::now();
        let mut child = Command::new("docker")
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
            network: Network::Registries,
            platform: None,
            entrypoint: Some("npm".into()),
            args: vec!["ci".into()],
            ..run
        };
        let args = docker_args(&fetch).join(" ");
        assert!(args.contains("--network bridge"), "{args}");
        assert!(!args.contains("--platform"), "{args}");
        assert!(
            args.contains("--entrypoint npm sourcegraph/scip-typescript@sha256:0000 ci"),
            "{args}"
        );
    }

    #[test]
    fn tail_keeps_the_last_lines() {
        assert_eq!(tail("a\nb\nc\n", 2), "b\nc");
        assert_eq!(tail("", 2), "");
    }
}
