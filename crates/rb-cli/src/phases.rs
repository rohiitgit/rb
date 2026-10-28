//! Phases 2-10 handlers: events, decisions, context, agent, verify,
//! production graph, feature graph, orchestration, thin IDE export.
//! Deterministic; no LLM calls.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use rb_store::Store;

fn live_id(store: &mut Store, repo: &str) -> anyhow::Result<Option<i64>> {
    Ok(store.live_snapshot(repo)?.map(|s| s.id))
}

fn service_keys(store: &mut Store, snap: i64) -> Vec<String> {
    store
        .services(snap)
        .map(|v| v.into_iter().map(|s| s.key).collect())
        .unwrap_or_default()
}

fn find_keys(store: &mut Store, snap: i64, q: &str, kind: &str) -> Vec<String> {
    let k = kind.parse().ok();
    store
        .find(snap, q, k, 5, 0)
        .map(|p| p.nodes.into_iter().map(|n| n.key).collect())
        .unwrap_or_default()
}

pub fn context_package(store: &mut Store, repo: &str, task: &str) -> anyhow::Result<Value> {
    let keywords = rb_ops::keywords(task);
    let snap = live_id(store, repo)?;
    let mut services = Vec::new();
    let mut endpoints = Vec::new();
    let mut tables = Vec::new();
    if let Some(id) = snap {
        services = service_keys(store, id);
        for w in keywords.iter().take(4) {
            endpoints.extend(find_keys(store, id, w, "endpoint"));
            tables.extend(find_keys(store, id, w, "table"));
        }
        endpoints.sort();
        endpoints.dedup();
        endpoints.truncate(10);
        tables.sort();
        tables.dedup();
        tables.truncate(10);
        // Keep only services named by a keyword; fall back to all (max 10).
        if !keywords.is_empty() {
            let kept: Vec<String> = services
                .iter()
                .filter(|s| keywords.iter().any(|w| s.to_lowercase().contains(w)))
                .cloned()
                .collect();
            if !kept.is_empty() {
                services = kept;
            }
        }
        services.truncate(10);
    }
    let decisions = store
        .search_decisions(repo, task, 5)
        .unwrap_or_default()
        .into_iter()
        .map(|d| {
            json!({"title": d.title, "reason": d.reason, "source": d.source, "author": d.author})
        })
        .collect::<Vec<_>>();
    let incidents = store
        .list_incidents(repo, None)
        .unwrap_or_default()
        .into_iter()
        .take(5)
        .map(|i| json!({"service": i.service, "severity": i.severity, "summary": i.summary}))
        .collect::<Vec<_>>();
    Ok(json!({
        "task": task,
        "keywords": keywords,
        "services": services,
        "endpoints": endpoints,
        "tables": tables,
        "decisions": decisions,
        "incidents": incidents,
        "snapshot": snap,
    }))
}

pub fn agent_plan(store: &mut Store, repo: &str, task: &str) -> anyhow::Result<Value> {
    let ctx = context_package(store, repo, task)?;
    let services: Vec<String> = ctx["services"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    let endpoints: Vec<String> = ctx["endpoints"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    let tables: Vec<String> = ctx["tables"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect();
    let steps = rb_ops::plan_from_context(&services, &endpoints, &tables, 8);
    let agents: Vec<&str> = rb_ops::route_task(task)
        .into_iter()
        .map(rb_ops::Agent::as_str)
        .collect();
    Ok(json!({
        "task": task,
        "agents": agents,
        "context": ctx,
        "steps": steps.into_iter().map(|s| {
            json!({"tool": s.tool, "target": s.target, "why": s.why})
        }).collect::<Vec<_>>(),
        "capabilities": ["read", "edit", "execute", "test"],
    }))
}

pub fn orchestrate_plan(store: &mut Store, repo: &str, task: &str) -> anyhow::Result<Value> {
    let plan = agent_plan(store, repo, task)?;
    let subtasks: Vec<Value> = plan["agents"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .map(|a| {
            json!({
                "agent": a,
                "task": task,
                "verification": "rb verify",
            })
        })
        .collect();
    Ok(json!({"task": task, "subtasks": subtasks, "plan": plan}))
}

pub fn verify_tree(root: &Path, max_files: usize) -> anyhow::Result<Value> {
    let mut files = Vec::new();
    collect_text_files(root, &mut files, max_files);
    let mut findings = Vec::new();
    for (path, text) in &files {
        for f in rb_ops::security_scan(path, text) {
            findings.push(json!({
                "rule": f.rule, "file": f.file, "line": f.line, "detail": f.detail,
            }));
        }
    }
    let cost = rb_ops::cost_check(files.len(), 0);
    Ok(json!({
        "files_scanned": files.len(),
        "security": findings,
        "cost": cost.into_iter().map(|c| {
            json!({"rule": c.rule, "detail": c.detail})
        }).collect::<Vec<_>>(),
    }))
}

fn collect_text_files(root: &Path, out: &mut Vec<(String, String)>, cap: usize) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for e in entries.flatten() {
            if out.len() >= cap {
                return;
            }
            let p = e.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if matches!(name, "target" | ".git" | "node_modules" | "var") {
                    continue;
                }
                stack.push(p);
            } else if let Some(ext) = p.extension().and_then(|e| e.to_str())
                && matches!(ext, "ts" | "js" | "tsx" | "py" | "java" | "go" | "rs")
                && let Ok(text) = std::fs::read_to_string(&p)
            {
                out.push((p.display().to_string(), text));
            }
        }
    }
}

pub fn ide_html() -> String {
    r#"<!doctype html><html><head><meta charset="utf-8"><title>rb thin IDE</title>
<style>body{font-family:monospace;display:grid;grid-template-columns:1fr 1fr;grid-template-rows:1fr 1fr;height:100vh;margin:0}section{border:1px solid #ccc;padding:8px;overflow:auto}h2{margin:0 0 8px;font-size:14px}</style>
</head><body>
<section><h2>Editor (read-only; edits via rb agent)</h2><pre id="editor">open a file:line from Graph Explorer</pre></section>
<section><h2>Graph Explorer</h2><input id="q" placeholder="rb find query"><button onclick="find()">find</button><pre id="graph"></pre></section>
<section><h2>Agent Interface</h2><input id="task" placeholder="task"><button onclick="agent()">plan</button><pre id="plan"></pre></section>
<section><h2>Review Interface</h2><button onclick="verify()">rb verify</button><pre id="review"></pre></section>
<script>
const base="http://127.0.0.1:8080/v1/repos";
async function find(){const r=await fetch(`${base}/demo/find?q=${encodeURIComponent(document.getElementById('q').value)}`);document.getElementById('graph').textContent=await r.text()}
async function agent(){document.getElementById('plan').textContent="run: rb agent \""+document.getElementById('task').value+"\" --repo demo"}
async function verify(){document.getElementById('review').textContent="run: rb verify --repo demo"}
</script></body></html>"#.to_owned()
}

pub fn write_ide(output: &PathBuf) -> anyhow::Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, ide_html())?;
    Ok(())
}
