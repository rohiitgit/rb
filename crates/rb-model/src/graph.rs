use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;

use anyhow::{Context, bail, ensure};
use serde_json::{Map, Value, json};

use crate::{Certainty, EdgeKind, NodeKey, NodeKind, Provenance};

/// Attribute bag. `serde_json::Map` is a `BTreeMap` (the workspace must never enable serde_json's
/// `preserve_order` feature), so serialisation is key-sorted and deterministic.
pub type Attrs = Map<String, Value>;

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

    /// Rebuilds a graph from stored rows (e.g. read back from Postgres), validating that ids are
    /// unique, keys are unique and every edge endpoint exists.
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
        for e in &edges {
            ensure!(
                ids.contains_key(&e.src) && ids.contains_key(&e.dst),
                "edge {} {}->{} references a missing node",
                e.kind,
                e.src,
                e.dst
            );
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
    edges: BTreeMap<EdgeId, (Certainty, Attrs)>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self::default()
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

    /// Adds an edge. Re-adding an identical edge is a no-op; a conflicting one is an error.
    pub fn add_edge(
        &mut self,
        src: NodeKey,
        kind: EdgeKind,
        dst: NodeKey,
        provenance: Provenance,
        certainty: Certainty,
        attrs: Attrs,
    ) -> anyhow::Result<()> {
        let id = (src, kind, dst, provenance);
        let value = (certainty, attrs);
        if let Some(existing) = self.edges.get(&id) {
            ensure!(
                *existing == value,
                "conflicting definitions for edge {} {} {} ({})",
                id.0,
                id.1,
                id.2,
                id.3
            );
            return Ok(());
        }
        self.edges.insert(id, value);
        Ok(())
    }

    /// Assigns dense ids in key order and resolves edges. Fails on dangling edge endpoints.
    pub fn finish(self) -> anyhow::Result<Graph> {
        let mut ids = HashMap::with_capacity(self.nodes.len());
        let mut nodes = Vec::with_capacity(self.nodes.len());
        for (i, (key, (name, attrs))) in self.nodes.into_iter().enumerate() {
            let id = i64::try_from(i + 1).context("node count overflows i64")?;
            ids.insert(key.clone(), id);
            nodes.push(Node {
                id,
                kind: key.kind,
                key: key.key,
                name,
                attrs,
            });
        }
        let mut edges = Vec::with_capacity(self.edges.len());
        for ((src, kind, dst, provenance), (certainty, attrs)) in self.edges {
            let (Some(&s), Some(&d)) = (ids.get(&src), ids.get(&dst)) else {
                bail!("edge {src} {kind} {dst} has an endpoint that is not a node");
            };
            edges.push(Edge {
                src: s,
                dst: d,
                kind,
                provenance,
                certainty,
                attrs,
            });
        }
        // BTreeMap order is by key; ids follow key order, so this is already (src, kind, dst, prov) order.
        Ok(Graph { nodes, edges })
    }
}

/// Canonical, id-independent text dump of a snapshot (design §3.3, §11.1 `rb dump`).
///
/// One JSON array per line: a header, then nodes sorted by `(kind, key)`, then edges sorted by
/// `(src key, kind, dst key, provenance)`. Two snapshots of the same inputs must dump byte-identically.
pub fn canonical_dump(commit_sha: &str, graph: &Graph) -> String {
    let by_id: HashMap<i64, &Node> = graph.nodes.iter().map(|n| (n.id, n)).collect();
    let mut out = String::new();
    let mut line = |v: Value| {
        // `Value`'s Display is compact JSON and cannot fail.
        let _ = writeln!(out, "{v}");
    };

    line(json!(["rb-dump", 1, commit_sha]));

    let mut nodes: Vec<&Node> = graph.nodes.iter().collect();
    nodes.sort_by(|a, b| (a.kind, &a.key).cmp(&(b.kind, &b.key)));
    for n in nodes {
        line(json!(["N", n.kind.as_str(), n.key, n.name, n.attrs]));
    }

    let mut edges: Vec<(&Node, &Edge, &Node)> = graph
        .edges
        .iter()
        .map(|e| (by_id[&e.src], e, by_id[&e.dst]))
        .collect();
    edges.sort_by(|(s1, e1, d1), (s2, e2, d2)| {
        (s1.kind, &s1.key, e1.kind, d1.kind, &d1.key, e1.provenance).cmp(&(
            s2.kind,
            &s2.key,
            e2.kind,
            d2.kind,
            &d2.key,
            e2.provenance,
        ))
    });
    for (s, e, d) in edges {
        line(json!([
            "E",
            s.kind.as_str(),
            s.key,
            e.kind.as_str(),
            d.kind.as_str(),
            d.key,
            e.provenance.as_str(),
            e.certainty.rank(),
            e.attrs
        ]));
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

    fn build(order: &[&str]) -> Graph {
        let mut b = GraphBuilder::new();
        b.add_node(svc(), "(root)", Attrs::new()).unwrap();
        for p in order {
            b.add_node(file(p), *p, attrs(json!({"z": 1, "a": 2})))
                .unwrap();
            b.add_edge(
                file(p),
                EdgeKind::InService,
                svc(),
                Provenance::Manifest,
                Certainty::Exact,
                Attrs::new(),
            )
            .unwrap();
        }
        b.finish().unwrap()
    }

    #[test]
    fn insertion_order_does_not_matter() {
        let a = build(&["b.rs", "a.rs", "c/d.rs"]);
        let b = build(&["c/d.rs", "b.rs", "a.rs"]);
        assert_eq!(a, b);
        assert_eq!(canonical_dump("abc", &a), canonical_dump("abc", &b));
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
        let dump = canonical_dump("abc", &build(&["a.rs"]));
        let lines: Vec<&str> = dump.lines().collect();
        assert_eq!(lines[0], r#"["rb-dump",1,"abc"]"#);
        assert_eq!(lines[1], r#"["N","file","a.rs","a.rs",{"a":2,"z":1}]"#);
        assert_eq!(lines[2], r#"["N","service","(root)","(root)",{}]"#);
        assert_eq!(
            lines[3],
            r#"["E","file","a.rs","IN_SERVICE","service","(root)","manifest",5,{}]"#
        );
        assert_eq!(lines.len(), 4);
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
    }

    #[test]
    fn dangling_edges_fail_finish() {
        let mut b = GraphBuilder::new();
        b.add_node(file("a"), "a", Attrs::new()).unwrap();
        b.add_edge(
            file("a"),
            EdgeKind::InService,
            svc(),
            Provenance::Manifest,
            Certainty::Exact,
            Attrs::new(),
        )
        .unwrap();
        assert!(b.finish().is_err());
    }
}
