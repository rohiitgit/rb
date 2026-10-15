//! Scoring (contract M8a, Core algorithm; S2, S3). Every gold item and every found edge lands in
//! exactly one bucket, so nothing is excluded silently (NFR 3). A cell stores counts only;
//! precision and recall are derived.

use std::collections::{BTreeMap, BTreeSet};

use rb_model::Certainty;
use serde::{Deserialize, Serialize};

use crate::identity::{EdgeId, Found, Loc, Side, Site};

/// Whether an item belongs to what a key can judge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    In,
    Out(&'static str),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    /// Every gold item, located or not.
    pub gold: u64,
    /// In-scope gold found, by the certainty rb gave it.
    #[serde(with = "certainty_counts")]
    pub tp: BTreeMap<Certainty, u64>,
    /// In-scope found edges that are not gold, by certainty.
    #[serde(with = "certainty_counts")]
    pub fp: BTreeMap<Certainty, u64>,
    /// In-scope gold not found.
    #[serde(rename = "fn")]
    pub fn_: u64,
    pub out_of_scope: BTreeMap<String, u64>,
    pub unmatchable: BTreeMap<String, u64>,
    pub found_out_of_scope: BTreeMap<String, u64>,
    pub found_unmatchable: BTreeMap<String, u64>,
    /// In-scope call sites of `ambiguous` edges, one per source and evidence span.
    pub ambiguous_sites: u64,
    /// Those sites where some candidate is gold.
    pub ambiguous_hits: u64,
}

/// How a cell can be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// No in-scope gold: nothing to measure, never 100 %.
    Vacuous,
    /// In-scope gold, but rb found nothing in scope: precision 0.
    Flagged,
    Scored,
}

impl Cell {
    pub fn tp_total(&self) -> u64 {
        self.tp.values().sum()
    }

    pub fn fp_total(&self) -> u64 {
        self.fp.values().sum()
    }

    pub fn in_scope_gold(&self) -> u64 {
        self.tp_total() + self.fn_
    }

    /// Every found edge, located or not.
    pub fn found(&self) -> u64 {
        self.tp_total()
            + self.fp_total()
            + self.found_out_of_scope.values().sum::<u64>()
            + self.found_unmatchable.values().sum::<u64>()
    }

    pub fn status(&self) -> Status {
        if self.in_scope_gold() == 0 {
            Status::Vacuous
        } else if self.tp_total() + self.fp_total() == 0 {
            Status::Flagged
        } else {
            Status::Scored
        }
    }

    pub fn precision(&self) -> Option<f64> {
        match self.status() {
            Status::Vacuous => None,
            Status::Flagged => Some(0.0),
            Status::Scored => Some(ratio(self.tp_total(), self.tp_total() + self.fp_total())),
        }
    }

    pub fn recall(&self) -> Option<f64> {
        match self.status() {
            Status::Vacuous => None,
            Status::Flagged | Status::Scored => Some(ratio(self.tp_total(), self.in_scope_gold())),
        }
    }
}

fn ratio(a: u64, b: u64) -> f64 {
    a as f64 / b as f64
}

pub fn score<G>(gold: &Side<G>, found: &Side<Found>, scope: impl Fn(&EdgeId) -> Scope) -> Cell {
    let owned = |m: &BTreeMap<&'static str, u64>| -> BTreeMap<String, u64> {
        m.iter().map(|(r, n)| ((*r).to_owned(), *n)).collect()
    };
    let mut cell = Cell {
        gold: gold.len(),
        unmatchable: owned(&gold.unmatchable),
        found_unmatchable: owned(&found.unmatchable),
        ..Cell::default()
    };
    for g in gold.located.keys() {
        match (scope(g), found.located.get(g)) {
            (Scope::Out(r), _) => bump(&mut cell.out_of_scope, r.to_owned()),
            (Scope::In, Some(f)) => bump(&mut cell.tp, f.certainty),
            (Scope::In, None) => cell.fn_ += 1,
        }
    }
    let mut in_scope = Vec::new();
    for (id, f) in &found.located {
        match scope(id) {
            Scope::Out(r) => bump(&mut cell.found_out_of_scope, r.to_owned()),
            Scope::In => {
                if !gold.located.contains_key(id) {
                    bump(&mut cell.fp, f.certainty);
                }
                in_scope.push((id, f));
            }
        }
    }
    // An ambiguous site is a source and span some `ambiguous` edge was found at. Its candidates are
    // every in-scope edge found there, whatever its merged certainty: rb keeps an edge's highest
    // certainty, so a candidate also resolved at another site is no longer `ambiguous` itself.
    let ambiguous: BTreeSet<(&Loc, &Site)> = in_scope
        .iter()
        .filter(|(_, f)| f.certainty == Certainty::Ambiguous)
        .flat_map(|(id, f)| f.sites.iter().map(move |s| (&id.src, s)))
        .collect();
    let hits: BTreeSet<(&Loc, &Site)> = in_scope
        .iter()
        .filter(|(id, _)| gold.located.contains_key(id))
        .flat_map(|(id, f)| f.sites.iter().map(move |s| (&id.src, s)))
        .filter(|site| ambiguous.contains(site))
        .collect();
    cell.ambiguous_sites = ambiguous.len() as u64;
    cell.ambiguous_hits = hits.len() as u64;
    assert_eq!(
        cell.in_scope_gold()
            + cell.out_of_scope.values().sum::<u64>()
            + cell.unmatchable.values().sum::<u64>(),
        cell.gold,
        "every gold item is in exactly one bucket"
    );
    assert_eq!(
        cell.found(),
        found.len(),
        "every found edge is in exactly one bucket"
    );
    cell
}

fn bump<K: Ord>(m: &mut BTreeMap<K, u64>, k: K) {
    *m.entry(k).or_default() += 1;
}

/// Counts by certainty, written as `{"heuristic": n}` (the label's name, not its rank).
mod certainty_counts {
    use std::collections::BTreeMap;

    use rb_model::Certainty;
    use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error};

    const ALL: [Certainty; 5] = [
        Certainty::Ambiguous,
        Certainty::Heuristic,
        Certainty::Synthetic,
        Certainty::Dispatch,
        Certainty::Exact,
    ];

    pub fn serialize<S: Serializer>(m: &BTreeMap<Certainty, u64>, s: S) -> Result<S::Ok, S::Error> {
        let named: BTreeMap<&str, u64> = m.iter().map(|(c, n)| (c.as_str(), *n)).collect();
        named.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<BTreeMap<Certainty, u64>, D::Error> {
        BTreeMap::<String, u64>::deserialize(d)?
            .into_iter()
            .map(|(name, n)| {
                ALL.iter()
                    .find(|c| c.as_str() == name)
                    .map(|c| (*c, n))
                    .ok_or_else(|| D::Error::custom(format!("unknown certainty {name:?}")))
            })
            .collect()
    }
}

/// A gold side whose every item is located (adapters add `unmatchable` counts themselves).
pub fn gold_of(ids: impl IntoIterator<Item = EdgeId>) -> Side<()> {
    Side {
        located: ids.into_iter().map(|id| (id, ())).collect(),
        unmatchable: BTreeMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Class;
    use rb_model::Provenance;
    use std::collections::BTreeSet;

    fn id(src: &str, dst: &str) -> EdgeId {
        let loc = |s: &str| Loc {
            path: format!("{s}.py"),
            line: 1,
            name: s.to_owned(),
        };
        EdgeId {
            src: loc(src),
            class: Class::Calls,
            dst: loc(dst),
        }
    }

    fn site(line: i32) -> Site {
        Site {
            path: "s.py".to_owned(),
            span: [line, 0, line, 1],
        }
    }

    fn f(certainty: Certainty, sites: &[i32]) -> Found {
        Found {
            certainty,
            provenances: BTreeSet::from([Provenance::Heuristic]),
            sites: sites.iter().map(|&l| site(l)).collect(),
        }
    }

    fn found(edges: Vec<(EdgeId, Found)>) -> Side<Found> {
        Side {
            located: edges.into_iter().collect(),
            unmatchable: BTreeMap::new(),
        }
    }

    fn ids<T>(side: &Side<T>) -> BTreeSet<&EdgeId> {
        side.located.keys().collect()
    }

    fn counts(m: &[(&str, u64)]) -> BTreeMap<String, u64> {
        m.iter().map(|(r, n)| ((*r).to_owned(), *n)).collect()
    }

    fn all_in(_: &EdgeId) -> Scope {
        Scope::In
    }

    #[test]
    fn each_bucket_arm() {
        let mut gold = gold_of([id("a", "b"), id("a", "c"), id("x", "b"), id("a", "z")]);
        gold.unmatchable.insert("location_collision", 2);
        let mut fnd = found(vec![
            (id("a", "b"), f(Certainty::Heuristic, &[1])),
            (id("a", "d"), f(Certainty::Exact, &[2])),
            (id("x", "q"), f(Certainty::Heuristic, &[3])),
        ]);
        fnd.unmatchable.insert("no_defined_in", 3);
        let scope = |e: &EdgeId| {
            if e.src.name == "x" {
                Scope::Out("not_covered")
            } else {
                Scope::In
            }
        };
        let c = score(&gold, &fnd, scope);
        assert_eq!(c.gold, 6);
        assert_eq!(c.tp, BTreeMap::from([(Certainty::Heuristic, 1)]));
        assert_eq!(c.fn_, 2);
        assert_eq!(c.fp, BTreeMap::from([(Certainty::Exact, 1)]));
        assert_eq!(c.out_of_scope, counts(&[("not_covered", 1)]));
        assert_eq!(c.unmatchable, counts(&[("location_collision", 2)]));
        assert_eq!(c.found_out_of_scope, counts(&[("not_covered", 1)]));
        assert_eq!(c.found_unmatchable, counts(&[("no_defined_in", 3)]));
        assert_eq!(c.found(), 6);
        assert_eq!(c.status(), Status::Scored);
        assert_eq!(c.precision(), Some(0.5));
        assert_eq!(c.recall(), Some(1.0 / 3.0));
    }

    #[test]
    fn vacuous_and_flagged() {
        let none = gold_of([]);
        let some = found(vec![(id("a", "b"), f(Certainty::Heuristic, &[1]))]);
        let c = score(&none, &some, all_in);
        assert_eq!(c.status(), Status::Vacuous);
        assert_eq!((c.precision(), c.recall()), (None, None));
        let out = score(&gold_of([id("a", "b")]), &some, |_| {
            Scope::Out("module_scope")
        });
        assert_eq!(out.status(), Status::Vacuous);
        let c = score(&gold_of([id("a", "b")]), &found(vec![]), all_in);
        assert_eq!(c.status(), Status::Flagged);
        assert_eq!((c.precision(), c.recall()), (Some(0.0), Some(0.0)));
        let c = score(&gold_of([id("a", "b")]), &some, all_in);
        assert_eq!(c.status(), Status::Scored);
        assert_eq!((c.precision(), c.recall()), (Some(1.0), Some(1.0)));
    }

    #[test]
    fn ambiguous_edges_are_scored_per_site_and_per_edge() {
        // Site 1: candidates b (gold) and c; site 2: candidates d and e, neither gold;
        // site 3 is heuristic, so not an ambiguous site.
        let gold = gold_of([id("a", "b"), id("a", "q")]);
        let fnd = found(vec![
            (id("a", "b"), f(Certainty::Ambiguous, &[1])),
            (id("a", "c"), f(Certainty::Ambiguous, &[1])),
            (id("a", "d"), f(Certainty::Ambiguous, &[2])),
            (id("a", "e"), f(Certainty::Ambiguous, &[2])),
            (id("a", "q"), f(Certainty::Heuristic, &[3])),
        ]);
        let c = score(&gold, &fnd, all_in);
        assert_eq!((c.ambiguous_sites, c.ambiguous_hits), (2, 1));
        assert_eq!(c.tp[&Certainty::Ambiguous], 1);
        // Out of scope, an ambiguous edge is no site at all.
        let c = score(&gold, &fnd, |e| {
            if e.dst.name == "d" || e.dst.name == "e" {
                Scope::Out("not_covered")
            } else {
                Scope::In
            }
        });
        assert_eq!((c.ambiguous_sites, c.ambiguous_hits), (1, 1));
        let c = score(&gold, &fnd, all_in);
        assert_eq!(c.fp[&Certainty::Ambiguous], 3);
        assert_eq!(c.tp[&Certainty::Heuristic], 1);
    }

    #[test]
    fn a_candidate_resolved_elsewhere_still_hits_its_ambiguous_site() {
        // f→g is ambiguous (with f→h) at line 1 and unique at line 2, so its merged certainty is
        // heuristic; line 1 is still an ambiguous site, and its gold candidate f→g is a hit.
        let fnd = found(vec![
            (id("f", "g"), f(Certainty::Heuristic, &[1, 2])),
            (id("f", "h"), f(Certainty::Ambiguous, &[1])),
        ]);
        let c = score(&gold_of([id("f", "g")]), &fnd, all_in);
        assert_eq!((c.ambiguous_sites, c.ambiguous_hits), (1, 1));
    }

    #[test]
    fn one_line_in_two_sources_is_two_sites() {
        let fnd = found(vec![
            (id("a", "b"), f(Certainty::Ambiguous, &[1])),
            (id("x", "b"), f(Certainty::Ambiguous, &[1])),
        ]);
        let c = score(&gold_of([id("a", "b")]), &fnd, all_in);
        assert_eq!((c.ambiguous_sites, c.ambiguous_hits), (2, 1));
    }

    /// A deterministic generator (splitmix64), as in rb-model's diff tests.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut z = self.0;
            z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            z ^ (z >> 31)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    #[test]
    fn buckets_partition_random_sides() {
        const CERTAINTIES: [Certainty; 4] = [
            Certainty::Ambiguous,
            Certainty::Heuristic,
            Certainty::Synthetic,
            Certainty::Exact,
        ];
        for seed in 0..300u64 {
            let mut r = Rng(seed);
            let names = ["a", "b", "c", "d", "e", "f"];
            let universe: Vec<EdgeId> = names
                .iter()
                .flat_map(|s| names.iter().map(move |d| id(s, d)))
                .collect();
            let mut gold = gold_of(universe.iter().filter(|_| r.below(3) == 0).cloned());
            gold.unmatchable.insert("location_collision", r.below(3));
            let mut edges = Vec::new();
            for e in &universe {
                if r.below(3) == 0 {
                    let c = CERTAINTIES[r.below(4) as usize];
                    edges.push((e.clone(), f(c, &[r.below(3) as i32])));
                }
            }
            let mut fnd = found(edges);
            fnd.unmatchable.insert("no_defined_in", r.below(3));
            let out = names[r.below(6) as usize];
            let scope = |e: &EdgeId| {
                if e.src.name == out {
                    Scope::Out("not_covered")
                } else {
                    Scope::In
                }
            };
            let c = score(&gold, &fnd, scope);
            let in_scope = |e: &&&EdgeId| e.src.name != out;
            let both = ids(&gold).intersection(&ids(&fnd)).filter(in_scope).count() as u64;
            let only_found = ids(&fnd).difference(&ids(&gold)).filter(in_scope).count() as u64;
            assert_eq!(c.tp_total(), both, "seed {seed}");
            assert_eq!(c.fp_total(), only_found, "seed {seed}");
            assert_eq!(c.gold, gold.len(), "seed {seed}");
            assert_eq!(c.found(), fnd.len(), "seed {seed}");
            assert!(c.ambiguous_hits <= c.ambiguous_sites, "seed {seed}");
        }
    }
}
