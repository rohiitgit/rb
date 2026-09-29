//! `rb` — Repository Brain CLI (design §11.1).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};
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
    /// Parse every file instead of reusing cached syntax facts (the cache is not updated either).
    #[arg(long)]
    no_cache: bool,
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
    let tools = tool_versions();

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

    let rb_indexer::Indexed {
        commit_sha,
        graph,
        diagnostics,
        parsed,
    } = if no_cache {
        rb_indexer::index_commit(&path, &commit_sha, &mut rb_indexer::NoCache)?
    } else {
        rb_indexer::index_commit(&path, &commit_sha, &mut StoreCache(store))?
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
    Ok(())
}

/// Syntax facts cached in Postgres, keyed by (blob, extractor version + language).
struct StoreCache<'a>(&'a mut Store);

impl rb_indexer::FactsCache for StoreCache<'_> {
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
        self.0.save_facts(&rows)
    }
}

fn tool_versions() -> Value {
    json!({
        "rb": env!("CARGO_PKG_VERSION"),
        "pipeline": rb_indexer::PIPELINE,
        "schema": rb_store::schema_version(),
    })
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
    out += "└─ APIs, tables, dependencies and call graph: not indexed yet (M2–M4)\n";
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
        let ops = a.ops.join("+");
        let assumed = if a.schema_assumed {
            ", schema assumed"
        } else {
            ""
        };
        format!(
            "{}  {ops}  {}:{}  [{}{assumed}]",
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
