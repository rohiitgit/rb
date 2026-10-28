//! Phases 2-10 handlers: events, decisions, context, agent, verify,
//! production graph, feature graph, orchestration, thin IDE export.
//! Deterministic; no LLM calls.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use rb_store::Store;

pub fn context_package(store: &mut Store, repo: &str, task: &str) -> anyhow::Result<Value> {
    store.context_package(repo, task)
}

pub fn agent_plan(store: &mut Store, repo: &str, task: &str) -> anyhow::Result<Value> {
    store.agent_plan(repo, task)
}

pub fn orchestrate_plan(store: &mut Store, repo: &str, task: &str) -> anyhow::Result<Value> {
    store.orchestrate_plan(repo, task)
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
<style>body{font-family:monospace;display:grid;grid-template-columns:1fr 1fr;grid-template-rows:auto 1fr 1fr;height:100vh;margin:0}header{grid-column:1/3;padding:8px;border-bottom:1px solid #ccc}section{border:1px solid #ccc;padding:8px;overflow:auto}h2{margin:0 0 8px;font-size:14px}input{font-family:monospace;width:60%}</style>
</head><body>
<header><b>rb thin IDE</b> (read-only; edits via <code>rb agent</code>)
base <input id="base" value="http://127.0.0.1:55480"> repo <input id="repo" value="demo"></header>
<section><h2>Editor (evidence viewer: symbol &rarr; callers with file:line)</h2><input id="sym" placeholder="symbol, e.g. Db#load()."><button onclick="callers()">callers</button><pre id="editor"></pre></section>
<section><h2>Graph Explorer (find + service card)</h2><input id="q" placeholder="find query"><button onclick="find()">find</button> <input id="svc" placeholder="service"><button onclick="card()">card</button><pre id="graph"></pre></section>
<section><h2>Agent Interface (task &rarr; plan from context only)</h2><input id="task" placeholder="task, e.g. fix login timeout"><button onclick="agent()">plan</button><pre id="plan"></pre></section>
<section><h2>Review Interface (orchestrate: agents, scopes, verification)</h2><input id="task2" placeholder="task"><button onclick="review()">orchestrate</button><pre id="review"></pre></section>
<script>
async function get(path, params, el) {
  const base = document.getElementById('base').value.replace(/\/$/, '');
  const repo = encodeURIComponent(document.getElementById('repo').value);
  const qs = new URLSearchParams(params).toString();
  let out;
  try {
    const r = await fetch(`${base}/v1/repos/${repo}/${path}?${qs}`);
    out = JSON.stringify(JSON.parse(await r.text()), null, 2);
  } catch (e) { out = "request failed: " + e + " (is `rb serve` running?)"; }
  document.getElementById(el).textContent = out;
}
function find() { get('find', {q: document.getElementById('q').value}, 'graph'); }
function card() { get('service', {name: document.getElementById('svc').value}, 'graph'); }
function callers() { get('callers', {target: document.getElementById('sym').value}, 'editor'); }
function agent() { get('agent', {task: document.getElementById('task').value}, 'plan'); }
function review() { get('orchestrate', {task: document.getElementById('task2').value}, 'review'); }
</script></body></html>"#.to_owned()
}

pub fn write_ide(output: &PathBuf) -> anyhow::Result<()> {
    write_ide_to(output, &ide_html())
}

fn write_ide_to(output: &PathBuf, html: &str) -> anyhow::Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(output, html)?;
    Ok(())
}

/// Phase 6: architecture violations on the repo's live snapshot, if it has one.
/// `None` means no live graph to check (not a pass).
pub fn arch_violations(
    store: &mut Store,
    repo: &str,
) -> anyhow::Result<Option<Vec<(String, String, String)>>> {
    let live = match store.live_snapshot(repo)? {
        Some(s) => s,
        None => return Ok(None),
    };
    let graph = store.load_graph(live.id)?;
    Ok(Some(
        rb_store::check::check(&graph)
            .into_iter()
            .map(|v| (v.rule.to_owned(), v.src.clone(), v.dst.clone()))
            .collect(),
    ))
}
