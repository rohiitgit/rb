//! The Service Card's derived sections (design §9.5, contract M4b S17, S18, D6): packages with
//! versions, database tables, service dependencies `via=calls|http|package` and their inverse,
//! shared tables (coupling, kept apart from dependencies), and a call-graph summary with entry
//! points by kind. All computed at query time from the stored edges; nothing here is stored.
//!
//! Two passes over the edges and one over the nodes per card (linear); every list is sorted
//! bytewise by key. A service never depends on itself: each via counts only edges that leave
//! (or enter) it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rb_model::{Certainty, Edge, EdgeKind, NodeKind, Provenance};
use serde::Serialize;

use crate::code::{SnapshotIndex, endpoint_display, entry_kinds};

/// A package the service depends on (`Service -DEPENDS_ON-> Package`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PackageUse {
    pub key: String,
    pub name: String,
    /// Empty when unresolved (`@?`).
    pub version: String,
    /// The `manifest` edge's `version_from` (`lockfile`, `literal`, `property`, `go_mod`,
    /// `unresolved`); `None` with only a `scip` edge.
    pub version_from: Option<String>,
    /// The edges' provenances (`manifest`, `scip`), sorted.
    pub provenance: Vec<String>,
}

/// A table the service's code queries or maps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableUse {
    pub key: String,
    /// The union of the `QUERIES` operations, sorted; `MAPS_TO` adds none.
    pub ops: Vec<String>,
    pub certainty: String,
    pub schema_assumed: bool,
}

/// A dependency between two services by one means.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceDep {
    pub key: String,
    pub name: String,
    /// `calls`, `http` or `package`.
    pub via: String,
    /// The best certainty of the edges behind it.
    pub certainty: String,
    /// `via=calls`: call and reference sites (evidence rows).
    pub calls: usize,
    pub references: usize,
    /// `via=http`: the endpoints, by key.
    pub endpoints: Vec<EndpointRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct EndpointRef {
    pub key: String,
    /// The name, with the router of a prefix-unknown endpoint.
    pub display: String,
}

/// Another service touching a table this one touches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SharedTables {
    pub key: String,
    pub name: String,
    pub tables: Vec<String>,
}

/// Edges with both ends in the service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallGraph {
    pub symbols: usize,
    /// Per kind (`CALLS`, `REFERENCES`, `IMPLEMENTS`, `INJECTS`), zeros included.
    pub edges: BTreeMap<String, usize>,
    /// Edges per certainty, zeros omitted.
    pub certainty: BTreeMap<String, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CardSections {
    pub packages: Vec<PackageUse>,
    pub tables: Vec<TableUse>,
    pub dependencies: Vec<ServiceDep>,
    pub depended_on_by: Vec<ServiceDep>,
    pub shares_tables: Vec<SharedTables>,
    pub call_graph: CallGraph,
    /// Entry points by kind: stored kinds, `endpoint` (handlers), `top-level` (files whose
    /// top-level code calls, references, queries or consumes).
    pub entry_points: BTreeMap<String, usize>,
}

const SUMMARY_KINDS: &[EdgeKind] = &[
    EdgeKind::Calls,
    EdgeKind::References,
    EdgeKind::Implements,
    EdgeKind::Injects,
];

/// Accumulates one dependency.
#[derive(Default)]
struct Acc {
    best: Option<Certainty>,
    calls: usize,
    references: usize,
    endpoints: BTreeSet<EndpointRef>,
}

impl Acc {
    fn add(&mut self, c: Certainty) {
        self.best = Some(self.best.map_or(c, |b| b.max(c)));
    }
}

/// The derived sections of service `key`'s card.
pub fn card_sections(ix: &SnapshotIndex, key: &str) -> CardSections {
    let graph = ix.graph();
    // Each file's service, each node's file.
    let mut service_of_file: HashMap<i64, i64> = HashMap::new();
    let mut file_of: HashMap<i64, i64> = HashMap::new();
    let mut exposed_by: HashMap<i64, i64> = HashMap::new();
    for e in graph.edges() {
        crate::work::tick(1);
        match e.kind {
            EdgeKind::InService => {
                service_of_file.insert(e.src, e.dst);
            }
            EdgeKind::DefinedIn => {
                file_of.entry(e.src).or_insert(e.dst);
            }
            EdgeKind::Exposes => {
                exposed_by.insert(e.dst, e.src);
            }
            _ => {}
        }
    }
    let service_of = |n: i64| -> Option<i64> {
        match ix.get_node(n)?.kind {
            NodeKind::File => service_of_file.get(&n).copied(),
            NodeKind::Service => Some(n),
            NodeKind::Endpoint => exposed_by.get(&n).copied(),
            _ => service_of_file.get(file_of.get(&n)?).copied(),
        }
    };
    let Some(me) = graph
        .nodes()
        .iter()
        .find(|n| n.kind == NodeKind::Service && n.key == key)
        .map(|n| n.id)
    else {
        return CardSections {
            packages: Vec::new(),
            tables: Vec::new(),
            dependencies: Vec::new(),
            depended_on_by: Vec::new(),
            shares_tables: Vec::new(),
            call_graph: CallGraph {
                symbols: 0,
                edges: BTreeMap::new(),
                certainty: BTreeMap::new(),
            },
            entry_points: BTreeMap::new(),
        };
    };
    let mine = |n: i64| service_of(n) == Some(me);

    let mut packages: BTreeMap<i64, (BTreeSet<String>, Option<String>)> = BTreeMap::new();
    let mut tables: BTreeMap<i64, (BTreeSet<String>, Certainty, bool)> = BTreeMap::new();
    // Every service's tables, for coupling.
    let mut table_users: BTreeMap<i64, BTreeSet<i64>> = BTreeMap::new();
    let mut out: BTreeMap<(i64, &str), Acc> = BTreeMap::new();
    let mut inn: BTreeMap<(i64, &str), Acc> = BTreeMap::new();
    let mut edges: BTreeMap<String, usize> = SUMMARY_KINDS
        .iter()
        .map(|k| (k.as_str().to_owned(), 0))
        .collect();
    let mut certainty: BTreeMap<Certainty, usize> = BTreeMap::new();
    let mut handlers: BTreeSet<i64> = BTreeSet::new();
    let mut top_level: BTreeSet<i64> = BTreeSet::new();
    let attr_str = |e: &Edge, k: &str| e.attrs.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    for e in graph.edges() {
        crate::work::tick(1);
        let (src_mine, dst_mine) = (mine(e.src), mine(e.dst));
        match e.kind {
            EdgeKind::DependsOn if e.src == me => match ix.node(e.dst).kind {
                NodeKind::Package => {
                    let entry = packages.entry(e.dst).or_default();
                    entry.0.insert(e.provenance.as_str().to_owned());
                    if e.provenance == Provenance::Manifest {
                        entry.1 = attr_str(e, "version_from");
                    }
                }
                NodeKind::Service if e.dst != me => {
                    out.entry((e.dst, "package")).or_default().add(e.certainty);
                }
                _ => {}
            },
            EdgeKind::DependsOn if e.dst == me && ix.node(e.src).kind == NodeKind::Service => {
                inn.entry((e.src, "package")).or_default().add(e.certainty);
            }
            EdgeKind::Queries | EdgeKind::MapsTo => {
                if let Some(s) = service_of(e.src) {
                    table_users.entry(e.dst).or_default().insert(s);
                }
                if src_mine {
                    let ops = e.attrs.get("ops").and_then(|o| o.as_array());
                    let assumed = e.attrs.get("schema_assumed").and_then(|a| a.as_bool());
                    let entry =
                        tables
                            .entry(e.dst)
                            .or_insert((BTreeSet::new(), e.certainty, false));
                    entry.0.extend(
                        ops.into_iter()
                            .flatten()
                            .filter_map(|o| o.as_str().map(str::to_owned)),
                    );
                    entry.1 = entry.1.max(e.certainty);
                    entry.2 |= assumed == Some(true);
                }
                if src_mine && ix.node(e.src).kind == NodeKind::File {
                    top_level.insert(e.src);
                }
            }
            EdgeKind::Calls | EdgeKind::References => {
                let sites = e.evidence.len().max(1);
                let count = |acc: &mut Acc| {
                    acc.add(e.certainty);
                    if e.kind == EdgeKind::Calls {
                        acc.calls += sites;
                    } else {
                        acc.references += sites;
                    }
                };
                match (src_mine, dst_mine) {
                    (true, false) => {
                        if let Some(t) = service_of(e.dst) {
                            count(out.entry((t, "calls")).or_default());
                        }
                    }
                    (false, true) => {
                        if let Some(s) = service_of(e.src) {
                            count(inn.entry((s, "calls")).or_default());
                        }
                    }
                    _ => {}
                }
                if src_mine && ix.node(e.src).kind == NodeKind::File {
                    top_level.insert(e.src);
                }
            }
            EdgeKind::Consumes => {
                let target = service_of(e.dst);
                if src_mine && ix.node(e.src).kind == NodeKind::File {
                    top_level.insert(e.src);
                }
                match (src_mine, target) {
                    (true, Some(t)) if t != me => {
                        let acc = out.entry((t, "http")).or_default();
                        acc.add(e.certainty);
                        acc.endpoints.insert(endpoint_ref(ix.node(e.dst)));
                    }
                    (false, Some(t)) if t == me => {
                        if let Some(s) = service_of(e.src) {
                            let acc = inn.entry((s, "http")).or_default();
                            acc.add(e.certainty);
                            acc.endpoints.insert(endpoint_ref(ix.node(e.dst)));
                        }
                    }
                    _ => {}
                }
            }
            EdgeKind::HandledBy if dst_mine => {
                handlers.insert(e.dst);
            }
            _ => {}
        }
        if src_mine && dst_mine && SUMMARY_KINDS.contains(&e.kind) {
            *edges.get_mut(e.kind.as_str()).expect("a summary kind") += 1;
            *certainty.entry(e.certainty).or_default() += 1;
        }
    }

    // Symbols and stored entry kinds.
    let mut symbols = 0;
    let mut entry_points: BTreeMap<String, usize> = BTreeMap::new();
    for n in graph.nodes() {
        crate::work::tick(1);
        if n.kind == NodeKind::Symbol && mine(n.id) {
            symbols += 1;
            for k in entry_kinds(n) {
                *entry_points.entry(k.to_owned()).or_default() += 1;
            }
        }
    }
    if !handlers.is_empty() {
        entry_points.insert("endpoint".into(), handlers.len());
    }
    if !top_level.is_empty() {
        entry_points.insert("top-level".into(), top_level.len());
    }

    let deps = |m: BTreeMap<(i64, &str), Acc>| -> Vec<ServiceDep> {
        let mut v: Vec<ServiceDep> = m
            .into_iter()
            .map(|((s, via), acc)| ServiceDep {
                key: ix.node(s).key.clone(),
                name: ix.node(s).name.clone(),
                via: via.to_owned(),
                certainty: acc.best.unwrap_or(Certainty::Ambiguous).as_str().to_owned(),
                calls: acc.calls,
                references: acc.references,
                endpoints: acc.endpoints.into_iter().collect(),
            })
            .collect();
        v.sort_by(|a, b| (&a.key, &a.via).cmp(&(&b.key, &b.via)));
        v
    };
    let mut shared: BTreeMap<i64, BTreeSet<String>> = BTreeMap::new();
    for t in tables.keys() {
        for &s in table_users.get(t).into_iter().flatten() {
            if s != me {
                shared.entry(s).or_default().insert(ix.node(*t).key.clone());
            }
        }
    }
    let mut shares_tables: Vec<SharedTables> = shared
        .into_iter()
        .map(|(s, tables)| SharedTables {
            key: ix.node(s).key.clone(),
            name: ix.node(s).name.clone(),
            tables: tables.into_iter().collect(),
        })
        .collect();
    shares_tables.sort_by(|a, b| a.key.cmp(&b.key));
    let mut packages: Vec<PackageUse> = packages
        .into_iter()
        .map(|(p, (provenance, version_from))| {
            let node = ix.node(p);
            let attr = |k: &str| {
                node.attrs
                    .get(k)
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned()
            };
            PackageUse {
                key: node.key.clone(),
                name: attr("name"),
                version: attr("version"),
                version_from,
                provenance: provenance.into_iter().collect(),
            }
        })
        .collect();
    packages.sort_by(|a, b| a.key.cmp(&b.key));
    let mut tables: Vec<TableUse> = tables
        .into_iter()
        .map(|(t, (ops, c, assumed))| TableUse {
            key: ix.node(t).key.clone(),
            ops: ops.into_iter().collect(),
            certainty: c.as_str().to_owned(),
            schema_assumed: assumed,
        })
        .collect();
    tables.sort_by(|a, b| a.key.cmp(&b.key));
    CardSections {
        packages,
        tables,
        dependencies: deps(out),
        depended_on_by: deps(inn),
        shares_tables,
        call_graph: CallGraph {
            symbols,
            edges,
            certainty: certainty
                .into_iter()
                .map(|(c, n)| (c.as_str().to_owned(), n))
                .collect(),
        },
        entry_points,
    }
}

fn endpoint_ref(node: &rb_model::Node) -> EndpointRef {
    EndpointRef {
        key: node.key.clone(),
        display: endpoint_display(node),
    }
}

#[cfg(test)]
mod tests {
    use rb_model::{Attrs, Evidence, GraphBuilder, NewEdge, NodeKey};
    use serde_json::json;

    use super::*;

    struct G {
        b: GraphBuilder,
    }

    impl G {
        fn new(services: &[&str]) -> Self {
            let mut b = GraphBuilder::new();
            for s in services {
                b.add_node(NodeKey::new(NodeKind::Service, *s), *s, Attrs::new())
                    .unwrap();
            }
            G { b }
        }

        fn file(&mut self, path: &str) -> NodeKey {
            let f = NodeKey::new(NodeKind::File, path);
            if !self.b.has_node(&f) {
                self.b.add_node(f.clone(), path, Attrs::new()).unwrap();
                let s = NodeKey::new(NodeKind::Service, path.split('/').next().unwrap());
                self.b
                    .add_edge(NewEdge::new(
                        f.clone(),
                        EdgeKind::InService,
                        s,
                        Provenance::Manifest,
                        Certainty::Exact,
                    ))
                    .unwrap();
            }
            f
        }

        fn sym(&mut self, path: &str, name: &str, attrs: serde_json::Value) -> NodeKey {
            let key = NodeKey::new(NodeKind::Symbol, format!("{path}#{name}"));
            self.b
                .add_node(key.clone(), name, attrs.as_object().cloned().unwrap())
                .unwrap();
            let file = self.file(path);
            self.b
                .add_edge(NewEdge::new(
                    key.clone(),
                    EdgeKind::DefinedIn,
                    file,
                    Provenance::Treesitter,
                    Certainty::Exact,
                ))
                .unwrap();
            key
        }

        fn node(&mut self, kind: NodeKind, key: &str, attrs: serde_json::Value) -> NodeKey {
            let k = NodeKey::new(kind, key);
            self.b
                .add_node(k.clone(), key, attrs.as_object().cloned().unwrap())
                .unwrap();
            k
        }

        #[allow(clippy::too_many_arguments)]
        fn edge(
            &mut self,
            src: &NodeKey,
            kind: EdgeKind,
            dst: &NodeKey,
            p: Provenance,
            c: Certainty,
            attrs: serde_json::Value,
            sites: i32,
        ) {
            let mut e = NewEdge::new(src.clone(), kind, dst.clone(), p, c)
                .attrs(attrs.as_object().cloned().unwrap());
            for i in 0..sites {
                // The evidence lies in the file of the end that is code, else the manifest.
                let path = [src, dst]
                    .into_iter()
                    .find(|k| matches!(k.kind, NodeKind::Symbol | NodeKind::File))
                    .map_or_else(
                        || "a/pom.xml".to_owned(),
                        |k| k.key.split('#').next().unwrap().to_owned(),
                    );
                self.file(&path);
                e = e.evidence(Evidence {
                    path,
                    span: [i, 0, i, 1],
                });
            }
            self.b.add_edge(e).unwrap();
        }
    }

    /// Contract M4b S17/S18 on a built graph: two vias between one pair, a self-call excluded,
    /// a library dependency by package, the inverse, shared tables, the tables, the call graph
    /// and entry points.
    #[test]
    fn dependencies_tables_and_call_graph() {
        use Certainty::{Exact, Heuristic, Synthetic};
        use EdgeKind::{Calls, Consumes, DependsOn, HandledBy, Implements, Queries, References};
        use Provenance::{Heuristic as H, HttpStitch, Manifest, Scip, SqlLiteral, Treesitter};
        let mut g = G::new(&["a", "b", "lib"]);
        let a1 = g.sym("a/x.ts", "a1", json!({"entry": ["main"]}));
        let a2 = g.sym("a/x.ts", "a2", json!({}));
        let b1 = g.sym("b/y.ts", "b1", json!({}));
        let l1 = g.sym("lib/z.ts", "l1", json!({}));
        let none = json!({});
        // a → a (self), a → b by calls (2 sites) and by a reference, a → lib by calls.
        g.edge(&a1, Calls, &a2, H, Exact, none.clone(), 1);
        g.edge(&a1, Implements, &a2, H, Heuristic, none.clone(), 1);
        g.edge(&a1, Calls, &b1, H, Heuristic, none.clone(), 2);
        g.edge(&a2, References, &b1, H, Synthetic, none.clone(), 1);
        g.edge(&a2, Calls, &l1, H, Exact, none.clone(), 1);
        // a → b by http too; b → a by http (depended on by).
        let eb = g.node(
            NodeKind::Endpoint,
            "HTTP GET /b @b",
            json!({"service": "b"}),
        );
        let ea = g.node(
            NodeKind::Endpoint,
            "HTTP GET /a @a",
            json!({"service": "a"}),
        );
        let (sa, sb, sl) = (
            NodeKey::new(NodeKind::Service, "a"),
            NodeKey::new(NodeKind::Service, "b"),
            NodeKey::new(NodeKind::Service, "lib"),
        );
        g.edge(
            &sb,
            EdgeKind::Exposes,
            &eb,
            Treesitter,
            Exact,
            none.clone(),
            0,
        );
        g.edge(
            &sa,
            EdgeKind::Exposes,
            &ea,
            Treesitter,
            Exact,
            none.clone(),
            0,
        );
        g.edge(&a1, Consumes, &eb, HttpStitch, Synthetic, none.clone(), 1);
        g.edge(&b1, Consumes, &ea, HttpStitch, Heuristic, none.clone(), 1);
        g.edge(&ea, HandledBy, &a2, Treesitter, Exact, none.clone(), 1);
        // A self-call over HTTP and a manifest naming its own service are no dependencies.
        g.edge(&a2, Consumes, &ea, HttpStitch, Heuristic, none.clone(), 1);
        // a → lib by package; packages from a manifest and from SCIP.
        g.edge(
            &sa,
            DependsOn,
            &sl,
            Manifest,
            Synthetic,
            json!({"via": "package"}),
            1,
        );
        g.edge(
            &sa,
            DependsOn,
            &sa,
            Manifest,
            Synthetic,
            json!({"via": "package"}),
            1,
        );
        let p = g.node(
            NodeKind::Package,
            "npm/x@1.0.0",
            json!({"name": "x", "version": "1.0.0"}),
        );
        g.edge(
            &sa,
            DependsOn,
            &p,
            Manifest,
            Exact,
            json!({"version_from": "lockfile"}),
            1,
        );
        g.edge(&sa, DependsOn, &p, Scip, Exact, none.clone(), 1);
        // Tables: a and lib query `t`; b queries `u` only; a's file top-level code queries.
        let t = g.node(NodeKind::Table, "public.t", json!({}));
        let u = g.node(NodeKind::Table, "public.u", json!({}));
        g.edge(
            &a2,
            Queries,
            &t,
            SqlLiteral,
            Exact,
            json!({"ops": ["read"], "schema_assumed": true}),
            1,
        );
        let file = g.file("a/x.ts");
        g.edge(
            &file,
            Queries,
            &t,
            SqlLiteral,
            Heuristic,
            json!({"ops": ["insert"]}),
            1,
        );
        g.edge(
            &l1,
            Queries,
            &t,
            SqlLiteral,
            Exact,
            json!({"ops": ["read"]}),
            1,
        );
        g.edge(
            &b1,
            Queries,
            &u,
            SqlLiteral,
            Exact,
            json!({"ops": ["read"]}),
            1,
        );
        let graph = g.b.finish().unwrap();

        let c = card_sections(&crate::code::SnapshotIndex::new(graph.clone()), "a");
        let deps: Vec<String> = c
            .dependencies
            .iter()
            .map(|d| {
                let eps: Vec<&str> = d.endpoints.iter().map(|e| e.key.as_str()).collect();
                format!(
                    "{} {} [{}] {}/{} {eps:?}",
                    d.key, d.via, d.certainty, d.calls, d.references
                )
            })
            .collect();
        assert_eq!(
            deps,
            [
                "b calls [synthetic] 2/1 []",
                "b http [synthetic] 0/0 [\"HTTP GET /b @b\"]",
                "lib calls [exact] 1/0 []",
                "lib package [synthetic] 0/0 []",
            ]
        );
        let by: Vec<String> = c
            .depended_on_by
            .iter()
            .map(|d| format!("{} {} [{}]", d.key, d.via, d.certainty))
            .collect();
        assert_eq!(by, ["b http [heuristic]"]);
        assert_eq!(c.shares_tables.len(), 1);
        assert_eq!(
            (
                c.shares_tables[0].key.as_str(),
                &c.shares_tables[0].tables[..]
            ),
            ("lib", &["public.t".to_owned()][..])
        );
        assert_eq!(
            c.tables,
            [TableUse {
                key: "public.t".into(),
                ops: vec!["insert".into(), "read".into()],
                certainty: "exact".into(),
                schema_assumed: true,
            }]
        );
        assert_eq!(c.packages.len(), 1);
        assert_eq!(c.packages[0].version_from.as_deref(), Some("lockfile"));
        assert_eq!(c.packages[0].provenance, ["manifest", "scip"]);
        assert_eq!(c.call_graph.symbols, 2);
        assert_eq!(c.call_graph.edges["CALLS"], 1);
        assert_eq!(c.call_graph.edges["IMPLEMENTS"], 1);
        assert_eq!(c.call_graph.edges["INJECTS"], 0);
        assert_eq!(
            c.call_graph.certainty,
            BTreeMap::from([("exact".to_owned(), 1), ("heuristic".to_owned(), 1)])
        );
        assert_eq!(
            c.entry_points,
            BTreeMap::from([
                ("endpoint".to_owned(), 1),
                ("main".to_owned(), 1),
                ("top-level".to_owned(), 1)
            ])
        );
        // A library seen from its side: depended on by `a` by calls and by package.
        let l = card_sections(&crate::code::SnapshotIndex::new(graph.clone()), "lib");
        let by: Vec<String> = l
            .depended_on_by
            .iter()
            .map(|d| format!("{} {}", d.key, d.via))
            .collect();
        assert_eq!(by, ["a calls", "a package"]);
        assert!(l.dependencies.is_empty());
    }

    /// Invariant 9: the card's aggregation is one pass over the edges (counted, 4× → ≤ 5×).
    #[test]
    fn card_aggregation_grows_linearly() {
        let count = |n: usize| {
            let mut g = G::new(&["a", "b"]);
            let b1 = g.sym("b/y.ts", "b1", json!({}));
            for i in 0..n {
                let s = g.sym("a/x.ts", &format!("s{i}"), json!({}));
                g.edge(
                    &s,
                    EdgeKind::Calls,
                    &b1,
                    Provenance::Heuristic,
                    Certainty::Heuristic,
                    json!({}),
                    1,
                );
            }
            let graph = g.b.finish().unwrap();
            crate::work::take();
            let c = card_sections(&crate::code::SnapshotIndex::new(graph.clone()), "a");
            assert_eq!(c.dependencies[0].calls, n);
            crate::work::take()
        };
        let (small, large) = (count(200), count(800));
        assert!(large <= small * 5, "{small} -> {large}");
    }
}
