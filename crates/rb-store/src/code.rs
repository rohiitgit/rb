//! Code questions over a snapshot graph held in memory (design §9.1–§9.3): which node the user
//! means, who calls it (Q1), and what breaks if it is removed (Q2). Pure functions of the graph.

use std::cell::RefCell;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

use rb_model::{Certainty, Edge, EdgeKind, Evidence, Graph, Node, NodeKind};

use crate::limits::{Caps, Limit};

/// Maximum caller depth (§9.2; contract M5 D6).
pub const MAX_CALLER_DEPTH: usize = crate::limits::CALLER_DEPTH_MAX;

/// Nodes reached by an impact walk before it stops and reports `nodes` (§9.3; D6).
pub const IMPACT_NODE_CAP: usize = crate::limits::NODES;

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

/// A snapshot's graph with its node index and its inbound and outbound adjacency, built once and
/// shared by every question on the snapshot (contract M5 S9; design §8.4's in-memory CSR):
/// immutable, `Send + Sync`, so a server can hold one per live snapshot. Adjacency lists keep the
/// graph's edge order (by `(src, kind, dst, provenance)`), as each question's own did before.
#[derive(Debug, Clone)]
pub struct SnapshotIndex {
    graph: Graph,
    /// Node id → position in `graph.nodes()`.
    position: HashMap<i64, usize>,
    /// CSR by node position: the edges into (out of) position `p` are
    /// `in_edges[in_start[p]..in_start[p + 1]]`, as indices into `graph.edges()`.
    in_start: Vec<usize>,
    in_edges: Vec<usize>,
    out_start: Vec<usize>,
    out_edges: Vec<usize>,
}

impl SnapshotIndex {
    /// Builds the index in one pass over the nodes and two over the edges (counted).
    pub fn new(graph: Graph) -> Self {
        crate::work::tick(graph.nodes().len() + graph.edges().len());
        let position: HashMap<i64, usize> = graph
            .nodes()
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id, i))
            .collect();
        let csr = |end: fn(&Edge) -> i64| -> (Vec<usize>, Vec<usize>) {
            let mut start = vec![0usize; graph.nodes().len() + 1];
            for e in graph.edges() {
                start[position[&end(e)] + 1] += 1;
            }
            for p in 1..start.len() {
                start[p] += start[p - 1];
            }
            let mut cursor = start.clone();
            let mut edges = vec![0usize; graph.edges().len()];
            for (i, e) in graph.edges().iter().enumerate() {
                let slot = &mut cursor[position[&end(e)]];
                edges[*slot] = i;
                *slot += 1;
            }
            (start, edges)
        };
        let (in_start, in_edges) = csr(|e| e.dst);
        let (out_start, out_edges) = csr(|e| e.src);
        SnapshotIndex {
            graph,
            position,
            in_start,
            in_edges,
            out_start,
            out_edges,
        }
    }

    pub fn graph(&self) -> &Graph {
        &self.graph
    }

    /// The node with id `id`, if the snapshot has one.
    pub fn get_node(&self, id: i64) -> Option<&Node> {
        self.position.get(&id).map(|&p| &self.graph.nodes()[p])
    }

    /// The node with id `id`.
    ///
    /// # Panics
    /// When the snapshot has no such node (ids come from its own edges and lookups).
    pub fn node(&self, id: i64) -> &Node {
        &self.graph.nodes()[self.position[&id]]
    }

    fn adjacent<'a>(
        &'a self,
        id: i64,
        start: &'a [usize],
        edges: &'a [usize],
    ) -> impl Iterator<Item = &'a Edge> {
        let range = self
            .position
            .get(&id)
            .map_or(0..0, |&p| start[p]..start[p + 1]);
        edges[range].iter().map(|&i| &self.graph.edges()[i])
    }

    fn edges_into(&self, id: i64) -> impl Iterator<Item = &Edge> {
        self.adjacent(id, &self.in_start, &self.in_edges)
    }

    fn edges_out_of(&self, id: i64) -> impl Iterator<Item = &Edge> {
        self.adjacent(id, &self.out_start, &self.out_edges)
    }
}

/// One question's view of a [`SnapshotIndex`]: its precision and the dispatch steps it pruned
/// (shared with `rb path`, Q3's roll-up and the card).
pub(crate) struct View<'g> {
    ix: &'g SnapshotIndex,
    precision: Precision,
    /// The dispatch steps `(implementing, implemented)` skipped under RTA, each once.
    pruned: RefCell<BTreeSet<(i64, i64)>>,
}

impl<'g> View<'g> {
    /// A view that expands dispatch by CHA.
    pub(crate) fn new(ix: &'g SnapshotIndex) -> Self {
        Self::with_precision(ix, Precision::Cha)
    }

    pub(crate) fn with_precision(ix: &'g SnapshotIndex, precision: Precision) -> Self {
        View {
            ix,
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
        self.ix.graph()
    }

    pub(crate) fn node(&self, id: i64) -> &'g Node {
        self.ix.node(id)
    }

    pub(crate) fn inbound(
        &self,
        id: i64,
        kind: EdgeKind,
        min: Certainty,
    ) -> impl Iterator<Item = &'g Edge> + use<'g> {
        self.ix
            .edges_into(id)
            .take_while(|_| crate::work::spend(1))
            .filter(move |e| e.kind == kind && e.certainty >= min)
    }

    pub(crate) fn outbound(
        &self,
        id: i64,
        kind: EdgeKind,
    ) -> impl Iterator<Item = &'g Edge> + use<'g> {
        self.ix
            .edges_out_of(id)
            .take_while(|_| crate::work::spend(1))
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

/// Describes nodes of one snapshot.
pub struct Describer<'g>(View<'g>);

impl<'g> Describer<'g> {
    pub fn new(ix: &'g SnapshotIndex) -> Self {
        Describer(View::new(ix))
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
pub fn resolve_target(ix: &SnapshotIndex, input: &str) -> Vec<i64> {
    let v = View::new(ix);
    let graph = ix.graph();
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
    /// The target's own entry marks (contract M7i S2): an endpoint handler names its endpoint.
    pub target_entry: Vec<String>,
    pub callers: Vec<Caller>,
    pub precision: Precision,
    /// Dispatch steps skipped because their implementing type is never instantiated (S10).
    pub pruned_by_rta: usize,
    /// The limits that cut the answer (contract M5 S10): `tree`, `work`.
    pub truncated: BTreeSet<Limit>,
    /// Counted operations the question spent (D6).
    pub work: u64,
    /// Callers in test code left out (`exclude_tests`; `None` when not asked).
    pub excluded_tests: Option<usize>,
    /// Every row found, when [`limit_callers`] cut the rows shown.
    pub summary: Option<CallerSummary>,
}

/// Every caller row of a tree whose shown rows were cut (contract M7i S4): how many, and how
/// many per service and per entry mark kind (`endpoint`, `test`, `main`, …), most first.
#[derive(Debug, Clone, PartialEq)]
pub struct CallerSummary {
    pub callers: usize,
    pub shown: usize,
    /// Every row's certainty, so the histogram counts every row found (contract M7j E4).
    pub certainties: Vec<Certainty>,
    pub by_service: Vec<(String, usize)>,
    pub by_entry: Vec<(String, usize)>,
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
    /// `tree` (caller entries) and `work` apply (contract M5 D6).
    pub caps: Caps,
    /// Callers in test code are left out and counted (contract M7i S4).
    pub exclude_tests: bool,
}

/// Callers of `target` up to `depth` levels (§9.2): breadth-first, so each caller appears once,
/// at its shortest distance, under the first node (in output order) that reached it.
/// At most `opts.caps.tree` entries are listed; a caller beyond them, or an element beyond the
/// `work` budget, stops the walk and names the limit in `truncated` (contract M5 S10).
pub fn callers(ix: &SnapshotIndex, target: i64, opts: CallerOptions) -> CallerTree {
    let (mut tree, spent) = crate::work::metered(opts.caps.work, || callers_walk(ix, target, opts));
    if spent.refused {
        tree.truncated.insert(Limit::Work);
    }
    tree.work = spent.work;
    tree
}

fn callers_walk(ix: &SnapshotIndex, target: i64, opts: CallerOptions) -> CallerTree {
    let v = View::with_precision(ix, opts.precision);
    let depth = opts.depth.clamp(1, MAX_CALLER_DEPTH);
    let mut truncated = BTreeSet::new();
    let mut entries = 0usize;
    let mut excluded = 0usize;
    let mut seen = HashSet::from([target]);
    // node → its callers (without their own callers yet), in discovery order.
    let mut children: HashMap<i64, Vec<Caller>> = HashMap::new();
    let mut frontier = vec![(target, Certainty::Exact)];
    'levels: for _ in 0..depth {
        let mut next = Vec::new();
        for &(n, c) in &frontier {
            for (src, step, dispatch, reference, evidence) in
                v.callers_of(n, opts.include_references, opts.min_certainty)
            {
                // A recursive target is its own direct caller (listed once, without children).
                let recursive = src == target && n == target;
                if !recursive && seen.contains(&src) {
                    continue;
                }
                if opts.exclude_tests && !recursive && in_tests(&v, src) {
                    seen.insert(src);
                    excluded += 1;
                    continue;
                }
                if entries == opts.caps.tree {
                    truncated.insert(Limit::Tree);
                    break 'levels;
                }
                entries += 1;
                seen.insert(src);
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
        target_entry: v.entry_marks(target, &[]),
        callers: build(target, &children),
        precision: v.precision(),
        pruned_by_rta: v.pruned(),
        truncated,
        work: 0,
        excluded_tests: opts.exclude_tests.then_some(excluded),
        summary: None,
    }
}

/// Test code: a test node ([`is_test`]) or anything declared in a test path.
fn in_tests(v: &View<'_>, n: i64) -> bool {
    is_test(v, n)
        || v.file_of(n)
            .is_some_and(|f| rb_model::is_test_path(&v.node(f).key))
}

/// Shows at most `max` rows of `tree`, in display order, and summarises every row found
/// (contract M7i S4). A tree within `max` is left as it is.
pub fn limit_callers(ix: &SnapshotIndex, tree: &mut CallerTree, max: usize) {
    fn rows<'c>(cs: &'c [Caller], out: &mut Vec<&'c Caller>) {
        for c in cs {
            out.push(c);
            rows(&c.callers, out);
        }
    }
    fn cut(cs: &mut Vec<Caller>, left: &mut usize) {
        let mut kept = 0;
        for c in cs.iter_mut() {
            if *left == 0 {
                break;
            }
            *left -= 1;
            kept += 1;
            cut(&mut c.callers, left);
        }
        cs.truncate(kept);
    }
    let mut all = Vec::new();
    rows(&tree.callers, &mut all);
    if all.len() <= max {
        return;
    }
    let d = Describer::new(ix);
    let mut by_service: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_entry: BTreeMap<String, usize> = BTreeMap::new();
    for c in &all {
        let service = d.describe(c.node).service.unwrap_or_else(|| "-".into());
        *by_service.entry(service).or_default() += 1;
        for mark in &c.entry {
            let kind = mark.split(' ').next().unwrap_or(mark);
            *by_entry.entry(kind.to_owned()).or_default() += 1;
        }
    }
    let most_first = |m: BTreeMap<String, usize>| {
        let mut v: Vec<(String, usize)> = m.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        v
    };
    let callers = all.len();
    let certainties = all.iter().map(|c| c.certainty).collect();
    cut(&mut tree.callers, &mut max.clone());
    tree.summary = Some(CallerSummary {
        callers,
        shown: max,
        certainties,
        by_service: most_first(by_service),
        by_entry: most_first(by_entry),
    });
    tree.truncated.insert(Limit::Shown);
}

/// Shows at most `max` rows of each list of `impact` (contract M7i S4), recording each cut
/// list's full length in `totals`; the service and owner roll-ups still count every row.
pub fn limit_impact(impact: &mut Impact, max: usize) {
    let found: Vec<Certainty> = impact
        .breaks
        .iter()
        .chain(&impact.affects)
        .map(|x| x.certainty)
        .collect();
    let mut totals = BTreeMap::new();
    for (name, xs) in [
        ("breaks", &mut impact.breaks),
        ("affects", &mut impact.affects),
        ("endpoints", &mut impact.endpoints),
        ("tests", &mut impact.tests),
    ] {
        if xs.len() > max {
            totals.insert(name, xs.len());
            xs.truncate(max);
        }
    }
    if let Orphans::Computed(xs) = &mut impact.orphans
        && xs.len() > max
    {
        totals.insert("orphans", xs.len());
        xs.truncate(max);
    }
    if !totals.is_empty() {
        impact.truncated.insert(Limit::Shown);
        impact.certainties_found = found;
    }
    impact.totals = totals;
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
    /// The limits that cut the answer (contract M5 S10): `nodes`, `work`.
    pub truncated: BTreeSet<Limit>,
    /// Counted operations the question spent (D6).
    pub work: u64,
    pub precision: Precision,
    /// Dispatch steps skipped because their implementing type is never instantiated (S10).
    pub pruned_by_rta: usize,
    /// The full length of each list [`limit_impact`] cut, by list name.
    pub totals: BTreeMap<&'static str, usize>,
    /// When a list was cut: the certainty of every `breaks` and `affects` row found, so the
    /// histogram counts them all (contract M7j E4); empty otherwise.
    pub certainties_found: Vec<Certainty>,
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
    ix: &SnapshotIndex,
    target: i64,
    min_certainty: Certainty,
    precision: Precision,
) -> Impact {
    impact_capped(ix, target, min_certainty, precision, Caps::default())
}

/// [`impact`] under `caps` (`nodes`, `work`; contract M5 S10). A walk stopped by `work` may have
/// missed the edges that keep a candidate alive, so its orphans are not computed.
pub fn impact_capped(
    ix: &SnapshotIndex,
    target: i64,
    min_certainty: Certainty,
    precision: Precision,
    caps: Caps,
) -> Impact {
    let (mut impact, spent) = crate::work::metered(caps.work, || {
        impact_walk(ix, target, min_certainty, precision, caps.nodes)
    });
    if spent.refused {
        impact.truncated.insert(Limit::Work);
        impact.orphans = Orphans::NotComputed("work cap reached");
    }
    impact.work = spent.work;
    impact
}

fn impact_walk(
    ix: &SnapshotIndex,
    target: i64,
    min_certainty: Certainty,
    precision: Precision,
    node_cap: usize,
) -> Impact {
    let v = View::with_precision(ix, precision);
    let mut truncated = BTreeSet::new();
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
    // Keys read adjacency (counted work): each is computed once, so the count does not depend
    // on the input order a comparison sort sees (the work cap is a function of the snapshot).
    removed.sort_by_cached_key(|&n| v.order_key(n));

    // Every node reached counts against `nodes`, wherever it is reached.
    let mut reached: HashMap<i64, (Certainty, usize)> = HashMap::new();
    'removed: for &r in &removed {
        for &kind in REQUIRING {
            for e in v.inbound(r, kind, min_certainty) {
                if in_removed.contains(&e.src) {
                    continue;
                }
                if !reached.contains_key(&e.src) && reached.len() >= node_cap {
                    truncated.insert(Limit::Nodes);
                    break 'removed;
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
    breaks.sort_by_cached_key(|x| v.order_key(x.node));

    let mut frontier: Vec<(i64, Certainty)> =
        breaks.iter().map(|x| (x.node, x.certainty)).collect();
    let mut affects = Vec::new();
    // Callers of a method a removed node overrides may dispatch into it (CHA): they keep
    // compiling but their behaviour changes, so they are affected at distance 1.
    'dispatch: for &r in &removed {
        for (m, via) in v.overridden(r, min_certainty) {
            for &kind in &[EdgeKind::Calls, EdgeKind::References] {
                for e in v.inbound(m, kind, min_certainty) {
                    if in_removed.contains(&e.src) || reached.contains_key(&e.src) {
                        continue;
                    }
                    if reached.len() >= node_cap {
                        truncated.insert(Limit::Nodes);
                        break 'dispatch;
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
    while !frontier.is_empty() && truncated.is_empty() {
        distance += 1;
        let mut next = Vec::new();
        for &(n, c) in &frontier {
            for (src, step, _, _, _) in v.callers_of(n, true, min_certainty) {
                if in_removed.contains(&src) || reached.contains_key(&src) {
                    continue;
                }
                if reached.len() >= node_cap {
                    truncated.insert(Limit::Nodes);
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
    affects.sort_by_cached_key(|x| (x.distance, v.order_key(x.node)));

    let service_of = |n: i64| -> Option<String> {
        let file = v.file_of(n)?;
        v.outbound(file, EdgeKind::InService)
            .map(|e| v.node(e.dst).key.clone())
            .next()
    };
    let endpoints = endpoint_rollup(&v, &removed, &breaks, &affects, min_certainty);
    // An endpoint is listed once, under `endpoints` (M4a open item 7).
    breaks.retain(|x| v.node(x.node).kind != NodeKind::Endpoint);
    let (orphans, cut) = orphans(&v, target, &removed, &in_removed, node_cap);
    if cut {
        truncated.insert(Limit::Nodes);
    }
    let mut tests: Vec<Impacted> = breaks
        .iter()
        .chain(&affects)
        .filter(|x| is_test(&v, x.node))
        .cloned()
        .collect();
    tests.sort_by_cached_key(|x| (x.distance, v.order_key(x.node)));
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
        work: 0,
        precision,
        pruned_by_rta: v.pruned(),
        totals: BTreeMap::new(),
        certainties_found: Vec::new(),
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
    node_cap: usize,
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
                if candidates.len() >= node_cap {
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
                let inside =
                    v.ix.edges_out_of(n)
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

    /// The index of a test graph.
    fn ix(g: &Graph) -> SnapshotIndex {
        SnapshotIndex::new(g.clone())
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
        let gi = ix(&g);
        let d = Describer::new(&gi);
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
            caps: Caps::default(),
            exclude_tests: false,
        }
    }

    #[test]
    fn target_resolution_tiers() {
        let g = fixture();
        let one = |input: &str| -> Vec<String> {
            resolve_target(&ix(&g), input)
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
            flat(&g, &callers(&ix(&g), t, opts(1))),
            [
                // Direct, ambiguous.
                "1 Job#tick(). [ambiguous]",
                // Calls the interface method `SvcImpl#run()` implements: may dispatch here. The
                // path is only as certain as its weakest step (the heuristic call).
                "1 Ctl#handle(). [heuristic] (dispatch)",
            ]
        );
        assert_eq!(
            flat(&g, &callers(&ix(&g), t, opts(2))),
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
            flat(&g, &callers(&ix(&g), t, strict)),
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
        assert!(callers(&ix(&g), t, opts(1)).callers.is_empty());
        let with_refs = CallerOptions {
            include_references: true,
            ..opts(1)
        };
        assert_eq!(
            flat(&g, &callers(&ix(&g), t, with_refs)),
            ["1 Ctl#other(). [heuristic] (ref)"]
        );
    }

    #[test]
    fn impact_tiers_rollups_and_class_members() {
        let g = fixture();
        let t = id(&g, "Svc#run().");
        let i = impact(&ix(&g), t, Certainty::Ambiguous, Precision::Cha);
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
        assert!(i.truncated.is_empty());

        // Removing the class removes its members too.
        let ctl = impact(
            &ix(&g),
            id(&g, "Ctl#"),
            Certainty::Ambiguous,
            Precision::Cha,
        );
        let removed: Vec<String> = ctl.removed.iter().map(|&n| desc(&g, n)).collect();
        assert_eq!(removed, ["Ctl#", "Ctl#handle()."]);
        assert_eq!(names(&ctl.breaks), ["1 page(). [heuristic]"]);
    }

    #[test]
    fn impact_on_an_implementation_includes_its_dispatch_callers() {
        let g = fixture();
        let i = impact(
            &ix(&g),
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
        let tree = callers(&ix(&g), id(&g, "fact()."), opts(3));
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
        let tree = callers(&ix(&g), a, opts(MAX_CALLER_DEPTH));
        assert!(flat(&g, &tree).len() <= 3, "{:?}", flat(&g, &tree));
        let i = impact(&ix(&g), a, Certainty::Ambiguous, Precision::Cha);
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
        let rta = callers(&ix(&g), a, with(Precision::Rta));
        assert!(flat(&g, &rta).is_empty(), "{:?}", flat(&g, &rta));
        assert_eq!((rta.precision, rta.pruned_by_rta), (Precision::Rta, 1));
        let cha = callers(&ix(&g), a, with(Precision::Cha));
        assert_eq!(flat(&g, &cha), ["1 c(). [heuristic] (dispatch)"]);
        assert_eq!((cha.precision, cha.pruned_by_rta), (Precision::Cha, 0));
        // The kept step is taken under `rta` too, and nothing is pruned.
        let kept = callers(&ix(&g), id(&g, "B#m()."), with(Precision::Rta));
        assert_eq!(flat(&g, &kept), ["1 c(). [heuristic] (dispatch)"]);
        assert_eq!(kept.pruned_by_rta, 0);

        // `rb impact` on `A#m()`: its dispatch caller is affected under `cha` only.
        let affected = |p| -> (Vec<String>, usize) {
            let i = impact(&ix(&g), a, Certainty::Ambiguous, p);
            let names = i.affects.iter().map(|x| desc(&g, x.node)).collect();
            (names, i.pruned_by_rta)
        };
        assert_eq!(affected(Precision::Cha), (vec!["c().".to_owned()], 0));
        assert_eq!(affected(Precision::Rta), (Vec::new(), 1));
        // Requiring edges are not dispatch: `A#m()` still breaks when `I#m()` is removed.
        let i = impact(
            &ix(&g),
            id(&g, "I#m()."),
            Certainty::Ambiguous,
            Precision::Rta,
        );
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
                &ix(&g),
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

    /// Contract M7i S4 (stress U5): `exclude_tests` leaves out (and counts) callers in test
    /// code, a test entry or anything declared in a test path, and does not walk past them;
    /// `limit_callers` cuts the rows shown in display order and summarises every row found.
    #[test]
    fn callers_exclude_tests_and_limit_with_a_summary() {
        let mut b = B::new();
        let t = b.sym("app/src/main/A.java", "T");
        let mut test = Attrs::new();
        test.insert("entry".into(), serde_json::json!(["test"]));
        let unit = b.sym_with("app/src/main/AUnit.java", "u", false, test);
        let helper = b.sym("app/src/test/Helper.java", "h");
        let behind = b.sym("app/src/main/C.java", "behind");
        let a = b.sym("app/src/main/B.java", "a");
        let a2 = b.sym("lib/src/L.java", "l");
        b.edge(&unit, EdgeKind::Calls, &t);
        b.edge(&helper, EdgeKind::Calls, &t);
        b.edge(&behind, EdgeKind::Calls, &helper);
        b.edge(&a, EdgeKind::Calls, &t);
        b.edge(&a2, EdgeKind::Calls, &a);
        let g = b.b.finish().unwrap();
        let ix = ix(&g);
        let names = |cs: &[Caller]| -> Vec<String> {
            fn walk(g: &Graph, cs: &[Caller], out: &mut Vec<String>) {
                for c in cs {
                    out.push(
                        g.nodes()
                            .iter()
                            .find(|n| n.id == c.node)
                            .unwrap()
                            .name
                            .clone(),
                    );
                    walk(g, &c.callers, out);
                }
            }
            let mut out = Vec::new();
            walk(&g, cs, &mut out);
            out
        };
        let all = callers(&ix, sym_id(&g, &t), opts(3));
        assert_eq!(names(&all.callers), ["u", "a", "l", "h", "behind"]);
        assert_eq!(all.excluded_tests, None);
        let kept = callers(
            &ix,
            sym_id(&g, &t),
            CallerOptions {
                exclude_tests: true,
                ..opts(3)
            },
        );
        assert_eq!(names(&kept.callers), ["a", "l"]);
        assert_eq!(kept.excluded_tests, Some(2));

        let mut cut = all.clone();
        limit_callers(&ix, &mut cut, 2);
        assert_eq!(names(&cut.callers), ["u", "a"]);
        assert!(cut.truncated.contains(&Limit::Shown));
        assert_eq!(
            cut.summary,
            Some(CallerSummary {
                callers: 5,
                shown: 2,
                certainties: vec![Certainty::Heuristic; 5],
                by_service: vec![("app".into(), 4), ("lib".into(), 1)],
                by_entry: vec![("test".into(), 1)],
            })
        );
        let mut within = all.clone();
        limit_callers(&ix, &mut within, 5);
        assert_eq!(within, all, "a tree within the limit is left as it is");
    }

    /// Contract M7i S4: `limit_impact` cuts each list past the limit and records its length;
    /// a list within it, and the roll-ups, are untouched.
    #[test]
    fn impact_limit_cuts_each_list_and_keeps_totals() {
        let mut b = B::new();
        let t = b.sym("app/src/main/A.java", "T");
        for i in 0..3 {
            let c = b.sym("app/src/main/B.java", &format!("c{i}"));
            b.edge(&c, EdgeKind::Calls, &t);
        }
        let g = b.b.finish().unwrap();
        let full = impact(
            &ix(&g),
            sym_id(&g, &t),
            Certainty::Ambiguous,
            Precision::Cha,
        );
        assert_eq!(full.breaks.len(), 3);
        let mut cut = full.clone();
        limit_impact(&mut cut, 2);
        assert_eq!(cut.breaks.len(), 2);
        assert_eq!(cut.totals.get("breaks"), Some(&3));
        assert!(cut.truncated.contains(&Limit::Shown));
        assert_eq!(cut.services, full.services);
        let mut within = full.clone();
        limit_impact(&mut within, 3);
        assert_eq!(within, full);
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
        let i = impact(
            &ix(&g),
            sym_id(&g, &t),
            Certainty::Ambiguous,
            Precision::Cha,
        );
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
        let i = impact(
            &ix(&g),
            sym_id(&g, &rm),
            Certainty::Ambiguous,
            Precision::Cha,
        );
        assert_eq!(orphan_lines(&g, &i), ["1 C.m (via R.m dispatch)"]);

        // A target in a library service: not computed.
        let mut b = B::new();
        let lt = b.sym("lib/l.java", "T");
        let lh = b.sym("lib/l.java", "h");
        b.edge(&lt, EdgeKind::Calls, &lh);
        let g = b.b.finish().unwrap();
        let i = impact(
            &ix(&g),
            sym_id(&g, &lt),
            Certainty::Ambiguous,
            Precision::Cha,
        );
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
        let cha = impact(
            &ix(&g),
            sym_id(&g, &t),
            Certainty::Ambiguous,
            Precision::Cha,
        );
        assert!(orphan_lines(&g, &cha).is_empty());
        // Under `rta` `K.m` is not reached by dispatch at all: no candidate, no orphan.
        let rta = impact(
            &ix(&g),
            sym_id(&g, &t),
            Certainty::Ambiguous,
            Precision::Rta,
        );
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
        let i = impact(
            &ix(&g),
            sym_id(&g, &t),
            Certainty::Ambiguous,
            Precision::Cha,
        );
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
                &callers(&ix(&g), sym_id(&g, target), opts).callers,
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
        let i = impact(&ix(&g), t, Certainty::Ambiguous, Precision::Cha);
        let Orphans::Computed(list) = &i.orphans else {
            panic!("{:?}", i.orphans)
        };
        assert_eq!(list.len(), 5000);
        assert_eq!(list.last().unwrap().distance, 5000);
    }

    /// Contract M5 S9: one index serves every request of a server, across threads.
    #[test]
    fn the_index_is_send_and_sync() {
        fn shared<T: Send + Sync>() {}
        shared::<SnapshotIndex>();
    }

    /// Contract M5 S9: the index's adjacency lists keep the graph's edge order, as each
    /// question's own adjacency did, so every answer and golden is unchanged.
    #[test]
    fn adjacency_keeps_the_edge_order() {
        let g = fixture();
        let ix = ix(&g);
        for n in g.nodes() {
            let into: Vec<&Edge> = g.edges().iter().filter(|e| e.dst == n.id).collect();
            let out: Vec<&Edge> = g.edges().iter().filter(|e| e.src == n.id).collect();
            assert_eq!(ix.edges_into(n.id).collect::<Vec<_>>(), into);
            assert_eq!(ix.edges_out_of(n.id).collect::<Vec<_>>(), out);
        }
        assert_eq!(ix.edges_into(-1).count(), 0, "an unknown id has no edges");
    }

    /// Contract M5 S9, invariant 10: a depth-1 caller question costs the same counted
    /// operations whether the graph has n or 4n other nodes (its neighbourhood is fixed); the
    /// adjacency it reads is built once per snapshot, counted apart, and grows at most 5× for 4×.
    #[test]
    fn a_depth_one_question_does_not_grow_with_the_graph() {
        let cost = |len: usize| {
            let (g, _) = chain_and_fan(len, 0);
            let n0 = g.nodes().iter().find(|n| n.name == "n0").unwrap().id;
            crate::work::take();
            let ix = SnapshotIndex::new(g);
            let build = crate::work::take();
            let tree = callers(&ix, n0, opts(1));
            let question = crate::work::take();
            assert_eq!(tree.callers.len(), 1);
            (build, question)
        };
        let ((build_small, small), (build_large, large)) = (cost(1000), cost(4000));
        assert_eq!(small, large, "depth-1 callers: {small} -> {large}");
        assert!(
            build_large <= build_small * 5,
            "index build: {build_small} -> {build_large}"
        );
    }

    /// Invariant 9: orphans grow linearly in chains and fans (counted, 4× → ≤ 5×).
    #[test]
    fn orphans_grow_linearly() {
        let count = |len, fan| {
            let (g, t) = chain_and_fan(len, fan);
            crate::work::take();
            let i = impact(&ix(&g), t, Certainty::Ambiguous, Precision::Cha);
            assert!(matches!(i.orphans, Orphans::Computed(ref l) if l.len() == len + fan));
            crate::work::take()
        };
        let (small, large) = (count(200, 200), count(800, 800));
        assert!(large <= small * 5, "{small} -> {large}");
    }

    /// A fixed-seed generator (xorshift64*), so a failing case is reproducible.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % n as u64) as usize
        }
    }

    /// A random snapshot graph over the real kinds: three services (one a library), files in
    /// them, `n` symbols with random `CALLS`, `REFERENCES`, `IMPLEMENTS` (cycles and
    /// self-loops included, mixed certainties, some `impl_instantiated: false`), endpoints
    /// exposed and handled, client sites consuming them, and tables queried.
    fn random_graph(seed: u64, n: usize) -> Graph {
        let mut r = Rng(seed | 1);
        let mut b = GraphBuilder::new();
        let services = ["a", "b", "lib"];
        for s in services {
            let mut attrs = Attrs::new();
            let role = if s == "lib" { "library" } else { "service" };
            attrs.insert("role".into(), role.into());
            b.add_node(NodeKey::new(NodeKind::Service, s), s, attrs)
                .unwrap();
        }
        let files: Vec<String> = (0..6)
            .map(|i| format!("{}/f{i}.ts", services[i % 3]))
            .collect();
        for f in &files {
            b.add_node(
                NodeKey::new(NodeKind::File, f.as_str()),
                f.as_str(),
                Attrs::new(),
            )
            .unwrap();
            let s = NodeKey::new(NodeKind::Service, f.split('/').next().unwrap());
            b.add_edge(NewEdge::new(
                NodeKey::new(NodeKind::File, f.as_str()),
                EdgeKind::InService,
                s,
                Provenance::Manifest,
                Certainty::Exact,
            ))
            .unwrap();
        }
        let certainties = [
            Certainty::Ambiguous,
            Certainty::Heuristic,
            Certainty::Synthetic,
            Certainty::Dispatch,
            Certainty::Exact,
        ];
        let sym = |i: usize| NodeKey::new(NodeKind::Symbol, format!("s{i:03}"));
        for i in 0..n {
            let mut attrs = Attrs::new();
            if r.below(3) > 0 {
                attrs.insert("orphan_eligible".into(), true.into());
            }
            b.add_node(sym(i), format!("s{i}"), attrs).unwrap();
            let f = &files[r.below(files.len())];
            b.add_edge(
                NewEdge::new(
                    sym(i),
                    EdgeKind::DefinedIn,
                    NodeKey::new(NodeKind::File, f.as_str()),
                    Provenance::Treesitter,
                    Certainty::Exact,
                )
                .evidence(Evidence {
                    path: f.clone(),
                    span: [i as i32, 0, i as i32, 1],
                }),
            )
            .unwrap();
        }
        for _ in 0..n * 2 {
            let (x, y) = (r.below(n), r.below(n));
            let kind = [
                EdgeKind::Calls,
                EdgeKind::Calls,
                EdgeKind::References,
                EdgeKind::Implements,
            ][r.below(4)];
            let mut e = NewEdge::new(
                sym(x),
                kind,
                sym(y),
                Provenance::Heuristic,
                certainties[r.below(5)],
            );
            if kind == EdgeKind::Implements && r.below(2) == 0 {
                let mut attrs = Attrs::new();
                attrs.insert("impl_instantiated".into(), false.into());
                e = e.attrs(attrs);
            }
            let _ = b.add_edge(e);
        }
        for k in 0..3 {
            let ep = NodeKey::new(
                NodeKind::Endpoint,
                format!("HTTP GET /e{k} @{}", services[k % 2]),
            );
            b.add_node(ep.clone(), format!("GET /e{k}"), Attrs::new())
                .unwrap();
            let svc = NodeKey::new(NodeKind::Service, services[k % 2]);
            b.add_edge(NewEdge::new(
                svc,
                EdgeKind::Exposes,
                ep.clone(),
                Provenance::Heuristic,
                Certainty::Exact,
            ))
            .unwrap();
            b.add_edge(NewEdge::new(
                ep.clone(),
                EdgeKind::HandledBy,
                sym(r.below(n)),
                Provenance::Heuristic,
                certainties[r.below(5)],
            ))
            .unwrap();
            let _ = b.add_edge(NewEdge::new(
                sym(r.below(n)),
                EdgeKind::Consumes,
                ep,
                Provenance::HttpStitch,
                certainties[r.below(3)],
            ));
        }
        for t in 0..2 {
            let table = NodeKey::new(NodeKind::Table, format!("public.t{t}"));
            b.add_node(table.clone(), format!("t{t}"), Attrs::new())
                .unwrap();
            for _ in 0..3 {
                let mut attrs = Attrs::new();
                attrs.insert("ops".into(), serde_json::json!(["read"]));
                let _ = b.add_edge(
                    NewEdge::new(
                        sym(r.below(n)),
                        EdgeKind::Queries,
                        table.clone(),
                        Provenance::SqlLiteral,
                        Certainty::Exact,
                    )
                    .attrs(attrs),
                );
            }
        }
        b.finish().unwrap()
    }

    /// Caps above anything a graph of this size can reach.
    fn huge() -> Caps {
        Caps {
            nodes: usize::MAX,
            tree: usize::MAX,
            work: u64::MAX,
        }
    }

    /// Caller entries in a tree.
    fn entries(cs: &[Caller]) -> usize {
        cs.iter().map(|c| 1 + entries(&c.callers)).sum()
    }

    /// Contract M5 S10 (property, invariant 3): on random graphs with cycles, for Q1, Q2, Q3 and
    /// Q4, with caps above the graph's size the answer is untruncated and equals the run under
    /// the default caps; with one cap lowered to any value, an answer that differs from the
    /// uncapped one names that cap in `truncated`, and an untruncated answer equals it; `tree`
    /// (Q1) and `work` (all) are strict: below the uncapped need they always name themselves,
    /// at it never.
    #[test]
    fn capped_answers_name_every_cap_that_cut_them() {
        let mut checked = [0usize; 4];
        for seed in 0..40u64 {
            let n = 4 + (seed as usize % 5) * 6;
            let g = random_graph(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0xabcd, n);
            let ix = ix(&g);
            let symbols: Vec<i64> = g
                .nodes()
                .iter()
                .filter(|x| x.kind == NodeKind::Symbol)
                .map(|x| x.id)
                .collect();
            let of_kind = |k| g.nodes().iter().filter(move |x| x.kind == k).map(|x| x.id);

            // Q1: callers, depth 4 with references.
            for &t in symbols.iter().take(6) {
                let run = |caps| {
                    callers(
                        &ix,
                        t,
                        CallerOptions {
                            depth: 4,
                            include_references: true,
                            caps,
                            exclude_tests: false,
                            ..opts(1)
                        },
                    )
                };
                let full = run(huge());
                assert!(full.truncated.is_empty());
                let plain = |mut x: CallerTree| {
                    x.truncated.clear();
                    x.work = 0;
                    x
                };
                assert_eq!(plain(run(Caps::default())), plain(full.clone()));
                let need = entries(&full.callers);
                for tree in 0..=need + 1 {
                    let got = run(Caps { tree, ..huge() });
                    assert_eq!(
                        got.truncated.contains(&Limit::Tree),
                        tree < need,
                        "seed {seed} tree {tree}/{need}"
                    );
                    if got.truncated.is_empty() {
                        assert_eq!(plain(got), plain(full.clone()));
                    }
                }
                for work in [
                    0,
                    full.work / 2,
                    full.work.saturating_sub(1),
                    full.work,
                    full.work + 1,
                ] {
                    let got = run(Caps { work, ..huge() });
                    assert_eq!(
                        got.truncated.contains(&Limit::Work),
                        work < full.work,
                        "seed {seed} work {work}/{}",
                        full.work
                    );
                    if got.truncated.is_empty() {
                        assert_eq!(plain(got), plain(full.clone()));
                    }
                }
                checked[0] += 1;
            }

            // Q2: impact under both precisions.
            for &t in symbols.iter().take(6) {
                for precision in [Precision::Cha, Precision::Rta] {
                    let run = |caps| impact_capped(&ix, t, Certainty::Ambiguous, precision, caps);
                    let full = run(huge());
                    assert!(full.truncated.is_empty());
                    let plain = |mut x: Impact| {
                        x.truncated.clear();
                        x.work = 0;
                        x
                    };
                    assert_eq!(plain(run(Caps::default())), plain(full.clone()));
                    for nodes in 0..=g.nodes().len() {
                        let got = run(Caps { nodes, ..huge() });
                        if plain(got.clone()) != plain(full.clone()) {
                            assert!(
                                got.truncated.contains(&Limit::Nodes),
                                "seed {seed} nodes {nodes}"
                            );
                        }
                    }
                    for work in [0, full.work / 2, full.work.saturating_sub(1), full.work] {
                        let got = run(Caps { work, ..huge() });
                        assert_eq!(
                            got.truncated.contains(&Limit::Work),
                            work < full.work,
                            "seed {seed} work {work}"
                        );
                        if got.truncated.is_empty() {
                            assert_eq!(plain(got), plain(full.clone()));
                        }
                    }
                    checked[1] += 1;
                }
            }

            // Q4: every endpoint's request path.
            for e in of_kind(NodeKind::Endpoint) {
                let run = |caps| {
                    crate::path::request_path(
                        &ix,
                        e,
                        crate::path::PathOptions {
                            caps,
                            ..Default::default()
                        },
                    )
                };
                let full = run(huge());
                let plain = |mut x: crate::path::RequestPath| {
                    x.truncated.clear();
                    x.work = 0;
                    x
                };
                assert!(
                    !full.truncated.contains(&Limit::Tree)
                        && !full.truncated.contains(&Limit::Work)
                );
                for tree in 0..=g.nodes().len() + 1 {
                    let got = run(Caps { tree, ..huge() });
                    if plain(got.clone()) != plain(full.clone()) {
                        assert!(
                            got.truncated.contains(&Limit::Tree),
                            "seed {seed} tree {tree}"
                        );
                    }
                }
                for work in [0, full.work / 2, full.work.saturating_sub(1), full.work] {
                    let got = run(Caps { work, ..huge() });
                    assert_eq!(
                        got.truncated.contains(&Limit::Work),
                        work < full.work,
                        "seed {seed} work {work}"
                    );
                    if got.truncated == full.truncated {
                        assert_eq!(plain(got), plain(full.clone()));
                    }
                }
                checked[2] += 1;
            }

            // Q3: every table's roll-up, with API hops.
            for t in of_kind(NodeKind::Table) {
                let run = |caps| {
                    crate::rollup::table_rollup(
                        &ix,
                        t,
                        crate::rollup::RollupOptions {
                            precision: Precision::Cha,
                            hops: 2,
                            caps,
                        },
                    )
                };
                let full = run(huge());
                let plain = |mut x: crate::rollup::TableRollup| {
                    x.truncated.clear();
                    x.work = 0;
                    x
                };
                let named = |x: &crate::rollup::TableRollup, l: Limit| {
                    x.truncated.iter().any(|n| n == l.as_str())
                };
                assert!(!named(&full, Limit::Nodes) && !named(&full, Limit::Work));
                for nodes in 0..=g.nodes().len() {
                    let got = run(Caps { nodes, ..huge() });
                    if plain(got.clone()) != plain(full.clone()) {
                        assert!(named(&got, Limit::Nodes), "seed {seed} nodes {nodes}");
                    }
                }
                for work in [0, full.work / 2, full.work.saturating_sub(1), full.work] {
                    let got = run(Caps { work, ..huge() });
                    assert_eq!(
                        named(&got, Limit::Work),
                        work < full.work,
                        "seed {seed} work {work}"
                    );
                    if got.truncated == full.truncated {
                        assert_eq!(plain(got), plain(full.clone()));
                    }
                }
                checked[3] += 1;
            }
        }
        assert!(checked.iter().all(|&c| c >= 40), "{checked:?}");
    }

    /// Contract M5 D6: a question's work count is a function of the snapshot and its
    /// parameters: repeated runs (fresh hash maps, so a different iteration order each time)
    /// count the same, so a work cap truncates at the same element on every run.
    #[test]
    fn work_counts_are_deterministic() {
        for seed in 0..10u64 {
            let g = random_graph(seed ^ 0x5eed, 22);
            let ix = ix(&g);
            for t in g
                .nodes()
                .iter()
                .filter(|n| n.kind == NodeKind::Symbol)
                .map(|n| n.id)
            {
                let impact_work: BTreeSet<u64> = (0..5)
                    .map(|_| impact(&ix, t, Certainty::Ambiguous, Precision::Cha).work)
                    .collect();
                assert_eq!(impact_work.len(), 1, "seed {seed}: {impact_work:?}");
                let caller_work: BTreeSet<u64> = (0..5)
                    .map(|_| {
                        callers(
                            &ix,
                            t,
                            CallerOptions {
                                depth: 4,
                                ..opts(1)
                            },
                        )
                        .work
                    })
                    .collect();
                assert_eq!(caller_work.len(), 1, "seed {seed}: {caller_work:?}");
            }
        }
    }

    /// Contract M5 S10 (units, D6): each cap has a row that hits it and one at the uncapped need
    /// that does not. Q1 `tree`: a target with three callers; Q2 `nodes`: a chain of three
    /// callers; `work` for both at the spent count.
    #[test]
    fn each_cap_hits_at_one_below_its_need() {
        let (g, _) = chain_and_fan(3, 0);
        let gi = ix(&g);
        let n2 = g.nodes().iter().find(|n| n.name == "n2").unwrap().id;
        let run = |caps| {
            callers(
                &gi,
                n2,
                CallerOptions {
                    depth: 5,
                    caps,
                    exclude_tests: false,
                    ..opts(1)
                },
            )
        };
        let full = run(Caps::default());
        assert_eq!(entries(&full.callers), 3, "n1, n0, T");
        assert!(
            run(Caps {
                tree: 3,
                ..Caps::default()
            })
            .truncated
            .is_empty()
        );
        let cut = run(Caps {
            tree: 2,
            ..Caps::default()
        });
        assert_eq!(cut.truncated, BTreeSet::from([Limit::Tree]));
        assert_eq!(entries(&cut.callers), 2);
        assert!(
            run(Caps {
                work: full.work,
                ..Caps::default()
            })
            .truncated
            .is_empty()
        );
        assert_eq!(
            run(Caps {
                work: full.work - 1,
                ..Caps::default()
            })
            .truncated,
            BTreeSet::from([Limit::Work])
        );

        let impact_at = |caps| impact_capped(&gi, n2, Certainty::Ambiguous, Precision::Cha, caps);
        let full = impact_at(Caps::default());
        assert_eq!(full.breaks.len() + full.affects.len(), 3);
        assert!(
            impact_at(Caps {
                nodes: 3,
                ..Caps::default()
            })
            .truncated
            .is_empty()
        );
        assert_eq!(
            impact_at(Caps {
                nodes: 2,
                ..Caps::default()
            })
            .truncated,
            BTreeSet::from([Limit::Nodes])
        );
        // `nodes` holds where the removed node's own dependants are reached too: three
        // direct callers do not fit in two.
        let mut b = B::new();
        let target = b.sym("app/a.java", "T");
        for k in 0..3 {
            let caller = b.sym("app/a.java", &format!("c{k}"));
            b.edge(&caller, EdgeKind::Calls, &target);
        }
        let fan_in = b.b.finish().unwrap();
        let (fan_ix, t_in) = (ix(&fan_in), sym_id(&fan_in, &target));
        let wide = |nodes| {
            let caps = Caps {
                nodes,
                ..Caps::default()
            };
            impact_capped(&fan_ix, t_in, Certainty::Ambiguous, Precision::Cha, caps)
        };
        assert!(wide(3).truncated.is_empty() && wide(3).breaks.len() == 3);
        let cut = wide(2);
        assert_eq!(cut.truncated, BTreeSet::from([Limit::Nodes]));
        assert_eq!(cut.breaks.len(), 2);
        assert!(
            impact_at(Caps {
                work: full.work,
                ..Caps::default()
            })
            .truncated
            .is_empty()
        );
        let cut = impact_at(Caps {
            work: full.work - 1,
            ..Caps::default()
        });
        assert_eq!(cut.truncated, BTreeSet::from([Limit::Work]));
        assert_eq!(cut.orphans, Orphans::NotComputed("work cap reached"));
    }
}
