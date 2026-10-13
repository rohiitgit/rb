//! Architecture rules as a CI gate (design §13 M6h row, contract M6h S1–S2): named
//! structural rules, each with a read-only Cypher query (the product path, over the live AGE
//! projection) and a relational twin (this module: pure functions of the snapshot graph),
//! proven equal on every fixture.
//!
//! Violations are computed at query time from the live snapshot; nothing is stored. Every rule
//! returns `(src key, dst key)` rows sorted by `(rule, src, dst)`, so repeated runs print
//! byte-identically regardless of engine row order.

use std::collections::{BTreeMap, BTreeSet};

use rb_model::{Certainty, EdgeKind, Graph, NodeKind};

/// Endpoints no client consumes.
pub const RULE_ORPHAN_ENDPOINTS: &str = "orphan_endpoints";
/// Data-modifying queries on a table another service migrates.
pub const RULE_UNOWNED_TABLE_WRITES: &str = "unowned_table_writes";
/// Dependency cycles over stored Service → Service `DEPENDS_ON`.
pub const RULE_SERVICE_CYCLES: &str = "service_cycles";

/// One built-in rule: its name, what it means, and its read-only Cypher query (single `MATCH`,
/// `ORDER BY` its two returned keys; timeouts and row caps ride the shared `cypher()` path).
pub struct Rule {
    pub name: &'static str,
    pub description: &'static str,
    pub cypher: &'static str,
}

pub const RULES: &[Rule] = &[
    Rule {
        name: RULE_ORPHAN_ENDPOINTS,
        description: "an endpoint no client site consumes (an ambiguous link does not count)",
        cypher: "MATCH (s:Service)-[:EXPOSES]->(e:Endpoint) OPTIONAL MATCH ()-[c:CONSUMES]->(e) WHERE c.certainty > 1 WITH s, e, COUNT(c) AS linked WHERE linked = 0 RETURN s.key, e.key ORDER BY s.key, e.key",
    },
    Rule {
        name: RULE_UNOWNED_TABLE_WRITES,
        description: "an insert/update/delete/merge query on a table migrated by another service",
        cypher: "MATCH (t:Table)<-[q:QUERIES]-(x) WHERE ANY(op IN q.ops WHERE op IN ['insert', 'update', 'delete', 'merge']) OPTIONAL MATCH (x)-[:DEFINED_IN]->(df:File)-[:IN_SERVICE]->(s1:Service) OPTIONAL MATCH (x)-[:IN_SERVICE]->(s2:Service) OPTIONAL MATCH (mf:File)-[:MIGRATES]->(t) OPTIONAL MATCH (mf)-[:IN_SERVICE]->(o:Service) WITH t, COALESCE(s1, s2) AS s, COLLECT(DISTINCT o.key) AS owners WHERE NOT s.key IN owners AND SIZE(owners) > 0 RETURN DISTINCT s.key, t.key ORDER BY s.key, t.key",
    },
    Rule {
        name: RULE_SERVICE_CYCLES,
        description: "a stored Service -> Service DEPENDS_ON edge on a dependency cycle",
        cypher: "MATCH p=(a:Service)-[:DEPENDS_ON*1..8]->(a) UNWIND RELATIONSHIPS(p) AS r WITH DISTINCT STARTNODE(r) AS x, ENDNODE(r) AS y RETURN x.key, y.key ORDER BY x.key, y.key",
    },
];

/// One rule violation: `(rule, src key, dst key)`, ordered.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Violation {
    pub rule: &'static str,
    pub src: String,
    pub dst: String,
}

/// Every violation of every rule on the graph, sorted by `(rule, src, dst)`.
pub fn check(graph: &Graph) -> Vec<Violation> {
    let mut out = Vec::new();
    out.extend(orphan_endpoints(graph));
    out.extend(unowned_table_writes(graph));
    out.extend(service_cycles(graph));
    out.sort();
    out
}

/// The key of the service a node belongs to: services own themselves, files through
/// `IN_SERVICE`, symbols through `DEFINED_IN` + `IN_SERVICE`, endpoints through their
/// `service` attribute (falling back to their `EXPOSES` source). Anything else has none.
fn service_of(graph: &Graph, by_id: &BTreeMap<i64, usize>, id: i64) -> Option<String> {
    let node = &graph.nodes()[by_id[&id]];
    match node.kind {
        NodeKind::Service => Some(node.key.clone()),
        NodeKind::File => graph.edges().iter().find_map(|e| {
            (e.src == id && e.kind == EdgeKind::InService)
                .then(|| graph.nodes()[by_id[&e.dst]].key.clone())
        }),
        NodeKind::Symbol => {
            let file = graph
                .edges()
                .iter()
                .find_map(|e| (e.src == id && e.kind == EdgeKind::DefinedIn).then_some(e.dst))?;
            service_of(graph, by_id, file)
        }
        NodeKind::Endpoint => {
            if let Some(service) = node.attrs.get("service").and_then(|v| v.as_str()) {
                return Some(service.to_owned());
            }
            graph.edges().iter().find_map(|e| {
                (e.dst == id && e.kind == EdgeKind::Exposes)
                    .then(|| graph.nodes()[by_id[&e.src]].key.clone())
            })
        }
        _ => None,
    }
}

/// Endpoints with no inbound `CONSUMES`: `(exposing service, endpoint)`.
fn orphan_endpoints(graph: &Graph) -> Vec<Violation> {
    crate::work::tick(graph.edges().len() + 1);
    // An ambiguous link guesses at several endpoints; it is no evidence this one is called
    // (contract M7g S3).
    let consumed: BTreeSet<i64> = graph
        .edges()
        .iter()
        .filter(|e| e.kind == EdgeKind::Consumes && e.certainty > Certainty::Ambiguous)
        .map(|e| e.dst)
        .collect();
    let by_id: BTreeMap<i64, usize> = graph
        .nodes()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id, i))
        .collect();
    graph
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::Endpoint && !consumed.contains(&n.id))
        .filter_map(|n| {
            service_of(graph, &by_id, n.id).map(|service| Violation {
                rule: RULE_ORPHAN_ENDPOINTS,
                src: service,
                dst: n.key.clone(),
            })
        })
        .collect()
}

/// Data-modifying operations on a table.
fn is_write(op: &str) -> bool {
    matches!(op, "insert" | "update" | "delete" | "merge")
}

/// `QUERIES` with a write op from a service that does not migrate the table, while another
/// service does: `(querying service, table)`. Tables nobody migrates never fire.
fn unowned_table_writes(graph: &Graph) -> Vec<Violation> {
    crate::work::tick(graph.edges().len() + 1);
    let by_id: BTreeMap<i64, usize> = graph
        .nodes()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id, i))
        .collect();
    // Table id → services migrating it.
    let mut owners: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for e in graph
        .edges()
        .iter()
        .filter(|e| e.kind == EdgeKind::Migrates)
    {
        if let Some(service) = service_of(graph, &by_id, e.src) {
            owners.entry(e.dst).or_default().insert(service);
        }
    }
    let mut out = BTreeSet::new();
    for e in graph.edges().iter().filter(|e| e.kind == EdgeKind::Queries) {
        let writes = e
            .attrs
            .get("ops")
            .and_then(|v| v.as_array())
            .is_some_and(|ops| ops.iter().any(|op| op.as_str().is_some_and(is_write)));
        if !writes {
            continue;
        }
        let (Some(service), Some(table_owners)) =
            (service_of(graph, &by_id, e.src), owners.get(&e.dst))
        else {
            continue;
        };
        if !table_owners.is_empty() && !table_owners.contains(&service) {
            let table = graph.nodes()[by_id[&e.dst]].key.clone();
            out.insert(Violation {
                rule: RULE_UNOWNED_TABLE_WRITES,
                src: service,
                dst: table,
            });
        }
    }
    out.into_iter().collect()
}

/// Stored Service → Service `DEPENDS_ON` links.
fn service_links(graph: &Graph) -> BTreeSet<(String, String)> {
    let by_id: BTreeMap<i64, usize> = graph
        .nodes()
        .iter()
        .enumerate()
        .map(|(i, n)| (n.id, i))
        .collect();
    graph
        .edges()
        .iter()
        .filter(|e| {
            e.kind == EdgeKind::DependsOn
                && graph.nodes()[by_id[&e.src]].kind == NodeKind::Service
                && graph.nodes()[by_id[&e.dst]].kind == NodeKind::Service
        })
        .map(|e| {
            (
                graph.nodes()[by_id[&e.src]].key.clone(),
                graph.nodes()[by_id[&e.dst]].key.clone(),
            )
        })
        .collect()
}

/// Strongly connected components over the links (iterative Tarjan): each component as its
/// sorted members. Self-links are components of one.
fn components(links: &BTreeSet<(String, String)>) -> Vec<Vec<String>> {
    crate::work::tick(links.len() + 1);
    let mut adj: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (a, b) in links {
        adj.entry(a).or_default().push(b);
        adj.entry(b).or_default();
    }
    // Iterative Tarjan (explicit call stack): linear, no recursion depth to bound.
    let mut index_of: BTreeMap<&str, usize> = BTreeMap::new();
    let mut low: BTreeMap<&str, usize> = BTreeMap::new();
    let mut on_stack: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = Vec::new();
    let mut next_index = 0usize;
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut work: Vec<(&str, usize)> = Vec::new();
    for root in adj.keys().copied().collect::<Vec<_>>() {
        if index_of.contains_key(root) {
            continue;
        }
        work.push((root, 0));
        while let Some(&(node, child)) = work.last() {
            if child == 0 {
                index_of.insert(node, next_index);
                low.insert(node, next_index);
                next_index += 1;
                stack.push(node);
                on_stack.insert(node);
            }
            let neighbours: &[&str] = adj.get(node).map(Vec::as_slice).unwrap_or(&[]);
            if child < neighbours.len() {
                work.last_mut().expect("non-empty").1 += 1;
                let next = neighbours[child];
                if !index_of.contains_key(next) {
                    work.push((next, 0));
                } else if on_stack.contains(next) {
                    let low_node = low[&node].min(index_of[&next]);
                    low.insert(node, low_node);
                }
            } else {
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    let low_parent = low[&parent].min(low[&node]);
                    low.insert(parent, low_parent);
                }
                if low[&node] == index_of[&node] {
                    let mut component = Vec::new();
                    while let Some(member) = stack.pop() {
                        on_stack.remove(member);
                        component.push(member.to_owned());
                        if member == node {
                            break;
                        }
                    }
                    component.sort();
                    out.push(component);
                }
            }
        }
    }
    out.sort();
    out
}

/// Every stored Service → Service `DEPENDS_ON` link on a dependency cycle (a self-link counts):
/// `(src service, dst service)`, sorted.
fn service_cycles(graph: &Graph) -> Vec<Violation> {
    let links = service_links(graph);
    let mut cyclic: BTreeSet<(&str, &str)> = BTreeSet::new();
    for component in components(&links) {
        if component.len() > 1 {
            let members: BTreeSet<&str> = component.iter().map(String::as_str).collect();
            for (a, b) in &links {
                if members.contains(a.as_str()) && members.contains(b.as_str()) {
                    cyclic.insert((a, b));
                }
            }
        }
    }
    for (a, b) in &links {
        if a == b {
            cyclic.insert((a, b));
        }
    }
    cyclic
        .into_iter()
        .map(|(a, b)| Violation {
            rule: RULE_SERVICE_CYCLES,
            src: a.to_owned(),
            dst: b.to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use rb_model::{Attrs, Certainty, GraphBuilder, NewEdge, NodeKey, Provenance};
    use serde_json::json;

    use super::*;

    fn svc(key: &str) -> (NodeKey, &str, Attrs) {
        (NodeKey::new(NodeKind::Service, key), key, Attrs::new())
    }

    /// An endpoint with an exposing service, optional consumers, and tables with migrates/queries.
    fn graph() -> Graph {
        let mut b = GraphBuilder::new();
        for (key, name, attrs) in [
            svc("a"),
            svc("b"),
            (NodeKey::new(NodeKind::File, "a/f.ts"), "f.ts", Attrs::new()),
            (
                NodeKey::new(NodeKind::Endpoint, "HTTP GET /x @a"),
                "GET /x",
                [("service".to_owned(), json!("a"))].into_iter().collect(),
            ),
            (
                NodeKey::new(NodeKind::Endpoint, "HTTP GET /y @b"),
                "GET /y",
                [("service".to_owned(), json!("b"))].into_iter().collect(),
            ),
            (NodeKey::new(NodeKind::Table, "public.t"), "t", Attrs::new()),
            (NodeKey::new(NodeKind::Symbol, "a#f"), "f", Attrs::new()),
        ] {
            b.add_node(key, name, attrs).unwrap();
        }
        let mut edge = |src: NodeKey, kind: EdgeKind, dst: NodeKey| {
            b.add_edge(NewEdge::new(
                src,
                kind,
                dst,
                Provenance::Treesitter,
                Certainty::Exact,
            ))
            .unwrap()
        };
        edge(
            NodeKey::new(NodeKind::Service, "a"),
            EdgeKind::Exposes,
            NodeKey::new(NodeKind::Endpoint, "HTTP GET /x @a"),
        );
        edge(
            NodeKey::new(NodeKind::Service, "b"),
            EdgeKind::Exposes,
            NodeKey::new(NodeKind::Endpoint, "HTTP GET /y @b"),
        );
        // Only /x is consumed.
        edge(
            NodeKey::new(NodeKind::Symbol, "a#f"),
            EdgeKind::Consumes,
            NodeKey::new(NodeKind::Endpoint, "HTTP GET /x @a"),
        );
        // a migrates t; b's symbol writes t without migrating it.
        edge(
            NodeKey::new(NodeKind::File, "a/f.ts"),
            EdgeKind::Migrates,
            NodeKey::new(NodeKind::Table, "public.t"),
        );
        let mut writes = NewEdge::new(
            NodeKey::new(NodeKind::Symbol, "a#f"),
            EdgeKind::Queries,
            NodeKey::new(NodeKind::Table, "public.t"),
            Provenance::SqlLiteral,
            Certainty::Heuristic,
        );
        writes.attrs = [("ops".to_owned(), json!(["read", "insert"]))]
            .into_iter()
            .collect();
        b.add_edge(writes).unwrap();
        b.finish().unwrap()
    }

    /// Contract M7g S3 (stress F24): only a non-ambiguous `CONSUMES` clears an endpoint — an
    /// ambiguous link is a guess at several endpoints, not evidence that this one is called —
    /// in the relational twin and in the rule's Cypher (bound to the same rank).
    #[test]
    fn ambiguous_consumers_do_not_clear_an_orphan() {
        let mut b = GraphBuilder::new();
        for (key, name, attrs) in [
            svc("a"),
            (
                NodeKey::new(NodeKind::Endpoint, "HTTP GET /z @a"),
                "GET /z",
                [("service".to_owned(), json!("a"))].into_iter().collect(),
            ),
            (NodeKey::new(NodeKind::Symbol, "c#g"), "g", Attrs::new()),
        ] {
            b.add_node(key, name, attrs).unwrap();
        }
        let z = NodeKey::new(NodeKind::Endpoint, "HTTP GET /z @a");
        b.add_edge(NewEdge::new(
            NodeKey::new(NodeKind::Service, "a"),
            EdgeKind::Exposes,
            z.clone(),
            Provenance::Treesitter,
            Certainty::Exact,
        ))
        .unwrap();
        b.add_edge(NewEdge::new(
            NodeKey::new(NodeKind::Symbol, "c#g"),
            EdgeKind::Consumes,
            z,
            Provenance::HttpStitch,
            Certainty::Ambiguous,
        ))
        .unwrap();
        let found = orphan_endpoints(&b.finish().unwrap());
        assert_eq!(found.len(), 1, "{found:?}");
        let rule = RULES
            .iter()
            .find(|r| r.name == RULE_ORPHAN_ENDPOINTS)
            .unwrap();
        assert!(
            rule.cypher
                .contains(&format!("c.certainty > {}", Certainty::Ambiguous.rank())),
            "{}",
            rule.cypher
        );
    }

    /// Contract M6h S1: orphan endpoints report their service and key; consumed ones stay silent.
    #[test]
    fn orphan_endpoints_name_their_service() {
        let found = orphan_endpoints(&graph());
        assert_eq!(
            found,
            [Violation {
                rule: RULE_ORPHAN_ENDPOINTS,
                src: "b".into(),
                dst: "HTTP GET /y @b".into(),
            }]
        );
    }

    /// Contract M6h S1: a service writing a table another service migrates violates; reads and
    /// owned writes stay silent.
    #[test]
    fn unowned_table_writes_fire_only_for_foreign_writes() {
        // b's symbol queries t (migrated by a): fresh graph per ops set.
        let build = |ops: serde_json::Value| {
            let mut b = GraphBuilder::new();
            for (key, name, attrs) in [
                svc("a"),
                svc("b"),
                (NodeKey::new(NodeKind::File, "a/f"), "f", Attrs::new()),
                (NodeKey::new(NodeKind::File, "b/g"), "g", Attrs::new()),
                (NodeKey::new(NodeKind::Symbol, "s"), "s", Attrs::new()),
                (NodeKey::new(NodeKind::Table, "public.t"), "t", Attrs::new()),
            ] {
                b.add_node(key, name, attrs).unwrap();
            }
            let edge = |b: &mut GraphBuilder, src: NodeKey, kind: EdgeKind, dst: NodeKey| {
                b.add_edge(NewEdge::new(
                    src,
                    kind,
                    dst,
                    Provenance::Treesitter,
                    Certainty::Exact,
                ))
                .unwrap()
            };
            edge(
                &mut b,
                NodeKey::new(NodeKind::File, "a/f"),
                EdgeKind::InService,
                NodeKey::new(NodeKind::Service, "a"),
            );
            edge(
                &mut b,
                NodeKey::new(NodeKind::File, "b/g"),
                EdgeKind::InService,
                NodeKey::new(NodeKind::Service, "b"),
            );
            edge(
                &mut b,
                NodeKey::new(NodeKind::Symbol, "s"),
                EdgeKind::DefinedIn,
                NodeKey::new(NodeKind::File, "b/g"),
            );
            edge(
                &mut b,
                NodeKey::new(NodeKind::File, "a/f"),
                EdgeKind::Migrates,
                NodeKey::new(NodeKind::Table, "public.t"),
            );
            let mut q = NewEdge::new(
                NodeKey::new(NodeKind::Symbol, "s"),
                EdgeKind::Queries,
                NodeKey::new(NodeKind::Table, "public.t"),
                Provenance::SqlLiteral,
                Certainty::Heuristic,
            );
            q.attrs = [("ops".to_owned(), ops)].into_iter().collect();
            b.add_edge(q).unwrap();
            b.finish().unwrap()
        };
        // b reads t: silent. b writes t: violation. merge writes too.
        for (ops, fire) in [
            (json!(["read"]), false),
            (json!(["read", "delete"]), true),
            (json!(["merge"]), true),
        ] {
            let found = unowned_table_writes(&build(ops.clone()));
            assert_eq!(
                found,
                if fire {
                    vec![Violation {
                        rule: RULE_UNOWNED_TABLE_WRITES,
                        src: "b".into(),
                        dst: "public.t".into(),
                    }]
                } else {
                    vec![]
                },
                "{ops}"
            );
        }
        // Two writers in one service report one violation (the Cypher twin's DISTINCT agrees).
        let mut b2 = GraphBuilder::new();
        for (key, name, attrs) in [
            svc("a"),
            svc("b"),
            (NodeKey::new(NodeKind::File, "a/f"), "f", Attrs::new()),
            (NodeKey::new(NodeKind::File, "b/g"), "g", Attrs::new()),
            (NodeKey::new(NodeKind::Symbol, "s"), "s", Attrs::new()),
            (NodeKey::new(NodeKind::Symbol, "t"), "t", Attrs::new()),
            (NodeKey::new(NodeKind::Table, "public.u"), "u", Attrs::new()),
        ] {
            b2.add_node(key, name, attrs).unwrap();
        }
        let plain = |src: NodeKey, kind: EdgeKind, dst: NodeKey| {
            NewEdge::new(src, kind, dst, Provenance::Treesitter, Certainty::Exact)
        };
        for (src, kind, dst) in [
            (
                NodeKey::new(NodeKind::File, "a/f"),
                EdgeKind::InService,
                NodeKey::new(NodeKind::Service, "a"),
            ),
            (
                NodeKey::new(NodeKind::File, "b/g"),
                EdgeKind::InService,
                NodeKey::new(NodeKind::Service, "b"),
            ),
            (
                NodeKey::new(NodeKind::File, "a/f"),
                EdgeKind::Migrates,
                NodeKey::new(NodeKind::Table, "public.u"),
            ),
        ] {
            b2.add_edge(plain(src, kind, dst)).unwrap();
        }
        for sym in ["s", "t"] {
            b2.add_edge(plain(
                NodeKey::new(NodeKind::Symbol, sym),
                EdgeKind::DefinedIn,
                NodeKey::new(NodeKind::File, "b/g"),
            ))
            .unwrap();
            let mut q = NewEdge::new(
                NodeKey::new(NodeKind::Symbol, sym),
                EdgeKind::Queries,
                NodeKey::new(NodeKind::Table, "public.u"),
                Provenance::SqlLiteral,
                Certainty::Heuristic,
            );
            q.attrs = [("ops".to_owned(), json!(["insert"]))]
                .into_iter()
                .collect();
            b2.add_edge(q).unwrap();
        }
        assert_eq!(
            unowned_table_writes(&b2.finish().unwrap()),
            [Violation {
                rule: RULE_UNOWNED_TABLE_WRITES,
                src: "b".into(),
                dst: "public.u".into(),
            }]
        );
    }

    /// Contract M6h S1 (evaluator pass 1, A1): a `QUERIES` edge without an `ops` attribute
    /// never fires (both twins skip it: no provable write).
    #[test]
    fn queries_without_ops_never_fire() {
        let mut b = GraphBuilder::new();
        for (key, name, attrs) in [
            svc("a"),
            (NodeKey::new(NodeKind::File, "a/f"), "f", Attrs::new()),
            (NodeKey::new(NodeKind::Symbol, "s"), "s", Attrs::new()),
            (NodeKey::new(NodeKind::Table, "public.t"), "t", Attrs::new()),
        ] {
            b.add_node(key, name, attrs).unwrap();
        }
        let plain = |src: NodeKey, kind: EdgeKind, dst: NodeKey| {
            NewEdge::new(src, kind, dst, Provenance::Treesitter, Certainty::Exact)
        };
        b.add_edge(plain(
            NodeKey::new(NodeKind::File, "a/f"),
            EdgeKind::InService,
            NodeKey::new(NodeKind::Service, "a"),
        ))
        .unwrap();
        b.add_edge(plain(
            NodeKey::new(NodeKind::Symbol, "s"),
            EdgeKind::DefinedIn,
            NodeKey::new(NodeKind::File, "a/f"),
        ))
        .unwrap();
        b.add_edge(plain(
            NodeKey::new(NodeKind::Symbol, "s"),
            EdgeKind::Queries,
            NodeKey::new(NodeKind::Table, "public.t"),
        ))
        .unwrap();
        assert!(unowned_table_writes(&b.finish().unwrap()).is_empty());
    }

    /// Contract M6h S1: cycle links report; DAG links and self-standing services stay silent.
    #[test]
    fn service_cycles_report_links_on_cycles() {
        let mut b = GraphBuilder::new();
        for key in ["a", "b", "c", "d"] {
            b.add_node(NodeKey::new(NodeKind::Service, key), key, Attrs::new())
                .unwrap();
        }
        let mut link = |a: &str, c: &str| {
            b.add_edge(NewEdge::new(
                NodeKey::new(NodeKind::Service, a),
                EdgeKind::DependsOn,
                NodeKey::new(NodeKind::Service, c),
                Provenance::Manifest,
                Certainty::Exact,
            ))
            .unwrap()
        };
        link("a", "b");
        link("b", "a");
        link("b", "c");
        link("d", "d");
        let found = service_cycles(&b.finish().unwrap());
        assert_eq!(
            found,
            [
                Violation {
                    rule: RULE_SERVICE_CYCLES,
                    src: "a".into(),
                    dst: "b".into()
                },
                Violation {
                    rule: RULE_SERVICE_CYCLES,
                    src: "b".into(),
                    dst: "a".into()
                },
                Violation {
                    rule: RULE_SERVICE_CYCLES,
                    src: "d".into(),
                    dst: "d".into()
                },
            ]
        );
    }

    /// Contract M6h S1/S4: every rule query is a single read-only MATCH: no write clause, and an
    /// ORDER BY over its two returned keys. Quoted literals (like the `'merge'` op) are not
    /// clauses, so they are stripped before the check.
    #[test]
    fn rule_queries_are_single_read_only_matches() {
        fn clauses(cypher: &str) -> String {
            let mut out = String::new();
            let mut quoted = false;
            for c in cypher.chars() {
                if c == '\'' {
                    quoted = !quoted;
                } else if !quoted {
                    out.push(c);
                }
            }
            out
        }
        assert_eq!(RULES.len(), 3);
        for rule in RULES {
            let upper = rule.cypher.to_ascii_uppercase();
            assert!(
                upper.trim_start().starts_with("MATCH "),
                "{}, not a single MATCH",
                rule.name
            );
            let body = clauses(&upper);
            for banned in [
                "CREATE", "MERGE", "DELETE", "DETACH", "REMOVE", "SET ", "DROP", "LOAD ",
            ] {
                assert!(
                    !body.contains(banned),
                    "{}, write clause {banned:?}",
                    rule.name
                );
            }
            assert!(upper.contains("ORDER BY"), "{}, no ORDER BY", rule.name);
        }
        // Multi-match rules dedupe their returned rows: the twin compares exact row sets
        // (evaluator pass 1). The COLLECT(DISTINCT …) alone does not dedupe rows.
        assert!(
            RULES
                .iter()
                .find(|r| r.name == RULE_UNOWNED_TABLE_WRITES)
                .is_some_and(|r| r.cypher.contains("RETURN DISTINCT")),
            "unowned_table_writes dedupes its rows"
        );
    }

    /// Contract M6h S1, invariants 5–6: checking 4× the services costs at most 5× the operations.
    #[test]
    fn checking_grows_linearly() {
        fn work(n: usize) -> u64 {
            let mut b = GraphBuilder::new();
            for i in 0..n {
                let s = format!("s{i}");
                b.add_node(NodeKey::new(NodeKind::Service, &s), &s, Attrs::new())
                    .unwrap();
                let e = format!("HTTP GET /{i} @s{i}");
                let mut attrs = Attrs::new();
                attrs.insert("service".into(), json!(s));
                b.add_node(NodeKey::new(NodeKind::Endpoint, &e), &e, attrs)
                    .unwrap();
                b.add_edge(NewEdge::new(
                    NodeKey::new(NodeKind::Service, &s),
                    EdgeKind::Exposes,
                    NodeKey::new(NodeKind::Endpoint, &e),
                    Provenance::Treesitter,
                    Certainty::Exact,
                ))
                .unwrap();
            }
            let g = b.finish().unwrap();
            crate::work::take();
            let found = check(&g);
            assert_eq!(found.len(), n);
            crate::work::take()
        }
        let (small, large) = (work(50), work(200));
        assert!(small > 0 && large <= small * 5, "{small} -> {large}");
    }
}
