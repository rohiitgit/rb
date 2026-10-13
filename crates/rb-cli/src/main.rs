//! `rb` — Repository Brain CLI (design §11.1).

use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use anyhow::{Context, ensure};

mod code;
mod path;
use clap::{Args, Parser, Subcommand, ValueEnum};
use rb_api::{Answer, ApiError, Endpoint, Params};
use rb_model::{ServiceRole, canonical_dump};
use rb_store::{LoadDir, ServiceCard, SnapshotInfo, Store, TableAccess, TableDeps};
use serde_json::{Value, json};

#[derive(Parser)]
#[command(
    name = "rb",
    version,
    about = "Repository Brain: deterministic code-intelligence graph"
)]
struct Cli {
    /// Postgres (with Apache AGE) connection URL.
    #[arg(
        long,
        global = true,
        env = "RB_DATABASE_URL",
        default_value = "postgresql://rb:rb@127.0.0.1:55432/rb"
    )]
    database_url: String,

    /// Host directory mounted at /tmp/age/rb-load in the database container (see compose.yaml).
    #[arg(
        long,
        global = true,
        env = "RB_AGE_LOAD_DIR",
        default_value = "var/age-load"
    )]
    age_load_dir: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct IndexArgs {
    /// Path to the repository (its root, containing .git).
    path: PathBuf,
    /// Repository name in the Brain ([a-z0-9_], max 40). Defaults to the directory name.
    #[arg(long)]
    repo: Option<String>,
    /// Revision to index.
    #[arg(long, default_value = "HEAD")]
    rev: String,
    /// Rebuild even if the live snapshot already covers this commit with the same tools.
    #[arg(long)]
    force: bool,
    /// Parse every file instead of reusing cached syntax facts (the facts it extracts are still
    /// written to that cache, so the next plain run reuses them), and run every SCIP job again
    /// instead of serving it from the index cache (its outcome is stored, so a cached `failed`
    /// can be retried).
    #[arg(long)]
    no_cache: bool,
    /// Run no SCIP indexer: every project is resolved heuristically and its job is `skipped`.
    #[arg(long)]
    no_scip: bool,
    /// Directory for indexer exports and package caches; the container runtime must be able to
    /// bind-mount it, so it must lie under the home directory (the path Docker Desktop's VM
    /// shares; see README, "Container runtime").
    #[arg(long, env = "RB_SCIP_WORK_DIR", default_value = "var/scip")]
    scip_work_dir: PathBuf,
    /// Seconds a SCIP job's index phase may take (its fetch phase gets the same budget); a run
    /// over it is killed and recorded as `timeout`. Default: 600 for the fetch, 900 for the index.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    scip_timeout_secs: Option<u64>,
    /// CPUs each SCIP container may use (0.5-64; default 2). Set on the indexing host, never
    /// read from the repository; not part of the index's cache key.
    #[arg(long, env = "RB_SCIP_CPUS", value_parser = parse_scip_cpus)]
    scip_cpus: Option<f32>,
    /// Memory each SCIP container may use, as `<n>m` or `<n>g` (512m-64g; default 4g). Set on
    /// the indexing host only; not part of the index's cache key.
    #[arg(long, env = "RB_SCIP_MEMORY", value_parser = parse_scip_memory)]
    scip_memory: Option<String>,
    /// Maven reactor threads (`-T`) for scip-java's builds (1-16; default 1, which adds no
    /// flag). Above 1 it is part of the index's cache key, so no cached index crosses thread
    /// settings. Set on the indexing host only.
    #[arg(
        long,
        env = "RB_SCIP_MAVEN_THREADS",
        value_parser = clap::value_parser!(u8).range(1..=16)
    )]
    scip_maven_threads: Option<u8>,
    /// Extra registry hosts the SCIP fetch phase may reach through its egress proxy, besides
    /// Maven Central and the npm and Yarn registries (comma-separated DNS names, HTTPS only).
    /// Set on the indexing host, never read from the repository: the repository's own build
    /// runs in the fetch phase, so it must not widen what that phase can reach.
    #[arg(long, env = "RB_SCIP_REGISTRIES", value_delimiter = ',')]
    scip_registries: Vec<String>,
    /// Retired snapshots kept per repository after this run, besides the live one (0-20); older
    /// ones and every failed run's row are deleted, and only the live snapshot keeps an AGE graph.
    #[arg(
        long,
        env = "RB_KEEP_RETIRED",
        default_value_t = rb_store::KEEP_RETIRED,
        value_parser = clap::value_parser!(i64).range(0..=rb_store::MAX_KEEP_RETIRED)
    )]
    keep: i64,
    /// Print timings: the AGE projection's load per label file (rows, CSV bytes, time). Wall
    /// times are printed only, never stored.
    #[arg(long)]
    profile: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Apply pending schema migrations.
    Migrate,
    /// Index a commit of a git repository into a new live snapshot.
    Index(IndexArgs),
    /// Show the live snapshot and counts per repository.
    Status {
        #[arg(long)]
        repo: Option<String>,
        /// Emit JSON (the API's envelope) instead of text.
        #[arg(long)]
        json: bool,
    },
    /// List a repository's kept snapshots (live, retired, failed), newest first.
    Snapshots {
        #[arg(long)]
        repo: Option<String>,
        /// Rows listed (1-1000; default 1000).
        #[arg(long)]
        limit: Option<String>,
        /// Rows skipped first (default 0): `--offset` and `--limit` page through the list.
        #[arg(long)]
        offset: Option<String>,
        /// Emit JSON (the API's envelope) instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Print the canonical, id-independent dump of a snapshot.
    Dump {
        #[arg(long)]
        repo: Option<String>,
        /// Snapshot id (default: the repo's live snapshot).
        #[arg(long)]
        snapshot: Option<i64>,
        /// Write to a file instead of stdout.
        #[arg(long, short)]
        output: Option<PathBuf>,
    },
    /// Search nodes by key or name (exact matches first, then ranked substring matches).
    Find {
        text: String,
        /// Restrict to one node kind (symbol, file, service, table, owner, endpoint, package).
        #[arg(long)]
        kind: Option<String>,
        /// Matches listed (1-1000; default 20).
        #[arg(long)]
        limit: Option<String>,
        /// Matches skipped first (default 0): `--offset` and `--limit` page through them.
        #[arg(long)]
        offset: Option<String>,
        #[arg(long)]
        repo: Option<String>,
        /// Answer from this kept snapshot (`rb snapshots` lists them) instead of the live one.
        #[arg(long)]
        snapshot: Option<String>,
        /// Emit JSON (the API's envelope) instead of text.
        #[arg(long)]
        json: bool,
    },
    /// List services, or show one service's card (by directory or name).
    Service {
        /// Service directory (e.g. `services/auth`, `.` for the repo root) or name.
        service: Option<String>,
        #[arg(long)]
        repo: Option<String>,
        /// Services listed (without a service) (1-1000; default 1000).
        #[arg(long)]
        limit: Option<String>,
        /// Services skipped first (default 0): `--offset` and `--limit` page through the list.
        #[arg(long)]
        offset: Option<String>,
        /// Answer from this kept snapshot (`rb snapshots` lists them) instead of the live one.
        #[arg(long)]
        snapshot: Option<String>,
        /// Emit JSON (the API's envelope) instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Show which migrations and code touch a table (by `schema.table` or bare name).
    TableDeps {
        table: String,
        #[arg(long)]
        repo: Option<String>,
        /// Answer from this kept snapshot (`rb snapshots` lists them) instead of the live one.
        #[arg(long)]
        snapshot: Option<String>,
        /// Emit JSON (the API's envelope) instead of text.
        #[arg(long)]
        json: bool,
        /// Dispatch expansion in the caller walk (as for `callers`); `cha` by default, since
        /// under-reporting a table's dependents is the dangerous failure.
        #[arg(long)]
        precision: Option<String>,
        /// API hops followed through `CONSUMES` to services reaching the table only through
        /// another service's endpoint (0-8; default 4).
        #[arg(long)]
        hops: Option<String>,
    },
    /// Show what calls a function or method (Q1), as a caller tree.
    Callers {
        /// A symbol key, `path:line`, a key suffix (`AuthService#login().`), `Class.method` or a
        /// name.
        target: String,
        #[arg(long)]
        repo: Option<String>,
        /// Answer from this kept snapshot (`rb snapshots` lists them) instead of the live one.
        #[arg(long)]
        snapshot: Option<String>,
        /// Emit JSON (the API's envelope) instead of text.
        #[arg(long)]
        json: bool,
        /// Levels of callers to follow (1-25; default 1).
        #[arg(long)]
        depth: Option<String>,
        /// Also count function values passed around (`REFERENCES`).
        #[arg(long)]
        include_references: bool,
        /// Ignore edges below this certainty (ambiguous, the default; heuristic, synthetic,
        /// dispatch, exact).
        #[arg(long)]
        min_certainty: Option<String>,
        /// Choose among several matching symbols (1-based, as listed).
        #[arg(long)]
        pick: Option<String>,
        /// Dispatch expansion: `cha` (the default) follows every overrider; `rta` (opt-in)
        /// skips, and counts, overriders whose type is proven never instantiated, and may hide
        /// callers reached through forms the scan cannot see.
        #[arg(long)]
        precision: Option<String>,
    },
    /// Show what breaks if a symbol or table is removed (Q2).
    Impact {
        /// As for `callers`; a table key (`public.users`) also works.
        target: String,
        #[arg(long)]
        repo: Option<String>,
        /// Answer from this kept snapshot (`rb snapshots` lists them) instead of the live one.
        #[arg(long)]
        snapshot: Option<String>,
        /// As for `callers`.
        #[arg(long)]
        min_certainty: Option<String>,
        #[arg(long)]
        pick: Option<String>,
        /// Dispatch expansion (as for `callers`); `cha` by default, since under-reporting is
        /// the dangerous failure here.
        #[arg(long)]
        precision: Option<String>,
        /// Emit JSON (the API's envelope) instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Show the request path through an endpoint (Q4): the client calls reaching it across
    /// services, and what its handler reaches (tables, other services' endpoints).
    Path {
        /// An endpoint key, `<METHOD> <path>`, part of an endpoint name, or a handler or client
        /// symbol (as for `callers`).
        query: String,
        #[arg(long)]
        repo: Option<String>,
        /// Answer from this kept snapshot (`rb snapshots` lists them) instead of the live one.
        #[arg(long)]
        snapshot: Option<String>,
        /// Call levels followed per service hop (1-25; default 12).
        #[arg(long)]
        depth: Option<String>,
        /// Service hops followed through `CONSUMES` (0-8; default 4).
        #[arg(long)]
        hops: Option<String>,
        /// Show helper calls that reach no table or endpoint instead of collapsing them.
        #[arg(long)]
        full: bool,
        #[arg(long, value_enum, default_value_t = Format::Text)]
        format: Format,
        /// The same as `--format json`, as on every other question (contract M7f S7).
        #[arg(long)]
        json: bool,
        /// Choose among several matching endpoints (1-based, as listed).
        #[arg(long)]
        pick: Option<String>,
        /// Dispatch expansion, both ways (as for `callers`; `cha` by default).
        #[arg(long)]
        precision: Option<String>,
    },
    /// Compare two kept snapshots (`rb snapshots` lists them) by natural identity: endpoints,
    /// table access and service dependencies added and removed, certainty shifts, then the
    /// nodes and edges added, removed and changed (text lists the first 50 rows of each section;
    /// `--json` pages them with `--limit`/`--offset`). Never re-indexes; names the tool versions
    /// that differ.
    Diff {
        /// A snapshot id, or a commit prefix (4-40 hex digits) unique among the kept snapshots;
        /// default: the parent of `to` while it is kept.
        from: Option<String>,
        /// As `from`; default: the live snapshot.
        to: Option<String>,
        #[arg(long)]
        repo: Option<String>,
        /// Emit JSON (the API's envelope) instead of text.
        #[arg(long)]
        json: bool,
        /// List the diagnostics added and removed, not only the counts per kind.
        #[arg(long)]
        diagnostics: bool,
        /// List the edges whose only change is their evidence (text; JSON always lists them).
        #[arg(long)]
        evidence: bool,
        /// Rows per section and page (1-1000; default 1000; JSON).
        #[arg(long)]
        limit: Option<String>,
        /// Rows skipped first in every section (default 0; JSON).
        #[arg(long)]
        offset: Option<String>,
    },
    /// Serve the REST API (design §11.2) on a loopback address until SIGINT or SIGTERM: the same
    /// answers as `rb … --json`, read-only, at most `--workers` at once and `--queue` waiting.
    Serve {
        /// Loopback address and port (`127.0.0.1:0` picks a free port); any other address is
        /// refused, since the server has no authentication.
        #[arg(long, default_value = rb_server::DEFAULT_BIND)]
        bind: std::net::SocketAddr,
        /// Requests answered at once, each on its own read-only connection (1-64).
        #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u16).range(1..=64))]
        workers: u16,
        /// Requests that may wait for a worker; one more gets `503 busy` (0-1024).
        #[arg(long, default_value_t = 16, value_parser = clap::value_parser!(u16).range(0..=1024))]
        queue: u16,
        /// Milliseconds a request may take, queue included, before `503 deadline_exceeded`.
        #[arg(long, default_value_t = 10_000, value_parser = clap::value_parser!(u64).range(1..=3_600_000))]
        deadline_ms: u64,
        /// The default Cypher statement timeout in milliseconds (a request may set its own,
        /// 1-30000).
        #[arg(
            long,
            env = "RB_CYPHER_TIMEOUT_MS",
            default_value_t = rb_store::CYPHER_TIMEOUT_MS_DEFAULT,
            value_parser = clap::value_parser!(u64).range(1..=rb_store::CYPHER_TIMEOUT_MS_MAX)
        )]
        cypher_timeout_ms: u64,
    },
    /// Remove what interrupted or older runs left behind: AGE graphs named `rb_*` that are not a
    /// live snapshot's, and `building` snapshot rows. Never runs implicitly.
    Gc {
        /// List what would be removed, and remove nothing.
        #[arg(long)]
        dry_run: bool,
        /// Only repositories whose names start with this (graphs named `rb_<prefix>…`).
        #[arg(long, default_value = "")]
        prefix: String,
        /// Also remove the SCIP fetch phase's Docker networks (`rb-scip-fetch`,
        /// `rb-scip-egress`) and any egress proxy container left by an interrupted run.
        #[arg(long)]
        egress: bool,
    },
    /// Manage the caches stored in the database.
    Cache {
        #[command(subcommand)]
        action: CacheAction,
    },
    /// Check the live snapshot against the built-in architecture rules: prints violations
    /// (sorted by rule, source, target) and exits 0 when clean, 1 on violations, 2 on errors.
    Check {
        #[arg(long)]
        repo: Option<String>,
        /// Run only this rule (orphan_endpoints, unowned_table_writes, service_cycles).
        #[arg(long)]
        rule: Option<String>,
        /// Emit JSON instead of text.
        #[arg(long)]
        json: bool,
        /// The Cypher statement timeout in milliseconds for each rule (1-30000).
        #[arg(
            long,
            env = "RB_CYPHER_TIMEOUT_MS",
            default_value_t = rb_store::CYPHER_TIMEOUT_MS_DEFAULT,
            value_parser = clap::value_parser!(u64).range(1..=rb_store::CYPHER_TIMEOUT_MS_MAX)
        )]
        timeout_ms: u64,
        /// Accept the violations listed in this file (as `rb check` prints them: save today's
        /// output to adopt the gate, then only new violations fail).
        #[arg(long)]
        baseline: Option<PathBuf>,
    },
    /// Run a read-only Cypher query against the live snapshot's AGE graph.
    Cypher {
        /// openCypher query, e.g. "MATCH (f:File)-[:IN_SERVICE]->(s:Service) RETURN s.name, count(f)".
        query: String,
        #[arg(long)]
        repo: Option<String>,
        /// Run against this snapshot (only the live one has an AGE graph).
        #[arg(long)]
        snapshot: Option<String>,
        /// Emit JSON (the API's envelope) instead of text.
        #[arg(long)]
        json: bool,
        /// Comma-separated names for the RETURN columns, in order (default `result`).
        #[arg(long)]
        cols: Option<String>,
        /// Statement timeout in milliseconds (1-30000; default 2000); a query over it is
        /// cancelled with an error, never cut short. At most 10,000 rows are printed, then
        /// `truncated: rows`.
        #[arg(long, env = "RB_CYPHER_TIMEOUT_MS")]
        timeout_ms: Option<String>,
    },
}

#[derive(Subcommand)]
enum CacheAction {
    /// Delete syntax-facts rows of other extractor versions (`--facts`) and SCIP index rows of
    /// other indexer images (`--scip`); prints the rows and bytes removed. Never runs
    /// implicitly: an older `rb` sharing the database may still read its rows.
    Prune {
        #[arg(long)]
        facts: bool,
        #[arg(long)]
        scip: bool,
        /// Report what would be removed, and remove nothing.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Format {
    Text,
    Json,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let json = wants_json(&cli.command);
    let check = matches!(cli.command, Command::Check { .. });
    match run_cli(cli) {
        Ok(code) => code,
        Err(e) => report_failure(e, json, check),
    }
}

/// Whether the command asked for JSON output, known before it runs (contract M7f S2).
fn wants_json(command: &Command) -> bool {
    match command {
        Command::Status { json, .. }
        | Command::Snapshots { json, .. }
        | Command::Find { json, .. }
        | Command::Service { json, .. }
        | Command::TableDeps { json, .. }
        | Command::Callers { json, .. }
        | Command::Impact { json, .. }
        | Command::Diff { json, .. }
        | Command::Cypher { json, .. }
        | Command::Check { json, .. } => *json,
        Command::Path { json, format, .. } => *json || *format == Format::Json,
        _ => false,
    }
}

/// A failed command (contract M7f S2/S3): a refused request or an unreachable database prints as
/// every question's error does — the envelope under `--json`, else one `Error:` line — and exits
/// by its kind; anything else keeps its full chain and exit 1. `rb check` exits 2 on any error,
/// so a failure is never read as violations (1).
fn report_failure(e: anyhow::Error, json: bool, check: bool) -> ExitCode {
    let code = if e.is::<ApiError>() || e.is::<rb_store::Unavailable>() {
        let api = ApiError::from(e);
        let code = api.kind.exit_code();
        let _ = answer_err(Out::json_if(json), api);
        ExitCode::from(code)
    } else {
        eprintln!("Error: {e:?}");
        ExitCode::FAILURE
    };
    if check { ExitCode::from(2) } else { code }
}

fn run_cli(cli: Cli) -> anyhow::Result<ExitCode> {
    if let Command::Serve {
        bind,
        workers,
        queue,
        deadline_ms,
        cypher_timeout_ms,
    } = cli.command
    {
        let config = rb_server::Config {
            bind,
            database_url: cli.database_url,
            workers: usize::from(workers),
            queue: usize::from(queue),
            deadline: Duration::from_millis(deadline_ms),
            settings: rb_api::Settings {
                cypher_timeout_ms,
                ..settings()?
            },
            hold: match std::env::var("RB_TEST_HOLD_MS") {
                Ok(ms) => Duration::from_millis(ms.parse().context("RB_TEST_HOLD_MS")?),
                Err(_) => Duration::ZERO,
            },
        };
        // A refused address writes nothing: the bind check comes before the sweep, which is
        // the server's only write (contract M6a D10); `serve` repeats the check for its other
        // callers.
        rb_server::check_bind(&config.bind)?;
        sweep_leftover_graphs(&config.database_url);
        rb_server::serve(
            config,
            |addr| {
                println!("rb serve: listening on http://{addr}");
                let _ = std::io::stdout().flush();
            },
            rb_server::Shutdown::Signal,
        )?;
        return Ok(ExitCode::SUCCESS);
    }
    let mut store = Store::connect(&cli.database_url)?;
    match cli.command {
        Command::Migrate => {
            let applied = store.migrate()?;
            if applied.is_empty() {
                println!("schema up to date ({})", rb_store::schema_version());
            } else {
                println!("applied: {}", applied.join(", "));
            }
        }
        Command::Index(args) => index(&mut store, &cli.age_load_dir, args)?,
        Command::Dump {
            repo,
            snapshot,
            output,
        } => {
            let dump = store.read_tx(|store| {
                let snap = match snapshot {
                    Some(id) => store.snapshot(id)?,
                    None => live_snapshot(store, repo)?,
                };
                let graph = store.load_graph(snap.id)?;
                let diagnostics = store.load_diagnostics(snap.id)?;
                Ok(canonical_dump(&snap.commit_sha, &graph, &diagnostics))
            })?;
            match output {
                Some(p) => {
                    std::fs::write(&p, dump).with_context(|| format!("writing {}", p.display()))?
                }
                None => std::io::stdout().write_all(dump.as_bytes())?,
            }
        }
        Command::Gc {
            dry_run,
            prefix,
            egress,
        } => {
            let plan = store.gc_plan(&prefix)?;
            let done = if dry_run {
                plan
            } else {
                store.gc_apply(&plan)?
            };
            print!("{}", gc_lines(&done, dry_run));
            if egress {
                rb_scip::docker_available().map_err(|e| anyhow::anyhow!("gc --egress: {e}"))?;
                let found = rb_scip::DockerRunner::default().egress_leftovers(!dry_run)?;
                let verb = if dry_run { "would remove" } else { "removed" };
                if found.is_empty() {
                    println!("gc: no egress network or proxy");
                }
                for item in found {
                    println!("{verb} {item}");
                }
            }
        }
        Command::Cache {
            action:
                CacheAction::Prune {
                    facts,
                    scip,
                    dry_run,
                },
        } => {
            ensure!(facts || scip, "pass --facts, --scip or both");
            let verb = if dry_run { "would remove" } else { "removed" };
            if facts {
                let r = store.prune_facts(rb_syntax::extractor_id(), dry_run)?;
                println!("facts: {verb} {} rows, {} bytes", r.rows, r.bytes);
            }
            if scip {
                // Rows of an image this run cannot identify are not provably stale: refuse.
                rb_scip::docker_available().map_err(|e| {
                    anyhow::anyhow!("{e}; the current scip-java image is unknown, nothing pruned")
                })?;
                let images = rb_scip::Images::resolve(&rb_scip::DockerRunner::default());
                let java = images
                    .java
                    .map_err(|e| anyhow::anyhow!("{e}; nothing pruned"))?;
                let r =
                    store.prune_scip(&[&java, rb_scip::jobs::SCIP_TYPESCRIPT_IMAGE], dry_run)?;
                println!("scip: {verb} {} rows, {} bytes", r.rows, r.bytes);
            }
        }
        Command::Check {
            repo,
            rule,
            json,
            timeout_ms,
            baseline,
        } => {
            return check_rules(
                &mut store,
                repo,
                rule,
                json,
                timeout_ms,
                baseline.as_deref(),
            );
        }
        question => {
            let asked = asked(question).expect("every other command is a question");
            return answer(&mut store, asked);
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// How a question's answer is printed: JSON, or text (`evidence`: `rb diff`'s evidence-only
/// edges listed, not only counted).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Out {
    Text { evidence: bool },
    Json,
}

impl Out {
    fn json_if(json: bool) -> Out {
        if json {
            Out::Json
        } else {
            Out::Text { evidence: false }
        }
    }
}

/// A question command: what it asks, of which repository, with which parameters, and how the
/// answer is printed.
type Asked = (Endpoint, Option<String>, Params, Out);

/// The question a command asks, its flags as the request layer's string parameters (contract M5
/// S12: `rb-api` alone validates them); `None` for the commands that are not questions.
fn asked(command: Command) -> Option<Asked> {
    let mut p = Params::new();
    Some(match command {
        Command::Status { repo, json } => (Endpoint::Status, repo, p, Out::json_if(json)),
        Command::Snapshots {
            repo,
            limit,
            offset,
            json,
        } => {
            p.set_opt("limit", limit).set_opt("offset", offset);
            (Endpoint::Snapshots, repo, p, Out::json_if(json))
        }
        Command::Find {
            text,
            kind,
            limit,
            offset,
            repo,
            snapshot,
            json,
        } => {
            p.set("q", text)
                .set_opt("kind", kind)
                .set_opt("limit", limit)
                .set_opt("offset", offset)
                .set_opt("snapshot", snapshot);
            (Endpoint::Find, repo, p, Out::json_if(json))
        }
        Command::Service {
            service,
            repo,
            limit,
            offset,
            snapshot,
            json,
        } => {
            p.set_opt("snapshot", snapshot)
                .set_opt("limit", limit)
                .set_opt("offset", offset);
            let endpoint = match service {
                Some(name) => {
                    p.set("name", name);
                    Endpoint::Service
                }
                None => Endpoint::Services,
            };
            (endpoint, repo, p, Out::json_if(json))
        }
        Command::TableDeps {
            table,
            repo,
            snapshot,
            json,
            precision,
            hops,
        } => {
            p.set("table", table)
                .set_opt("snapshot", snapshot)
                .set_opt("precision", precision)
                .set_opt("hops", hops);
            (Endpoint::TableDeps, repo, p, Out::json_if(json))
        }
        Command::Callers {
            target,
            repo,
            snapshot,
            json,
            depth,
            include_references,
            min_certainty,
            pick,
            precision,
        } => {
            p.set("target", target)
                .set_opt("snapshot", snapshot)
                .set_opt("depth", depth)
                .set_flag("include_references", include_references)
                .set_opt("min_certainty", min_certainty)
                .set_opt("pick", pick)
                .set_opt("precision", precision);
            (Endpoint::Callers, repo, p, Out::json_if(json))
        }
        Command::Impact {
            target,
            repo,
            snapshot,
            min_certainty,
            pick,
            precision,
            json,
        } => {
            p.set("target", target)
                .set_opt("snapshot", snapshot)
                .set_opt("min_certainty", min_certainty)
                .set_opt("pick", pick)
                .set_opt("precision", precision);
            (Endpoint::Impact, repo, p, Out::json_if(json))
        }
        Command::Path {
            query,
            repo,
            snapshot,
            depth,
            hops,
            full,
            format,
            json,
            pick,
            precision,
        } => {
            p.set("endpoint", query)
                .set_opt("snapshot", snapshot)
                .set_opt("depth", depth)
                .set_opt("hops", hops)
                .set_flag("full", full)
                .set_opt("pick", pick)
                .set_opt("precision", precision);
            (
                Endpoint::Path,
                repo,
                p,
                Out::json_if(json || format == Format::Json),
            )
        }
        Command::Cypher {
            query,
            repo,
            snapshot,
            json,
            cols,
            timeout_ms,
        } => {
            p.set("query", query)
                .set_opt("snapshot", snapshot)
                .set_opt("columns", cols)
                .set_opt("timeout_ms", timeout_ms);
            (Endpoint::Cypher, repo, p, Out::json_if(json))
        }
        Command::Diff {
            from,
            to,
            repo,
            json,
            diagnostics,
            evidence,
            limit,
            offset,
        } => {
            p.set_opt("from", from)
                .set_opt("to", to)
                .set_flag("diagnostics", diagnostics)
                .set_opt("limit", limit)
                .set_opt("offset", offset);
            let out = if json {
                Out::Json
            } else {
                Out::Text { evidence }
            };
            (Endpoint::Diff, repo, p, out)
        }
        Command::Migrate
        | Command::Index(_)
        | Command::Check { .. }
        | Command::Serve { .. }
        | Command::Dump { .. }
        | Command::Gc { .. }
        | Command::Cache { .. } => return None,
    })
}

/// Answers a question through the request layer (contract M5 D2, S12), the same parameters and
/// code the server uses, and prints it; a refused request prints its message and exits with
/// D5's code.
fn answer(store: &mut Store, (endpoint, repo, params, out): Asked) -> anyhow::Result<ExitCode> {
    // `--repo` wins; else the working directory's checkout when indexed; else a
    // directive error — never a guess across several repositories. `rb status` is the
    // exception: outside an indexed checkout it is the overview of every repository, which
    // guesses nothing (README; contract M7b S1).
    let repo = match repo {
        Some(r) => Some(r),
        None => match cwd_repo_name() {
            Some(name) if is_indexed(store, &name) => Some(name),
            _ if matches!(endpoint, Endpoint::Status) => None,
            name => return answer_err(out, repo_needed(name.as_deref(), &cwd_display())),
        },
    };
    let settings = settings()?;
    let answered = rb_api::parse(endpoint, repo.as_deref(), &params, &settings)
        .and_then(|req| rb_api::run(store, &mut rb_api::LoadEach, &req, &settings));
    match answered {
        Ok(output) => {
            match out {
                Out::Json => print!("{}", output.body()),
                Out::Text { evidence } => {
                    print!("{}", render_text(&output, evidence));
                    // A page that is not the last says so on stderr: stdout stays rows only.
                    if let Some(line) = more_line(&output.answer) {
                        eprintln!("{line}");
                    }
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(e) => answer_err(out, e),
    }
}

/// Prints a refused request exactly like `answer` does and exits with its code.
fn answer_err(out: Out, e: ApiError) -> anyhow::Result<ExitCode> {
    match out {
        // The error body the server sends for the same request (contract M5 S14).
        Out::Json => print!("{}", e.body()),
        Out::Text { .. } => eprintln!("Error: {}", e.cli_text()),
    }
    Ok(ExitCode::from(e.kind.exit_code()))
}

/// The SCIP budget `rb index`'s flags set (contract M6a D9); every unset flag keeps
/// `rb_scip::Budget::default()`'s value.
fn scip_budget(args: &IndexArgs) -> rb_scip::Budget {
    let mut budget = rb_scip::Budget::default();
    if let Some(secs) = args.scip_timeout_secs {
        budget.fetch_timeout = Duration::from_secs(secs);
        budget.index_timeout = Duration::from_secs(secs);
    }
    if let Some(cpus) = args.scip_cpus {
        budget.cpus = cpus;
    }
    if let Some(memory) = &args.scip_memory {
        budget.memory.clone_from(memory);
    }
    if let Some(threads) = args.scip_maven_threads {
        budget.maven_threads = threads;
    }
    budget
}

/// `--scip-cpus`: a finite number from 0.5 to 64.
fn parse_scip_cpus(text: &str) -> Result<f32, String> {
    let cpus: f32 = text.parse().map_err(|e| format!("{e}"))?;
    if (0.5..=64.0).contains(&cpus) {
        Ok(cpus)
    } else {
        Err("must be from 0.5 to 64".into())
    }
}

/// `--scip-memory`: `<n>m` or `<n>g` with `n` digits only, from 512m to 64g; kept as written.
fn parse_scip_memory(text: &str) -> Result<String, String> {
    let (digits, unit) = text.split_at(text.len().saturating_sub(1));
    let mib = match unit {
        "m" => 1,
        "g" => 1024,
        _ => return Err("must be <n>m or <n>g".into()),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err("must be <n>m or <n>g".into());
    }
    let n: u64 = digits.parse().map_err(|e| format!("{e}"))?;
    if (512..=64 * 1024).contains(&n.saturating_mul(mib)) {
        Ok(text.to_owned())
    } else {
        Err("must be from 512m to 64g".into())
    }
}

/// `rb serve`'s startup sweep (contract M5 amendment 16): drops the AGE graphs that non-live
/// snapshots still name (left by a publisher killed between its flip and its retention), for
/// every repository, one graph per transaction under the DDL lock, after the bind address passed
/// its check (contract M6a D10) and before any request is answered; the requests themselves stay
/// read-only. Reads never use such a graph, so a sweep that cannot run (no database yet) is
/// reported on stderr and the server starts anyway.
fn sweep_leftover_graphs(database_url: &str) {
    let swept = Store::connect(database_url).and_then(|mut store| {
        let repos = store.repo_names()?;
        store.sweep_graphs(&repos)
    });
    match swept {
        Ok(graphs) => {
            for g in graphs {
                eprintln!("rb serve: dropped leftover AGE graph {g}");
            }
        }
        Err(e) => eprintln!("rb serve: warning: leftover AGE graphs not swept: {e:#}"),
    }
}

/// The request layer's settings: D6's caps, lowered by `RB_TEST_CAPS` in tests (contract M5 S14).
fn settings() -> anyhow::Result<rb_api::Settings> {
    match std::env::var(rb_api::Settings::TEST_CAPS_VAR) {
        Ok(spec) => rb_api::Settings::default()
            .with_test_caps(&spec)
            .map_err(anyhow::Error::msg),
        Err(_) => Ok(rb_api::Settings::default()),
    }
}

/// `rows 1-20 of 57 listed; next page: --offset 20` when a paged list continues (D18).
fn more_line(answer: &Answer) -> Option<String> {
    let (offset, shown, total, next) = match answer {
        Answer::Find(p) => (p.offset, p.items.len(), p.total, p.next_offset?),
        Answer::Services(p) => (p.offset, p.items.len(), p.total, p.next_offset?),
        Answer::Snapshots(p) => (p.offset, p.items.len(), p.total, p.next_offset?),
        _ => return None,
    };
    Some(format!(
        "rows {}-{} of {total} listed; next page: --offset {next}",
        offset + 1,
        offset + shown as u64
    ))
}

/// The text rendering of an answer (the CLI's own; JSON is `rb-api`'s).
fn render_text(output: &rb_api::Output, evidence: bool) -> String {
    match &output.answer {
        Answer::Diff(d) => diff_text(d, evidence),
        Answer::Status { all, reports } => status_text(*all, reports),
        Answer::Snapshots(snapshots) => snapshots
            .items
            .iter()
            .map(|x| {
                format!(
                    "{}\t{}\t{}\tparent {}\tAGE graph {}\n",
                    x.id,
                    short(&x.commit_sha),
                    x.status,
                    x.parent_id
                        .map_or_else(|| "-".to_owned(), |p| p.to_string()),
                    x.age_graph.as_deref().unwrap_or("-")
                )
            })
            .collect(),
        Answer::Find(found) => found
            .items
            .iter()
            .map(|n| format!("{}\t{}\t{}\n", n.kind, n.key, n.name))
            .collect(),
        Answer::Services(services) => services
            .items
            .iter()
            .map(|s| format!("{}\t{}\t{}\t{}\n", s.key, s.name, s.role, files(s.files)))
            .collect(),
        Answer::Card(card) => {
            let mut text = render_card(card);
            if !output.truncated.is_empty() {
                let _ = writeln!(text, "truncated: {}", output.truncated.join(", "));
            }
            text
        }
        Answer::Callers { ix, tree } => code::render_callers(ix, tree),
        Answer::Impact { ix, impact } => code::render_impact(ix, impact),
        Answer::TableDeps(deps) => render_table_deps(deps),
        Answer::Path { ix, path } => path::render_text(ix, path),
        Answer::Cypher(rows) => {
            let mut out = format!("{}\n", rows.columns.join("\t"));
            for row in &rows.rows {
                let _ = writeln!(out, "{}", row.join("\t"));
            }
            if !rows.truncated.is_empty() {
                let _ = writeln!(out, "truncated: {}", rows.truncated.join(", "));
            }
            out
        }
    }
}

fn index(store: &mut Store, load_dir: &Path, args: IndexArgs) -> anyhow::Result<()> {
    let budget = scip_budget(&args);
    let IndexArgs {
        path,
        repo,
        rev,
        force,
        no_cache,
        no_scip,
        scip_work_dir,
        scip_timeout_secs: _,
        scip_cpus: _,
        scip_memory: _,
        scip_maven_threads: _,
        scip_registries,
        keep,
        profile,
    } = args;
    let egress = rb_scip::Egress::with(&scip_registries).map_err(|e| anyhow::anyhow!("{e}"))?;
    let path = path
        .canonicalize()
        .with_context(|| format!("repository path {}", path.display()))?;
    let repo = match repo {
        Some(r) => r,
        None => repo_name_from_path(&path),
    };
    ensure!(
        !repo.is_empty()
            && repo.len() <= 40
            && repo
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
        "invalid repo name {repo:?}: use [a-z0-9_], max 40 characters"
    );
    ensure!(
        load_dir.is_dir(),
        "AGE load directory {} does not exist; run from the project root (compose mounts ./var/age-load) or set RB_AGE_LOAD_DIR",
        load_dir.display()
    );

    let commit_sha = rb_git::Repo::open(&path)?.resolve_commit(&rev)?;
    store.migrate()?;
    // Decided here, before the "up to date" check: a snapshot indexed without Docker, or
    // without an indexer image, is heuristic and must be rebuilt once they answer (contract M3b
    // amendments 17 and 29). The images are looked up once; the jobs run in and cache under
    // the same ones.
    let scip_mode = if no_scip {
        rb_indexer::ScipMode::Off
    } else {
        match rb_scip::docker_available() {
            Ok(()) => rb_indexer::ScipMode::On,
            Err(reason) => rb_indexer::ScipMode::Unavailable(reason),
        }
    };
    let images = match &scip_mode {
        rb_indexer::ScipMode::On => rb_scip::Images::resolve(&rb_scip::DockerRunner::default()),
        rb_indexer::ScipMode::Unavailable(detail) => {
            // The detail names the host (its socket path); the stored reason does not
            // (contract M3b amendment 49).
            eprintln!("scip: {detail}; every project stays heuristic");
            rb_scip::Images::unresolved()
        }
        rb_indexer::ScipMode::Off => rb_scip::Images::unresolved(),
    };
    let tools = tool_versions(&scip_mode, &images);

    if !force && let Some(live) = store.live_snapshot(&repo)? {
        let scip_on = scip_mode == rb_indexer::ScipMode::On;
        let mut unfinished = |id: i64| store.unfinished_scip_jobs(id);
        match rebuild_reason(&live, &commit_sha, &tools, scip_on, &mut unfinished)? {
            None => {
                println!(
                    "{repo}: up to date — live snapshot {} already covers {} (use --force to rebuild)",
                    live.id,
                    short(&commit_sha)
                );
                return Ok(());
            }
            Some(Rebuild::UnfinishedJobs(n)) => {
                let jobs = if n == 1 { "job" } else { "jobs" };
                println!("{repo}: rebuilding: {n} SCIP {jobs} did not finish last time");
            }
            Some(Rebuild::Commit | Rebuild::Tools) => {}
        }
    }

    let scip = rb_indexer::ScipOptions {
        mode: scip_mode,
        images,
        cache_dir: scip_cache_dir(&scip_work_dir, &repo),
        work_dir: scip_work_dir,
        budget,
        egress,
    };
    // Both caches live in the store; the indexer borrows each through its own trait.
    let cell = RefCell::new(&mut *store);
    // `--no-cache` reads no facts but writes what it extracts (contract M5 S5).
    let rb_indexer::Indexed {
        commit_sha,
        graph,
        diagnostics,
        parsed,
        scanned,
        scip: jobs,
        scip_ungrouped,
    } = if no_cache {
        rb_indexer::index_commit(
            &path,
            &commit_sha,
            &mut WriteOnlyFactsCache(&mut StoreCache(&cell)),
            &scip,
            &mut rb_indexer::WriteOnlyScipCache(&mut StoreCache(&cell)),
        )?
    } else {
        rb_indexer::index_commit(
            &path,
            &commit_sha,
            &mut StoreCache(&cell),
            &scip,
            &mut StoreCache(&cell),
        )?
    };
    store.set_keep_retired(keep)?;
    let rb_store::Published {
        snapshot: snap,
        retention,
        projection,
    } = store.publish_snapshot(
        &repo,
        &commit_sha,
        &tools,
        &graph,
        &diagnostics,
        &LoadDir::new(load_dir),
    )?;
    if let Some(id) = retention.live_marked_retired {
        eprintln!(
            "warning: {repo}'s live pointer names snapshot {id}, whose status is not `live`; retention kept it"
        );
    }
    println!(
        "{repo}: snapshot {} live @ {} — {} nodes, {} edges, {} diagnostics; AGE graph {}; parsed {parsed} source files; scanned {scanned} other files as text",
        snap.id,
        short(&snap.commit_sha),
        graph.nodes().len(),
        graph.edges().len(),
        diagnostics.len(),
        snap.age_graph.as_deref().unwrap_or("-")
    );
    if profile {
        print!("{}", projection_lines(&projection));
    }
    for j in &jobs {
        let mut line = format!(
            "  scip: {} {} {}",
            root_label(&j.root),
            j.indexer,
            j.status.as_str()
        );
        if j.cached {
            line.push_str(" (cached)");
        }
        if j.unpinned {
            line.push_str(" unpinned");
        }
        // The container run's time (the cached run's when served from the cache) is printed
        // here only; it never enters the graph (contract M3b amendment 32).
        if j.elapsed_ms > 0 {
            let _ = write!(line, " {:.1}s", j.elapsed_ms as f64 / 1000.0);
        }
        line.push_str(&status_detail(&j.status));
        println!("{line}");
        // The docker client's whole text, for the terminal only: the stored reason is its
        // redacted first line (contract M3b amendment 49).
        if matches!(j.status, rb_scip::RunStatus::Environment(_)) {
            for l in j.stderr_tail.lines() {
                eprintln!("    {l}");
            }
        }
    }
    // Workspace roots whose packages keep separate jobs, and why (contract M6a D5); printed
    // only, so a dump does not change through them.
    for u in &scip_ungrouped {
        println!(
            "  scip: workspace {} not grouped ({})",
            root_label(&u.root),
            u.reason
        );
    }
    Ok(())
}

/// Why the live snapshot is rebuilt instead of being "up to date".
#[derive(Debug, PartialEq, Eq)]
enum Rebuild {
    Commit,
    Tools,
    /// The last run left this many SCIP jobs unfinished (contract M3b amendment 44).
    UnfinishedJobs(i64),
}

/// Whether `live` covers a run of `commit_sha` with `tools` (contract M3b amendments 5, 17, 29
/// and 44): the same commit, the same tool configuration and, when SCIP is on, no job of the
/// last run left unfinished (`unfinished` counts the live snapshot's `environment`, `timeout`,
/// `skipped` and `unsupported` jobs; those outcomes are not cached and are decided again, so a
/// snapshot that records them is heuristic for a reason the next run may not have). With SCIP
/// off or unavailable every job is skipped by configuration, which `tools` already records.
fn rebuild_reason(
    live: &SnapshotInfo,
    commit_sha: &str,
    tools: &Value,
    scip_on: bool,
    unfinished: &mut dyn FnMut(i64) -> anyhow::Result<i64>,
) -> anyhow::Result<Option<Rebuild>> {
    if live.commit_sha != commit_sha {
        return Ok(Some(Rebuild::Commit));
    }
    if live.tool_versions != *tools {
        return Ok(Some(Rebuild::Tools));
    }
    if scip_on {
        let n = unfinished(live.id)?;
        if n > 0 {
            return Ok(Some(Rebuild::UnfinishedJobs(n)));
        }
    }
    Ok(None)
}

/// ` (fetch phase, 600 s budget)` after a `timeout` status (contract M3b amendment 31),
/// ` (fetch phase)` after a `failed` one (amendment 56).
fn phase_detail(phase: &str, budget_secs: Option<u64>) -> String {
    match budget_secs {
        Some(budget) => format!(" ({phase} phase, {budget} s budget)"),
        None => format!(" ({phase} phase)"),
    }
}

/// What `rb index` prints after a job's status: the phase of a timeout or failure, else the
/// reason a job did not run, else nothing.
fn status_detail(status: &rb_scip::RunStatus) -> String {
    match status {
        rb_scip::RunStatus::Timeout { phase, budget_secs } => {
            phase_detail(phase.as_str(), Some(*budget_secs))
        }
        rb_scip::RunStatus::Failed(phase) => phase_detail(phase.as_str(), None),
        other => other
            .reason()
            .map_or_else(String::new, |reason| format!(": {reason}")),
    }
}

/// The `rb status` resolution line (contract M3a R8, M3b S9 and amendment 41). With no
/// heuristic site at all (an all-SCIP repository) there is no share to print.
fn code_line(r: &rb_store::ResolutionCoverage) -> String {
    if r.resolved + r.ambiguous + r.unresolved == 0 {
        format!(
            "code: no heuristic references to own code ({} external); {} exact calls (scip)",
            r.external, r.exact_calls
        )
    } else {
        format!(
            "code: {:.1}% of references to own code resolved ({} resolved, {} ambiguous, {} unresolved; {} external); {} exact calls (scip)",
            r.resolved_share() * 100.0,
            r.resolved,
            r.ambiguous,
            r.unresolved,
            r.external,
            r.exact_calls
        )
    }
}

/// The `rb status` HTTP line (contract M4a S14, M6b S4): endpoints, the services exposing them,
/// and the client call sites per bucket, zero buckets omitted, in a fixed order. The S4
/// exclusions subdivide `unmatched` and print with it.
fn http_line(c: &rb_store::HttpCoverage) -> String {
    const ORDER: &[(&str, &str)] = &[
        ("synthetic", "synthetic"),
        ("heuristic", "heuristic"),
        ("ambiguous", "ambiguous"),
        ("too_ambiguous", "too ambiguous"),
        ("unmatched", "unmatched"),
        ("http_in_process", "in-process"),
        ("http_empty_url", "empty-url"),
        ("external", "external"),
        ("dynamic", "dynamic"),
        ("self", "self"),
    ];
    let mut line = format!(
        "http: {} routes in {} services; {} client calls",
        c.routes, c.services, c.sites
    );
    let parts: Vec<String> = ORDER
        .iter()
        .filter_map(|(bucket, label)| {
            let n = *c.buckets.get(*bucket)?;
            (n > 0).then(|| format!("{n} {label}"))
        })
        .collect();
    if !parts.is_empty() {
        let _ = write!(line, ": {}", parts.join(", "));
    }
    line
}

/// The `rb status` DI line (contract M4b S7, amendment 2): injection points per bucket, zero
/// buckets omitted, in a fixed order.
fn di_line(c: &rb_store::DiCoverage) -> String {
    const ORDER: &[(&str, &str)] = &[
        ("plain", "plain"),
        ("capped", "capped"),
        ("ambiguous", "ambiguous"),
        ("too_ambiguous", "too ambiguous"),
        ("external", "external"),
        ("unresolved", "unresolved"),
        ("optional_absent", "optional absent"),
        ("not_a_bean", "not a bean"),
        ("test", "test"),
    ];
    let mut line = format!("di: {} injection points", c.points);
    let parts: Vec<String> = ORDER
        .iter()
        .filter_map(|(bucket, label)| {
            let n = *c.buckets.get(*bucket)?;
            (n > 0).then(|| format!("{n} {label}"))
        })
        .collect();
    if !parts.is_empty() {
        let _ = write!(line, ": {}", parts.join(", "));
    }
    line
}

/// The `rb status` RTA line (contract M4b S9).
fn rta_line(c: &rb_store::RtaCoverage) -> String {
    format!(
        "rta: {} of {} implementing types never instantiated ({} dispatch edges prunable)",
        c.proven, c.types, c.prunable_edges
    )
}

/// `.` for the repository root, as in the `scip_status` diagnostic.
fn root_label(root: &str) -> &str {
    if root.is_empty() { "." } else { root }
}

/// The `rb status` line for one `scip_status` diagnostic (contract M3b S9).
fn scip_status_line(d: &rb_model::Diagnostic) -> String {
    let field = |k: &str| d.detail[k].as_str().unwrap_or("?").to_owned();
    let mut line = format!(
        "scip: {} {} {}",
        d.path.as_deref().unwrap_or("?"),
        field("indexer"),
        field("status")
    );
    // No lockfile: the fetch resolved versions at run time, so the index is not reproducible.
    if d.detail["unpinned"].as_bool() == Some(true) {
        line.push_str(" unpinned");
    }
    // A timeout's stored `reason` is rendered from its phase and budget, so the detail is
    // printed once, as `rb index` prints it (contract M3b amendment 50); a failure prints its
    // phase (amendment 56).
    match d.detail["phase"].as_str() {
        Some(phase) => line.push_str(&phase_detail(phase, d.detail["budget_secs"].as_u64())),
        None => {
            if let Some(reason) = d.detail["reason"].as_str() {
                let _ = write!(line, ": {reason}");
            }
        }
    }
    // A grouped workspace root (contract M6a D5), only when there are members.
    if let Some(members) = d.detail["workspace_members"].as_u64() {
        let _ = write!(line, ", workspace {members} members");
    }
    // Exported link targets outside the root (contract M6a D7), only when there are any.
    if let Some(targets) = d.detail["link_targets"].as_u64() {
        let _ = write!(line, ", {targets} link targets");
    }
    if let Some(c) = d.detail.get("coverage") {
        let n = |k: &str| c[k].as_u64().unwrap_or(0);
        let _ = write!(
            line,
            ", definitions matched {}/{} ({} by name)",
            n("definitions_matched"),
            n("definitions_total"),
            n("definitions_matched_by_name"),
        );
        // A field a snapshot written before it existed lacks is omitted, not printed as 0
        // (contract M3b amendment 67): `definitions_without_occurrence` from amendment 62.
        if let Some(without) = c["definitions_without_occurrence"].as_u64() {
            let _ = write!(line, ", definitions without occurrence {without}");
        }
        let _ = write!(
            line,
            ", references matched {}, external {}, undocumented {}, local {}, unanchored {}, unmatched {}, unconverted {}",
            n("references_matched"),
            n("references_external"),
            n("references_undocumented"),
            n("references_local"),
            n("references_unanchored"),
            n("references_unmatched"),
            n("occurrences_unconverted"),
        );
        // Documents the agreement check left to Tier B (contract M3b amendment 64), with the
        // reasons when there are any; omitted for a snapshot written before it existed.
        if let Some(rejected) = c["documents_rejected"].as_u64() {
            let _ = write!(line, ", documents rejected {rejected}");
            if let Some(reasons) = c["documents_rejected_reasons"]
                .as_object()
                .filter(|r| !r.is_empty())
            {
                let reasons: Vec<String> = reasons
                    .iter()
                    .map(|(reason, k)| format!("{reason} {}", k.as_u64().unwrap_or(0)))
                    .collect();
                let _ = write!(line, " ({})", reasons.join(", "));
            }
        }
        // Documents of a link target's sources the job ignored (contract M6a D7), when there
        // are any.
        if n("documents_outside_root") > 0 {
            let _ = write!(
                line,
                ", {} documents outside the root",
                n("documents_outside_root")
            );
        }
        // Occurrences removed as Lombok-generated before the agreement checks (contract M6a
        // D1, D3), when there are any.
        if n("lombok_removed") > 0 {
            let _ = write!(
                line,
                ", lombok-generated {} in {} documents",
                n("lombok_removed"),
                n("documents_lombok")
            );
        }
        let _ = write!(
            line,
            ", relationships dropped {} ({} in project, {} anonymous)",
            n("relationships_dropped"),
            n("relationships_dropped_in_project"),
            n("relationships_anonymous")
        );
        // Files the indexer left out (excluded by its tsconfig): no edges of any kind.
        if n("files_without_document") > 0 {
            let _ = write!(
                line,
                ", {} files without a document",
                n("files_without_document")
            );
            // Of those, the ones their tsconfig positively excludes (contract M6e S10).
            if n("files_without_document_out_of_scope") > 0 {
                let _ = write!(
                    line,
                    " ({} out of tsconfig scope)",
                    n("files_without_document_out_of_scope")
                );
            }
        }
        // Declared supertypes the index has no type occurrence for: the `IMPLEMENTS` walk
        // stops at them.
        if n("supertypes_unresolved") > 0 {
            let _ = write!(
                line,
                ", {} supertypes unresolved",
                n("supertypes_unresolved")
            );
        }
        // Declared supertypes the type's own symbol information does not relate it to
        // (contract M3b amendment 78): the walk does not lead through them either.
        if n("supertypes_unconfirmed") > 0 {
            let _ = write!(
                line,
                ", {} supertypes unconfirmed",
                n("supertypes_unconfirmed")
            );
        }
        // Definitions scip-java keys as document-local symbols: they keep heuristic keys
        // (contract M3b amendment 54).
        if n("definitions_local") > 0 {
            let _ = write!(line, ", {} local definitions", n("definitions_local"));
        }
    }
    line
}

/// Syntax facts and SCIP indexes cached in Postgres: facts by (blob, extractor version +
/// language), indexes by (project input hash, image digest).
struct StoreCache<'a, 'b>(&'a RefCell<&'b mut Store>);

impl rb_indexer::ScipCache for StoreCache<'_, '_> {
    fn get(
        &mut self,
        input_hash: &str,
        image: &str,
    ) -> anyhow::Result<Option<rb_indexer::ScipCacheEntry>> {
        Ok(self
            .0
            .borrow_mut()
            .scip_index_get(input_hash, image)?
            .map(|e| rb_indexer::ScipCacheEntry {
                status: e.status,
                phase: e.phase,
                index: e.index,
                stderr_tail: e.stderr_tail,
                elapsed_ms: u64::try_from(e.elapsed_ms).unwrap_or(0),
            }))
    }

    fn put(
        &mut self,
        input_hash: &str,
        image: &str,
        entry: &rb_indexer::ScipCacheEntry,
    ) -> anyhow::Result<()> {
        self.0.borrow_mut().scip_index_put(
            input_hash,
            image,
            &rb_store::CachedScipIndex {
                status: entry.status.clone(),
                phase: entry.phase.clone(),
                index: entry.index.clone(),
                stderr_tail: entry.stderr_tail.clone(),
                elapsed_ms: i32::try_from(entry.elapsed_ms).unwrap_or(i32::MAX),
            },
        )
    }
}

impl rb_indexer::FactsCache for StoreCache<'_, '_> {
    fn load(
        &mut self,
        keys: &[rb_indexer::FactsKey],
    ) -> anyhow::Result<HashMap<rb_indexer::FactsKey, rb_syntax::FileFacts>> {
        let by_row: HashMap<(String, String), &rb_indexer::FactsKey> = keys
            .iter()
            .map(|k| ((k.blob.clone(), k.extractor()), k))
            .collect();
        let rows: Vec<(String, String)> = by_row.keys().cloned().collect();
        self.0
            .borrow_mut()
            .load_facts(&rows)?
            .into_iter()
            .map(|(row, bytes)| {
                let facts = serde_json::from_slice(&bytes)
                    .with_context(|| format!("decoding cached facts for blob {}", row.0))?;
                Ok((by_row[&row].clone(), facts))
            })
            .collect()
    }

    fn save(
        &mut self,
        facts: &[(rb_indexer::FactsKey, rb_syntax::FileFacts)],
    ) -> anyhow::Result<()> {
        let rows = facts
            .iter()
            .map(|(k, f)| Ok((k.blob.clone(), k.extractor(), serde_json::to_vec(f)?)))
            .collect::<anyhow::Result<Vec<_>>>()?;
        self.0.borrow_mut().save_facts(&rows)
    }
}

/// A facts cache that is never read, only written (`rb index --no-cache`, contract M5 S5, M4a
/// open item 4): every file is parsed again, and its facts are stored for the next plain run,
/// as [`rb_indexer::WriteOnlyScipCache`] does for SCIP jobs.
struct WriteOnlyFactsCache<'a>(&'a mut dyn rb_indexer::FactsCache);

impl rb_indexer::FactsCache for WriteOnlyFactsCache<'_> {
    fn load(
        &mut self,
        _: &[rb_indexer::FactsKey],
    ) -> anyhow::Result<HashMap<rb_indexer::FactsKey, rb_syntax::FileFacts>> {
        Ok(HashMap::new())
    }

    fn save(
        &mut self,
        facts: &[(rb_indexer::FactsKey, rb_syntax::FileFacts)],
    ) -> anyhow::Result<()> {
        self.0.save(facts)
    }
}

/// `rb index --profile`'s AGE lines (contract M5 S6): one per loaded file, then the totals.
fn projection_lines(p: &rb_store::ProjectionProfile) -> String {
    let mut out = String::new();
    for f in &p.files {
        let _ = writeln!(
            out,
            "  profile: age {} ({}): {} rows, {} bytes, {:.1} ms",
            f.label,
            f.file,
            f.rows,
            f.bytes,
            f.load.as_secs_f64() * 1000.0
        );
    }
    let rows: usize = p.files.iter().map(|f| f.rows).sum();
    let bytes: u64 = p.files.iter().map(|f| f.bytes).sum();
    let ms: f64 = p.files.iter().map(|f| f.load.as_secs_f64() * 1000.0).sum();
    let _ = writeln!(
        out,
        "  profile: age total: {} files, {rows} rows, {bytes} bytes, {ms:.1} ms",
        p.files.len()
    );
    out
}

/// The repository's own package-manager caches under the SCIP work directory,
/// `<work>/cache/<repo>` (contract M5 S22): one repository's fetch never writes where another's
/// reads. Repository names are `[a-z0-9_]`, so the name is a safe path component.
fn scip_cache_dir(work: &Path, repo: &str) -> PathBuf {
    work.join("cache").join(repo)
}

/// `rb gc`'s report: one line per graph and `building` row, then a summary.
fn gc_lines(done: &rb_store::GcPlan, dry_run: bool) -> String {
    if done.graphs.is_empty() && done.building.is_empty() {
        return "gc: nothing to remove\n".to_owned();
    }
    let verb = if dry_run { "would drop" } else { "dropped" };
    let mut out = String::new();
    for g in &done.graphs {
        let owner = match (&g.snapshot, &g.repo, &g.status) {
            (Some(id), Some(repo), Some(status)) => format!("snapshot {id} of {repo} ({status})"),
            _ => "no snapshot names it".to_owned(),
        };
        let _ = writeln!(out, "{verb} graph {}: {owner}", g.name);
    }
    for b in &done.building {
        let _ = writeln!(
            out,
            "{verb} building snapshot {} of {} @ {}",
            b.id,
            b.repo,
            short(&b.commit_sha)
        );
    }
    let _ = writeln!(
        out,
        "gc: {verb} {} graph(s) and {} building row(s)",
        done.graphs.len(),
        done.building.len()
    );
    out
}

/// What a snapshot was built with; `scip` (`on`, `off`, `unavailable`) because a run without
/// indexers is a different graph, whether they were turned off or Docker was missing, and with
/// `on` the identity of each indexer image (the pinned one's digest reference; the local one's
/// ID, or `absent`; contract M3b amendments 29 and 48), since a graph indexed without an image
/// is heuristic and a rebuilt image may index differently.
fn tool_versions(scip: &rb_indexer::ScipMode, images: &rb_scip::Images) -> Value {
    let mut tools = json!({
        "rb": env!("CARGO_PKG_VERSION"),
        "pipeline": rb_indexer::PIPELINE,
        "schema": rb_store::schema_version(),
        "scip": scip.as_str(),
    });
    if *scip == rb_indexer::ScipMode::On {
        tools["scip_java"] = images.identity(rb_scip::jobs::Indexer::ScipJava).into();
        tools["scip_typescript"] = images
            .identity(rb_scip::jobs::Indexer::ScipTypescript)
            .into();
        tools["scip_go"] = images.identity(rb_scip::jobs::Indexer::ScipGo).into();
        tools["scip_python"] = images.identity(rb_scip::jobs::Indexer::ScipPython).into();
    }
    tools
}

/// Rows of one text section of `rb diff`: the first [`DIFF_TEXT_ROWS`], then how many more.
const DIFF_TEXT_ROWS: usize = 50;

fn diff_rows<T>(out: &mut String, sign: char, rows: &[T], render: impl Fn(&T) -> String) {
    for r in rows.iter().take(DIFF_TEXT_ROWS) {
        let _ = writeln!(out, "  {sign} {}", render(r));
    }
    if rows.len() > DIFF_TEXT_ROWS {
        let _ = writeln!(
            out,
            "  … {} more (--json for all)",
            rows.len() - DIFF_TEXT_ROWS
        );
    }
}

/// `rb diff`'s text (contract M5 S17): the header, the summaries, then each exact section's
/// counts and first rows.
fn diff_text(d: &rb_api::diff::DiffAnswer, evidence: bool) -> String {
    use rb_model::diff::{Change, EdgeRow, NodeRow};
    let mut out = String::new();
    let side =
        |s: &SnapshotInfo| format!("snapshot {} of {} @ {}", s.id, s.repo, short(&s.commit_sha));
    let _ = writeln!(out, "diff {} → {}", side(&d.from), side(&d.to));
    if !d.tools_differ.is_empty() {
        let _ = writeln!(out, "tools differ: {}", d.tools_differ.join(", "));
    }
    let s = &d.summaries;
    let summary = |out: &mut String, title: &str, added: usize, removed: usize| {
        let _ = writeln!(out, "{title}: {added} added, {removed} removed");
    };
    summary(
        &mut out,
        "endpoints",
        s.endpoints.added.len(),
        s.endpoints.removed.len(),
    );
    let endpoint = |e: &rb_store::diff::EndpointRow| format!("{}  [{}]", e.display, e.service);
    diff_rows(&mut out, '+', &s.endpoints.added, endpoint);
    diff_rows(&mut out, '-', &s.endpoints.removed, endpoint);
    summary(
        &mut out,
        "table access",
        s.table_access.added.len(),
        s.table_access.removed.len(),
    );
    let access =
        |t: &rb_store::diff::TableAccessRow| format!("{}  {}  {}", t.service, t.table, t.op);
    diff_rows(&mut out, '+', &s.table_access.added, access);
    diff_rows(&mut out, '-', &s.table_access.removed, access);
    summary(
        &mut out,
        "service dependencies",
        s.service_dependencies.added.len(),
        s.service_dependencies.removed.len(),
    );
    let dep = |x: &rb_store::diff::DependencyRow| format!("{} -> {}  via {}", x.from, x.to, x.via);
    diff_rows(&mut out, '+', &s.service_dependencies.added, dep);
    diff_rows(&mut out, '-', &s.service_dependencies.removed, dep);
    if s.certainty.is_empty() {
        out.push_str("certainty: no edge changed certainty\n");
    } else {
        let shifts: Vec<String> = s
            .certainty
            .iter()
            .map(|(k, x)| format!("{k} {} raised, {} lowered", x.raised, x.lowered))
            .collect();
        let _ = writeln!(out, "certainty: {}", shifts.join("; "));
    }
    let e = &d.exact;
    let _ = writeln!(
        out,
        "nodes: {} added, {} removed, {} changed",
        e.nodes_added.len(),
        e.nodes_removed.len(),
        e.nodes_changed.len()
    );
    let node = |n: &NodeRow| format!("{} {}", n.kind, n.key);
    diff_rows(&mut out, '+', &e.nodes_added, node);
    diff_rows(&mut out, '-', &e.nodes_removed, node);
    let node_change = |c: &Change<NodeRow>| {
        let mut parts = Vec::new();
        if c.before.name != c.after.name {
            parts.push(format!("name {:?} → {:?}", c.before.name, c.after.name));
        }
        if c.before.attrs != c.after.attrs {
            parts.push(format!(
                "attrs {} → {}",
                serde_json::Value::Object(c.before.attrs.clone()),
                serde_json::Value::Object(c.after.attrs.clone())
            ));
        }
        format!("{}: {}", node(&c.after), parts.join("; "))
    };
    diff_rows(&mut out, '~', &e.nodes_changed, node_change);
    let _ = writeln!(
        out,
        "edges: {} added, {} removed, {} changed, {} evidence only",
        e.edges_added.len(),
        e.edges_removed.len(),
        e.edges_changed.len(),
        e.edges_evidence_only.len()
    );
    let edge = |x: &EdgeRow| {
        format!(
            "{} {} -> {} ({})",
            x.kind, x.src_key, x.dst_key, x.provenance
        )
    };
    let with_certainty = |x: &EdgeRow| format!("{}  [{}]", edge(x), x.certainty.as_str());
    diff_rows(&mut out, '+', &e.edges_added, with_certainty);
    diff_rows(&mut out, '-', &e.edges_removed, with_certainty);
    let edge_change = |c: &Change<EdgeRow>| {
        let mut parts = Vec::new();
        if c.before.certainty != c.after.certainty {
            parts.push(format!(
                "certainty {} → {}",
                c.before.certainty.as_str(),
                c.after.certainty.as_str()
            ));
        }
        if c.before.attrs != c.after.attrs {
            parts.push(format!(
                "attrs {} → {}",
                serde_json::Value::Object(c.before.attrs.clone()),
                serde_json::Value::Object(c.after.attrs.clone())
            ));
        }
        if c.before.evidence != c.after.evidence {
            parts.push(format!(
                "evidence changed ({} → {} sites)",
                c.before.evidence.len(),
                c.after.evidence.len()
            ));
        }
        format!("{}: {}", edge(&c.after), parts.join("; "))
    };
    diff_rows(&mut out, '~', &e.edges_changed, edge_change);
    if evidence {
        diff_rows(&mut out, '=', &e.edges_evidence_only, edge_change);
    }
    let changed: Vec<String> = d
        .diagnostics
        .counts
        .iter()
        .filter(|(_, (b, a))| b != a)
        .map(|(k, (b, a))| format!("{k} {b} → {a}"))
        .collect();
    if changed.is_empty() {
        out.push_str("diagnostics: no kind changed count\n");
    } else {
        let _ = writeln!(out, "diagnostics: {}", changed.join(", "));
    }
    if d.list_diagnostics {
        let line = |x: &rb_model::Diagnostic| rb_model::diff::diagnostic_line(x);
        diff_rows(&mut out, '+', &d.diagnostics.added, line);
        diff_rows(&mut out, '-', &d.diagnostics.removed, line);
    }
    out
}

/// `rb status`'s text: per repository its live snapshot and counts, then its coverage lines.
fn status_text(all: bool, reports: &[rb_api::StatusReport]) -> String {
    let mut out = String::new();
    if all && reports.is_empty() {
        out.push_str("no repositories indexed yet\n");
    }
    for r in reports {
        match &r.live {
            Some(live) => {
                let _ = writeln!(
                    out,
                    "{}: live snapshot {} @ {} — {} nodes, {} edges, {} diagnostics; AGE graph {}; {} snapshot(s) kept",
                    r.name,
                    live.id,
                    short(&live.commit_sha),
                    r.nodes,
                    r.edges,
                    r.diagnostics,
                    live.age_graph.as_deref().unwrap_or("-"),
                    r.snapshots
                );
            }
            None => {
                let _ = writeln!(out, "{}: no live snapshot", r.name);
            }
        }
        if let Some(c) = &r.coverage {
            let _ = writeln!(
                out,
                "  sql: {} tables; {} code sites and {} migration statements touching them; {} unparsed",
                c.sql.tables, c.sql.query_sites, c.sql.migration_statements, c.sql.unparsed
            );
            let _ = writeln!(out, "  {}", code_line(&c.code));
            let _ = writeln!(out, "  {}", http_line(&c.http));
            let _ = writeln!(out, "  {}", di_line(&c.di));
            let _ = writeln!(out, "  {}", rta_line(&c.rta));
            if let Some(line) = excluded_line(&c.excluded) {
                let _ = writeln!(out, "  {line}");
            }
            if let Some(line) = unsupported_languages_line(&c.unsupported_languages) {
                let _ = writeln!(out, "  {line}");
            }
            if let Some(line) = unsupported_frameworks_line(&c.unsupported_frameworks) {
                let _ = writeln!(out, "  {line}");
            }
            for d in &c.scip {
                let _ = writeln!(out, "  {}", scip_status_line(d));
            }
        }
    }
    out
}

/// `excluded from analysis: <reason> <n> files, …` from the `excluded` diagnostics (contract
/// M7e S3); `None` when nothing was excluded.
fn excluded_line(excluded: &[rb_model::Diagnostic]) -> Option<String> {
    counts_line("excluded from analysis", "reason", excluded)
}

/// `not analysed (no grammar): <language> <n> files, …` from the `language_unsupported`
/// diagnostics (contract M7g S1); `None` when every code file has a grammar.
fn unsupported_languages_line(languages: &[rb_model::Diagnostic]) -> Option<String> {
    counts_line("not analysed (no grammar)", "language", languages)
}

/// `frameworks without a pack (…): <framework> in <service>, …` from the
/// `framework_unsupported` diagnostics (contract M7g S2); `None` when there are none.
fn unsupported_frameworks_line(frameworks: &[rb_model::Diagnostic]) -> Option<String> {
    let parts: Vec<String> = frameworks
        .iter()
        .map(|d| {
            format!(
                "{} in {}",
                d.detail["framework"].as_str().unwrap_or("?"),
                d.detail["service"].as_str().unwrap_or("?")
            )
        })
        .collect();
    (!parts.is_empty()).then(|| {
        format!(
            "frameworks without a pack (routes and clients not read): {}",
            parts.join(", ")
        )
    })
}

/// `<label>: <detail[key]> <n> files, …` over count diagnostics (`{key, files}`), in the
/// stored (canonical) order; `None` when there are none.
fn counts_line(label: &str, key: &str, counts: &[rb_model::Diagnostic]) -> Option<String> {
    let parts: Vec<String> = counts
        .iter()
        .map(|d| {
            let files = d.detail["files"].as_u64().unwrap_or(0);
            let unit = if files == 1 { "file" } else { "files" };
            format!("{} {files} {unit}", d.detail[key].as_str().unwrap_or("?"))
        })
        .collect();
    (!parts.is_empty()).then(|| format!("{label}: {}", parts.join(", ")))
}

/// Text rendering of the Service Card's structure sections (design §9.5).
fn render_card(c: &ServiceCard) -> String {
    let kind = match c.role {
        ServiceRole::Service => "deployable",
        ServiceRole::Library => "library",
        ServiceRole::Unassigned => "files outside every detected project",
    };
    let mut out = format!("{}  (path: {}, {kind})\n", c.name, c.key);
    if !c.detected_by.is_empty() {
        out += &format!("├─ Detected by: {}\n", c.detected_by.join(", "));
    }
    if !c.manifests.is_empty() {
        out += &format!("├─ Manifests: {}\n", c.manifests.join(", "));
    }
    let langs: Vec<String> = c
        .languages
        .iter()
        .map(|(l, n)| format!("{l} {n}"))
        .collect();
    out += &format!("├─ Files: {} ({})\n", c.files, langs.join(", "));
    let owners = if c.owners.is_empty() {
        "none".to_owned()
    } else {
        c.owners.join(", ")
    };
    out += &format!("├─ Owners: {owners}\n");
    out += "├─ File ownership\n";
    let width = c
        .file_owners
        .iter()
        .map(|o| o.owner.len())
        .max()
        .unwrap_or(0)
        .max("unowned".len());
    for o in &c.file_owners {
        out += &format!(
            "│    {:<width$}  {:>10}  {:>3}%\n",
            o.owner,
            files(o.files),
            percent(o.files, c.files)
        );
    }
    out += &format!(
        "│    {:<width$}  {:>10}  {:>3}%\n",
        "unowned",
        files(c.unowned_files),
        percent(c.unowned_files, c.files)
    );
    // The HTTP sections (contract M4a S20), then the derived ones (contract M4b S18), the
    // last one closing the tree.
    let mut sections = vec![
        section("APIs", c.apis.iter().map(api_line).collect()),
        section("Calls APIs of", peer_lines(&c.consumes)),
        section("APIs called by", peer_lines(&c.consumed_by)),
    ];
    if c.unstitched > 0 {
        sections.push((
            format!(
                "HTTP client calls not stitched: {} (rb status)",
                c.unstitched
            ),
            Vec::new(),
        ));
    }
    sections.extend(derived_sections(&c.derived));
    let n = sections.len();
    for (i, (head, lines)) in sections.into_iter().enumerate() {
        let (branch, rail) = if i + 1 == n {
            ("└", " ")
        } else {
            ("├", "│")
        };
        out += &format!("{branch}─ {head}\n");
        for l in lines {
            out += &format!("{rail}    {l}\n");
        }
    }
    out
}

/// The card's sections of contract M4b S18: packages with versions, database tables, service
/// dependencies by via and the packages count, the inverse, shared tables (coupling), and the
/// call graph with entry points by kind.
fn derived_sections(d: &rb_store::card::CardSections) -> Vec<(String, Vec<String>)> {
    let packages = d
        .packages
        .iter()
        .map(|p| {
            let version = if p.version.is_empty() {
                "?"
            } else {
                p.version.as_str()
            };
            let mut line = format!("{} {version}", p.name);
            if let Some(from) = &p.version_from {
                let _ = write!(line, "  [{from}]");
            }
            if p.provenance.iter().any(|x| x == "scip") {
                line.push_str("  (scip)");
            }
            line
        })
        .collect();
    let tables = d
        .tables
        .iter()
        .map(|t| {
            let ops = if t.ops.is_empty() {
                "mapped".to_owned()
            } else {
                t.ops.join(", ")
            };
            let assumed = if t.schema_assumed {
                "  (schema assumed)"
            } else {
                ""
            };
            format!("{}  {ops}  [{}]{assumed}", t.key, t.certainty)
        })
        .collect();
    let dep_line = |s: &rb_store::card::ServiceDep| {
        let mut line = format!("{} ({})  via {}  [{}]", s.name, s.key, s.via, s.certainty);
        match s.via.as_str() {
            "calls" => {
                let mut parts = Vec::new();
                for (n, what) in [(s.calls, "call site"), (s.references, "reference site")] {
                    if n > 0 {
                        parts.push(format!("{n} {what}{}", if n == 1 { "" } else { "s" }));
                    }
                }
                let _ = write!(line, "  {}", parts.join(", "));
            }
            "http" => {
                let eps: Vec<&str> = s.endpoints.iter().map(|e| e.display.as_str()).collect();
                let _ = write!(line, "  {}", eps.join(", "));
            }
            _ => {}
        }
        line
    };
    let services: BTreeSet<&str> = d.dependencies.iter().map(|s| s.key.as_str()).collect();
    let dependencies = if services.is_empty() && d.packages.is_empty() {
        ("Dependencies: none".to_owned(), Vec::new())
    } else {
        let mut lines: Vec<String> = d.dependencies.iter().map(dep_line).collect();
        lines.push(format!("packages: {}", d.packages.len()));
        (
            format!(
                "Dependencies ({} service{}, {} package{})",
                services.len(),
                if services.len() == 1 { "" } else { "s" },
                d.packages.len(),
                if d.packages.len() == 1 { "" } else { "s" }
            ),
            lines,
        )
    };
    let shares = d
        .shares_tables
        .iter()
        .map(|s| format!("{} ({})  {}", s.name, s.key, s.tables.join(", ")))
        .collect();
    let g = &d.call_graph;
    let kinds: Vec<String> = g.edges.iter().map(|(k, n)| format!("{k} {n}")).collect();
    let total: usize = g.certainty.values().sum();
    let mut shares_by: Vec<(&String, &usize)> = g.certainty.iter().collect();
    shares_by.sort_by_key(|(c, _)| {
        std::cmp::Reverse(code::parse_certainty(c).unwrap_or(rb_model::Certainty::Ambiguous))
    });
    let certainty: Vec<String> = shares_by
        .iter()
        .map(|(c, n)| format!("{c} {:.1}%", **n as f64 * 100.0 / total as f64))
        .collect();
    let entries: Vec<String> = d
        .entry_points
        .iter()
        .map(|(k, n)| format!("{k} {n}"))
        .collect();
    let call_graph = vec![
        format!("{} symbols · {}", g.symbols, kinds.join(", ")),
        format!(
            "certainty: {}",
            if certainty.is_empty() {
                "-".to_owned()
            } else {
                certainty.join(", ")
            }
        ),
        format!(
            "entry points: {}",
            if entries.is_empty() {
                "none".to_owned()
            } else {
                entries.join(", ")
            }
        ),
    ];
    vec![
        section("Packages", packages),
        section("Database tables", tables),
        dependencies,
        section(
            "Depended on by",
            d.depended_on_by.iter().map(dep_line).collect(),
        ),
        section("Shares tables with", shares),
        (
            "Call graph (edges within the service)".to_owned(),
            call_graph,
        ),
    ]
}

/// A card section: `Title (n)` with its lines, or `Title: none`. Grouped sections count their
/// groups (the lines not indented).
fn section(title: &str, lines: Vec<String>) -> (String, Vec<String>) {
    let n = lines.iter().filter(|l| !l.starts_with(' ')).count();
    if n == 0 {
        (format!("{title}: none"), lines)
    } else {
        (format!("{title} ({n})"), lines)
    }
}

/// `POST /api/v1/login  → AuthController#login().  [exact]`: an endpoint, its handlers and the
/// handler certainty (the path's when no handler was resolved).
fn api_line(a: &rb_store::Api) -> String {
    let handlers: Vec<String> = a
        .handlers
        .iter()
        .map(|h| match h.kind.as_str() {
            "file" => format!("{} (top-level code)", h.key),
            _ => rb_store::code::symbol_display(&h.key).to_owned(),
        })
        .collect();
    let certainty = a
        .handlers
        .iter()
        .filter_map(|h| code::parse_certainty(&h.certainty).ok())
        .max()
        .map_or_else(|| a.certainty.clone(), |c| c.as_str().to_owned());
    let mut line = if handlers.is_empty() {
        format!("{}  → (handler unresolved)  [{certainty}]", a.name)
    } else {
        format!("{}  → {}  [{certainty}]", a.name, handlers.join(", "))
    };
    match (&a.router, a.prefix_unknown) {
        (Some(router), _) => {
            let _ = write!(line, "  (prefix unknown, router {router})");
        }
        (None, true) => line.push_str("  (prefix unknown)"),
        (None, false) => {}
    }
    line
}

/// `service  (key)` per peer, then `  ENDPOINT  [certainty]  n sites` per endpoint.
fn peer_lines(peers: &[rb_store::HttpPeer]) -> Vec<String> {
    let mut lines = Vec::new();
    for p in peers {
        lines.push(format!("{}  ({})", p.service_name, p.service_key));
        for u in &p.endpoints {
            let sites = if u.sites == 1 { "site" } else { "sites" };
            lines.push(format!(
                "  {}  [{}]  {} {sites}",
                u.name, u.certainty, u.sites
            ));
        }
    }
    lines
}

/// "1 file", "3 files".
fn files(n: usize) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{n} files")
    }
}

/// Text rendering of `rb table-deps` (design §9.4, symbol level).
fn render_table_deps(d: &TableDeps) -> String {
    let access_line = |a: &TableAccess| {
        // Mappings carry no operations.
        let ops = if a.ops.is_empty() {
            String::new()
        } else {
            format!("{}  ", a.ops.join("+"))
        };
        let assumed = if a.schema_assumed {
            ", schema assumed"
        } else {
            ""
        };
        format!(
            "{}  {ops}{}:{}  [{}{assumed}]",
            source_label(a),
            a.file,
            a.line,
            a.certainty
        )
    };
    let mut out = format!("{}  (table {})\n", d.key, d.name);
    if let Ok(p) = code::parse_precision(&d.rollup.precision) {
        out += &code::precision_lines(p, d.rollup.pruned_by_rta);
    }
    out += "├─ Migrations\n";
    if d.migrations.is_empty() {
        out += "│    none (not created by any .sql file in this repo)\n";
    }
    for m in &d.migrations {
        out += &format!("│    {}  ({})\n", access_line(m), m.service_name);
    }
    if !d.mappings.is_empty() {
        out += "├─ Mapped by\n";
        for m in &d.mappings {
            out += &format!("│    {}  ({})\n", access_line(m), m.service_name);
        }
    }
    out += "├─ Accessed by\n";
    if d.accesses.is_empty() {
        out += "│    no code in this repo\n";
    }
    let mut current = None;
    for a in &d.accesses {
        if current != Some(&a.service_key) {
            out += &format!("│    {} ({})\n", a.service_name, a.service_key);
            current = Some(&a.service_key);
        }
        out += &format!("│      {}\n", access_line(a));
    }
    // Q3's roll-up (contract M4b S14): services by their code's reach, then the services that
    // reach the table only through another service's API.
    let r = &d.rollup;
    let services = section(
        "Services (callers up to entry points)",
        r.services.iter().flat_map(service_reach_lines).collect(),
    );
    let apis = section(
        "Through APIs",
        r.through_apis
            .iter()
            .map(|a| {
                let chain: Vec<String> = a
                    .chain
                    .iter()
                    .map(|c| format!("{} of {}", c.display, c.service_name))
                    .collect();
                format!(
                    "{} ({})  via {}  [{}]",
                    a.name,
                    a.key,
                    chain.join(", then "),
                    a.certainty
                )
            })
            .collect(),
    );
    for (i, (head, lines)) in [services, apis].into_iter().enumerate() {
        let (branch, rail) = if i == 1 { ("└", " ") } else { ("├", "│") };
        out += &format!("{branch}─ {head}\n");
        for l in lines {
            out += &format!("{rail}    {l}\n");
        }
    }
    if !r.truncated.is_empty() {
        out += &format!("truncated: {}\n", r.truncated.join(", "));
    }
    out
}

/// A service of Q3's roll-up: `name (key)  ops  direct|indirect via …  [certainty]`, then its
/// endpoints and other entry points, indented.
fn service_reach_lines(s: &rb_store::rollup::ServiceReach) -> Vec<String> {
    let how = if s.direct {
        "direct".to_owned()
    } else {
        format!("indirect via {}", s.via.join(", "))
    };
    let mut lines = vec![format!(
        "{} ({})  {}  {how}  [{}]",
        s.name,
        s.key,
        s.ops.join("+"),
        s.certainty
    )];
    for e in &s.endpoints {
        lines.push(format!("  endpoint {}  [{}]", e.display, e.certainty));
    }
    if !s.entry_points.is_empty() {
        let kinds: Vec<String> = s
            .entry_points
            .iter()
            .map(|(k, n)| format!("{k} {n}"))
            .collect();
        lines.push(format!("  entry points: {}", kinds.join(", ")));
    }
    lines
}

/// A symbol's descriptors (e.g. `UserDao#emailOf()`), which carry its class; a file's name.
fn source_label(a: &TableAccess) -> &str {
    match a.source_key.rsplit_once("`/") {
        Some((_, descriptors)) if a.source_kind == "symbol" => descriptors.trim_end_matches('.'),
        _ => &a.source_name,
    }
}

/// Integer percentage rounded half up; 0 when `total` is 0.
fn percent(part: usize, total: usize) -> usize {
    if total == 0 {
        0
    } else {
        (part * 200 + total) / (total * 2)
    }
}

/// One `rb check` violation line: `rule src -> dst`, sorted by (rule, src, dst) by construction.
fn check_line(rule: &str, src: &str, dst: &str) -> String {
    format!("{rule} {src} -> {dst}")
}

/// Renders `rb check` output: the sorted violation lines plus the summary, or the JSON
/// envelope. Pure function of the rows (tested without a database).
fn format_check(violations: &[(String, String, String)], truncated: bool, json: bool) -> String {
    if json {
        serde_json::json!({
            "rules": violations
                .iter()
                .map(|(rule, src, dst)| serde_json::json!({
                    "rule": rule,
                    "src": src,
                    "dst": dst,
                }))
                .collect::<Vec<_>>(),
            "truncated": truncated,
        })
        .to_string()
    } else {
        let mut out = String::new();
        for (rule, src, dst) in violations {
            out.push_str(&check_line(rule, src, dst));
            out.push('\n');
        }
        out.push_str(&format!(
            "check: {} violation{}",
            violations.len(),
            if violations.len() == 1 { "" } else { "s" }
        ));
        if truncated {
            out.push_str(" (truncated: rows)");
        }
        out.push('\n');
        out
    }
}

/// The rows of one `rb check` run: sorted unique `(rule, src, dst)` triples plus whether any
/// rule's row list was cut by the cap.
struct CheckOutcome {
    violations: Vec<(String, String, String)>,
    truncated: bool,
    /// What the orphan rule cannot see, when it ran ([`unlinked_note`]).
    note: Option<String>,
    /// Violations the baseline accepted, when one was given.
    accepted: Option<usize>,
}

/// The violations a baseline accepts: the `rule src -> dst` lines `rb check` prints
/// ([`check_line`]); its summary, comments and anything else are ignored (contract M7g S3).
fn parse_baseline(text: &str) -> BTreeSet<(String, String, String)> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let (rule, rest) = l.split_once(' ')?;
            let (src, dst) = rest.split_once(" -> ")?;
            rb_store::check::RULES
                .iter()
                .any(|r| r.name == rule)
                .then(|| (rule.to_owned(), src.to_owned(), dst.to_owned()))
        })
        .collect()
}

/// `violations` without the accepted ones, and how many were accepted.
fn apply_baseline(
    violations: Vec<(String, String, String)>,
    accepted: &BTreeSet<(String, String, String)>,
) -> (Vec<(String, String, String)>, usize) {
    let total = violations.len();
    let kept: Vec<_> = violations
        .into_iter()
        .filter(|v| !accepted.contains(v))
        .collect();
    let n = total - kept.len();
    (kept, n)
}

/// The caveat for `orphan_endpoints` from a snapshot's `http_coverage` counts: client sites
/// linked to no endpoint (dynamic, unmatched, ambiguous or too ambiguous) may call an endpoint
/// the rule reports; `None` when every site is linked.
fn unlinked_note(coverage: &serde_json::Value) -> Option<String> {
    let count = |k: &str| coverage[k].as_u64().unwrap_or(0);
    let parts: Vec<String> = ["dynamic", "unmatched", "ambiguous", "too_ambiguous"]
        .iter()
        .filter(|k| count(k) > 0)
        .map(|k| format!("{} {}", k.replace('_', " "), count(k)))
        .collect();
    let total: u64 = ["dynamic", "unmatched", "ambiguous", "too_ambiguous"]
        .iter()
        .map(|k| count(k))
        .sum();
    (total > 0).then(|| {
        format!(
            "note: {total} client call sites are linked to no endpoint ({}); an endpoint reported orphan may still be called through them",
            parts.join(", ")
        )
    })
}

/// Runs the built-in architecture rules' Cypher queries against the live snapshot (contract M6h
/// S3–S4): the shared read-only `cypher()` path carries the timeout and row cap, so rules can
/// neither write nor hang the database. Exit 0 when clean, 1 on violations, 2 on errors.
fn check_rules(
    store: &mut Store,
    repo: Option<String>,
    rule: Option<String>,
    json: bool,
    timeout_ms: u64,
    baseline: Option<&Path>,
) -> anyhow::Result<ExitCode> {
    let outcome = (|| -> anyhow::Result<CheckOutcome> {
        let snap = live_snapshot(store, repo)?;
        let accepted: BTreeSet<(String, String, String)> = match baseline {
            Some(path) => parse_baseline(
                &std::fs::read_to_string(path)
                    .with_context(|| format!("reading baseline {}", path.display()))?,
            ),
            None => BTreeSet::new(),
        };
        let selected: Vec<&rb_store::check::Rule> = match &rule {
            Some(name) => vec![
                rb_store::check::RULES
                    .iter()
                    .find(|r| r.name == name)
                    .with_context(|| format!("unknown rule {name:?}"))?,
            ],
            None => rb_store::check::RULES.iter().collect(),
        };
        let orphan_rule = selected
            .iter()
            .any(|r| r.name == rb_store::check::RULE_ORPHAN_ENDPOINTS);
        let limits = rb_store::CypherLimits {
            timeout_ms,
            rows: rb_store::CYPHER_ROWS_MAX,
        };
        let mut violations = Vec::new();
        let mut truncated = false;
        for r in selected {
            let rows = store.cypher(
                snap.id,
                r.cypher,
                &["src".to_owned(), "dst".to_owned()],
                limits,
            )?;
            truncated |= !rows.truncated.is_empty();
            for row in rows.rows {
                violations.push((r.name.to_owned(), row[0].clone(), row[1].clone()));
            }
        }
        violations.sort();
        violations.dedup();
        // The client sites the orphan rule cannot see (contract M7g S3).
        let note = if orphan_rule {
            store
                .load_diagnostics(snap.id)?
                .iter()
                .find(|d| d.kind == "http_coverage")
                .and_then(|d| unlinked_note(&d.detail))
        } else {
            None
        };
        let (violations, accepted) = apply_baseline(violations, &accepted);
        Ok(CheckOutcome {
            violations,
            truncated,
            note,
            accepted: baseline.map(|_| accepted),
        })
    })();
    let CheckOutcome {
        violations,
        truncated,
        note,
        accepted,
    } = match outcome {
        Ok(v) => v,
        // Errors exit 2, never 1 (violations), through the shared rendering (contract M7f S3).
        Err(e) => return Ok(report_failure(e, json, true)),
    };
    // A cut row list would silently pass a violating snapshot: say so loudly (contract M6h S4).
    if truncated {
        eprintln!(
            "warning: rule rows truncated at {}",
            rb_store::CYPHER_ROWS_MAX
        );
    }
    if let Some(note) = note {
        eprintln!("{note}");
    }
    if let Some(n) = accepted {
        eprintln!(
            "baseline: {n} accepted violation{} not reported",
            if n == 1 { "" } else { "s" }
        );
    }
    print!("{}", format_check(&violations, truncated, json));
    Ok(if violations.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn live_snapshot(store: &mut Store, repo: Option<String>) -> anyhow::Result<SnapshotInfo> {
    let repo = match repo {
        Some(r) => r,
        None => match cwd_repo_name() {
            Some(name) if is_indexed(store, &name) => name,
            name => return Err(repo_needed(name.as_deref(), &cwd_display()).into()),
        },
    };
    // Typed like every question's errors (contract M7f S3): `not_found` and `no_live_snapshot`.
    if !store.repo_names()?.contains(&repo) {
        return Err(ApiError::not_found(format!("repository {repo:?} is not indexed")).into());
    }
    match store.live_snapshot(&repo)? {
        Some(snap) => Ok(snap),
        None => Err(ApiError::new(
            rb_api::ErrorKind::NoLiveSnapshot,
            format!("repository {repo:?} has no live snapshot"),
        )
        .into()),
    }
}

/// Derives a valid repo name from a directory name: lowercase, `[a-z0-9_]`, max 40 characters.
fn repo_name_from_path(path: &Path) -> String {
    let raw = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut name = String::new();
    for c in raw.chars() {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            c
        } else {
            '_'
        };
        if !(c == '_' && name.ends_with('_')) {
            name.push(c);
        }
    }
    let name: String = name.trim_matches('_').chars().take(40).collect();
    if name.is_empty() { "repo".into() } else { name }
}

/// The nearest ancestor of `start` (or `start` itself) holding `.git`, as a
/// directory or a file (worktrees and submodules keep a `.git` file).
fn git_root(start: &Path) -> Option<PathBuf> {
    let mut cur: Option<&Path> = Some(start);
    while let Some(dir) = cur {
        if dir.join(".git").symlink_metadata().is_ok() {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

/// The `--repo` directive when no repository can be inferred (contract M7f S1): `checkout` is the
/// unindexed checkout's name, `None` outside any git checkout (then `cwd` is named).
fn repo_needed(checkout: Option<&str>, cwd: &str) -> ApiError {
    let why = match checkout {
        Some(name) => format!(
            "no indexed repository matches this checkout ({name:?}); index it (`rb index .`) or pass --repo"
        ),
        None => format!(
            "no git repository found in {cwd:?}; run rb from inside an indexed git checkout (or pass --repo)"
        ),
    };
    ApiError::invalid("repo", format!("is needed: {why}"))
}

/// The working directory as the directive names it.
fn cwd_display() -> String {
    std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into())
}

/// The repository implied by the working directory: the enclosing git root's
/// directory name, sanitised exactly like `index` names it. No existence
/// check: callers decide between using it and reporting it.
fn cwd_repo_name() -> Option<String> {
    let root = git_root(&std::env::current_dir().ok()?)?;
    Some(repo_name_from_path(&root))
}

/// Whether a repository of this name is indexed (store errors count as no).
fn is_indexed(store: &mut Store, name: &str) -> bool {
    store
        .repo_names()
        .map(|names| names.iter().any(|r| r == name))
        .unwrap_or(false)
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentages_round_half_up() {
        assert_eq!(percent(1, 3), 33);
        assert_eq!(percent(2, 3), 67);
        assert_eq!(percent(1, 8), 13);
        assert_eq!(percent(0, 0), 0);
    }

    #[test]
    fn file_counts_pluralise() {
        assert_eq!(files(0), "0 files");
        assert_eq!(files(1), "1 file");
        assert_eq!(files(12), "12 files");
    }

    /// Contract M5 S5: the `--no-cache` facts cache answers no load and forwards every save.
    #[test]
    fn write_only_facts_cache_forwards_saves_only() {
        #[derive(Default)]
        struct Recorder {
            loads: usize,
            saved: Vec<String>,
        }
        impl rb_indexer::FactsCache for Recorder {
            fn load(
                &mut self,
                _: &[rb_indexer::FactsKey],
            ) -> anyhow::Result<HashMap<rb_indexer::FactsKey, rb_syntax::FileFacts>> {
                self.loads += 1;
                Ok(HashMap::new())
            }
            fn save(
                &mut self,
                facts: &[(rb_indexer::FactsKey, rb_syntax::FileFacts)],
            ) -> anyhow::Result<()> {
                self.saved.extend(facts.iter().map(|(k, _)| k.blob.clone()));
                Ok(())
            }
        }
        let mut inner = Recorder::default();
        let key = rb_indexer::FactsKey {
            blob: "b1".into(),
            language: rb_syntax::Language::Go,
        };
        let facts = rb_syntax::extract(rb_syntax::Language::Go, b"package p\n").unwrap();
        let mut cache = WriteOnlyFactsCache(&mut inner);
        use rb_indexer::FactsCache as _;
        assert!(cache.load(std::slice::from_ref(&key)).unwrap().is_empty());
        cache.save(&[(key, facts)]).unwrap();
        assert_eq!((inner.loads, inner.saved), (0, vec!["b1".to_owned()]));
    }

    /// Contract M5 S6: `rb index --profile` prints one line per loaded file and the totals.
    #[test]
    fn projection_lines_list_files_and_totals() {
        let file = |file: &str, label, rows, bytes, ms| rb_store::LoadedFile {
            file: file.into(),
            label,
            rows,
            bytes,
            load: Duration::from_millis(ms),
        };
        let p = rb_store::ProjectionProfile {
            files: vec![
                file("File.csv", "File", 3, 120, 4),
                file("QUERIES.ops.csv", "QUERIES", 2, 90, 6),
            ],
        };
        assert_eq!(
            projection_lines(&p),
            "  profile: age File (File.csv): 3 rows, 120 bytes, 4.0 ms\n\
             \x20 profile: age QUERIES (QUERIES.ops.csv): 2 rows, 90 bytes, 6.0 ms\n\
             \x20 profile: age total: 2 files, 5 rows, 210 bytes, 10.0 ms\n"
        );
    }

    /// Contract M6h S3: `rb check` violation lines name the rule, source and target.
    #[test]
    fn check_lines_name_rule_source_and_target() {
        assert_eq!(
            check_line(
                "orphan_endpoints",
                "services/auth",
                "HTTP GET /x @services/auth"
            ),
            "orphan_endpoints services/auth -> HTTP GET /x @services/auth"
        );
    }

    /// Contract M6h S4: truncated row lists say so loudly, in both formats; empty stays clean.
    #[test]
    fn check_output_marks_truncation() {
        let violations = vec![("r".to_owned(), "a".to_owned(), "b".to_owned())];
        assert_eq!(
            format_check(&violations, false, false),
            "r a -> b\ncheck: 1 violation\n"
        );
        assert!(format_check(&violations, true, false).contains("(truncated: rows)"));
        assert_eq!(format_check(&[], false, false), "check: 0 violations\n");
        let json = format_check(&violations, true, true);
        let body: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(body["truncated"], true);
        assert_eq!(body["rules"].as_array().unwrap().len(), 1);
    }

    /// Contract M5 S4: `rb gc`'s lines name each graph's owner or its absence.
    #[test]
    fn gc_lines_name_what_is_removed() {
        let mut plan = rb_store::GcPlan::default();
        assert_eq!(gc_lines(&plan, true), "gc: nothing to remove\n");
        plan.graphs = vec![
            rb_store::GcGraph {
                name: "rb_x_7".into(),
                snapshot: Some(7),
                repo: Some("x".into()),
                status: Some("retired".into()),
            },
            rb_store::GcGraph {
                name: "rb_x_9".into(),
                snapshot: None,
                repo: None,
                status: None,
            },
        ];
        plan.building = vec![rb_store::GcSnapshot {
            id: 8,
            repo: "x".into(),
            commit_sha: "a".repeat(40),
        }];
        assert_eq!(
            gc_lines(&plan, false),
            "dropped graph rb_x_7: snapshot 7 of x (retired)\n\
             dropped graph rb_x_9: no snapshot names it\n\
             dropped building snapshot 8 of x @ aaaaaaaaaaaa\n\
             gc: dropped 2 graph(s) and 1 building row(s)\n"
        );
    }

    /// Contract M5 S3: `--keep` is bounded at parse time (0-20).
    #[test]
    fn keep_is_validated_at_parse_time() {
        for bad in ["-1", "21"] {
            assert!(
                Cli::try_parse_from(["rb", "index", ".", "--keep", bad]).is_err(),
                "{bad}"
            );
        }
        for good in ["0", "20"] {
            assert!(Cli::try_parse_from(["rb", "index", ".", "--keep", good]).is_ok());
        }
    }

    /// Contract M5 S12: question flags reach the request layer as text, which alone validates
    /// them; its refusal names the flag in the CLI's text.
    #[test]
    fn question_flags_are_validated_by_the_request_layer() {
        let parse = |args: &[&str]| {
            let cli = Cli::try_parse_from(args).unwrap();
            let (endpoint, repo, params, _) = asked(cli.command).unwrap();
            rb_api::parse(
                endpoint,
                repo.as_deref(),
                &params,
                &rb_api::Settings::default(),
            )
        };
        for (args, flag) in [
            (
                &["rb", "find", "x", "--limit", "1001"][..],
                "--limit must be between 1 and 1000 (got 1001)",
            ),
            (
                &["rb", "find", "x", "--limit", "0"],
                "--limit must be between 1 and 1000 (got 0)",
            ),
            (
                &["rb", "callers", "x", "--depth", "26"],
                "--depth must be between 1 and 25 (got 26)",
            ),
            (
                &["rb", "path", "x", "--hops", "9"],
                "--hops must be between 0 and 8 (got 9)",
            ),
            (
                &["rb", "table-deps", "t", "--hops=-1"],
                "--hops must be an integer between 0 and 8 (got \"-1\")",
            ),
            (
                &["rb", "cypher", "RETURN 1", "--timeout-ms", "30001"],
                "--timeout-ms must be between 1 and 30000 (got 30001)",
            ),
            (
                &["rb", "impact", "x", "--precision", "vta"],
                "--precision must be one of rta, cha (got \"vta\")",
            ),
        ] {
            let e = parse(args).unwrap_err();
            assert_eq!(e.kind.exit_code(), 2, "{args:?}");
            assert_eq!(e.cli_text(), flag, "{args:?}");
        }
        for args in [
            &["rb", "find", "x", "--limit", "1000"][..],
            &[
                "rb",
                "callers",
                "x",
                "--depth",
                "25",
                "--include-references",
                "--pick",
                "2",
            ],
            &[
                "rb", "path", "GET /", "--hops", "0", "--full", "--format", "json",
            ],
            &["rb", "service"],
            &["rb", "service", ".", "--json"],
            &[
                "rb",
                "cypher",
                "RETURN 1",
                "--cols",
                "a,b",
                "--timeout-ms",
                "30000",
            ],
        ] {
            assert!(parse(args).is_ok(), "{args:?}");
        }
        assert!(asked(Cli::try_parse_from(["rb", "migrate"]).unwrap().command).is_none());
    }

    /// Contract M7f S2+S3 (stress F5/F6): a refused request or an unreachable database exits by
    /// its kind (`unavailable` 6, `not_found` 3); `rb check` exits 2 on any error, so an outage
    /// is never read as violations (1); other failures keep exit 1.
    #[test]
    fn failures_exit_by_kind_and_check_errors_exit_2() {
        let down = || {
            anyhow::Error::from(rb_store::Unavailable {
                url: "postgresql://rb:***@127.0.0.1:1/rb".into(),
                cause: "Connection refused".into(),
            })
        };
        assert_eq!(report_failure(down(), false, false), ExitCode::from(6));
        assert_eq!(report_failure(down(), true, true), ExitCode::from(2));
        let missing =
            || anyhow::Error::from(ApiError::not_found("repository \"x\" is not indexed"));
        assert_eq!(report_failure(missing(), false, false), ExitCode::from(3));
        assert_eq!(report_failure(missing(), false, true), ExitCode::from(2));
        assert_eq!(
            report_failure(anyhow::anyhow!("disk full"), false, false),
            ExitCode::FAILURE
        );
        assert_eq!(
            report_failure(anyhow::anyhow!("disk full"), false, true),
            ExitCode::from(2)
        );
        // Which commands asked for JSON, known before the command runs.
        let wants = |args: &[&str]| wants_json(&Cli::try_parse_from(args).unwrap().command);
        assert!(wants(&["rb", "check", "--json"]));
        assert!(wants(&["rb", "status", "--json"]));
        assert!(wants(&["rb", "path", "x", "--format", "json"]));
        assert!(!wants(&["rb", "callers", "x"]));
        assert!(!wants(&["rb", "migrate"]));
    }

    /// Contract M7g S3 (stress F24): `rb check`'s own text output is its baseline — every
    /// violation line `check_line` writes reads back (endpoint keys hold spaces), the summary
    /// and comments do not — and the caveat names the client sites the gate cannot see.
    #[test]
    fn check_output_is_its_own_baseline_and_names_unlinked_sites() {
        let violations = vec![
            (
                "orphan_endpoints".to_owned(),
                ".".to_owned(),
                "HTTP DELETE /api/v1/articles/{} @.".to_owned(),
            ),
            ("service_cycles".to_owned(), "a".to_owned(), "b".to_owned()),
        ];
        let text = format!(
            "# accepted 2026-10-13\nmoved svc -> other (not a rule)\n{}",
            format_check(&violations, false, false)
        );
        let baseline = parse_baseline(&text);
        assert_eq!(
            baseline,
            violations.iter().cloned().collect::<BTreeSet<_>>()
        );
        let (kept, accepted) = apply_baseline(
            vec![
                violations[0].clone(),
                (
                    "orphan_endpoints".to_owned(),
                    ".".to_owned(),
                    "HTTP GET /new @.".to_owned(),
                ),
            ],
            &baseline,
        );
        assert_eq!(accepted, 1);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].2, "HTTP GET /new @.");
        let coverage = json!({"sites": 50, "dynamic": 3, "unmatched": 11, "ambiguous": 2, "too_ambiguous": 0, "heuristic": 30});
        assert_eq!(
            unlinked_note(&coverage).as_deref(),
            Some(
                "note: 16 client call sites are linked to no endpoint (dynamic 3, unmatched 11, ambiguous 2); an endpoint reported orphan may still be called through them"
            )
        );
        assert_eq!(unlinked_note(&json!({"sites": 4, "heuristic": 4})), None);
    }

    /// Contract M7f S7: `rb path --json` is `--format json`, as `--json` is on every other
    /// question; without either it is text.
    #[test]
    fn path_takes_json_like_every_other_question() {
        let out = |args: &[&str]| {
            let (_, _, _, out) = asked(Cli::try_parse_from(args).unwrap().command).unwrap();
            matches!(out, Out::Json)
        };
        assert!(out(&["rb", "path", "GET /x", "--json"]));
        assert!(out(&["rb", "path", "GET /x", "--format", "json"]));
        assert!(!out(&["rb", "path", "GET /x"]));
    }

    /// Contract M5 D18: a page that is not the last names the rows shown and the next offset.
    #[test]
    fn more_line_names_the_next_offset() {
        let page = |offset, next| {
            Answer::Find(rb_api::Paged::from_rows(
                25,
                rb_api::Page { limit: 8, offset },
                vec![
                    rb_store::FoundNode {
                        kind: "file".into(),
                        key: "k".into(),
                        name: "n".into(),
                    };
                    next
                ],
            ))
        };
        assert_eq!(
            more_line(&page(8, 8)).as_deref(),
            Some("rows 9-16 of 25 listed; next page: --offset 16")
        );
        assert_eq!(more_line(&page(24, 1)), None, "the last page");
    }

    /// Contract M3b amendment 41: an all-SCIP repository has no heuristic site, so the line
    /// says so instead of rendering 0/0 as 100%.
    #[test]
    fn code_line_without_heuristic_sites() {
        let none = rb_store::ResolutionCoverage {
            resolved: 0,
            ambiguous: 0,
            unresolved: 0,
            external: 1220,
            exact_calls: 256,
        };
        assert_eq!(
            code_line(&none),
            "code: no heuristic references to own code (1220 external); 256 exact calls (scip)"
        );
        let some = rb_store::ResolutionCoverage {
            resolved: 3,
            ambiguous: 1,
            unresolved: 1,
            external: 2,
            exact_calls: 0,
        };
        assert_eq!(
            code_line(&some),
            "code: 80.0% of references to own code resolved (3 resolved, 1 ambiguous, 1 unresolved; 2 external); 0 exact calls (scip)"
        );
    }

    /// Contract M4a S14: zero buckets are omitted and the order is fixed, whatever the order of
    /// the stored counts.
    #[test]
    fn http_line_orders_buckets_and_omits_zeros() {
        let mut c = rb_store::HttpCoverage {
            routes: 3,
            services: 2,
            sites: 0,
            buckets: Default::default(),
        };
        assert_eq!(
            http_line(&c),
            "http: 3 routes in 2 services; 0 client calls"
        );
        for (bucket, n) in [
            ("self", 1),
            ("dynamic", 2),
            ("external", 1),
            ("unmatched", 0),
            ("too_ambiguous", 1),
            ("ambiguous", 2),
            ("heuristic", 1),
            ("synthetic", 2),
        ] {
            c.buckets.insert(bucket.into(), n);
            c.sites += n;
        }
        assert_eq!(
            http_line(&c),
            "http: 3 routes in 2 services; 10 client calls: 2 synthetic, 1 heuristic, 2 ambiguous, 1 too ambiguous, 1 external, 2 dynamic, 1 self"
        );
    }

    /// Contract M4b S9.
    #[test]
    fn rta_line_counts_types_and_edges() {
        let c = rb_store::RtaCoverage {
            types: 7,
            proven: 2,
            prunable_edges: 5,
            kept: Default::default(),
        };
        assert_eq!(
            rta_line(&c),
            "rta: 2 of 7 implementing types never instantiated (5 dispatch edges prunable)"
        );
    }

    /// Contract M4b S7: zero buckets omitted, a fixed order, the points first.
    #[test]
    fn di_line_orders_buckets_and_omits_zeros() {
        let mut c = rb_store::DiCoverage::default();
        assert_eq!(di_line(&c), "di: 0 injection points");
        for (bucket, n) in [
            ("test", 1),
            ("not_a_bean", 1),
            ("optional_absent", 0),
            ("unresolved", 2),
            ("external", 1),
            ("too_ambiguous", 1),
            ("ambiguous", 1),
            ("capped", 3),
            ("plain", 2),
        ] {
            c.buckets.insert(bucket.into(), n);
            c.points += n;
        }
        assert_eq!(
            di_line(&c),
            "di: 12 injection points: 2 plain, 3 capped, 1 ambiguous, 1 too ambiguous, 1 external, 2 unresolved, 1 not a bean, 1 test"
        );
    }

    /// Contract M4a S20: the three HTTP sections replace the M4 placeholder; an unresolved
    /// handler and an unknown prefix are said so; the unstitched line closes the tree when
    /// there is one.
    #[test]
    fn card_http_sections() {
        let mut card = ServiceCard {
            key: "services/auth".into(),
            name: "auth-service".into(),
            role: ServiceRole::Service,
            detected_by: vec![],
            manifests: vec![],
            files: 1,
            languages: vec![("java".into(), 1)],
            owners: vec![],
            file_owners: vec![],
            unowned_files: 1,
            apis: vec![],
            consumes: vec![],
            consumed_by: vec![],
            unstitched: 0,
            derived: rb_store::card::CardSections {
                packages: vec![],
                tables: vec![],
                dependencies: vec![],
                depended_on_by: vec![],
                shares_tables: vec![],
                call_graph: rb_store::card::CallGraph {
                    symbols: 0,
                    edges: Default::default(),
                    certainty: Default::default(),
                },
                entry_points: Default::default(),
            },
        };
        // The HTTP sections, between the file ownership and the derived sections.
        let tail = |card: &ServiceCard| -> Vec<String> {
            render_card(card)
                .lines()
                .skip_while(|l| !l.starts_with("├─ APIs"))
                .take_while(|l| !l.starts_with("├─ Packages"))
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(
            tail(&card),
            [
                "├─ APIs: none",
                "├─ Calls APIs of: none",
                "├─ APIs called by: none"
            ]
        );
        let handler = |key: &str, kind: &str, c: &str| rb_store::Handler {
            kind: kind.into(),
            key: key.into(),
            certainty: c.into(),
        };
        card.apis = vec![
            rb_store::Api {
                key: "HTTP GET /a @services/auth".into(),
                name: "GET /a".into(),
                certainty: "heuristic".into(),
                prefix_unknown: true,
                router: Some("a.ts#r".into()),
                handlers: vec![],
            },
            rb_store::Api {
                key: "HTTP POST /b @services/auth".into(),
                name: "POST /b".into(),
                certainty: "exact".into(),
                prefix_unknown: false,
                router: None,
                handlers: vec![
                    handler("rb-heuristic . . . `A.java`/A#b().", "symbol", "ambiguous"),
                    handler("src/server.ts", "file", "ambiguous"),
                ],
            },
        ];
        card.consumes = vec![rb_store::HttpPeer {
            service_key: "services/notification".into(),
            service_name: "notification".into(),
            endpoints: vec![rb_store::HttpUse {
                key: "HTTP POST /m @services/notification".into(),
                name: "POST /m".into(),
                certainty: "synthetic".into(),
                sites: 2,
            }],
        }];
        card.unstitched = 3;
        assert_eq!(
            tail(&card),
            [
                "├─ APIs (2)",
                "│    GET /a  → (handler unresolved)  [heuristic]  (prefix unknown, router a.ts#r)",
                "│    POST /b  → A#b()., src/server.ts (top-level code)  [ambiguous]",
                "├─ Calls APIs of (1)",
                "│    notification  (services/notification)",
                "│      POST /m  [synthetic]  2 sites",
                "├─ APIs called by: none",
                "├─ HTTP client calls not stitched: 3 (rb status)",
            ]
        );
        // The derived sections (contract M4b S18), each `none` when empty, the call graph
        // closing the tree.
        let derived = |card: &ServiceCard| -> Vec<String> {
            render_card(card)
                .lines()
                .skip_while(|l| !l.starts_with("├─ Packages"))
                .map(str::to_owned)
                .collect()
        };
        assert_eq!(
            derived(&card),
            [
                "├─ Packages: none",
                "├─ Database tables: none",
                "├─ Dependencies: none",
                "├─ Depended on by: none",
                "├─ Shares tables with: none",
                "└─ Call graph (edges within the service)",
                "     0 symbols · ",
                "     certainty: -",
                "     entry points: none",
            ]
        );
        use rb_store::card::{EndpointRef, PackageUse, ServiceDep, SharedTables, TableUse};
        let d = &mut card.derived;
        d.packages = vec![
            PackageUse {
                key: "npm/x@1.0.0".into(),
                name: "x".into(),
                version: "1.0.0".into(),
                version_from: Some("lockfile".into()),
                provenance: vec!["manifest".into(), "scip".into()],
            },
            PackageUse {
                key: "maven/a/b@?".into(),
                name: "a/b".into(),
                version: String::new(),
                version_from: Some("unresolved".into()),
                provenance: vec!["manifest".into()],
            },
        ];
        d.tables = vec![TableUse {
            key: "public.users".into(),
            ops: vec!["read".into(), "update".into()],
            certainty: "exact".into(),
            schema_assumed: true,
        }];
        let dep = |key: &str, via: &str, calls, refs, eps: &[&str]| ServiceDep {
            key: key.into(),
            name: key.to_uppercase(),
            via: via.into(),
            certainty: "heuristic".into(),
            calls,
            references: refs,
            endpoints: eps
                .iter()
                .map(|e| EndpointRef {
                    key: format!("HTTP {e} @{key}"),
                    display: (*e).into(),
                })
                .collect(),
        };
        d.dependencies = vec![
            dep("lib", "calls", 2, 1, &[]),
            dep("lib", "package", 0, 0, &[]),
            dep("n", "http", 0, 0, &["POST /m"]),
        ];
        d.depended_on_by = vec![dep("web", "http", 0, 0, &["POST /b"])];
        d.shares_tables = vec![SharedTables {
            key: "billing".into(),
            name: "BILLING".into(),
            tables: vec!["public.users".into()],
        }];
        d.call_graph.symbols = 3;
        d.call_graph.edges = [("CALLS", 2), ("IMPLEMENTS", 1)]
            .into_iter()
            .map(|(k, n)| (k.to_owned(), n))
            .collect();
        d.call_graph.certainty = [("heuristic", 1), ("exact", 2)]
            .into_iter()
            .map(|(k, n)| (k.to_owned(), n))
            .collect();
        d.entry_points = [("main", 1), ("endpoint", 2)]
            .into_iter()
            .map(|(k, n)| (k.to_owned(), n))
            .collect();
        assert_eq!(
            derived(&card),
            [
                "├─ Packages (2)",
                "│    x 1.0.0  [lockfile]  (scip)",
                "│    a/b ?  [unresolved]",
                "├─ Database tables (1)",
                "│    public.users  read, update  [exact]  (schema assumed)",
                "├─ Dependencies (2 services, 2 packages)",
                "│    LIB (lib)  via calls  [heuristic]  2 call sites, 1 reference site",
                "│    LIB (lib)  via package  [heuristic]",
                "│    N (n)  via http  [heuristic]  POST /m",
                "│    packages: 2",
                "├─ Depended on by (1)",
                "│    WEB (web)  via http  [heuristic]  POST /b",
                "├─ Shares tables with (1)",
                "│    BILLING (billing)  public.users",
                "└─ Call graph (edges within the service)",
                "     3 symbols · CALLS 2, IMPLEMENTS 1",
                "     certainty: exact 66.7%, heuristic 33.3%",
                "     entry points: endpoint 2, main 1",
            ]
        );
    }

    /// Contract M3b amendments 29, 31 and 48: the image identities are recorded only when SCIP
    /// is on (the pinned scip-typescript image by its digest reference, the local scip-java
    /// one by its ID or `absent`); a timeout prints its phase and budget.
    /// Contract M6d S1 (evaluator pass 1, E1-1): the locally built scip-go
    /// image and the pinned scip-python image are recorded too, so a rebuilt
    /// image is never served as "up to date".
    #[test]
    fn tool_versions_record_image_identities_when_scip_is_on() {
        let images = rb_scip::Images {
            java: Err("absent".into()),
            go: Err("absent".into()),
            proxy: Ok("sha256:proxy".into()),
        };
        let on = tool_versions(&rb_indexer::ScipMode::On, &images);
        assert_eq!(on["scip"], "on");
        assert_eq!(on["scip_java"], "absent");
        assert_eq!(on["scip_typescript"], rb_scip::jobs::SCIP_TYPESCRIPT_IMAGE);
        assert_eq!(on["scip_go"], "absent");
        assert_eq!(on["scip_python"], rb_scip::jobs::SCIP_PYTHON_IMAGE);
        let present = rb_scip::Images {
            java: Ok("sha256:abc".into()),
            go: Ok("sha256:abc".into()),
            proxy: Ok("sha256:proxy".into()),
        };
        assert_eq!(
            tool_versions(&rb_indexer::ScipMode::On, &present)["scip_java"],
            "sha256:abc"
        );
        assert_eq!(
            tool_versions(&rb_indexer::ScipMode::On, &present)["scip_go"],
            "sha256:abc"
        );
        assert_eq!(on["pipeline"], rb_indexer::PIPELINE);
        let off = tool_versions(&rb_indexer::ScipMode::Off, &rb_scip::Images::unresolved());
        assert_eq!(off["scip"], "off");
        assert!(off.get("scip_java").is_none() && off.get("scip_typescript").is_none());
        let unavailable = tool_versions(
            &rb_indexer::ScipMode::Unavailable("no docker".into()),
            &rb_scip::Images::unresolved(),
        );
        assert!(unavailable.get("scip_java").is_none());
        assert_eq!(
            phase_detail("fetch", Some(600)),
            " (fetch phase, 600 s budget)"
        );
    }

    /// Contract M3b amendment 44: the live snapshot covers a run only with the same commit, the
    /// same tools and, with SCIP on, no unfinished job; with SCIP off or unavailable the count
    /// is not consulted.
    #[test]
    fn rebuild_reason_requires_finished_scip_jobs() {
        let tools = json!({"rb": "1", "scip": "on"});
        let live = SnapshotInfo {
            id: 7,
            repo: "r".into(),
            commit_sha: "c".repeat(40),
            parent_id: None,
            status: "live".into(),
            tool_versions: tools.clone(),
            age_graph: None,
        };
        // The fake store: the snapshots asked about, and the count it answers.
        let asked = RefCell::new(Vec::new());
        let count = |n: i64| {
            let asked = &asked;
            move |id: i64| -> anyhow::Result<i64> {
                asked.borrow_mut().push(id);
                Ok(n)
            }
        };
        let commit = "c".repeat(40);
        assert_eq!(
            rebuild_reason(&live, &commit, &tools, true, &mut count(0)).unwrap(),
            None
        );
        assert_eq!(
            rebuild_reason(&live, &commit, &tools, true, &mut count(2)).unwrap(),
            Some(Rebuild::UnfinishedJobs(2))
        );
        assert_eq!(
            rebuild_reason(&live, &commit, &tools, false, &mut count(2)).unwrap(),
            None,
            "skipped by configuration"
        );
        assert_eq!(
            rebuild_reason(&live, &"d".repeat(40), &tools, true, &mut count(0)).unwrap(),
            Some(Rebuild::Commit)
        );
        assert_eq!(
            rebuild_reason(&live, &commit, &json!({"rb": "2"}), true, &mut count(0)).unwrap(),
            Some(Rebuild::Tools)
        );
        assert_eq!(
            *asked.borrow(),
            [7, 7],
            "the count is read for the matching snapshot only"
        );
    }

    /// Contract M3b amendment 50: a timeout's detail is printed once; other statuses print
    /// their reason; coverage lists `relationships dropped` (amendment 52).
    /// Contract M7f S1 (stress F4): the repo directive is a predicate on `--repo`, the same
    /// for questions and for `check`/`dump`, naming the checkout or the directory.
    #[test]
    fn repo_directive_reads_as_a_predicate_on_repo() {
        let unindexed = repo_needed(Some("demo_spring"), "/w");
        assert_eq!(
            unindexed.cli_text(),
            "--repo is needed: no indexed repository matches this checkout (\"demo_spring\"); index it (`rb index .`) or pass --repo"
        );
        assert_eq!(unindexed.param.as_deref(), Some("repo"));
        assert!(unindexed.message.starts_with("repo is needed: "));
        assert_eq!(
            repo_needed(None, "/tmp").cli_text(),
            "--repo is needed: no git repository found in \"/tmp\"; run rb from inside an indexed git checkout (or pass --repo)"
        );
    }

    /// Contract M7e S3: the status line names what was not analysed and why, in the stored
    /// (canonical) order; nothing excluded prints no line.
    #[test]
    fn excluded_line_counts_each_reason() {
        let d = |reason: &str, files: u64| rb_model::Diagnostic {
            kind: "excluded".into(),
            path: None,
            span: None,
            detail: json!({ "reason": reason, "files": files }),
        };
        assert_eq!(
            excluded_line(&[d("go_vendor", 1020), d("node_modules", 1)]).as_deref(),
            Some("excluded from analysis: go_vendor 1020 files, node_modules 1 file")
        );
        assert_eq!(excluded_line(&[]), None);
        // Contract M7g S1: languages without a grammar, counted the same way.
        let lang = |language: &str, files: u64| rb_model::Diagnostic {
            kind: "language_unsupported".into(),
            path: None,
            span: None,
            detail: json!({ "language": language, "files": files }),
        };
        assert_eq!(
            unsupported_languages_line(&[lang("php", 84), lang("rust", 1)]).as_deref(),
            Some("not analysed (no grammar): php 84 files, rust 1 file")
        );
        assert_eq!(unsupported_languages_line(&[]), None);
        // Contract M7g S2: frameworks without a pack, by service.
        let fw = |service: &str, framework: &str| rb_model::Diagnostic {
            kind: "framework_unsupported".into(),
            path: None,
            span: None,
            detail: json!({ "service": service, "framework": framework, "package": "p" }),
        };
        assert_eq!(
            unsupported_frameworks_line(&[fw("api", "echo"), fw("py", "flask")]).as_deref(),
            Some(
                "frameworks without a pack (routes and clients not read): echo in api, flask in py"
            )
        );
        assert_eq!(unsupported_frameworks_line(&[]), None);
    }

    #[test]
    fn scip_status_line_prints_a_timeout_once() {
        let diagnostic = |detail: Value| rb_model::Diagnostic {
            kind: "scip_status".into(),
            path: Some(".".into()),
            span: None,
            detail,
        };
        let timeout = diagnostic(json!({
            "indexer": "scip-typescript", "status": "timeout", "unpinned": true,
            "phase": "fetch", "budget_secs": 3, "reason": "fetch phase over its 3 s budget",
        }));
        assert_eq!(
            scip_status_line(&timeout),
            "scip: . scip-typescript timeout unpinned (fetch phase, 3 s budget)"
        );
        let environment = diagnostic(json!({
            "indexer": "scip-java", "status": "environment", "unpinned": false,
            "reason": "docker: Error response from daemon: No such image",
        }));
        assert_eq!(
            scip_status_line(&environment),
            "scip: . scip-java environment: docker: Error response from daemon: No such image"
        );
        let ok = diagnostic(json!({
            "indexer": "scip-java", "status": "ok", "unpinned": false, "image": "sha256:1",
            "coverage": {"definitions_matched": 2, "definitions_total": 3,
                         "definitions_without_occurrence": 1, "relationships_dropped": 4,
                         "documents_rejected": 0, "documents_rejected_reasons": {}},
        }));
        // Contract M3b amendments 62 and 64: the definitions without an occurrence, the
        // rejected documents and the anonymous classes' links are printed, zero or not.
        assert_eq!(
            scip_status_line(&ok),
            "scip: . scip-java ok, definitions matched 2/3 (0 by name), definitions without occurrence 1, references matched 0, external 0, undocumented 0, local 0, unanchored 0, unmatched 0, unconverted 0, documents rejected 0, relationships dropped 4 (0 in project, 0 anonymous)"
        );
        let rejected = diagnostic(json!({
            "indexer": "scip-java", "status": "ok", "unpinned": false, "image": "sha256:1",
            "coverage": {"definitions_without_occurrence": 0, "documents_rejected": 3,
                         "documents_rejected_reasons": {"comment_terminator": 2, "not_utf8": 1}},
        }));
        assert!(
            scip_status_line(&rejected).contains(
                ", unconverted 0, documents rejected 3 (comment_terminator 2, not_utf8 1), relationships dropped 0"
            ),
            "{}",
            scip_status_line(&rejected)
        );
        // Contract M6a D3: the Lombok removals follow the rejected documents, only when there
        // are any.
        let lombok = diagnostic(json!({
            "indexer": "scip-java", "status": "ok", "unpinned": false, "image": "sha256:1",
            "coverage": {"definitions_without_occurrence": 0, "documents_rejected": 1,
                         "documents_rejected_reasons": {"structure_disagreement": 1},
                         "lombok_removed": 27, "documents_lombok": 4},
        }));
        assert!(
            scip_status_line(&lombok).contains(
                ", documents rejected 1 (structure_disagreement 1), lombok-generated 27 in 4 documents, relationships dropped 0"
            ),
            "{}",
            scip_status_line(&lombok)
        );
        assert!(!scip_status_line(&rejected).contains("lombok"));
        // Contract M6a D5: a grouped workspace root's members, failed or not.
        let grouped = diagnostic(json!({
            "indexer": "scip-typescript", "status": "failed", "unpinned": false,
            "phase": "fetch", "workspace_members": 7,
        }));
        assert_eq!(
            scip_status_line(&grouped),
            "scip: . scip-typescript failed (fetch phase), workspace 7 members"
        );
        assert!(!scip_status_line(&rejected).contains("workspace"));
        // Contract M6a D7: link targets after the status, documents outside the root with the
        // coverage, each only when non-zero.
        let linked = diagnostic(json!({
            "indexer": "scip-typescript", "status": "ok", "unpinned": false, "image": "sha256:1",
            "link_targets": 1,
            "coverage": {"definitions_without_occurrence": 0, "documents_rejected": 0,
                         "documents_rejected_reasons": {}, "documents_outside_root": 2},
        }));
        let line = scip_status_line(&linked);
        assert!(
            line.starts_with("scip: . scip-typescript ok, 1 link targets, definitions matched")
                && line.contains(
                    ", documents rejected 0, 2 documents outside the root, relationships dropped"
                ),
            "{line}"
        );
        assert!(!scip_status_line(&rejected).contains("link targets"));
        assert!(!scip_status_line(&rejected).contains("outside the root"));
        // Contract M3b amendment 67: a snapshot written before a field existed (the evaluator's
        // `fp3_class_validator`, before amendments 62 and 64) omits it instead of printing 0.
        let old = diagnostic(json!({
            "indexer": "scip-typescript", "status": "ok", "unpinned": false, "image": "sha256:1",
            "coverage": {"definitions_matched": 417, "definitions_total": 424},
        }));
        assert_eq!(
            scip_status_line(&old),
            "scip: . scip-typescript ok, definitions matched 417/424 (0 by name), references matched 0, external 0, undocumented 0, local 0, unanchored 0, unmatched 0, unconverted 0, relationships dropped 0 (0 in project, 0 anonymous)"
        );
        // Local definitions (contract M3b amendment 54) are printed when there are any.
        let local = diagnostic(json!({
            "indexer": "scip-java", "status": "ok", "unpinned": false, "image": "sha256:1",
            "coverage": {"definitions_matched": 2, "definitions_local": 1, "definitions_total": 3,
                         "relationships_dropped": 4, "relationships_dropped_in_project": 1,
                         "relationships_anonymous": 2},
        }));
        assert!(
            scip_status_line(&local).ends_with(
                ", relationships dropped 4 (1 in project, 2 anonymous), 1 local definitions"
            ),
            "{}",
            scip_status_line(&local)
        );
        // Declared supertypes the index does not confirm (contract M3b amendment 78) are
        // printed when there are any, after the unresolved ones.
        let unconfirmed = diagnostic(json!({
            "indexer": "scip-java", "status": "ok", "unpinned": false, "image": "sha256:1",
            "coverage": {"definitions_matched": 2, "definitions_total": 2,
                         "supertypes_unresolved": 2, "supertypes_unconfirmed": 1},
        }));
        assert!(
            scip_status_line(&unconfirmed)
                .ends_with(", 2 supertypes unresolved, 1 supertypes unconfirmed"),
            "{}",
            scip_status_line(&unconfirmed)
        );
    }

    /// Contract M3b amendment 56: a failed job names its phase on both lines.
    #[test]
    fn scip_status_line_prints_the_failed_phase() {
        let failed = rb_model::Diagnostic {
            kind: "scip_status".into(),
            path: Some("services/broken".into()),
            span: None,
            detail: json!({
                "indexer": "scip-java", "status": "failed", "unpinned": false,
                "image": "sha256:1", "phase": "fetch", "stderr_tail": "[ERROR] boom",
            }),
        };
        assert_eq!(
            scip_status_line(&failed),
            "scip: services/broken scip-java failed (fetch phase)"
        );
        assert_eq!(
            status_detail(&rb_scip::RunStatus::Failed(rb_scip::Phase::Index)),
            " (index phase)"
        );
        assert_eq!(
            status_detail(&rb_scip::RunStatus::Timeout {
                phase: rb_scip::Phase::Fetch,
                budget_secs: 600
            }),
            " (fetch phase, 600 s budget)"
        );
        assert_eq!(
            status_detail(&rb_scip::RunStatus::Skipped(
                "docker is not available".into()
            )),
            ": docker is not available"
        );
        assert_eq!(status_detail(&rb_scip::RunStatus::Ok), "");
    }

    /// Contract M5 S22: each repository's package caches live in a directory of its own, so
    /// two repositories share no Maven or npm cache; the allow-list's extra hosts come from the
    /// host's flag or environment and are validated.
    #[test]
    fn scip_caches_are_per_repository_and_registries_are_validated() {
        let work = Path::new("/w/var/scip");
        assert_eq!(
            scip_cache_dir(work, "shop"),
            Path::new("/w/var/scip/cache/shop")
        );
        assert_ne!(
            scip_cache_dir(work, "shop"),
            scip_cache_dir(work, "shop_api")
        );
        assert!(!scip_cache_dir(work, "shop").starts_with(scip_cache_dir(work, "sho")));
        let cli = Cli::try_parse_from([
            "rb",
            "index",
            ".",
            "--scip-registries",
            "repo.spring.io,maven.example.org",
        ])
        .unwrap();
        let Command::Index(args) = cli.command else {
            panic!("an index command")
        };
        assert_eq!(
            args.scip_registries,
            ["repo.spring.io", "maven.example.org"]
        );
        let egress = rb_scip::Egress::with(&args.scip_registries).unwrap();
        assert!(egress.hosts.contains(&"repo.spring.io".to_owned()));
        assert!(rb_scip::Egress::with(&["*.example.org".to_owned()]).is_err());
    }

    /// Contract M6a D9, S12: the budget flags at each bound and one beyond; their defaults are
    /// `cfa8ab9`'s budget; the flags reach `rb_scip::Budget`.
    #[test]
    fn scip_budget_flags_are_bounded() {
        let parse = |extra: &[&str]| {
            let mut argv = vec!["rb", "index", "."];
            argv.extend_from_slice(extra);
            Cli::try_parse_from(argv).map(|cli| match cli.command {
                Command::Index(args) => scip_budget(&args),
                _ => panic!("an index command"),
            })
        };
        assert_eq!(parse(&[]).unwrap(), rb_scip::Budget::default());
        for ok in [
            &["--scip-cpus", "0.5"][..],
            &["--scip-cpus", "64"],
            &["--scip-memory", "512m"],
            &["--scip-memory", "64g"],
            &["--scip-memory", "65536m"],
            &["--scip-maven-threads", "1"],
            &["--scip-maven-threads", "16"],
        ] {
            assert!(parse(ok).is_ok(), "{ok:?}");
        }
        for bad in [
            &["--scip-cpus", "0.4"][..],
            &["--scip-cpus", "64.5"],
            &["--scip-cpus", "NaN"],
            &["--scip-memory", "511m"],
            &["--scip-memory", "65g"],
            &["--scip-memory", "65537m"],
            &["--scip-memory", "4"],
            &["--scip-memory", "4gb"],
            &["--scip-memory", "-1g"],
            &["--scip-maven-threads", "0"],
            &["--scip-maven-threads", "17"],
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        let budget = parse(&[
            "--scip-cpus",
            "8",
            "--scip-memory",
            "6g",
            "--scip-maven-threads",
            "4",
            "--scip-timeout-secs",
            "30",
        ])
        .unwrap();
        assert_eq!(
            (
                budget.cpus,
                budget.memory.as_str(),
                budget.maven_threads,
                budget.index_timeout.as_secs()
            ),
            (8.0, "6g", 4, 30)
        );
    }

    #[test]
    fn repo_names_are_sanitised() {
        assert_eq!(
            repo_name_from_path(Path::new("/x/My-Repo.git")),
            "my_repo_git"
        );
        assert_eq!(repo_name_from_path(Path::new("/x/__a  b__")), "a_b");
        assert_eq!(repo_name_from_path(Path::new("/x/ü")), "repo");
        assert_eq!(
            repo_name_from_path(Path::new(&format!("/x/{}", "a".repeat(50)))).len(),
            40
        );
    }

    #[test]
    fn git_root_finds_dir_file_and_nesting() {
        let base = std::env::temp_dir().join(format!("rb-git-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("proj");
        let nested = repo.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir(repo.join(".git")).unwrap();
        assert_eq!(git_root(&nested), Some(repo.clone()));
        assert_eq!(git_root(&repo), Some(repo.clone()));
        assert_eq!(git_root(&base), None);
        // Worktrees and submodules keep a `.git` file instead of a directory.
        let wt = base.join("wt");
        std::fs::create_dir_all(&wt).unwrap();
        std::fs::write(wt.join(".git"), "gitdir: elsewhere").unwrap();
        assert_eq!(git_root(&wt), Some(wt.clone()));
        let _ = std::fs::remove_dir_all(&base);
    }
}
