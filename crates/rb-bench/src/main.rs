//! `rb-bench`: the accuracy benchmark's command line (contract M8a). `bench/run.sh` indexes and
//! dumps; this binary scores, renders and gates.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use rb_bench::compare::{self, NOT_COMPARABLE};
use rb_bench::scoreboard::{Perf, Rb, Scoreboard};
use rb_bench::{render, run};

#[derive(Parser)]
#[command(name = "rb-bench", about = "Repository Brain accuracy benchmark")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Score a `bench/run.sh` output directory: writes the scoreboard and the perf file.
    Score {
        /// The run's output directory (`var/bench/out/<sha12>`).
        out: PathBuf,
        #[arg(long)]
        milestone: String,
        /// The commit the binary was built from.
        #[arg(long)]
        rb_commit: String,
        #[arg(long, default_value = "bench/manifest.tsv")]
        manifest: PathBuf,
        /// Where `bench/fetch.sh` cloned the targets.
        #[arg(long, default_value = "var/bench/repos")]
        repos: PathBuf,
        /// Where to write the scoreboard (`bench/scoreboard/<milestone>.json`).
        #[arg(long)]
        write: PathBuf,
        /// Where to write the perf file (`bench/scoreboard/<milestone>-perf.json`).
        #[arg(long)]
        perf: PathBuf,
    },
    /// Print a scoreboard as a Markdown table.
    Render {
        file: PathBuf,
        /// Print how often each certainty label is right instead.
        #[arg(long)]
        certainty: bool,
    },
    /// Gate HEAD against BASE: exit 0 pass, 1 regression, 2 not comparable.
    Compare {
        base: PathBuf,
        head: PathBuf,
        /// Accepted regressions: `milestone<TAB>cell<TAB>reason` per line.
        #[arg(long)]
        accept: Option<PathBuf>,
        /// The two perf files, base then head: warnings only.
        #[arg(long, num_args = 2, value_names = ["BASE_PERF", "HEAD_PERF"])]
        perf: Vec<PathBuf>,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse().command) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("rb-bench: {e}");
            ExitCode::from(NOT_COMPARABLE)
        }
    }
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write_file(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn board(path: &Path) -> Result<Scoreboard, String> {
    Scoreboard::from_json(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))
}

fn perf(path: &Path) -> Result<Perf, String> {
    Perf::from_json(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))
}

/// The `(bench, class)` pairs a manifest claims to measure; each needs a non-vacuous cell (S8).
fn claims(targets: &[run::Target]) -> Vec<(&'static str, &'static str)> {
    let mut claims = Vec::new();
    if targets
        .iter()
        .any(|t| t.kind == "stress" && t.modes.iter().any(|m| m == "scip"))
    {
        claims.push(("scip", "calls"));
    }
    if targets.iter().any(|t| t.kind == "pycg") {
        claims.extend([("pycg", "calls"), ("pycg", "calls_file_scope")]);
    }
    claims
}

fn run(command: Command) -> Result<u8, String> {
    match command {
        Command::Score {
            out,
            milestone,
            rb_commit,
            manifest,
            repos,
            write,
            perf: perf_file,
        } => {
            let targets = run::parse_manifest(&read(&manifest)?)?;
            let (perf, perf_notes) = run::perf(&out, &milestone, &targets)?;
            let rb = Rb {
                commit: rb_commit,
                binary_sha256: perf.binary_sha256.clone(),
            };
            let (board, notes) = run::scoreboard(&targets, &out, &repos, &milestone, rb)?;
            for note in perf_notes.iter().chain(&notes) {
                println!("{note}");
            }
            board.check_non_vacuous(&claims(&targets))?;
            write_file(&write, &board.to_json())?;
            write_file(&perf_file, &perf.to_json())?;
            print!("{}", render::render(&board));
            Ok(compare::PASS)
        }
        Command::Render { file, certainty } => {
            let b = board(&file)?;
            let text = if certainty {
                render::render_certainty(&b)
            } else {
                render::render(&b)
            };
            print!("{text}");
            Ok(compare::PASS)
        }
        Command::Compare {
            base,
            head,
            accept,
            perf: perf_files,
        } => {
            let (base, head) = (board(&base)?, board(&head)?);
            let accept = match accept {
                Some(p) => compare::parse_accept(&read(&p)?, &head.milestone)?,
                None => compare::Accept::default(),
            };
            let report = compare::compare(&base, &head, &accept);
            print!("{}", report.text);
            if let [b, h] = &perf_files[..] {
                for w in compare::compare_perf(&perf(b)?, &perf(h)?) {
                    println!("warning: {w}");
                }
            }
            Ok(report.exit)
        }
    }
}
