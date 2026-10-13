//! From one `bench/run.sh` output directory to a scoreboard and a perf file (contract M8a, S8):
//! the manifest names the targets and pins, the dumps are scored, the `.meta` files are timed runs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dump::{Dump, parse};
use crate::scoreboard::{CellKey, Label, Perf, Rb, Record, Run, SCHEMA, Scoreboard};
use crate::{pycg, scip_ref};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub name: String,
    pub kind: String,
    pub url: String,
    pub sha: String,
    pub modes: Vec<String>,
}

/// `bench/manifest.tsv`: `name kind url sha modes`, tab-separated; `#` comments and the header
/// row are skipped.
pub fn parse_manifest(text: &str) -> Result<Vec<Target>, String> {
    let mut targets = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') || line.starts_with("name\t") {
            continue;
        }
        let [name, kind, url, sha, modes] = line.split('\t').collect::<Vec<_>>()[..] else {
            return Err(format!(
                "manifest line {}: expected 5 tab-separated fields",
                i + 1
            ));
        };
        let valid_sha = sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit());
        if !valid_sha {
            return Err(format!("manifest line {}: {sha:?} is not a commit", i + 1));
        }
        targets.push(Target {
            name: name.to_owned(),
            kind: kind.to_owned(),
            url: url.to_owned(),
            sha: sha.to_owned(),
            modes: modes.split(',').map(str::to_owned).collect(),
        });
    }
    Ok(targets)
}

/// A `.meta` file: `key: value` lines.
fn meta(text: &str) -> BTreeMap<&str, &str> {
    text.lines().filter_map(|l| l.split_once(": ")).collect()
}

fn read(path: &Path) -> Result<String, String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_dump(path: &Path) -> Result<Option<Dump>, String> {
    if !path.exists() {
        return Ok(None);
    }
    parse(&read(path)?)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// The timed runs of the manifest's targets in an output directory. Every run must be of one
/// binary; a run that failed is reported, not timed.
pub fn perf(
    out: &Path,
    milestone: &str,
    targets: &[Target],
) -> Result<(Perf, Vec<String>), String> {
    let mut runs = BTreeMap::new();
    let mut notes = Vec::new();
    let mut binary: Option<String> = None;
    let mut metas: Vec<_> = fs::read_dir(out)
        .map_err(|e| format!("{}: {e}", out.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "meta"))
        .filter(|p| {
            let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned());
            let target = stem.as_deref().and_then(|s| s.split('.').next());
            targets.iter().any(|t| Some(t.name.as_str()) == target)
        })
        .collect();
    metas.sort();
    for path in metas {
        let text = read(&path)?;
        let m = meta(&text);
        let field = |k: &str| {
            m.get(k)
                .copied()
                .ok_or_else(|| format!("{}: no {k}", path.display()))
        };
        let sha = field("binary_sha256")?;
        match &binary {
            Some(b) if b != sha => {
                return Err(format!("{}: another binary ({sha})", path.display()));
            }
            _ => binary = Some(sha.to_owned()),
        }
        let label = field("label")?;
        if field("exit")? != "0" {
            notes.push(format!("run {label} failed (exit {})", field("exit")?));
            continue;
        }
        let number = |k: &str| -> Result<f64, String> {
            field(k)?
                .parse()
                .map_err(|_| format!("{}: {k} is not a number", path.display()))
        };
        let (target, run) = label
            .split_once('.')
            .ok_or_else(|| format!("{}: label {label:?} has no run", path.display()))?;
        runs.insert(
            format!("{target}:{run}"),
            Run {
                wall_s: number("wall_s")?,
                peak_rss_bytes: number("peak_rss_bytes")? as u64,
                load_start: number("load_start")?,
                load_end: number("load_end")?,
            },
        );
    }
    let perf = Perf {
        schema: SCHEMA,
        milestone: milestone.to_owned(),
        hardware: read(&out.join("hardware.txt"))?.trim().to_owned(),
        binary_sha256: binary.ok_or("no timed run in the output directory")?,
        runs,
    };
    Ok((perf, notes))
}

/// The directories under `root` that hold a `callgraph.json`, relative and sorted.
pub fn pycg_cases(root: &Path) -> Result<Vec<String>, String> {
    let mut cases = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|n| n == "callgraph.json") {
                let case = dir.strip_prefix(root).map_err(|e| e.to_string())?;
                cases.push(case.to_string_lossy().into_owned());
            }
        }
    }
    cases.sort();
    Ok(cases)
}

/// The dump file of a PyCG case (`bench/run.sh` writes `/` as `__`).
fn pycg_dump(out: &Path, case: &str) -> PathBuf {
    out.join("pycg")
        .join(format!("{}.jsonl", case.replace('/', "__")))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Scores every target of the manifest whose dumps are in `out`; PyCG's cases are read from its
/// clone under `repos`. Notes say what was not scored and why, and what each SCIP run covered.
pub fn scoreboard(
    targets: &[Target],
    out: &Path,
    repos: &Path,
    milestone: &str,
    rb: Rb,
) -> Result<(Scoreboard, Vec<String>), String> {
    let mut board = Scoreboard {
        milestone: milestone.to_owned(),
        rb,
        pins: targets
            .iter()
            .map(|t| (t.name.clone(), t.sha.clone()))
            .collect(),
        keys: BTreeMap::new(),
        cells: BTreeMap::new(),
    };
    let mut notes = Vec::new();
    for t in targets.iter().filter(|t| t.kind == "stress") {
        if !t.modes.iter().any(|m| m == "scip") {
            continue;
        }
        let (Some(scip), Some(noscip)) = (
            read_dump(&out.join(format!("{}.scip.jsonl", t.name)))?,
            read_dump(&out.join(format!("{}.noscip.jsonl", t.name)))?,
        ) else {
            notes.push(format!("{}: no SCIP agreement (a dump is missing)", t.name));
            continue;
        };
        for class in scip_ref::CLASSES {
            let (cell, covered) = scip_ref::agreement(&scip, &noscip, class);
            if class == scip_ref::CLASSES[0] {
                // `rb status` names each SCIP job's outcome (`scip: . scip-java failed (…)`).
                let status = fs::read_to_string(out.join(format!("{}.scip.status", t.name)))
                    .unwrap_or_default();
                let jobs: Vec<&str> = status
                    .lines()
                    .filter_map(|l| l.trim().strip_prefix("scip: "))
                    .map(|l| l.split(',').next().unwrap_or(l))
                    .collect();
                let jobs = if jobs.is_empty() {
                    "none (`rb status` names no SCIP job)".to_owned()
                } else {
                    jobs.join("; ")
                };
                notes.push(format!(
                    "{}: SCIP covers {} of {} files with symbols; jobs: {jobs}",
                    t.name, covered.files, covered.of
                ));
            }
            let key = CellKey {
                bench: "scip".to_owned(),
                target: t.name.clone(),
                mode: "noscip".to_owned(),
                class: class.as_str().to_owned(),
            };
            let record = Record {
                label: Label::Agreement,
                cell,
                covered: Some(covered),
            };
            board.cells.insert(key, record);
        }
    }
    for t in targets.iter().filter(|t| t.kind == "pycg") {
        let snippets = repos.join(&t.name).join("micro-benchmark/snippets");
        let cases = pycg_cases(&snippets)?;
        let missing: Vec<&String> = cases
            .iter()
            .filter(|c| !pycg_dump(out, c).exists())
            .collect();
        if !missing.is_empty() {
            return Err(format!(
                "pycg: {} of {} cases have no dump: {missing:?}",
                missing.len(),
                cases.len()
            ));
        }
        // The answer key's identity: every call graph's path and bytes, in case order.
        let mut key = Sha256::new();
        for case in &cases {
            let callgraph = read(&snippets.join(case).join("callgraph.json"))?;
            key.update(format!("{case}\n").as_bytes());
            key.update(callgraph.as_bytes());
            let dump = read_dump(&pycg_dump(out, case))?.expect("checked above");
            let cells =
                pycg::score_case(&callgraph, &dump).map_err(|e| format!("pycg {case}: {e}"))?;
            for (class, cell) in cells {
                let key = CellKey {
                    bench: "pycg".to_owned(),
                    target: case.clone(),
                    mode: "noscip".to_owned(),
                    class: class.as_str().to_owned(),
                };
                let record = Record {
                    label: Label::Accuracy,
                    cell,
                    covered: None,
                };
                board.cells.insert(key, record);
            }
        }
        board
            .keys
            .insert(format!("{}:callgraph.json", t.name), hex(&key.finalize()));
        notes.push(format!("{}: {} cases scored", t.name, cases.len()));
    }
    Ok((board, notes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROW: &str = "go_clean_arch\tstress\thttps://x/y.git\te06c6d0cb37069b0ef56e3df67f80ca130a1ab82\tnoscip,scip";

    #[test]
    fn manifest_rows_parse_and_bad_rows_fail() {
        let text = format!("# c\nname\tkind\turl\tsha\tmodes\n{ROW}\n");
        let t = parse_manifest(&text).unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].modes, vec!["noscip", "scip"]);
        assert!(
            parse_manifest("a\tb\tc\n")
                .unwrap_err()
                .contains("5 tab-separated")
        );
        let short = ROW.replace("e06c6d0cb37069b0ef56e3df67f80ca130a1ab82", "e06c6d0");
        assert!(parse_manifest(&short).unwrap_err().contains("not a commit"));
    }

    #[test]
    fn the_committed_manifest_parses() {
        let text = include_str!("../../../bench/manifest.tsv");
        let t = parse_manifest(text).unwrap();
        assert_eq!(t.iter().filter(|t| t.kind == "stress").count(), 13);
        assert_eq!(t.iter().filter(|t| t.kind == "pycg").count(), 1);
    }

    fn write_meta(dir: &Path, label: &str, sha: &str, exit: &str) {
        let text = format!(
            "label: {label}\nbinary_sha256: {sha}\ncmd: x\nload_start: 1.5\nfree_start_kib: 9\nexit: {exit}\nwall_s: 2.25\npeak_rss_bytes: 1024\nload_end: 1.75\n"
        );
        fs::write(dir.join(format!("{label}.meta")), text).unwrap();
    }

    #[test]
    fn perf_reads_metas_of_one_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        fs::write(dir.join("hardware.txt"), "Mac16,12 10 cpus\n").unwrap();
        write_meta(dir, "a.noscip-fresh", "s1", "0");
        write_meta(dir, "a.noscip-cached", "s1", "1");
        write_meta(dir, "other.noscip-fresh", "s9", "0");
        let targets = parse_manifest(&ROW.replacen("go_clean_arch", "a", 1)).unwrap();
        let (p, notes) = perf(dir, "m8a", &targets).unwrap();
        assert_eq!(p.hardware, "Mac16,12 10 cpus");
        assert_eq!(p.runs.len(), 1);
        assert_eq!(p.runs["a:noscip-fresh"].wall_s, 2.25);
        assert_eq!(p.runs["a:noscip-fresh"].peak_rss_bytes, 1024);
        assert_eq!(notes, vec!["run a.noscip-cached failed (exit 1)"]);
        write_meta(dir, "b.noscip-fresh", "s2", "0");
        let targets = parse_manifest(&format!(
            "{}\n{}",
            ROW.replacen("go_clean_arch", "a", 1),
            ROW.replacen("go_clean_arch", "b", 1)
        ))
        .unwrap();
        assert!(
            perf(dir, "m8a", &targets)
                .unwrap_err()
                .contains("another binary")
        );
    }

    #[test]
    fn pycg_cases_are_scored_and_a_missing_dump_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let (repos, out) = (tmp.path().join("repos"), tmp.path().join("out"));
        let snippets = repos.join("pycg/micro-benchmark/snippets");
        for case in ["a/one", "b/c/two"] {
            fs::create_dir_all(snippets.join(case)).unwrap();
            fs::write(
                snippets.join(case).join("callgraph.json"),
                r#"{"main": []}"#,
            )
            .unwrap();
        }
        fs::write(snippets.join("README.md"), "x").unwrap();
        assert_eq!(pycg_cases(&snippets).unwrap(), vec!["a/one", "b/c/two"]);
        let target = parse_manifest(
            "pycg\tpycg\thttps://x.git\t8d5dc40837803beef1d8d379fbf2cdad6cd94641\tnoscip\n",
        )
        .unwrap();
        let rb = || Rb {
            commit: "c".to_owned(),
            binary_sha256: "b".to_owned(),
        };
        fs::create_dir_all(out.join("pycg")).unwrap();
        let dump = "[\"rb-dump\",2,\"x\"]\n[\"N\",\"file\",\"main.py\",\"main.py\",{}]\n";
        fs::write(out.join("pycg/a__one.jsonl"), dump).unwrap();
        let err = scoreboard(&target, &out, &repos, "m8a", rb()).unwrap_err();
        assert!(err.contains("1 of 2 cases have no dump"), "{err}");
        fs::write(out.join("pycg/b__c__two.jsonl"), dump).unwrap();
        let (board, _) = scoreboard(&target, &out, &repos, "m8a", rb()).unwrap();
        assert_eq!(board.cells.len(), 4);
        assert!(board.cells.keys().any(|k| k.target == "b/c/two"));
        assert_eq!(board.keys.len(), 1);
        let first = board.keys["pycg:callgraph.json"].clone();
        fs::write(
            snippets.join("a/one/callgraph.json"),
            r#"{"main": ["main.f"]}"#,
        )
        .unwrap();
        let (board, _) = scoreboard(&target, &out, &repos, "m8a", rb()).unwrap();
        assert_ne!(
            board.keys["pycg:callgraph.json"], first,
            "the key hash follows the key"
        );
    }
}
