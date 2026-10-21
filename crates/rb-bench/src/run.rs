//! From one `bench/run.sh` output directory to a scoreboard and a perf file (contract M8a, S8):
//! the manifest names the targets and pins, the dumps are scored, the `.meta` files are timed runs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::dump::{Dump, parse};
use crate::scoreboard::{CellKey, Label, Perf, Rb, Record, Run, SCHEMA, Scoreboard};
use crate::{handkey, msecend, pycg, sample, scip_ref};

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

/// Refuse to score output from a different revision than the target pinned by the manifest.
pub fn check_pin(dump: &Dump, target: &Target, mode: &str) -> Result<(), String> {
    if !dump.commit.eq_ignore_ascii_case(&target.sha) {
        return Err(format!(
            "{}.{mode}.jsonl: dump commit {} does not match manifest pin {}",
            target.name, dump.commit, target.sha
        ));
    }
    Ok(())
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

/// A microSecEnD application's dataset directory, from its upstream URL
/// (`https://github.com/Owner/Repo.git` is `owner_repo`).
pub fn msecend_app(url: &str) -> Result<String, String> {
    let slug = url
        .strip_prefix("https://github.com/")
        .and_then(|s| s.strip_suffix(".git"))
        .ok_or_else(|| format!("not a GitHub URL: {url}"))?;
    Ok(slug.replacen('/', "_", 1).to_lowercase())
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
    keys: &Path,
    samples: &Path,
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
        samples: BTreeMap::new(),
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
        check_pin(&scip, t, "scip")?;
        check_pin(&noscip, t, "noscip")?;
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
    // Hand keys: every target with `<keys>/<name>/key.txt`, raw and adjudicated (T3, T4).
    for t in targets {
        let dir = keys.join(&t.name);
        let Ok(raw_text) = fs::read_to_string(dir.join("key.txt")) else {
            continue;
        };
        let adjudication = fs::read_to_string(dir.join("adjudication.md")).unwrap_or_default();
        let raw = handkey::parse_key(&raw_text).map_err(|e| format!("{}: {e}", t.name))?;
        let effective = handkey::adjudicate(raw.clone(), &adjudication)
            .map_err(|e| format!("{}: {e}", t.name))?;
        let checkout = repos.join(&t.name);
        handkey::check_places(&effective, &checkout).map_err(|e| format!("{}: {e}", t.name))?;
        let mut hash = Sha256::new();
        hash.update(raw_text.as_bytes());
        hash.update(adjudication.as_bytes());
        board
            .keys
            .insert(format!("handkey:{}", t.name), hex(&hash.finalize()));
        let dump = read_dump(&out.join(format!("{}.noscip.jsonl", t.name)))?
            .ok_or_else(|| format!("{}: no noscip dump for its hand key", t.name))?;
        check_pin(&dump, t, "noscip")?;
        for (bench, key) in [(handkey::RAW_BENCH, &raw), ("handkey", &effective)] {
            for (class, cell) in handkey::score_key(key, &dump) {
                let key = CellKey {
                    bench: bench.to_owned(),
                    target: t.name.clone(),
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
        notes.push(format!(
            "{}: hand key {} lines, {} after adjudication",
            t.name,
            raw.len(),
            effective.len()
        ));
    }
    // The `exact` sample (T5): drawn from the stress dumps, tallied against the verdicts at each
    // target's pin. A drawn edge without a verdict is `missing`, which the gate refuses.
    let verdict_text = fs::read_to_string(samples).unwrap_or_default();
    let verdicts = sample::parse_verdicts(&verdict_text)?;
    let mut stress_dumps = Vec::new();
    for t in targets.iter().filter(|t| t.kind == "stress") {
        for mode in ["noscip", "scip"] {
            if let Some(d) = read_dump(&out.join(format!("{}.{mode}.jsonl", t.name)))? {
                check_pin(&d, t, mode)?;
                stress_dumps.push((t.name.clone(), mode, d));
            }
        }
    }
    if !stress_dumps.is_empty() {
        let drawn = sample::draw(stress_dumps.iter().map(|(t, m, d)| (t.as_str(), *m, d)));
        board.samples = sample::tally(&drawn, &board.pins, &verdicts);
        board.keys.insert(
            "exact:verdicts".to_owned(),
            hex(&Sha256::digest(verdict_text.as_bytes())),
        );
        let missing: u64 = board.samples.values().map(|t| t.missing).sum();
        if missing > 0 {
            notes.push(format!(
                "exact sample: {missing} drawn edges have no verdict"
            ));
        }
    }
    let mut models = Sha256::new();
    let apps: Vec<&Target> = targets.iter().filter(|t| t.kind == "microsecend").collect();
    for t in &apps {
        let app = msecend_app(&t.url)?;
        let dir = repos.join("microsecend").join("dataset").join(&app);
        let model = read(&dir.join(format!("{app}.json")))?;
        let trace = read(&dir.join(format!("{app}_traceability.json")))?;
        models.update(format!("{app}\n").as_bytes());
        // The app's endpoint overrides, from a blind adjudication of the model (T2).
        let overrides = fs::read_to_string(keys.join("microsecend").join(format!("{}.md", t.name)))
            .unwrap_or_default();
        models.update(model.as_bytes());
        models.update(trace.as_bytes());
        models.update(overrides.as_bytes());
        let dump = read_dump(&out.join(format!("{}.noscip.jsonl", t.name)))?
            .ok_or_else(|| format!("{}: no noscip dump", t.name))?;
        check_pin(&dump, t, "noscip")?;
        let (cells, app_notes) = msecend::score_app(&model, &trace, &overrides, &dump)
            .map_err(|e| format!("{}: {e}", t.name))?;
        for (class, cell) in cells {
            let key = CellKey {
                bench: "microsecend".to_owned(),
                target: t.name.clone(),
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
        notes.push(format!("{}: {}", t.name, app_notes.join("; ")));
    }
    if !apps.is_empty() {
        board
            .keys
            .insert("microsecend:models".to_owned(), hex(&models.finalize()));
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
        assert_eq!(t.iter().filter(|t| t.kind == "microsecend").count(), 12);
        assert_eq!(t.iter().filter(|t| t.kind == "dataset").count(), 1);
        for a in t.iter().filter(|t| t.kind == "microsecend") {
            assert!(msecend_app(&a.url).is_ok(), "{}", a.url);
        }
        assert_eq!(
            msecend_app("https://github.com/jferrater/Tap-And-Eat-MicroServices.git").unwrap(),
            "jferrater_tap-and-eat-microservices"
        );
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
        let err = scoreboard(
            &target,
            &out,
            &repos,
            &tmp.path().join("keys"),
            &tmp.path().join("exact.tsv"),
            "m8a",
            rb(),
        )
        .unwrap_err();
        assert!(err.contains("1 of 2 cases have no dump"), "{err}");
        fs::write(out.join("pycg/b__c__two.jsonl"), dump).unwrap();
        let (board, _) = scoreboard(
            &target,
            &out,
            &repos,
            &tmp.path().join("keys"),
            &tmp.path().join("exact.tsv"),
            "m8a",
            rb(),
        )
        .unwrap();
        assert_eq!(board.cells.len(), 4);
        assert!(board.cells.keys().any(|k| k.target == "b/c/two"));
        assert_eq!(board.keys.len(), 1);
        let first = board.keys["pycg:callgraph.json"].clone();
        fs::write(
            snippets.join("a/one/callgraph.json"),
            r#"{"main": ["main.f"]}"#,
        )
        .unwrap();
        let (board, _) = scoreboard(
            &target,
            &out,
            &repos,
            &tmp.path().join("keys"),
            &tmp.path().join("exact.tsv"),
            "m8a",
            rb(),
        )
        .unwrap();
        assert_ne!(
            board.keys["pycg:callgraph.json"], first,
            "the key hash follows the key"
        );
    }

    /// A dump is scored only against the revision the manifest pins: a dump whose header names
    /// another commit is refused, and the pin matches whatever the letter case.
    #[test]
    fn a_dump_of_another_commit_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let (out, repos, keys) = (
            tmp.path().join("out"),
            tmp.path().join("repos"),
            tmp.path().join("keys"),
        );
        for d in [&out, &repos.join("t/a"), &keys.join("t")] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(repos.join("t/a/X.java"), "class X {}\n").unwrap();
        fs::write(keys.join("t/key.txt"), "").unwrap();
        let target = parse_manifest(
            &ROW.replacen("go_clean_arch", "t", 1)
                .replace("noscip,scip", "noscip")
                .replacen("stress", "handkey", 1),
        )
        .unwrap();
        let pin = target[0].sha.clone();
        let score = || {
            scoreboard(
                &target,
                &out,
                &repos,
                &keys,
                &tmp.path().join("exact.tsv"),
                "m9d",
                Rb {
                    commit: "c".into(),
                    binary_sha256: "b".into(),
                },
            )
        };
        let dump = |commit: &str| format!("[\"rb-dump\",2,\"{commit}\"]\n");
        fs::write(
            out.join("t.noscip.jsonl"),
            dump("0123456789abcdef0123456789abcdef01234567"),
        )
        .unwrap();
        let err = score().unwrap_err();
        assert!(err.contains("does not match manifest pin"), "{err}");
        assert!(err.contains("t.noscip.jsonl"), "{err}");
        fs::write(out.join("t.noscip.jsonl"), dump(&pin.to_uppercase())).unwrap();
        assert!(score().is_ok(), "the pin in upper case is the pin");
    }

    #[test]
    fn a_hand_key_is_scored_raw_and_adjudicated() {
        let tmp = tempfile::tempdir().unwrap();
        let (out, repos, keys) = (
            tmp.path().join("out"),
            tmp.path().join("repos"),
            tmp.path().join("keys"),
        );
        for d in [&out, &repos.join("t/a"), &keys.join("t")] {
            fs::create_dir_all(d).unwrap();
        }
        fs::write(repos.join("t/a/X.java"), "class X {\n  void q() {}\n}\n").unwrap();
        fs::write(
            keys.join("t/key.txt"),
            "QUERIES users read a/X.java:2\nMAPS users a/X.java:1\n",
        )
        .unwrap();
        fs::write(
            keys.join("t/adjudication.md"),
            "- REMOVE `MAPS users a/X.java:1` — no entity\n",
        )
        .unwrap();
        let dump = [
            // The dump of the pinned commit (`ROW`'s sha), as `check_pin` requires.
            r#"["rb-dump",2,"e06c6d0cb37069b0ef56e3df67f80ca130a1ab82"]"#,
            r#"["N","symbol","rb-heuristic . . . `a/X.java`/X#q().","q",{}]"#,
            r#"["E","symbol","rb-heuristic . . . `a/X.java`/X#q().","DEFINED_IN","file","a/X.java","treesitter",5,{},[["a/X.java",[1,7,1,8]]]]"#,
            r#"["E","symbol","rb-heuristic . . . `a/X.java`/X#q().","QUERIES","table","public.users","sql_literal",5,{"ops":["read"],"schema_assumed":true},[["a/X.java",[1,10,1,11]]]]"#,
        ];
        fs::write(out.join("t.noscip.jsonl"), dump.join("\n") + "\n").unwrap();
        let target = parse_manifest(
            &ROW.replacen("go_clean_arch", "t", 1)
                .replace("noscip,scip", "noscip")
                .replacen("stress", "handkey", 1),
        )
        .unwrap();
        let rb = Rb {
            commit: "c".into(),
            binary_sha256: "b".into(),
        };
        let (board, _) = scoreboard(
            &target,
            &out,
            &repos,
            &keys,
            &tmp.path().join("exact.tsv"),
            "m8b",
            rb,
        )
        .unwrap();
        let cell = |bench: &str, class: &str| {
            board
                .cells
                .iter()
                .find(|(k, _)| k.bench == bench && k.class == class)
                .map(|(_, r)| r.cell.clone())
                .unwrap()
        };
        assert_eq!(
            cell("handkey_raw", "table_maps").fn_,
            1,
            "the raw key's MAPS line"
        );
        assert_eq!(
            cell("handkey", "table_maps").gold,
            0,
            "removed by adjudication"
        );
        assert_eq!(cell("handkey", "table_queries").tp_total(), 1);
        assert!(board.keys.contains_key("handkey:t"));
        fs::write(keys.join("t/key.txt"), "MAPS users a/X.java:9\n").unwrap();
        fs::write(keys.join("t/adjudication.md"), "").unwrap();
        let rb = Rb {
            commit: "c".into(),
            binary_sha256: "b".into(),
        };
        let err = scoreboard(
            &target,
            &out,
            &repos,
            &keys,
            &tmp.path().join("exact.tsv"),
            "m8b",
            rb,
        )
        .unwrap_err();
        assert!(err.contains("past the end"), "{err}");
    }
}
