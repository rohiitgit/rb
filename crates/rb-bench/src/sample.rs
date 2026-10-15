//! The `exact` sample (contract M8b, T5): how often an edge rb labels `exact` is right, judged by
//! reading the source. Per family (`scip`, `sql`, `orm`, `routes`, `manifest`) at most 60 edges
//! are drawn from the stress repositories' dumps, stratified by repository (largest remainder)
//! and within one by the M3b rule (sorted bytewise, `N = ⌊count/k⌋`, indices `⌊N/2⌋ + i·N`). The
//! draw is a pure function of the dumps, so each binary redraws it; verdicts are keyed by rb's own
//! edge identity and the repository's pin, and carry over for every edge still drawn.

use std::collections::BTreeMap;

use rb_model::Certainty;
use rb_model::diff::EdgeRow;

use crate::dump::Dump;

pub const FAMILIES: [&str; 5] = ["manifest", "orm", "routes", "scip", "sql"];
pub const PER_FAMILY: usize = 60;

/// Which family an `exact` edge of a `noscip` or `scip` dump belongs to. SCIP edges are taken
/// from the SCIP dump only, everything else from the `noscip` dump; structure (definitions,
/// containment, a file's service, CODEOWNERS) is not sampled.
pub fn family(e: &EdgeRow, mode: &str) -> Option<&'static str> {
    use rb_model::{EdgeKind as K, Provenance as P};
    if e.certainty != Certainty::Exact {
        return None;
    }
    match (mode, e.provenance, e.kind) {
        ("scip", P::Scip, _) => Some("scip"),
        ("noscip", P::SqlLiteral | P::SqlFile, _) => Some("sql"),
        ("noscip", P::Orm, _) => Some("orm"),
        ("noscip", _, K::Exposes | K::HandledBy) => Some("routes"),
        ("noscip", P::Manifest, K::DependsOn) => Some("manifest"),
        _ => None,
    }
}

/// The dump mode a family is drawn from.
pub fn family_mode(family: &str) -> &'static str {
    if family == "scip" { "scip" } else { "noscip" }
}

/// rb's identity of an edge (`rb_model::diff::EdgeId`), as one line of text.
pub fn identity(e: &EdgeRow) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}",
        e.src_kind, e.src_key, e.kind, e.dst_kind, e.dst_key, e.provenance
    )
}

/// The M3b rule: `k` of `sorted`, evenly spaced from the middle of the first stride (all of them
/// when there are no more than `k`).
pub fn m3b<T>(sorted: &[T], k: usize) -> Vec<&T> {
    if sorted.len() <= k {
        return sorted.iter().collect();
    }
    let n = sorted.len() / k;
    (0..k).map(|i| &sorted[n / 2 + i * n]).collect()
}

/// `total` split over the strata in proportion to their sizes by largest remainder, each stratum
/// with an edge getting at least one when `total` allows; never more than a stratum holds.
pub fn allocate(sizes: &BTreeMap<String, usize>, total: usize) -> BTreeMap<String, usize> {
    let all: usize = sizes.values().sum();
    if all <= total {
        return sizes.clone();
    }
    let mut out: BTreeMap<String, usize> = BTreeMap::new();
    let mut rest: Vec<(usize, &String)> = Vec::new();
    for (s, &n) in sizes {
        let share = n * total;
        out.insert(s.clone(), (share / all).max(usize::from(n > 0)).min(n));
        rest.push((share % all, s));
    }
    // Largest remainders first, ties by name, until the total is reached.
    rest.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let mut given: usize = out.values().sum();
    for (_, s) in rest.iter().cycle().take(rest.len() * 2) {
        if given >= total {
            break;
        }
        if out[*s] < sizes[*s] {
            *out.get_mut(*s).expect("allocated") += 1;
            given += 1;
        }
    }
    // Minimums can overshoot: take back from the largest allocations.
    while given > total {
        let (s, _) = out
            .iter()
            .max_by_key(|(s, n)| (**n, std::cmp::Reverse((*s).clone())))
            .expect("non-empty");
        let s = s.clone();
        *out.get_mut(&s).expect("allocated") -= 1;
        given -= 1;
    }
    out
}

/// A drawn edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Drawn {
    pub family: &'static str,
    pub target: String,
    pub identity: String,
}

/// The draw over `(target, mode, dump)` triples.
pub fn draw<'a>(dumps: impl IntoIterator<Item = (&'a str, &'a str, &'a Dump)>) -> Vec<Drawn> {
    let mut pools: BTreeMap<&'static str, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for (target, mode, dump) in dumps {
        for e in &dump.edges {
            if let Some(f) = family(e, mode) {
                let pool = pools
                    .entry(f)
                    .or_default()
                    .entry(target.to_owned())
                    .or_default();
                pool.push(identity(e));
            }
        }
    }
    let mut out = Vec::new();
    for (family, by_target) in pools {
        let sizes = by_target
            .iter()
            .map(|(t, v)| (t.clone(), v.len()))
            .collect();
        let k = allocate(&sizes, PER_FAMILY);
        for (target, mut ids) in by_target {
            ids.sort();
            ids.dedup();
            for id in m3b(&ids, k[&target]) {
                out.push(Drawn {
                    family,
                    target: target.clone(),
                    identity: id.clone(),
                });
            }
        }
    }
    out
}

/// One judged edge of `bench/samples/exact.tsv`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub target: String,
    pub pin: String,
    pub identity: String,
    pub right: bool,
}

/// `exact.tsv`: `family<TAB>target<TAB>pin<TAB>identity<TAB>TRUE|FALSE<TAB>evidence`, `#` comments.
pub fn parse_verdicts(text: &str) -> Result<Vec<Verdict>, String> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        let [_, target, pin, identity, verdict, evidence] = f[..] else {
            return Err(format!(
                "exact.tsv line {}: expected 6 tab-separated fields",
                i + 1
            ));
        };
        let right = match verdict {
            "TRUE" => true,
            "FALSE" => false,
            v => {
                return Err(format!(
                    "exact.tsv line {}: verdict {v:?} is not TRUE or FALSE",
                    i + 1
                ));
            }
        };
        if evidence.trim().is_empty() {
            return Err(format!(
                "exact.tsv line {}: a verdict needs file:line evidence",
                i + 1
            ));
        }
        out.push(Verdict {
            target: target.to_owned(),
            pin: pin.to_owned(),
            identity: identity.to_owned(),
            right,
        });
    }
    Ok(out)
}

/// Per family: verdicts found right, found wrong, and drawn edges without a verdict at this pin.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Tally {
    pub right: u64,
    pub wrong: u64,
    pub missing: u64,
}

pub fn tally(
    drawn: &[Drawn],
    pins: &BTreeMap<String, String>,
    verdicts: &[Verdict],
) -> BTreeMap<String, Tally> {
    let by: BTreeMap<(&str, &str, &str), bool> = verdicts
        .iter()
        .map(|v| {
            (
                (v.target.as_str(), v.pin.as_str(), v.identity.as_str()),
                v.right,
            )
        })
        .collect();
    let mut out: BTreeMap<String, Tally> = FAMILIES
        .iter()
        .map(|f| ((*f).to_owned(), Tally::default()))
        .collect();
    for d in drawn {
        let pin = pins.get(&d.target).map_or("", String::as_str);
        let t = out.get_mut(d.family).expect("every family");
        match by.get(&(d.target.as_str(), pin, d.identity.as_str())) {
            Some(true) => t.right += 1,
            Some(false) => t.wrong += 1,
            None => t.missing += 1,
        }
    }
    out
}

/// The Wilson score interval at 95 % for `k` right out of `n`.
pub fn wilson(k: u64, n: u64) -> Option<(f64, f64)> {
    if n == 0 {
        return None;
    }
    let (k, n, z) = (k as f64, n as f64, 1.96_f64);
    let p = k / n;
    let denom = 1.0 + z * z / n;
    let centre = (p + z * z / (2.0 * n)) / denom;
    let half = z * ((p * (1.0 - p) / n) + z * z / (4.0 * n * n)).sqrt() / denom;
    Some(((centre - half).max(0.0), (centre + half).min(1.0)))
}

/// A drawn edge as a judge reads it: its kind and attributes, each end with its definition
/// site, and the evidence sites, all 1-based.
pub fn describe(dump: &Dump, locator: &crate::identity::Locator, identity: &str) -> Option<String> {
    let e = dump.edges.iter().find(|e| self::identity(e) == identity)?;
    let end = |kind: rb_model::NodeKind, key: &str| match locator.loc(kind, key) {
        Ok(l) if l.line > 0 => format!(
            "{kind} {key} (defined at {}:{}, name {})",
            l.path, l.line, l.name
        ),
        _ => format!("{kind} {key}"),
    };
    let sites: Vec<String> = e
        .evidence
        .iter()
        .map(|ev| format!("{}:{}", ev.path, ev.span[0] + 1))
        .collect();
    Some(format!(
        "  edge: {} ({}, exact) {}\n  from: {}\n  to:   {}\n  evidence: {}",
        e.kind,
        e.provenance,
        serde_json::Value::Object(e.attrs.clone()),
        end(e.src_kind, &e.src_key),
        end(e.dst_kind, &e.dst_key),
        if sites.is_empty() {
            "none".to_owned()
        } else {
            sites.join(", ")
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_m3b_rule() {
        let v: Vec<u32> = (0..100).collect();
        assert_eq!(
            m3b(&v, 20).into_iter().copied().collect::<Vec<_>>(),
            (0..20).map(|i| 2 + 5 * i).collect::<Vec<_>>()
        );
        assert_eq!(
            m3b(&v, 3).into_iter().copied().collect::<Vec<_>>(),
            vec![16, 49, 82]
        );
        assert_eq!(m3b(&v[..5], 20).len(), 5);
    }

    #[test]
    fn allocation_is_proportional_with_a_floor_and_a_cap() {
        let sizes: BTreeMap<String, usize> = [("big", 9000), ("mid", 900), ("one", 1)]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        let a = allocate(&sizes, 60);
        assert_eq!(a.values().sum::<usize>(), 60);
        assert_eq!(a["one"], 1);
        assert!(a["big"] > a["mid"] && a["mid"] >= 5, "{a:?}");
        let small: BTreeMap<String, usize> = [("a", 3), ("b", 4)]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect();
        assert_eq!(allocate(&small, 60), small);
    }

    #[test]
    fn wilson_intervals() {
        let (lo, hi) = wilson(60, 60).unwrap();
        assert!((lo - 0.9398).abs() < 1e-3 && hi > 0.9999, "{lo} {hi}");
        let (lo, hi) = wilson(30, 60).unwrap();
        assert!(
            (lo - 0.3773).abs() < 1e-3 && (hi - 0.6227).abs() < 1e-3,
            "{lo} {hi}"
        );
        assert_eq!(wilson(0, 0), None);
    }

    #[test]
    fn verdicts_parse_and_tally() {
        let text = "# c\nsql\tr\tp1\tid1\tTRUE\ta.sql:3 INSERT\nsql\tr\tp1\tid2\tFALSE\ta.go:9 no such call\n";
        let v = parse_verdicts(text).unwrap();
        assert_eq!(v.len(), 2);
        assert!(parse_verdicts("sql\tr\tp\tid\tMAYBE\tx\n").is_err());
        assert!(
            parse_verdicts("sql\tr\tp\tid\tTRUE\t \n")
                .unwrap_err()
                .contains("evidence")
        );
        let drawn: Vec<Drawn> = ["id1", "id2", "id3"]
            .iter()
            .map(|i| Drawn {
                family: "sql",
                target: "r".into(),
                identity: (*i).into(),
            })
            .collect();
        let pins = BTreeMap::from([("r".to_owned(), "p1".to_owned())]);
        let t = tally(&drawn, &pins, &v);
        assert_eq!(
            t["sql"],
            Tally {
                right: 1,
                wrong: 1,
                missing: 1
            }
        );
        let moved = BTreeMap::from([("r".to_owned(), "p2".to_owned())]);
        assert_eq!(
            tally(&drawn, &moved, &v)["sql"].missing,
            3,
            "verdicts hold only at their pin"
        );
    }

    #[test]
    fn families_of_real_rows() {
        let d = crate::dump::parse(include_str!(
            "../../rb-cli/tests/golden/polyglot-monorepo/dump.jsonl"
        ))
        .unwrap();
        let fams: std::collections::BTreeSet<&str> =
            d.edges.iter().filter_map(|e| family(e, "noscip")).collect();
        assert!(fams.contains("routes") && fams.contains("sql"), "{fams:?}");
        let py = crate::dump::parse(include_str!(
            "../../rb-cli/tests/golden/python-fastapi-sqlalchemy/dump.jsonl"
        ))
        .unwrap();
        assert!(
            py.edges.iter().any(|e| family(e, "noscip") == Some("orm")),
            "an exact ORM edge"
        );
        let drawn = draw([("t", "noscip", &d)]);
        assert!(!drawn.is_empty());
        assert_eq!(drawn, draw([("t", "noscip", &d)]), "deterministic");
    }
}
