//! `rb diff`'s summaries (contract M5 S17, D11): what the exact sections mean for a reader, each a
//! pure function of the two snapshots' graphs, computed with the Service Card's own rule for
//! which service a node belongs to ([`crate::card::Membership`]):
//! - endpoints added and removed, by key, shown with their router and service;
//! - table access added and removed, as `(service, table, op)`;
//! - service dependencies added and removed, as `(from, to, via)` with the card's `via`
//!   (`calls`, `http`, `package`);
//! - per edge kind, the edges whose certainty rose or fell (from the exact sections).
//!
//! The card's per-service sections equal these sets restricted to the service (tested on every
//! fixture).

use std::collections::{BTreeMap, BTreeSet};

use rb_model::diff::GraphDiff;
use rb_model::{EdgeKind, NodeKind};
use serde::Serialize;

use crate::card::Membership;
use crate::code::{SnapshotIndex, endpoint_display};

/// An endpoint as the summary lists it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct EndpointRow {
    pub key: String,
    /// The name, with the router of a prefix-unknown endpoint.
    pub display: String,
    /// The exposing service's name (its key when no node names it).
    pub service: String,
}

/// A service's direct access to a table: one row per operation (`mapped` for a mapping with no
/// operation), from code (`QUERIES`), mappings (`MAPS_TO`) and migrations (`MIGRATES`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct TableAccessRow {
    pub service: String,
    pub table: String,
    pub op: String,
}

/// A dependency of one service on another, by the card's `via`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct DependencyRow {
    pub from: String,
    pub to: String,
    pub via: String,
}

/// Edges of one kind whose certainty rose or fell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Shift {
    pub raised: usize,
    pub lowered: usize,
}

/// Added and removed rows of one summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Delta<T> {
    pub added: Vec<T>,
    pub removed: Vec<T>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Summaries {
    pub endpoints: Delta<EndpointRow>,
    pub table_access: Delta<TableAccessRow>,
    pub service_dependencies: Delta<DependencyRow>,
    /// Edge kind → certainty shifts, for the kinds with any.
    pub certainty: BTreeMap<String, Shift>,
}

/// The snapshot's endpoints.
pub fn endpoints(ix: &SnapshotIndex) -> BTreeSet<EndpointRow> {
    let graph = ix.graph();
    let names: BTreeMap<&str, &str> = graph
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::Service)
        .map(|n| (n.key.as_str(), n.name.as_str()))
        .collect();
    graph
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::Endpoint)
        .map(|n| {
            let service = n
                .attrs
                .get("service")
                .and_then(|s| s.as_str())
                .unwrap_or_default();
            EndpointRow {
                key: n.key.clone(),
                display: endpoint_display(n),
                service: names.get(service).copied().unwrap_or(service).to_owned(),
            }
        })
        .collect()
}

/// Every service's direct table access.
pub fn table_access(ix: &SnapshotIndex) -> BTreeSet<TableAccessRow> {
    let m = Membership::new(ix);
    let mut out = BTreeSet::new();
    for e in ix.graph().edges() {
        if !matches!(
            e.kind,
            EdgeKind::Queries | EdgeKind::MapsTo | EdgeKind::Migrates
        ) || ix.node(e.dst).kind != NodeKind::Table
        {
            continue;
        }
        let Some(s) = m.service_of(ix, e.src) else {
            continue;
        };
        let ops: Vec<String> = e
            .attrs
            .get("ops")
            .and_then(|o| o.as_array())
            .into_iter()
            .flatten()
            .filter_map(|o| o.as_str().map(str::to_owned))
            .collect();
        let ops = if ops.is_empty() {
            vec!["mapped".to_owned()]
        } else {
            ops
        };
        for op in ops {
            out.insert(TableAccessRow {
                service: ix.node(s).key.clone(),
                table: ix.node(e.dst).key.clone(),
                op,
            });
        }
    }
    out
}

/// Every service's dependencies on other services, as the card derives them for one: a
/// `DEPENDS_ON` between services (`package`), a call or reference into another service's code
/// (`calls`), a `CONSUMES` of another service's endpoint (`http`).
pub fn service_dependencies(ix: &SnapshotIndex) -> BTreeSet<DependencyRow> {
    let m = Membership::new(ix);
    let mut out = BTreeSet::new();
    for e in ix.graph().edges() {
        let via = match e.kind {
            EdgeKind::DependsOn
                if ix.node(e.src).kind == NodeKind::Service
                    && ix.node(e.dst).kind == NodeKind::Service =>
            {
                "package"
            }
            EdgeKind::Calls | EdgeKind::References => "calls",
            EdgeKind::Consumes => "http",
            _ => continue,
        };
        let (Some(s), Some(t)) = (m.service_of(ix, e.src), m.service_of(ix, e.dst)) else {
            continue;
        };
        if s != t {
            out.insert(DependencyRow {
                from: ix.node(s).key.clone(),
                to: ix.node(t).key.clone(),
                via: via.to_owned(),
            });
        }
    }
    out
}

fn delta<T: Ord + Clone>(a: &BTreeSet<T>, b: &BTreeSet<T>) -> Delta<T> {
    Delta {
        added: b.difference(a).cloned().collect(),
        removed: a.difference(b).cloned().collect(),
    }
}

/// The summaries of the change from `a` to `b`, whose exact sections are `exact`.
pub fn summaries(a: &SnapshotIndex, b: &SnapshotIndex, exact: &GraphDiff) -> Summaries {
    let mut certainty: BTreeMap<String, Shift> = BTreeMap::new();
    for c in &exact.edges_changed {
        if c.before.certainty == c.after.certainty {
            continue;
        }
        let shift = certainty
            .entry(c.after.kind.as_str().to_owned())
            .or_default();
        if c.after.certainty > c.before.certainty {
            shift.raised += 1;
        } else {
            shift.lowered += 1;
        }
    }
    Summaries {
        endpoints: delta(&endpoints(a), &endpoints(b)),
        table_access: delta(&table_access(a), &table_access(b)),
        service_dependencies: delta(&service_dependencies(a), &service_dependencies(b)),
        certainty,
    }
}

#[cfg(test)]
mod tests {
    use rb_model::{
        Attrs, Certainty, Evidence, GraphBuilder, NewEdge, NodeKey, Provenance, diff::diff,
    };
    use serde_json::json;

    use super::*;

    /// Two services `a` and `b`, a file each, a symbol each; `edits` adds the rest.
    fn graph(edits: impl FnOnce(&mut GraphBuilder)) -> SnapshotIndex {
        let mut g = GraphBuilder::new();
        for s in ["a", "b"] {
            let mut role = Attrs::new();
            role.insert("role".into(), "service".into());
            g.add_node(NodeKey::new(NodeKind::Service, s), s.to_uppercase(), role)
                .unwrap();
            let f = format!("{s}/x.ts");
            g.add_node(NodeKey::new(NodeKind::File, &f), "x.ts", Attrs::new())
                .unwrap();
            g.add_edge(NewEdge::new(
                NodeKey::new(NodeKind::File, &f),
                EdgeKind::InService,
                NodeKey::new(NodeKind::Service, s),
                Provenance::Manifest,
                Certainty::Exact,
            ))
            .unwrap();
            g.add_node(NodeKey::new(NodeKind::Symbol, s), s, Attrs::new())
                .unwrap();
            g.add_edge(
                NewEdge::new(
                    NodeKey::new(NodeKind::Symbol, s),
                    EdgeKind::DefinedIn,
                    NodeKey::new(NodeKind::File, &f),
                    Provenance::Treesitter,
                    Certainty::Exact,
                )
                .evidence(Evidence {
                    path: f.clone(),
                    span: [0, 0, 0, 1],
                }),
            )
            .unwrap();
        }
        g.add_node(NodeKey::new(NodeKind::Table, "public.t"), "t", Attrs::new())
            .unwrap();
        edits(&mut g);
        SnapshotIndex::new(g.finish().unwrap())
    }

    fn query(g: &mut GraphBuilder, ops: &[&str], certainty: Certainty) {
        g.add_edge(
            NewEdge::new(
                NodeKey::new(NodeKind::Symbol, "a"),
                EdgeKind::Queries,
                NodeKey::new(NodeKind::Table, "public.t"),
                Provenance::SqlLiteral,
                certainty,
            )
            .attrs(json!({"ops": ops}).as_object().cloned().unwrap()),
        )
        .unwrap();
    }

    fn call(g: &mut GraphBuilder) {
        g.add_edge(NewEdge::new(
            NodeKey::new(NodeKind::Symbol, "a"),
            EdgeKind::Calls,
            NodeKey::new(NodeKind::Symbol, "b"),
            Provenance::Heuristic,
            Certainty::Heuristic,
        ))
        .unwrap();
    }

    /// Contract M5 S17: each summary from a graph pair whose only difference is that row.
    #[test]
    fn each_summary_names_its_rows() {
        let base = graph(|g| query(g, &["read"], Certainty::Heuristic));
        let ops = graph(|g| query(g, &["read", "update"], Certainty::Exact));
        let d = summaries(&base, &ops, &diff(base.graph(), ops.graph()));
        assert_eq!(
            d.table_access.added,
            [TableAccessRow {
                service: "a".into(),
                table: "public.t".into(),
                op: "update".into()
            }]
        );
        assert!(d.table_access.removed.is_empty());
        assert_eq!(
            d.certainty,
            BTreeMap::from([(
                "QUERIES".to_owned(),
                Shift {
                    raised: 1,
                    lowered: 0
                }
            )])
        );
        // The reverse: removed and lowered.
        let back = summaries(&ops, &base, &diff(ops.graph(), base.graph()));
        assert_eq!(back.table_access.removed.len(), 1);
        assert_eq!(back.certainty["QUERIES"].lowered, 1);

        let called = graph(|g| {
            query(g, &["read"], Certainty::Heuristic);
            call(g);
        });
        let d = summaries(&base, &called, &diff(base.graph(), called.graph()));
        assert_eq!(
            d.service_dependencies.added,
            [DependencyRow {
                from: "a".into(),
                to: "b".into(),
                via: "calls".into()
            }]
        );
        assert!(d.endpoints.added.is_empty() && d.certainty.is_empty());

        let exposed = graph(|g| {
            query(g, &["read"], Certainty::Heuristic);
            let ep = NodeKey::new(NodeKind::Endpoint, "HTTP GET /x @b");
            let attrs = json!({"service": "b"}).as_object().cloned().unwrap();
            g.add_node(ep.clone(), "GET /x", attrs).unwrap();
            g.add_edge(NewEdge::new(
                NodeKey::new(NodeKind::Service, "b"),
                EdgeKind::Exposes,
                ep.clone(),
                Provenance::Treesitter,
                Certainty::Exact,
            ))
            .unwrap();
            g.add_edge(NewEdge::new(
                NodeKey::new(NodeKind::Symbol, "a"),
                EdgeKind::Consumes,
                ep,
                Provenance::HttpStitch,
                Certainty::Heuristic,
            ))
            .unwrap();
        });
        let d = summaries(&base, &exposed, &diff(base.graph(), exposed.graph()));
        assert_eq!(
            d.endpoints.added,
            [EndpointRow {
                key: "HTTP GET /x @b".into(),
                display: "GET /x".into(),
                service: "B".into()
            }]
        );
        assert_eq!(
            d.service_dependencies.added,
            [DependencyRow {
                from: "a".into(),
                to: "b".into(),
                via: "http".into()
            }]
        );
        // A call within one service is no dependency.
        let same = graph(|g| {
            query(g, &["read"], Certainty::Heuristic);
            g.add_edge(NewEdge::new(
                NodeKey::new(NodeKind::Symbol, "a"),
                EdgeKind::Calls,
                NodeKey::new(NodeKind::Symbol, "a"),
                Provenance::Heuristic,
                Certainty::Heuristic,
            ))
            .unwrap();
        });
        assert_eq!(
            summaries(&base, &same, &diff(base.graph(), same.graph())),
            summaries(&base, &base, &diff(base.graph(), base.graph()))
        );
    }
}
