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
