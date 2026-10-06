//! `rb path` (design §10.3, contract M4a S15–S19): the endpoints an input names, and the request
//! path through one of them. Upstream: the client sites in other services that `CONSUME` it and
//! their callers, continuing through other endpoints they handle. Downstream: its handlers' calls
//! (CHA dispatch included) within their service and library services, down to the sinks
//! `QUERIES → Table` and `CONSUMES → Endpoint`, which recurses into that endpoint's own
//! downstream. These are statically possible paths, not a trace. Pure functions of the graph.
//!
//! Every walk terminates by construction: a visited set on symbols and files per direction, a
//! visited set on endpoints per direction, a depth cap per hop, a hop cap and a node cap; each
//! cap that cuts the output is named in `truncated`.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashSet};

use rb_model::{Certainty, Edge, EdgeKind, Graph, NodeKind, Span};

use crate::code::{Precision, View, resolve_target};

/// Default and maximum call levels followed per hop (design §11.2).
pub const DEFAULT_DEPTH: usize = 12;
pub const MAX_DEPTH: usize = 25;
/// Default and maximum service hops (`CONSUMES` followed into another endpoint).
pub const DEFAULT_HOPS: usize = 4;
pub const MAX_HOPS: usize = 8;
/// Nodes in one answer at most (design §11.2).
pub const NODE_CAP: usize = 5_000;

/// How a tree node was reached from its parent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Via {
    /// Downstream: an endpoint's handler (`HANDLED_BY`).
    Handler,
    /// Downstream: a call.
    Calls,
    /// Downstream: an implementation a call may dispatch to (CHA); upstream: a caller of a
    /// method the node overrides.
    Dispatch,
    /// Downstream: a table the node queries.
    Queries,
    /// Downstream: an endpoint the node consumes; upstream: a client site consuming the
    /// endpoint above.
    Consumes,
    /// Downstream: a call into another deployable service's code, shown and not followed.
    Crosses,
    /// Upstream: a caller.
    CalledBy,
    /// Upstream: an endpoint the node handles (the request entered there).
    Handles,
}

impl Via {
    pub fn as_str(self) -> &'static str {
        match self {
            Via::Handler => "handler",
            Via::Calls => "calls",
            Via::Dispatch => "dispatch",
            Via::Queries => "queries",
            Via::Consumes => "consumes",
            Via::Crosses => "crosses",
            Via::CalledBy => "called_by",
            Via::Handles => "handles",
        }
    }
}

/// A limit that cut the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Limit {
    Depth,
    Hops,
    Nodes,
}

impl Limit {
    pub fn as_str(self) -> &'static str {
        match self {
            Limit::Depth => "depth",
            Limit::Hops => "hops",
            Limit::Nodes => "nodes",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathNode {
    pub node: i64,
    pub via: Via,
    /// The certainty of the step that reached this node.
    pub certainty: Certainty,
    /// The weakest certainty from the endpoint to this node.
    pub branch: Certainty,
    pub children: Vec<PathNode>,
    /// Nodes below this one whose subtrees reach no sink, hidden (downstream, without `--full`).
    pub collapsed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestPath {
    pub endpoint: i64,
    pub upstream: Vec<PathNode>,
    pub downstream: Vec<PathNode>,
    /// Step certainties of the shown nodes, best first.
    pub histogram: Vec<(Certainty, usize)>,
    pub truncated: BTreeSet<Limit>,
    pub precision: Precision,
    /// Dispatch steps skipped because their implementing type is never instantiated (S10).
    pub pruned_by_rta: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathOptions {
    pub depth: usize,
    pub hops: usize,
    /// Show helper calls instead of collapsing them.
    pub full: bool,
    /// How dispatch is expanded, both ways (contract M4b D5: `rta` for Q4).
    pub precision: Precision,
}

impl Default for PathOptions {
    fn default() -> Self {
        PathOptions {
            depth: DEFAULT_DEPTH,
            hops: DEFAULT_HOPS,
            full: false,
            precision: Precision::Rta,
        }
    }
}

/// The endpoints `input` names (S15), best tier only, sorted by key: an exact endpoint key;
/// `normalised`, the input read as `<METHOD> <path>` and normalised by the caller, against
/// endpoint names; a case-insensitive substring of endpoint names; a symbol (design §9.1 forms)
/// that handles endpoints or consumes them. Several means the caller must choose.
pub fn resolve_endpoints(graph: &Graph, input: &str, normalised: Option<&str>) -> Vec<i64> {
    let mut endpoints: Vec<&rb_model::Node> = graph
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::Endpoint)
        .collect();
    endpoints.sort_by(|a, b| a.key.cmp(&b.key));
    let tier = |keep: &dyn Fn(&rb_model::Node) -> bool| -> Vec<i64> {
        endpoints.iter().filter(|n| keep(n)).map(|n| n.id).collect()
    };
    let exact = tier(&|n| n.key == input);
    if !exact.is_empty() {
        return exact;
    }
    if let Some(name) = normalised {
        let named = tier(&|n| n.name == name);
        if !named.is_empty() {
            return named;
        }
    }
    let lower = input.to_lowercase();
    let substring = tier(&|n| n.name.to_lowercase().contains(&lower));
    if !substring.is_empty() {
        return substring;
    }
    let v = View::new(graph);
    let mut found: BTreeSet<(&str, i64)> = BTreeSet::new();
    for s in resolve_target(graph, input) {
        let handled = v
            .inbound(s, EdgeKind::HandledBy, Certainty::Ambiguous)
            .map(|e| e.src);
        let consumed = v.outbound(s, EdgeKind::Consumes).map(|e| e.dst);
        for e in handled.chain(consumed) {
            found.insert((v.node(e).key.as_str(), e));
        }
    }
    found.into_iter().map(|(_, id)| id).collect()
}

/// The request path through `endpoint` (S16–S18).
pub fn request_path(graph: &Graph, endpoint: i64, opts: PathOptions) -> RequestPath {
    let opts = PathOptions {
        depth: opts.depth.clamp(1, MAX_DEPTH),
        hops: opts.hops.min(MAX_HOPS),
        ..opts
    };
    let mut w = Walk {
        v: View::with_precision(graph, opts.precision),
        opts,
        items: Vec::new(),
        truncated: BTreeSet::new(),
        seen: HashSet::new(),
        endpoints: HashSet::from([endpoint]),
    };
    let up = w.upstream(endpoint, 0, Certainty::Exact);
    w.seen.clear();
    w.endpoints = HashSet::from([endpoint]);
    let down = w.downstream(endpoint, 0, Certainty::Exact);
    let upstream: Vec<PathNode> = up.iter().map(|&i| w.tree(i, false).0).collect();
    let downstream: Vec<PathNode> = down.iter().map(|&i| w.tree(i, !opts.full).0).collect();
    let mut counts: BTreeMap<Reverse<Certainty>, usize> = BTreeMap::new();
    fn count(nodes: &[PathNode], counts: &mut BTreeMap<Reverse<Certainty>, usize>) {
        for n in nodes {
            *counts.entry(Reverse(n.certainty)).or_default() += 1;
            count(&n.children, counts);
        }
    }
    count(&upstream, &mut counts);
    count(&downstream, &mut counts);
    RequestPath {
        endpoint,
        upstream,
        downstream,
        histogram: counts.into_iter().map(|(c, n)| (c.0, n)).collect(),
        truncated: w.truncated,
        precision: opts.precision,
        pruned_by_rta: w.v.pruned(),
    }
}

/// One node of the answer while it is built.
struct Item {
    node: i64,
    via: Via,
    certainty: Certainty,
    branch: Certainty,
    children: Vec<usize>,
    /// Shown whatever lies below: a root, a sink or a crossing.
    keep: bool,
}

/// The inline-handler spans a node is walked within (`None`: the whole node).
type Scope = Option<Vec<Span>>;

/// A node whose neighbours the next level adds.
struct Frontier {
    item: usize,
    node: i64,
    branch: Certainty,
    /// Downstream: an inline handler's spans, which the first step's evidence must lie in.
    /// Upstream: the evidence of the step that reached the node.
    spans: Option<Vec<Span>>,
    /// Downstream: the spans of inline handlers of other endpoints inside the node, whose code
    /// runs only when those endpoints are requested (amendment 12, Q7).
    outside: Vec<Span>,
    /// Downstream: the service of the hop's handler.
    home: Option<i64>,
}

/// A neighbour before it is placed.
struct Candidate<'g> {
    via: Via,
    node: i64,
    certainty: Certainty,
    edge: Option<&'g Edge>,
}

struct Walk<'g> {
    v: View<'g>,
    opts: PathOptions,
    items: Vec<Item>,
    truncated: BTreeSet<Limit>,
    /// Symbols and files expanded in the current direction, each with its inline-handler scope:
    /// one `File` handling several endpoints inline is walked once per span scope, so no
    /// endpoint's downstream (or upstream continuation) is dropped (evaluator pass 1, P4).
    seen: HashSet<(i64, Scope)>,
    /// Endpoints expanded in the current direction.
    endpoints: HashSet<i64>,
}

/// Whether `inner` lies within `outer` (both `[row, col, row, col]`).
pub(crate) fn within(inner: Span, outer: Span) -> bool {
    (outer[0], outer[1]) <= (inner[0], inner[1]) && (inner[2], inner[3]) <= (outer[2], outer[3])
}

/// The `handler_spans` of a `HANDLED_BY` edge to an inline handler (shared with `rb impact`).
pub(crate) fn handler_spans(e: &Edge) -> Option<Vec<Span>> {
    let spans = e.attrs.get("handler_spans")?.as_array()?;
    Some(
        spans
            .iter()
            .filter_map(|s| {
                let s: Vec<i32> = s
                    .as_array()?
                    .iter()
                    .map(|x| x.as_i64().and_then(|x| i32::try_from(x).ok()))
                    .collect::<Option<_>>()?;
                s.try_into().ok()
            })
            .collect(),
    )
}

impl<'g> Walk<'g> {
    fn add(&mut self, c: &Candidate<'_>, branch: Certainty, keep: bool) -> Option<usize> {
        if self.items.len() >= NODE_CAP {
            self.truncated.insert(Limit::Nodes);
            return None;
        }
        self.items.push(Item {
            node: c.node,
            via: c.via,
            certainty: c.certainty,
            branch,
            children: Vec::new(),
            keep,
        });
        Some(self.items.len() - 1)
    }

    /// Best step first, then by how, then by natural key; one candidate per `(how, node)`.
    fn order(&self, mut cs: Vec<Candidate<'g>>) -> Vec<Candidate<'g>> {
        cs.sort_by(|a, b| {
            (Reverse(a.certainty), a.via, &self.v.node(a.node).key).cmp(&(
                Reverse(b.certainty),
                b.via,
                &self.v.node(b.node).key,
            ))
        });
        let mut placed = HashSet::new();
        cs.retain(|c| placed.insert((c.via, c.node)));
        cs
    }

    fn service_of(&self, n: i64) -> Option<i64> {
        let file = self.v.file_of(n)?;
        self.v
            .outbound(file, EdgeKind::InService)
            .map(|e| e.dst)
            .next()
    }

    /// Whether `n` is code of a deployable service other than `home` (D8): `role = service`, or
    /// any service that exposes endpoints, since it serves requests whatever its manifest says
    /// (an Express package without a start script is `library`; evaluator pass 1).
    fn crosses(&self, n: i64, home: Option<i64>) -> bool {
        match (self.service_of(n), home) {
            (Some(s), Some(h)) if s != h => {
                self.v.node(s).attrs.get("role").and_then(|r| r.as_str()) == Some("service")
                    || self.v.outbound(s, EdgeKind::Exposes).next().is_some()
            }
            _ => false,
        }
    }

    /// Methods that implement or override `m`, transitively, each with the weakest
    /// `IMPLEMENTS` certainty on the way (CHA; under RTA not through a pruned step).
    fn implementations(&self, m: i64) -> Vec<(i64, Certainty)> {
        let mut seen = HashSet::from([m]);
        let mut out = Vec::new();
        let mut stack = vec![(m, Certainty::Exact)];
        while let Some((n, c)) = stack.pop() {
            for e in self
                .v
                .inbound(n, EdgeKind::Implements, Certainty::Ambiguous)
                .filter(|e| self.v.dispatches(e))
            {
                if seen.insert(e.src) {
                    let c = c.min(e.certainty);
                    out.push((e.src, c));
                    stack.push((e.src, c));
                }
            }
        }
        out
    }

    /// Upstream: the inline-handler spans of `n` (from its inbound `HANDLED_BY` edges) that the
    /// evidence it was reached by lies in; `None` when `n` is no inline handler.
    fn inline_scope(&self, n: i64, reached_by: &[Span]) -> Scope {
        let mut scope: Option<BTreeSet<Span>> = None;
        for e in self.v.inbound(n, EdgeKind::HandledBy, Certainty::Ambiguous) {
            if let Some(spans) = handler_spans(e) {
                scope.get_or_insert_with(BTreeSet::new).extend(
                    spans
                        .into_iter()
                        .filter(|s| reached_by.iter().any(|ev| within(*ev, *s))),
                );
            }
        }
        scope.map(|s| s.into_iter().collect())
    }

    /// Downstream: the inline-handler spans of `n` (from its inbound `HANDLED_BY` edges) that
    /// do not hold any of `own` (the spans the node is walked within): code there runs only
    /// when its own endpoint is requested (invariant 8; amendment 12, Q7).
    fn other_handlers(&self, n: i64, own: Option<&[Span]>) -> Vec<Span> {
        let own = own.unwrap_or(&[]);
        let mut out: BTreeSet<Span> = BTreeSet::new();
        for e in self.v.inbound(n, EdgeKind::HandledBy, Certainty::Ambiguous) {
            out.extend(
                handler_spans(e)
                    .into_iter()
                    .flatten()
                    .filter(|s| !own.iter().any(|o| within(*o, *s))),
            );
        }
        out.into_iter().collect()
    }

    // -----------------------------------------------------------------------------------------
    // Downstream (S16)

    /// The handlers of `endpoint` as roots, with everything below them.
    fn downstream(&mut self, endpoint: i64, hop: usize, branch: Certainty) -> Vec<usize> {
        let handlers = self.order(
            self.v
                .outbound(endpoint, EdgeKind::HandledBy)
                .map(|e| Candidate {
                    via: Via::Handler,
                    node: e.dst,
                    certainty: e.certainty,
                    edge: Some(e),
                })
                .collect(),
        );
        let mut roots = Vec::new();
        let mut frontier = Vec::new();
        for c in handlers {
            let b = branch.min(c.certainty);
            let Some(i) = self.add(&c, b, true) else {
                break;
            };
            roots.push(i);
            let spans = c.edge.and_then(handler_spans);
            if self.seen.insert((c.node, spans.clone())) {
                frontier.push(Frontier {
                    item: i,
                    node: c.node,
                    branch: b,
                    outside: self.other_handlers(c.node, spans.as_deref()),
                    spans,
                    home: self.service_of(c.node),
                });
            }
        }
        self.expand_down(frontier, hop);
        roots
    }

    fn expand_down(&mut self, mut frontier: Vec<Frontier>, hop: usize) {
        for _ in 0..self.opts.depth {
            let mut next = Vec::new();
            for f in frontier {
                // An edge counts when one of its evidence rows lies in the node's own spans (if
                // any) and in no other endpoint's inline handler.
                let inside = |e: &&Edge| {
                    e.evidence.iter().any(|ev| {
                        f.spans
                            .as_ref()
                            .is_none_or(|spans| spans.iter().any(|s| within(ev.span, *s)))
                            && !f.outside.iter().any(|o| within(ev.span, *o))
                    })
                };
                // A call to this node may dispatch to the methods implementing it (CHA).
                let mut cs: Vec<Candidate> = self
                    .implementations(f.node)
                    .into_iter()
                    .map(|(x, c)| Candidate {
                        via: Via::Dispatch,
                        node: x,
                        certainty: c.min(Certainty::Dispatch),
                        edge: None,
                    })
                    .collect();
                for (kind, via) in [
                    (EdgeKind::Calls, Via::Calls),
                    (EdgeKind::Queries, Via::Queries),
                    (EdgeKind::Consumes, Via::Consumes),
                ] {
                    for e in self.v.outbound(f.node, kind).filter(inside) {
                        cs.push(Candidate {
                            via,
                            node: e.dst,
                            certainty: e.certainty,
                            edge: Some(e),
                        });
                    }
                }
                for mut c in self.order(cs) {
                    let b = f.branch.min(c.certainty);
                    let child = match c.via {
                        Via::Calls | Via::Dispatch if self.crosses(c.node, f.home) => {
                            c.via = Via::Crosses;
                            self.add(&c, b, true)
                        }
                        Via::Calls | Via::Dispatch => {
                            if !self.seen.insert((c.node, None)) {
                                continue;
                            }
                            let i = self.add(&c, b, false);
                            if let Some(i) = i {
                                next.push(Frontier {
                                    item: i,
                                    node: c.node,
                                    branch: b,
                                    spans: None,
                                    outside: self.other_handlers(c.node, None),
                                    home: f.home,
                                });
                            }
                            i
                        }
                        Via::Consumes => {
                            let i = self.add(&c, b, true);
                            if let Some(i) = i
                                && self.endpoints.insert(c.node)
                            {
                                if hop < self.opts.hops {
                                    let below = self.downstream(c.node, hop + 1, b);
                                    self.items[i].children.extend(below);
                                } else if self
                                    .v
                                    .outbound(c.node, EdgeKind::HandledBy)
                                    .next()
                                    .is_some()
                                {
                                    self.truncated.insert(Limit::Hops);
                                }
                            }
                            i
                        }
                        _ => self.add(&c, b, true),
                    };
                    match child {
                        Some(i) => self.items[f.item].children.push(i),
                        None => return,
                    }
                }
            }
            frontier = next;
            if frontier.is_empty() {
                return;
            }
        }
        let more = frontier.iter().any(|f| {
            [EdgeKind::Calls, EdgeKind::Queries, EdgeKind::Consumes]
                .iter()
                .any(|&k| self.v.outbound(f.node, k).next().is_some())
        });
        if more {
            self.truncated.insert(Limit::Depth);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Upstream (S17)

    /// The client sites consuming `endpoint` as roots, with their callers above them.
    fn upstream(&mut self, endpoint: i64, hop: usize, branch: Certainty) -> Vec<usize> {
        let sites = self.order(
            self.v
                .inbound(endpoint, EdgeKind::Consumes, Certainty::Ambiguous)
                .map(|e| Candidate {
                    via: Via::Consumes,
                    node: e.src,
                    certainty: e.certainty,
                    edge: Some(e),
                })
                .collect(),
        );
        let mut roots = Vec::new();
        let mut frontier = Vec::new();
        for c in sites {
            let b = branch.min(c.certainty);
            let Some(i) = self.add(&c, b, true) else {
                break;
            };
            roots.push(i);
            let reached_by: Vec<Span> = c
                .edge
                .map(|e| e.evidence.iter().map(|ev| ev.span).collect())
                .unwrap_or_default();
            if self
                .seen
                .insert((c.node, self.inline_scope(c.node, &reached_by)))
            {
                frontier.push(Frontier {
                    item: i,
                    node: c.node,
                    branch: b,
                    spans: Some(reached_by),
                    outside: Vec::new(),
                    home: None,
                });
            }
        }
        self.expand_up(frontier, hop);
        roots
    }

    fn expand_up(&mut self, mut frontier: Vec<Frontier>, hop: usize) {
        for _ in 0..self.opts.depth {
            let mut next = Vec::new();
            for f in frontier {
                let reached_by = f.spans.clone().unwrap_or_default();
                let mut cs = Vec::new();
                // The node handles another endpoint: the request entered there. An inline
                // handler handles it only within its spans (invariant 8).
                let mut inline: Vec<Span> = Vec::new();
                for e in self
                    .v
                    .inbound(f.node, EdgeKind::HandledBy, Certainty::Ambiguous)
                {
                    let spans = handler_spans(e);
                    let handles = spans.as_ref().is_none_or(|spans| {
                        reached_by
                            .iter()
                            .any(|ev| spans.iter().any(|s| within(*ev, *s)))
                    });
                    inline.extend(spans.into_iter().flatten());
                    if handles {
                        cs.push(Candidate {
                            via: Via::Handles,
                            node: e.src,
                            certainty: e.certainty,
                            edge: Some(e),
                        });
                    }
                }
                // Callers, and callers of the methods the node overrides (CHA, as `rb callers`).
                // Not when every row that reached the node lies in one of its inline handlers:
                // that code runs only when the handler's endpoint is requested, and calling the
                // node merely registers it (amendment 13, the upstream mirror of Q7).
                let in_handlers = !reached_by.is_empty()
                    && reached_by
                        .iter()
                        .all(|ev| inline.iter().any(|s| within(*ev, *s)));
                let overridden = if in_handlers {
                    Vec::new()
                } else {
                    self.v.overridden(f.node, Certainty::Ambiguous)
                };
                let callees = (!in_handlers).then_some((f.node, None));
                for (m, via_c) in callees
                    .into_iter()
                    .chain(overridden.into_iter().map(|(m, c)| (m, Some(c))))
                {
                    for e in self.v.inbound(m, EdgeKind::Calls, Certainty::Ambiguous) {
                        cs.push(match via_c {
                            None => Candidate {
                                via: Via::CalledBy,
                                node: e.src,
                                certainty: e.certainty,
                                edge: Some(e),
                            },
                            Some(c) => Candidate {
                                via: Via::Dispatch,
                                node: e.src,
                                certainty: e.certainty.min(c).min(Certainty::Dispatch),
                                edge: Some(e),
                            },
                        });
                    }
                }
                for c in self.order(cs) {
                    let b = f.branch.min(c.certainty);
                    let child = if c.via == Via::Handles {
                        let i = self.add(&c, b, true);
                        if let Some(i) = i
                            && self.endpoints.insert(c.node)
                        {
                            if hop < self.opts.hops {
                                let above = self.upstream(c.node, hop + 1, b);
                                self.items[i].children.extend(above);
                            } else if self
                                .v
                                .inbound(c.node, EdgeKind::Consumes, Certainty::Ambiguous)
                                .next()
                                .is_some()
                            {
                                self.truncated.insert(Limit::Hops);
                            }
                        }
                        i
                    } else {
                        let reached_by: Vec<Span> = c
                            .edge
                            .map(|e| e.evidence.iter().map(|ev| ev.span).collect())
                            .unwrap_or_default();
                        if !self
                            .seen
                            .insert((c.node, self.inline_scope(c.node, &reached_by)))
                        {
                            continue;
                        }
                        let i = self.add(&c, b, true);
                        if let Some(i) = i {
                            next.push(Frontier {
                                item: i,
                                node: c.node,
                                branch: b,
                                spans: Some(reached_by),
                                outside: Vec::new(),
                                home: None,
                            });
                        }
                        i
                    };
                    match child {
                        Some(i) => self.items[f.item].children.push(i),
                        None => return,
                    }
                }
            }
            frontier = next;
            if frontier.is_empty() {
                return;
            }
        }
        let more = frontier.iter().any(|f| {
            [EdgeKind::Calls, EdgeKind::HandledBy].iter().any(|&k| {
                self.v
                    .inbound(f.node, k, Certainty::Ambiguous)
                    .next()
                    .is_some()
            })
        });
        if more {
            self.truncated.insert(Limit::Depth);
        }
    }

    /// The tree below item `i`; with `collapse`, children whose subtrees reach no sink are
    /// hidden and counted. Returns the node, whether it reaches a sink (or must be shown), and
    /// the number of nodes in its subtree.
    fn tree(&self, i: usize, collapse: bool) -> (PathNode, bool, usize) {
        let item = &self.items[i];
        let mut children = Vec::new();
        let mut collapsed = 0;
        let mut reaches = item.keep;
        let mut size = 1;
        for &c in &item.children {
            let (child, child_reaches, child_size) = self.tree(c, collapse);
            size += child_size;
            reaches |= child_reaches;
            if collapse && !child_reaches {
                collapsed += child_size;
            } else {
                children.push(child);
            }
        }
        let node = PathNode {
            node: item.node,
            via: item.via,
            certainty: item.certainty,
            branch: item.branch,
            children,
            collapsed,
        };
        (node, reaches, size)
    }
}

#[cfg(test)]
mod tests {
    use rb_model::{
        Attrs, Certainty, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind, Provenance,
    };
    use serde_json::json;

    use super::*;

    /// A graph built by name: files belong to the service named by their first path segment,
    /// symbols are `path#name` declared at a line, endpoints `HTTP <name> @<service>`.
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
                let service = path.split('/').next().unwrap();
                self.b
                    .add_edge(NewEdge::new(
                        f.clone(),
                        EdgeKind::InService,
                        NodeKey::new(NodeKind::Service, service),
                        Provenance::Manifest,
                        Certainty::Exact,
                    ))
                    .unwrap();
            }
            f
        }

        /// `path#name`, declared at `line` (0-based).
        fn sym(&mut self, path: &str, name: &str, line: i32) -> NodeKey {
            let key = NodeKey::new(NodeKind::Symbol, format!("{path}#{name}"));
            if !self.b.has_node(&key) {
                self.b.add_node(key.clone(), name, Attrs::new()).unwrap();
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
                            span: [line, 0, line, 1],
                        }),
                    )
                    .unwrap();
            }
            key
        }

        fn endpoint(&mut self, name: &str, service: &str) -> NodeKey {
            let key = NodeKey::new(NodeKind::Endpoint, format!("HTTP {name} @{service}"));
            if !self.b.has_node(&key) {
                self.b.add_node(key.clone(), name, Attrs::new()).unwrap();
            }
            key
        }

        fn table(&mut self, name: &str) -> NodeKey {
            let key = NodeKey::new(NodeKind::Table, name);
            if !self.b.has_node(&key) {
                self.b.add_node(key.clone(), name, Attrs::new()).unwrap();
            }
            key
        }

        /// An edge with one evidence row at `(path, line)`.
        fn edge(
            &mut self,
            src: &NodeKey,
            kind: EdgeKind,
            dst: &NodeKey,
            c: Certainty,
            at: (&str, i32),
        ) {
            self.edge_attrs(src, kind, dst, c, at, Attrs::new());
        }

        fn edge_attrs(
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

        fn finish(self) -> Graph {
            self.b.finish().unwrap()
        }
    }

    use Certainty::{Ambiguous, Dispatch, Exact, Heuristic, Synthetic};
    use EdgeKind::{Calls, Consumes, HandledBy, Implements, Queries};

    /// `via name [certainty]` per node in tree order, indented, with `+k` for collapsed nodes
    /// and `~branch` when the branch is weaker than the step.
    fn lines(g: &Graph, nodes: &[PathNode]) -> Vec<String> {
        fn walk(g: &Graph, nodes: &[PathNode], depth: usize, out: &mut Vec<String>) {
            for n in nodes {
                let key = &g.nodes().iter().find(|x| x.id == n.node).unwrap().key;
                let short = key.rsplit_once('#').map_or(key.as_str(), |(_, s)| s);
                let mut s = format!(
                    "{}{} {short} [{}]",
                    "  ".repeat(depth),
                    n.via.as_str(),
                    n.certainty.as_str()
                );
                if n.branch < n.certainty {
                    s.push_str(&format!(" ~{}", n.branch.as_str()));
                }
                if n.collapsed > 0 {
                    s.push_str(&format!(" +{}", n.collapsed));
                }
                out.push(s);
                walk(g, &n.children, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(g, nodes, 0, &mut out);
        out
    }

    fn id(g: &Graph, key: &NodeKey) -> i64 {
        g.nodes()
            .iter()
            .find(|n| n.kind == key.kind && n.key == key.key)
            .unwrap()
            .id
    }

    /// The fixture plan's login path: web → auth (Java) → notification (TS inline handler),
    /// with a helper that reaches no sink, a call into another deployable service, a query
    /// outside the inline handler's span, and dispatch through an interface.
    fn login_graph() -> (Graph, NodeKey, NodeKey) {
        let mut g = G::new(&[
            ("web", "service"),
            ("auth", "service"),
            ("notification", "service"),
            ("billing", "service"),
            ("shared", "library"),
        ]);
        let login_ep = g.endpoint("POST /api/v1/login", "auth");
        let email_ep = g.endpoint("POST /internal/mfa/email", "notification");
        let on_submit = g.sym("web/Login.tsx", "onSubmit", 3);
        let web_login = g.sym("web/api.ts", "login", 5);
        g.edge(
            &on_submit,
            Calls,
            &web_login,
            Heuristic,
            ("web/Login.tsx", 7),
        );
        g.edge(
            &web_login,
            Consumes,
            &login_ep,
            Heuristic,
            ("web/api.ts", 6),
        );
        let ctl = g.sym("auth/Ctl.java", "login", 13);
        g.edge(&login_ep, HandledBy, &ctl, Exact, ("auth/Ctl.java", 12));
        let send = g.sym("auth/Mfa.java", "sendCode", 8);
        g.edge(&ctl, Calls, &send, Exact, ("auth/Ctl.java", 15));
        g.edge(&send, Consumes, &email_ep, Synthetic, ("auth/Mfa.java", 7));
        let hash = g.sym("shared/util.ts", "hash", 1);
        let pad = g.sym("shared/util.ts", "pad", 4);
        g.edge(&ctl, Calls, &hash, Heuristic, ("auth/Ctl.java", 16));
        g.edge(&hash, Calls, &pad, Heuristic, ("shared/util.ts", 2));
        let invoice = g.sym("billing/inv.go", "charge", 2);
        g.edge(&ctl, Calls, &invoice, Heuristic, ("auth/Ctl.java", 17));
        let svc = g.sym("auth/Svc.java", "Svc.audit", 2);
        let imp = g.sym("auth/SvcImpl.java", "SvcImpl.audit", 4);
        g.edge(&imp, Implements, &svc, Exact, ("auth/SvcImpl.java", 4));
        g.edge(&ctl, Calls, &svc, Exact, ("auth/Ctl.java", 18));
        let audit = g.table("public.audit");
        g.edge(&imp, Queries, &audit, Exact, ("auth/SvcImpl.java", 5));
        // The inline handler spans lines 6–9 of server.ts; a query at line 8 is inside, one at
        // line 20 (another handler's) is not.
        let server = g.file("notification/server.ts");
        let mut spans = Attrs::new();
        spans.insert("handler_spans".into(), json!([[6, 0, 9, 1]]));
        g.edge_attrs(
            &email_ep,
            HandledBy,
            &server,
            Exact,
            ("notification/server.ts", 6),
            spans,
        );
        let log = g.table("public.email_log");
        let other = g.table("public.other");
        g.edge(&server, Queries, &log, Exact, ("notification/server.ts", 8));
        g.b.add_edge(
            NewEdge::new(
                server.clone(),
                Queries,
                other.clone(),
                Provenance::SqlLiteral,
                Exact,
            )
            .evidence(Evidence {
                path: "notification/server.ts".into(),
                span: [20, 2, 20, 8],
            }),
        )
        .unwrap();
        (g.finish(), login_ep, email_ep)
    }

    #[test]
    fn the_login_path_spans_three_services() {
        let (g, login, _) = login_graph();
        let p = request_path(&g, id(&g, &login), PathOptions::default());
        assert_eq!(
            lines(&g, &p.upstream),
            [
                "consumes login [heuristic]",
                "  called_by onSubmit [heuristic]",
            ]
        );
        assert_eq!(
            lines(&g, &p.downstream),
            [
                "handler login [exact] +2",
                "  calls sendCode [exact]",
                "    consumes HTTP POST /internal/mfa/email @notification [synthetic]",
                "      handler notification/server.ts [exact] ~synthetic",
                "        queries public.email_log [exact] ~synthetic",
                "  calls Svc.audit [exact]",
                "    dispatch SvcImpl.audit [dispatch]",
                "      queries public.audit [exact] ~dispatch",
                "  crosses charge [heuristic]",
            ]
        );
        assert!(p.truncated.is_empty());
        // `--full` shows the helpers.
        let full = request_path(
            &g,
            id(&g, &login),
            PathOptions {
                full: true,
                ..PathOptions::default()
            },
        );
        let full = lines(&g, &full.downstream);
        assert!(
            full.contains(&"  calls hash [heuristic]".to_owned()),
            "{full:?}"
        );
        assert!(
            full.contains(&"    calls pad [heuristic]".to_owned()),
            "{full:?}"
        );
        assert_eq!(
            p.histogram,
            [(Exact, 6), (Dispatch, 1), (Synthetic, 1), (Heuristic, 3)]
        );
    }

    /// S17: upstream continues through an endpoint a node handles, and through an inline
    /// handler only from a site inside its spans.
    #[test]
    fn upstream_continues_through_handlers_and_inline_spans() {
        let (g, _, email) = login_graph();
        let p = request_path(&g, id(&g, &email), PathOptions::default());
        assert_eq!(
            lines(&g, &p.upstream),
            [
                "consumes sendCode [synthetic]",
                "  called_by login [exact] ~synthetic",
                "    handles HTTP POST /api/v1/login @auth [exact] ~synthetic",
                "      consumes login [heuristic]",
                "        called_by onSubmit [heuristic]",
            ]
        );
        let mut b = G::new(&[("a", "service"), ("n", "service"), ("x", "service")]);
        let target = b.endpoint("GET /t", "x");
        let entry = b.endpoint("POST /e", "n");
        let server = b.file("n/server.ts");
        let mut spans = Attrs::new();
        spans.insert("handler_spans".into(), json!([[6, 0, 9, 1]]));
        b.edge_attrs(&entry, HandledBy, &server, Exact, ("n/server.ts", 6), spans);
        // The top-level file consumes `/t` at line 7 (inside the handler) …
        b.edge(&server, Consumes, &target, Heuristic, ("n/server.ts", 7));
        let caller = b.sym("a/c.ts", "c", 1);
        b.edge(&caller, Consumes, &entry, Heuristic, ("a/c.ts", 2));
        let g = b.finish();
        assert_eq!(
            lines(
                &g,
                &request_path(&g, id(&g, &target), PathOptions::default()).upstream
            ),
            [
                "consumes n/server.ts [heuristic]",
                "  handles HTTP POST /e @n [exact] ~heuristic",
                "    consumes c [heuristic]",
            ]
        );
        // … and nothing continues from a site outside the spans.
        let mut b = G::new(&[("n", "service"), ("x", "service")]);
        let target = b.endpoint("GET /t", "x");
        let entry = b.endpoint("POST /e", "n");
        let server = b.file("n/server.ts");
        let mut spans = Attrs::new();
        spans.insert("handler_spans".into(), json!([[6, 0, 9, 1]]));
        b.edge_attrs(&entry, HandledBy, &server, Exact, ("n/server.ts", 6), spans);
        b.edge(&server, Consumes, &target, Heuristic, ("n/server.ts", 12));
        let g = b.finish();
        assert_eq!(
            lines(
                &g,
                &request_path(&g, id(&g, &target), PathOptions::default()).upstream
            ),
            ["consumes n/server.ts [heuristic]"]
        );
    }

    #[test]
    fn inputs_resolve_by_tier_and_never_pick() {
        let (g, login, email) = login_graph();
        let names = |ids: Vec<i64>| -> Vec<String> {
            ids.iter()
                .map(|&i| g.nodes().iter().find(|n| n.id == i).unwrap().key.clone())
                .collect()
        };
        assert_eq!(
            names(resolve_endpoints(&g, &login.key, None)),
            [login.key.as_str()]
        );
        assert_eq!(
            names(resolve_endpoints(
                &g,
                "post /API/v1//login/",
                Some("POST /api/v1/login")
            )),
            [login.key.as_str()]
        );
        // A case-insensitive substring of names; several are all listed.
        assert_eq!(
            names(resolve_endpoints(&g, "LOGIN", None)),
            [login.key.as_str()]
        );
        assert_eq!(
            names(resolve_endpoints(&g, "post", None)),
            [login.key.as_str(), email.key.as_str()]
        );
        // Symbol forms: a handler starts at its endpoints, a client at what it consumes.
        assert_eq!(
            names(resolve_endpoints(&g, "auth/Ctl.java#login", None)),
            [login.key.as_str()]
        );
        assert_eq!(
            names(resolve_endpoints(&g, "sendCode", None)),
            [email.key.as_str()]
        );
        assert!(resolve_endpoints(&g, "nothing", None).is_empty());
    }

    /// Every walk has a cycle test: a `CALLS` cycle, self-recursion, an upstream cycle and
    /// `CONSUMES` A ↔ B across services (the endpoint guard).
    #[test]
    fn cycles_terminate() {
        let mut b = G::new(&[("a", "service"), ("b", "service")]);
        let ea = b.endpoint("GET /a", "a");
        let eb = b.endpoint("GET /b", "b");
        let ha = b.sym("a/h.ts", "ha", 1);
        let hb = b.sym("b/h.ts", "hb", 1);
        let x = b.sym("a/h.ts", "x", 3);
        let y = b.sym("a/h.ts", "y", 5);
        b.edge(&ea, HandledBy, &ha, Exact, ("a/h.ts", 0));
        b.edge(&eb, HandledBy, &hb, Exact, ("b/h.ts", 0));
        b.edge(&ha, Calls, &x, Exact, ("a/h.ts", 2));
        b.edge(&x, Calls, &y, Exact, ("a/h.ts", 4));
        b.edge(&y, Calls, &x, Exact, ("a/h.ts", 6));
        b.edge(&y, Calls, &y, Exact, ("a/h.ts", 7));
        b.edge(&y, Consumes, &eb, Heuristic, ("a/h.ts", 8));
        b.edge(&hb, Consumes, &ea, Heuristic, ("b/h.ts", 2));
        b.edge(&hb, Calls, &hb, Exact, ("b/h.ts", 3));
        let g = b.finish();
        let p = request_path(
            &g,
            id(&g, &ea),
            PathOptions {
                depth: MAX_DEPTH,
                hops: MAX_HOPS,
                full: true,
                ..PathOptions::default()
            },
        );
        assert_eq!(
            lines(&g, &p.downstream),
            [
                "handler ha [exact]",
                "  calls x [exact]",
                "    calls y [exact]",
                "      consumes HTTP GET /b @b [heuristic]",
                "        handler hb [exact] ~heuristic",
                "          consumes HTTP GET /a @a [heuristic]",
            ]
        );
        assert_eq!(
            lines(&g, &p.upstream),
            [
                "consumes hb [heuristic]",
                "  handles HTTP GET /b @b [exact] ~heuristic",
                "    consumes y [heuristic]",
                "      called_by x [exact] ~heuristic",
                "        called_by ha [exact] ~heuristic",
                // The cycle closes at the starting endpoint, shown and not expanded again.
                "          handles HTTP GET /a @a [exact] ~heuristic",
            ]
        );
        assert!(p.truncated.is_empty(), "{:?}", p.truncated);
    }

    /// A chain of `n` calls under one handler, the last querying a table.
    fn chain(n: usize) -> (Graph, NodeKey) {
        let mut b = G::new(&[("s", "service")]);
        let e = b.endpoint("GET /c", "s");
        let mut prev = b.sym("s/c.ts", "f0", 0);
        b.edge(&e, HandledBy, &prev, Exact, ("s/c.ts", 0));
        for i in 1..=n {
            let next = b.sym("s/c.ts", &format!("f{i}"), i as i32);
            b.edge(&prev, Calls, &next, Exact, ("s/c.ts", i as i32));
            prev = next;
        }
        let t = b.table("t");
        b.edge(&prev, Queries, &t, Exact, ("s/c.ts", n as i32 + 1));
        (b.finish(), e)
    }

    fn depth(nodes: &[PathNode]) -> usize {
        nodes
            .iter()
            .map(|n| 1 + depth(&n.children))
            .max()
            .unwrap_or(0)
    }

    /// Contract M4b S10: one graph under both precisions. The handler calls `I.m`, implemented
    /// by `A.m` (its type proven never instantiated: `impl_instantiated: false`) and `B.m`.
    /// Downstream, `rta` skips the dispatch into `A.m` and counts it; upstream from the
    /// endpoint `A.m` consumes, the dispatch caller through `I.m` is skipped and counted.
    #[test]
    fn rta_prunes_dispatch_both_ways() {
        let mut b = G::new(&[("s", "service"), ("t", "service")]);
        let e = b.endpoint("GET /e", "s");
        let h = b.sym("s/h.java", "h", 1);
        let i = b.sym("s/i.java", "I.m", 1);
        let a = b.sym("s/a.java", "A.m", 1);
        let bm = b.sym("s/b.java", "B.m", 1);
        b.edge(&e, HandledBy, &h, Exact, ("s/h.java", 0));
        b.edge(&h, Calls, &i, Exact, ("s/h.java", 2));
        let mut flagged = Attrs::new();
        flagged.insert("impl_instantiated".into(), false.into());
        b.edge_attrs(&a, Implements, &i, Exact, ("s/a.java", 1), flagged);
        b.edge(&bm, Implements, &i, Exact, ("s/b.java", 1));
        let ta = b.table("ta");
        let tb = b.table("tb");
        b.edge(&a, Queries, &ta, Exact, ("s/a.java", 2));
        b.edge(&bm, Queries, &tb, Exact, ("s/b.java", 2));
        let remote = b.endpoint("GET /r", "t");
        b.edge(&a, Consumes, &remote, Heuristic, ("s/a.java", 3));
        let g = b.finish();
        let with = |precision| PathOptions {
            precision,
            ..PathOptions::default()
        };
        assert_eq!(PathOptions::default().precision, Precision::Rta, "D5");
        let rta = request_path(&g, id(&g, &e), with(Precision::Rta));
        assert_eq!(
            lines(&g, &rta.downstream),
            [
                "handler h [exact]",
                "  calls I.m [exact]",
                "    dispatch B.m [dispatch]",
                "      queries tb [exact] ~dispatch",
            ]
        );
        assert_eq!((rta.precision, rta.pruned_by_rta), (Precision::Rta, 1));
        let cha = request_path(&g, id(&g, &e), with(Precision::Cha));
        assert_eq!(
            lines(&g, &cha.downstream),
            [
                "handler h [exact]",
                "  calls I.m [exact]",
                "    dispatch A.m [dispatch]",
                "      queries ta [exact] ~dispatch",
                "      consumes HTTP GET /r @t [heuristic]",
                "    dispatch B.m [dispatch]",
                "      queries tb [exact] ~dispatch",
            ]
        );
        assert_eq!((cha.precision, cha.pruned_by_rta), (Precision::Cha, 0));
        // Upstream of `GET /r`: `A.m` consumes it, and `h` calls `I.m`, which `A.m` overrides.
        let up = |p| {
            let r = request_path(&g, id(&g, &remote), with(p));
            (lines(&g, &r.upstream), r.pruned_by_rta)
        };
        assert_eq!(
            up(Precision::Rta),
            (vec!["consumes A.m [heuristic]".to_owned()], 1)
        );
        assert_eq!(
            up(Precision::Cha),
            (
                vec![
                    "consumes A.m [heuristic]".to_owned(),
                    "  dispatch h [dispatch] ~heuristic".to_owned(),
                    "    handles HTTP GET /e @s [exact] ~heuristic".to_owned(),
                ],
                0
            )
        );
    }

    #[test]
    fn a_5000_deep_chain_is_cut_at_the_depth() {
        let (g, e) = chain(5000);
        for d in [DEFAULT_DEPTH, MAX_DEPTH] {
            let p = request_path(
                &g,
                id(&g, &e),
                PathOptions {
                    depth: d,
                    hops: 0,
                    full: true,
                    ..PathOptions::default()
                },
            );
            assert_eq!(depth(&p.downstream), d + 1, "the handler and {d} levels");
            assert_eq!(p.truncated, BTreeSet::from([Limit::Depth]));
        }
        // Collapsed, the cut chain reaches no sink: only the handler shows.
        let p = request_path(&g, id(&g, &e), PathOptions::default());
        assert_eq!(
            lines(&g, &p.downstream),
            [format!("handler f0 [exact] +{DEFAULT_DEPTH}")]
        );
        // Within the depth the table is reached and nothing is cut.
        let (g, e) = chain(10);
        let p = request_path(&g, id(&g, &e), PathOptions::default());
        assert!(p.truncated.is_empty());
        assert_eq!(depth(&p.downstream), 12);
    }

    #[test]
    fn hops_and_nodes_are_capped() {
        // Ten services, each endpoint's handler consuming the next one's.
        let services: Vec<(String, &str)> = (0..10).map(|i| (format!("s{i}"), "service")).collect();
        let refs: Vec<(&str, &str)> = services.iter().map(|(s, r)| (s.as_str(), *r)).collect();
        let mut b = G::new(&refs);
        let eps: Vec<NodeKey> = (0..10)
            .map(|i| b.endpoint("GET /x", &format!("s{i}")))
            .collect();
        for i in 0..10 {
            let h = b.sym(&format!("s{i}/h.ts"), "h", 0);
            b.edge(&eps[i], HandledBy, &h, Exact, (&format!("s{i}/h.ts"), 0));
            if i + 1 < 10 {
                b.edge(
                    &h,
                    Consumes,
                    &eps[i + 1],
                    Heuristic,
                    (&format!("s{i}/h.ts"), 1),
                );
            }
        }
        let g = b.finish();
        let hops_shown = |p: &RequestPath| {
            lines(&g, &p.downstream)
                .iter()
                .filter(|l| l.trim_start().starts_with("consumes"))
                .count()
        };
        let p = request_path(&g, id(&g, &eps[0]), PathOptions::default());
        assert_eq!(
            hops_shown(&p),
            DEFAULT_HOPS + 1,
            "4 expanded hops and the endpoint beyond"
        );
        assert_eq!(p.truncated, BTreeSet::from([Limit::Hops]));
        let p = request_path(
            &g,
            id(&g, &eps[0]),
            PathOptions {
                hops: 99,
                ..PathOptions::default()
            },
        );
        assert_eq!(hops_shown(&p), MAX_HOPS + 1, "--hops is capped at 8");

        // 6,000 queries from one handler: 5,000 nodes and `nodes`.
        let mut b = G::new(&[("s", "service")]);
        let e = b.endpoint("GET /wide", "s");
        let h = b.sym("s/h.ts", "h", 0);
        b.edge(&e, HandledBy, &h, Exact, ("s/h.ts", 0));
        for i in 0..6000 {
            let t = b.table(&format!("t{i:04}"));
            b.edge(&h, Queries, &t, Exact, ("s/h.ts", 1));
        }
        let g = b.finish();
        let p = request_path(&g, id(&g, &e), PathOptions::default());
        let shown = 1 + p.downstream[0].children.len();
        assert_eq!(shown, NODE_CAP);
        assert_eq!(p.truncated, BTreeSet::from([Limit::Nodes]));
    }

    /// A handler fanning out to `n` helpers that each query a table, and `n` callers of one
    /// client of the endpoint.
    fn fan(n: usize) -> (Graph, NodeKey) {
        let mut b = G::new(&[("s", "service"), ("w", "service")]);
        let e = b.endpoint("GET /f", "s");
        let h = b.sym("s/h.ts", "h", 0);
        b.edge(&e, HandledBy, &h, Exact, ("s/h.ts", 0));
        let client = b.sym("w/c.ts", "client", 0);
        b.edge(&client, Consumes, &e, Heuristic, ("w/c.ts", 1));
        for i in 0..n {
            let f = b.sym("s/h.ts", &format!("f{i}"), i as i32 + 1);
            b.edge(&h, Calls, &f, Exact, ("s/h.ts", i as i32 + 1));
            let t = b.table(&format!("t{i}"));
            b.edge(&f, Queries, &t, Exact, ("s/h.ts", i as i32 + 1));
            let c = b.sym("w/c.ts", &format!("c{i}"), i as i32 + 2);
            b.edge(&c, Calls, &client, Exact, ("w/c.ts", i as i32 + 2));
        }
        (b.finish(), e)
    }

    /// Invariant 11: four times a fan-out (and fan-in), or a chain, makes at most five times
    /// the adjacency work (linear is four, quadratic sixteen). Counted, not timed, so machine
    /// load cannot fail it (contract M4a amendment 12, Q4).
    #[test]
    fn rb_path_grows_linearly() {
        let work = |g: &Graph, e: &NodeKey| {
            let e = id(g, e);
            crate::work::take();
            request_path(
                g,
                e,
                PathOptions {
                    depth: MAX_DEPTH,
                    hops: MAX_HOPS,
                    full: false,
                    ..PathOptions::default()
                },
            );
            crate::work::take()
        };
        for (small, large) in [(fan(500), fan(2000)), (chain(500), chain(2000))] {
            let (a, b) = (work(&small.0, &small.1), work(&large.0, &large.1));
            assert!(a > 0 && b <= 5 * a, "{a} → {b}");
        }
    }

    /// Evaluator pass 1, P4: two endpoints handled inline in one file each walk their own
    /// span-scoped downstream, so the second endpoint's table is not dropped because the file
    /// was already walked for the first.
    #[test]
    fn inline_handlers_in_one_file_each_walk_their_own_scope() {
        let mut b = G::new(&[("auth", "service"), ("notify", "service")]);
        let login = b.endpoint("POST /login", "auth");
        let email = b.endpoint("POST /email", "notify");
        let sms = b.endpoint("POST /sms", "notify");
        let ctl = b.sym("auth/a.ts", "login", 1);
        b.edge(&login, HandledBy, &ctl, Exact, ("auth/a.ts", 0));
        let both = b.sym("auth/a.ts", "sendBoth", 5);
        b.edge(&ctl, Calls, &both, Exact, ("auth/a.ts", 2));
        b.edge(&both, Consumes, &email, Synthetic, ("auth/a.ts", 6));
        b.edge(&both, Consumes, &sms, Synthetic, ("auth/a.ts", 7));
        let server = b.file("notify/server.ts");
        for (ep, span) in [(&email, [2, 0, 5, 1]), (&sms, [7, 0, 10, 1])] {
            let mut spans = Attrs::new();
            spans.insert("handler_spans".into(), json!([span]));
            b.edge_attrs(
                ep,
                HandledBy,
                &server,
                Exact,
                ("notify/server.ts", span[0]),
                spans,
            );
        }
        let email_log = b.table("public.email_log");
        let sms_log = b.table("public.sms_log");
        b.edge(&server, Queries, &email_log, Exact, ("notify/server.ts", 3));
        b.edge(&server, Queries, &sms_log, Exact, ("notify/server.ts", 8));
        let g = b.finish();
        let p = request_path(&g, id(&g, &login), PathOptions::default());
        assert_eq!(
            lines(&g, &p.downstream),
            [
                "handler login [exact]",
                "  calls sendBoth [exact]",
                "    consumes HTTP POST /email @notify [synthetic]",
                "      handler notify/server.ts [exact] ~synthetic",
                "        queries public.email_log [exact] ~synthetic",
                "    consumes HTTP POST /sms @notify [synthetic]",
                "      handler notify/server.ts [exact] ~synthetic",
                "        queries public.sms_log [exact] ~synthetic",
            ]
        );
        assert!(p.truncated.is_empty(), "{:?}", p.truncated);
    }

    /// The upstream counterpart of P4: one file reached from two inline handlers (through two
    /// helpers consuming the endpoint) continues into each handler's endpoint.
    #[test]
    fn upstream_reaches_each_inline_handler_of_one_file() {
        let mut b = G::new(&[("a", "service"), ("n", "service"), ("x", "service")]);
        let target = b.endpoint("GET /t", "x");
        let e1 = b.endpoint("POST /e1", "n");
        let e2 = b.endpoint("POST /e2", "n");
        let server = b.file("n/server.ts");
        for (ep, span) in [(&e1, [2, 0, 5, 1]), (&e2, [7, 0, 10, 1])] {
            let mut spans = Attrs::new();
            spans.insert("handler_spans".into(), json!([span]));
            b.edge_attrs(
                ep,
                HandledBy,
                &server,
                Exact,
                ("n/server.ts", span[0]),
                spans,
            );
        }
        let mail = b.sym("n/lib.ts", "sendMail", 1);
        let text = b.sym("n/lib.ts", "sendSms", 5);
        b.edge(&server, Calls, &mail, Heuristic, ("n/server.ts", 3));
        b.edge(&server, Calls, &text, Heuristic, ("n/server.ts", 8));
        b.edge(&mail, Consumes, &target, Heuristic, ("n/lib.ts", 2));
        b.edge(&text, Consumes, &target, Heuristic, ("n/lib.ts", 6));
        let c1 = b.sym("a/c.ts", "c1", 1);
        let c2 = b.sym("a/c.ts", "c2", 5);
        b.edge(&c1, Consumes, &e1, Heuristic, ("a/c.ts", 2));
        b.edge(&c2, Consumes, &e2, Heuristic, ("a/c.ts", 6));
        let g = b.finish();
        let p = request_path(&g, id(&g, &target), PathOptions::default());
        assert_eq!(
            lines(&g, &p.upstream),
            [
                "consumes sendMail [heuristic]",
                "  called_by n/server.ts [heuristic]",
                "    handles HTTP POST /e1 @n [exact] ~heuristic",
                "      consumes c1 [heuristic]",
                "consumes sendSms [heuristic]",
                "  called_by n/server.ts [heuristic]",
                "    handles HTTP POST /e2 @n [exact] ~heuristic",
                "      consumes c2 [heuristic]",
            ]
        );
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q7): a function reached by `CALLS` that
    /// registers inline handlers of other endpoints is walked outside those handlers' spans:
    /// calling it registers them, it does not run them.
    #[test]
    fn a_called_registrar_is_walked_outside_its_inline_handlers() {
        let mut b = G::new(&[("loop", "service"), ("auth", "service")]);
        let a = b.endpoint("GET /a", "loop");
        let bb = b.endpoint("GET /b", "loop");
        let login = b.endpoint("POST /login", "auth");
        let fa = b.sym("loop/s.ts", "fa", 1);
        let fb = b.sym("loop/s.ts", "fb", 7);
        for (ep, f, span) in [(&a, &fa, [2, 0, 5, 1]), (&bb, &fb, [8, 0, 12, 1])] {
            let mut spans = Attrs::new();
            spans.insert("handler_spans".into(), json!([span]));
            b.edge_attrs(ep, HandledBy, f, Exact, ("loop/s.ts", span[0]), spans);
        }
        let a_log = b.table("a_log");
        let b_log = b.table("b_log");
        let setup_log = b.table("setup_log");
        let setup = b.sym("loop/s.ts", "setup", 14);
        b.edge(&fa, Calls, &fb, Heuristic, ("loop/s.ts", 3));
        b.edge(&fa, Queries, &a_log, Exact, ("loop/s.ts", 4));
        b.edge(&fb, Queries, &b_log, Exact, ("loop/s.ts", 9));
        b.edge(&fb, Consumes, &login, Heuristic, ("loop/s.ts", 10));
        b.edge(&fb, Calls, &setup, Heuristic, ("loop/s.ts", 13));
        b.edge(&setup, Queries, &setup_log, Exact, ("loop/s.ts", 15));
        let g = b.finish();
        let p = request_path(&g, id(&g, &a), PathOptions::default());
        assert_eq!(
            lines(&g, &p.downstream),
            [
                "handler fa [exact]",
                "  queries a_log [exact]",
                "  calls fb [heuristic]",
                "    calls setup [heuristic]",
                "      queries setup_log [exact] ~heuristic",
            ]
        );
        // `/b` itself still walks its handler's span.
        let p = request_path(&g, id(&g, &bb), PathOptions::default());
        let got = lines(&g, &p.downstream);
        assert!(
            got.iter().any(|l| l.contains("queries b_log"))
                && got.iter().any(|l| l.contains("consumes HTTP POST /login")),
            "{got:?}"
        );
        // Upstream, the mirror (amendment 13; evaluator pass 3, R5): the site inside `/b`'s
        // inline handler is entered only at `GET /b`; `fa` calling `fb` registers `/b`, it does
        // not run the call, so `fa` and `GET /a` are no entries.
        let p = request_path(&g, id(&g, &login), PathOptions::default());
        assert_eq!(
            lines(&g, &p.upstream),
            [
                "consumes fb [heuristic]",
                "  handles HTTP GET /b @loop [exact] ~heuristic",
            ]
        );
    }

    /// Evaluator pass 2, Q5 (M21): an upstream root is keyed by its inline-handler scope, so
    /// the same file reached again within that scope is not walked a second time.
    #[test]
    fn an_upstream_root_reached_again_in_its_scope_is_walked_once() {
        let mut b = G::new(&[("n", "service"), ("x", "service")]);
        let target = b.endpoint("GET /t", "x");
        let a = b.endpoint("POST /a", "n");
        let server = b.file("n/server.ts");
        let mut spans = Attrs::new();
        spans.insert("handler_spans".into(), json!([[2, 0, 5, 1]]));
        b.edge_attrs(&a, HandledBy, &server, Exact, ("n/server.ts", 2), spans);
        let helper = b.sym("n/lib.ts", "g", 1);
        b.edge(&server, Consumes, &target, Heuristic, ("n/server.ts", 3));
        b.edge(&server, Calls, &helper, Heuristic, ("n/server.ts", 4));
        b.edge(&helper, Consumes, &target, Heuristic, ("n/lib.ts", 2));
        let g = b.finish();
        let p = request_path(&g, id(&g, &target), PathOptions::default());
        assert_eq!(
            lines(&g, &p.upstream),
            [
                "consumes g [heuristic]",
                "consumes n/server.ts [heuristic]",
                "  handles HTTP POST /a @n [exact] ~heuristic",
            ]
        );
    }

    /// Evaluator pass 1, P5: upstream stops at the hop cap, and a chain of callers deeper than
    /// `--depth` is cut and named in `truncated`.
    #[test]
    fn upstream_hops_and_depth_are_capped() {
        // Ten services, each endpoint consumed by the handler of the next one's.
        let services: Vec<(String, &str)> = (0..10).map(|i| (format!("s{i}"), "service")).collect();
        let refs: Vec<(&str, &str)> = services.iter().map(|(s, r)| (s.as_str(), *r)).collect();
        let mut b = G::new(&refs);
        let eps: Vec<NodeKey> = (0..10)
            .map(|i| b.endpoint("GET /x", &format!("s{i}")))
            .collect();
        for i in 0..10 {
            let h = b.sym(&format!("s{i}/h.ts"), "h", 0);
            b.edge(&eps[i], HandledBy, &h, Exact, (&format!("s{i}/h.ts"), 0));
            if i > 0 {
                b.edge(
                    &h,
                    Consumes,
                    &eps[i - 1],
                    Heuristic,
                    (&format!("s{i}/h.ts"), 1),
                );
            }
        }
        let g = b.finish();
        let entered = |p: &RequestPath| {
            lines(&g, &p.upstream)
                .iter()
                .filter(|l| l.trim_start().starts_with("handles"))
                .count()
        };
        let p = request_path(&g, id(&g, &eps[0]), PathOptions::default());
        assert_eq!(
            entered(&p),
            DEFAULT_HOPS + 1,
            "4 expanded hops and the endpoint beyond"
        );
        assert_eq!(p.truncated, BTreeSet::from([Limit::Hops]));
        let p = request_path(
            &g,
            id(&g, &eps[0]),
            PathOptions {
                hops: 99,
                ..PathOptions::default()
            },
        );
        assert_eq!(entered(&p), MAX_HOPS + 1, "--hops is capped at 8");

        // A client called through a chain of 100 callers.
        let mut b = G::new(&[("s", "service"), ("w", "service")]);
        let e = b.endpoint("GET /c", "s");
        let mut prev = b.sym("w/c.ts", "c0", 0);
        b.edge(&prev, Consumes, &e, Heuristic, ("w/c.ts", 0));
        for i in 1..=100 {
            let caller = b.sym("w/c.ts", &format!("c{i}"), i);
            b.edge(&caller, Calls, &prev, Exact, ("w/c.ts", i));
            prev = caller;
        }
        let g = b.finish();
        let p = request_path(&g, id(&g, &e), PathOptions::default());
        assert_eq!(
            depth(&p.upstream),
            DEFAULT_DEPTH + 1,
            "the client and 12 callers"
        );
        assert_eq!(p.truncated, BTreeSet::from([Limit::Depth]));
        let (g, e) = chain(3);
        assert!(
            request_path(&g, id(&g, &e), PathOptions::default())
                .truncated
                .is_empty()
        );
    }

    /// Evaluator pass 1, suspected item 2: an Express service without a start script is
    /// `role = library`, yet it serves requests; a call into the code of a service that exposes
    /// endpoints crosses into it (D8) whatever its role, while a library exposing none is walked
    /// in-process.
    #[test]
    fn a_library_role_service_that_exposes_endpoints_is_crossed_into() {
        let mut b = G::new(&[
            ("auth", "service"),
            ("notify", "library"),
            ("shared", "library"),
        ]);
        let login = b.endpoint("POST /login", "auth");
        let email = b.endpoint("POST /email", "notify");
        let notify = NodeKey::new(NodeKind::Service, "notify");
        b.edge(
            &notify,
            EdgeKind::Exposes,
            &email,
            Exact,
            ("notify/server.ts", 1),
        );
        let ctl = b.sym("auth/a.ts", "login", 1);
        b.edge(&login, HandledBy, &ctl, Exact, ("auth/a.ts", 0));
        let send = b.sym("notify/mail.ts", "send", 1);
        let hash = b.sym("shared/u.ts", "hash", 1);
        let t = b.table("t");
        b.edge(&ctl, Calls, &send, Heuristic, ("auth/a.ts", 2));
        b.edge(&ctl, Calls, &hash, Heuristic, ("auth/a.ts", 3));
        b.edge(&hash, Queries, &t, Exact, ("shared/u.ts", 2));
        let g = b.finish();
        let p = request_path(&g, id(&g, &login), PathOptions::default());
        assert_eq!(
            lines(&g, &p.downstream),
            [
                "handler login [exact]",
                "  crosses send [heuristic]",
                "  calls hash [heuristic]",
                "    queries t [exact] ~heuristic",
            ]
        );
    }

    /// Ambiguous handlers are each a root; the weakest step bounds the branch below.
    #[test]
    fn ambiguous_handlers_are_each_a_root() {
        let mut b = G::new(&[("s", "service")]);
        let e = b.endpoint("GET /a", "s");
        let h1 = b.sym("s/a.ts", "a1", 0);
        let h2 = b.sym("s/b.ts", "a2", 0);
        let t = b.table("t");
        b.edge(&e, HandledBy, &h1, Ambiguous, ("s/a.ts", 0));
        b.edge(&e, HandledBy, &h2, Ambiguous, ("s/a.ts", 0));
        b.edge(&h2, Queries, &t, Exact, ("s/b.ts", 1));
        let g = b.finish();
        let p = request_path(&g, id(&g, &e), PathOptions::default());
        assert_eq!(
            lines(&g, &p.downstream),
            [
                "handler a1 [ambiguous]",
                "handler a2 [ambiguous]",
                "  queries t [exact] ~ambiguous",
            ]
        );
    }
}
