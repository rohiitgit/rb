//! Code questions over a snapshot graph held in memory (design §9.1–§9.3): which node the user
//! means, who calls it (Q1), and what breaks if it is removed (Q2). Pure functions of the graph.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use rb_model::{Certainty, Edge, EdgeKind, Evidence, Graph, Node, NodeKind};

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

/// How dispatch is expanded (design §6.2, contract M4b S10 and D5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Precision {
    /// Rapid type analysis: a dispatch step into a type proven never instantiated (its
    /// `IMPLEMENTS` edge carries `impl_instantiated: false`, S9) is skipped and counted.
    Rta,
    /// Class hierarchy analysis: every overriding method is a dispatch target.
    Cha,
}

impl Precision {
    pub fn as_str(self) -> &'static str {
        match self {
            Precision::Rta => "rta",
            Precision::Cha => "cha",
        }
    }
}

/// Whether an `IMPLEMENTS` edge leaves a type RTA proved never instantiated (S9); the attribute
/// is written only as `false`, absent meaning "not proven".
fn never_instantiated(e: &Edge) -> bool {
    e.attrs
        .get("impl_instantiated")
        .and_then(serde_json::Value::as_bool)
        == Some(false)
}

/// Adjacency of a graph, built once per question (shared with `rb path`).
pub(crate) struct View<'g> {
    graph: &'g Graph,
    index: HashMap<i64, usize>,
    inbound: HashMap<i64, Vec<&'g Edge>>,
    outbound: HashMap<i64, Vec<&'g Edge>>,
    precision: Precision,
    /// The dispatch steps `(implementing, implemented)` skipped under RTA, each once.
    pruned: RefCell<BTreeSet<(i64, i64)>>,
}

impl<'g> View<'g> {
    /// A view that expands dispatch by CHA.
    pub(crate) fn new(graph: &'g Graph) -> Self {
        Self::with_precision(graph, Precision::Cha)
    }

    pub(crate) fn with_precision(graph: &'g Graph, precision: Precision) -> Self {
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
            precision,
            pruned: RefCell::new(BTreeSet::new()),
        }
    }

    /// Whether a dispatch step along the `IMPLEMENTS` edge `e` is taken: always under CHA;
    /// under RTA not when `e` leaves a type proven never instantiated, and then the step is
    /// recorded as pruned (invariant 4: `rta` never under-reports silently).
    pub(crate) fn dispatches(&self, e: &Edge) -> bool {
        if self.precision == Precision::Rta && never_instantiated(e) {
            self.pruned.borrow_mut().insert((e.src, e.dst));
            return false;
        }
        true
    }

    /// The dispatch steps skipped so far.
    pub(crate) fn pruned(&self) -> usize {
        self.pruned.borrow().len()
    }

    pub(crate) fn precision(&self) -> Precision {
        self.precision
    }

    pub(crate) fn graph(&self) -> &'g Graph {
        self.graph
    }

    pub(crate) fn node(&self, id: i64) -> &'g Node {
        &self.graph.nodes()[self.index[&id]]
    }

    pub(crate) fn inbound(
        &self,
        id: i64,
        kind: EdgeKind,
        min: Certainty,
    ) -> impl Iterator<Item = &'g Edge> {
        self.inbound
            .get(&id)
            .into_iter()
            .flatten()
            .copied()
            .inspect(|_| crate::work::tick(1))
            .filter(move |e| e.kind == kind && e.certainty >= min)
    }

    pub(crate) fn outbound(&self, id: i64, kind: EdgeKind) -> impl Iterator<Item = &'g Edge> {
        self.outbound
            .get(&id)
            .into_iter()
            .flatten()
            .copied()
            .inspect(|_| crate::work::tick(1))
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
    pub(crate) fn file_of(&self, id: i64) -> Option<i64> {
        if self.node(id).kind == NodeKind::File {
            return Some(id);
        }
        self.outbound(id, EdgeKind::DefinedIn).map(|e| e.dst).next()
    }

    /// Methods `id` implements or overrides, transitively (each once), with the weakest
    /// `IMPLEMENTS` certainty on the way; under RTA not through a pruned step.
    pub(crate) fn overridden(&self, id: i64, min: Certainty) -> Vec<(i64, Certainty)> {
        let mut seen = HashSet::from([id]);
        let mut out = Vec::new();
        let mut stack = vec![(id, Certainty::Exact)];
        while let Some((n, c)) = stack.pop() {
            for e in self
                .outbound(n, EdgeKind::Implements)
                .filter(|e| e.certainty >= min && self.dispatches(e))
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

    /// Who calls (or references) `n`: `(source, certainty of the step, via dispatch, via
    /// reference, the edge's evidence)`. Dispatch steps go through a method `n` overrides and
    /// are at most `dispatch` certain.
    pub(crate) fn callers_of(
        &self,
        n: i64,
        include_references: bool,
        min: Certainty,
    ) -> Vec<(i64, Certainty, bool, bool, &'g [Evidence])> {
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
                        e.evidence.as_slice(),
                    ));
                }
            }
        }
        out.sort_by_cached_key(|a| (a.2, a.3, self.order_key(a.0)));
        out
    }

    /// The entry marks of `n` (contract M4b S11), sorted bytewise: its stored kinds; for a
    /// file, `top-level`, and `test` in a test path; `endpoint <name>` per endpoint it handles,
    /// an inline handler's only when evidence of the step that reached it (`reached_by`) lies
    /// in its spans (M4a invariant 8).
    fn entry_marks(&self, n: i64, reached_by: &[Evidence]) -> Vec<String> {
        let node = self.node(n);
        let mut marks: BTreeSet<String> =
            entry_kinds(node).into_iter().map(str::to_owned).collect();
        if node.kind == NodeKind::File {
            marks.insert("top-level".into());
            if rb_model::is_test_path(&node.key) {
                marks.insert("test".into());
            }
        }
        for e in self.inbound(n, EdgeKind::HandledBy, Certainty::Ambiguous) {
            if let Some(spans) = crate::path::handler_spans(e)
                && !reached_by
                    .iter()
                    .any(|ev| spans.iter().any(|s| crate::path::within(ev.span, *s)))
            {
                continue;
            }
            marks.insert(format!("endpoint {}", endpoint_display(self.node(e.src))));
        }
        marks.into_iter().collect()
    }
}

/// An endpoint's name, then `  (prefix unknown, router <file>#<binding>)` when it is keyed by
/// its router (contract M4a amendment 10, M4b S13 item 7).
pub fn endpoint_display(node: &Node) -> String {
    match node.attrs.get("router").and_then(serde_json::Value::as_str) {
        Some(router) => format!("{}  (prefix unknown, router {router})", node.name),
        None => node.name.clone(),
    }
}

/// How a node is shown to a user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo {
    /// A symbol's descriptors (`AuthService#login().`, `com/acme/auth/AuthService#login().`), a
    /// file's path, or another node's key.
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
            NodeKind::Symbol => symbol_display(&node.key).to_owned(),
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

/// A symbol key's descriptor part, as shown to a user: after the last backtick-slash (heuristic
/// and scip-typescript keys carry the file in a descriptor, `` src/`users.ts`/Cache#get(). ``),
/// else after the last space (a scip-java key's `scheme manager package version` head), else
/// the whole key (contract M3b amendment 39).
pub fn symbol_display(key: &str) -> &str {
    key.rsplit_once("`/")
        .or_else(|| key.rsplit_once(' '))
        .map_or(key, |(_, d)| d)
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
                // The descriptors, split at every separator, the space before a scip-java key's
                // descriptors included (`… 1.4.0 com/acme/…`; contract M3b amendment 39).
                let parts: Vec<&str> = symbol_display(&n.key)
                    .split(['#', '.', '(', ')', '/', ' '])
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
    pub precision: Precision,
    /// Dispatch steps skipped because their implementing type is never instantiated (S10).
    pub pruned_by_rta: usize,
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
    /// Its entry marks (contract M4b S11), empty for none; expansion continues past them.
    pub entry: Vec<String>,
    pub callers: Vec<Caller>,
}

#[derive(Debug, Clone, Copy)]
pub struct CallerOptions {
    pub depth: usize,
    pub include_references: bool,
    pub min_certainty: Certainty,
    pub precision: Precision,
}

/// Callers of `target` up to `depth` levels (§9.2): breadth-first, so each caller appears once,
/// at its shortest distance, under the first node (in output order) that reached it.
pub fn callers(graph: &Graph, target: i64, opts: CallerOptions) -> CallerTree {
    let v = View::with_precision(graph, opts.precision);
    let depth = opts.depth.clamp(1, MAX_CALLER_DEPTH);
    let mut seen = HashSet::from([target]);
    // node → its callers (without their own callers yet), in discovery order.
    let mut children: HashMap<i64, Vec<Caller>> = HashMap::new();
    let mut frontier = vec![(target, Certainty::Exact)];
    for _ in 0..depth {
        let mut next = Vec::new();
        for &(n, c) in &frontier {
            for (src, step, dispatch, reference, evidence) in
                v.callers_of(n, opts.include_references, opts.min_certainty)
            {
                // A recursive target is its own direct caller (listed once, without children).
                let recursive = src == target && n == target;
                if !recursive && !seen.insert(src) {
                    continue;
                }
                let c = c.min(step);
                children.entry(n).or_default().push(Caller {
                    node: src,
                    certainty: c,
                    dispatch,
                    reference,
                    entry: v.entry_marks(src, evidence),
                    callers: Vec::new(),
                });
                if !recursive {
                    next.push((src, c));
                }
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    fn build(n: i64, children: &HashMap<i64, Vec<Caller>>) -> Vec<Caller> {
        children
            .get(&n)
            .into_iter()
            .flatten()
            .map(|c| Caller {
                callers: if c.node == n {
                    Vec::new()
                } else {
                    build(c.node, children)
                },
                ..c.clone()
            })
            .collect()
    }
    CallerTree {
        target,
        callers: build(target, &children),
        precision: v.precision(),
        pruned_by_rta: v.pruned(),
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
    /// Endpoints handled by a removed, breaking or affected node (design §9.3 roll-up): one
    /// more than the handler's distance (a removed handler's endpoint is at 1, and only there),
    /// the weaker of the handler's certainty and its `HANDLED_BY`. They are not repeated under
    /// `breaks` (M4a open item 7).
    pub endpoints: Vec<Impacted>,
    /// What may become dead code (contract M4b S13, D11).
    pub orphans: Orphans,
    /// Impacted tests: nodes whose `entry` holds `test`, and files of test paths (§9.3).
    pub tests: Vec<Impacted>,
    pub truncated: bool,
    pub precision: Precision,
    /// Dispatch steps skipped because their implementing type is never instantiated (S10).
    pub pruned_by_rta: usize,
}

/// The orphans of an impact, or why they were not computed.
#[derive(Debug, Clone, PartialEq)]
pub enum Orphans {
    /// `library service`, `not in a deployable service`, or `not a symbol`.
    NotComputed(&'static str),
    /// Sorted by distance, then declaring file, line and key.
    Computed(Vec<Orphan>),
}

/// A symbol that may become dead code: forward-reachable from the removed set within the
/// target's service, and reached by no live node.
#[derive(Debug, Clone, PartialEq)]
pub struct Orphan {
    pub node: i64,
    /// Steps from the removed set.
    pub distance: usize,
    /// The node it was first reached from (removed or another orphan) and how: `calls`,
    /// `references` or `dispatch`.
    pub via: (i64, &'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Impacted {
    pub node: i64,
    pub certainty: Certainty,
    pub distance: usize,
}

/// What removing `target` breaks (§9.3), dispatch included at `precision`.
pub fn impact(
    graph: &Graph,
    target: i64,
    min_certainty: Certainty,
    precision: Precision,
) -> Impact {
    let v = View::with_precision(graph, precision);
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
            for (src, step, _, _, _) in v.callers_of(n, true, min_certainty) {
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
    let endpoints = endpoint_rollup(&v, &removed, &breaks, &affects, min_certainty);
    // An endpoint is listed once, under `endpoints` (M4a open item 7).
    breaks.retain(|x| v.node(x.node).kind != NodeKind::Endpoint);
    let (orphans, cut) = orphans(&v, target, &removed, &in_removed);
    truncated |= cut;
    let mut tests: Vec<Impacted> = breaks
        .iter()
        .chain(&affects)
        .filter(|x| is_test(&v, x.node))
        .cloned()
        .collect();
    tests.sort_by_key(|x| (x.distance, v.order_key(x.node)));
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
        endpoints,
        orphans,
        tests,
        truncated,
        precision,
        pruned_by_rta: v.pruned(),
    }
}

/// A test: a node whose stored `entry` holds `test` (S11), or a file of a test path (a TS/JS
/// test file's top-level code).
fn is_test(v: &View<'_>, n: i64) -> bool {
    let node = v.node(n);
    match node.kind {
        NodeKind::File => rb_model::is_test_path(&node.key),
        _ => entry_kinds(node).contains(&"test"),
    }
}

/// The stored entry kinds of a node (S11), sorted as stored.
pub fn entry_kinds(node: &Node) -> Vec<&str> {
    node.attrs
        .get("entry")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .collect()
}

/// The service node of `n`'s declaring file.
fn service_node(v: &View<'_>, n: i64) -> Option<i64> {
    let file = v.file_of(n)?;
    v.outbound(file, EdgeKind::InService).map(|e| e.dst).next()
}

/// The orphans of removing `removed` (contract M4b S13, D11), and whether the candidate walk
/// stopped at the node cap (unwalked nodes count as live, so a cut can only keep code).
///
/// Candidates are the symbols of the target's service forward-reachable from the removed set
/// over `CALLS`, `REFERENCES` and dispatch. A candidate is live when it is ineligible or an
/// entry point, when a requiring edge reaches it from a live node (every node outside the
/// removed set and the candidates is live), or when it implements a live method (dispatch);
/// liveness then flows forward along requiring edges and to implementations. The orphans are
/// the candidates that are not live: the greatest fixpoint, so a dead cycle is found. Each walk
/// is one breadth-first search with a visited set; every edge certainty counts, whatever
/// `--min-certainty` says, since ignoring an edge could only make more orphans.
fn orphans(
    v: &View<'_>,
    target: i64,
    removed: &[i64],
    in_removed: &HashSet<i64>,
) -> (Orphans, bool) {
    if v.node(target).kind != NodeKind::Symbol {
        return (Orphans::NotComputed("not a symbol"), false);
    }
    let Some(service) = service_node(v, target) else {
        return (Orphans::NotComputed("not in a deployable service"), false);
    };
    match v.node(service).attrs.get("role").and_then(|r| r.as_str()) {
        Some("service") => {}
        Some("library") => return (Orphans::NotComputed("library service"), false),
        _ => return (Orphans::NotComputed("not in a deployable service"), false),
    }
    let any = Certainty::Ambiguous;
    // Implementations a call to `m` may dispatch to (under RTA not through a pruned step).
    let implementations = |m: i64| {
        v.inbound(m, EdgeKind::Implements, any)
            .filter(|e| v.dispatches(e))
            .map(|e| e.src)
    };

    // Candidates, breadth-first from the removed set.
    let mut candidates: HashMap<i64, Orphan> = HashMap::new();
    let mut frontier: Vec<i64> = removed.to_vec();
    let mut cut = false;
    let mut distance = 0;
    while !frontier.is_empty() && !cut {
        distance += 1;
        let mut next = Vec::new();
        for &n in &frontier {
            let reached = v
                .outbound(n, EdgeKind::Calls)
                .map(|e| (e.dst, "calls"))
                .chain(
                    v.outbound(n, EdgeKind::References)
                        .map(|e| (e.dst, "references")),
                )
                .chain(implementations(n).map(|m| (m, "dispatch")));
            for (m, how) in reached {
                if in_removed.contains(&m)
                    || candidates.contains_key(&m)
                    || v.node(m).kind != NodeKind::Symbol
                    || service_node(v, m) != Some(service)
                {
                    continue;
                }
                if candidates.len() >= IMPACT_NODE_CAP {
                    cut = true;
                    break;
                }
                candidates.insert(
                    m,
                    Orphan {
                        node: m,
                        distance,
                        via: (n, how),
                    },
                );
                next.push(m);
            }
        }
        frontier = next;
    }

    // Liveness: seeds, then forward along requiring edges and to implementations.
    let outside = |x: i64| !in_removed.contains(&x) && !candidates.contains_key(&x);
    let mut live: HashSet<i64> = HashSet::new();
    let mut queue: VecDeque<i64> = VecDeque::new();
    for &c in candidates.keys() {
        let node = v.node(c);
        let eligible = node.attrs.get("orphan_eligible") == Some(&serde_json::Value::Bool(true));
        let seed = !eligible
            || !entry_kinds(node).is_empty()
            || REQUIRING
                .iter()
                .any(|&k| v.inbound(c, k, any).any(|e| outside(e.src)))
            || v.outbound(c, EdgeKind::Implements)
                .any(|e| outside(e.dst) && v.dispatches(e));
        if seed && live.insert(c) {
            queue.push_back(c);
        }
    }
    while let Some(x) = queue.pop_front() {
        let forward = REQUIRING
            .iter()
            .flat_map(|&k| v.outbound(x, k).map(|e| e.dst))
            .chain(implementations(x));
        for y in forward.collect::<Vec<_>>() {
            if candidates.contains_key(&y) && live.insert(y) {
                queue.push_back(y);
            }
        }
    }
    let mut list: Vec<Orphan> = candidates
        .into_values()
        .filter(|o| !live.contains(&o.node))
        .collect();
    list.sort_by_cached_key(|o| (o.distance, v.order_key(o.node)));
    (Orphans::Computed(list), cut)
}

/// The endpoints the impacted set handles (contract M4a S21), at their smallest distance and,
/// there, their best certainty; sorted by distance, then key. An inline handler (with
/// `handler_spans`) handles an endpoint only within its spans, so it counts only when one of
/// its requiring edges into the removed or impacted set has evidence there (invariant 8).
fn endpoint_rollup(
    v: &View<'_>,
    removed: &[i64],
    breaks: &[Impacted],
    affects: &[Impacted],
    min: Certainty,
) -> Vec<Impacted> {
    let impacted: HashMap<i64, (Certainty, usize)> = removed
        .iter()
        .map(|&n| (n, (Certainty::Exact, 0)))
        .chain(
            breaks
                .iter()
                .chain(affects)
                .map(|x| (x.node, (x.certainty, x.distance))),
        )
        .collect();
    let mut found: HashMap<i64, (usize, Certainty)> = HashMap::new();
    for (&n, &(c, d)) in &impacted {
        for e in v.inbound(n, EdgeKind::HandledBy, min) {
            if d > 0
                && let Some(spans) = crate::path::handler_spans(e)
            {
                let inside = v
                    .outbound
                    .get(&n)
                    .into_iter()
                    .flatten()
                    .filter(|x| REQUIRING.contains(&x.kind) && impacted.contains_key(&x.dst))
                    .flat_map(|x| &x.evidence)
                    .any(|ev| spans.iter().any(|s| crate::path::within(ev.span, *s)));
                if !inside {
                    continue;
                }
            }
            let candidate = (d + 1, c.min(e.certainty));
            let best = found.entry(e.src).or_insert(candidate);
            if (candidate.0, Reverse(candidate.1)) < (best.0, Reverse(best.1)) {
                *best = candidate;
            }
        }
    }
    let mut out: Vec<Impacted> = found
        .into_iter()
        .map(|(node, (distance, certainty))| Impacted {
            node,
            certainty,
            distance,
        })
        .collect();
    out.sort_by(|a, b| (a.distance, &v.node(a.node).key).cmp(&(b.distance, &v.node(b.node).key)));
    out
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

    /// Contract M3b amendment 39: a scip-java key, whose descriptors follow a space rather than
    /// a backtick-slash, displays as its descriptors like every other key; a key with neither
    /// shows whole.
    #[test]
    fn scip_java_keys_display_as_their_descriptor() {
        let g = fixture();
        let d = Describer::new(&g);
        let info = d.describe(id(&g, "acme/auth/AuthService#authenticate()."));
        assert_eq!(info.display, "com/acme/auth/AuthService#authenticate().");
        assert_eq!(
            d.describe(id(&g, "SvcImpl#run().")).display,
            "SvcImpl#run()."
        );
        assert_eq!(symbol_display("plain"), "plain");
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
            precision: Precision::Rta,
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
        // The fully qualified dotted form crosses the space before the descriptors.
        assert_eq!(one("com.acme.auth.AuthService.authenticate"), [SCIP_KEY]);
        assert_eq!(one("acme.auth.AuthService.authenticate"), [SCIP_KEY]);
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
        let i = impact(&g, t, Certainty::Ambiguous, Precision::Cha);
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
        let ctl = impact(&g, id(&g, "Ctl#"), Certainty::Ambiguous, Precision::Cha);
        let removed: Vec<String> = ctl.removed.iter().map(|&n| desc(&g, n)).collect();
        assert_eq!(removed, ["Ctl#", "Ctl#handle()."]);
        assert_eq!(names(&ctl.breaks), ["1 page(). [heuristic]"]);
    }

    #[test]
    fn impact_on_an_implementation_includes_its_dispatch_callers() {
        let g = fixture();
        let i = impact(
            &g,
            id(&g, "SvcImpl#run()."),
            Certainty::Ambiguous,
            Precision::Cha,
        );
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
        let i = impact(&g, a, Certainty::Ambiguous, Precision::Cha);
        assert!(i.breaks.len() + i.affects.len() <= 2);
    }

    /// Contract M4b S10: one graph under both precisions. `A#m` and `B#m` implement `I#m`, which
    /// `c` calls; `A`'s step carries `impl_instantiated: false` (S9 proved `A` never
    /// instantiated), `B`'s does not (kept, say by an instantiated subtype). Under `rta` the
    /// dispatch step into `A#m` is skipped and counted, under `cha` it is taken; `B#m` keeps its
    /// dispatch caller under both.
    #[test]
    fn rta_prunes_flagged_dispatch_steps_and_counts_them() {
        let mut b = GraphBuilder::new();
        let file = NodeKey::new(NodeKind::File, "a.java");
        b.add_node(file.clone(), "a.java", Attrs::new()).unwrap();
        let s =
            |n: &str| NodeKey::new(NodeKind::Symbol, format!("rb-heuristic . . . `a.java`/{n}"));
        for (i, n) in ["I#m().", "A#m().", "B#m().", "c()."].iter().enumerate() {
            b.add_node(s(n), *n, Attrs::new()).unwrap();
            b.add_edge(
                NewEdge::new(
                    s(n),
                    EdgeKind::DefinedIn,
                    file.clone(),
                    Provenance::Treesitter,
                    Certainty::Exact,
                )
                .evidence(Evidence {
                    path: "a.java".into(),
                    span: [i as i32, 0, i as i32, 1],
                }),
            )
            .unwrap();
        }
        let mut flagged = Attrs::new();
        flagged.insert("impl_instantiated".into(), false.into());
        for (src, attrs) in [("A#m().", flagged), ("B#m().", Attrs::new())] {
            b.add_edge(
                NewEdge::new(
                    s(src),
                    EdgeKind::Implements,
                    s("I#m()."),
                    Provenance::Heuristic,
                    Certainty::Heuristic,
                )
                .attrs(attrs),
            )
            .unwrap();
        }
        b.add_edge(NewEdge::new(
            s("c()."),
            EdgeKind::Calls,
            s("I#m()."),
            Provenance::Heuristic,
            Certainty::Heuristic,
        ))
        .unwrap();
        let g = b.finish().unwrap();
        let with = |precision| CallerOptions {
            precision,
            ..opts(2)
        };
        let a = id(&g, "A#m().");
        let rta = callers(&g, a, with(Precision::Rta));
        assert!(flat(&g, &rta).is_empty(), "{:?}", flat(&g, &rta));
        assert_eq!((rta.precision, rta.pruned_by_rta), (Precision::Rta, 1));
        let cha = callers(&g, a, with(Precision::Cha));
        assert_eq!(flat(&g, &cha), ["1 c(). [heuristic] (dispatch)"]);
        assert_eq!((cha.precision, cha.pruned_by_rta), (Precision::Cha, 0));
        // The kept step is taken under `rta` too, and nothing is pruned.
        let kept = callers(&g, id(&g, "B#m()."), with(Precision::Rta));
        assert_eq!(flat(&g, &kept), ["1 c(). [heuristic] (dispatch)"]);
        assert_eq!(kept.pruned_by_rta, 0);

        // `rb impact` on `A#m()`: its dispatch caller is affected under `cha` only.
        let affected = |p| -> (Vec<String>, usize) {
            let i = impact(&g, a, Certainty::Ambiguous, p);
            let names = i.affects.iter().map(|x| desc(&g, x.node)).collect();
            (names, i.pruned_by_rta)
        };
        assert_eq!(affected(Precision::Cha), (vec!["c().".to_owned()], 0));
        assert_eq!(affected(Precision::Rta), (Vec::new(), 1));
        // Requiring edges are not dispatch: `A#m()` still breaks when `I#m()` is removed.
        let i = impact(&g, id(&g, "I#m()."), Certainty::Ambiguous, Precision::Rta);
        let breaks: Vec<String> = i.breaks.iter().map(|x| desc(&g, x.node)).collect();
        assert_eq!(breaks, ["A#m().", "B#m().", "c()."]);
        // The walk from the breaking `A#m()` skips its dispatch step and counts it, although
        // that step's callers are in the answer anyway: the count is of steps skipped, an
        // upper bound on what `cha` would add, never less.
        assert_eq!(i.pruned_by_rta, 1);
    }

    /// Contract M4a S21: endpoints handled by the removed, breaking or affected nodes, one
    /// further than their handler, at the weaker certainty; an inline handler only when its
    /// edge into the impacted set lies in its spans.
    #[test]
    fn impact_rolls_up_endpoints() {
        let mut b = GraphBuilder::new();
        let file = |p: &str| NodeKey::new(NodeKind::File, p);
        let sym = |n: &str| {
            NodeKey::new(
                NodeKind::Symbol,
                format!("rb-heuristic . . . `a.ts`/{n}()."),
            )
        };
        let ep = |n: &str| NodeKey::new(NodeKind::Endpoint, format!("HTTP {n} @s"));
        b.add_node(file("a.ts"), "a.ts", Attrs::new()).unwrap();
        for n in ["svc", "other", "handle", "helper"] {
            b.add_node(sym(n), n, Attrs::new()).unwrap();
        }
        for n in ["GET /inline", "GET /named"] {
            b.add_node(ep(n), n, Attrs::new()).unwrap();
        }
        let edge =
            |b: &mut GraphBuilder, s: NodeKey, kind, d: NodeKey, c, line: i32, attrs: Attrs| {
                b.add_edge(
                    NewEdge::new(s, kind, d, Provenance::Heuristic, c)
                        .attrs(attrs)
                        .evidence(Evidence {
                            path: "a.ts".into(),
                            span: [line, 2, line, 9],
                        }),
                )
                .unwrap();
            };
        let mut spans = Attrs::new();
        spans.insert("handler_spans".into(), serde_json::json!([[5, 0, 9, 1]]));
        // The file's inline handler (lines 5–9) calls `svc`; its other top-level code (line 12)
        // calls `other`.
        edge(
            &mut b,
            ep("GET /inline"),
            EdgeKind::HandledBy,
            file("a.ts"),
            Certainty::Exact,
            5,
            spans,
        );
        edge(
            &mut b,
            file("a.ts"),
            EdgeKind::Calls,
            sym("svc"),
            Certainty::Heuristic,
            6,
            Attrs::new(),
        );
        edge(
            &mut b,
            file("a.ts"),
            EdgeKind::Calls,
            sym("other"),
            Certainty::Heuristic,
            12,
            Attrs::new(),
        );
        // `GET /named` is handled by `handle`, which calls `helper`.
        edge(
            &mut b,
            ep("GET /named"),
            EdgeKind::HandledBy,
            sym("handle"),
            Certainty::Exact,
            1,
            Attrs::new(),
        );
        edge(
            &mut b,
            sym("handle"),
            EdgeKind::Calls,
            sym("helper"),
            Certainty::Ambiguous,
            2,
            Attrs::new(),
        );
        let g = b.finish().unwrap();
        let rollup = |name: &str| -> Vec<String> {
            impact(
                &g,
                id(&g, &format!("{name}().")),
                Certainty::Ambiguous,
                Precision::Cha,
            )
            .endpoints
            .iter()
            .map(|x| {
                let key = &g.nodes().iter().find(|n| n.id == x.node).unwrap().key;
                format!("{} {key} [{}]", x.distance, x.certainty.as_str())
            })
            .collect()
        };
        assert_eq!(rollup("svc"), ["2 HTTP GET /inline @s [heuristic]"]);
        assert!(
            rollup("other").is_empty(),
            "outside the inline handler's spans"
        );
        assert_eq!(rollup("handle"), ["1 HTTP GET /named @s [exact]"]);
        assert_eq!(rollup("helper"), ["2 HTTP GET /named @s [ambiguous]"]);
    }

    /// A graph built by name for `rb impact`'s roll-ups: files belong to the service named by
    /// their first path segment (`app` deployable, `lib` a library); symbols are
    /// `<path>#<name>`, `orphan_eligible` unless listed in `ineligible`.
    struct B {
        b: GraphBuilder,
        line: i32,
    }

    impl B {
        fn new() -> Self {
            let mut b = GraphBuilder::new();
            for (s, role) in [("app", "service"), ("lib", "library")] {
                let mut attrs = Attrs::new();
                attrs.insert("role".into(), role.into());
                b.add_node(NodeKey::new(NodeKind::Service, s), s, attrs)
                    .unwrap();
            }
            B { b, line: 0 }
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

        /// A symbol with `attrs` (`orphan_eligible: true` added unless `eligible` is false).
        fn sym_with(
            &mut self,
            path: &str,
            name: &str,
            eligible: bool,
            mut attrs: Attrs,
        ) -> NodeKey {
            let key = NodeKey::new(NodeKind::Symbol, format!("{path}#{name}"));
            if eligible {
                attrs.insert("orphan_eligible".into(), true.into());
            }
            self.b.add_node(key.clone(), name, attrs).unwrap();
            let file = self.file(path);
            self.line += 1;
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
                        span: [self.line, 0, self.line, 1],
                    }),
                )
                .unwrap();
            key
        }

        fn sym(&mut self, path: &str, name: &str) -> NodeKey {
            self.sym_with(path, name, true, Attrs::new())
        }

        fn edge(&mut self, src: &NodeKey, kind: EdgeKind, dst: &NodeKey) {
            self.b
                .add_edge(NewEdge::new(
                    src.clone(),
                    kind,
                    dst.clone(),
                    Provenance::Heuristic,
                    Certainty::Heuristic,
                ))
                .unwrap();
        }
    }

    /// `distance name (via pred kind)` per orphan, or the reason they were not computed.
    fn orphan_lines(g: &Graph, i: &Impact) -> Vec<String> {
        let name = |id: i64| {
            let k = &g.nodes().iter().find(|n| n.id == id).unwrap().key;
            k.rsplit_once('#').map_or(k.clone(), |(_, n)| n.to_owned())
        };
        match &i.orphans {
            Orphans::NotComputed(reason) => vec![format!("not computed: {reason}")],
            Orphans::Computed(list) => list
                .iter()
                .map(|o| {
                    format!(
                        "{} {} (via {} {})",
                        o.distance,
                        name(o.node),
                        name(o.via.0),
                        o.via.1
                    )
                })
                .collect(),
        }
    }

    fn sym_id(g: &Graph, key: &NodeKey) -> i64 {
        g.nodes().iter().find(|n| n.key == key.key).unwrap().id
    }

    /// Contract M4b S13, D11: orphans are the candidates no live node reaches. A dead cycle is
    /// orphaned; a cycle with an outside caller is not; an ineligible or entry candidate is a
    /// live seed and keeps what it calls; an implementation of a live interface method lives by
    /// dispatch, one of a dead or removed method does not.
    #[test]
    fn orphans_are_the_candidates_no_live_node_reaches() {
        let mut b = B::new();
        let t = b.sym("app/a.java", "T");
        let outside = b.sym("app/b.java", "X");
        // A dead mutual recursion, and a cycle with an outside caller.
        let (a1, a2) = (b.sym("app/a.java", "a1"), b.sym("app/a.java", "a2"));
        b.edge(&t, EdgeKind::Calls, &a1);
        b.edge(&a1, EdgeKind::Calls, &a2);
        b.edge(&a2, EdgeKind::Calls, &a1);
        let (c1, c2) = (b.sym("app/a.java", "c1"), b.sym("app/a.java", "c2"));
        b.edge(&t, EdgeKind::Calls, &c1);
        b.edge(&c1, EdgeKind::Calls, &c2);
        b.edge(&c2, EdgeKind::Calls, &c1);
        b.edge(&outside, EdgeKind::Calls, &c2);
        // An ineligible candidate and what it calls; an eligible one with an entry kind.
        let f = b.sym_with("app/a.java", "f", false, Attrs::new());
        let g1 = b.sym("app/a.java", "g");
        b.edge(&t, EdgeKind::References, &f);
        b.edge(&f, EdgeKind::Calls, &g1);
        let mut entry = Attrs::new();
        entry.insert("entry".into(), serde_json::json!(["scheduled"]));
        let h = b.sym_with("app/a.java", "h", true, entry);
        b.edge(&t, EdgeKind::Calls, &h);
        // A dead interface method and its implementation; a live one and its implementation.
        let (im, am) = (b.sym("app/a.java", "I.m"), b.sym("app/a.java", "A.m"));
        b.edge(&t, EdgeKind::Calls, &im);
        b.edge(&am, EdgeKind::Implements, &im);
        let (jm, bm) = (b.sym("app/a.java", "J.m"), b.sym("app/a.java", "B.m"));
        b.edge(&t, EdgeKind::Calls, &jm);
        b.edge(&outside, EdgeKind::Calls, &jm);
        b.edge(&bm, EdgeKind::Implements, &jm);
        // A candidate implementing a method outside the candidates (live by definition).
        let (em, fm) = (b.sym("app/a.java", "E.m"), b.sym("app/c.java", "F.m"));
        b.edge(&t, EdgeKind::Calls, &em);
        b.edge(&em, EdgeKind::Implements, &fm);
        // Something in another service is never a candidate.
        let l = b.sym("lib/l.java", "l");
        b.edge(&t, EdgeKind::Calls, &l);
        let g = b.b.finish().unwrap();
        let i = impact(&g, sym_id(&g, &t), Certainty::Ambiguous, Precision::Cha);
        assert_eq!(
            orphan_lines(&g, &i),
            [
                "1 a1 (via T calls)",
                "1 I.m (via T calls)",
                "2 a2 (via a1 calls)",
                "2 A.m (via I.m dispatch)",
            ]
        );

        // An implementation of a removed method gets nothing from it.
        let mut b = B::new();
        let rm = b.sym("app/a.java", "R.m");
        let cm = b.sym("app/a.java", "C.m");
        b.edge(&cm, EdgeKind::Implements, &rm);
        let g = b.b.finish().unwrap();
        let i = impact(&g, sym_id(&g, &rm), Certainty::Ambiguous, Precision::Cha);
        assert_eq!(orphan_lines(&g, &i), ["1 C.m (via R.m dispatch)"]);

        // A target in a library service: not computed.
        let mut b = B::new();
        let lt = b.sym("lib/l.java", "T");
        let lh = b.sym("lib/l.java", "h");
        b.edge(&lt, EdgeKind::Calls, &lh);
        let g = b.b.finish().unwrap();
        let i = impact(&g, sym_id(&g, &lt), Certainty::Ambiguous, Precision::Cha);
        assert_eq!(orphan_lines(&g, &i), ["not computed: library service"]);
    }

    /// Contract M4b S13 with `--precision rta`: an implementation whose dispatch step is
    /// pruned gets no liveness from a live interface method.
    #[test]
    fn rta_orphans_follow_the_pruned_dispatch() {
        let mut b = B::new();
        let t = b.sym("app/a.java", "T");
        let outside = b.sym("app/b.java", "X");
        let (jm, km) = (b.sym("app/a.java", "J.m"), b.sym("app/a.java", "K.m"));
        b.edge(&t, EdgeKind::Calls, &jm);
        b.edge(&outside, EdgeKind::Calls, &jm);
        let mut flagged = Attrs::new();
        flagged.insert("impl_instantiated".into(), false.into());
        b.b.add_edge(
            NewEdge::new(
                km.clone(),
                EdgeKind::Implements,
                jm.clone(),
                Provenance::Heuristic,
                Certainty::Heuristic,
            )
            .attrs(flagged),
        )
        .unwrap();
        let g = b.b.finish().unwrap();
        let cha = impact(&g, sym_id(&g, &t), Certainty::Ambiguous, Precision::Cha);
        assert!(orphan_lines(&g, &cha).is_empty());
        // Under `rta` `K.m` is not reached by dispatch at all: no candidate, no orphan.
        let rta = impact(&g, sym_id(&g, &t), Certainty::Ambiguous, Precision::Rta);
        assert!(orphan_lines(&g, &rta).is_empty());
        assert_eq!(rta.pruned_by_rta, 1);
    }

    /// Contract M4b S13: impacted tests (an `entry` with `test`, a file of a test path), and
    /// M4a item 7: an endpoint whose handler is removed is listed once, under `endpoints`.
    #[test]
    fn tests_and_endpoints_are_listed_once() {
        let mut b = B::new();
        let t = b.sym("app/src/main/A.java", "T");
        let mut test = Attrs::new();
        test.insert("entry".into(), serde_json::json!(["test"]));
        let tm = b.sym_with("app/src/test/ATest.java", "t", false, test);
        b.edge(&tm, EdgeKind::Calls, &t);
        let spec = b.file("app/web/a.spec.ts");
        b.edge(&spec, EdgeKind::Calls, &t);
        let plain = b.sym("app/src/main/B.java", "user");
        b.edge(&plain, EdgeKind::Calls, &t);
        let ep = NodeKey::new(NodeKind::Endpoint, "HTTP GET /t @app");
        b.b.add_node(ep.clone(), "GET /t", Attrs::new()).unwrap();
        b.edge(&ep, EdgeKind::HandledBy, &t);
        let g = b.b.finish().unwrap();
        let i = impact(&g, sym_id(&g, &t), Certainty::Ambiguous, Precision::Cha);
        let keys = |xs: &[Impacted]| -> Vec<String> {
            xs.iter()
                .map(|x| {
                    let k = &g.nodes().iter().find(|n| n.id == x.node).unwrap().key;
                    format!("{} {k}", x.distance)
                })
                .collect()
        };
        assert_eq!(
            keys(&i.breaks),
            [
                "1 app/src/main/B.java#user",
                "1 app/src/test/ATest.java#t",
                "1 app/web/a.spec.ts"
            ]
        );
        assert_eq!(keys(&i.endpoints), ["1 HTTP GET /t @app"]);
        assert_eq!(
            keys(&i.tests),
            ["1 app/src/test/ATest.java#t", "1 app/web/a.spec.ts"]
        );
    }

    /// Contract M4b S11: entry marks wherever an entry point appears in the caller tree, and
    /// expansion past it; an inline handler's endpoint only for a call inside its spans.
    #[test]
    fn callers_mark_entry_points() {
        let mut b = B::new();
        let t = b.sym("app/a.java", "T");
        let mut main = Attrs::new();
        main.insert("entry".into(), serde_json::json!(["lifecycle", "main"]));
        let m = b.sym_with("app/a.java", "main", false, main);
        let outer = b.sym("app/a.java", "outer");
        b.edge(&m, EdgeKind::Calls, &t);
        b.edge(&outer, EdgeKind::Calls, &m);
        let h = b.sym("app/h.java", "handle");
        b.edge(&h, EdgeKind::Calls, &t);
        let ep = NodeKey::new(NodeKind::Endpoint, "HTTP GET / @app ?router=r.go#g");
        let mut attrs = Attrs::new();
        attrs.insert("router".into(), "r.go#g".into());
        b.b.add_node(ep.clone(), "GET /", attrs).unwrap();
        b.edge(&ep, EdgeKind::HandledBy, &h);
        // Top-level code of a test file, and of a server file inside and outside its inline
        // handler (lines 5-9).
        let spec = b.file("app/a.spec.ts");
        b.edge(&spec, EdgeKind::Calls, &t);
        let server = b.file("app/server.ts");
        let inline = NodeKey::new(NodeKind::Endpoint, "HTTP POST /x @app");
        b.b.add_node(inline.clone(), "POST /x", Attrs::new())
            .unwrap();
        let mut spans = Attrs::new();
        spans.insert("handler_spans".into(), serde_json::json!([[5, 0, 9, 1]]));
        b.b.add_edge(
            NewEdge::new(
                inline,
                EdgeKind::HandledBy,
                server.clone(),
                Provenance::Treesitter,
                Certainty::Exact,
            )
            .attrs(spans),
        )
        .unwrap();
        let at = |line: i32| Evidence {
            path: "app/server.ts".into(),
            span: [line, 2, line, 6],
        };
        let t2 = b.sym("app/a.java", "T2");
        for (dst, line) in [(&t, 6), (&t2, 12)] {
            b.b.add_edge(
                NewEdge::new(
                    server.clone(),
                    EdgeKind::Calls,
                    dst.clone(),
                    Provenance::Heuristic,
                    Certainty::Heuristic,
                )
                .evidence(at(line)),
            )
            .unwrap();
        }
        let g = b.b.finish().unwrap();
        let marks = |target: &NodeKey| -> Vec<String> {
            let mut out = Vec::new();
            fn walk(g: &Graph, cs: &[Caller], depth: usize, out: &mut Vec<String>) {
                for c in cs {
                    let k = &g.nodes().iter().find(|n| n.id == c.node).unwrap().key;
                    out.push(format!("{depth} {k} [{}]", c.entry.join(", ")));
                    walk(g, &c.callers, depth + 1, out);
                }
            }
            let opts = CallerOptions {
                precision: Precision::Rta,
                ..opts(3)
            };
            walk(
                &g,
                &callers(&g, sym_id(&g, target), opts).callers,
                1,
                &mut out,
            );
            out
        };
        assert_eq!(
            marks(&t),
            [
                "1 app/a.java#main [lifecycle, main]",
                "2 app/a.java#outer []",
                "1 app/a.spec.ts [test, top-level]",
                "1 app/h.java#handle [endpoint GET /  (prefix unknown, router r.go#g)]",
                "1 app/server.ts [endpoint POST /x, top-level]",
            ]
        );
        // The same file outside the handler's spans is top-level code only.
        assert_eq!(marks(&t2), ["1 app/server.ts [top-level]"]);
    }

    /// A chain `T → n1 → … → n<len>` of eligible symbols, plus `fan` symbols `T` calls.
    fn chain_and_fan(len: usize, fan: usize) -> (Graph, i64) {
        let mut b = B::new();
        let t = b.sym("app/a.java", "T");
        let mut prev = t.clone();
        for k in 0..len {
            let n = b.sym("app/a.java", &format!("n{k}"));
            b.edge(&prev, EdgeKind::Calls, &n);
            prev = n;
        }
        for k in 0..fan {
            let n = b.sym("app/a.java", &format!("f{k}"));
            b.edge(&t, EdgeKind::Calls, &n);
        }
        let g = b.b.finish().unwrap();
        let id = sym_id(&g, &t);
        (g, id)
    }

    /// Invariant 8: a 5,000-deep chain is walked whole (one BFS with a visited set).
    #[test]
    fn a_5000_deep_dead_chain_is_orphaned() {
        let (g, t) = chain_and_fan(5000, 0);
        let i = impact(&g, t, Certainty::Ambiguous, Precision::Cha);
        let Orphans::Computed(list) = &i.orphans else {
            panic!("{:?}", i.orphans)
        };
        assert_eq!(list.len(), 5000);
        assert_eq!(list.last().unwrap().distance, 5000);
    }

    /// Invariant 9: orphans grow linearly in chains and fans (counted, 4× → ≤ 5×).
    #[test]
    fn orphans_grow_linearly() {
        let count = |len, fan| {
            let (g, t) = chain_and_fan(len, fan);
            crate::work::take();
            let i = impact(&g, t, Certainty::Ambiguous, Precision::Cha);
            assert!(matches!(i.orphans, Orphans::Computed(ref l) if l.len() == len + fan));
            crate::work::take()
        };
        let (small, large) = (count(200, 200), count(800, 800));
        assert!(large <= small * 5, "{small} -> {large}");
    }
}
