//! Markdown views of a scoreboard (contract M8a, S6, S10). Every number is derived from the
//! stored counts here, so a Result section quotes tool output, never hand-typed figures (L3).

use std::collections::BTreeMap;
use std::fmt::Write as _;

use rb_model::Certainty;

use crate::compare::pct;
use crate::score::{Cell, Status};
use crate::scoreboard::{Label, Scoreboard};

/// Strongest first, as the certainty table reads.
const LABELS: [Certainty; 5] = [
    Certainty::Exact,
    Certainty::Dispatch,
    Certainty::Synthetic,
    Certainty::Heuristic,
    Certainty::Ambiguous,
];

#[derive(Default)]
struct Group {
    cells: [u64; 3],
    total: Cell,
}

fn add(into: &mut Cell, c: &Cell) {
    into.gold += c.gold;
    into.fn_ += c.fn_;
    into.ambiguous_sites += c.ambiguous_sites;
    into.ambiguous_hits += c.ambiguous_hits;
    for (to, from) in [(&mut into.tp, &c.tp), (&mut into.fp, &c.fp)] {
        for (k, n) in from {
            *to.entry(*k).or_default() += n;
        }
    }
    for (to, from) in [
        (&mut into.out_of_scope, &c.out_of_scope),
        (&mut into.unmatchable, &c.unmatchable),
    ] {
        for (k, n) in from {
            *to.entry(k.clone()).or_default() += n;
        }
    }
}

fn reasons(m: &BTreeMap<String, u64>) -> String {
    if m.is_empty() {
        return "–".to_owned();
    }
    m.iter()
        .map(|(r, n)| format!("{r} {n}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// One row per bench, class, mode and label, micro-averaged over its targets.
pub fn render(board: &Scoreboard) -> String {
    let mut groups: BTreeMap<(&str, &str, &str, Label), Group> = BTreeMap::new();
    for (k, r) in &board.cells {
        let g = groups
            .entry((&k.bench, &k.class, &k.mode, r.label))
            .or_default();
        g.cells[match r.cell.status() {
            Status::Scored => 0,
            Status::Flagged => 1,
            Status::Vacuous => 2,
        }] += 1;
        add(&mut g.total, &r.cell);
    }
    let mut out = format!(
        "Scoreboard {} (rb {}, binary {})\n\n",
        board.milestone, board.rb.commit, board.rb.binary_sha256
    );
    out.push_str("| Bench | Class | Mode | Label | Targets (scored / flagged / vacuous) | Gold | In scope | TP | FP | FN | Precision | Recall | Out of scope | Unmatchable |\n");
    out.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
    for ((bench, class, mode, label), g) in &groups {
        let c = &g.total;
        let (tp, fp) = (c.tp_total(), c.fp_total());
        let _ = writeln!(
            out,
            "| {bench} | {class} | {mode} | {} | {} / {} / {} | {} | {} | {tp} | {fp} | {} | {} | {} | {} | {} |",
            label.as_str(),
            g.cells[0],
            g.cells[1],
            g.cells[2],
            c.gold,
            c.in_scope_gold(),
            c.fn_,
            pct(tp, tp + fp),
            pct(tp, c.in_scope_gold()),
            reasons(&c.out_of_scope),
            reasons(&c.unmatchable),
        );
    }
    out
}

/// How often each certainty label is right, per kind of key (S6). `ambiguous` is read as the
/// share of its call sites whose candidates include the right target.
pub fn render_certainty(board: &Scoreboard) -> String {
    let mut totals: BTreeMap<Label, Cell> = BTreeMap::new();
    // `handkey_raw` restates `handkey` before adjudication: counted once, adjudicated.
    for (_, r) in board
        .cells
        .iter()
        // `table_ops` restates `table_queries` edges once per op: the edges count once.
        .filter(|(k, _)| k.bench != crate::handkey::RAW_BENCH && k.class != "table_ops")
    {
        add(totals.entry(r.label).or_default(), &r.cell);
    }
    let mut out =
        String::from("| Label | Accuracy (independent keys) | Agreement (SCIP) |\n|---|---|---|\n");
    for c in LABELS {
        let cols: Vec<String> = [Label::Accuracy, Label::Agreement]
            .iter()
            .map(|l| column(c, totals.get(l)))
            .collect();
        let _ = writeln!(out, "| {} | {} | {} |", c.as_str(), cols[0], cols[1]);
    }
    if !board.samples.is_empty() {
        out.push_str("\n| `exact` sample family | Right / judged | Wilson 95 % | Without a verdict |\n|---|---|---|---|\n");
        for (family, t) in &board.samples {
            let n = t.right + t.wrong;
            let interval = crate::sample::wilson(t.right, n)
                .map_or("n/a".to_owned(), |(lo, hi)| {
                    format!("{:.1}–{:.1} %", 100.0 * lo, 100.0 * hi)
                });
            let _ = writeln!(
                out,
                "| {family} | {} / {n} | {interval} | {} |",
                t.right, t.missing
            );
        }
    }
    out
}

fn column(c: Certainty, total: Option<&Cell>) -> String {
    if c == Certainty::Dispatch {
        return "n/a (not stored)".to_owned();
    }
    let (tp, fp) = total.map_or((0, 0), |t| {
        let n = |m: &BTreeMap<Certainty, u64>| m.get(&c).copied().unwrap_or(0);
        (n(&t.tp), n(&t.fp))
    });
    if tp + fp == 0 {
        return "no edges".to_owned();
    }
    let edges = format!("{} of {} edges", pct(tp, tp + fp), tp + fp);
    match total {
        Some(t) if c == Certainty::Ambiguous && t.ambiguous_sites > 0 => format!(
            "site hit {} of {} sites; {edges}",
            pct(t.ambiguous_hits, t.ambiguous_sites),
            t.ambiguous_sites
        ),
        _ => edges,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scoreboard::tests::{board, key, record};

    fn sample() -> Scoreboard {
        let mut agree = record(8, 2, 2);
        agree.label = Label::Agreement;
        agree.cell.tp.insert(Certainty::Exact, 0);
        agree.cell.ambiguous_sites = 2;
        agree.cell.ambiguous_hits = 1;
        let mut scip = key("scip", "a");
        scip.mode = "noscip".to_owned();
        let mut oos = record(3, 1, 1);
        oos.cell.out_of_scope.insert("module_scope".to_owned(), 5);
        oos.cell.gold += 5;
        board(vec![
            (key("pycg", "a"), oos),
            (key("pycg", "b"), record(0, 0, 0)),
            (scip, agree),
        ])
    }

    #[test]
    fn render_golden() {
        let expected = "\
Scoreboard m8a (rb c, binary b)

| Bench | Class | Mode | Label | Targets (scored / flagged / vacuous) | Gold | In scope | TP | FP | FN | Precision | Recall | Out of scope | Unmatchable |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| pycg | calls | noscip | accuracy | 1 / 0 / 1 | 9 | 4 | 3 | 1 | 1 | 75.0% | 75.0% | module_scope 5 | – |
| scip | calls | noscip | agreement | 1 / 0 / 0 | 10 | 10 | 8 | 2 | 2 | 80.0% | 80.0% | – | – |
";
        assert_eq!(render(&sample()), expected);
    }

    #[test]
    fn table_ops_are_not_counted_as_edges() {
        let mut r = record(1, 0, 0);
        r.cell.tp = BTreeMap::from([(Certainty::Synthetic, 1)]);
        let mut q = key("handkey", "t");
        q.class = "table_queries".to_owned();
        let mut ops = key("handkey", "t");
        ops.class = "table_ops".to_owned();
        let text = render_certainty(&board(vec![(q, r.clone()), (ops, r)]));
        assert!(text.contains("| synthetic | 100.0% of 1 edges |"), "{text}");
    }

    #[test]
    fn a_raw_hand_key_is_not_counted_twice() {
        let mut r = record(1, 0, 0);
        r.cell.tp = BTreeMap::from([(Certainty::Exact, 1)]);
        let mut raw = key("handkey_raw", "t");
        raw.class = "endpoints".to_owned();
        let mut adj = key("handkey", "t");
        adj.class = "endpoints".to_owned();
        let text = render_certainty(&board(vec![(raw, r.clone()), (adj, r)]));
        assert!(text.contains("| exact | 100.0% of 1 edges |"), "{text}");
    }

    #[test]
    fn the_exact_sample_renders_with_wilson_intervals() {
        let mut b = sample();
        b.samples.insert(
            "sql".to_owned(),
            crate::sample::Tally {
                right: 30,
                wrong: 30,
                missing: 2,
            },
        );
        let text = render_certainty(&b);
        assert!(
            text.contains("| sql | 30 / 60 | 37.7–62.3 % | 2 |"),
            "{text}"
        );
    }

    #[test]
    fn certainty_golden() {
        let expected = "\
| Label | Accuracy (independent keys) | Agreement (SCIP) |
|---|---|---|
| exact | no edges | no edges |
| dispatch | n/a (not stored) | n/a (not stored) |
| synthetic | no edges | no edges |
| heuristic | 100.0% of 3 edges | 100.0% of 8 edges |
| ambiguous | 0.0% of 1 edges | site hit 50.0% of 2 sites; 0.0% of 2 edges |
";
        assert_eq!(render_certainty(&sample()), expected);
    }
}
