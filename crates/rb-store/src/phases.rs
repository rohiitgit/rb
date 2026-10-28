//! Phases 2-8 persistence: events, decisions, deployments, incidents, features.
//! Append-only logs; answers are deterministic selects, never embeddings.

use serde::Serialize;
use serde_json::Value;

use crate::Store;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Event {
    pub id: i64,
    pub kind: String,
    pub commit_sha: String,
    pub path: Option<String>,
    pub detail: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Decision {
    pub id: i64,
    pub title: String,
    pub reason: String,
    pub decided_on: String,
    pub source: String,
    pub author: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Deployment {
    pub id: i64,
    pub service: String,
    pub commit_sha: String,
    pub version: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Incident {
    pub id: i64,
    pub service: String,
    pub deployment_id: Option<i64>,
    pub severity: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Feature {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub links: Vec<(String, String)>,
}

impl Store {
    pub fn record_event(
        &mut self,
        repo: &str,
        kind: &str,
        commit_sha: &str,
        path: Option<&str>,
        detail: &Value,
    ) -> anyhow::Result<i64> {
        anyhow::ensure!(
            rb_ops::is_event_kind(kind),
            "unknown event kind {kind:?}: commit.created, file.changed, pr.merged, branch.deleted"
        );
        let repo_id = self.ensure_repo(repo, None)?;
        let id: i64 = self
            .client
            .query_one(
                "INSERT INTO rb.event (repo_id, kind, commit_sha, path, detail)
                 VALUES ($1,$2,$3,$4,$5) RETURNING id",
                &[&repo_id, &kind, &commit_sha, &path, detail],
            )?
            .get(0);
        Ok(id)
    }

    pub fn list_events(&mut self, repo: &str, limit: i64) -> anyhow::Result<Vec<Event>> {
        let rows = self.client.query(
            "SELECT e.id, e.kind, e.commit_sha, e.path, e.detail FROM rb.event e
             JOIN rb.repo r ON r.id = e.repo_id WHERE r.name = $1
             ORDER BY e.id DESC LIMIT $2",
            &[&repo, &limit],
        )?;
        Ok(rows
            .iter()
            .map(|r| Event {
                id: r.get(0),
                kind: r.get(1),
                commit_sha: r.get(2),
                path: r.get(3),
                detail: r.get(4),
            })
            .collect())
    }

    /// Phase 2: which of `blobs` already sit in the facts cache (any extractor).
    /// Those need no extraction on the next index; the rest do.
    pub fn cached_blobs(&mut self, blobs: &[String]) -> anyhow::Result<Vec<bool>> {
        if blobs.is_empty() {
            return Ok(Vec::new());
        }
        let rows = self.client.query(
            "SELECT DISTINCT blob_sha FROM rb.file_facts_cache WHERE blob_sha = ANY($1)",
            &[&blobs],
        )?;
        let hit: std::collections::BTreeSet<String> = rows.iter().map(|r| r.get(0)).collect();
        Ok(blobs.iter().map(|b| hit.contains(b)).collect())
    }

    pub fn record_decision(
        &mut self,
        repo: &str,
        title: &str,
        reason: &str,
        decided_on: &str,
        source: &str,
        author: &str,
    ) -> anyhow::Result<i64> {
        let repo_id = self.ensure_repo(repo, None)?;
        let id: i64 = self
            .client
            .query_one(
                "INSERT INTO rb.decision (repo_id, title, reason, decided_on, source, author)
                 VALUES ($1,$2,$3,$4,$5,$6)
                 ON CONFLICT (repo_id, title) DO UPDATE
                 SET reason=EXCLUDED.reason, decided_on=EXCLUDED.decided_on,
                     source=EXCLUDED.source, author=EXCLUDED.author
                 RETURNING id",
                &[&repo_id, &title, &reason, &decided_on, &source, &author],
            )?
            .get(0);
        Ok(id)
    }

    pub fn search_decisions(
        &mut self,
        repo: &str,
        query: &str,
        limit: i64,
    ) -> anyhow::Result<Vec<Decision>> {
        let words = rb_ops::keywords(query);
        let rows = self.client.query(
            "SELECT d.id, d.title, d.reason, d.decided_on, d.source, d.author
             FROM rb.decision d JOIN rb.repo r ON r.id = d.repo_id
             WHERE r.name = $1 ORDER BY d.id LIMIT 1000",
            &[&repo],
        )?;
        let mut scored: Vec<(usize, Decision)> = rows
            .iter()
            .map(|r| {
                let d = Decision {
                    id: r.get(0),
                    title: r.get(1),
                    reason: r.get(2),
                    decided_on: r.get(3),
                    source: r.get(4),
                    author: r.get(5),
                };
                let s = rb_ops::decision_score(&words, &d.title, &d.reason);
                (s, d)
            })
            .filter(|(s, _)| *s > 0 || words.is_empty())
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.id.cmp(&b.1.id)));
        scored.truncate(limit.max(1) as usize);
        Ok(scored.into_iter().map(|(_, d)| d).collect())
    }

    pub fn record_deployment(
        &mut self,
        repo: &str,
        service: &str,
        commit_sha: &str,
        version: &str,
        status: &str,
    ) -> anyhow::Result<i64> {
        let repo_id = self.ensure_repo(repo, None)?;
        let id: i64 = self
            .client
            .query_one(
                "INSERT INTO rb.deployment (repo_id, service, commit_sha, version, status)
                 VALUES ($1,$2,$3,$4,$5) RETURNING id",
                &[&repo_id, &service, &commit_sha, &version, &status],
            )?
            .get(0);
        Ok(id)
    }

    /// Phase 4: the newest deployments first (at most `limit`), for the context package.
    pub fn list_deployments(
        &mut self,
        repo: &str,
        service: Option<&str>,
        limit: i64,
    ) -> anyhow::Result<Vec<Deployment>> {
        let rows = match service {
            Some(s) => self.client.query(
                "SELECT d.id, d.service, d.commit_sha, d.version, d.status
                 FROM rb.deployment d JOIN rb.repo r ON r.id = d.repo_id
                 WHERE r.name = $1 AND d.service = $2
                 ORDER BY d.id DESC LIMIT $3",
                &[&repo, &s, &limit],
            )?,
            None => self.client.query(
                "SELECT d.id, d.service, d.commit_sha, d.version, d.status
                 FROM rb.deployment d JOIN rb.repo r ON r.id = d.repo_id
                 WHERE r.name = $1 ORDER BY d.id DESC LIMIT $2",
                &[&repo, &limit],
            )?,
        };
        Ok(rows
            .iter()
            .map(|r| Deployment {
                id: r.get(0),
                service: r.get(1),
                commit_sha: r.get(2),
                version: r.get(3),
                status: r.get(4),
            })
            .collect())
    }

    pub fn record_incident(
        &mut self,
        repo: &str,
        service: &str,
        deployment_id: Option<i64>,
        severity: &str,
        summary: &str,
    ) -> anyhow::Result<i64> {
        let repo_id = self.ensure_repo(repo, None)?;
        let id: i64 = self
            .client
            .query_one(
                "INSERT INTO rb.incident (repo_id, service, deployment_id, severity, summary)
                 VALUES ($1,$2,$3,$4,$5) RETURNING id",
                &[&repo_id, &service, &deployment_id, &severity, &summary],
            )?
            .get(0);
        Ok(id)
    }

    pub fn list_incidents(
        &mut self,
        repo: &str,
        service: Option<&str>,
    ) -> anyhow::Result<Vec<Incident>> {
        let rows = match service {
            Some(s) => self.client.query(
                "SELECT i.id, i.service, i.deployment_id, i.severity, i.summary
                 FROM rb.incident i JOIN rb.repo r ON r.id = i.repo_id
                 WHERE r.name = $1 AND i.service = $2 ORDER BY i.id DESC LIMIT 100",
                &[&repo, &s],
            )?,
            None => self.client.query(
                "SELECT i.id, i.service, i.deployment_id, i.severity, i.summary
                 FROM rb.incident i JOIN rb.repo r ON r.id = i.repo_id
                 WHERE r.name = $1 ORDER BY i.id DESC LIMIT 100",
                &[&repo],
            )?,
        };
        Ok(rows
            .iter()
            .map(|r| Incident {
                id: r.get(0),
                service: r.get(1),
                deployment_id: r.get(2),
                severity: r.get(3),
                summary: r.get(4),
            })
            .collect())
    }

    /// Phase 7: blame an incident on its deployment. Returns the incident (latest
    /// for the service by default), its deployment if linked, and how many
    /// incidents share that deployment (the customer-impact scope).
    pub fn incident_blame(
        &mut self,
        repo: &str,
        service: &str,
        incident_id: Option<i64>,
    ) -> anyhow::Result<Option<(Incident, Option<Deployment>, i64)>> {
        let incident = match incident_id {
            Some(id) => self
                .client
                .query_opt(
                    "SELECT i.id, i.service, i.deployment_id, i.severity, i.summary
                     FROM rb.incident i JOIN rb.repo r ON r.id = i.repo_id
                     WHERE r.name = $1 AND i.id = $2",
                    &[&repo, &id],
                )?
                .map(|r| Incident {
                    id: r.get(0),
                    service: r.get(1),
                    deployment_id: r.get(2),
                    severity: r.get(3),
                    summary: r.get(4),
                })
                .filter(|i| i.service == service),
            None => self.list_incidents(repo, Some(service))?.into_iter().next(),
        };
        let Some(incident) = incident else {
            return Ok(None);
        };
        let deployment = match incident.deployment_id {
            Some(did) => self
                .client
                .query_opt(
                    "SELECT d.id, d.service, d.commit_sha, d.version, d.status
                     FROM rb.deployment d JOIN rb.repo r ON r.id = d.repo_id
                     WHERE r.name = $1 AND d.id = $2",
                    &[&repo, &did],
                )?
                .map(|r| Deployment {
                    id: r.get(0),
                    service: r.get(1),
                    commit_sha: r.get(2),
                    version: r.get(3),
                    status: r.get(4),
                }),
            None => None,
        };
        let siblings: i64 = match incident.deployment_id {
            Some(did) => self
                .client
                .query_one(
                    "SELECT count(*) FROM rb.incident i JOIN rb.repo r ON r.id = i.repo_id
                     WHERE r.name = $1 AND i.deployment_id = $2",
                    &[&repo, &did],
                )?
                .get(0),
            None => 1,
        };
        Ok(Some((incident, deployment, siblings)))
    }

    pub fn record_feature(
        &mut self,
        repo: &str,
        name: &str,
        description: &str,
    ) -> anyhow::Result<i64> {
        let repo_id = self.ensure_repo(repo, None)?;
        let id: i64 = self
            .client
            .query_one(
                "INSERT INTO rb.feature (repo_id, name, description) VALUES ($1,$2,$3)
                 ON CONFLICT (repo_id, name) DO UPDATE SET description=EXCLUDED.description
                 RETURNING id",
                &[&repo_id, &name, &description],
            )?
            .get(0);
        Ok(id)
    }

    pub fn link_feature(
        &mut self,
        repo: &str,
        name: &str,
        kind: &str,
        target: &str,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            matches!(
                kind,
                "service" | "endpoint" | "table" | "deployment" | "metric"
            ),
            "unknown feature link kind {kind:?}"
        );
        let repo_id = self.ensure_repo(repo, None)?;
        let fid: i64 = self
            .client
            .query_one(
                "SELECT id FROM rb.feature WHERE repo_id = $1 AND name = $2",
                &[&repo_id, &name],
            )
            .map(|r| r.get(0))?;
        self.client.execute(
            "INSERT INTO rb.feature_link (feature_id, kind, target) VALUES ($1,$2,$3)
             ON CONFLICT DO NOTHING",
            &[&fid, &kind, &target],
        )?;
        Ok(())
    }

    pub fn show_feature(&mut self, repo: &str, name: &str) -> anyhow::Result<Feature> {
        let repo_id = self.ensure_repo(repo, None)?;
        let row = self
            .client
            .query_opt(
                "SELECT id, description FROM rb.feature WHERE repo_id = $1 AND name = $2",
                &[&repo_id, &name],
            )?
            .ok_or_else(|| anyhow::anyhow!("no feature {name:?} in {repo:?}"))?;
        let id: i64 = row.get(0);
        let links: Vec<(String, String)> = self
            .client
            .query(
                "SELECT kind, target FROM rb.feature_link WHERE feature_id = $1
                 ORDER BY kind, target",
                &[&id],
            )?
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect();
        Ok(Feature {
            id,
            name: name.to_owned(),
            description: row.get(1),
            links,
        })
    }
}

impl Store {
    fn phase_service_keys(&mut self, snap: i64) -> Vec<String> {
        self.services(snap)
            .map(|v| v.into_iter().map(|s| s.key).collect())
            .unwrap_or_default()
    }

    fn phase_find_keys(&mut self, snap: i64, q: &str, kind: &str) -> Vec<String> {
        let k = kind.parse().ok();
        self.find(snap, q, k, 5, 0)
            .map(|p| p.nodes.into_iter().map(|n| n.key).collect())
            .unwrap_or_default()
    }

    /// Phase 4: the context package for a task. Deterministic in the live snapshot.
    pub fn context_package(&mut self, repo: &str, task: &str) -> anyhow::Result<Value> {
        let keywords = rb_ops::keywords(task);
        let snap = self.live_snapshot(repo)?.map(|s| s.id);
        let mut services = Vec::new();
        let mut endpoints = Vec::new();
        let mut tables = Vec::new();
        if let Some(id) = snap {
            services = self.phase_service_keys(id);
            for w in keywords.iter().take(4) {
                endpoints.extend(self.phase_find_keys(id, w, "endpoint"));
                tables.extend(self.phase_find_keys(id, w, "table"));
            }
            endpoints.sort();
            endpoints.dedup();
            endpoints.truncate(10);
            tables.sort();
            tables.dedup();
            tables.truncate(10);
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
        let decisions = self
            .search_decisions(repo, task, 5)
            .unwrap_or_default()
            .into_iter()
            .map(|d| {
                serde_json::json!({"title": d.title, "reason": d.reason, "source": d.source, "author": d.author})
            })
            .collect::<Vec<_>>();
        let incidents = self
            .list_incidents(repo, None)
            .unwrap_or_default()
            .into_iter()
            .take(5)
            .map(|i| serde_json::json!({"service": i.service, "severity": i.severity, "summary": i.summary}))
            .collect::<Vec<_>>();
        let deployments = self
            .list_deployments(repo, None, 5)
            .unwrap_or_default()
            .into_iter()
            .map(|d| {
                serde_json::json!({"service": d.service, "commit": d.commit_sha, "version": d.version, "status": d.status})
            })
            .collect::<Vec<_>>();
        Ok(serde_json::json!({
            "task": task,
            "keywords": keywords,
            "services": services,
            "endpoints": endpoints,
            "tables": tables,
            "decisions": decisions,
            "incidents": incidents,
            "deployments": deployments,
            "snapshot": snap,
        }))
    }

    /// Phase 5: a deterministic agent plan drawn only from the context package.
    pub fn agent_plan(&mut self, repo: &str, task: &str) -> anyhow::Result<Value> {
        let ctx = self.context_package(repo, task)?;
        let strs = |k: &str| {
            ctx[k]
                .as_array()
                .unwrap_or(&vec![])
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect::<Vec<_>>()
        };
        let (services, endpoints, tables) = (strs("services"), strs("endpoints"), strs("tables"));
        let steps = rb_ops::plan_from_context(&services, &endpoints, &tables, 8);
        let agents: Vec<&str> = rb_ops::route_task(task)
            .into_iter()
            .map(rb_ops::Agent::as_str)
            .collect();
        Ok(serde_json::json!({
            "task": task,
            "agents": agents,
            "context": ctx,
            "steps": steps.into_iter().map(|s| {
                serde_json::json!({"tool": s.tool, "target": s.target, "why": s.why})
            }).collect::<Vec<_>>(),
            "capabilities": ["read", "edit", "execute", "test"],
        }))
    }

    /// Phase 9: orchestrate the routed agents, each with its evidence slice.
    pub fn orchestrate_plan(&mut self, repo: &str, task: &str) -> anyhow::Result<Value> {
        let plan = self.agent_plan(repo, task)?;
        let ctx = &plan["context"];
        let arr = |k: &str| ctx[k].as_array().cloned().unwrap_or_default();
        let subtasks: Vec<Value> = plan["agents"]
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .map(|a| {
                let scope = match a.as_str() {
                    Some("database") => {
                        serde_json::json!({"tables": arr("tables"), "decisions": arr("decisions")})
                    }
                    Some("frontend") => {
                        serde_json::json!({"endpoints": arr("endpoints"), "services": arr("services")})
                    }
                    Some("security") => {
                        serde_json::json!({"endpoints": arr("endpoints"), "decisions": arr("decisions")})
                    }
                    Some("qa") => {
                        serde_json::json!({"endpoints": arr("endpoints"), "tables": arr("tables")})
                    }
                    _ => serde_json::json!({"services": arr("services"), "incidents": arr("incidents")}),
                };
                serde_json::json!({
                    "agent": a,
                    "task": task,
                    "scope": scope,
                    "verification": "rb verify",
                })
            })
            .collect();
        Ok(serde_json::json!({"task": task, "subtasks": subtasks, "plan": plan}))
    }
}
