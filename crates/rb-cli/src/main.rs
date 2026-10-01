//! `rb` — Repository Brain CLI (design §11.1).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, bail, ensure};

mod code;
use clap::{Args, Parser, Subcommand};
use rb_model::{NodeKind, ServiceRole, canonical_dump};
use rb_store::{LoadDir, Resolved, ServiceCard, SnapshotInfo, Store, TableAccess, TableDeps};
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
    /// Parse every file instead of reusing cached syntax facts (that cache is not updated
    /// either), and run every SCIP job again instead of serving it from the index cache (its
    /// outcome is stored, so a cached `failed` can be retried).
    #[arg(long)]
    no_cache: bool,
    /// Run no SCIP indexer: every project is resolved heuristically and its job is `skipped`.
    #[arg(long)]
    no_scip: bool,
    /// Directory for indexer exports and package caches; the container runtime must be able to
    /// bind-mount it (a path the runtime shares with its VM, such as one under the home
    /// directory).
    #[arg(long, env = "RB_SCIP_WORK_DIR", default_value = "var/scip")]
    scip_work_dir: PathBuf,
    /// Seconds a SCIP job's index phase may take (its fetch phase gets the same budget); a run
    /// over it is killed and recorded as `timeout`. Default: 600 for the fetch, 900 for the index.
    #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
    scip_timeout_secs: Option<u64>,
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
        kind: Option<NodeKind>,
        #[arg(long, default_value_t = 20, value_parser = clap::value_parser!(i64).range(1..=1000))]
        limit: i64,
        #[arg(long)]
        repo: Option<String>,
    },
    /// List services, or show one service's card (by directory or name).
    Service {
        /// Service directory (e.g. `services/auth`, `.` for the repo root) or name.
        service: Option<String>,
        #[arg(long)]
        repo: Option<String>,
        /// Emit JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Show which migrations and code touch a table (by `schema.table` or bare name).
    TableDeps {
        table: String,
        #[arg(long)]
        repo: Option<String>,
        /// Emit JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Show what calls a function or method (Q1), as a caller tree.
    Callers {
        /// A symbol key, `path:line`, a key suffix (`AuthService#login().`), `Class.method` or a
        /// name.
        target: String,
        #[arg(long)]
        repo: Option<String>,
        /// Levels of callers to follow (1-25).
        #[arg(long, default_value_t = 1)]
        depth: usize,
        /// Also count function values passed around (`REFERENCES`).
        #[arg(long)]
        include_references: bool,
        /// Ignore edges below this certainty (ambiguous, heuristic, synthetic, dispatch, exact).
        #[arg(long, default_value = "ambiguous", value_parser = code::parse_certainty)]
        min_certainty: rb_model::Certainty,
        /// Choose among several matching symbols (1-based, as listed).
        #[arg(long)]
        pick: Option<usize>,
    },
    /// Show what breaks if a symbol or table is removed (Q2).
    Impact {
        /// As for `callers`; a table key (`public.users`) also works.
        target: String,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long, default_value = "ambiguous", value_parser = code::parse_certainty)]
        min_certainty: rb_model::Certainty,
        #[arg(long)]
        pick: Option<usize>,
    },
    /// Run a read-only Cypher query against the live snapshot's AGE graph.
    Cypher {
        /// openCypher query, e.g. "MATCH (f:File)-[:IN_SERVICE]->(s:Service) RETURN s.name, count(f)".
        query: String,
        #[arg(long)]
        repo: Option<String>,
        /// Comma-separated names for the RETURN columns, in order.
        #[arg(long, default_value = "result", value_delimiter = ',')]
        cols: Vec<String>,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
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
        Command::Status { repo } => {
            let repos = match repo {
                Some(r) => vec![r],
                None => store.repo_names()?,
            };
            if repos.is_empty() {
                println!("no repositories indexed yet");
            }
            for r in repos {
                let s = store.status(&r)?;
                match &s.live {
                    Some(live) => println!(
                        "{}: live snapshot {} @ {} — {} nodes, {} edges, {} diagnostics; AGE graph {}; {} snapshot(s) kept",
                        s.name,
                        live.id,
                        short(&live.commit_sha),
                        s.nodes,
                        s.edges,
                        s.diagnostics,
                        live.age_graph.as_deref().unwrap_or("-"),
                        s.snapshots
                    ),
                    None => println!("{}: no live snapshot", s.name),
                }
                if let Some(live) = &s.live {
                    let c = store.sql_coverage(live.id)?;
                    println!(
                        "  sql: {} tables; {} code sites and {} migration statements touching them; {} unparsed",
                        c.tables, c.query_sites, c.migration_statements, c.unparsed
                    );
                    println!("  {}", code_line(&store.resolution_coverage(live.id)?));
                    for d in store.load_diagnostics(live.id)? {
                        if d.kind == "scip_status" {
                            println!("  {}", scip_status_line(&d));
                        }
                    }
                }
            }
        }
        Command::Dump {
            repo,
            snapshot,
            output,
        } => {
            let snap = match snapshot {
                Some(id) => store.snapshot(id)?,
                None => live_snapshot(&mut store, repo)?,
            };
            let graph = store.load_graph(snap.id)?;
            let diagnostics = store.load_diagnostics(snap.id)?;
            let dump = canonical_dump(&snap.commit_sha, &graph, &diagnostics);
            match output {
                Some(p) => {
                    std::fs::write(&p, dump).with_context(|| format!("writing {}", p.display()))?
                }
                None => std::io::stdout().write_all(dump.as_bytes())?,
            }
        }
        Command::Find {
            text,
            kind,
            limit,
            repo,
        } => {
            let snap = live_snapshot(&mut store, repo)?;
            for n in store.find(snap.id, &text, kind, limit)? {
                println!("{}\t{}\t{}", n.kind, n.key, n.name);
            }
        }
        Command::Service {
            service,
            repo,
            json,
        } => {
            let snap = live_snapshot(&mut store, repo)?;
            match service {
                None => {
                    let services = store.services(snap.id)?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&services)?);
                    } else {
                        for s in services {
                            println!("{}\t{}\t{}\t{}", s.key, s.name, s.role, files(s.files));
                        }
                    }
                }
                Some(text) => {
                    let found = match store.resolve_service(snap.id, &text)? {
                        Resolved::One(n) => n,
                        Resolved::None => {
                            bail!("no service matches {text:?} (try `rb service` to list them)")
                        }
                        Resolved::Ambiguous(candidates) => {
                            let list: Vec<String> = candidates
                                .iter()
                                .map(|c| format!("  {}  ({})", c.key, c.name))
                                .collect();
                            bail!(
                                "{text:?} matches several services; pass a directory:\n{}",
                                list.join("\n")
                            );
                        }
                    };
                    let card = store.service_card(snap.id, &found.key)?;
                    if json {
                        println!("{}", serde_json::to_string_pretty(&card)?);
                    } else {
                        print!("{}", render_card(&card));
                    }
                }
            }
        }
        Command::TableDeps { table, repo, json } => {
            let snap = live_snapshot(&mut store, repo)?;
            let found = match store.resolve_table(snap.id, &table)? {
                Resolved::One(n) => n,
                Resolved::None => {
                    bail!("no table matches {table:?} (try `rb find {table} --kind table`)")
                }
                Resolved::Ambiguous(candidates) => {
                    let list: Vec<String> =
                        candidates.iter().map(|c| format!("  {}", c.key)).collect();
                    bail!(
                        "{table:?} matches several tables; pass schema.table:\n{}",
                        list.join("\n")
                    );
                }
            };
            let deps = store.table_deps(snap.id, &found.key)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&deps)?);
            } else {
                print!("{}", render_table_deps(&deps));
            }
        }
        Command::Callers {
            target,
            repo,
            depth,
            include_references,
            min_certainty,
            pick,
        } => {
            ensure!(
                (1..=rb_store::code::MAX_CALLER_DEPTH).contains(&depth),
                "--depth must be between 1 and {}",
                rb_store::code::MAX_CALLER_DEPTH
            );
            let snap = live_snapshot(&mut store, repo)?;
            let graph = store.load_graph(snap.id)?;
            let t = code::pick_target(&graph, &target, pick)?;
            let tree = rb_store::code::callers(
                &graph,
                t,
                rb_store::code::CallerOptions {
                    depth,
                    include_references,
                    min_certainty,
                },
            );
            print!("{}", code::render_callers(&graph, &tree));
        }
        Command::Impact {
            target,
            repo,
            min_certainty,
            pick,
        } => {
            let snap = live_snapshot(&mut store, repo)?;
            let graph = store.load_graph(snap.id)?;
            let t = code::pick_target(&graph, &target, pick)?;
            let impact = rb_store::code::impact(&graph, t, min_certainty);
            print!("{}", code::render_impact(&graph, &impact));
        }
        Command::Cypher { query, repo, cols } => {
            let snap = live_snapshot(&mut store, repo)?;
            for row in store.cypher(snap.id, &query, &cols)? {
                println!("{}", row.join("\t"));
            }
        }
    }
    Ok(())
}

fn index(store: &mut Store, load_dir: &Path, args: IndexArgs) -> anyhow::Result<()> {
    let IndexArgs {
        path,
        repo,
        rev,
        force,
        no_cache,
        no_scip,
        scip_work_dir,
        scip_timeout_secs,
    } = args;
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
    let images = match scip_mode {
        rb_indexer::ScipMode::On => rb_scip::Images::resolve(&rb_scip::DockerRunner::default()),
        _ => rb_scip::Images::unresolved(),
    };
    let tools = tool_versions(&scip_mode, &images);

    if !force
        && let Some(live) = store.live_snapshot(&repo)?
        && live.commit_sha == commit_sha
        && live.tool_versions == tools
    {
        println!(
            "{repo}: up to date — live snapshot {} already covers {} (use --force to rebuild)",
            live.id,
            short(&commit_sha)
        );
        return Ok(());
    }

    let mut budget = rb_scip::Budget::default();
    if let Some(secs) = scip_timeout_secs {
        budget.fetch_timeout = Duration::from_secs(secs);
        budget.index_timeout = Duration::from_secs(secs);
    }
    let scip = rb_indexer::ScipOptions {
        mode: scip_mode,
        images,
        cache_dir: scip_work_dir.join("cache"),
        work_dir: scip_work_dir,
        budget,
    };
    // Both caches live in the store; the indexer borrows each through its own trait.
    let cell = RefCell::new(&mut *store);
    let rb_indexer::Indexed {
        commit_sha,
        graph,
        diagnostics,
        parsed,
        scip: jobs,
    } = if no_cache {
        rb_indexer::index_commit(
            &path,
            &commit_sha,
            &mut rb_indexer::NoCache,
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
    let snap = store.publish_snapshot(
        &repo,
        &commit_sha,
        &tools,
        &graph,
        &diagnostics,
        &LoadDir::new(load_dir),
    )?;
    println!(
        "{repo}: snapshot {} live @ {} — {} nodes, {} edges, {} diagnostics; AGE graph {}; parsed {parsed} source files",
        snap.id,
        short(&snap.commit_sha),
        graph.nodes().len(),
        graph.edges().len(),
        diagnostics.len(),
        snap.age_graph.as_deref().unwrap_or("-")
    );
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
        match &j.status {
            rb_scip::RunStatus::Timeout { phase, budget_secs } => {
                line.push_str(&timeout_detail(phase.as_str(), *budget_secs));
            }
            other => {
                if let Some(reason) = other.reason() {
                    let _ = write!(line, ": {reason}");
                }
            }
        }
        println!("{line}");
    }
    Ok(())
}

/// ` (fetch phase, 600 s budget)` after a `timeout` status (contract M3b amendment 31).
fn timeout_detail(phase: &str, budget_secs: u64) -> String {
    format!(" ({phase} phase, {budget_secs} s budget)")
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
    if let (Some(phase), Some(budget)) =
        (d.detail["phase"].as_str(), d.detail["budget_secs"].as_u64())
    {
        line.push_str(&timeout_detail(phase, budget));
    }
    if let Some(reason) = d.detail["reason"].as_str() {
        let _ = write!(line, ": {reason}");
    }
    if let Some(c) = d.detail.get("coverage") {
        let n = |k: &str| c[k].as_u64().unwrap_or(0);
        let _ = write!(
            line,
            ", definitions matched {}/{} ({} by name), references matched {}, external {}, undocumented {}, local {}, unanchored {}, unmatched {}, unconverted {}",
            n("definitions_matched"),
            n("definitions_total"),
            n("definitions_matched_by_name"),
            n("references_matched"),
            n("references_external"),
            n("references_undocumented"),
            n("references_local"),
            n("references_unanchored"),
            n("references_unmatched"),
            n("occurrences_unconverted")
        );
        // Files the indexer left out (excluded by its tsconfig): no edges of any kind.
        if n("files_without_document") > 0 {
            let _ = write!(
                line,
                ", {} files without a document",
                n("files_without_document")
            );
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

/// What a snapshot was built with; `scip` (`on`, `off`, `unavailable`) because a run without
/// indexers is a different graph, whether they were turned off or Docker was missing, and with
/// `on` the identity of each indexer image (its ID, or `absent`; contract M3b amendment 29),
/// since a graph indexed without an image is heuristic and a rebuilt image may index differently.
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
    }
    tools
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
    // Packages named by the service's exact (SCIP) references; heuristic projects have none.
    if c.packages.is_empty() {
        out += "├─ Packages: none\n";
    } else {
        out += &format!("├─ Packages ({})\n", c.packages.len());
        for p in &c.packages {
            out += &format!("│    {p}\n");
        }
    }
    out += "└─ Endpoints and request paths: not indexed yet (M4)\n";
    out
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
    out += "└─ Callers up to entry points arrive with call resolution (M3)\n";
    out
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

fn live_snapshot(store: &mut Store, repo: Option<String>) -> anyhow::Result<SnapshotInfo> {
    let repo = match repo {
        Some(r) => r,
        None => {
            let names = store.repo_names()?;
            match names.as_slice() {
                [only] => only.clone(),
                [] => bail!("no repositories indexed yet"),
                many => bail!(
                    "several repositories indexed; pass --repo (one of: {})",
                    many.join(", ")
                ),
            }
        }
    };
    store
        .live_snapshot(&repo)?
        .with_context(|| format!("repository {repo:?} has no live snapshot"))
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

    #[test]
    fn find_limit_is_validated_at_parse_time() {
        for bad in ["0", "-1", "1001"] {
            assert!(
                Cli::try_parse_from(["rb", "find", "x", "--limit", bad]).is_err(),
                "{bad}"
            );
        }
        assert!(Cli::try_parse_from(["rb", "find", "x", "--limit", "1000"]).is_ok());
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

    /// Contract M3b amendments 29 and 31: the image identities are recorded only when SCIP is
    /// on; a timeout prints its phase and budget.
    #[test]
    fn tool_versions_record_image_identities_when_scip_is_on() {
        let images = rb_scip::Images {
            java: Err("absent".into()),
            typescript: Some("sha256:abc".into()),
        };
        let on = tool_versions(&rb_indexer::ScipMode::On, &images);
        assert_eq!(on["scip"], "on");
        assert_eq!(on["scip_java"], "absent");
        assert_eq!(on["scip_typescript"], "sha256:abc");
        assert_eq!(on["pipeline"], rb_indexer::PIPELINE);
        let off = tool_versions(&rb_indexer::ScipMode::Off, &rb_scip::Images::unresolved());
        assert_eq!(off["scip"], "off");
        assert!(off.get("scip_java").is_none() && off.get("scip_typescript").is_none());
        let unavailable = tool_versions(
            &rb_indexer::ScipMode::Unavailable("no docker".into()),
            &rb_scip::Images::unresolved(),
        );
        assert!(unavailable.get("scip_java").is_none());
        assert_eq!(timeout_detail("fetch", 600), " (fetch phase, 600 s budget)");
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
}
