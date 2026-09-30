//! Code questions over a snapshot graph held in memory (design §9.1–§9.3): which node the user
//! means, who calls it (Q1), and what breaks if it is removed (Q2). Pure functions of the graph.

use std::collections::{BTreeMap, HashMap, HashSet};

use rb_model::{Certainty, Edge, EdgeKind, Graph, Node, NodeKind};

/// Default and maximum caller depth (§9.2).
pub const MAX_CALLER_DEPTH: usize = 25;

/// Nodes reached by an impact walk before it stops and reports `truncated` (§9.3).
pub const IMPACT_NODE_CAP: usize = 200_000;

/// Edges whose source stops working when their target is removed (§9.3, tier 1).
const REQUIRING: &[EdgeKind] = &[
    EdgeKind::Calls,
    EdgeKind::References,
    EdgeKind::Implements,
    EdgeKind::Injects,
    EdgeKind::MapsTo,
    EdgeKind::Queries,
    EdgeKind::HandledBy,
];

/// Adjacency of a graph, built once per question.
struct View<'g> {
    graph: &'g Graph,
    index: HashMap<i64, usize>,
    inbound: HashMap<i64, Vec<&'g Edge>>,
    outbound: HashMap<i64, Vec<&'g Edge>>,
}

impl<'g> View<'g> {
    fn new(graph: &'g Graph) -> Self {
        let index = graph
            .nodes()
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id, i))
            .collect();
        let mut inbound: HashMap<i64, Vec<&Edge>> = HashMap::new();
        let mut outbound: HashMap<i64, Vec<&Edge>> = HashMap::new();
        for e in graph.edges() {
            inbound.entry(e.dst).or_default().push(e);
            outbound.entry(e.src).or_default().push(e);
        }
        View {
            graph,
            index,
            inbound,
            outbound,
        }
    }

    fn node(&self, id: i64) -> &'g Node {
        &self.graph.nodes()[self.index[&id]]
    }

    fn inbound(&self, id: i64, kind: EdgeKind, min: Certainty) -> impl Iterator<Item = &'g Edge> {
        self.inbound
            .get(&id)
            .into_iter()
            .flatten()
            .copied()
            .filter(move |e| e.kind == kind && e.certainty >= min)
    }

    fn outbound(&self, id: i64, kind: EdgeKind) -> impl Iterator<Item = &'g Edge> {
        self.outbound
            .get(&id)
            .into_iter()
            .flatten()
            .copied()
            .filter(move |e| e.kind == kind)
    }

    /// Where a node is declared: its file and 0-based line (a `File` is its own location).
    fn location(&self, id: i64) -> (String, i32) {
        let node = self.node(id);
        if node.kind == NodeKind::File {
            return (node.key.clone(), 0);
        }
        self.outbound(id, EdgeKind::DefinedIn)
            .flat_map(|e| e.evidence.first())
            .map(|ev| (ev.path.clone(), ev.span[0]))
            .next()
            .unwrap_or_default()
    }

    /// Output order: by declaring file, line, then key.
    fn order_key(&self, id: i64) -> (String, i32, String) {
        let (path, line) = self.location(id);
        (path, line, self.node(id).key.clone())
    }

    /// The file node declaring `id` (or `id` itself when it is a file).
    fn file_of(&self, id: i64) -> Option<i64> {
        if self.node(id).kind == NodeKind::File {
            return Some(id);
        }
        self.outbound(id, EdgeKind::DefinedIn).map(|e| e.dst).next()
    }

    /// Methods `id` implements or overrides, transitively (each once), with the weakest
    /// `IMPLEMENTS` certainty on the way.
    fn overridden(&self, id: i64, min: Certainty) -> Vec<(i64, Certainty)> {
        let mut seen = HashSet::from([id]);
        let mut out = Vec::new();
        let mut stack = vec![(id, Certainty::Exact)];
        while let Some((n, c)) = stack.pop() {
            for e in self
                .outbound(n, EdgeKind::Implements)
                .filter(|e| e.certainty >= min)
            {
                if seen.insert(e.dst) {
                    let c = c.min(e.certainty);
                    out.push((e.dst, c));
                    stack.push((e.dst, c));
                }
            }
        }
        out
    }

    /// Who calls (or references) `n`: `(source, certainty of the step, via dispatch, via reference)`.
    /// Dispatch steps go through a method `n` overrides and are at most `dispatch` certain.
    fn callers_of(
        &self,
        n: i64,
        include_references: bool,
        min: Certainty,
    ) -> Vec<(i64, Certainty, bool, bool)> {
        let kinds: &[EdgeKind] = if include_references {
            &[EdgeKind::Calls, EdgeKind::References]
        } else {
            &[EdgeKind::Calls]
        };
        let direct = std::iter::once((n, Certainty::Exact, false));
        let via = self
            .overridden(n, min)
            .into_iter()
            .map(|(m, c)| (m, c.min(Certainty::Dispatch), true));
        let mut out = Vec::new();
        for (m, step, dispatch) in direct.chain(via) {
            for &kind in kinds {
                for e in self.inbound(m, kind, min) {
                    out.push((
                        e.src,
                        step.min(e.certainty),
                        dispatch,
                        kind == EdgeKind::References,
                    ));
                }
            }
        }
        out.sort_by_cached_key(|a| (a.2, a.3, self.order_key(a.0)));
        out
    }
}

/// How a node is shown to a user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    /// A symbol's descriptor (`AuthService#login().`), a file's path, or another node's key.
    pub display: String,
    /// Declaring file (empty when unknown).
    pub path: String,
    /// 1-based declaring line (0 when unknown).
    pub line: i32,
    /// Name of the declaring file's service.
    pub service: Option<String>,
}

/// Describes nodes of one graph (builds the adjacency once).
pub struct Describer<'g>(View<'g>);

impl<'g> Describer<'g> {
    pub fn new(graph: &'g Graph) -> Self {
        Describer(View::new(graph))
    }

    pub fn describe(&self, id: i64) -> NodeInfo {
        let v = &self.0;
        let node = v.node(id);
        let display = match node.kind {
            NodeKind::Symbol => node
                .key
                .rsplit_once("`/")
                .map_or(node.key.clone(), |(_, d)| d.to_owned()),
            _ => node.key.clone(),
        };
        let (path, line) = v.location(id);
        let line = if path.is_empty() { 0 } else { line + 1 };
        let service = v.file_of(id).and_then(|f| {
            v.outbound(f, EdgeKind::InService)
                .map(|e| v.node(e.dst).name.clone())
                .next()
        });
        NodeInfo {
            display,
            path,
            line,
            service,
        }
    }
}

/// The nodes `input` may mean, best tier only: an exact key, `path:line`, a key suffix (after a
/// `/`, or the whole descriptor part of a SCIP key), a dotted name (`AuthService.login`) or a
/// plain name. Several means the caller must choose (§9.1).
/// `path:line` picks the symbol declared at or nearest above that line in that file: symbols
/// record where their name is, not their full extent.
pub fn resolve_target(graph: &Graph, input: &str) -> Vec<i64> {
    let v = View::new(graph);
    let wanted = |n: &&Node| matches!(n.kind, NodeKind::Symbol | NodeKind::Table);
    let sorted = |mut ids: Vec<&Node>| -> Vec<i64> {
        ids.sort_by(|a, b| a.key.cmp(&b.key));
        ids.into_iter().map(|n| n.id).collect()
    };
    let nodes = graph.nodes().iter().filter(wanted);
    let exact: Vec<&Node> = nodes.clone().filter(|n| n.key == input).collect();
    if !exact.is_empty() {
        return sorted(exact);
    }
    if let Some((path, line)) = input.rsplit_once(':')
        && let Ok(line) = line.parse::<i32>()
    {
        let best = graph
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::Symbol)
            .filter_map(|n| {
                let (p, l) = v.location(n.id);
                (p == path && l < line).then_some((l, n))
            })
            .max_by_key(|(l, _)| *l);
        if let Some((l, _)) = best {
            let at_line: Vec<&Node> = graph
                .nodes()
                .iter()
                .filter(|n| n.kind == NodeKind::Symbol && v.location(n.id) == (path.to_owned(), l))
                .collect();
            return sorted(at_line);
        }
    }
    // A suffix starts after a `/`, or after the space that starts a SCIP key's descriptors
    // (`com/acme/auth/AuthService#authenticate().` names the whole descriptor part).
    let (slash, space) = (format!("/{input}"), format!(" {input}"));
    let suffix: Vec<&Node> = nodes
        .clone()
        .filter(|n| {
            n.kind == NodeKind::Symbol && (n.key.ends_with(&slash) || n.key.ends_with(&space))
        })
        .collect();
    if !suffix.is_empty() {
        return sorted(suffix);
    }
    if let Some((scope, name)) = input.rsplit_once('.') {
        let dotted: Vec<&Node> = nodes
            .clone()
            .filter(|n| n.name == name && n.kind == NodeKind::Symbol)
            .filter(|n| {
                let desc = n.key.rsplit_once("`/").map_or(n.key.as_str(), |(_, d)| d);
                let parts: Vec<&str> = desc
                    .split(['#', '.', '(', ')', '/'])
                    .filter(|p| !p.is_empty())
                    .collect();
                let wanted: Vec<&str> = scope.split('.').collect();
                parts
                    .windows(wanted.len() + 1)
                    .any(|w| w[..wanted.len()] == wanted[..] && w[wanted.len()] == name)
            })
            .collect();
        if !dotted.is_empty() {
            return sorted(dotted);
        }
    }
    sorted(nodes.filter(|n| n.name == input).collect())
}

#[derive(Debug, Clone, PartialEq)]
pub struct CallerTree {
    pub target: i64,
    pub callers: Vec<Caller>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Caller {
    pub node: i64,
    /// The weakest certainty on the path from this caller to the target.
    pub certainty: Certainty,
    /// Reached through an overridden method (CHA) rather than a direct call.
    pub dispatch: bool,
    /// Reached through `REFERENCES` (a function value) rather than `CALLS`.
    pub reference: bool,
    pub callers: Vec<Caller>,
}

#[derive(Debug, Clone, Copy)]
pub struct CallerOptions {
    pub depth: usize,
    pub include_references: bool,
    pub min_certainty: Certainty,
}

/// Callers of `target` up to `depth` levels (§9.2): breadth-first, so each caller appears once,
/// at its shortest distance, under the first node (in output order) that reached it.
pub fn callers(graph: &Graph, target: i64, opts: CallerOptions) -> CallerTree {
    let v = View::new(graph);
    let depth = opts.depth.clamp(1, MAX_CALLER_DEPTH);
    let mut seen = HashSet::from([target]);
    // node → (parent, certainty, dispatch, reference); children in discovery order.
    let mut children: HashMap<i64, Vec<(i64, Certainty, bool, bool)>> = HashMap::new();
    let mut frontier = vec![(target, Certainty::Exact)];
    for _ in 0..depth {
        let mut next = Vec::new();
        for &(n, c) in &frontier {
            for (src, step, dispatch, reference) in
                v.callers_of(n, opts.include_references, opts.min_certainty)
            {
                // A recursive target is its own direct caller (listed once, without children).
                if src == target && n == target {
                    children
                        .entry(n)
                        .or_default()
                        .push((src, c.min(step), dispatch, reference));
                    continue;
                }
                if seen.insert(src) {
                    let c = c.min(step);
                    children
                        .entry(n)
                        .or_default()
                        .push((src, c, dispatch, reference));
                    next.push((src, c));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    fn build(n: i64, children: &HashMap<i64, Vec<(i64, Certainty, bool, bool)>>) -> Vec<Caller> {
        children
            .get(&n)
            .into_iter()
            .flatten()
            .map(|&(node, certainty, dispatch, reference)| Caller {
                node,
                certainty,
                dispatch,
                reference,
                callers: if node == n {
                    Vec::new()
                } else {
                    build(node, children)
                },
            })
            .collect()
    }
    CallerTree {
        target,
        callers: build(target, &children),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Impact {
    /// The target and, for a type, everything it contains.
    pub removed: Vec<i64>,
    /// Nodes with a requiring edge into `removed`: they stop compiling or fail at startup.
    pub breaks: Vec<Impacted>,
    /// Nodes that reach `breaks` through calls or references: their behaviour may change.
    pub affects: Vec<Impacted>,
    /// Services of the impacted nodes: `(service key, impacted count, is the target's own)`.
    pub services: Vec<(String, usize, bool)>,
    /// Owners of the impacted nodes' files: `(owner key, impacted count)`.
    pub owners: Vec<(String, usize)>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Impacted {
    pub node: i64,
    pub certainty: Certainty,
    pub distance: usize,
}

/// What removing `target` breaks (§9.3), CHA dispatch included.
pub fn impact(graph: &Graph, target: i64, min_certainty: Certainty) -> Impact {
    let v = View::new(graph);
    // The target plus everything it contains.
    let mut removed = vec![target];
    let mut in_removed = HashSet::from([target]);
    let mut i = 0;
    while i < removed.len() {
        let n = removed[i];
        for e in v.outbound(n, EdgeKind::Contains) {
            if in_removed.insert(e.dst) {
                removed.push(e.dst);
            }
        }
        i += 1;
    }
    removed.sort_by_key(|&n| v.order_key(n));

    let mut reached: HashMap<i64, (Certainty, usize)> = HashMap::new();
    for &r in &removed {
        for &kind in REQUIRING {
            for e in v.inbound(r, kind, min_certainty) {
                if in_removed.contains(&e.src) {
                    continue;
                }
                let entry = reached.entry(e.src).or_insert((e.certainty, 1));
                entry.0 = entry.0.max(e.certainty);
            }
        }
    }
    let mut breaks: Vec<Impacted> = reached
        .iter()
        .map(|(&node, &(certainty, distance))| Impacted {
            node,
            certainty,
            distance,
        })
        .collect();
    breaks.sort_by_key(|x| v.order_key(x.node));

    let mut truncated = false;
    let mut frontier: Vec<(i64, Certainty)> =
        breaks.iter().map(|x| (x.node, x.certainty)).collect();
    let mut affects = Vec::new();
    // Callers of a method a removed node overrides may dispatch into it (CHA): they keep
    // compiling but their behaviour changes, so they are affected at distance 1.
    for &r in &removed {
        for (m, via) in v.overridden(r, min_certainty) {
            for &kind in &[EdgeKind::Calls, EdgeKind::References] {
                for e in v.inbound(m, kind, min_certainty) {
                    if in_removed.contains(&e.src) || reached.contains_key(&e.src) {
                        continue;
                    }
                    let c = via.min(Certainty::Dispatch).min(e.certainty);
                    reached.insert(e.src, (c, 1));
                    affects.push(Impacted {
                        node: e.src,
                        certainty: c,
                        distance: 1,
                    });
                    frontier.push((e.src, c));
                }
            }
        }
    }
    let mut distance = 1;
    while !frontier.is_empty() && !truncated {
        distance += 1;
        let mut next = Vec::new();
        for &(n, c) in &frontier {
            for (src, step, _, _) in v.callers_of(n, true, min_certainty) {
                if in_removed.contains(&src) || reached.contains_key(&src) {
                    continue;
                }
                if reached.len() >= IMPACT_NODE_CAP {
                    truncated = true;
                    break;
                }
                let c = c.min(step);
                reached.insert(src, (c, distance));
                affects.push(Impacted {
                    node: src,
                    certainty: c,
                    distance,
                });
                next.push((src, c));
            }
        }
        frontier = next;
    }
    affects.sort_by_key(|x| (x.distance, v.order_key(x.node)));

    let service_of = |n: i64| -> Option<String> {
        let file = v.file_of(n)?;
        v.outbound(file, EdgeKind::InService)
            .map(|e| v.node(e.dst).key.clone())
            .next()
    };
    let own = service_of(target);
    let mut services: BTreeMap<String, usize> = BTreeMap::new();
    let mut owners: BTreeMap<String, usize> = BTreeMap::new();
    for x in breaks.iter().chain(&affects) {
        if let Some(s) = service_of(x.node) {
            *services.entry(s).or_default() += 1;
        }
        if let Some(file) = v.file_of(x.node) {
            for e in v.outbound(file, EdgeKind::OwnedBy) {
                *owners.entry(v.node(e.dst).key.clone()).or_default() += 1;
            }
        }
    }
    Impact {
        removed,
        breaks,
        affects,
        services: services
            .into_iter()
            .map(|(s, n)| {
                let is_own = own.as_deref() == Some(s.as_str());
                (s, n, is_own)
            })
            .collect(),
        owners: owners.into_iter().collect(),
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use rb_model::{
        Attrs, Certainty, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind, Provenance,
    };

    use super::*;

    const SCIP_KEY: &str =
        "scip-java maven maven/com.acme/svc 1.0 com/acme/auth/AuthService#authenticate().";

    /// A small codebase:
    /// - `api/Ctl.java`: `Ctl#handle()` calls `Svc#run()` (interface method, heuristic);
    ///   `Ctl#other()` passes `Util#fmt()` as a value (REFERENCES).
    /// - `core/Svc.java`: interface `Svc` with `run()`; `SvcImpl#run()` implements it.
    /// - `core/Job.java`: `Job#tick()` calls `SvcImpl#run()` (ambiguous).
    /// - `web/Page.ts`: `page()` calls `Ctl#handle()`.
    ///
    /// Services: api (Ctl), core (Svc, Job), web (Page). Owners: @api on api, @core on core.
    /// `core/Svc.java` also holds one symbol with a SCIP key, [`SCIP_KEY`], with no edges.
    fn fixture() -> Graph {
        let mut b = GraphBuilder::new();
        let file = |p: &str| NodeKey::new(NodeKind::File, p);
        let sym = |k: &str| NodeKey::new(NodeKind::Symbol, k);
        let at = |p: &str, line: i32| Evidence {
            path: p.to_owned(),
            span: [line, 0, line, 5],
        };
        let files = [
            ("api/Ctl.java", "api", "@api"),
            ("core/Svc.java", "core", "@core"),
            ("core/Job.java", "core", "@core"),
            ("web/Page.ts", "web", ""),
        ];
        for (p, service, owner) in files {
            b.add_node(file(p), p, Attrs::new()).unwrap();
            let s = NodeKey::new(NodeKind::Service, service);
            if !b.has_node(&s) {
                b.add_node(s.clone(), service, Attrs::new()).unwrap();
            }
            b.add_edge(NewEdge::new(
                file(p),
                EdgeKind::InService,
                s,
                Provenance::Manifest,
                Certainty::Exact,
            ))
            .unwrap();
            if !owner.is_empty() {
                let o = NodeKey::new(NodeKind::Owner, owner);
                if !b.has_node(&o) {
                    b.add_node(o.clone(), owner, Attrs::new()).unwrap();
                }
                b.add_edge(NewEdge::new(
                    file(p),
                    EdgeKind::OwnedBy,
                    o,
                    Provenance::Codeowners,
                    Certainty::Exact,
                ))
                .unwrap();
            }
        }
        let symbols = [
            ("api/Ctl.java", "Ctl#", 2),
            ("api/Ctl.java", "Ctl#handle().", 3),
            ("api/Ctl.java", "Ctl#other().", 6),
            ("api/Ctl.java", "Util#fmt().", 9),
            ("core/Svc.java", "Svc#", 1),
            ("core/Svc.java", "Svc#run().", 2),
            ("core/Svc.java", "SvcImpl#", 4),
            ("core/Svc.java", "SvcImpl#run().", 5),
            ("core/Job.java", "Job#tick().", 1),
            ("web/Page.ts", "page().", 0),
        ];
        for (p, desc, line) in symbols {
            let key = format!("rb-heuristic . . . `{p}`/{desc}");
            let name = desc
                .trim_end_matches(['.', '#', '(', ')'])
                .rsplit(['#', '.'])
                .next()
                .unwrap();
            b.add_node(sym(&key), name, Attrs::new()).unwrap();
            b.add_edge(
                NewEdge::new(
                    sym(&key),
                    EdgeKind::DefinedIn,
                    file(p),
                    Provenance::Treesitter,
                    Certainty::Exact,
                )
                .evidence(at(p, line)),
            )
            .unwrap();
        }
        // One SCIP-keyed symbol (a key whose descriptor starts after a space, not a `/`).
        b.add_node(sym(SCIP_KEY), "authenticate", Attrs::new())
            .unwrap();
        b.add_edge(
            NewEdge::new(
                sym(SCIP_KEY),
                EdgeKind::DefinedIn,
                file("core/Svc.java"),
                Provenance::Scip,
                Certainty::Exact,
            )
            .evidence(at("core/Svc.java", 9)),
        )
        .unwrap();
        let k = |p: &str, d: &str| sym(&format!("rb-heuristic . . . `{p}`/{d}"));
        let edge = |b: &mut GraphBuilder, s: NodeKey, kind, d: NodeKey, c, p: &str, line| {
            b.add_edge(NewEdge::new(s, kind, d, Provenance::Heuristic, c).evidence(at(p, line)))
                .unwrap();
        };
        let ctl = |d| k("api/Ctl.java", d);
        let svc = |d| k("core/Svc.java", d);
        edge(
            &mut b,
            k("api/Ctl.java", "Ctl#"),
            EdgeKind::Contains,
            ctl("Ctl#handle()."),
            Certainty::Exact,
            "api/Ctl.java",
            3,
        );
        edge(
            &mut b,
            ctl("Ctl#handle()."),
            EdgeKind::Calls,
            svc("Svc#run()."),
            Certainty::Heuristic,
            "api/Ctl.java",
            4,
        );
        edge(
            &mut b,
            ctl("Ctl#other()."),
            EdgeKind::References,
            ctl("Util#fmt()."),
            Certainty::Heuristic,
            "api/Ctl.java",
            7,
        );
        edge(
            &mut b,
            svc("SvcImpl#"),
            EdgeKind::Implements,
            svc("Svc#"),
            Certainty::Heuristic,
            "core/Svc.java",
            4,
        );
        edge(
            &mut b,
            svc("SvcImpl#run()."),
            EdgeKind::Implements,
            svc("Svc#run()."),
            Certainty::Heuristic,
            "core/Svc.java",
            5,
        );
        edge(
            &mut b,
            k("core/Job.java", "Job#tick()."),
            EdgeKind::Calls,
            svc("SvcImpl#run()."),
            Certainty::Ambiguous,
            "core/Job.java",
            2,
        );
        edge(
            &mut b,
            k("web/Page.ts", "page()."),
            EdgeKind::Calls,
            ctl("Ctl#handle()."),
            Certainty::Heuristic,
            "web/Page.ts",
            1,
        );
        b.finish().unwrap()
    }

    fn id(g: &Graph, desc: &str) -> i64 {
        g.nodes()
            .iter()
            .find(|n| n.key.ends_with(&format!("/{desc}")))
            .unwrap()
            .id
    }

    fn desc(g: &Graph, id: i64) -> String {
        let key = &g.nodes().iter().find(|n| n.id == id).unwrap().key;
        key.rsplit_once("`/")
            .map_or(key.clone(), |(_, d)| d.to_owned())
    }

    /// `depth desc [certainty] (dispatch) (ref)` in tree order.
    fn flat(g: &Graph, t: &CallerTree) -> Vec<String> {
        fn walk(g: &Graph, cs: &[Caller], depth: usize, out: &mut Vec<String>) {
            for c in cs {
                let mut s = format!("{depth} {} [{}]", desc(g, c.node), c.certainty.as_str());
                if c.dispatch {
                    s.push_str(" (dispatch)");
                }
                if c.reference {
                    s.push_str(" (ref)");
                }
                out.push(s);
                walk(g, &c.callers, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(g, &t.callers, 1, &mut out);
        out
    }

    fn opts(depth: usize) -> CallerOptions {
        CallerOptions {
            depth,
            include_references: false,
            min_certainty: Certainty::Ambiguous,
        }
    }

    #[test]
    fn target_resolution_tiers() {
        let g = fixture();
        let one = |input: &str| -> Vec<String> {
            resolve_target(&g, input)
                .into_iter()
                .map(|i| desc(&g, i))
                .collect()
        };
        assert_eq!(
            one("rb-heuristic . . . `core/Svc.java`/Svc#run()."),
            ["Svc#run()."]
        );
        assert_eq!(one("SvcImpl#run()."), ["SvcImpl#run()."]);
        assert_eq!(one("SvcImpl.run"), ["SvcImpl#run()."]);
        // A plain name matching several symbols is a candidate list, never a pick.
        assert_eq!(one("run"), ["Svc#run().", "SvcImpl#run()."]);
        // `path:line` (1-based): the symbol declared at or nearest above that line.
        assert_eq!(one("core/Svc.java:6"), ["SvcImpl#run()."]);
        assert!(one("nothing").is_empty());
        // A SCIP key's descriptor starts after a space (contract M3b amendment 24), so the whole
        // descriptor is a suffix as much as its tail after a `/`; the dotted form works too.
        assert_eq!(one(SCIP_KEY), [SCIP_KEY]);
        assert_eq!(one("com/acme/auth/AuthService#authenticate()."), [SCIP_KEY]);
        assert_eq!(one("auth/AuthService#authenticate()."), [SCIP_KEY]);
        assert_eq!(one("AuthService#authenticate()."), [SCIP_KEY]);
        assert_eq!(one("AuthService.authenticate"), [SCIP_KEY]);
        assert_eq!(one("core/Svc.java:10"), [SCIP_KEY]);
        // Part of a descriptor is not a suffix.
        assert!(one("acme/auth/AuthService#authenticate()").is_empty());
    }

    #[test]
    fn callers_with_dispatch_depth_and_min_certainty() {
        let g = fixture();
        let t = id(&g, "SvcImpl#run().");
        assert_eq!(
            flat(&g, &callers(&g, t, opts(1))),
            [
                // Direct, ambiguous.
                "1 Job#tick(). [ambiguous]",
                // Calls the interface method `SvcImpl#run()` implements: may dispatch here. The
                // path is only as certain as its weakest step (the heuristic call).
                "1 Ctl#handle(). [heuristic] (dispatch)",
            ]
        );
        assert_eq!(
            flat(&g, &callers(&g, t, opts(2))),
            [
                "1 Job#tick(). [ambiguous]",
                "1 Ctl#handle(). [heuristic] (dispatch)",
                "2 page(). [heuristic]",
            ]
        );
        let strict = CallerOptions {
            min_certainty: Certainty::Heuristic,
            ..opts(2)
        };
        assert_eq!(
            flat(&g, &callers(&g, t, strict)),
            [
                "1 Ctl#handle(). [heuristic] (dispatch)",
                "2 page(). [heuristic]"
            ]
        );
    }

    #[test]
    fn references_only_on_request() {
        let g = fixture();
        let t = id(&g, "Util#fmt().");
        assert!(callers(&g, t, opts(1)).callers.is_empty());
        let with_refs = CallerOptions {
            include_references: true,
            ..opts(1)
        };
        assert_eq!(
            flat(&g, &callers(&g, t, with_refs)),
            ["1 Ctl#other(). [heuristic] (ref)"]
        );
    }

    #[test]
    fn impact_tiers_rollups_and_class_members() {
        let g = fixture();
        let t = id(&g, "Svc#run().");
        let i = impact(&g, t, Certainty::Ambiguous);
        let names = |xs: &[Impacted]| -> Vec<String> {
            xs.iter()
                .map(|x| {
                    format!(
                        "{} {} [{}]",
                        x.distance,
                        desc(&g, x.node),
                        x.certainty.as_str()
                    )
                })
                .collect()
        };
        assert_eq!(
            names(&i.breaks),
            [
                "1 Ctl#handle(). [heuristic]",
                "1 SvcImpl#run(). [heuristic]"
            ]
        );
        // `Job#tick()` calls the implementation that breaks; `page()` calls the caller that breaks.
        assert_eq!(
            names(&i.affects),
            ["2 Job#tick(). [ambiguous]", "2 page(). [heuristic]"]
        );
        assert_eq!(
            i.services,
            [
                ("api".to_owned(), 1, false),
                ("core".to_owned(), 2, true),
                ("web".to_owned(), 1, false)
            ]
        );
        assert_eq!(i.owners, [("@api".to_owned(), 1), ("@core".to_owned(), 2)]);
        assert!(!i.truncated);

        // Removing the class removes its members too.
        let ctl = impact(&g, id(&g, "Ctl#"), Certainty::Ambiguous);
        let removed: Vec<String> = ctl.removed.iter().map(|&n| desc(&g, n)).collect();
        assert_eq!(removed, ["Ctl#", "Ctl#handle()."]);
        assert_eq!(names(&ctl.breaks), ["1 page(). [heuristic]"]);
    }

    #[test]
    fn impact_on_an_implementation_includes_its_dispatch_callers() {
        let g = fixture();
        let i = impact(&g, id(&g, "SvcImpl#run()."), Certainty::Ambiguous);
        let names: Vec<String> = i
            .breaks
            .iter()
            .chain(&i.affects)
            .map(|x| {
                format!(
                    "{} {} [{}]",
                    x.distance,
                    desc(&g, x.node),
                    x.certainty.as_str()
                )
            })
            .collect();
        // `Ctl#handle()` calls the interface method and may dispatch here; `page()` calls it.
        assert_eq!(
            names,
            [
                "1 Job#tick(). [ambiguous]",
                "1 Ctl#handle(). [heuristic]",
                "2 page(). [heuristic]"
            ]
        );
    }

    #[test]
    fn a_recursive_target_lists_itself_once() {
        let mut b = GraphBuilder::new();
        let f = NodeKey::new(NodeKind::File, "r.ts");
        b.add_node(f.clone(), "r.ts", Attrs::new()).unwrap();
        let s = |n: &str| {
            NodeKey::new(
                NodeKind::Symbol,
                format!("rb-heuristic . . . `r.ts`/{n}()."),
            )
        };
        b.add_node(s("fact"), "fact", Attrs::new()).unwrap();
        b.add_node(s("main"), "main", Attrs::new()).unwrap();
        for (x, y) in [("fact", "fact"), ("main", "fact")] {
            b.add_edge(NewEdge::new(
                s(x),
                EdgeKind::Calls,
                s(y),
                Provenance::Heuristic,
                Certainty::Heuristic,
            ))
            .unwrap();
        }
        let g = b.finish().unwrap();
        let tree = callers(&g, id(&g, "fact()."), opts(3));
        assert_eq!(
            flat(&g, &tree),
            ["1 fact(). [heuristic]", "1 main(). [heuristic]"]
        );
    }

    #[test]
    fn cycles_terminate() {
        let mut b = GraphBuilder::new();
        let f = NodeKey::new(NodeKind::File, "a.ts");
        b.add_node(f.clone(), "a.ts", Attrs::new()).unwrap();
        let s = |n: &str| {
            NodeKey::new(
                NodeKind::Symbol,
                format!("rb-heuristic . . . `a.ts`/{n}()."),
            )
        };
        for n in ["a", "b", "c"] {
            b.add_node(s(n), n, Attrs::new()).unwrap();
        }
        for (x, y) in [("a", "b"), ("b", "c"), ("c", "a")] {
            b.add_edge(NewEdge::new(
                s(x),
                EdgeKind::Calls,
                s(y),
                Provenance::Heuristic,
                Certainty::Heuristic,
            ))
            .unwrap();
            b.add_edge(NewEdge::new(
                s(x),
                EdgeKind::Implements,
                s(y),
                Provenance::Heuristic,
                Certainty::Heuristic,
            ))
            .unwrap();
        }
        let g = b.finish().unwrap();
        let a = id(&g, "a().");
        let tree = callers(&g, a, opts(MAX_CALLER_DEPTH));
        assert!(flat(&g, &tree).len() <= 3, "{:?}", flat(&g, &tree));
        let i = impact(&g, a, Certainty::Ambiguous);
        assert!(i.breaks.len() + i.affects.len() <= 2);
    }
}
