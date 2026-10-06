//! `rb callers` (Q1) and `rb impact` (Q2): choosing the target and rendering the answers
//! (design §9.1–§9.3). The questions themselves live in `rb_store::code`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::bail;
use rb_model::{Certainty, Graph};
use rb_store::code::{Caller, CallerTree, Describer, Impact, Impacted, Orphans, Precision};
use serde_json::{Value, json};

pub(crate) fn parse_certainty(s: &str) -> Result<Certainty, String> {
    [
        Certainty::Ambiguous,
        Certainty::Heuristic,
        Certainty::Synthetic,
        Certainty::Dispatch,
        Certainty::Exact,
    ]
    .into_iter()
    .find(|c| c.as_str() == s)
    .ok_or_else(|| {
        format!("unknown certainty {s:?} (ambiguous, heuristic, synthetic, dispatch, exact)")
    })
}

pub(crate) fn parse_precision(s: &str) -> Result<Precision, String> {
    [Precision::Rta, Precision::Cha]
        .into_iter()
        .find(|p| p.as_str() == s)
        .ok_or_else(|| format!("unknown precision {s:?} (rta, cha)"))
}

/// The header of an answer that expands dispatch (contract M4b S10): `precision=<p>`, and how
/// many dispatch steps RTA skipped when it skipped any.
pub(crate) fn precision_lines(precision: Precision, pruned: usize) -> String {
    let mut out = format!("precision={}\n", precision.as_str());
    if pruned > 0 {
        let steps = if pruned == 1 { "step" } else { "steps" };
        let _ = writeln!(
            out,
            "rta: {pruned} dispatch {steps} pruned (implementations never instantiated; --precision cha shows them)"
        );
    }
    out
}

/// The node `input` names. Several candidates are listed, never picked silently, unless `pick`
/// (1-based, in the listed order) chooses one.
pub(crate) fn pick_target(graph: &Graph, input: &str, pick: Option<usize>) -> anyhow::Result<i64> {
    let candidates = rb_store::code::resolve_target(graph, input);
    let d = Describer::new(graph);
    match (candidates.as_slice(), pick) {
        ([], _) => bail!("nothing matches {input:?} (try `rb find {input}`)"),
        ([only], _) => Ok(*only),
        (many, Some(n)) if (1..=many.len()).contains(&n) => Ok(many[n - 1]),
        (many, _) => {
            let list: Vec<String> = many
                .iter()
                .enumerate()
                .map(|(i, &id)| format!("  {}. {}", i + 1, line(&d, id)))
                .collect();
            bail!(
                "{input:?} matches {} symbols; pass --pick N:\n{}",
                many.len(),
                list.join("\n")
            )
        }
    }
}

/// `display  path:line  [service]`
fn line(d: &Describer<'_>, id: i64) -> String {
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

/// The name of the service exposing endpoint `id` (its key when no node names it).
fn endpoint_service(graph: &Graph, id: i64) -> String {
    let Some(e) = graph.nodes().iter().find(|n| n.id == id) else {
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
        .find(|n| n.kind == rb_model::NodeKind::Service && n.key == key)
        .map_or(key, |n| n.name.as_str())
        .to_owned()
}

/// `POST /api/v1/login  [auth-service]`: an endpoint's name and the name of its service, then
/// `  (prefix unknown, router <file>#<binding>)` for an endpoint keyed by its router (M4a
/// open item 7, contract M4b S13).
fn endpoint_label(graph: &Graph, id: i64) -> String {
    let Some(e) = graph.nodes().iter().find(|n| n.id == id) else {
        return id.to_string();
    };
    let mut label = format!("{}  [{}]", e.name, endpoint_service(graph, id));
    if let Some(router) = e.attrs.get("router").and_then(|r| r.as_str()) {
        let _ = write!(label, "  (prefix unknown, router {router})");
    }
    label
}

fn histogram<'c>(certainties: impl Iterator<Item = &'c Certainty>) -> String {
    let mut counts: BTreeMap<std::cmp::Reverse<Certainty>, usize> = BTreeMap::new();
    for c in certainties {
        *counts.entry(std::cmp::Reverse(*c)).or_default() += 1;
    }
    let parts: Vec<String> = counts
        .into_iter()
        .map(|(c, n)| format!("{} {n}", c.0.as_str()))
        .collect();
    format!(
        "certainty: {}\n",
        if parts.is_empty() {
            "-".to_owned()
        } else {
            parts.join(", ")
        }
    )
}

pub(crate) fn render_callers(graph: &Graph, tree: &CallerTree) -> String {
    let d = Describer::new(graph);
    let mut out = format!("callers of {}\n", line(&d, tree.target));
    out.push_str(&precision_lines(tree.precision, tree.pruned_by_rta));
    fn walk(
        d: &Describer<'_>,
        cs: &[Caller],
        depth: usize,
        out: &mut String,
        all: &mut Vec<Certainty>,
    ) {
        for c in cs {
            let mut how = c.certainty.as_str().to_owned();
            if c.dispatch {
                how.push_str(", via dispatch");
            }
            if c.reference {
                how.push_str(", as a reference");
            }
            // Entry points are marked wherever they appear (contract M4b S11).
            if !c.entry.is_empty() {
                let _ = write!(how, "  (entry: {})", c.entry.join(", "));
            }
            let _ = writeln!(out, "{}{}  {how}", "  ".repeat(depth), line(d, c.node));
            all.push(c.certainty);
            walk(d, &c.callers, depth + 1, out, all);
        }
    }
    let mut all = Vec::new();
    walk(&d, &tree.callers, 1, &mut out, &mut all);
    if all.is_empty() {
        out.push_str("  (no callers found)\n");
    }
    out.push_str(&histogram(all.iter()));
    out
}

/// A node as JSON: its key and how it is shown.
fn node_json(graph: &Graph, d: &Describer<'_>, id: i64) -> Value {
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

/// `rb impact --json` (contract M4b S13): every section of the text answer, with keys.
pub(crate) fn impact_json(graph: &Graph, impact: &Impact) -> Value {
    let d = Describer::new(graph);
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
                "service": endpoint_service(graph, x.node),
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
    json!({
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
        "truncated": impact.truncated,
    })
}

pub(crate) fn render_impact(graph: &Graph, impact: &Impact) -> String {
    let d = Describer::new(graph);
    let target = impact.removed.first().copied();
    let mut out = String::new();
    if let Some(t) = target {
        let _ = writeln!(out, "removing {}", line(&d, t));
    }
    for &r in impact.removed.iter().skip(1) {
        let _ = writeln!(out, "  with {}", line(&d, r));
    }
    out.push_str(&precision_lines(impact.precision, impact.pruned_by_rta));
    let section = |title: &str, xs: &[Impacted], out: &mut String| {
        let _ = writeln!(out, "{title} ({}):", xs.len());
        for x in xs {
            let _ = writeln!(
                out,
                "  {}  {}  {}",
                x.distance,
                line(&d, x.node),
                x.certainty.as_str()
            );
        }
    };
    section("breaks: requiring edges into it", &impact.breaks, &mut out);
    section(
        "affects: reaches those through calls or references",
        &impact.affects,
        &mut out,
    );
    // Endpoints the removed or impacted code handles (contract M4a S21), each once: at
    // distance 1 exactly when a handler is removed (M4a open item 7).
    let _ = writeln!(
        out,
        "endpoints: handled by the removed or impacted code ({}):",
        impact.endpoints.len()
    );
    for x in &impact.endpoints {
        let _ = writeln!(
            out,
            "  {}  {}  {}{}",
            x.distance,
            endpoint_label(graph, x.node),
            x.certainty.as_str(),
            if x.distance == 1 {
                "  (handler removed)"
            } else {
                ""
            }
        );
    }
    // What may become dead code (contract M4b S13).
    match &impact.orphans {
        Orphans::NotComputed(reason) => {
            let _ = writeln!(out, "orphans: not computed ({reason})");
        }
        Orphans::Computed(list) => {
            let _ = writeln!(out, "orphans: may become dead code ({}):", list.len());
            for o in list {
                let _ = writeln!(
                    out,
                    "  {}  {}  via {} ({})",
                    o.distance,
                    line(&d, o.node),
                    d.describe(o.via.0).display,
                    o.via.1
                );
            }
        }
    }
    section("tests", &impact.tests, &mut out);
    let services: Vec<String> = impact
        .services
        .iter()
        .map(|(key, n, own)| {
            let name = graph
                .nodes()
                .iter()
                .find(|node| node.kind == rb_model::NodeKind::Service && &node.key == key)
                .map_or(key.as_str(), |node| node.name.as_str());
            format!("{name}{} {n}", if *own { " (own)" } else { "" })
        })
        .collect();
    let _ = writeln!(
        out,
        "services: {}",
        if services.is_empty() {
            "-".to_owned()
        } else {
            services.join("; ")
        }
    );
    let owners: Vec<String> = impact
        .owners
        .iter()
        .map(|(o, n)| format!("{o} {n}"))
        .collect();
    let _ = writeln!(
        out,
        "owners: {}",
        if owners.is_empty() {
            "-".to_owned()
        } else {
            owners.join("; ")
        }
    );
    out.push_str(&histogram(
        impact
            .breaks
            .iter()
            .chain(&impact.affects)
            .map(|x| &x.certainty),
    ));
    if impact.truncated {
        let _ = writeln!(
            out,
            "truncated: stopped after {} nodes",
            rb_store::code::IMPACT_NODE_CAP
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contract M4b S10: every answer that expands dispatch states its precision, and the RTA
    /// line appears exactly when steps were pruned.
    #[test]
    fn precision_header_counts_pruned_steps() {
        assert_eq!(precision_lines(Precision::Cha, 0), "precision=cha\n");
        assert_eq!(precision_lines(Precision::Rta, 0), "precision=rta\n");
        assert_eq!(
            precision_lines(Precision::Rta, 1),
            "precision=rta\nrta: 1 dispatch step pruned (implementations never instantiated; --precision cha shows them)\n"
        );
        assert!(precision_lines(Precision::Rta, 3).contains("rta: 3 dispatch steps pruned"));
        assert_eq!(parse_precision("cha"), Ok(Precision::Cha));
        assert!(parse_precision("vta").is_err());
    }

    /// Contract M4b S13 and M4a open item 7 in the text and JSON answers: the endpoint of a
    /// removed handler once, with its router; the orphan with the step that reached it; the
    /// impacted test.
    #[test]
    fn impact_lists_endpoints_orphans_and_tests() {
        use rb_model::{
            Attrs, Certainty, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind,
            Provenance,
        };
        let mut b = GraphBuilder::new();
        let obj = |v: Value| v.as_object().cloned().unwrap();
        b.add_node(
            NodeKey::new(NodeKind::Service, "app"),
            "app",
            obj(json!({"role": "service"})),
        )
        .unwrap();
        let sym = |k: &str| NodeKey::new(NodeKind::Symbol, k);
        for (path, name, attrs, line) in [
            ("app/A.java", "T", json!({"orphan_eligible": true}), 1),
            ("app/A.java", "h", json!({"orphan_eligible": true}), 2),
            ("app/test/ATest.java", "t", json!({"entry": ["test"]}), 1),
        ] {
            let f = NodeKey::new(NodeKind::File, path);
            if !b.has_node(&f) {
                b.add_node(f.clone(), path, Attrs::new()).unwrap();
                b.add_edge(NewEdge::new(
                    f.clone(),
                    EdgeKind::InService,
                    NodeKey::new(NodeKind::Service, "app"),
                    Provenance::Manifest,
                    Certainty::Exact,
                ))
                .unwrap();
            }
            let key = format!("{path}/{name}");
            b.add_node(sym(&key), name, obj(attrs)).unwrap();
            b.add_edge(
                NewEdge::new(
                    sym(&key),
                    EdgeKind::DefinedIn,
                    f,
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
        let edge = |s: &str, kind, d: NodeKey| {
            NewEdge::new(sym(s), kind, d, Provenance::Heuristic, Certainty::Heuristic)
        };
        b.add_edge(edge("app/A.java/T", EdgeKind::Calls, sym("app/A.java/h")))
            .unwrap();
        b.add_edge(edge(
            "app/test/ATest.java/t",
            EdgeKind::Calls,
            sym("app/A.java/T"),
        ))
        .unwrap();
        let ep = NodeKey::new(NodeKind::Endpoint, "HTTP GET / @app ?router=a.go#r");
        b.add_node(
            ep.clone(),
            "GET /",
            obj(json!({"service": "app", "prefix_unknown": true, "router": "a.go#r"})),
        )
        .unwrap();
        b.add_edge(NewEdge::new(
            ep,
            EdgeKind::HandledBy,
            sym("app/A.java/T"),
            Provenance::Treesitter,
            Certainty::Exact,
        ))
        .unwrap();
        let graph = b.finish().unwrap();
        let t = graph.nodes().iter().find(|n| n.name == "T").unwrap().id;
        let impact = rb_store::code::impact(&graph, t, Certainty::Ambiguous, Precision::Cha);
        let text = render_impact(&graph, &impact);
        for want in [
            "breaks: requiring edges into it (1):\n  1  app/test/ATest.java/t",
            "endpoints: handled by the removed or impacted code (1):\n  1  GET /  [app]  (prefix unknown, router a.go#r)  exact  (handler removed)\n",
            "orphans: may become dead code (1):\n  1  app/A.java/h  app/A.java:3  [app]  via app/A.java/T (calls)\n",
            "tests (1):\n  1  app/test/ATest.java/t",
        ] {
            assert!(text.contains(want), "{want}\n---\n{text}");
        }
        assert!(
            !text.contains("breaks: requiring edges into it (2)"),
            "{text}"
        );
        // `rb callers` marks the endpoint handler and the test method (S11).
        let h = graph.nodes().iter().find(|n| n.name == "h").unwrap().id;
        let tree = rb_store::code::callers(
            &graph,
            h,
            rb_store::code::CallerOptions {
                depth: 2,
                include_references: false,
                min_certainty: Certainty::Ambiguous,
                precision: Precision::Rta,
            },
        );
        let callers = render_callers(&graph, &tree);
        assert!(
            callers.contains(
                "  app/A.java/T  app/A.java:2  [app]  heuristic  (entry: endpoint GET /  (prefix unknown, router a.go#r))\n    app/test/ATest.java/t  app/test/ATest.java:2  [app]  heuristic  (entry: test)\n"
            ),
            "{callers}"
        );
        let json = impact_json(&graph, &impact);
        assert_eq!(json["endpoints"][0]["handler_removed"], true);
        assert_eq!(json["endpoints"][0]["router"], "a.go#r");
        assert_eq!(json["orphans"]["nodes"][0]["via"]["how"], "calls");
        assert_eq!(json["tests"][0]["distance"], 1);
        assert_eq!(json["precision"], "cha");
    }
}
