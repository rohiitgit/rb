//! `rb path` (Q4, design §10.3, contract M4a S15–S18): choosing the endpoint and rendering the
//! request path as text or JSON. The walks live in `rb_store::path`.

use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::bail;
use rb_frameworks::endpoint::{endpoint_name, norm_method, norm_path};
use rb_model::{EdgeKind, Graph, Node, NodeKind};
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
    describer: Describer<'g>,
}

impl<'g> Ctx<'g> {
    fn new(graph: &'g Graph) -> Self {
        Ctx {
            graph,
            by_id: graph.nodes().iter().map(|n| (n.id, n)).collect(),
            describer: Describer::new(graph),
        }
    }

    fn node(&self, id: i64) -> &'g Node {
        self.by_id[&id]
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

/// The endpoint `input` names. Several candidates are listed as `N. <name>  [<service>]`, never
/// picked silently, unless `pick` (1-based, in the listed order) chooses one (S15).
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
                    format!(
                        "  {}. {}  [{}]",
                        i + 1,
                        ctx.node(id).name,
                        ctx.endpoint_service(id)
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
        let _ = writeln!(out, "{}{line}  {}", "  ".repeat(depth), certainty(n));
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

fn json_nodes(ctx: &Ctx<'_>, parent: Option<i64>, nodes: &[PathNode]) -> Value {
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
                "children": json_nodes(ctx, Some(n.node), &n.children),
                "collapsed": n.collapsed,
            })
        })
        .collect()
}

/// The S18 JSON: `{endpoint, upstream, downstream, histogram, truncated}`.
pub(crate) fn render_json(graph: &Graph, path: &RequestPath) -> Value {
    let ctx = Ctx::new(graph);
    let e = ctx.node(path.endpoint);
    json!({
        "endpoint": {
            "key": e.key,
            "name": e.name,
            "service": ctx.endpoint_service(path.endpoint),
        },
        "upstream": json_nodes(&ctx, Some(path.endpoint), &path.upstream),
        "downstream": json_nodes(&ctx, None, &path.downstream),
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
}
