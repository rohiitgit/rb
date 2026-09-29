//! `rb` — Repository Brain CLI (design §11.1). M0 commands: migrate, index, status, dump, cypher.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};
use clap::{Parser, Subcommand};
use rb_model::{canonical_dump, graph_from_manifest};
use rb_store::{Diagnostic, LoadDir, SnapshotInfo, Store};
use serde_json::{Value, json};

/// Identifies the extraction pipeline; bump whenever indexing output changes for the same input.
const PIPELINE: &str = "m0-manifest-1";

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

#[derive(Subcommand)]
enum Command {
    /// Apply pending schema migrations.
    Migrate,
    /// Index a commit of a git repository into a new live snapshot.
    Index {
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
    },
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
        Command::Index {
            path,
            repo,
            rev,
            force,
        } => index(&mut store, &cli.age_load_dir, &path, repo, &rev, force)?,
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
            let dump = canonical_dump(&snap.commit_sha, &store.load_graph(snap.id)?);
            match output {
                Some(p) => {
                    std::fs::write(&p, dump).with_context(|| format!("writing {}", p.display()))?
                }
                None => std::io::stdout().write_all(dump.as_bytes())?,
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

fn index(
    store: &mut Store,
    load_dir: &Path,
    path: &Path,
    repo: Option<String>,
    rev: &str,
    force: bool,
) -> anyhow::Result<()> {
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

    let manifest = rb_git::read_manifest(&path, rev)?;
    store.migrate()?;
    let tools = tool_versions();

    if !force
        && let Some(live) = store.live_snapshot(&repo)?
        && live.commit_sha == manifest.commit_sha
        && live.tool_versions == tools
    {
        println!(
            "{repo}: up to date — live snapshot {} already covers {} (use --force to rebuild)",
            live.id,
            short(&manifest.commit_sha)
        );
        return Ok(());
    }

    let graph = graph_from_manifest(&manifest)?;
    let diagnostics: Vec<Diagnostic> = manifest
        .skipped
        .iter()
        .map(|s| Diagnostic {
            kind: format!("skipped_{}", s.reason),
            detail: json!({ "path": s.path }),
        })
        .collect();
    let snap = store.publish_snapshot(
        &repo,
        &manifest.commit_sha,
        &tools,
        &graph,
        &diagnostics,
        &LoadDir::new(load_dir),
    )?;
    println!(
        "{repo}: snapshot {} live @ {} — {} nodes, {} edges, {} diagnostics; AGE graph {}",
        snap.id,
        short(&snap.commit_sha),
        graph.nodes().len(),
        graph.edges().len(),
        diagnostics.len(),
        snap.age_graph.as_deref().unwrap_or("-")
    );
    Ok(())
}

fn tool_versions() -> Value {
    json!({
        "rb": env!("CARGO_PKG_VERSION"),
        "pipeline": PIPELINE,
        "schema": rb_store::schema_version(),
    })
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
