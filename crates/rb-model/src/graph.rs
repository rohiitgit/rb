use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use anyhow::{Context, bail, ensure};
use serde_json::{Map, Value, json};

use crate::{Certainty, EdgeKind, NodeKey, NodeKind, Provenance};

/// Attribute bag. `serde_json::Map` is a `BTreeMap` (the workspace must never enable serde_json's
/// `preserve_order` feature), so serialisation is key-sorted and deterministic.
pub type Attrs = Map<String, Value>;

/// `[start_line, start_col, end_line, end_col]`: 0-based, half-open, columns in UTF-8 bytes
/// (the SCIP / Tree-sitter convention). Displays add 1 to lines.
pub type Span = [i32; 4];

/// Source location that justifies an edge (design §6.3). `path` must be a `File` node.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Evidence {
    pub path: String,
    pub span: Span,
}

/// Something the indexer could not model or chose to skip (design §6.5, §12.4).
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub kind: String,
    pub path: Option<String>,
    pub span: Option<Span>,
    pub detail: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub id: i64,
    pub kind: NodeKind,
    pub key: String,
    pub name: String,
    pub attrs: Attrs,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    pub src: i64,
    pub dst: i64,
    pub kind: EdgeKind,
    pub provenance: Provenance,
    pub certainty: Certainty,
    pub attrs: Attrs,
    /// Sorted and de-duplicated.
    pub evidence: Vec<Evidence>,
}

/// An edge to add to a [`GraphBuilder`], addressed by natural keys.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEdge {
    pub src: NodeKey,
    pub kind: EdgeKind,
    pub dst: NodeKey,
    pub provenance: Provenance,
    pub certainty: Certainty,
    pub attrs: Attrs,
    pub evidence: Vec<Evidence>,
}

impl NewEdge {
    pub fn new(
        src: NodeKey,
        kind: EdgeKind,
        dst: NodeKey,
        provenance: Provenance,
        certainty: Certainty,
    ) -> Self {
        Self {
            src,
            kind,
            dst,
            provenance,
            certainty,
            attrs: Attrs::new(),
            evidence: Vec::new(),
        }
    }

    pub fn attrs(mut self, attrs: Attrs) -> Self {
        self.attrs = attrs;
        self
    }

    pub fn evidence(mut self, evidence: Evidence) -> Self {
        self.evidence.push(evidence);
        self
    }
}

/// An immutable snapshot graph. Nodes are sorted by natural key and numbered `1..=n` in that order;
/// edges are sorted by `(src, kind, dst, provenance)`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Graph {
    nodes: Vec<Node>,
    edges: Vec<Edge>,
}

impl Graph {
    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }

    /// Rebuilds a graph from stored rows (e.g. read back from Postgres), validating that ids and keys
    /// are unique, every edge endpoint exists and all evidence points at `File` nodes.
    pub fn from_parts(mut nodes: Vec<Node>, mut edges: Vec<Edge>) -> anyhow::Result<Self> {
        nodes.sort_by(|a, b| (a.kind, &a.key).cmp(&(b.kind, &b.key)));
        let mut ids = HashMap::with_capacity(nodes.len());
        for (i, n) in nodes.iter().enumerate() {
            ensure!(ids.insert(n.id, i).is_none(), "duplicate node id {}", n.id);
            if i > 0 {
                let prev = &nodes[i - 1];
                ensure!(
                    (prev.kind, &prev.key) != (n.kind, &n.key),
                    "duplicate node key {}:{}",
                    n.kind,
                    n.key
                );
            }
        }
        let files = file_paths(nodes.iter().map(|n| (n.kind, n.key.as_str())));
        for e in &mut edges {
            ensure!(
                ids.contains_key(&e.src) && ids.contains_key(&e.dst),
                "edge {} {}->{} references a missing node",
                e.kind,
                e.src,
                e.dst
            );
            check_evidence(&files, &e.evidence)?;
            e.evidence.sort();
            e.evidence.dedup();
        }
        edges.sort_by(|a, b| {
            (a.src, a.kind, a.dst, a.provenance).cmp(&(b.src, b.kind, b.dst, b.provenance))
        });
        Ok(Self { nodes, edges })
    }
}

type EdgeId = (NodeKey, EdgeKind, NodeKey, Provenance);

/// Collects nodes and edges by natural key; insertion order never affects the result.
#[derive(Debug, Default)]
pub struct GraphBuilder {
    nodes: BTreeMap<NodeKey, (String, Attrs)>,
    edges: BTreeMap<EdgeId, (Certainty, Attrs, BTreeSet<Evidence>)>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn has_node(&self, key: &NodeKey) -> bool {
        self.nodes.contains_key(key)
    }

    /// Adds a node. Re-adding an identical node is a no-op; a conflicting one is an error.
    pub fn add_node(
        &mut self,
        key: NodeKey,
        name: impl Into<String>,
        attrs: Attrs,
    ) -> anyhow::Result<()> {
        let value = (name.into(), attrs);
        if let Some(existing) = self.nodes.get(&key) {
            ensure!(*existing == value, "conflicting definitions for node {key}");
            return Ok(());
        }
        self.nodes.insert(key, value);
        Ok(())
    }

    /// Adds an edge. Re-adding the same edge merges its evidence; differing certainty or attrs is
    /// an error.
    pub fn add_edge(&mut self, edge: NewEdge) -> anyhow::Result<()> {
        let id = (edge.src, edge.kind, edge.dst, edge.provenance);
        match self.edges.get_mut(&id) {
            Some((certainty, attrs, evidence)) => {
                ensure!(
                    *certainty == edge.certainty && *attrs == edge.attrs,
                    "conflicting definitions for edge {} {} {} ({})",
                    id.0,
                    id.1,
                    id.2,
                    id.3
                );
                evidence.extend(edge.evidence);
            }
            None => {
                self.edges.insert(
                    id,
                    (
                        edge.certainty,
                        edge.attrs,
                        edge.evidence.into_iter().collect(),
                    ),
                );
            }
        }
        Ok(())
    }

    /// The sources of the edges of `kind` added so far.
    pub fn edge_sources(&self, kind: EdgeKind) -> BTreeSet<&NodeKey> {
        self.edges
            .keys()
            .filter(|(_, k, _, _)| *k == kind)
            .map(|(src, _, _, _)| src)
            .collect()
    }

    /// The sources of `IMPLEMENTS` edges into `dst` added so far: the definitions
    /// implementing it (contract M6f S4b: a route handled by an interface method is also
    /// handled by its implementors). One pass over the edges.
    pub fn implementors(&self, dst: &NodeKey) -> Vec<NodeKey> {
        let mut out: Vec<NodeKey> = self
            .edges
            .keys()
            .filter(|(_, k, d, _)| *k == EdgeKind::Implements && d == dst)
            .map(|(src, _, _, _)| src.clone())
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// Sets attribute `name` to `value` on every edge of `kind` added so far whose source is in
    /// `sources`; returns how many edges it set (contract M4b D4: RTA's
    /// `impl_instantiated: false` on `IMPLEMENTS`, set once every producer has run). An edge
    /// re-added afterwards with its former attributes is then a conflict, so callers run this
    /// after the edge kind's last producer.
    pub fn set_edge_attr(
        &mut self,
        kind: EdgeKind,
        sources: &BTreeSet<NodeKey>,
        name: &str,
        value: Value,
    ) -> usize {
        let mut n = 0;
        for ((src, k, _, _), (_, attrs, _)) in &mut self.edges {
            if *k == kind && sources.contains(src) {
                attrs.insert(name.to_owned(), value.clone());
                n += 1;
            }
        }
        n
    }

    /// Sets attribute `name` to `value` on the node `key`; returns whether the node exists
    /// (contract M4b S11/S12: attributes derived once every edge producer has run). A node
    /// re-added afterwards with its former attributes is then a conflict.
    pub fn set_node_attr(&mut self, key: &NodeKey, name: &str, value: Value) -> bool {
        match self.nodes.get_mut(key) {
            Some((_, attrs)) => {
                attrs.insert(name.to_owned(), value);
                true
            }
            None => false,
        }
    }

    /// For each of `dsts` with inbound edges added so far: the kind and the evidence of each
    /// edge into it, in edge-key order. One pass over the edges.
    pub fn inbound(
        &self,
        dsts: &BTreeSet<NodeKey>,
    ) -> BTreeMap<NodeKey, Vec<(EdgeKind, Vec<Evidence>)>> {
        let mut out: BTreeMap<NodeKey, Vec<(EdgeKind, Vec<Evidence>)>> = BTreeMap::new();
        for ((_, kind, dst, _), (_, _, evidence)) in &self.edges {
            if dsts.contains(dst) {
                out.entry(dst.clone())
                    .or_default()
                    .push((*kind, evidence.iter().cloned().collect()));
            }
        }
        out
    }

    /// Assigns dense ids in key order and resolves edges. Fails on dangling edge endpoints and on
    /// evidence that does not point at a `File` node.
    pub fn finish(self) -> anyhow::Result<Graph> {
        let files = file_paths(self.nodes.keys().map(|k| (k.kind, k.key.as_str())));
        let mut ids = HashMap::with_capacity(self.nodes.len());
        for (i, key) in self.nodes.keys().enumerate() {
            ids.insert(
                key.clone(),
                i64::try_from(i + 1).context("node count overflows i64")?,
            );
        }
        let mut edges = Vec::with_capacity(self.edges.len());
        for ((src, kind, dst, provenance), (certainty, attrs, evidence)) in self.edges {
            let (Some(&s), Some(&d)) = (ids.get(&src), ids.get(&dst)) else {
                bail!("edge {src} {kind} {dst} has an endpoint that is not a node");
            };
            let evidence: Vec<Evidence> = evidence.into_iter().collect();
            check_evidence(&files, &evidence)?;
            edges.push(Edge {
                src: s,
                dst: d,
                kind,
                provenance,
                certainty,
                attrs,
                evidence,
            });
        }
        let nodes = self
            .nodes
            .into_iter()
            .map(|(key, (name, attrs))| Node {
                id: ids[&key],
                kind: key.kind,
                key: key.key,
                name,
                attrs,
            })
            .collect();
        // BTreeMap order is by key; ids follow key order, so this is already (src, kind, dst, prov) order.
        Ok(Graph { nodes, edges })
    }
}

fn file_paths<'a>(nodes: impl Iterator<Item = (NodeKind, &'a str)>) -> HashSet<&'a str> {
    nodes
        .filter(|(k, _)| *k == NodeKind::File)
        .map(|(_, key)| key)
        .collect()
}

fn check_evidence(files: &HashSet<&str>, evidence: &[Evidence]) -> anyhow::Result<()> {
    for ev in evidence {
        ensure!(
            files.contains(ev.path.as_str()),
            "evidence references {:?}, which is not a File node",
            ev.path
        );
    }
    Ok(())
}

/// Sorts diagnostics into their canonical order (kind, path, span, detail).
pub fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|a, b| {
        (&a.kind, &a.path, &a.span, a.detail.to_string()).cmp(&(
            &b.kind,
            &b.path,
            &b.span,
            b.detail.to_string(),
        ))
    });
}

/// Canonical, id-independent text dump of a snapshot (design §3.3, §11.1 `rb dump`).
///
/// One JSON array per line: a header, nodes sorted by `(kind, key)`, edges sorted by
/// `(src key, kind, dst key, provenance)` with their evidence, then diagnostics in canonical order.
/// Two snapshots of the same inputs must dump byte-identically.
pub fn canonical_dump(commit_sha: &str, graph: &Graph, diagnostics: &[Diagnostic]) -> String {
    let mut out = String::new();
    // `Value`'s Display is compact JSON and cannot fail.
    let _ = writeln!(out, "{}", json!(["rb-dump", 2, commit_sha]));
    // The rows and their lines are `rb diff`'s (contract M5 S16): one rendering of each row.
    for n in crate::diff::node_rows(graph) {
        let _ = writeln!(out, "{}", n.line());
    }
    for e in crate::diff::edge_rows(graph) {
        let _ = writeln!(out, "{}", e.line());
    }
    let mut diagnostics = diagnostics.to_vec();
    sort_diagnostics(&mut diagnostics);
    for d in &diagnostics {
        let _ = writeln!(out, "{}", crate::diff::diagnostic_line(d));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(v: Value) -> Attrs {
        v.as_object().cloned().unwrap()
    }

    fn file(p: &str) -> NodeKey {
        NodeKey::new(NodeKind::File, p)
    }

    fn svc() -> NodeKey {
        NodeKey::new(NodeKind::Service, "(root)")
    }

    fn in_root(p: &str) -> NewEdge {
        NewEdge::new(
            file(p),
            EdgeKind::InService,
            svc(),
            Provenance::Manifest,
            Certainty::Exact,
        )
    }

    fn build(order: &[&str]) -> Graph {
        let mut b = GraphBuilder::new();
        b.add_node(svc(), "(root)", Attrs::new()).unwrap();
        for p in order {
            b.add_node(file(p), *p, attrs(json!({"z": 1, "a": 2})))
                .unwrap();
            b.add_edge(in_root(p)).unwrap();
        }
        b.finish().unwrap()
    }

    fn ev(p: &str, line: i32) -> Evidence {
        Evidence {
            path: p.into(),
            span: [line, 0, line, 5],
        }
    }

    #[test]
    fn insertion_order_does_not_matter() {
        let a = build(&["b.rs", "a.rs", "c/d.rs"]);
        let b = build(&["c/d.rs", "b.rs", "a.rs"]);
        assert_eq!(a, b);
        assert_eq!(
            canonical_dump("abc", &a, &[]),
            canonical_dump("abc", &b, &[])
        );
    }

    #[test]
    fn ids_are_dense_and_follow_key_order() {
        let g = build(&["b.rs", "a.rs"]);
        let got: Vec<(i64, &str)> = g.nodes().iter().map(|n| (n.id, n.key.as_str())).collect();
        // File sorts before Service (declaration order of NodeKind).
        assert_eq!(got, vec![(1, "a.rs"), (2, "b.rs"), (3, "(root)")]);
        assert_eq!(g.edges().len(), 2);
        assert!(g.edges().windows(2).all(|w| w[0].src < w[1].src));
    }

    #[test]
    fn dump_is_key_sorted_json_lines() {
        let diag = Diagnostic {
            kind: "skipped_symlink".into(),
            path: Some("l".into()),
            span: None,
            detail: json!({}),
        };
        let dump = canonical_dump("abc", &build(&["a.rs"]), &[diag]);
        let lines: Vec<&str> = dump.lines().collect();
        assert_eq!(
            lines,
            [
                r#"["rb-dump",2,"abc"]"#,
                r#"["N","file","a.rs","a.rs",{"a":2,"z":1}]"#,
                r#"["N","service","(root)","(root)",{}]"#,
                r#"["E","file","a.rs","IN_SERVICE","service","(root)","manifest",5,{},[]]"#,
                r#"["D","skipped_symlink","l",null,{}]"#,
            ]
        );
    }

    #[test]
    fn evidence_is_merged_sorted_and_deduplicated() {
        let mut b = GraphBuilder::new();
        b.add_node(svc(), "(root)", Attrs::new()).unwrap();
        b.add_node(file("a.rs"), "a.rs", Attrs::new()).unwrap();
        b.add_edge(in_root("a.rs").evidence(ev("a.rs", 7))).unwrap();
        b.add_edge(
            in_root("a.rs")
                .evidence(ev("a.rs", 2))
                .evidence(ev("a.rs", 7)),
        )
        .unwrap();
        let g = b.finish().unwrap();
        assert_eq!(g.edges()[0].evidence, vec![ev("a.rs", 2), ev("a.rs", 7)]);
        assert!(
            canonical_dump("c", &g, &[]).contains(r#"[["a.rs",[2,0,2,5]],["a.rs",[7,0,7,5]]]"#)
        );
    }

    #[test]
    fn evidence_must_point_at_a_file_node() {
        let mut b = GraphBuilder::new();
        b.add_node(svc(), "(root)", Attrs::new()).unwrap();
        b.add_node(file("a.rs"), "a.rs", Attrs::new()).unwrap();
        b.add_edge(in_root("a.rs").evidence(ev("missing.rs", 1)))
            .unwrap();
        assert!(b.finish().is_err());
    }

    #[test]
    fn from_parts_round_trips_and_reorders() {
        let g = build(&["a.rs", "b.rs"]);
        let mut nodes = g.nodes().to_vec();
        let mut edges = g.edges().to_vec();
        nodes.reverse();
        edges.reverse();
        let back = Graph::from_parts(nodes, edges).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn from_parts_rejects_dangling_and_duplicates() {
        let g = build(&["a.rs"]);
        let mut edges = g.edges().to_vec();
        edges[0].dst = 99;
        assert!(Graph::from_parts(g.nodes().to_vec(), edges).is_err());

        let mut nodes = g.nodes().to_vec();
        nodes.push(Node {
            id: 42,
            ..nodes[0].clone()
        });
        assert!(Graph::from_parts(nodes, g.edges().to_vec()).is_err());
    }

    #[test]
    fn conflicting_duplicates_are_rejected_identical_ones_ignored() {
        let mut b = GraphBuilder::new();
        b.add_node(file("a"), "a", Attrs::new()).unwrap();
        b.add_node(file("a"), "a", Attrs::new()).unwrap();
        assert!(b.add_node(file("a"), "other", Attrs::new()).is_err());

        b.add_node(svc(), "(root)", Attrs::new()).unwrap();
        b.add_edge(in_root("a")).unwrap();
        let mut heuristic = in_root("a");
        heuristic.certainty = Certainty::Heuristic;
        assert!(b.add_edge(heuristic).is_err());
    }

    /// Contract M4b D4: an attribute set on the existing edges of one kind and source set;
    /// other kinds and sources are untouched, and a later re-add with the old attributes
    /// conflicts (the producers must run first).
    #[test]
    fn edge_attributes_set_by_kind_and_source() {
        let mut b = GraphBuilder::new();
        b.add_node(svc(), "(root)", Attrs::new()).unwrap();
        for p in ["a", "b"] {
            b.add_node(file(p), p, Attrs::new()).unwrap();
            b.add_edge(in_root(p)).unwrap();
        }
        b.add_edge(NewEdge::new(
            file("a"),
            EdgeKind::Contains,
            file("b"),
            Provenance::Manifest,
            Certainty::Exact,
        ))
        .unwrap();
        assert_eq!(
            b.edge_sources(EdgeKind::InService)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>(),
            [file("a"), file("b")]
        );
        let n = b.set_edge_attr(
            EdgeKind::InService,
            &BTreeSet::from([file("a")]),
            "flag",
            Value::Bool(false),
        );
        assert_eq!(n, 1);
        assert!(b.add_edge(in_root("a")).is_err());
        let g = b.finish().unwrap();
        let flagged: Vec<(EdgeKind, bool)> = g
            .edges()
            .iter()
            .map(|e| (e.kind, e.attrs.contains_key("flag")))
            .collect();
        assert_eq!(
            flagged,
            [
                (EdgeKind::InService, true),
                (EdgeKind::Contains, false),
                (EdgeKind::InService, false)
            ]
        );
    }

    /// Contract M4b S11/S12: node attributes set after every producer has run (the `entry` and
    /// `orphan_eligible` attributes), and the kinds and evidence of the edges into chosen nodes.
    #[test]
    fn node_attributes_and_inbound_evidence() {
        let mut b = GraphBuilder::new();
        b.add_node(svc(), "(root)", Attrs::new()).unwrap();
        for p in ["a", "b"] {
            b.add_node(file(p), p, Attrs::new()).unwrap();
        }
        let at = |line: i32| Evidence {
            path: "a".into(),
            span: [line, 0, line, 3],
        };
        b.add_edge(in_root("a").evidence(at(1))).unwrap();
        b.add_edge(in_root("b").evidence(at(2))).unwrap();
        b.add_edge(in_root("b").evidence(at(3))).unwrap();
        let inbound = b.inbound(&BTreeSet::from([svc()]));
        assert_eq!(
            inbound[&svc()],
            [
                (EdgeKind::InService, vec![at(1)]),
                (EdgeKind::InService, vec![at(2), at(3)])
            ]
        );
        assert!(b.inbound(&BTreeSet::from([file("a")])).is_empty());
        assert!(b.set_node_attr(&file("b"), "flag", Value::Bool(true)));
        assert!(!b.set_node_attr(&file("c"), "flag", Value::Bool(true)));
        // A node re-added with its former attributes now conflicts.
        assert!(b.add_node(file("b"), "b", Attrs::new()).is_err());
        let g = b.finish().unwrap();
        let flagged: Vec<(&str, bool)> = g
            .nodes()
            .iter()
            .map(|n| (n.key.as_str(), n.attrs.contains_key("flag")))
            .collect();
        assert_eq!(flagged, [("a", false), ("b", true), ("(root)", false)]);
    }

    #[test]
    fn dangling_edges_fail_finish() {
        let mut b = GraphBuilder::new();
        b.add_node(file("a"), "a", Attrs::new()).unwrap();
        b.add_edge(in_root("a")).unwrap();
        assert!(b.finish().is_err());
    }
}
