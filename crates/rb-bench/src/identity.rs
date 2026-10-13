//! Location identity (contract M8a, Identity). Keys can't be compared across indexing modes (a
//! SCIP key replaces the heuristic key of a covered file) or with external answer keys, so every
//! edge endpoint is reduced to where it is defined: a symbol to the lowest span of its
//! `DEFINED_IN` evidence (the name token; a merged declaration's other sites are its aliases), a
//! file to the file itself. Anything that can't be reduced is counted under a reason, never
//! dropped.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rb_model::diff::EdgeRow;
use rb_model::{Certainty, EdgeKind, NodeKind, Provenance, Span};

use crate::dump::Dump;

/// Where something is defined. `line` is 1-based; 0 means the file itself.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Loc {
    pub path: String,
    pub line: u32,
    pub name: String,
}

/// The `name` of a file's location.
pub const FILE_NAME: &str = "<file>";

impl Loc {
    pub fn file(path: &str) -> Loc {
        Loc {
            path: path.to_owned(),
            line: 0,
            name: FILE_NAME.to_owned(),
        }
    }
}

/// The edge classes scored (contract M8a, Edge classes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// `CALLS` from a symbol (or from any other non-file node, which is then `not_locatable`).
    Calls,
    /// `CALLS` from a file: top-level code, in both modes.
    CallsFileScope,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Calls => "calls",
            Class::CallsFileScope => "calls_file_scope",
        }
    }

    fn selects(self, e: &EdgeRow) -> bool {
        match self {
            Class::Calls => e.kind == EdgeKind::Calls && e.src_kind != NodeKind::File,
            Class::CallsFileScope => e.kind == EdgeKind::Calls && e.src_kind == NodeKind::File,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EdgeId {
    pub src: Loc,
    pub class: Class,
    pub dst: Loc,
}

/// Why something has no location; the bucket names of the scoreboard.
pub mod reason {
    pub const NO_DEFINED_IN: &str = "no_defined_in";
    pub const LOCATION_COLLISION: &str = "location_collision";
    pub const UNKNOWN_NODE: &str = "unknown_node";
    pub const NOT_LOCATABLE: &str = "not_locatable";
    pub const NEGATIVE_SPAN: &str = "negative_span";
}

/// The location of every symbol of one dump.
pub struct Locator {
    symbols: HashMap<String, Result<Loc, &'static str>>,
    /// A merged declaration's other definition sites, to its location.
    aliases: HashMap<Loc, Loc>,
}

impl Locator {
    pub fn new(dump: &Dump) -> Locator {
        let mut sites: HashMap<&str, Vec<(&str, Span)>> = HashMap::new();
        for e in &dump.edges {
            if e.kind == EdgeKind::DefinedIn && e.src_kind == NodeKind::Symbol {
                let all = sites.entry(e.src_key.as_str()).or_default();
                all.extend(e.evidence.iter().map(|ev| (ev.path.as_str(), ev.span)));
            }
        }
        // Every site of each symbol, lowest first (one symbol's sites share its name, so `Loc`
        // orders them by path and line); the first is its location.
        let mut all: HashMap<&str, Result<Vec<Loc>, &'static str>> = dump
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Symbol)
            .map(|n| {
                let here = sites.remove(n.key.as_str()).unwrap_or_default();
                let locs: Result<BTreeSet<Loc>, &'static str> = here
                    .iter()
                    .map(|(path, span)| {
                        let line = u32::try_from(span[0]).map_err(|_| reason::NEGATIVE_SPAN)?;
                        Ok(Loc {
                            path: (*path).to_owned(),
                            line: line + 1,
                            name: n.name.clone(),
                        })
                    })
                    .collect();
                let locs = match locs {
                    Ok(locs) if locs.is_empty() => Err(reason::NO_DEFINED_IN),
                    other => other.map(|ls| ls.into_iter().collect()),
                };
                (n.key.as_str(), locs)
            })
            .collect();
        // Two symbols sharing any site can't be told apart by any key: neither is matched.
        let mut holders: HashMap<Loc, usize> = HashMap::new();
        for loc in all.values().flatten().flatten() {
            *holders.entry(loc.clone()).or_default() += 1;
        }
        for locs in all.values_mut() {
            if locs
                .as_ref()
                .is_ok_and(|ls| ls.iter().any(|l| holders[l] > 1))
            {
                *locs = Err(reason::LOCATION_COLLISION);
            }
        }
        let mut aliases = HashMap::new();
        let mut symbols = HashMap::new();
        for (key, locs) in all {
            let loc = locs.map(|ls| {
                for alias in &ls[1..] {
                    aliases.insert(alias.clone(), ls[0].clone());
                }
                ls[0].clone()
            });
            symbols.insert(key.to_owned(), loc);
        }
        Locator { symbols, aliases }
    }

    pub fn loc(&self, kind: NodeKind, key: &str) -> Result<Loc, &'static str> {
        match kind {
            NodeKind::Symbol => self
                .symbols
                .get(key)
                .cloned()
                .unwrap_or(Err(reason::UNKNOWN_NODE)),
            NodeKind::File => Ok(Loc::file(key)),
            _ => Err(reason::NOT_LOCATABLE),
        }
    }

    /// The location an answer key's location stands for: an alias's symbol location, or itself.
    pub fn canonical(&self, loc: &Loc) -> Loc {
        self.aliases.get(loc).unwrap_or(loc).clone()
    }
}

/// One evidence site of a found edge.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Site {
    pub path: String,
    pub span: Span,
}

/// A found edge: every dump row with its `EdgeId` merged (one row per provenance).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub certainty: Certainty,
    pub provenances: BTreeSet<Provenance>,
    pub sites: BTreeSet<Site>,
}

/// One side of a comparison: the located items and, per reason, how many could not be located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side<T> {
    pub located: BTreeMap<EdgeId, T>,
    pub unmatchable: BTreeMap<&'static str, u64>,
}

impl<T> Default for Side<T> {
    fn default() -> Self {
        Side {
            located: BTreeMap::new(),
            unmatchable: BTreeMap::new(),
        }
    }
}

impl<T> Side<T> {
    /// Every item, located or not.
    pub fn len(&self) -> u64 {
        self.located.len() as u64 + self.unmatchable.values().sum::<u64>()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Side<Found> {
    /// Adds a found edge; one already at `id` keeps the higher certainty and gains the other's
    /// provenances and sites (the one merge rule of every found side).
    pub fn insert(&mut self, id: EdgeId, found: Found) {
        match self.located.entry(id) {
            std::collections::btree_map::Entry::Vacant(v) => {
                v.insert(found);
            }
            std::collections::btree_map::Entry::Occupied(mut o) => {
                let x = o.get_mut();
                x.certainty = x.certainty.max(found.certainty);
                x.provenances.extend(found.provenances);
                x.sites.extend(found.sites);
            }
        }
    }
}

/// The edges of `class` in `dump`, by location identity. An edge with an endpoint that has no
/// location is counted once per (source, target) pair under the endpoint's reason, the source's
/// first.
pub fn found(dump: &Dump, locator: &Locator, class: Class) -> Side<Found> {
    let mut side = Side::default();
    let mut unlocated: BTreeMap<(&str, &str), &'static str> = BTreeMap::new();
    for e in dump.edges.iter().filter(|e| class.selects(e)) {
        let src = locator.loc(e.src_kind, &e.src_key);
        let dst = locator.loc(e.dst_kind, &e.dst_key);
        let (src, dst) = match (src, dst) {
            (Ok(src), Ok(dst)) => (src, dst),
            (Err(r), _) | (_, Err(r)) => {
                unlocated.insert((&e.src_key, &e.dst_key), r);
                continue;
            }
        };
        let sites = e.evidence.iter().map(|ev| Site {
            path: ev.path.clone(),
            span: ev.span,
        });
        let found = Found {
            certainty: e.certainty,
            provenances: BTreeSet::from([e.provenance]),
            sites: sites.collect(),
        };
        side.insert(EdgeId { src, class, dst }, found);
    }
    for r in unlocated.into_values() {
        *side.unmatchable.entry(r).or_default() += 1;
    }
    side
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dump::parse;

    fn sym(path: &str, name: &str) -> String {
        format!("rb-heuristic . . . `{path}`/{name}().")
    }

    fn n(key: &str, name: &str) -> String {
        format!(r#"["N","symbol",{},"{name}",{{}}]"#, serde_json::json!(key))
    }

    fn defined(key: &str, path: &str, spans: &[[i32; 4]]) -> String {
        let ev: Vec<_> = spans.iter().map(|s| serde_json::json!([path, s])).collect();
        format!(
            r#"["E","symbol",{},"DEFINED_IN","file","{path}","treesitter",5,{{}},{}]"#,
            serde_json::json!(key),
            serde_json::json!(ev)
        )
    }

    fn calls(src: (&str, &str), dst: &str, prov: &str, cert: i32, at: (&str, [i32; 4])) -> String {
        format!(
            r#"["E","{}",{},"CALLS","symbol",{},"{prov}",{cert},{{}},[["{}",{}]]]"#,
            src.0,
            serde_json::json!(src.1),
            serde_json::json!(dst),
            at.0,
            serde_json::json!(at.1)
        )
    }

    fn dump(rows: &[String]) -> Dump {
        parse(&format!("[\"rb-dump\",2,\"x\"]\n{}\n", rows.join("\n"))).unwrap()
    }

    fn loc(path: &str, line: u32, name: &str) -> Loc {
        Loc {
            path: path.to_owned(),
            line,
            name: name.to_owned(),
        }
    }

    #[test]
    fn a_symbol_is_located_at_its_lowest_definition_site() {
        let f = sym("a.ts", "f");
        // A merged declaration: two sites in one row, plus a second row in another file.
        let d = dump(&[
            n(&f, "f"),
            defined(&f, "b.ts", &[[1, 0, 1, 1]]),
            defined(&f, "a.ts", &[[9, 2, 9, 3], [4, 6, 4, 7]]),
        ]);
        let l = Locator::new(&d);
        assert_eq!(l.loc(NodeKind::Symbol, &f), Ok(loc("a.ts", 5, "f")));
        // Its other sites are aliases of that location; any other location stands for itself.
        assert_eq!(l.canonical(&loc("b.ts", 2, "f")), loc("a.ts", 5, "f"));
        assert_eq!(l.canonical(&loc("a.ts", 10, "f")), loc("a.ts", 5, "f"));
        assert_eq!(l.canonical(&loc("a.ts", 10, "g")), loc("a.ts", 10, "g"));
    }

    #[test]
    fn an_alias_site_shared_with_another_symbol_collides() {
        let (f, g) = (sym("a.ts", "f"), sym("b.ts", "f"));
        let d = dump(&[
            n(&f, "f"),
            n(&g, "f"),
            defined(&f, "a.ts", &[[1, 0, 1, 1]]),
            defined(&f, "b.ts", &[[3, 0, 3, 1]]),
            defined(&g, "b.ts", &[[3, 4, 3, 5]]),
        ]);
        let l = Locator::new(&d);
        assert_eq!(l.loc(NodeKind::Symbol, &f), Err(reason::LOCATION_COLLISION));
        assert_eq!(l.loc(NodeKind::Symbol, &g), Err(reason::LOCATION_COLLISION));
    }

    #[test]
    fn a_negative_span_is_not_a_file_location() {
        let f = sym("a.py", "f");
        let d = dump(&[n(&f, "f"), defined(&f, "a.py", &[[-1, 0, 0, 1]])]);
        assert_eq!(
            Locator::new(&d).loc(NodeKind::Symbol, &f),
            Err(reason::NEGATIVE_SPAN)
        );
    }

    #[test]
    fn files_and_unlocatable_nodes() {
        let g = sym("a.py", "g");
        let d = dump(&[n(&g, "g")]);
        let l = Locator::new(&d);
        assert_eq!(l.loc(NodeKind::File, "a.py"), Ok(Loc::file("a.py")));
        assert_eq!(l.loc(NodeKind::Symbol, &g), Err(reason::NO_DEFINED_IN));
        assert_eq!(l.loc(NodeKind::Symbol, "nope"), Err(reason::UNKNOWN_NODE));
        assert_eq!(
            l.loc(NodeKind::Endpoint, "HTTP GET /x @."),
            Err(reason::NOT_LOCATABLE)
        );
    }

    #[test]
    fn two_symbols_at_one_location_both_collide() {
        let (a, b, c) = (sym("a.js", "x"), sym("a.js", "x(+1)"), sym("a.js", "y"));
        let d = dump(&[
            n(&a, "x"),
            n(&b, "x"),
            n(&c, "y"),
            defined(&a, "a.js", &[[2, 0, 2, 1]]),
            defined(&b, "a.js", &[[2, 9, 2, 10]]),
            defined(&c, "a.js", &[[2, 20, 2, 21]]),
        ]);
        let l = Locator::new(&d);
        assert_eq!(l.loc(NodeKind::Symbol, &a), Err(reason::LOCATION_COLLISION));
        assert_eq!(l.loc(NodeKind::Symbol, &b), Err(reason::LOCATION_COLLISION));
        assert_eq!(l.loc(NodeKind::Symbol, &c), Ok(loc("a.js", 3, "y")));
    }

    #[test]
    fn rows_of_one_edge_merge_and_classes_split_by_source_kind() {
        let (f, g) = (sym("a.py", "f"), sym("a.py", "g"));
        let d = dump(&[
            n(&f, "f"),
            n(&g, "g"),
            defined(&f, "a.py", &[[0, 4, 0, 5]]),
            defined(&g, "a.py", &[[5, 4, 5, 5]]),
            calls(("symbol", &f), &g, "heuristic", 2, ("a.py", [1, 4, 1, 5])),
            calls(("symbol", &f), &g, "scip", 5, ("a.py", [2, 4, 2, 5])),
            calls(("file", "a.py"), &g, "scip", 5, ("a.py", [9, 0, 9, 1])),
        ]);
        let l = Locator::new(&d);
        let calls = found(&d, &l, Class::Calls);
        let id = EdgeId {
            src: loc("a.py", 1, "f"),
            class: Class::Calls,
            dst: loc("a.py", 6, "g"),
        };
        assert_eq!(calls.located.len(), 1);
        let x = &calls.located[&id];
        assert_eq!(x.certainty, Certainty::Exact);
        assert_eq!(
            x.provenances,
            BTreeSet::from([Provenance::Heuristic, Provenance::Scip])
        );
        assert_eq!(x.sites.len(), 2);
        let top = found(&d, &l, Class::CallsFileScope);
        assert_eq!(top.located.len(), 1);
        assert!(top.located.keys().all(|k| k.src == Loc::file("a.py")));
    }

    #[test]
    fn calls_from_any_other_kind_are_counted_not_dropped() {
        let g = sym("a.py", "g");
        let d = dump(&[
            n(&g, "g"),
            r#"["N","endpoint","HTTP GET /x @.","GET /x",{}]"#.to_owned(),
            defined(&g, "a.py", &[[5, 4, 5, 5]]),
            calls(
                ("endpoint", "HTTP GET /x @."),
                &g,
                "heuristic",
                2,
                ("a.py", [1, 4, 1, 5]),
            ),
        ]);
        let side = found(&d, &Locator::new(&d), Class::Calls);
        assert_eq!(
            side.unmatchable,
            BTreeMap::from([(reason::NOT_LOCATABLE, 1)])
        );
    }

    #[test]
    fn unlocated_edges_are_counted_once_per_pair() {
        let (f, g) = (sym("a.py", "f"), sym("a.py", "g"));
        let d = dump(&[
            n(&f, "f"),
            n(&g, "g"),
            defined(&g, "a.py", &[[5, 4, 5, 5]]),
            calls(("symbol", &f), &g, "heuristic", 2, ("a.py", [1, 4, 1, 5])),
            calls(("symbol", &f), &g, "di", 2, ("a.py", [1, 4, 1, 5])),
        ]);
        let side = found(&d, &Locator::new(&d), Class::Calls);
        assert!(side.located.is_empty());
        assert_eq!(
            side.unmatchable,
            BTreeMap::from([(reason::NO_DEFINED_IN, 1)])
        );
        assert_eq!(side.len(), 1);
    }
}
