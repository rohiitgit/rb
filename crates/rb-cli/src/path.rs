//! `rb path` (Q4, design §10.3, contract M4a S15–S18): choosing the endpoint and rendering the
//! request path as text or JSON. The walks live in `rb_store::path`.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::bail;
use rb_frameworks::endpoint::{endpoint_name, norm_method, norm_path};
use rb_model::{Attrs, EdgeKind, Graph, Node, NodeKind};
use rb_store::code::Describer;
use rb_store::path::{PathNode, RequestPath, Via};
use serde_json::{Value, json};

/// `<METHOD> <path>` normalised as routes are (S3), when the input has that form.
pub(crate) fn normalised(input: &str) -> Option<String> {
    let (method, path) = input.trim().split_once(char::is_whitespace)?;
    let path = path.trim();
    let is_method = !method.is_empty() && method.chars().all(|c| c.is_ascii_alphabetic());
    (is_method && !path.is_empty()).then(|| endpoint_name(&norm_method(method), &norm_path(path)))
}

/// Node lookups and descriptions for one graph.
struct Ctx<'g> {
    graph: &'g Graph,
    by_id: HashMap<i64, &'g Node>,
    /// The attributes of each `CONSUMES` edge, by `(client, endpoint)`.
    consumes: HashMap<(i64, i64), &'g Attrs>,
    describer: Describer<'g>,
}

impl<'g> Ctx<'g> {
    fn new(graph: &'g Graph) -> Self {
        Ctx {
            graph,
            by_id: graph.nodes().iter().map(|n| (n.id, n)).collect(),
            consumes: graph
                .edges()
                .iter()
                .filter(|e| e.kind == EdgeKind::Consumes)
                .map(|e| ((e.src, e.dst), &e.attrs))
                .collect(),
            describer: Describer::new(graph),
        }
    }

    fn node(&self, id: i64) -> &'g Node {
        self.by_id[&id]
    }

    /// How a `CONSUMES` step was stitched (its edge's attributes): upstream the client is the
    /// node and the endpoint its parent, downstream the other way round.
    fn stitch(&self, parent: Option<i64>, n: &PathNode, upstream: bool) -> Option<&'g Attrs> {
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
    fn endpoint_service(&self, id: i64) -> String {
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
    fn shown(&self, parent: Option<i64>, n: &PathNode) -> Shown {
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

struct Shown {
    label: String,
    display: String,
    path: String,
    line: i32,
    service: String,
}

/// The endpoint `input` names. Several candidates are listed as `N. <name>  [<service>]`, plus
/// `  (prefix unknown, router <file>#<binding>)` for an endpoint keyed by its router (contract M4a
/// amendment 10), never picked silently, unless `pick` (1-based, in the listed order) chooses
/// one (S15).
pub(crate) fn pick_endpoint(
    graph: &Graph,
    input: &str,
    pick: Option<usize>,
) -> anyhow::Result<i64> {
    let candidates = rb_store::path::resolve_endpoints(graph, input, normalised(input).as_deref());
    let ctx = Ctx::new(graph);
    match (candidates.as_slice(), pick) {
        ([], _) => bail!("no endpoint matches {input:?} (try `rb find {input} --kind endpoint`)"),
        ([only], _) => Ok(*only),
        (many, Some(n)) if (1..=many.len()).contains(&n) => Ok(many[n - 1]),
        (many, _) => {
            let list: Vec<String> = many
                .iter()
                .enumerate()
                .map(|(i, &id)| {
                    let node = ctx.node(id);
                    let router = node.attrs.get("router").and_then(Value::as_str);
                    format!(
                        "  {}. {}  [{}]{}",
                        i + 1,
                        node.name,
                        ctx.endpoint_service(id),
                        router.map_or_else(String::new, |r| format!(
                            "  (prefix unknown, router {r})"
                        ))
                    )
                })
                .collect();
            bail!(
                "{input:?} matches {} endpoints; pass --pick N:\n{}",
                many.len(),
                list.join("\n")
            )
        }
    }
}

/// `  (via host gateway, suffix match, 2 candidates)`: the parts of a stitch that qualify it
/// (S22), or nothing.
fn stitch_note(attrs: Option<&Attrs>) -> String {
    let Some(attrs) = attrs else {
        return String::new();
    };
    let text = |k: &str| attrs.get(k).and_then(Value::as_str);
    let mut parts = Vec::new();
    if let Some(h) = text("via_host") {
        parts.push(format!("via host {h}"));
    }
    if let Some(h) = text("host") {
        parts.push(format!("host {h}"));
    }
    if text("match") == Some("suffix") {
        parts.push("suffix match".to_owned());
    }
    if let Some(n) = attrs.get("candidates").and_then(Value::as_u64) {
        parts.push(format!("{n} candidates"));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!("  ({})", parts.join(", "))
    }
}

/// `[step]`, or `[step, branch weaker]` when the branch is weaker than the step.
fn certainty(n: &PathNode) -> String {
    if n.branch < n.certainty {
        format!("[{}, branch {}]", n.certainty.as_str(), n.branch.as_str())
    } else {
        format!("[{}]", n.certainty.as_str())
    }
}

fn render_nodes(
    ctx: &Ctx<'_>,
    parent: Option<i64>,
    nodes: &[PathNode],
    depth: usize,
    upstream: bool,
    out: &mut String,
) {
    for n in nodes {
        let s = ctx.shown(parent, n);
        let line = match (upstream, n.via) {
            (true, Via::Consumes) => format!("client {}", s.label),
            (true, Via::Dispatch) => format!("← (dispatch) {}", s.label),
            (true, Via::Handles) => format!("⇐ entered at {}", s.label),
            (true, _) => format!("← {}", s.label),
            (false, Via::Handler) if parent.is_some() => format!("⇒ {}", s.label),
            (false, Via::Handler) => s.label,
            (false, Via::Dispatch) => format!("→ (dispatch) {}", s.label),
            (false, Via::Queries) => format!("→ QUERIES {}", s.label),
            (false, Via::Consumes) => format!("→ CONSUMES {}", s.label),
            (false, Via::Crosses) => {
                format!("→ {}  crosses into {} (not followed)", s.label, s.service)
            }
            (false, _) => format!("→ {}", s.label),
        };
        let _ = writeln!(
            out,
            "{}{line}  {}{}",
            "  ".repeat(depth),
            certainty(n),
            stitch_note(ctx.stitch(parent, n, upstream))
        );
        render_nodes(ctx, Some(n.node), &n.children, depth + 1, upstream, out);
        if n.collapsed > 0 {
            let calls = if n.collapsed == 1 { "call" } else { "calls" };
            let _ = writeln!(
                out,
                "{}+ {} helper {calls}",
                "  ".repeat(depth + 1),
                n.collapsed
            );
        }
    }
}

pub(crate) fn render_text(graph: &Graph, path: &RequestPath) -> String {
    let ctx = Ctx::new(graph);
    let e = ctx.node(path.endpoint);
    let service = ctx.endpoint_service(path.endpoint);
    let mut out = format!("Request path: {}  [{service}]\n", e.name);
    out.push_str(&crate::code::precision_lines(
        path.precision,
        path.pruned_by_rta,
    ));
    out.push_str(
        "Statically possible paths from the call graph and HTTP stitching, not a trace of one request.\n",
    );
    out.push_str("UPSTREAM\n");
    if path.upstream.is_empty() {
        out.push_str("  (no client calls found)\n");
    }
    render_nodes(&ctx, Some(path.endpoint), &path.upstream, 1, true, &mut out);
    let _ = writeln!(out, "ENDPOINT  {}  [{service}]", e.name);
    out.push_str("DOWNSTREAM\n");
    if path.downstream.is_empty() {
        out.push_str("  (no handler found)\n");
    }
    render_nodes(&ctx, None, &path.downstream, 1, false, &mut out);
    let parts: Vec<String> = path
        .histogram
        .iter()
        .map(|(c, n)| format!("{} {n}", c.as_str()))
        .collect();
    let _ = writeln!(
        out,
        "certainty: {}",
        if parts.is_empty() {
            "-".to_owned()
        } else {
            parts.join(", ")
        }
    );
    if !path.truncated.is_empty() {
        let limits: Vec<&str> = path.truncated.iter().map(|l| l.as_str()).collect();
        let _ = writeln!(out, "truncated: {}", limits.join(", "));
    }
    out
}

fn json_nodes(ctx: &Ctx<'_>, parent: Option<i64>, nodes: &[PathNode], upstream: bool) -> Value {
    nodes
        .iter()
        .map(|n| {
            let s = ctx.shown(parent, n);
            json!({
                "key": ctx.node(n.node).key,
                "display": s.display,
                "path": s.path,
                "line": s.line,
                "service": s.service,
                "certainty": n.certainty.as_str(),
                "branch": n.branch.as_str(),
                "via": n.via.as_str(),
                "stitch": ctx.stitch(parent, n, upstream),
                "children": json_nodes(ctx, Some(n.node), &n.children, upstream),
                "collapsed": n.collapsed,
            })
        })
        .collect()
}

/// The S18 JSON: `{endpoint, precision, pruned_by_rta, upstream, downstream, histogram,
/// truncated}` (contract M4a S18, M4b S10).
pub(crate) fn render_json(graph: &Graph, path: &RequestPath) -> Value {
    let ctx = Ctx::new(graph);
    let e = ctx.node(path.endpoint);
    json!({
        "endpoint": {
            "key": e.key,
            "name": e.name,
            "service": ctx.endpoint_service(path.endpoint),
        },
        "precision": path.precision.as_str(),
        "pruned_by_rta": path.pruned_by_rta,
        "upstream": json_nodes(&ctx, Some(path.endpoint), &path.upstream, true),
        "downstream": json_nodes(&ctx, None, &path.downstream, false),
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

    /// Amendment 10: endpoints that share a name on different routers (prefix unknown) are
    /// listed with their router, and each is reachable by `--pick` or by its full key.
    #[test]
    fn prefix_unknown_candidates_show_their_router() {
        use rb_model::GraphBuilder;
        let mut b = GraphBuilder::new();
        b.add_node(
            rb_model::NodeKey::new(NodeKind::Service, "s"),
            "s",
            Attrs::new(),
        )
        .unwrap();
        let keys = [
            "HTTP GET / @s",
            "HTTP GET / @s ?router=a.go#ArticlesRegister.router",
            "HTTP GET / @s ?router=t.go#TagsRegister.router",
        ];
        for k in keys {
            let mut attrs = json!({"service": "s"}).as_object().cloned().unwrap();
            if let Some((_, r)) = k.split_once(" ?router=") {
                attrs.insert("prefix_unknown".into(), true.into());
                attrs.insert("router".into(), r.into());
            }
            b.add_node(
                rb_model::NodeKey::new(NodeKind::Endpoint, k),
                "GET /",
                attrs,
            )
            .unwrap();
        }
        let graph = b.finish().unwrap();
        let id = |k: &str| graph.nodes().iter().find(|n| n.key == k).unwrap().id;
        let err = pick_endpoint(&graph, "GET /", None)
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "\"GET /\" matches 3 endpoints; pass --pick N:\n  1. GET /  [s]\n  2. GET /  [s]  (prefix unknown, router a.go#ArticlesRegister.router)\n  3. GET /  [s]  (prefix unknown, router t.go#TagsRegister.router)"
        );
        assert_eq!(
            pick_endpoint(&graph, "GET /", Some(3)).unwrap(),
            id(keys[2])
        );
        assert_eq!(pick_endpoint(&graph, keys[1], None).unwrap(), id(keys[1]));
    }

    /// S22: a `CONSUMES` step says how it was stitched (the host it named, the proxy it went
    /// through, a suffix match, the number of candidates), in text and in JSON, both upstream
    /// (the client under the endpoint) and downstream (the endpoint under the client).
    #[test]
    fn consumes_steps_show_how_they_were_stitched() {
        use rb_model::{Attrs, Certainty, Evidence, GraphBuilder, NewEdge, Provenance};
        let mut b = GraphBuilder::new();
        let key = |kind, k: &str| rb_model::NodeKey::new(kind, k);
        for s in ["w", "s"] {
            let mut attrs = Attrs::new();
            attrs.insert("role".into(), "service".into());
            b.add_node(key(NodeKind::Service, s), s, attrs).unwrap();
        }
        let add = |b: &mut GraphBuilder, kind, k: &str, name: &str, attrs: Value| {
            b.add_node(key(kind, k), name, attrs.as_object().cloned().unwrap())
                .unwrap();
        };
        add(&mut b, NodeKind::File, "w/c.ts", "c.ts", json!({}));
        add(&mut b, NodeKind::File, "s/h.ts", "h.ts", json!({}));
        let x = "HTTP POST /x @s";
        let y = "HTTP GET /y @w";
        add(
            &mut b,
            NodeKind::Endpoint,
            x,
            "POST /x",
            json!({"service": "s"}),
        );
        add(
            &mut b,
            NodeKind::Endpoint,
            y,
            "GET /y",
            json!({"service": "w"}),
        );
        for (name, file) in [("a", "w/c.ts"), ("d", "w/c.ts"), ("h", "s/h.ts")] {
            add(
                &mut b,
                NodeKind::Symbol,
                name,
                name,
                json!({"kind": "function"}),
            );
            let e = NewEdge::new(
                key(NodeKind::Symbol, name),
                EdgeKind::DefinedIn,
                key(NodeKind::File, file),
                Provenance::Treesitter,
                Certainty::Exact,
            )
            .evidence(Evidence {
                path: file.into(),
                span: [0, 9, 0, 10],
            });
            b.add_edge(e).unwrap();
        }
        for (file, s) in [("w/c.ts", "w"), ("s/h.ts", "s")] {
            let e = NewEdge::new(
                key(NodeKind::File, file),
                EdgeKind::InService,
                key(NodeKind::Service, s),
                Provenance::Manifest,
                Certainty::Exact,
            );
            b.add_edge(e).unwrap();
        }
        let consumes = |src: &str, c, attrs: Value| {
            NewEdge::new(
                key(NodeKind::Symbol, src),
                EdgeKind::Consumes,
                key(NodeKind::Endpoint, x),
                Provenance::HttpStitch,
                c,
            )
            .attrs(attrs.as_object().cloned().unwrap())
            .evidence(Evidence {
                path: "w/c.ts".into(),
                span: [1, 0, 1, 5],
            })
        };
        b.add_edge(consumes(
            "a",
            Certainty::Heuristic,
            json!({"match": "exact", "via_host": "gateway", "sites": 1}),
        ))
        .unwrap();
        b.add_edge(consumes(
            "d",
            Certainty::Ambiguous,
            json!({"match": "suffix", "host": "s", "candidates": 2, "sites": 1}),
        ))
        .unwrap();
        for (ep, h, file) in [(y, "a", "w/c.ts"), (x, "h", "s/h.ts")] {
            let e = NewEdge::new(
                key(NodeKind::Endpoint, ep),
                EdgeKind::HandledBy,
                key(NodeKind::Symbol, h),
                Provenance::Treesitter,
                Certainty::Exact,
            )
            .evidence(Evidence {
                path: file.into(),
                span: [0, 0, 0, 1],
            });
            b.add_edge(e).unwrap();
        }
        let graph = b.finish().unwrap();
        let id = |k: &str| graph.nodes().iter().find(|n| n.key == k).unwrap().id;
        let opts = rb_store::path::PathOptions {
            depth: rb_store::path::DEFAULT_DEPTH,
            hops: rb_store::path::DEFAULT_HOPS,
            full: false,
            precision: rb_store::code::Precision::Rta,
        };

        let up = rb_store::path::request_path(&graph, id(x), opts);
        let text = render_text(&graph, &up);
        assert!(
            text.contains("  client a  w/c.ts:1  [w]  [heuristic]  (via host gateway)\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "  client d  w/c.ts:1  [w]  [ambiguous]  (host s, suffix match, 2 candidates)\n"
            ),
            "{text}"
        );
        // Only `CONSUMES` steps carry a stitch: the handler line does not.
        assert!(text.contains("  h  s/h.ts:1  [s]  [exact]\n"), "{text}");
        let json = render_json(&graph, &up);
        assert_eq!(json["upstream"][0]["stitch"]["via_host"], "gateway");
        assert_eq!(json["upstream"][1]["stitch"]["candidates"], 2);
        assert_eq!(json["downstream"][0]["stitch"], Value::Null);

        let down = rb_store::path::request_path(&graph, id(y), opts);
        let text = render_text(&graph, &down);
        assert!(
            text.contains("→ CONSUMES POST /x  [s]  [heuristic]  (via host gateway)\n"),
            "{text}"
        );
        let json = render_json(&graph, &down);
        assert_eq!(
            json["downstream"][0]["children"][0]["stitch"]["match"],
            "exact"
        );
    }
}
