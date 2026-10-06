//! Q3's roll-up (design §9.4 steps 2–5, contract M4b S14, D12): from the code that queries a
//! table, its callers up to entry points, grouped by service, and the services that reach the
//! table only through another service's API. A pure function of the snapshot graph.
//!
//! Every walk terminates by construction: the caller walk is one breadth-first search with a
//! visited set, a depth cap and a node cap; the labels it then propagates (operations, the
//! services holding the data access, the best certainty) only grow, each within a bounded set,
//! so their worklist empties; the API hops reuse `rb path`'s upstream walk with its endpoint
//! visited set and hop cap.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use rb_model::{Certainty, EdgeKind, Evidence, NodeKind};
use serde::Serialize;

use crate::code::{
    MAX_CALLER_DEPTH, Precision, SnapshotIndex, View, endpoint_display, entry_kinds,
};
use crate::path::{self, Caps, Limit, PathNode, PathOptions, Via};

/// Default and maximum API hops (D12).
pub const DEFAULT_HOPS: usize = path::DEFAULT_HOPS;
pub const MAX_HOPS: usize = path::MAX_HOPS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RollupOptions {
    /// Dispatch expansion (D5: `cha` by default for Q3).
    pub precision: Precision,
    /// API hops followed through `CONSUMES` (0: none).
    pub hops: usize,
    /// `nodes`, `work`, and `tree` for the API hops' walks (contract M5 D6).
    pub caps: Caps,
}

/// A service whose code reaches the table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ServiceReach {
    pub key: String,
    pub name: String,
    /// The union of the operations its code reaches, sorted.
    pub ops: Vec<String>,
    /// It holds code that queries the table itself.
    pub direct: bool,
    /// When not direct: the services whose code it reaches the table through, by key.
    pub via: Vec<String>,
    /// Endpoints handled by its reached code, by key.
    pub endpoints: Vec<EndpointReach>,
    /// Its other entry points reached, by kind (`top-level` for files).
    pub entry_points: BTreeMap<String, usize>,
    /// The best path certainty (each path's weakest step).
    pub certainty: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EndpointReach {
    pub key: String,
    /// The name, with the router of a prefix-unknown endpoint.
    pub display: String,
    pub certainty: String,
}

/// A service reaching the table only through other services' endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiReach {
    pub key: String,
    pub name: String,
    /// The endpoints its requests go through, nearest first, the last one reaching the table.
    pub chain: Vec<ChainStep>,
    pub certainty: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ChainStep {
    pub endpoint: String,
    pub display: String,
    pub service_key: String,
    pub service_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableRollup {
    pub precision: String,
    /// Dispatch steps skipped because their implementing type is never instantiated.
    pub pruned_by_rta: usize,
    pub services: Vec<ServiceReach>,
    pub through_apis: Vec<ApiReach>,
    /// The limits that cut the answer (`depth`, `hops`, `nodes`, `tree`, `work`).
    pub truncated: Vec<String>,
    /// Counted operations the question spent (contract M5 D6); not part of the JSON.
    #[serde(skip)]
    pub work: u64,
}

/// What a node reaches of the table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Label {
    ops: BTreeSet<String>,
    /// Services holding the queries it reaches.
    services: BTreeSet<i64>,
    best: Option<Certainty>,
}

impl Label {
    /// Merges what `other` reaches through a step of certainty `step`; whether it grew.
    fn absorb(&mut self, other: &Label, step: Certainty) -> bool {
        let before = (self.ops.len(), self.services.len(), self.best);
        self.ops.extend(other.ops.iter().cloned());
        self.services.extend(other.services.iter().copied());
        if let Some(b) = other.best {
            let c = b.min(step);
            self.best = Some(self.best.map_or(c, |x| x.max(c)));
        }
        before != (self.ops.len(), self.services.len(), self.best)
    }
}

/// One caller step found by the walk: `src` calls (or references, or may dispatch to) `dst`.
struct Step<'g> {
    src: i64,
    dst: i64,
    certainty: Certainty,
    evidence: &'g [Evidence],
}

/// Q3's roll-up for `table` (S14), under `opts.caps` (contract M5 S10).
pub fn table_rollup(ix: &SnapshotIndex, table: i64, opts: RollupOptions) -> TableRollup {
    let (mut rollup, spent) = crate::work::metered(opts.caps.work, || rollup_walk(ix, table, opts));
    if spent.refused && !rollup.truncated.iter().any(|l| l == Limit::Work.as_str()) {
        rollup.truncated.push(Limit::Work.as_str().to_owned());
        rollup.truncated.sort();
    }
    rollup.work = spent.work;
    rollup
}

fn rollup_walk(ix: &SnapshotIndex, table: i64, opts: RollupOptions) -> TableRollup {
    let v = View::with_precision(ix, opts.precision);
    let any = Certainty::Ambiguous;
    let mut truncated: BTreeSet<Limit> = BTreeSet::new();

    // Data-access nodes and what each query does.
    let mut labels: HashMap<i64, Label> = HashMap::new();
    let mut access_evidence: HashMap<i64, Vec<(Certainty, &[Evidence])>> = HashMap::new();
    for e in v.inbound(table, EdgeKind::Queries, any) {
        let label = labels.entry(e.src).or_default();
        let ops = e.attrs.get("ops").and_then(serde_json::Value::as_array);
        label.ops.extend(
            ops.into_iter()
                .flatten()
                .filter_map(|o| o.as_str().map(str::to_owned)),
        );
        if let Some(s) = service_of(&v, e.src) {
            label.services.insert(s);
        }
        label.best = Some(label.best.map_or(e.certainty, |b| b.max(e.certainty)));
        access_evidence
            .entry(e.src)
            .or_default()
            .push((e.certainty, e.evidence.as_slice()));
    }
    let accesses: BTreeSet<i64> = labels.keys().copied().collect();

    // Callers up to entry points: breadth-first, depth and node caps.
    let mut reached: HashSet<i64> = accesses.iter().copied().collect();
    let mut steps: Vec<Step<'_>> = Vec::new();
    let mut frontier: Vec<i64> = accesses.iter().copied().collect();
    let mut depth = 0;
    while !frontier.is_empty() {
        if depth == MAX_CALLER_DEPTH {
            if frontier
                .iter()
                .any(|&n| !v.callers_of(n, true, any).is_empty())
            {
                truncated.insert(Limit::Depth);
            }
            break;
        }
        depth += 1;
        let mut next = Vec::new();
        'frontier: for &n in &frontier {
            for (src, certainty, _, _, evidence) in v.callers_of(n, true, any) {
                if !reached.contains(&src) {
                    if reached.len() >= opts.caps.nodes {
                        truncated.insert(Limit::Nodes);
                        break 'frontier;
                    }
                    reached.insert(src);
                    next.push(src);
                }
                steps.push(Step {
                    src,
                    dst: n,
                    certainty,
                    evidence,
                });
            }
        }
        frontier = next;
    }

    // Labels flow from callee to caller until nothing grows.
    let mut callers_of: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, s) in steps.iter().enumerate() {
        callers_of.entry(s.dst).or_default().push(i);
    }
    let mut queue: VecDeque<i64> = accesses.iter().copied().collect();
    'labels: while let Some(n) = queue.pop_front() {
        let Some(label) = labels.get(&n).cloned() else {
            continue;
        };
        for &i in callers_of.get(&n).into_iter().flatten() {
            if !crate::work::spend(1) {
                break 'labels;
            }
            let s = &steps[i];
            if labels.entry(s.src).or_default().absorb(&label, s.certainty) {
                queue.push_back(s.src);
            }
        }
    }

    // Per service.
    let mut by_service: BTreeMap<String, (i64, Vec<i64>)> = BTreeMap::new();
    for &n in &reached {
        if let Some(s) = service_of(&v, n) {
            by_service
                .entry(v.node(s).key.clone())
                .or_insert((s, Vec::new()))
                .1
                .push(n);
        }
    }
    // The steps leaving each node, for inline handlers' spans.
    let mut leaving: HashMap<i64, Vec<(Certainty, &[Evidence])>> = HashMap::new();
    for s in &steps {
        let c = labels
            .get(&s.dst)
            .and_then(|l| l.best)
            .map_or(s.certainty, |b| b.min(s.certainty));
        leaving.entry(s.src).or_default().push((c, s.evidence));
    }
    for (n, rows) in access_evidence {
        leaving.entry(n).or_default().extend(rows);
    }
    let mut services = Vec::new();
    let mut through_endpoints: BTreeMap<i64, Certainty> = BTreeMap::new();
    for (key, (service, nodes)) in &by_service {
        let mut ops = BTreeSet::new();
        let mut via = BTreeSet::new();
        let mut best: Option<Certainty> = None;
        let mut endpoints: BTreeMap<i64, Certainty> = BTreeMap::new();
        let mut entry_points: BTreeMap<String, usize> = BTreeMap::new();
        for &n in nodes {
            let label = labels.get(&n).cloned().unwrap_or_default();
            ops.extend(label.ops);
            via.extend(label.services.into_iter().filter(|s| s != service));
            best = best.max(label.best);
            let node = v.node(n);
            for kind in entry_kinds(node) {
                *entry_points.entry(kind.to_owned()).or_default() += 1;
            }
            if node.kind == NodeKind::File {
                *entry_points.entry("top-level".to_owned()).or_default() += 1;
            }
            for e in v.inbound(n, EdgeKind::HandledBy, any) {
                // An inline handler counts only through a step inside its spans (M4a
                // invariant 8), at that step's certainty.
                let reach = match path::handler_spans(e) {
                    Some(spans) => leaving
                        .get(&n)
                        .into_iter()
                        .flatten()
                        .filter(|(_, ev)| {
                            ev.iter()
                                .any(|x| spans.iter().any(|s| path::within(x.span, *s)))
                        })
                        .map(|(c, _)| *c)
                        .max(),
                    None => label.best,
                };
                if let Some(c) = reach {
                    let c = c.min(e.certainty);
                    let slot = endpoints.entry(e.src).or_insert(c);
                    *slot = (*slot).max(c);
                }
            }
        }
        let direct = nodes.iter().any(|n| accesses.contains(n));
        for (&e, &c) in &endpoints {
            let slot = through_endpoints.entry(e).or_insert(c);
            *slot = (*slot).max(c);
        }
        let mut endpoints: Vec<EndpointReach> = endpoints
            .into_iter()
            .map(|(e, c)| EndpointReach {
                key: v.node(e).key.clone(),
                display: endpoint_display(v.node(e)),
                certainty: c.as_str().to_owned(),
            })
            .collect();
        endpoints.sort_by(|a, b| a.key.cmp(&b.key));
        let mut via: Vec<String> = via.into_iter().map(|s| v.node(s).key.clone()).collect();
        via.sort();
        services.push(ServiceReach {
            key: key.clone(),
            name: v.node(*service).name.clone(),
            ops: ops.into_iter().collect(),
            direct,
            via: if direct { Vec::new() } else { via },
            endpoints,
            entry_points,
            certainty: best.unwrap_or(any).as_str().to_owned(),
        });
    }

    // Through APIs: the upstream continuation of `rb path` from each endpoint reaching the
    // table; a service already listed above is not repeated.
    let listed: BTreeSet<&str> = services.iter().map(|s| s.key.as_str()).collect();
    let mut through: BTreeMap<String, ApiReach> = BTreeMap::new();
    let mut pruned = 0;
    if opts.hops > 0 {
        let path_opts = PathOptions {
            depth: MAX_CALLER_DEPTH,
            hops: opts.hops - 1,
            full: true,
            precision: opts.precision,
            caps: opts.caps,
        };
        for (&e, &c) in &through_endpoints {
            let up = path::upstream_of(ix, e, path_opts);
            truncated.extend(up.truncated.iter().copied());
            pruned += up.pruned_by_rta;
            let start = vec![chain_step(&v, e)];
            collect_clients(&v, &up.nodes, &start, c, &listed, &mut through);
        }
    }
    let mut pruned_steps = v.pruned();
    pruned_steps += pruned;
    TableRollup {
        precision: opts.precision.as_str().to_owned(),
        pruned_by_rta: pruned_steps,
        services,
        through_apis: through.into_values().collect(),
        truncated: truncated.iter().map(|l| l.as_str().to_owned()).collect(),
        work: 0,
    }
}

fn service_of(v: &View<'_>, n: i64) -> Option<i64> {
    let file = v.file_of(n)?;
    v.outbound(file, EdgeKind::InService).map(|e| e.dst).next()
}

fn chain_step(v: &View<'_>, endpoint: i64) -> ChainStep {
    let node = v.node(endpoint);
    let service_key = node
        .attrs
        .get("service")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let service_name = v
        .graph()
        .nodes()
        .iter()
        .find(|n| n.kind == NodeKind::Service && n.key == service_key)
        .map_or_else(|| service_key.clone(), |n| n.name.clone());
    ChainStep {
        endpoint: node.key.clone(),
        display: endpoint_display(node),
        service_key,
        service_name,
    }
}

/// Walks an upstream tree: each client site (`via = consumes`) is a request into the
/// endpoint above it; an endpoint the walk entered (`via = handles`) extends the chain for
/// the clients above it. Keeps, per service not otherwise listed, the best certainty, then
/// the shortest chain, then the chain's keys bytewise.
fn collect_clients(
    v: &View<'_>,
    nodes: &[PathNode],
    chain: &[ChainStep],
    reach: Certainty,
    listed: &BTreeSet<&str>,
    out: &mut BTreeMap<String, ApiReach>,
) {
    for n in nodes {
        if !crate::work::spend(1) {
            return;
        }
        match n.via {
            Via::Consumes => {
                if let Some(s) = service_of(v, n.node) {
                    let key = v.node(s).key.clone();
                    if !listed.contains(key.as_str()) {
                        let mut steps = chain.to_vec();
                        steps.reverse();
                        let found = ApiReach {
                            key: key.clone(),
                            name: v.node(s).name.clone(),
                            chain: steps,
                            certainty: reach.min(n.branch).as_str().to_owned(),
                        };
                        let rank = |a: &ApiReach| {
                            (
                                std::cmp::Reverse(rank_of(&a.certainty)),
                                a.chain.len(),
                                a.chain.clone(),
                            )
                        };
                        match out.get(&key) {
                            Some(old) if rank(old) <= rank(&found) => {}
                            _ => {
                                out.insert(key, found);
                            }
                        }
                    }
                }
                collect_clients(v, &n.children, chain, reach, listed, out);
            }
            Via::Handles => {
                let mut longer = chain.to_vec();
                longer.push(chain_step(v, n.node));
                collect_clients(v, &n.children, &longer, reach, listed, out);
            }
            _ => collect_clients(v, &n.children, chain, reach, listed, out),
        }
    }
}

fn rank_of(c: &str) -> Certainty {
    [
        Certainty::Ambiguous,
        Certainty::Heuristic,
        Certainty::Synthetic,
        Certainty::Dispatch,
        Certainty::Exact,
    ]
    .into_iter()
    .find(|x| x.as_str() == c)
    .unwrap_or(Certainty::Ambiguous)
}

#[cfg(test)]
mod tests {
    use rb_model::{Attrs, GraphBuilder, NewEdge, NodeKey, Provenance};
    use serde_json::json;

    use super::*;

    /// The index of a test graph.
    fn ix(g: &Graph) -> SnapshotIndex {
        SnapshotIndex::new(g.clone())
    }
    use rb_model::Graph;

    /// Files belong to the service named by their first path segment; symbols are
    /// `<path>#<name>`; endpoints `HTTP <name> @<service>`.
    struct G {
        b: GraphBuilder,
    }

    impl G {
        fn new(services: &[(&str, &str)]) -> Self {
            let mut b = GraphBuilder::new();
            for (key, role) in services {
                let mut attrs = Attrs::new();
                attrs.insert("role".into(), (*role).into());
                b.add_node(NodeKey::new(NodeKind::Service, *key), *key, attrs)
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

        fn sym_with(&mut self, path: &str, name: &str, attrs: Attrs) -> NodeKey {
            let key = NodeKey::new(NodeKind::Symbol, format!("{path}#{name}"));
            if !self.b.has_node(&key) {
                self.b.add_node(key.clone(), name, attrs).unwrap();
                let file = self.file(path);
                self.b
                    .add_edge(
                        NewEdge::new(
                            key.clone(),
                            EdgeKind::DefinedIn,
                            file,
                            Provenance::Treesitter,
                            Certainty::Exact,
                        )
                        .evidence(Evidence {
                            path: path.into(),
                            span: [0, 0, 0, 1],
                        }),
                    )
                    .unwrap();
            }
            key
        }

        fn sym(&mut self, path: &str, name: &str) -> NodeKey {
            self.sym_with(path, name, Attrs::new())
        }

        fn endpoint(&mut self, name: &str, service: &str) -> NodeKey {
            let key = NodeKey::new(NodeKind::Endpoint, format!("HTTP {name} @{service}"));
            if !self.b.has_node(&key) {
                let attrs = json!({"service": service}).as_object().cloned().unwrap();
                self.b.add_node(key.clone(), name, attrs).unwrap();
            }
            key
        }

        fn table(&mut self) -> NodeKey {
            let key = NodeKey::new(NodeKind::Table, "public.t");
            if !self.b.has_node(&key) {
                self.b.add_node(key.clone(), "t", Attrs::new()).unwrap();
            }
            key
        }

        fn edge_at(
            &mut self,
            src: &NodeKey,
            kind: EdgeKind,
            dst: &NodeKey,
            c: Certainty,
            at: (&str, i32),
            attrs: Attrs,
        ) {
            self.file(at.0);
            let provenance = match kind {
                EdgeKind::Consumes => Provenance::HttpStitch,
                EdgeKind::HandledBy => Provenance::Treesitter,
                EdgeKind::Queries => Provenance::SqlLiteral,
                _ => Provenance::Heuristic,
            };
            self.b
                .add_edge(
                    NewEdge::new(src.clone(), kind, dst.clone(), provenance, c)
                        .attrs(attrs)
                        .evidence(Evidence {
                            path: at.0.into(),
                            span: [at.1, 2, at.1, 8],
                        }),
                )
                .unwrap();
        }

        fn edge(&mut self, src: &NodeKey, kind: EdgeKind, dst: &NodeKey, c: Certainty) {
            // The evidence lies in the file of whichever end is a symbol.
            let sym = if src.kind == NodeKind::Symbol {
                src
            } else {
                dst
            };
            let path = sym.key.split('#').next().unwrap().to_owned();
            self.edge_at(src, kind, dst, c, (&path, 1), Attrs::new());
        }

        fn query(&mut self, src: &NodeKey, op: &str, c: Certainty, at: (&str, i32)) {
            let t = self.table();
            let attrs = json!({"ops": [op]}).as_object().cloned().unwrap();
            self.edge_at(src, EdgeKind::Queries, &t, c, at, attrs);
        }

        fn finish(self) -> Graph {
            self.b.finish().unwrap()
        }
    }

    fn rollup(g: &Graph, hops: usize) -> TableRollup {
        let t = g
            .nodes()
            .iter()
            .find(|n| n.kind == NodeKind::Table)
            .unwrap()
            .id;
        table_rollup(
            &ix(g),
            t,
            RollupOptions {
                precision: Precision::Cha,
                hops,
                caps: Caps::default(),
            },
        )
    }

    /// `key ops direct|via [certainty] endpoints… entries…` per service.
    fn lines(r: &TableRollup) -> Vec<String> {
        r.services
            .iter()
            .map(|s| {
                let how = if s.direct {
                    "direct".to_owned()
                } else {
                    format!("via {}", s.via.join("+"))
                };
                let eps: Vec<String> = s
                    .endpoints
                    .iter()
                    .map(|e| format!("{} [{}]", e.display, e.certainty))
                    .collect();
                let entries: Vec<String> = s
                    .entry_points
                    .iter()
                    .map(|(k, n)| format!("{k} {n}"))
                    .collect();
                format!(
                    "{} {} {how} [{}] eps: {} entries: {}",
                    s.key,
                    s.ops.join("+"),
                    s.certainty,
                    eps.join(", "),
                    entries.join(", ")
                )
            })
            .collect()
    }

    fn apis(r: &TableRollup) -> Vec<String> {
        r.through_apis
            .iter()
            .map(|a| {
                let chain: Vec<String> = a
                    .chain
                    .iter()
                    .map(|c| format!("{} of {}", c.display, c.service_key))
                    .collect();
                format!("{} via {} [{}]", a.key, chain.join(" then "), a.certainty)
            })
            .collect()
    }

    use Certainty::{Exact, Heuristic, Synthetic};
    use EdgeKind::{Calls, Consumes, HandledBy, Implements};

    /// S14: direct and indirect services, operations, entry kinds, dispatch, the best
    /// certainty, an inline handler in a file with two endpoints, and a two-hop API chain.
    #[test]
    fn services_endpoints_and_api_hops() {
        let mut g = G::new(&[
            ("auth", "service"),
            ("lib", "library"),
            ("web", "service"),
            ("app", "service"),
            ("gw", "service"),
            ("edge", "service"),
        ]);
        // `lib` queries; `auth` calls it through an interface (dispatch) and directly.
        let dao = g.sym("lib/dao.java", "Dao.read");
        let write = g.sym("lib/dao.java", "Dao.write");
        g.query(&dao, "read", Exact, ("lib/dao.java", 3));
        g.query(&write, "update", Synthetic, ("lib/dao.java", 4));
        let iface = g.sym("lib/repo.java", "Repo.read");
        g.edge(&dao, Implements, &iface, Exact);
        let mut main = Attrs::new();
        main.insert("entry".into(), json!(["main"]));
        let login = g.sym("auth/ctl.java", "login");
        let boot = g.sym_with("auth/app.java", "main", main);
        g.edge(&login, Calls, &iface, Heuristic);
        g.edge(&boot, Calls, &write, Exact);
        let ep = g.endpoint("POST /login", "auth");
        g.edge(&ep, HandledBy, &login, Exact);
        // `app`'s server file handles two endpoints inline: only `/x`'s handler (lines 6–9)
        // reaches the table; `/y`'s (lines 11–13) calls something else.
        let server = g.file("app/server.ts");
        let other = g.sym("app/util.ts", "other");
        for (name, span) in [("POST /x", [6, 0, 9, 1]), ("POST /y", [11, 0, 13, 1])] {
            let e = g.endpoint(name, "app");
            let attrs = json!({"handler_spans": [span]})
                .as_object()
                .cloned()
                .unwrap();
            g.edge_at(
                &e,
                HandledBy,
                &server,
                Exact,
                ("app/server.ts", span[0]),
                attrs,
            );
        }
        g.edge_at(
            &server,
            Calls,
            &write,
            Heuristic,
            ("app/server.ts", 7),
            Attrs::new(),
        );
        g.edge_at(
            &server,
            Calls,
            &other,
            Exact,
            ("app/server.ts", 12),
            Attrs::new(),
        );
        // `web` consumes `/login`; `gw` consumes an endpoint `web` handles with that client.
        let client = g.sym("web/api.ts", "login");
        g.edge(&client, Consumes, &ep, Heuristic);
        let web_ep = g.endpoint("GET /proxy", "web");
        g.edge(&web_ep, HandledBy, &client, Exact);
        // `gw` reaches `/login` two ways at one certainty: the shorter chain is kept.
        let gw = g.sym("gw/route.ts", "forward");
        g.edge(&gw, Consumes, &web_ep, Certainty::Ambiguous);
        let edge = g.sym("edge/fwd.ts", "fwd");
        g.edge(&edge, Consumes, &web_ep, Synthetic);
        let gw_direct = g.sym("gw/route.ts", "direct");
        g.edge(&gw_direct, Consumes, &ep, Certainty::Ambiguous);
        // `app` also consumes `/login`: it is listed once, under services.
        let app_client = g.sym("app/util.ts", "callLogin");
        g.edge(&app_client, Consumes, &ep, Heuristic);
        let graph = g.finish();
        let r = rollup(&graph, DEFAULT_HOPS);
        assert_eq!(
            lines(&r),
            [
                "app update via lib [heuristic] eps: POST /x [heuristic] entries: top-level 1",
                "auth read+update via lib [synthetic] eps: POST /login [heuristic] entries: main 1",
                "lib read+update direct [exact] eps:  entries: ",
            ]
        );
        assert_eq!(
            apis(&r),
            [
                "edge via GET /proxy of web then POST /login of auth [heuristic]",
                "gw via POST /login of auth [ambiguous]",
                "web via POST /login of auth [heuristic]",
            ]
        );
        assert!(r.truncated.is_empty(), "{:?}", r.truncated);
        // One hop: only the direct consumers; none: no API dependents.
        assert_eq!(
            apis(&rollup(&graph, 1)),
            [
                "gw via POST /login of auth [ambiguous]",
                "web via POST /login of auth [heuristic]"
            ]
        );
        assert!(rollup(&graph, 0).through_apis.is_empty());
    }

    /// Invariant 8: a `CONSUMES` cycle A ↔ B terminates, and the service that is direct is not
    /// listed again through the API.
    #[test]
    fn a_consumes_cycle_terminates() {
        let mut g = G::new(&[("a", "service"), ("b", "service")]);
        let ha = g.sym("a/h.ts", "ha");
        let hb = g.sym("b/h.ts", "hb");
        g.query(&ha, "read", Exact, ("a/h.ts", 2));
        let ea = g.endpoint("GET /a", "a");
        let eb = g.endpoint("GET /b", "b");
        g.edge(&ea, HandledBy, &ha, Exact);
        g.edge(&eb, HandledBy, &hb, Exact);
        g.edge(&hb, Consumes, &ea, Heuristic);
        g.edge(&ha, Consumes, &eb, Heuristic);
        let graph = g.finish();
        let r = rollup(&graph, MAX_HOPS);
        assert_eq!(apis(&r), ["b via GET /a of a [heuristic]"]);
        assert_eq!(r.services.len(), 1);
    }

    /// A chain `access ← c1 ← … ← c<len>` plus `fan` direct callers of the access.
    fn chain_and_fan(len: usize, fan: usize) -> Graph {
        let mut g = G::new(&[("s", "service")]);
        let access = g.sym("s/a.ts", "access");
        g.query(&access, "read", Exact, ("s/a.ts", 1));
        let mut prev = access.clone();
        for k in 0..len {
            let c = g.sym("s/a.ts", &format!("c{k}"));
            g.edge(&c, Calls, &prev, Exact);
            prev = c;
        }
        for k in 0..fan {
            let f = g.sym("s/b.ts", &format!("f{k}"));
            g.edge(&f, Calls, &access, Exact);
        }
        g.finish()
    }

    /// Invariant 8: a 5,000-deep caller chain is cut at depth 25 and the cut is named.
    #[test]
    fn a_5000_deep_caller_chain_is_cut_at_25() {
        let r = rollup(&chain_and_fan(5000, 0), DEFAULT_HOPS);
        assert_eq!(r.truncated, ["depth"]);
        let r = rollup(&chain_and_fan(MAX_CALLER_DEPTH, 0), DEFAULT_HOPS);
        assert!(r.truncated.is_empty(), "{:?}", r.truncated);
    }

    /// Contract M5 S10 (D6): Q3's `nodes` cap counts the access and every caller reached: an
    /// access with three callers fits in four, not in three; `work` at the spent count holds,
    /// one less cuts.
    #[test]
    fn rollup_caps_hit_at_one_below_their_need() {
        let g = chain_and_fan(3, 0);
        let t = g
            .nodes()
            .iter()
            .find(|n| n.kind == NodeKind::Table)
            .unwrap()
            .id;
        let run = |caps| {
            table_rollup(
                &ix(&g),
                t,
                RollupOptions {
                    precision: Precision::Cha,
                    hops: DEFAULT_HOPS,
                    caps,
                },
            )
        };
        let full = run(Caps::default());
        assert!(full.truncated.is_empty());
        let nodes = |n| {
            run(Caps {
                nodes: n,
                ..Caps::default()
            })
            .truncated
        };
        assert!(nodes(4).is_empty());
        assert_eq!(nodes(3), ["nodes"]);
        let work = |w| {
            run(Caps {
                work: w,
                ..Caps::default()
            })
            .truncated
        };
        assert!(work(full.work).is_empty());
        assert_eq!(work(full.work - 1), ["work"]);
    }

    /// Invariant 9: the roll-up grows linearly in chains and fans (counted, 4× → ≤ 5×).
    #[test]
    fn the_rollup_grows_linearly() {
        let count = |len, fan| {
            let g = chain_and_fan(len, fan);
            crate::work::take();
            let r = rollup(&g, DEFAULT_HOPS);
            assert_eq!(r.services.len(), 1);
            crate::work::take()
        };
        let (small, large) = (count(20, 200), count(20, 800));
        assert!(large <= small * 5, "{small} -> {large}");
        let (small, large) = (count(5, 50), count(20, 200));
        assert!(large <= small * 5, "{small} -> {large}");
    }
}
