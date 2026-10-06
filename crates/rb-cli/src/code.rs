//! `rb callers` (Q1) and `rb impact` (Q2): choosing the target and rendering the answers
//! (design §9.1–§9.3). The questions themselves live in `rb_store::code`.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::bail;
use rb_model::{Certainty, Graph};
use rb_store::code::{Caller, CallerTree, Describer, Impact, Impacted};

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

/// `POST /api/v1/login  [auth-service]`: an endpoint's name and the name of its service.
fn endpoint_label(graph: &Graph, id: i64) -> String {
    let Some(e) = graph.nodes().iter().find(|n| n.id == id) else {
        return id.to_string();
    };
    let key = e
        .attrs
        .get("service")
        .and_then(|s| s.as_str())
        .unwrap_or_default();
    let service = graph
        .nodes()
        .iter()
        .find(|n| n.kind == rb_model::NodeKind::Service && n.key == key)
        .map_or(key, |n| n.name.as_str());
    format!("{}  [{service}]", e.name)
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
    // Endpoints the removed or impacted code handles (contract M4a S21).
    let _ = writeln!(
        out,
        "endpoints: handled by the removed or impacted code ({}):",
        impact.endpoints.len()
    );
    for x in &impact.endpoints {
        let _ = writeln!(
            out,
            "  {}  {}  {}",
            x.distance,
            endpoint_label(graph, x.node),
            x.certainty.as_str()
        );
    }
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
