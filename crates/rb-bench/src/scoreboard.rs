//! The scoreboard (contract M8a, S8): counts per cell, never ratios, so micro-averages and the
//! per-cell gate can be derived. Written deterministically: fields in a fixed order, maps and
//! cells sorted, no timestamps. Timing lives in a separate [`Perf`] file because it is noisy.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::sample::Tally;
use crate::score::{Cell, Status};

pub const SCHEMA: u64 = 1;

/// A cell's identity. Its text form, `bench:target:mode:class`, is what accept files name.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CellKey {
    pub bench: String,
    pub target: String,
    pub mode: String,
    pub class: String,
}

impl CellKey {
    pub fn id(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.bench, self.target, self.mode, self.class
        )
    }
}

/// What a cell's key can claim: `accuracy` from an independent key, `agreement` from SCIP.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Label {
    Accuracy,
    Agreement,
}

impl Label {
    pub fn as_str(self) -> &'static str {
        match self {
            Label::Accuracy => "accuracy",
            Label::Agreement => "agreement",
        }
    }
}

/// How many of a target's source files the reference covers (SCIP agreement only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Covered {
    pub files: u64,
    pub of: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub label: Label,
    pub cell: Cell,
    pub covered: Option<Covered>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rb {
    pub commit: String,
    pub binary_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scoreboard {
    pub milestone: String,
    pub rb: Rb,
    /// Every pinned input by name: a dataset (`pycg`) or a target, to its commit.
    pub pins: BTreeMap<String, String>,
    /// Every answer key by name (a key file's path, or `scip:<target>`), to its sha256.
    pub keys: BTreeMap<String, String>,
    pub cells: BTreeMap<CellKey, Record>,
    /// The `exact` sample per family (contract M8b, T5); empty before M8b.
    pub samples: BTreeMap<String, Tally>,
}

/// The file form: the cells as a sorted list of flat rows.
#[derive(Serialize, Deserialize)]
struct Wire {
    schema: u64,
    milestone: String,
    rb: Rb,
    pins: BTreeMap<String, String>,
    keys: BTreeMap<String, String>,
    cells: Vec<Row>,
    #[serde(default)]
    samples: BTreeMap<String, Tally>,
}

#[derive(Serialize, Deserialize)]
struct Row {
    #[serde(flatten)]
    key: CellKey,
    label: Label,
    #[serde(flatten)]
    cell: Cell,
    covered: Option<Covered>,
}

impl Scoreboard {
    pub fn to_json(&self) -> String {
        let wire = Wire {
            schema: SCHEMA,
            milestone: self.milestone.clone(),
            rb: self.rb.clone(),
            pins: self.pins.clone(),
            keys: self.keys.clone(),
            samples: self.samples.clone(),
            cells: self
                .cells
                .iter()
                .map(|(key, r)| Row {
                    key: key.clone(),
                    label: r.label,
                    cell: r.cell.clone(),
                    covered: r.covered,
                })
                .collect(),
        };
        // Serializing owned maps and strings cannot fail.
        serde_json::to_string_pretty(&wire).expect("scoreboard serializes") + "\n"
    }

    pub fn from_json(text: &str) -> Result<Scoreboard, String> {
        let schema = serde_json::from_str::<serde_json::Value>(text)
            .map_err(|e| format!("not a scoreboard: {e}"))?
            .get("schema")
            .and_then(serde_json::Value::as_u64);
        if schema != Some(SCHEMA) {
            return Err(format!(
                "scoreboard schema {schema:?} is not supported (expected {SCHEMA})"
            ));
        }
        let wire: Wire =
            serde_json::from_str(text).map_err(|e| format!("not a scoreboard: {e}"))?;
        let mut cells = BTreeMap::new();
        for row in wire.cells {
            let id = row.key.id();
            let record = Record {
                label: row.label,
                cell: row.cell,
                covered: row.covered,
            };
            if cells.insert(row.key, record).is_some() {
                return Err(format!("cell {id} appears twice"));
            }
        }
        Ok(Scoreboard {
            milestone: wire.milestone,
            rb: wire.rb,
            pins: wire.pins,
            keys: wire.keys,
            cells,
            samples: wire.samples,
        })
    }

    /// Every `(bench, class)` a run claims to measure has at least one non-vacuous cell (L429:
    /// a vacuous comparison proves nothing).
    pub fn check_non_vacuous(&self, claims: &[(&str, &str)]) -> Result<(), String> {
        let empty: Vec<String> = claims
            .iter()
            .filter(|(bench, class)| {
                !self.cells.iter().any(|(k, r)| {
                    k.bench == *bench && k.class == *class && r.cell.status() != Status::Vacuous
                })
            })
            .map(|(bench, class)| format!("{bench}/{class}"))
            .collect();
        if empty.is_empty() {
            Ok(())
        } else {
            Err(format!("no non-vacuous cell for {}", empty.join(", ")))
        }
    }
}

/// Timing of one milestone's run (contract M8a, S7, S8), compared only on the same hardware.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Perf {
    pub schema: u64,
    pub milestone: String,
    pub hardware: String,
    pub binary_sha256: String,
    /// By `target:run`, e.g. `re_mattermost:noscip-fresh`.
    pub runs: BTreeMap<String, Run>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Run {
    pub wall_s: f64,
    pub peak_rss_bytes: u64,
    pub load_start: f64,
    pub load_end: f64,
}

impl Perf {
    pub fn from_json(text: &str) -> Result<Perf, String> {
        let perf: Perf = serde_json::from_str(text).map_err(|e| format!("not a perf file: {e}"))?;
        if perf.schema != SCHEMA {
            return Err(format!(
                "perf schema {} is not supported (expected {SCHEMA})",
                perf.schema
            ));
        }
        Ok(perf)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("perf serializes") + "\n"
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use rb_model::Certainty;

    pub(crate) fn key(bench: &str, target: &str) -> CellKey {
        CellKey {
            bench: bench.to_owned(),
            target: target.to_owned(),
            mode: "noscip".to_owned(),
            class: "calls".to_owned(),
        }
    }

    pub(crate) fn board(cells: Vec<(CellKey, Record)>) -> Scoreboard {
        Scoreboard {
            milestone: "m8a".to_owned(),
            rb: Rb {
                commit: "c".to_owned(),
                binary_sha256: "b".to_owned(),
            },
            pins: BTreeMap::from([("pycg".to_owned(), "p".to_owned())]),
            keys: BTreeMap::from([("scip:x".to_owned(), "k".to_owned())]),
            cells: cells.into_iter().collect(),
            samples: BTreeMap::new(),
        }
    }

    pub(crate) fn record(tp: u64, fp: u64, fn_: u64) -> Record {
        let mut cell = Cell {
            gold: tp + fn_,
            fn_,
            ..Cell::default()
        };
        if tp > 0 {
            cell.tp.insert(Certainty::Heuristic, tp);
        }
        if fp > 0 {
            cell.fp.insert(Certainty::Ambiguous, fp);
        }
        Record {
            label: Label::Accuracy,
            cell,
            covered: None,
        }
    }

    #[test]
    fn json_round_trips_and_is_deterministic() {
        let mut r = record(3, 1, 2);
        r.cell.out_of_scope.insert("module_scope".to_owned(), 4);
        r.cell.gold += 4;
        r.covered = Some(Covered { files: 3, of: 5 });
        r.label = Label::Agreement;
        let b = board(vec![
            (key("pycg", "b/one"), record(1, 0, 0)),
            (key("scip", "a"), r),
        ]);
        let text = b.to_json();
        assert_eq!(Scoreboard::from_json(&text), Ok(b.clone()));
        assert_eq!(Scoreboard::from_json(&text).unwrap().to_json(), text);
        assert!(text.contains("\"fn\": 2"), "{text}");
        assert!(text.contains("\"heuristic\": 3"), "{text}");
        assert!(text.find("b/one").unwrap() < text.find("\"target\": \"a\"").unwrap());
    }

    #[test]
    fn foreign_schemas_and_duplicates_are_rejected() {
        let text = board(vec![(key("pycg", "a"), record(1, 0, 0))]).to_json();
        let v2 = text.replacen("\"schema\": 1", "\"schema\": 2", 1);
        assert!(Scoreboard::from_json(&v2).unwrap_err().contains("schema"));
        assert!(Scoreboard::from_json("[]").unwrap_err().contains("schema"));
        let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
        let row = v["cells"][0].clone();
        v["cells"].as_array_mut().unwrap().push(row);
        let twice = Scoreboard::from_json(&v.to_string()).unwrap_err();
        assert!(twice.contains("twice"), "{twice}");
    }

    #[test]
    fn every_claimed_class_needs_a_non_vacuous_cell() {
        let b = board(vec![
            (key("pycg", "a"), record(0, 2, 0)),
            (key("scip", "a"), record(1, 0, 0)),
        ]);
        assert_eq!(b.check_non_vacuous(&[("scip", "calls")]), Ok(()));
        let err = b
            .check_non_vacuous(&[("pycg", "calls"), ("scip", "calls"), ("scip", "x")])
            .unwrap_err();
        assert_eq!(err, "no non-vacuous cell for pycg/calls, scip/x");
    }
}
