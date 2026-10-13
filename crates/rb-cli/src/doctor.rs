//! `rb doctor` (contract M7h S5, stress U7): one row per thing `rb` needs from its environment,
//! each `ok`, `warn` (a feature degrades: SCIP, disk headroom) or `fail` (indexing or every
//! question fails), with the fix. Rows that need an unreachable database are `skip`. Exits 1
//! when any row fails. Every probe is bounded: Docker by its probe timeout (M7b).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use rb_scip::Runner as _;
use rb_store::{LoadDir, Store};
use serde_json::json;

/// Free space below which the SCIP work dir's volume warns (stress-campaign disk floor).
const DISK_FLOOR: u64 = 6 << 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Ok,
    Warn,
    Fail,
    Skip,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::Ok => "ok",
            Status::Warn => "warn",
            Status::Fail => "fail",
            Status::Skip => "skip",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Row {
    pub name: &'static str,
    pub status: Status,
    pub detail: String,
    pub fix: Option<String>,
}

fn row(name: &'static str, status: Status, detail: impl Into<String>) -> Row {
    Row {
        name,
        status,
        detail: detail.into(),
        fix: None,
    }
}

impl Row {
    fn fix(mut self, fix: impl Into<String>) -> Row {
        self.fix = Some(fix.into());
        self
    }
}

/// What the checks look at: the global flags and the SCIP work dir `rb index` would use.
pub(crate) struct Inputs<'a> {
    pub database_url: &'a str,
    pub age_load_dir: &'a Path,
    pub scip_work_dir: &'a Path,
    pub home: Option<PathBuf>,
}

/// How long the database checks may take together, as the Docker probe (M7b): a server that
/// accepts a connection and never answers must not hang `rb doctor` (contract M7j E2).
const DB_PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Runs every check, in order.
pub(crate) fn diagnose(inputs: &Inputs<'_>) -> Vec<Row> {
    let load_dir = LoadDir::new(inputs.age_load_dir);
    let dir_ok = inputs.age_load_dir.is_dir();
    let (tx, rx) = std::sync::mpsc::channel();
    let (url, dir) = (inputs.database_url.to_owned(), load_dir.clone());
    // A probe still blocked at the timeout is left behind; the process exits after the report.
    std::thread::spawn(move || {
        let _ = tx.send(database_rows(&url, &dir, dir_ok));
    });
    let mut rows = rx
        .recv_timeout(DB_PROBE_TIMEOUT)
        .unwrap_or_else(|_| silent_database(inputs.database_url));
    // The database rows end with the mount check, which reads after the load-dir row.
    let mount = rows.pop().expect("the database rows end with the mount");
    rows.push(load_dir_row(inputs.age_load_dir, dir_ok));
    rows.push(mount);
    let docker = rb_scip::docker_available();
    rows.push(docker_row(&docker, std::env::var("DOCKER_CONTEXT").ok()));
    rows.push(match docker {
        Ok(()) => {
            let runner = rb_scip::DockerRunner::default();
            let absent: Vec<&str> = IMAGES
                .iter()
                .copied()
                .filter(|image| runner.image_id(image).is_none())
                .collect();
            images_row(&absent)
        }
        Err(_) => row("scip images", Status::Skip, "needs Docker"),
    });
    let work_dir = absolute(inputs.scip_work_dir);
    rows.push(work_dir_row(&work_dir, inputs.home.as_deref()));
    rows.push(disk_row(&work_dir, free_bytes(&work_dir)));
    rows
}

/// The database, Postgres, AGE, migrations and age-load mount rows, in that order.
fn database_rows(url: &str, load_dir: &LoadDir, dir_ok: bool) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut store = match Store::connect(url) {
        Ok(store) => {
            rows.push(row("database", Status::Ok, rb_store::redact(url)));
            store
        }
        Err(e) => {
            rows.push(
                row("database", Status::Fail, format!("{e:#}"))
                    .fix("start it: `docker compose up -d --wait` (with RB_DB_PORT matching the URL), or set RB_DATABASE_URL"),
            );
            rows.extend(needs_the_database());
            return rows;
        }
    };
    match store.server_versions() {
        Ok((postgres, age)) => {
            rows.push(row("postgres", Status::Ok, postgres));
            rows.push(age_row(age));
        }
        Err(e) => {
            rows.push(
                row("postgres", Status::Fail, format!("{e:#}")).fix(
                    "point RB_DATABASE_URL at the compose database (Postgres 18 + Apache AGE)",
                ),
            );
            rows.push(row("age", Status::Skip, "needs the server's answer"));
        }
    }
    rows.push(match store.pending_migrations() {
        Ok(pending) => migrations_row(&pending),
        Err(e) => row("migrations", Status::Fail, format!("{e:#}")).fix("`rb migrate`"),
    });
    rows.push(if dir_ok {
        mount_row(mount_probe(&mut store, load_dir))
    } else {
        row("age-load mount", Status::Skip, "needs the age-load dir")
    });
    rows
}

/// `skip` rows for the checks after an unreachable database.
fn needs_the_database() -> Vec<Row> {
    ["postgres", "age", "migrations", "age-load mount"]
        .into_iter()
        .map(|name| row(name, Status::Skip, "needs the database"))
        .collect()
}

/// The database rows when the server took the connection but answered nothing in time.
fn silent_database(url: &str) -> Vec<Row> {
    let mut rows =
        vec![row(
        "database",
        Status::Fail,
        format!(
            "{} answered nothing within {} s",
            rb_store::redact(url),
            DB_PROBE_TIMEOUT.as_secs()
        ),
    )
    .fix("restart it (`docker compose restart`), and check that RB_DATABASE_URL names Postgres")];
    rows.extend(needs_the_database());
    rows
}

/// The pinned images SCIP runs (README, "SCIP images").
const IMAGES: [&str; 5] = [
    rb_scip::jobs::SCIP_JAVA_IMAGE,
    rb_scip::jobs::SCIP_TYPESCRIPT_IMAGE,
    rb_scip::jobs::SCIP_GO_IMAGE,
    rb_scip::jobs::SCIP_PYTHON_IMAGE,
    rb_scip::egress::PROXY_IMAGE,
];

fn age_row(age: Option<String>) -> Row {
    match age {
        Some(version) => row("age", Status::Ok, version),
        None => row("age", Status::Fail, "the AGE extension is not installed")
            .fix("use the compose database image (Postgres 18 + Apache AGE)"),
    }
}

fn migrations_row(pending: &[&str]) -> Row {
    if pending.is_empty() {
        return row("migrations", Status::Ok, "up to date");
    }
    row(
        "migrations",
        Status::Warn,
        format!("{} pending: {}", pending.len(), pending.join(", ")),
    )
    .fix("`rb migrate` (`rb index` also applies them)")
}

fn load_dir_row(dir: &Path, exists: bool) -> Row {
    let shown = dir.display().to_string();
    if !exists {
        return row("age-load dir", Status::Fail, format!("{shown} does not exist")).fix(
            "run from the project root (compose mounts ./var/age-load) or set RB_AGE_LOAD_DIR to that directory's absolute path",
        );
    }
    if dir.is_relative() {
        return row(
            "age-load dir",
            Status::Warn,
            format!("{shown} is relative: indexing works only from this directory"),
        )
        .fix("set RB_AGE_LOAD_DIR to its absolute path");
    }
    row("age-load dir", Status::Ok, shown)
}

/// Writes a probe file into the load dir and asks the server whether it sees it there.
fn mount_probe(store: &mut Store, dir: &LoadDir) -> Result<bool, String> {
    let name = format!("rb-doctor-{}.probe", std::process::id());
    let path = dir.host.join(&name);
    std::fs::write(&path, b"rb doctor\n")
        .map_err(|e| format!("writing {}: {e}", path.display()))?;
    let seen = store
        .server_sees_load_file(dir, &name)
        .map_err(|e| format!("{e:#}"));
    let _ = std::fs::remove_file(&path);
    seen
}

fn mount_row(seen: Result<bool, String>) -> Row {
    match seen {
        Ok(true) => row(
            "age-load mount",
            Status::Ok,
            "the database reads this directory at /tmp/age/rb-load",
        ),
        Ok(false) => row(
            "age-load mount",
            Status::Fail,
            "the database does not see this directory at /tmp/age/rb-load",
        )
        .fix("point RB_AGE_LOAD_DIR at the directory compose mounts (./var/age-load of the checkout that started it)"),
        Err(e) => row("age-load mount", Status::Warn, format!("not verified: {e}")).fix(
            "the check reads the file through `pg_stat_file` (superuser or pg_read_server_files); a first `rb index` also proves the mount",
        ),
    }
}

fn docker_row(docker: &Result<(), String>, context: Option<String>) -> Row {
    let context = context.map_or_else(
        || "docker's current context".to_owned(),
        |c| format!("DOCKER_CONTEXT={c}"),
    );
    match docker {
        Ok(()) => row("docker", Status::Ok, format!("the daemon answers ({context})")),
        Err(reason) => row("docker", Status::Warn, format!("{reason} ({context})"))
            .fix("start Docker (or set DOCKER_CONTEXT); without it, SCIP is skipped and every edge stays heuristic (`--no-scip` works)"),
    }
}

fn images_row(absent: &[&str]) -> Row {
    if absent.is_empty() {
        return row(
            "scip images",
            Status::Ok,
            format!("{} pinned images present", IMAGES.len()),
        );
    }
    row(
        "scip images",
        Status::Warn,
        format!("absent: {}", absent.join(", ")),
    )
    .fix("build or pull them (README, \"SCIP images\"); projects needing an absent one stay heuristic")
}

fn work_dir_row(dir: &Path, home: Option<&Path>) -> Row {
    let shown = dir.display().to_string();
    match home {
        Some(home) if !dir.starts_with(home) => row(
            "scip work dir",
            Status::Warn,
            format!("{shown} is outside {}", home.display()),
        )
        .fix("Docker Desktop shares only the home directory: set RB_SCIP_WORK_DIR under it"),
        _ => row("scip work dir", Status::Ok, shown),
    }
}

fn disk_row(dir: &Path, free: Option<u64>) -> Row {
    let gib = |b: u64| format!("{:.1} GiB", b as f64 / f64::from(1u32 << 30));
    match free {
        Some(b) if b < DISK_FLOOR => row(
            "disk",
            Status::Warn,
            format!(
                "{} free under {} (below {})",
                gib(b),
                dir.display(),
                gib(DISK_FLOOR)
            ),
        )
        .fix("free space: SCIP caches and AGE loads need several GiB on large repositories"),
        Some(b) => row(
            "disk",
            Status::Ok,
            format!("{} free under {}", gib(b), dir.display()),
        ),
        None => row(
            "disk",
            Status::Warn,
            format!("free space under {} unknown", dir.display()),
        )
        .fix("check it by hand (`df -h`)"),
    }
}

/// `dir` made absolute against the working directory (not resolved: it may not exist yet).
fn absolute(dir: &Path) -> PathBuf {
    std::env::current_dir()
        .map(|cwd| cwd.join(dir))
        .unwrap_or_else(|_| dir.to_path_buf())
}

/// Bytes free on the volume holding `dir` (or its nearest existing ancestor), from `df -Pk`.
fn free_bytes(dir: &Path) -> Option<u64> {
    let dir = absolute(dir);
    let existing = dir.ancestors().find(|d| d.exists())?;
    let out = Command::new("df").arg("-Pk").arg(existing).output().ok()?;
    parse_df(&String::from_utf8_lossy(&out.stdout))
}

/// The available KiB column of `df -Pk`'s one data row, in bytes.
fn parse_df(text: &str) -> Option<u64> {
    let kib: u64 = text
        .lines()
        .nth(1)?
        .split_whitespace()
        .nth(3)?
        .parse()
        .ok()?;
    Some(kib * 1024)
}

pub(crate) fn render_text(rows: &[Row]) -> String {
    let mut out = String::new();
    for r in rows {
        out.push_str(&format!(
            "{:<5} {:<15} {}\n",
            r.status.as_str(),
            r.name,
            r.detail
        ));
        if let Some(fix) = &r.fix {
            out.push_str(&format!("{:<21} fix: {fix}\n", ""));
        }
    }
    let count = |s: Status| rows.iter().filter(|r| r.status == s).count();
    out.push_str(&format!(
        "doctor: {} failed, {} warnings\n",
        count(Status::Fail),
        count(Status::Warn)
    ));
    out
}

pub(crate) fn render_json(rows: &[Row]) -> String {
    let checks: Vec<_> = rows
        .iter()
        .map(|r| {
            json!({
                "name": r.name,
                "status": r.status.as_str(),
                "detail": r.detail,
                "fix": r.fix,
            })
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&json!({ "checks": checks })).expect("json");
    text.push('\n');
    text
}

/// 1 when any row failed, else 0 (warnings and skips do not fail).
pub(crate) fn exit_code(rows: &[Row]) -> ExitCode {
    if rows.iter().any(|r| r.status == Status::Fail) {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contract M7h S5: each row's status follows its facts, with a fix whenever it is not ok.
    #[test]
    fn rows_follow_their_facts() {
        assert_eq!(migrations_row(&[]).status, Status::Ok);
        let pending = migrations_row(&["0007_repo_checkout"]);
        assert_eq!(
            (pending.status, pending.detail.as_str()),
            (Status::Warn, "1 pending: 0007_repo_checkout")
        );
        assert!(pending.fix.unwrap().contains("rb migrate"));

        assert_eq!(age_row(Some("1.6.0".into())).status, Status::Ok);
        assert_eq!(age_row(None).status, Status::Fail);

        assert_eq!(
            load_dir_row(Path::new("/abs/load"), true).status,
            Status::Ok
        );
        assert_eq!(
            load_dir_row(Path::new("var/age-load"), true).status,
            Status::Warn
        );
        assert_eq!(
            load_dir_row(Path::new("/abs/load"), false).status,
            Status::Fail
        );

        assert_eq!(mount_row(Ok(true)).status, Status::Ok);
        assert_eq!(mount_row(Ok(false)).status, Status::Fail);
        assert_eq!(mount_row(Err("denied".into())).status, Status::Warn);

        let up = docker_row(&Ok(()), Some("desktop-linux".into()));
        assert_eq!(
            (up.status, up.detail.as_str()),
            (
                Status::Ok,
                "the daemon answers (DOCKER_CONTEXT=desktop-linux)"
            )
        );
        assert_eq!(
            docker_row(&Err("not responding".into()), None).status,
            Status::Warn
        );

        assert_eq!(images_row(&[]).status, Status::Ok);
        let absent = images_row(&["rb-scip-go:0.2.7"]);
        assert_eq!(
            (absent.status, absent.detail.as_str()),
            (Status::Warn, "absent: rb-scip-go:0.2.7")
        );

        let home = Path::new("/Users/me");
        assert_eq!(
            work_dir_row(Path::new("/Users/me/x/var/scip"), Some(home)).status,
            Status::Ok
        );
        assert_eq!(
            work_dir_row(Path::new("/tmp/scip"), Some(home)).status,
            Status::Warn
        );
        assert_eq!(
            work_dir_row(Path::new("/tmp/scip"), None).status,
            Status::Ok
        );

        let dir = Path::new("/d");
        assert_eq!(disk_row(dir, Some(DISK_FLOOR)).status, Status::Ok);
        assert_eq!(disk_row(dir, Some(DISK_FLOOR - 1)).status, Status::Warn);
        assert_eq!(disk_row(dir, None).status, Status::Warn);
    }

    /// Contract M7j E2: a silent server is a failed database row with a fix and the rows that
    /// need it skipped, in the usual order (the mount last).
    #[test]
    fn a_silent_database_fails_with_a_fix() {
        let rows = silent_database("postgresql://rb:pw@h:1/rb");
        let names: Vec<&str> = rows.iter().map(|r| r.name).collect();
        assert_eq!(
            names,
            [
                "database",
                "postgres",
                "age",
                "migrations",
                "age-load mount"
            ]
        );
        assert_eq!(rows[0].status, Status::Fail);
        assert!(
            rows[0]
                .detail
                .starts_with("postgresql://rb:***@h:1/rb answered nothing")
        );
        assert!(rows[0].fix.is_some());
        assert!(rows[1..].iter().all(|r| r.status == Status::Skip));
    }

    /// Contract M7j E2 (evaluator item 10): every `warn` and `fail` row the builders make has a
    /// fix; `skip` rows name what they wait for.
    #[test]
    fn every_warn_and_fail_row_has_a_fix() {
        let dir = Path::new("/d");
        let rows = [
            migrations_row(&["0007"]),
            age_row(None),
            load_dir_row(Path::new("var/x"), true),
            load_dir_row(dir, false),
            mount_row(Ok(false)),
            mount_row(Err("denied".into())),
            docker_row(&Err("down".into()), None),
            images_row(&["x"]),
            work_dir_row(Path::new("/tmp/x"), Some(Path::new("/Users/me"))),
            disk_row(dir, Some(1)),
            disk_row(dir, None),
        ];
        for r in rows.iter().chain(&silent_database("postgresql://h/rb")) {
            match r.status {
                Status::Warn | Status::Fail => assert!(r.fix.is_some(), "{r:?}"),
                Status::Skip => assert!(r.detail.starts_with("needs "), "{r:?}"),
                Status::Ok => {}
            }
        }
    }

    /// `df -Pk`'s available column, in bytes; anything else is unknown.
    #[test]
    fn df_output_gives_free_bytes() {
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                  /dev/disk3s5 971350180 512 7340032 1% /System/Volumes/Data\n";
        assert_eq!(parse_df(df), Some(7_516_192_768));
        assert_eq!(parse_df("Filesystem\n"), None);
    }

    /// Text names each row and its fix, then a summary; only a failed row fails the exit.
    #[test]
    fn rendering_and_exit_code() {
        let rows = vec![
            row("database", Status::Ok, "postgresql://rb:***@h:1/rb"),
            row("docker", Status::Warn, "not responding").fix("start Docker"),
        ];
        assert_eq!(
            render_text(&rows),
            "ok    database        postgresql://rb:***@h:1/rb\n\
             warn  docker          not responding\n\
             \x20                     fix: start Docker\n\
             doctor: 0 failed, 1 warnings\n"
        );
        assert_eq!(exit_code(&rows), ExitCode::SUCCESS);
        let failed = vec![row("age", Status::Fail, "missing")];
        assert_eq!(exit_code(&failed), ExitCode::FAILURE);
        let json: serde_json::Value = serde_json::from_str(&render_json(&rows)).unwrap();
        assert_eq!(json["checks"][1]["status"], "warn");
        assert_eq!(json["checks"][1]["fix"], "start Docker");
        assert_eq!(json["checks"][0]["fix"], serde_json::Value::Null);
    }
}
