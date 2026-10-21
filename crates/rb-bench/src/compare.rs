//! The gate (contract M8a, D2, S9). Scores are deterministic, so every count that moves in the
//! bad direction is a real regression: the gate works per cell, and aggregates are printed for
//! reading only. Unlike scoreboards (other pins or keys) are refused, not compared.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use rb_model::Certainty;

use crate::score::{Cell, Status};
use crate::scoreboard::{Perf, Record, Scoreboard};

/// Exit codes: 0 pass, 1 regression, 2 not comparable.
pub const PASS: u8 = 0;
pub const REGRESSION: u8 = 1;
pub const NOT_COMPARABLE: u8 = 2;

/// Accepted regressions for one milestone: `milestone<TAB>cell id<TAB>reason` per line, `#`
/// comments. Lines of another milestone are kept as stale, not applied.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Accept {
    pub cells: BTreeMap<String, String>,
    pub stale: Vec<String>,
}

pub fn parse_accept(text: &str, milestone: &str) -> Result<Accept, String> {
    let mut accept = Accept::default();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let [ms, cell, reason] = line.split('\t').collect::<Vec<_>>()[..] else {
            return Err(format!(
                "accept line {}: expected milestone<TAB>cell<TAB>reason",
                i + 1
            ));
        };
        if reason.trim().is_empty() {
            return Err(format!("accept line {}: a reason is required", i + 1));
        }
        if ms == milestone {
            accept.cells.insert(cell.to_owned(), reason.to_owned());
        } else {
            accept.stale.push(format!("{ms}\t{cell}"));
        }
    }
    Ok(accept)
}

#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub exit: u8,
    pub text: String,
}

/// One way a cell got worse.
#[derive(Debug, PartialEq, Eq)]
struct Worse {
    what: String,
    /// A new wrong `exact` edge: never acceptable.
    exact_fp: bool,
}

pub fn compare(base: &Scoreboard, head: &Scoreboard, accept: &Accept) -> Report {
    let mut out = String::new();
    let unlike = unlike(base, head);
    if !unlike.is_empty() {
        let _ = writeln!(
            out,
            "not comparable (re-score the base against HEAD's pins and keys first):"
        );
        for line in unlike {
            let _ = writeln!(out, "  {line}");
        }
        return Report {
            exit: NOT_COMPARABLE,
            text: out,
        };
    }
    let mut exit = PASS;
    let mut used = BTreeSet::new();
    for (key, b) in &base.cells {
        let id = key.id();
        let worse = match head.cells.get(key) {
            None => vec![Worse {
                what: "cell vanished".to_owned(),
                exact_fp: false,
            }],
            Some(h) => {
                for change in changes(&b.cell, &h.cell) {
                    let _ = writeln!(out, "changed {id}: {change}");
                }
                worse(b, h)
            }
        };
        if worse.is_empty() {
            continue;
        }
        let accepted = accept.cells.get(&id);
        for w in worse {
            match accepted {
                Some(reason) if !w.exact_fp => {
                    used.insert(id.clone());
                    let _ = writeln!(out, "accepted {id}: {} ({reason})", w.what);
                }
                _ => {
                    exit = REGRESSION;
                    let never = if w.exact_fp {
                        " (never acceptable)"
                    } else {
                        ""
                    };
                    let _ = writeln!(out, "REGRESSION {id}: {}{never}", w.what);
                }
            }
        }
    }
    for key in head.cells.keys().filter(|k| !base.cells.contains_key(*k)) {
        let _ = writeln!(out, "new {}", key.id());
    }
    // The `exact` sample (T5): a drawn edge without a verdict, or one more wrong `exact` edge,
    // fails whatever the accept file says.
    let sample_families: BTreeSet<_> = base.samples.keys().chain(head.samples.keys()).collect();
    for family in sample_families {
        let b = base.samples.get(family).copied().unwrap_or_default();
        let Some(h) = head.samples.get(family) else {
            exit = REGRESSION;
            let _ = writeln!(out, "REGRESSION exact sample {family}: family vanished");
            continue;
        };
        if h.missing > 0 {
            exit = REGRESSION;
            let _ = writeln!(
                out,
                "REGRESSION exact sample {family}: {} drawn edges have no verdict (never acceptable)",
                h.missing
            );
        }
        if h.wrong > b.wrong {
            exit = REGRESSION;
            let _ = writeln!(
                out,
                "REGRESSION exact sample {family}: wrong {} → {} (never acceptable)",
                b.wrong, h.wrong
            );
        }
    }
    for id in accept.cells.keys().filter(|id| !used.contains(*id)) {
        let _ = writeln!(out, "unused accept line: {id}");
    }
    for line in &accept.stale {
        let _ = writeln!(out, "stale accept line (another milestone): {line}");
    }
    out.push_str(&aggregates(base, head));
    Report { exit, text: out }
}

fn unlike(base: &Scoreboard, head: &Scoreboard) -> Vec<String> {
    let mut lines = Vec::new();
    for (what, b, h) in [
        ("pin", &base.pins, &head.pins),
        ("key", &base.keys, &head.keys),
    ] {
        for name in b.keys().chain(h.keys()).collect::<BTreeSet<_>>() {
            let (x, y) = (b.get(name), h.get(name));
            if x != y {
                lines.push(format!("{what} {name}: {x:?} → {y:?}"));
            }
        }
    }
    lines
}

fn worse(b: &Record, h: &Record) -> Vec<Worse> {
    let (bc, hc) = (&b.cell, &h.cell);
    let mut w = Vec::new();
    let mut add = |what: String, exact_fp: bool| w.push(Worse { what, exact_fp });
    if hc.tp_total() < bc.tp_total() {
        add(format!("tp {} → {}", bc.tp_total(), hc.tp_total()), false);
    }
    // A right edge that loses certainty (exact → heuristic, heuristic → ambiguous) is worse
    // even when the total holds: the right edges at or above each label must not fall.
    for c in [
        Certainty::Exact,
        Certainty::Dispatch,
        Certainty::Synthetic,
        Certainty::Heuristic,
    ] {
        let at_least = |m: &BTreeMap<Certainty, u64>| -> u64 {
            m.iter().filter(|(k, _)| **k >= c).map(|(_, n)| n).sum()
        };
        let (x, y) = (at_least(&bc.tp), at_least(&hc.tp));
        if y < x {
            add(format!("tp at or above {} {x} → {y}", c.as_str()), false);
        }
    }
    for c in certainties(&bc.fp, &hc.fp) {
        let (x, y) = (count(&bc.fp, &c), count(&hc.fp, &c));
        if y > x {
            add(
                format!("fp {} {x} → {y}", c.as_str()),
                c == Certainty::Exact,
            );
        }
    }
    if hc.fn_ > bc.fn_ {
        add(format!("fn {} → {}", bc.fn_, hc.fn_), false);
    }
    for (name, bm, hm) in reason_maps(bc, hc) {
        for r in bm.keys().chain(hm.keys()).collect::<BTreeSet<_>>() {
            let (x, y) = (count(bm, r), count(hm, r));
            if y > x {
                add(format!("{name}{{{r}}} {x} → {y}"), false);
            }
        }
    }
    if hc.gold < bc.gold {
        add(format!("gold {} → {}", bc.gold, hc.gold), false);
    }
    match (b.covered, h.covered) {
        (Some(x), Some(y)) if y.files * x.of < x.files * y.of => add(
            format!("covered {}/{} → {}/{}", x.files, x.of, y.files, y.of),
            false,
        ),
        (Some(x), None) => add(format!("covered {}/{} → none", x.files, x.of), false),
        _ => {}
    }
    let (bs, hs) = (bc.ambiguous_sites, hc.ambiguous_sites);
    if bs > 0 && hs > 0 && hc.ambiguous_hits * bs < bc.ambiguous_hits * hs {
        add(
            format!(
                "ambiguous site hits {}/{bs} → {}/{hs}",
                bc.ambiguous_hits, hc.ambiguous_hits
            ),
            false,
        );
    }
    let (sb, sh) = (bc.status(), hc.status());
    if sh != sb && sh != Status::Scored {
        add(format!("status {sb:?} → {sh:?}"), false);
    }
    w
}

/// Every count that differs, for classification (the invariant's habit: every diff classified).
fn changes(b: &Cell, h: &Cell) -> Vec<String> {
    let mut lines = Vec::new();
    let mut note = |name: String, x: u64, y: u64| {
        if x != y {
            lines.push(format!("{name} {x} → {y}"));
        }
    };
    note("gold".to_owned(), b.gold, h.gold);
    for (name, bm, hm) in [("tp", &b.tp, &h.tp), ("fp", &b.fp, &h.fp)] {
        for c in certainties(bm, hm) {
            note(
                format!("{name} {}", c.as_str()),
                count(bm, &c),
                count(hm, &c),
            );
        }
    }
    note("fn".to_owned(), b.fn_, h.fn_);
    for (name, bm, hm) in reason_maps(b, h) {
        for r in bm.keys().chain(hm.keys()).collect::<BTreeSet<_>>() {
            note(format!("{name}{{{r}}}"), count(bm, r), count(hm, r));
        }
    }
    note(
        "ambiguous_sites".to_owned(),
        b.ambiguous_sites,
        h.ambiguous_sites,
    );
    note(
        "ambiguous_hits".to_owned(),
        b.ambiguous_hits,
        h.ambiguous_hits,
    );
    lines
}

type ReasonMap = BTreeMap<String, u64>;

fn reason_maps<'a>(b: &'a Cell, h: &'a Cell) -> [(&'static str, &'a ReasonMap, &'a ReasonMap); 4] {
    [
        ("out_of_scope", &b.out_of_scope, &h.out_of_scope),
        ("unmatchable", &b.unmatchable, &h.unmatchable),
        (
            "found_out_of_scope",
            &b.found_out_of_scope,
            &h.found_out_of_scope,
        ),
        (
            "found_unmatchable",
            &b.found_unmatchable,
            &h.found_unmatchable,
        ),
    ]
}

fn certainties(a: &BTreeMap<Certainty, u64>, b: &BTreeMap<Certainty, u64>) -> BTreeSet<Certainty> {
    a.keys().chain(b.keys()).copied().collect()
}

fn count<K: Ord>(m: &BTreeMap<K, u64>, k: &K) -> u64 {
    m.get(k).copied().unwrap_or(0)
}

/// Micro-averaged precision and recall per bench, class, mode and label, base → head.
fn aggregates(base: &Scoreboard, head: &Scoreboard) -> String {
    let groups = |s: &Scoreboard| {
        let mut g: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
        for (k, r) in &s.cells {
            let e = g
                .entry(format!(
                    "{} {} {} {}",
                    k.bench,
                    k.class,
                    k.mode,
                    r.label.as_str()
                ))
                .or_default();
            e.0 += r.cell.tp_total();
            e.1 += r.cell.fp_total();
            e.2 += r.cell.fn_;
        }
        g
    };
    let (gb, gh) = (groups(base), groups(head));
    let mut out = String::from("aggregates (micro-averaged, reading only):\n");
    for name in gb.keys().chain(gh.keys()).collect::<BTreeSet<_>>() {
        let pr = |t: Option<&(u64, u64, u64)>| match t {
            Some(&(tp, fp, fn_)) => format!("P {} R {}", pct(tp, tp + fp), pct(tp, tp + fn_)),
            None => "absent".to_owned(),
        };
        let _ = writeln!(out, "  {name}: {} → {}", pr(gb.get(name)), pr(gh.get(name)));
    }
    out
}

/// A percentage with one decimal, or `n/a` for an empty denominator.
pub fn pct(a: u64, b: u64) -> String {
    if b == 0 {
        "n/a".to_owned()
    } else {
        format!("{:.1}%", 100.0 * a as f64 / b as f64)
    }
}

/// Perf warnings (never a failure: timing is noisy on a shared host, lesson L293).
pub fn compare_perf(base: &Perf, head: &Perf) -> Vec<String> {
    if base.hardware != head.hardware {
        return vec![format!(
            "perf not compared: hardware {:?} → {:?}",
            base.hardware, head.hardware
        )];
    }
    let mut warnings = Vec::new();
    for (run, b) in &base.runs {
        let Some(h) = head.runs.get(run) else {
            warnings.push(format!("perf {run}: run vanished"));
            continue;
        };
        if run.ends_with("-fresh") && h.wall_s > 1.25 * b.wall_s {
            warnings.push(format!(
                "perf {run}: wall {:.1} s → {:.1} s (over 1.25×)",
                b.wall_s, h.wall_s
            ));
        }
        if h.peak_rss_bytes as f64 > 1.15 * b.peak_rss_bytes as f64 {
            warnings.push(format!(
                "perf {run}: peak RSS {} → {} bytes (over 1.15×)",
                b.peak_rss_bytes, h.peak_rss_bytes
            ));
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoreboard::tests::{board, key, record};
    use crate::scoreboard::{Covered, Run};

    fn run(base: &Scoreboard, head: &Scoreboard, accept: &str) -> Report {
        compare(base, head, &parse_accept(accept, "m8a").unwrap())
    }

    fn one(r: Record) -> Scoreboard {
        board(vec![(key("pycg", "a"), r)])
    }

    #[test]
    fn the_exact_sample_gates_missing_verdicts_and_new_wrong_edges() {
        use crate::sample::Tally;
        let mut b = one(record(1, 0, 0));
        b.samples.insert(
            "sql".to_owned(),
            Tally {
                right: 59,
                wrong: 1,
                missing: 0,
            },
        );
        assert_eq!(run(&b, &b, "").exit, PASS);
        let mut h = b.clone();
        h.samples.insert(
            "sql".to_owned(),
            Tally {
                right: 58,
                wrong: 1,
                missing: 1,
            },
        );
        let rep = run(&b, &h, "");
        assert_eq!(rep.exit, REGRESSION);
        assert!(
            rep.text
                .contains("exact sample sql: 1 drawn edges have no verdict"),
            "{}",
            rep.text
        );
        let mut h = b.clone();
        h.samples.insert(
            "sql".to_owned(),
            Tally {
                right: 58,
                wrong: 2,
                missing: 0,
            },
        );
        let rep = run(&b, &h, "m8a\texact sample sql\tno\n");
        assert_eq!(rep.exit, REGRESSION);
        assert!(
            rep.text.contains("wrong 1 → 2 (never acceptable)"),
            "{}",
            rep.text
        );
    }

    /// A family the base sampled but the head does not is a regression: its verdicts would
    /// otherwise leave the gate unread.
    #[test]
    fn a_vanished_sample_family_fails() {
        use crate::sample::Tally;
        let mut b = one(record(1, 0, 0));
        b.samples.insert(
            "sql".to_owned(),
            Tally {
                right: 59,
                wrong: 1,
                missing: 0,
            },
        );
        let h = one(record(1, 0, 0));
        let rep = run(&b, &h, "");
        assert_eq!(rep.exit, REGRESSION, "{}", rep.text);
        assert!(
            rep.text
                .contains("REGRESSION exact sample sql: family vanished"),
            "{}",
            rep.text
        );
    }

    #[test]
    fn identical_boards_pass() {
        let b = one(record(3, 1, 1));
        let r = run(&b, &b, "");
        assert_eq!(r.exit, PASS, "{}", r.text);
        assert!(!r.text.contains("changed"), "{}", r.text);
    }

    #[test]
    fn each_bad_movement_fails() {
        let base = record(3, 1, 1);
        let mut cases: Vec<(&str, Record)> = Vec::new();
        let mut r = base.clone();
        r.cell.tp.insert(Certainty::Heuristic, 2);
        r.cell.fn_ = 2;
        cases.push(("tp 3 → 2", r));
        let mut r = base.clone();
        r.cell.fp.insert(Certainty::Heuristic, 1);
        cases.push(("fp heuristic 0 → 1", r));
        let mut r = base.clone();
        r.cell.out_of_scope.insert("module_scope".to_owned(), 1);
        r.cell.gold += 1;
        cases.push(("out_of_scope{module_scope} 0 → 1", r));
        let mut r = base.clone();
        r.cell
            .found_unmatchable
            .insert("no_defined_in".to_owned(), 1);
        cases.push(("found_unmatchable{no_defined_in} 0 → 1", r));
        let mut r = base.clone();
        r.cell.fn_ = 0;
        r.cell.gold = 3;
        cases.push(("gold 4 → 3", r));
        for (needle, head) in cases {
            let rep = run(&one(base.clone()), &one(head), "");
            assert_eq!(rep.exit, REGRESSION, "{needle}: {}", rep.text);
            assert!(rep.text.contains(needle), "{needle}: {}", rep.text);
        }
    }

    #[test]
    fn certainty_downgrades_of_right_edges_regress() {
        let mut b = record(3, 0, 0);
        b.cell.tp = BTreeMap::from([(Certainty::Exact, 2), (Certainty::Heuristic, 1)]);
        let mut h = b.clone();
        h.cell.tp = BTreeMap::from([(Certainty::Exact, 1), (Certainty::Heuristic, 2)]);
        let rep = run(&one(b.clone()), &one(h), "");
        assert_eq!(rep.exit, REGRESSION, "{}", rep.text);
        assert!(
            rep.text.contains("tp at or above exact 2 → 1"),
            "{}",
            rep.text
        );
        let mut h = b.clone();
        h.cell.tp = BTreeMap::from([(Certainty::Exact, 2), (Certainty::Ambiguous, 1)]);
        let rep = run(&one(b.clone()), &one(h), "");
        assert!(
            rep.text.contains("tp at or above heuristic 3 → 2"),
            "{}",
            rep.text
        );
        let mut up = b.clone();
        up.cell.tp = BTreeMap::from([(Certainty::Exact, 3)]);
        assert_eq!(run(&one(b), &one(up), "").exit, PASS);
    }

    #[test]
    fn coverage_ambiguity_and_status_regress() {
        let mut b = record(3, 1, 1);
        b.covered = Some(Covered { files: 4, of: 8 });
        b.cell.ambiguous_sites = 4;
        b.cell.ambiguous_hits = 2;
        let mut h = b.clone();
        h.covered = Some(Covered { files: 4, of: 9 });
        let rep = run(&one(b.clone()), &one(h), "");
        assert!(
            rep.text
                .contains("REGRESSION pycg:a:noscip:calls: covered 4/8 → 4/9"),
            "{}",
            rep.text
        );
        let mut h = b.clone();
        h.covered = Some(Covered { files: 5, of: 9 });
        assert_eq!(run(&one(b.clone()), &one(h), "").exit, PASS);
        let mut h = b.clone();
        h.cell.ambiguous_sites = 5;
        let rep = run(&one(b.clone()), &one(h), "");
        assert!(
            rep.text.contains("ambiguous site hits 2/4 → 2/5"),
            "{}",
            rep.text
        );
        let mut h = b.clone();
        h.cell.tp.clear();
        h.cell.fp.clear();
        h.cell.fn_ = 4;
        let rep = run(&one(b), &one(h), "");
        assert!(rep.text.contains("status Scored → Flagged"), "{}", rep.text);
    }

    #[test]
    fn accept_lines_excuse_all_but_exact_fp() {
        let b = record(3, 1, 1);
        let mut h = b.clone();
        h.cell.fn_ = 2;
        h.cell.gold = 5;
        let line = "m8a\tpycg:a:noscip:calls\tmodule split in two\n";
        let rep = run(&one(b.clone()), &one(h), line);
        assert_eq!(rep.exit, PASS, "{}", rep.text);
        assert!(
            rep.text
                .contains("accepted pycg:a:noscip:calls: fn 1 → 2 (module split in two)")
        );
        let mut h = b.clone();
        h.cell.fp.insert(Certainty::Exact, 1);
        let rep = run(&one(b.clone()), &one(h), line);
        assert_eq!(rep.exit, REGRESSION, "{}", rep.text);
        assert!(
            rep.text.contains("fp exact 0 → 1 (never acceptable)"),
            "{}",
            rep.text
        );
        let stale = run(&one(b.clone()), &one(b), "m7j\tpycg:a:noscip:calls\told\n");
        assert!(stale.text.contains("stale accept line"), "{}", stale.text);
        assert!(parse_accept("m8a\tx\n", "m8a").is_err());
        assert!(parse_accept("m8a\tx\t \n", "m8a").is_err());
    }

    #[test]
    fn vanished_cells_fail_and_new_cells_are_listed() {
        let b = board(vec![
            (key("pycg", "a"), record(1, 0, 0)),
            (key("pycg", "b"), record(1, 0, 0)),
        ]);
        let h = board(vec![
            (key("pycg", "a"), record(1, 0, 0)),
            (key("pycg", "c"), record(1, 0, 0)),
        ]);
        let rep = run(&b, &h, "");
        assert_eq!(rep.exit, REGRESSION);
        assert!(
            rep.text
                .contains("REGRESSION pycg:b:noscip:calls: cell vanished")
        );
        assert!(rep.text.contains("new pycg:c:noscip:calls"));
    }

    #[test]
    fn other_pins_or_keys_are_not_comparable() {
        let b = one(record(1, 0, 0));
        let mut h = b.clone();
        h.pins.insert("pycg".to_owned(), "q".to_owned());
        let rep = run(&b, &h, "");
        assert_eq!(rep.exit, NOT_COMPARABLE);
        assert!(rep.text.contains("pin pycg"), "{}", rep.text);
        let mut h = b.clone();
        h.keys.clear();
        assert_eq!(run(&b, &h, "").exit, NOT_COMPARABLE);
    }

    #[test]
    fn improvements_pass_and_aggregates_print() {
        let b = one(record(2, 2, 2));
        let h = one(record(3, 1, 1));
        let rep = run(&b, &h, "");
        assert_eq!(rep.exit, PASS, "{}", rep.text);
        assert!(
            rep.text.contains("changed pycg:a:noscip:calls: fn 2 → 1"),
            "{}",
            rep.text
        );
        assert!(
            rep.text
                .contains("pycg calls noscip accuracy: P 50.0% R 50.0% → P 75.0% R 75.0%"),
            "{}",
            rep.text
        );
    }

    #[test]
    fn perf_only_warns_and_only_on_one_hardware() {
        let perf = |wall: f64, rss: u64, hw: &str| Perf {
            schema: 1,
            milestone: "m8a".to_owned(),
            hardware: hw.to_owned(),
            binary_sha256: "b".to_owned(),
            runs: BTreeMap::from([(
                "re_mm:noscip-fresh".to_owned(),
                Run {
                    wall_s: wall,
                    peak_rss_bytes: rss,
                    load_start: 1.0,
                    load_end: 1.0,
                },
            )]),
        };
        assert!(compare_perf(&perf(10.0, 100, "m4"), &perf(12.4, 114, "m4")).is_empty());
        let w = compare_perf(&perf(10.0, 100, "m4"), &perf(12.6, 116, "m4"));
        assert_eq!(w.len(), 2, "{w:?}");
        let w = compare_perf(&perf(10.0, 100, "m4"), &perf(99.0, 999, "m5"));
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("not compared"));
    }
}
