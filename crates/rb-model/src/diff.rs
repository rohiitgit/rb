//! `rb diff`'s exact sections (contract M5 S16, D11): the difference between two snapshot graphs
//! by natural identity, as the canonical dump writes them. Nodes are identified by `(kind, key)`
//! and edges by `(src kind, src key, kind, dst kind, dst key, provenance)`, the dump's own sort
//! keys; surrogate ids never appear. Moves are not inferred: a key that changes is a removal and
//! an addition (D11, fork F4). Pure functions; every section is in the dump's order.
//!
//! The properties (tested on random graphs): `apply(dump lines of A, diff(A, B))` is B's dump
//! lines, byte for byte; `diff(A, A)` is empty; `diff(B, A)` is `diff(A, B)` inverted.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::str::FromStr;

use serde_json::{Value, json};

use crate::graph::sort_diagnostics;
use crate::{Attrs, Certainty, Diagnostic, EdgeKind, Evidence, Graph, NodeKind, Provenance};

/// A node as the dump writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct NodeRow {
    pub kind: NodeKind,
    pub key: String,
    pub name: String,
    pub attrs: Attrs,
}

/// An edge as the dump writes it; `evidence` sorted.
#[derive(Debug, Clone, PartialEq)]
pub struct EdgeRow {
    pub src_kind: NodeKind,
    pub src_key: String,
    pub kind: EdgeKind,
    pub dst_kind: NodeKind,
    pub dst_key: String,
    pub provenance: Provenance,
    pub certainty: Certainty,
    pub attrs: Attrs,
    pub evidence: Vec<Evidence>,
}

/// A node's identity, in the dump's order.
pub type NodeId<'a> = (NodeKind, &'a str);
/// An edge's identity, in the dump's order.
pub type EdgeId<'a> = (NodeKind, &'a str, EdgeKind, NodeKind, &'a str, Provenance);

impl NodeRow {
    pub fn id(&self) -> NodeId<'_> {
        (self.kind, &self.key)
    }

    /// Its `N` line of the canonical dump (without the newline).
    pub fn line(&self) -> String {
        json!(["N", self.kind.as_str(), self.key, self.name, self.attrs]).to_string()
    }
}

impl EdgeRow {
    pub fn id(&self) -> EdgeId<'_> {
        (
            self.src_kind,
            &self.src_key,
            self.kind,
            self.dst_kind,
            &self.dst_key,
            self.provenance,
        )
    }

    /// Its `E` line of the canonical dump (without the newline).
    pub fn line(&self) -> String {
        let evidence: Vec<Value> = self
            .evidence
            .iter()
            .map(|ev| json!([ev.path, ev.span]))
            .collect();
        json!([
            "E",
            self.src_kind.as_str(),
            self.src_key,
            self.kind.as_str(),
            self.dst_kind.as_str(),
            self.dst_key,
            self.provenance.as_str(),
            self.certainty.rank(),
            self.attrs,
            evidence
        ])
        .to_string()
    }
}

/// A diagnostic's `D` line of the canonical dump (without the newline).
pub fn diagnostic_line(d: &Diagnostic) -> String {
    json!(["D", d.kind, d.path, d.span, d.detail]).to_string()
}

/// Counts the diff's elementary steps (sort comparisons, merge steps) in test builds, for the
/// growth test (lesson 2026-10-06: growth is tested by counted operations).
mod count {
    #[cfg(test)]
    thread_local! {
        static STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    #[inline]
    pub(crate) fn tick() {
        #[cfg(test)]
        STEPS.with(|c| c.set(c.get() + 1));
    }

    #[cfg(test)]
    pub(crate) fn take() -> u64 {
        STEPS.with(|c| c.replace(0))
    }
}

/// The graph's nodes in the dump's order.
pub fn node_rows(g: &Graph) -> Vec<NodeRow> {
    let mut rows: Vec<NodeRow> = g
        .nodes()
        .iter()
        .map(|n| NodeRow {
            kind: n.kind,
            key: n.key.clone(),
            name: n.name.clone(),
            attrs: n.attrs.clone(),
        })
        .collect();
    rows.sort_by(|a, b| {
        count::tick();
        a.id().cmp(&b.id())
    });
    rows
}

/// The graph's edges in the dump's order, each with its evidence sorted.
pub fn edge_rows(g: &Graph) -> Vec<EdgeRow> {
    let by_id: std::collections::HashMap<i64, &crate::Node> =
        g.nodes().iter().map(|n| (n.id, n)).collect();
    let mut rows: Vec<EdgeRow> = g
        .edges()
        .iter()
        .map(|e| {
            let (s, d) = (by_id[&e.src], by_id[&e.dst]);
            let mut evidence = e.evidence.clone();
            evidence.sort();
            EdgeRow {
                src_kind: s.kind,
                src_key: s.key.clone(),
                kind: e.kind,
                dst_kind: d.kind,
                dst_key: d.key.clone(),
                provenance: e.provenance,
                certainty: e.certainty,
                attrs: e.attrs.clone(),
                evidence,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        count::tick();
        a.id().cmp(&b.id())
    });
    rows
}

/// A row present on both sides with a difference: as it was and as it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Change<T> {
    pub before: T,
    pub after: T,
}

/// The exact difference of two graphs (D11), every section in the dump's order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GraphDiff {
    pub nodes_added: Vec<NodeRow>,
    pub nodes_removed: Vec<NodeRow>,
    /// The name or the attributes differ.
    pub nodes_changed: Vec<Change<NodeRow>>,
    pub edges_added: Vec<EdgeRow>,
    pub edges_removed: Vec<EdgeRow>,
    /// The certainty or the attributes differ (the evidence may too).
    pub edges_changed: Vec<Change<EdgeRow>>,
    /// Only the evidence differs.
    pub edges_evidence_only: Vec<Change<EdgeRow>>,
}

impl GraphDiff {
    pub fn is_empty(&self) -> bool {
        self.nodes_added.is_empty()
            && self.nodes_removed.is_empty()
            && self.nodes_changed.is_empty()
            && self.edges_added.is_empty()
            && self.edges_removed.is_empty()
            && self.edges_changed.is_empty()
            && self.edges_evidence_only.is_empty()
    }

    /// The diff from B to A, for this diff from A to B.
    pub fn invert(&self) -> GraphDiff {
        fn swap<T: Clone>(xs: &[Change<T>]) -> Vec<Change<T>> {
            xs.iter()
                .map(|c| Change {
                    before: c.after.clone(),
                    after: c.before.clone(),
                })
                .collect()
        }
        GraphDiff {
            nodes_added: self.nodes_removed.clone(),
            nodes_removed: self.nodes_added.clone(),
            nodes_changed: swap(&self.nodes_changed),
            edges_added: self.edges_removed.clone(),
            edges_removed: self.edges_added.clone(),
            edges_changed: swap(&self.edges_changed),
            edges_evidence_only: swap(&self.edges_evidence_only),
        }
    }
}

/// One side of a merge-join over two lists sorted by identity.
enum Side<T> {
    Left(T),
    Right(T),
    Both(T, T),
}

/// Merges two lists sorted by `cmp` (one step per element).
fn merge<T>(a: Vec<T>, b: Vec<T>, cmp: impl Fn(&T, &T) -> Ordering) -> Vec<Side<T>> {
    let mut out = Vec::with_capacity(a.len().max(b.len()));
    let (mut a, mut b) = (a.into_iter().peekable(), b.into_iter().peekable());
    loop {
        count::tick();
        let order = match (a.peek(), b.peek()) {
            (Some(x), Some(y)) => cmp(x, y),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => break,
        };
        out.push(match order {
            Ordering::Less => Side::Left(a.next().expect("peeked")),
            Ordering::Greater => Side::Right(b.next().expect("peeked")),
            Ordering::Equal => Side::Both(a.next().expect("peeked"), b.next().expect("peeked")),
        });
    }
    out
}

/// The difference from `a` to `b` (D11): linear after sorting each side, and sorting a graph
/// already in key order (every graph a builder or the store makes) is linear too.
pub fn diff(a: &Graph, b: &Graph) -> GraphDiff {
    let mut d = GraphDiff::default();
    for side in merge(node_rows(a), node_rows(b), |x, y| x.id().cmp(&y.id())) {
        match side {
            Side::Left(n) => d.nodes_removed.push(n),
            Side::Right(n) => d.nodes_added.push(n),
            Side::Both(x, y) if x != y => d.nodes_changed.push(Change {
                before: x,
                after: y,
            }),
            Side::Both(..) => {}
        }
    }
    for side in merge(edge_rows(a), edge_rows(b), |x, y| x.id().cmp(&y.id())) {
        match side {
            Side::Left(e) => d.edges_removed.push(e),
            Side::Right(e) => d.edges_added.push(e),
            Side::Both(x, y) if x.certainty != y.certainty || x.attrs != y.attrs => {
                d.edges_changed.push(Change {
                    before: x,
                    after: y,
                })
            }
            Side::Both(x, y) if x.evidence != y.evidence => d.edges_evidence_only.push(Change {
                before: x,
                after: y,
            }),
            Side::Both(..) => {}
        }
    }
    d
}

/// A dump row's identity, typed so that it sorts as the dump does: nodes first, then edges.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum RowId {
    Node(NodeKind, String),
    Edge(NodeKind, String, EdgeKind, NodeKind, String, Provenance),
}

impl RowId {
    fn of_node(n: &NodeRow) -> RowId {
        RowId::Node(n.kind, n.key.clone())
    }

    fn of_edge(e: &EdgeRow) -> RowId {
        RowId::Edge(
            e.src_kind,
            e.src_key.clone(),
            e.kind,
            e.dst_kind,
            e.dst_key.clone(),
            e.provenance,
        )
    }

    /// The identity of a dump line; `None` for a header or `D` line.
    fn of_line(line: &str) -> Result<Option<RowId>, String> {
        let v: Value = serde_json::from_str(line).map_err(|e| format!("{e}: {line}"))?;
        let s = |i: usize| -> Result<String, String> {
            v[i].as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("field {i} is not a string: {line}"))
        };
        Ok(match v[0].as_str() {
            Some("N") => Some(RowId::Node(kind_of(&s(1)?)?, s(2)?)),
            Some("E") => Some(RowId::Edge(
                kind_of(&s(1)?)?,
                s(2)?,
                EdgeKind::from_str(&s(3)?).map_err(|e| e.to_string())?,
                kind_of(&s(4)?)?,
                s(5)?,
                Provenance::from_str(&s(6)?).map_err(|e| e.to_string())?,
            )),
            _ => None,
        })
    }
}

fn kind_of(s: &str) -> Result<NodeKind, String> {
    NodeKind::from_str(s).map_err(|e| e.to_string())
}

/// Applies `d` to the `N` and `E` lines of a dump (other lines are ignored) and returns the
/// patched `N` and `E` lines in the dump's order. It checks what it patches: a removed or changed
/// row must be present exactly as `d`'s before-row renders, and an added one absent, so a diff
/// applies only to the graph it was computed from.
pub fn apply(lines: &[&str], d: &GraphDiff) -> Result<Vec<String>, String> {
    let mut rows: BTreeMap<RowId, String> = BTreeMap::new();
    for line in lines {
        if let Some(id) = RowId::of_line(line)?
            && rows.insert(id, (*line).to_owned()).is_some()
        {
            return Err(format!("duplicate row: {line}"));
        }
    }
    let mut remove = |id: RowId, line: String| -> Result<(), String> {
        match rows.remove(&id) {
            Some(have) if have == line => Ok(()),
            Some(have) => Err(format!("expected {line}, found {have}")),
            None => Err(format!("missing {line}")),
        }
    };
    for n in &d.nodes_removed {
        remove(RowId::of_node(n), n.line())?;
    }
    for e in &d.edges_removed {
        remove(RowId::of_edge(e), e.line())?;
    }
    for c in &d.nodes_changed {
        remove(RowId::of_node(&c.before), c.before.line())?;
    }
    for c in d.edges_changed.iter().chain(&d.edges_evidence_only) {
        remove(RowId::of_edge(&c.before), c.before.line())?;
    }
    let mut add = |id: RowId, line: String| -> Result<(), String> {
        match rows.insert(id, line.clone()) {
            None => Ok(()),
            Some(have) => Err(format!("{line} is already present as {have}")),
        }
    };
    for n in d
        .nodes_added
        .iter()
        .chain(d.nodes_changed.iter().map(|c| &c.after))
    {
        add(RowId::of_node(n), n.line())?;
    }
    for e in d
        .edges_added
        .iter()
        .chain(d.edges_changed.iter().map(|c| &c.after))
        .chain(d.edges_evidence_only.iter().map(|c| &c.after))
    {
        add(RowId::of_edge(e), e.line())?;
    }
    Ok(rows.into_values().collect())
}

/// The diagnostics' difference (D11): per kind, the count before and after; and the multiset
/// difference of the dump's `D` lines, in canonical order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DiagnosticsDiff {
    pub counts: BTreeMap<String, (usize, usize)>,
    pub added: Vec<Diagnostic>,
    pub removed: Vec<Diagnostic>,
}

pub fn diagnostics_diff(a: &[Diagnostic], b: &[Diagnostic]) -> DiagnosticsDiff {
    let mut out = DiagnosticsDiff::default();
    for d in a {
        out.counts.entry(d.kind.clone()).or_default().0 += 1;
    }
    for d in b {
        out.counts.entry(d.kind.clone()).or_default().1 += 1;
    }
    let (mut a, mut b) = (a.to_vec(), b.to_vec());
    sort_diagnostics(&mut a);
    sort_diagnostics(&mut b);
    let key = |d: &Diagnostic| (d.kind.clone(), d.path.clone(), d.span, d.detail.to_string());
    for side in merge(a, b, |x, y| key(x).cmp(&key(y))) {
        match side {
            Side::Left(d) => out.removed.push(d),
            Side::Right(d) => out.added.push(d),
            Side::Both(..) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests;
