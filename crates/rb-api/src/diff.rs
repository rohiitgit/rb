//! `rb diff` and `GET /v1/repos/{repo}/diff` (contract M5 S16, S17, D11, D12): two kept
//! snapshots, their exact sections ([`rb_model::diff`]), their summaries
//! ([`rb_store::diff`]), the diagnostics' difference, and the tool-version keys that differ.
//! `rb diff` never re-indexes (D12).

use std::collections::{BTreeMap, BTreeSet};

use rb_model::diff::{Change, DiagnosticsDiff, EdgeRow, GraphDiff, NodeRow};
use rb_store::diff::Summaries;
use rb_store::{SnapshotInfo, Store};
use serde_json::{Value, json};

use crate::{ApiError, Indexes, Output, Page, Paged, kept_snapshot, live_snapshot, snapshot_ref};

/// The page size of each section: default and maximum (D18).
pub const LIMIT_MAX: u64 = 1_000;

/// A computed diff: the two snapshots and every section (each paged when serialised).
pub struct DiffAnswer {
    pub from: SnapshotInfo,
    pub to: SnapshotInfo,
    /// `tool_versions` keys whose values differ (D12): the diff then includes tool-induced
    /// changes.
    pub tools_differ: Vec<String>,
    pub summaries: Summaries,
    pub exact: GraphDiff,
    pub diagnostics: DiagnosticsDiff,
    pub page: Page,
    pub list_diagnostics: bool,
}

fn node_json(n: &NodeRow) -> Value {
    json!({"kind": n.kind.as_str(), "key": n.key, "name": n.name, "attrs": n.attrs})
}

fn edge_json(e: &EdgeRow) -> Value {
    json!({
        "src_kind": e.src_kind.as_str(),
        "src_key": e.src_key,
        "kind": e.kind.as_str(),
        "dst_kind": e.dst_kind.as_str(),
        "dst_key": e.dst_key,
        "provenance": e.provenance.as_str(),
        "certainty": e.certainty.as_str(),
        "attrs": e.attrs,
        "evidence": e.evidence.iter().map(|ev| json!([ev.path, ev.span])).collect::<Vec<_>>(),
    })
}

fn change_json<T>(c: &Change<T>, f: fn(&T) -> Value) -> Value {
    json!({"before": f(&c.before), "after": f(&c.after)})
}

fn snapshot_json(s: &SnapshotInfo) -> Value {
    json!({"id": s.id, "repo": s.repo, "commit_sha": s.commit_sha, "status": s.status})
}

impl DiffAnswer {
    /// The `result`: each section a page of `page` (D18).
    pub fn to_json(&self) -> Value {
        let page = |items: Vec<Value>| -> Value {
            serde_json::to_value(Paged::of(items.len() as u64, self.page, items.into_iter()))
                .expect("a page serialises")
        };
        let d = &self.exact;
        let mut diagnostics = json!({
            "counts": self
                .diagnostics
                .counts
                .iter()
                .map(|(k, (b, a))| (k.clone(), json!({"before": b, "after": a})))
                .collect::<serde_json::Map<_, _>>(),
        });
        if self.list_diagnostics {
            let line = |x: &rb_model::Diagnostic| {
                serde_json::from_str::<Value>(&rb_model::diff::diagnostic_line(x))
                    .expect("a dump line is JSON")
            };
            diagnostics["added"] = page(self.diagnostics.added.iter().map(line).collect());
            diagnostics["removed"] = page(self.diagnostics.removed.iter().map(line).collect());
        }
        json!({
            "from": snapshot_json(&self.from),
            "to": snapshot_json(&self.to),
            "tools_differ": self.tools_differ,
            "summary": self.summaries,
            "nodes": {
                "added": page(d.nodes_added.iter().map(node_json).collect()),
                "removed": page(d.nodes_removed.iter().map(node_json).collect()),
                "changed": page(d.nodes_changed.iter().map(|c| change_json(c, node_json)).collect()),
            },
            "edges": {
                "added": page(d.edges_added.iter().map(edge_json).collect()),
                "removed": page(d.edges_removed.iter().map(edge_json).collect()),
                "changed": page(d.edges_changed.iter().map(|c| change_json(c, edge_json)).collect()),
                "evidence_only": page(
                    d.edges_evidence_only.iter().map(|c| change_json(c, edge_json)).collect()
                ),
            },
            "diagnostics": diagnostics,
        })
    }
}

/// The `tool_versions` keys (of either side) whose values differ, sorted.
pub fn tools_differ(a: &Value, b: &Value) -> Vec<String> {
    let keys = |v: &Value| -> BTreeSet<String> {
        v.as_object()
            .map(|o| o.keys().cloned().collect())
            .unwrap_or_default()
    };
    keys(a)
        .union(&keys(b))
        .filter(|k| a.get(k.as_str()) != b.get(k.as_str()))
        .cloned()
        .collect()
}

/// The snapshot a selector names (D11): digits are a snapshot id (of any repository: two
/// repositories' snapshots may be compared); otherwise a commit prefix (4 to 40 hex digits)
/// unique among `repo`'s kept snapshots; several matches are `ambiguous`, never a pick.
pub fn select(
    store: &mut Store,
    repo: &str,
    selector: &str,
    param: &str,
) -> Result<SnapshotInfo, ApiError> {
    if selector.bytes().all(|b| b.is_ascii_digit()) {
        let id = selector.parse::<i64>().map_err(|_| {
            ApiError::invalid(param, format_args!("{selector:?} is no snapshot id"))
        })?;
        return kept_snapshot(store, id);
    }
    let prefix = selector.to_ascii_lowercase();
    if !(4..=40).contains(&prefix.len()) || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ApiError::invalid(
            param,
            format_args!(
                "must be a snapshot id or a commit prefix of 4 to 40 hex digits (got {selector:?})"
            ),
        ));
    }
    let found: Vec<SnapshotInfo> = store
        .snapshots(repo)?
        .into_iter()
        .filter(|x| {
            (x.status == "live" || x.status == "retired") && x.commit_sha.starts_with(&prefix)
        })
        .collect();
    match found.as_slice() {
        [] => Err(ApiError::not_found(format!(
            "no kept snapshot of {repo:?} is at a commit starting with {prefix} (`rb snapshots` lists them)"
        ))),
        [only] => Ok(only.clone()),
        many => {
            let lines: Vec<String> = many
                .iter()
                .map(|x| format!("  {}  {}  {}", x.id, short(&x.commit_sha), x.status))
                .collect();
            Err(ApiError::ambiguous(
                format!(
                    "{selector:?} matches {} kept snapshots; pass a snapshot id:\n{}",
                    many.len(),
                    lines.join("\n")
                ),
                many.iter()
                    .map(|x| json!({"id": x.id, "commit_sha": x.commit_sha, "status": x.status}))
                    .collect(),
            ))
        }
    }
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(12)]
}

/// `to`'s parent, the default `from` (D11), while it is kept; otherwise `invalid_param` naming
/// the kept snapshots.
fn parent_of(store: &mut Store, repo: &str, to: &SnapshotInfo) -> Result<SnapshotInfo, ApiError> {
    if let Some(p) = to.parent_id
        && let Some(parent) = store.snapshot_opt(p)?
        && (parent.status == "live" || parent.status == "retired")
    {
        return Ok(parent);
    }
    let kept: Vec<String> = store
        .snapshots(repo)?
        .iter()
        .filter(|x| x.id != to.id && (x.status == "live" || x.status == "retired"))
        .map(|x| x.id.to_string())
        .collect();
    let why = match to.parent_id {
        Some(p) => format!("snapshot {}'s parent {p} is no longer kept", to.id),
        None => format!("snapshot {} has no parent", to.id),
    };
    Err(ApiError::invalid(
        "from",
        format_args!(
            "is required: {why} (kept: {})",
            if kept.is_empty() {
                "none".to_owned()
            } else {
                kept.join(", ")
            }
        ),
    ))
}

/// Answers a diff inside the request's read transaction: both snapshots' graphs, then every
/// section. Not cut by the work cap: each part is linear in the two snapshots (counted in
/// `rb_model::diff`'s growth test), and the server's deadline bounds it (amendment 8).
pub(crate) fn answer(
    store: &mut Store,
    indexes: &mut dyn Indexes,
    repo: &str,
    from: Option<&str>,
    to: Option<&str>,
    page: Page,
    list_diagnostics: bool,
) -> Result<Output, ApiError> {
    let live = store.live_snapshot(repo)?.map(|l| l.id);
    let to = match to {
        Some(sel) => select(store, repo, sel, "to")?,
        None => live_snapshot(store, repo)?,
    };
    let from = match from {
        Some(sel) => select(store, repo, sel, "from")?,
        None => parent_of(store, repo, &to)?,
    };
    let (from_live, to_live) = (live == Some(from.id), live == Some(to.id));
    let a = indexes.index(store, &from, from_live)?;
    let b = indexes.index(store, &to, to_live)?;
    let exact = rb_model::diff::diff(a.graph(), b.graph());
    let summaries = rb_store::diff::summaries(&a, &b, &exact);
    let diagnostics = rb_model::diff::diagnostics_diff(
        &store.load_diagnostics(from.id)?,
        &store.load_diagnostics(to.id)?,
    );
    let query = json!({
        "from": from.id,
        "to": to.id,
        "limit": page.limit,
        "offset": page.offset,
        "diagnostics": list_diagnostics,
    });
    Ok(Output {
        repo: Some(repo.to_owned()),
        snapshot: Some(snapshot_ref(&to)),
        query,
        limits: BTreeMap::new(),
        truncated: Vec::new(),
        certainty: None,
        answer: crate::Answer::Diff(Box::new(DiffAnswer {
            tools_differ: tools_differ(&from.tool_versions, &to.tool_versions),
            from,
            to,
            summaries,
            exact,
            diagnostics,
            page,
            list_diagnostics,
        })),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn differing_tool_versions_are_named() {
        let a = json!({"rb": "0.1.0", "pipeline": "m4b-di-8", "scip": "off"});
        let b = json!({"rb": "0.1.0", "pipeline": "m5-1", "scip": "on", "scip_java": "sha256:1"});
        assert_eq!(tools_differ(&a, &b), ["pipeline", "scip", "scip_java"]);
        assert!(tools_differ(&a, &a).is_empty());
    }
}
