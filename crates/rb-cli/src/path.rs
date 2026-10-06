//! `rb path` (Q4, design §10.3, contract M4a S15–S18): the request path as text. The walks live
//! in `rb_store::path`; choosing the endpoint and the JSON live in `rb_api::render`.

use std::fmt::Write as _;

use rb_api::render::{PathView, router_note};
use rb_model::Attrs;
use rb_store::code::SnapshotIndex;
use rb_store::path::{PathNode, RequestPath, Via};
use serde_json::Value;

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
    ctx: &PathView<'_>,
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

pub(crate) fn render_text(ix: &SnapshotIndex, path: &RequestPath) -> String {
    let ctx = PathView::new(ix);
    let e = ctx.node(path.endpoint);
    let service = ctx.endpoint_service(path.endpoint);
    let mut out = format!("Request path: {}  [{service}]{}\n", e.name, router_note(e));
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

#[cfg(test)]
mod tests {
    use super::*;
    use rb_api::render::{path_json as render_json, pick_endpoint};
    use rb_model::{EdgeKind, NodeKind};
    use serde_json::json;

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
        let err = pick_endpoint(
            &rb_store::code::SnapshotIndex::new(graph.clone()),
            "GET /",
            None,
        )
        .unwrap_err()
        .message;
        assert_eq!(
            err,
            "\"GET /\" matches 3 endpoints; pass --pick N:\n  1. GET /  [s]\n  2. GET /  [s]  (prefix unknown, router a.go#ArticlesRegister.router)\n  3. GET /  [s]  (prefix unknown, router t.go#TagsRegister.router)"
        );
        assert_eq!(
            pick_endpoint(
                &rb_store::code::SnapshotIndex::new(graph.clone()),
                "GET /",
                Some(3)
            )
            .unwrap(),
            id(keys[2])
        );
        assert_eq!(
            pick_endpoint(
                &rb_store::code::SnapshotIndex::new(graph.clone()),
                keys[1],
                None
            )
            .unwrap(),
            id(keys[1])
        );
        // The request path's header and JSON name the router too (M4b S13 item 7).
        let path = rb_store::path::request_path(
            &rb_store::code::SnapshotIndex::new(graph.clone()),
            id(keys[1]),
            rb_store::path::PathOptions::default(),
        );
        assert!(
            render_text(&rb_store::code::SnapshotIndex::new(graph.clone()), &path).starts_with(
                "Request path: GET /  [s]  (prefix unknown, router a.go#ArticlesRegister.router)\n"
            ),
            "{}",
            render_text(&rb_store::code::SnapshotIndex::new(graph.clone()), &path)
        );
        assert_eq!(
            render_json(&rb_store::code::SnapshotIndex::new(graph.clone()), &path)["endpoint"]["router"],
            "a.go#ArticlesRegister.router"
        );
        let plain = rb_store::path::request_path(
            &rb_store::code::SnapshotIndex::new(graph.clone()),
            id(keys[0]),
            rb_store::path::PathOptions::default(),
        );
        assert!(
            render_text(&rb_store::code::SnapshotIndex::new(graph.clone()), &plain)
                .starts_with("Request path: GET /  [s]\n")
        );
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
            caps: Default::default(),
        };

        let up = rb_store::path::request_path(
            &rb_store::code::SnapshotIndex::new(graph.clone()),
            id(x),
            opts,
        );
        let text = render_text(&rb_store::code::SnapshotIndex::new(graph.clone()), &up);
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
        let json = render_json(&rb_store::code::SnapshotIndex::new(graph.clone()), &up);
        assert_eq!(json["upstream"][0]["stitch"]["via_host"], "gateway");
        assert_eq!(json["upstream"][1]["stitch"]["candidates"], 2);
        assert_eq!(json["downstream"][0]["stitch"], Value::Null);

        let down = rb_store::path::request_path(
            &rb_store::code::SnapshotIndex::new(graph.clone()),
            id(y),
            opts,
        );
        let text = render_text(&rb_store::code::SnapshotIndex::new(graph.clone()), &down);
        assert!(
            text.contains("→ CONSUMES POST /x  [s]  [heuristic]  (via host gateway)\n"),
            "{text}"
        );
        let json = render_json(&rb_store::code::SnapshotIndex::new(graph.clone()), &down);
        assert_eq!(
            json["downstream"][0]["children"][0]["stitch"]["match"],
            "exact"
        );
    }
}
