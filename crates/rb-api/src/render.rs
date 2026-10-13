//! Choosing the node a request names, and the JSON of each answer (contract M5 S12: moved from
//! `rb-cli` without behaviour change). The CLI's text renderings use the same lookups.

use std::collections::HashMap;
use std::fmt::Write as _;

use rb_frameworks::endpoint::{endpoint_name, norm_method, norm_path};
use rb_model::{Attrs, EdgeKind, Graph, Node, NodeKind};
use rb_store::code::{Caller, CallerTree, Describer, Impact, Impacted, Orphans, SnapshotIndex};
use rb_store::path::{PathNode, RequestPath, Via};
use serde_json::{Value, json};

use crate::error::ApiError;

/// `display  path:line  [service]`: how a node is listed (callers, impact, candidates).
pub fn describe_line(d: &Describer<'_>, id: i64) -> String {
    let info = d.describe(id);
    // A file (top-level code) is its own location.
    let is_file = info.display == info.path;
    let mut s = if is_file {
        format!("{} (top-level code)", info.display)
    } else {
        info.display
    };
    if !info.path.is_empty() && !is_file {
        let _ = write!(s, "  {}:{}", info.path, info.line);
    }
    if let Some(service) = info.service {
        let _ = write!(s, "  [{service}]");
    }
    s
}

/// The node `input` names (Q1, Q2). Several candidates are listed, never picked silently, unless
/// `pick` (1-based, in the listed order) chooses one.
pub fn pick_target(ix: &SnapshotIndex, input: &str, pick: Option<usize>) -> Result<i64, ApiError> {
    let mut candidates = rb_store::code::resolve_target(ix, input);
    if candidates.is_empty() {
        candidates = near_miss_symbols(ix, input);
    }
    let d = Describer::new(ix);
    match (candidates.as_slice(), pick) {
        ([], _) => Err(ApiError::not_found(format!(
            "nothing matches {input:?} (try `rb find {}`)",
            shell_arg(input)
        ))),
        ([only], None | Some(1)) => Ok(*only),
        ([_], Some(n)) => Err(pick_beyond_one(input, n)),
        (many, Some(n)) if (1..=many.len()).contains(&n) => Ok(many[n - 1]),
        (many, _) => {
            let list: Vec<String> = many
                .iter()
                .enumerate()
                .map(|(i, &id)| format!("  {}. {}", i + 1, describe_line(&d, id)))
                .collect();
            Err(ApiError::ambiguous(
                format!(
                    "{input:?} matches {} symbols; pass --pick N:\n{}",
                    many.len(),
                    list.join("\n")
                ),
                many.iter()
                    .map(|&id| node_json(ix.graph(), &d, id))
                    .collect(),
            ))
        }
    }
}

/// `--pick N` beyond the only candidate (contract M7j E6): refused, never ignored. Beyond several
/// candidates the request lists them instead (M5 amendment 5).
fn pick_beyond_one(input: &str, n: usize) -> ApiError {
    ApiError::invalid(
        "pick",
        format!("is {n}, but only 1 candidate matches {input:?}"),
    )
}

/// Symbols a near-miss key names (contract M7f S4): `rb find`'s rule — a case-insensitive
/// substring of a symbol's key or name — for an input written in symbol-key syntax (`#` or `(`),
/// in key order. Plain words already resolve by name, and stay out so a short word never lists
/// half the graph.
fn near_miss_symbols(ix: &SnapshotIndex, input: &str) -> Vec<i64> {
    if !input.contains(['#', '(']) {
        return Vec::new();
    }
    let needle = input.trim().to_lowercase();
    let mut hits: Vec<(&str, i64)> = ix
        .graph()
        .nodes()
        .iter()
        .filter(|n| n.kind == NodeKind::Symbol)
        .filter(|n| {
            n.key.to_lowercase().contains(&needle) || n.name.to_lowercase().contains(&needle)
        })
        .map(|n| (n.key.as_str(), n.id))
        .collect();
    hits.sort_unstable();
    hits.into_iter().map(|(_, id)| id).collect()
}

/// `arg` as a shell word (contract M7f S5): as written when it holds only characters no shell
/// splits or expands, else single-quoted.
pub fn shell_arg(arg: &str) -> String {
    let plain = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_-./:@%+=,".contains(c));
    if plain {
        arg.to_owned()
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// The name of the service exposing endpoint `id` (its key when no node names it).
pub fn endpoint_service(ix: &SnapshotIndex, id: i64) -> String {
    let graph = ix.graph();
    let Some(e) = ix.get_node(id) else {
        return String::new();
    };
    let key = e
        .attrs
        .get("service")
        .and_then(|s| s.as_str())
        .unwrap_or_default();
    graph
        .nodes()
        .iter()
        .find(|n| n.kind == NodeKind::Service && n.key == key)
        .map_or(key, |n| n.name.as_str())
        .to_owned()
}

/// A node as JSON: its key and how it is shown.
pub fn node_json(graph: &Graph, d: &Describer<'_>, id: i64) -> Value {
    let info = d.describe(id);
    let key = graph
        .nodes()
        .iter()
        .find(|n| n.id == id)
        .map(|n| n.key.as_str());
    json!({
        "key": key,
        "display": info.display,
        "path": info.path,
        "line": info.line,
        "service": info.service,
    })
}

/// `rb callers --json` (M4b open item 10, contract M5 S11): the target, then the caller tree as
/// the text shows it, each caller with its certainty, how it was reached and its `entry` marks.
pub fn callers_json(ix: &SnapshotIndex, tree: &CallerTree) -> Value {
    fn walk(graph: &Graph, d: &Describer<'_>, callers: &[Caller]) -> Vec<Value> {
        callers
            .iter()
            .map(|c| {
                let mut v = node_json(graph, d, c.node);
                v["certainty"] = c.certainty.as_str().into();
                v["dispatch"] = c.dispatch.into();
                v["reference"] = c.reference.into();
                v["entry"] = c.entry.clone().into();
                v["callers"] = walk(graph, d, &c.callers).into();
                v
            })
            .collect()
    }
    let graph = ix.graph();
    let d = Describer::new(ix);
    let mut out = json!({
        "target": node_json(graph, &d, tree.target),
        "precision": tree.precision.as_str(),
        "pruned_by_rta": tree.pruned_by_rta,
        "callers": walk(graph, &d, &tree.callers),
        "truncated": tree.truncated.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
    });
    // Only when the target has marks, so other answers keep their bytes (contract M7i S2).
    if !tree.target_entry.is_empty() {
        out["target_entry"] = tree.target_entry.clone().into();
    }
    // Only when asked (`exclude_tests`) or when `limit` cut the rows (contract M7i S4).
    if let Some(n) = tree.excluded_tests {
        out["excluded_tests"] = n.into();
    }
    if let Some(s) = &tree.summary {
        let counts = |xs: &[(String, usize)], name: &str| -> Vec<Value> {
            xs.iter()
                .map(|(k, n)| json!({ name: k, "callers": n }))
                .collect()
        };
        out["summary"] = json!({
            "callers": s.callers,
            "shown": s.shown,
            "by_service": counts(&s.by_service, "service"),
            "by_entry": counts(&s.by_entry, "entry"),
        });
    }
    out
}

/// `rb impact --json` (contract M4b S13): every section of the text answer, with keys.
pub fn impact_json(ix: &SnapshotIndex, impact: &Impact) -> Value {
    let graph = ix.graph();
    let d = Describer::new(ix);
    let impacted = |xs: &[Impacted]| -> Vec<Value> {
        xs.iter()
            .map(|x| {
                let mut v = node_json(graph, &d, x.node);
                v["distance"] = x.distance.into();
                v["certainty"] = x.certainty.as_str().into();
                v
            })
            .collect()
    };
    let endpoints: Vec<Value> = impact
        .endpoints
        .iter()
        .map(|x| {
            let node = graph.nodes().iter().find(|n| n.id == x.node);
            json!({
                "key": node.map(|n| n.key.as_str()),
                "name": node.map(|n| n.name.as_str()),
                "service": endpoint_service(ix, x.node),
                "router": node.and_then(|n| n.attrs.get("router")),
                "distance": x.distance,
                "certainty": x.certainty.as_str(),
                "handler_removed": x.distance == 1,
            })
        })
        .collect();
    let orphans = match &impact.orphans {
        Orphans::NotComputed(reason) => json!({"computed": false, "reason": reason}),
        Orphans::Computed(list) => json!({
            "computed": true,
            "nodes": list
                .iter()
                .map(|o| {
                    let mut v = node_json(graph, &d, o.node);
                    v["distance"] = o.distance.into();
                    v["via"] = json!({
                        "key": node_json(graph, &d, o.via.0)["key"],
                        "how": o.via.1,
                    });
                    v
                })
                .collect::<Vec<_>>(),
        }),
    };
    let mut out = json!({
        "removed": impact
            .removed
            .iter()
            .map(|&n| node_json(graph, &d, n))
            .collect::<Vec<_>>(),
        "precision": impact.precision.as_str(),
        "pruned_by_rta": impact.pruned_by_rta,
        "breaks": impacted(&impact.breaks),
        "affects": impacted(&impact.affects),
        "endpoints": endpoints,
        "orphans": orphans,
        "tests": impacted(&impact.tests),
        "services": impact
            .services
            .iter()
            .map(|(key, n, own)| json!({"key": key, "impacted": n, "own": own}))
            .collect::<Vec<_>>(),
        "owners": impact
            .owners
            .iter()
            .map(|(o, n)| json!({"owner": o, "impacted": n}))
            .collect::<Vec<_>>(),
        "truncated": impact.truncated.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
    });
    // The full length of each list `limit` cut, only when one was (contract M7i S4).
    if !impact.totals.is_empty() {
        out["totals"] = json!(impact.totals);
    }
    out
}

/// `<METHOD> <path>` normalised as routes are (contract M4a S3), when the input has that form.
pub fn normalised(input: &str) -> Option<String> {
    let (method, path) = input.trim().split_once(char::is_whitespace)?;
    let path = path.trim();
    let is_method = !method.is_empty() && method.chars().all(|c| c.is_ascii_alphabetic());
    (is_method && !path.is_empty()).then(|| endpoint_name(&norm_method(method), &norm_path(path)))
}

/// `  (prefix unknown, router <file>#<binding>)` for an endpoint keyed by its router (contract
/// M4a amendment 10, M4b S13 item 7), else nothing.
pub fn router_note(e: &Node) -> String {
    e.attrs
        .get("router")
        .and_then(Value::as_str)
        .map_or_else(String::new, |r| format!("  (prefix unknown, router {r})"))
}

/// Node lookups and descriptions for a request path's graph (Q4).
pub struct PathView<'g> {
    graph: &'g Graph,
    by_id: HashMap<i64, &'g Node>,
    /// The attributes of each `CONSUMES` edge, by `(client, endpoint)`.
    consumes: HashMap<(i64, i64), &'g Attrs>,
    describer: Describer<'g>,
}

/// How a request-path node is shown: its text label and its JSON fields.
pub struct Shown {
    pub label: String,
    pub display: String,
    pub path: String,
    pub line: i32,
    pub service: String,
}

impl<'g> PathView<'g> {
    pub fn new(ix: &'g SnapshotIndex) -> Self {
        let graph = ix.graph();
        PathView {
            graph,
            by_id: graph.nodes().iter().map(|n| (n.id, n)).collect(),
            consumes: graph
                .edges()
                .iter()
                .filter(|e| e.kind == EdgeKind::Consumes)
                .map(|e| ((e.src, e.dst), &e.attrs))
                .collect(),
            describer: Describer::new(ix),
        }
    }

    pub fn node(&self, id: i64) -> &'g Node {
        self.by_id[&id]
    }

    /// How a `CONSUMES` step was stitched (its edge's attributes): upstream the client is the
    /// node and the endpoint its parent, downstream the other way round.
    pub fn stitch(&self, parent: Option<i64>, n: &PathNode, upstream: bool) -> Option<&'g Attrs> {
        if n.via != Via::Consumes {
            return None;
        }
        let parent = parent?;
        let pair = if upstream {
            (n.node, parent)
        } else {
            (parent, n.node)
        };
        self.consumes.get(&pair).copied()
    }

    /// The name of the service exposing an endpoint (its key when no node names it).
    pub fn endpoint_service(&self, id: i64) -> String {
        let key = self.node(id).attrs["service"].as_str().unwrap_or_default();
        self.graph
            .nodes()
            .iter()
            .find(|n| n.kind == NodeKind::Service && n.key == key)
            .map_or_else(|| key.to_owned(), |n| n.name.clone())
    }

    /// How a tree node is shown: its label (an endpoint's name, a table with the operations the
    /// parent performs on it, a symbol with its location, a file as top-level code), plus the
    /// JSON fields.
    pub fn shown(&self, parent: Option<i64>, n: &PathNode) -> Shown {
        let node = self.node(n.node);
        match node.kind {
            NodeKind::Endpoint => {
                let service = self.endpoint_service(n.node);
                Shown {
                    label: format!("{}  [{service}]", node.name),
                    display: node.name.clone(),
                    path: String::new(),
                    line: 0,
                    service,
                }
            }
            NodeKind::Table => {
                let ops: Vec<String> = parent
                    .and_then(|p| {
                        self.graph
                            .edges()
                            .iter()
                            .find(|e| e.src == p && e.dst == n.node && e.kind == EdgeKind::Queries)
                    })
                    .and_then(|e| e.attrs.get("ops")?.as_array().cloned())
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|o| o.as_str().map(str::to_owned))
                    .collect();
                let label = if ops.is_empty() {
                    node.key.clone()
                } else {
                    format!("{} ({})", node.key, ops.join(", "))
                };
                Shown {
                    label,
                    display: node.key.clone(),
                    path: String::new(),
                    line: 0,
                    service: String::new(),
                }
            }
            _ => {
                let info = self.describer.describe(n.node);
                let service = info.service.unwrap_or_default();
                let mut label = if node.kind == NodeKind::File {
                    format!("{} (top-level code)", info.display)
                } else {
                    format!("{}  {}:{}", info.display, info.path, info.line)
                };
                if !service.is_empty() {
                    let _ = write!(label, "  [{service}]");
                }
                Shown {
                    label,
                    display: info.display,
                    path: info.path,
                    line: info.line,
                    service,
                }
            }
        }
    }
}

/// The endpoint `input` names (Q4). Several candidates are listed as `N. <name>  [<service>]`,
/// plus the router of an endpoint keyed by it (contract M4a amendment 10), never picked silently,
/// unless `pick` (1-based, in the listed order) chooses one (M4a S15).
pub fn pick_endpoint(
    ix: &SnapshotIndex,
    input: &str,
    pick: Option<usize>,
) -> Result<i64, ApiError> {
    let candidates = rb_store::path::resolve_endpoints(ix, input, normalised(input).as_deref());
    let view = PathView::new(ix);
    match (candidates.as_slice(), pick) {
        ([], _) => Err(ApiError::not_found(format!(
            "no endpoint matches {input:?} (try `rb find {} --kind endpoint`)",
            shell_arg(input)
        ))),
        ([only], None | Some(1)) => Ok(*only),
        ([_], Some(n)) => Err(pick_beyond_one(input, n)),
        (many, Some(n)) if (1..=many.len()).contains(&n) => Ok(many[n - 1]),
        (many, _) => {
            let list: Vec<String> = many
                .iter()
                .enumerate()
                .map(|(i, &id)| {
                    let node = view.node(id);
                    format!(
                        "  {}. {}  [{}]{}",
                        i + 1,
                        node.name,
                        view.endpoint_service(id),
                        router_note(node)
                    )
                })
                .collect();
            Err(ApiError::ambiguous(
                format!(
                    "{input:?} matches {} endpoints; pass --pick N:\n{}",
                    many.len(),
                    list.join("\n")
                ),
                many.iter().map(|&id| endpoint_json(&view, id)).collect(),
            ))
        }
    }
}

/// An endpoint as JSON: key, name, service name and the router of a prefix-unknown one.
fn endpoint_json(view: &PathView<'_>, id: i64) -> Value {
    let e = view.node(id);
    json!({
        "key": e.key,
        "name": e.name,
        "service": view.endpoint_service(id),
        "router": e.attrs.get("router"),
    })
}

fn path_nodes(
    view: &PathView<'_>,
    parent: Option<i64>,
    nodes: &[PathNode],
    upstream: bool,
) -> Value {
    nodes
        .iter()
        .map(|n| {
            let s = view.shown(parent, n);
            json!({
                "key": view.node(n.node).key,
                "display": s.display,
                "path": s.path,
                "line": s.line,
                "service": s.service,
                "certainty": n.certainty.as_str(),
                "branch": n.branch.as_str(),
                "via": n.via.as_str(),
                "stitch": view.stitch(parent, n, upstream),
                "children": path_nodes(view, Some(n.node), &n.children, upstream),
                "collapsed": n.collapsed,
            })
        })
        .collect()
}

/// `rb path` as JSON: `{endpoint, precision, pruned_by_rta, upstream, downstream, histogram,
/// truncated}` (contract M4a S18, M4b S10).
pub fn path_json(ix: &SnapshotIndex, path: &RequestPath) -> Value {
    let view = PathView::new(ix);
    json!({
        "endpoint": endpoint_json(&view, path.endpoint),
        "precision": path.precision.as_str(),
        "pruned_by_rta": path.pruned_by_rta,
        "upstream": path_nodes(&view, Some(path.endpoint), &path.upstream, true),
        "downstream": path_nodes(&view, None, &path.downstream, false),
        "histogram": path
            .histogram
            .iter()
            .map(|(c, n)| json!({ "certainty": c.as_str(), "count": n }))
            .collect::<Vec<_>>(),
        "truncated": path.truncated.iter().map(|l| l.as_str()).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;
    use rb_model::{Certainty, Evidence, GraphBuilder, NewEdge, NodeKey, Provenance};

    #[test]
    fn method_and_path_inputs_are_normalised_like_routes() {
        assert_eq!(
            normalised("post /api/v1//login/").as_deref(),
            Some("POST /api/v1/login")
        );
        assert_eq!(
            normalised("GET /users/:id").as_deref(),
            Some("GET /users/{}")
        );
        assert_eq!(normalised("login"), None);
        assert_eq!(normalised("AuthController.login x"), None);
        assert_eq!(normalised("POST "), None);
    }

    /// Contract M5 S12: an ambiguous target lists its candidates in the error's `candidates`
    /// in the order of the CLI's numbered text, and nothing matching is `not_found`.
    #[test]
    fn ambiguous_targets_list_candidates_in_the_texts_order() {
        let mut b = GraphBuilder::new();
        let mut attrs = Attrs::new();
        attrs.insert("role".into(), "service".into());
        b.add_node(NodeKey::new(NodeKind::Service, "s"), "svc", attrs)
            .unwrap();
        for (file, sym, line) in [
            ("b/B.java", "b/B.java/B#run().", 3),
            ("a/A.java", "a/A.java/A#run().", 7),
        ] {
            let f = NodeKey::new(NodeKind::File, file);
            b.add_node(f.clone(), file, Attrs::new()).unwrap();
            b.add_edge(NewEdge::new(
                f.clone(),
                EdgeKind::InService,
                NodeKey::new(NodeKind::Service, "s"),
                Provenance::Manifest,
                Certainty::Exact,
            ))
            .unwrap();
            b.add_node(NodeKey::new(NodeKind::Symbol, sym), "run", Attrs::new())
                .unwrap();
            b.add_edge(
                NewEdge::new(
                    NodeKey::new(NodeKind::Symbol, sym),
                    EdgeKind::DefinedIn,
                    f,
                    Provenance::Treesitter,
                    Certainty::Exact,
                )
                .evidence(Evidence {
                    path: file.into(),
                    span: [line, 0, line, 3],
                }),
            )
            .unwrap();
        }
        let ix = SnapshotIndex::new(b.finish().unwrap());
        let e = pick_target(&ix, "run", None).unwrap_err();
        assert_eq!(e.kind, ErrorKind::Ambiguous);
        assert_eq!(
            e.message,
            "\"run\" matches 2 symbols; pass --pick N:\n  1. a/A.java/A#run().  a/A.java:8  [svc]\n  2. b/B.java/B#run().  b/B.java:4  [svc]"
        );
        let keys: Vec<&str> = e
            .candidates
            .iter()
            .map(|c| c["key"].as_str().unwrap())
            .collect();
        assert_eq!(keys, ["a/A.java/A#run().", "b/B.java/B#run()."]);
        assert_eq!(e.candidates[0]["line"], 8);
        let second = pick_target(&ix, "run", Some(2)).unwrap();
        assert_eq!(ix.node(second).key, "b/B.java/B#run().");
        // A pick beyond the list is no choice: the candidates again.
        assert_eq!(
            pick_target(&ix, "run", Some(3)).unwrap_err().kind,
            ErrorKind::Ambiguous
        );
        let none = pick_target(&ix, "walk", None).unwrap_err();
        assert_eq!(
            (none.kind, none.message.as_str()),
            (
                ErrorKind::NotFound,
                "nothing matches \"walk\" (try `rb find walk`)"
            )
        );
        // Contract M7f S4 (stress F7): a near-miss key (`A#run()` without the trailing `.`)
        // resolves when `find`'s rule — a case-insensitive substring of a symbol's key or name —
        // yields exactly one symbol; several are ambiguous, as everywhere.
        let near = pick_target(&ix, "A#run()", None).unwrap();
        assert_eq!(ix.node(near).key, "a/A.java/A#run().");
        assert_eq!(pick_target(&ix, "a#RUN()", None).unwrap(), near);
        assert_eq!(
            pick_target(&ix, "#run(", None).unwrap_err().kind,
            ErrorKind::Ambiguous
        );
        // A plain fragment is no key syntax: it stays not found, never a substring sweep.
        assert_eq!(
            pick_target(&ix, "un", None).unwrap_err().kind,
            ErrorKind::NotFound
        );
        // Contract M7f S5 (stress F8): the suggested command quotes what a shell would split.
        let miss = pick_target(&ix, "Z#run()", None).unwrap_err();
        assert_eq!(
            miss.message,
            "nothing matches \"Z#run()\" (try `rb find 'Z#run()'`)"
        );
    }
}
