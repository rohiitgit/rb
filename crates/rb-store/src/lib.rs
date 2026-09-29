//! Postgres storage for the Repository Brain (design §8).
//!
//! The relational schema `rb.*` is the source of truth; each live snapshot also gets an AGE graph
//! projection. A snapshot is published in a single transaction (rows, AGE graph, live-pointer flip),
//! so readers never see a partial graph and a failed load leaves nothing behind but a `failed` row.

mod age;
mod query;

use anyhow::{Context, bail};
use postgres::binary_copy::BinaryCopyInWriter;
use postgres::types::Type;
use postgres::{Client, NoTls};
use std::collections::HashMap;

use rb_model::{
    Certainty, Diagnostic, Edge, EdgeKind, Evidence, Graph, Node, NodeKind, Provenance,
};
use serde_json::Value;

pub use age::{LoadDir, graph_name};
pub use query::{
    FoundNode, OwnerShare, Resolved, ServiceCard, ServiceSummary, SqlCoverage, TableAccess,
    TableDeps,
};

/// Embedded migrations, applied in order and recorded in `public.rb_schema_migrations`.
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_init",
        include_str!("../../../migrations/0001_init.sql"),
    ),
    (
        "0002_evidence_paths",
        include_str!("../../../migrations/0002_evidence_paths.sql"),
    ),
    (
        "0003_trgm_schema",
        include_str!("../../../migrations/0003_trgm_schema.sql"),
    ),
];

/// Name of the newest embedded migration; recorded in each snapshot's `tool_versions`.
pub fn schema_version() -> &'static str {
    MIGRATIONS[MIGRATIONS.len() - 1].0
}

/// Retired snapshots kept per repo (plus the live one) before being dropped (design §8.3).
pub const KEEP_RETIRED: i64 = 2;

/// Arbitrary constant for `pg_advisory_xact_lock` so concurrent `rb migrate` runs serialise.
const MIGRATION_LOCK: i64 = 0x7262_6d69_6772;

#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotInfo {
    pub id: i64,
    pub repo: String,
    pub commit_sha: String,
    pub status: String,
    pub tool_versions: Value,
    pub age_graph: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepoStatus {
    pub name: String,
    pub live: Option<SnapshotInfo>,
    pub nodes: i64,
    pub edges: i64,
    pub diagnostics: i64,
    pub snapshots: i64,
}

pub struct Store {
    client: Client,
}

impl Store {
    pub fn connect(url: &str) -> anyhow::Result<Self> {
        let mut client = Client::connect(url, NoTls)
            .with_context(|| format!("connecting to {}", redact(url)))?;
        // AGE's cypher() and agtype live in ag_catalog; rb.* is always schema-qualified.
        client.batch_execute(r#"SET search_path = ag_catalog, "$user", public"#)?;
        Ok(Self { client })
    }

    /// Applies pending migrations; returns the names applied by this call.
    pub fn migrate(&mut self) -> anyhow::Result<Vec<&'static str>> {
        let mut tx = self.client.transaction()?;
        tx.execute("SELECT pg_advisory_xact_lock($1)", &[&MIGRATION_LOCK])?;
        // Objects created by migrations must land in `public`, not in AGE's ag_catalog.
        tx.batch_execute("SET LOCAL search_path = public")?;
        tx.batch_execute(
            "CREATE TABLE IF NOT EXISTS public.rb_schema_migrations (
               name text PRIMARY KEY, applied_at timestamptz NOT NULL DEFAULT now())",
        )?;
        let mut applied = Vec::new();
        for (name, sql) in MIGRATIONS {
            let done = tx
                .query_opt(
                    "SELECT 1 FROM public.rb_schema_migrations WHERE name = $1",
                    &[name],
                )?
                .is_some();
            if !done {
                tx.batch_execute(sql)
                    .with_context(|| format!("applying migration {name}"))?;
                tx.execute(
                    "INSERT INTO public.rb_schema_migrations (name) VALUES ($1)",
                    &[name],
                )?;
                applied.push(*name);
            }
        }
        tx.commit()?;
        Ok(applied)
    }

    pub fn ensure_repo(&mut self, name: &str, origin_url: Option<&str>) -> anyhow::Result<i64> {
        let row = self.client.query_one(
            "INSERT INTO rb.repo (name, origin_url) VALUES ($1, $2)
             ON CONFLICT (name) DO UPDATE SET origin_url = COALESCE(EXCLUDED.origin_url, rb.repo.origin_url)
             RETURNING id",
            &[&name, &origin_url],
        )?;
        Ok(row.get(0))
    }

    pub fn repo_names(&mut self) -> anyhow::Result<Vec<String>> {
        let rows = self
            .client
            .query("SELECT name FROM rb.repo ORDER BY name COLLATE \"C\"", &[])?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// Deletes a repository, all its snapshots and their AGE graphs. Returns false if it did not exist.
    pub fn delete_repo(&mut self, name: &str) -> anyhow::Result<bool> {
        let mut tx = self.client.transaction()?;
        let graphs = tx.query(
            "SELECT s.age_graph FROM rb.snapshot s JOIN rb.repo r ON r.id = s.repo_id
             WHERE r.name = $1 AND s.age_graph IS NOT NULL",
            &[&name],
        )?;
        for row in graphs {
            age::drop_graph_if_exists(&mut tx, row.get(0))?;
        }
        // Clear the live pointer first so snapshot rows can cascade away with the repo.
        tx.execute(
            "UPDATE rb.repo SET live_snapshot_id = NULL WHERE name = $1",
            &[&name],
        )?;
        let deleted = tx.execute("DELETE FROM rb.repo WHERE name = $1", &[&name])? > 0;
        tx.commit()?;
        Ok(deleted)
    }

    pub fn live_snapshot(&mut self, repo: &str) -> anyhow::Result<Option<SnapshotInfo>> {
        let row = self.client.query_opt(
            &format!("{SNAPSHOT_SELECT} WHERE r.name = $1 AND s.id = r.live_snapshot_id"),
            &[&repo],
        )?;
        Ok(row.as_ref().map(snapshot_from_row))
    }

    pub fn snapshot(&mut self, id: i64) -> anyhow::Result<SnapshotInfo> {
        let row = self
            .client
            .query_opt(&format!("{SNAPSHOT_SELECT} WHERE s.id = $1"), &[&id])?
            .with_context(|| format!("snapshot {id} does not exist"))?;
        Ok(snapshot_from_row(&row))
    }

    /// Publishes `graph` as the repo's new live snapshot: rows + AGE projection + live flip in one
    /// transaction, then retention. On failure the snapshot row is marked `failed`.
    pub fn publish_snapshot(
        &mut self,
        repo: &str,
        commit_sha: &str,
        tool_versions: &Value,
        graph: &Graph,
        diagnostics: &[Diagnostic],
        load_dir: &LoadDir,
    ) -> anyhow::Result<SnapshotInfo> {
        let repo_id = self.ensure_repo(repo, None)?;
        let parent: Option<i64> = self
            .client
            .query_one(
                "SELECT live_snapshot_id FROM rb.repo WHERE id = $1",
                &[&repo_id],
            )?
            .get(0);
        let snapshot_id: i64 = self
            .client
            .query_one(
                "INSERT INTO rb.snapshot (repo_id, commit_sha, parent_id, status, tool_versions)
                 VALUES ($1, $2, $3, 'building', $4) RETURNING id",
                &[&repo_id, &commit_sha, &parent, tool_versions],
            )?
            .get(0);
        let graph_name = graph_name(repo, snapshot_id);

        if let Err(err) = self.load_and_flip(
            repo_id,
            snapshot_id,
            &graph_name,
            graph,
            diagnostics,
            load_dir,
        ) {
            // The transaction rolled back (rows and AGE graph); keep a record of the attempt.
            self.client
                .execute(
                    "UPDATE rb.snapshot SET status = 'failed' WHERE id = $1",
                    &[&snapshot_id],
                )
                .context("marking snapshot failed")?;
            return Err(err.context(format!("publishing snapshot {snapshot_id}")));
        }
        self.apply_retention(repo_id)?;
        self.snapshot(snapshot_id)
    }

    fn load_and_flip(
        &mut self,
        repo_id: i64,
        snapshot_id: i64,
        graph_name: &str,
        graph: &Graph,
        diagnostics: &[Diagnostic],
        load_dir: &LoadDir,
    ) -> anyhow::Result<()> {
        let mut tx = self.client.transaction()?;
        // Serialise publishers of the same repo.
        tx.execute(
            "SELECT 1 FROM rb.repo WHERE id = $1 FOR UPDATE",
            &[&repo_id],
        )?;

        let sink = tx.copy_in(
            "COPY rb.node (snapshot_id, id, kind, key, name, attrs) FROM STDIN (FORMAT binary)",
        )?;
        let mut w = BinaryCopyInWriter::new(
            sink,
            &[
                Type::INT8,
                Type::INT8,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::JSONB,
            ],
        );
        for n in graph.nodes() {
            let attrs = Value::Object(n.attrs.clone());
            w.write(&[
                &snapshot_id,
                &n.id,
                &n.kind.as_str(),
                &n.key,
                &n.name,
                &attrs,
            ])?;
        }
        w.finish().context("copying nodes")?;

        let sink = tx.copy_in(
            "COPY rb.edge (snapshot_id, src, dst, kind, provenance, certainty, attrs) FROM STDIN (FORMAT binary)",
        )?;
        let mut w = BinaryCopyInWriter::new(
            sink,
            &[
                Type::INT8,
                Type::INT8,
                Type::INT8,
                Type::TEXT,
                Type::TEXT,
                Type::INT2,
                Type::JSONB,
            ],
        );
        for e in graph.edges() {
            let attrs = Value::Object(e.attrs.clone());
            w.write(&[
                &snapshot_id,
                &e.src,
                &e.dst,
                &e.kind.as_str(),
                &e.provenance.as_str(),
                &e.certainty.rank(),
                &attrs,
            ])?;
        }
        w.finish().context("copying edges")?;

        let file_ids: HashMap<&str, i64> = graph
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::File)
            .map(|n| (n.key.as_str(), n.id))
            .collect();

        let sink = tx.copy_in(
            "COPY rb.edge_evidence (snapshot_id, src, kind, dst, provenance, file_id, span) FROM STDIN (FORMAT binary)",
        )?;
        let mut w = BinaryCopyInWriter::new(
            sink,
            &[
                Type::INT8,
                Type::INT8,
                Type::TEXT,
                Type::INT8,
                Type::TEXT,
                Type::INT8,
                Type::INT4_ARRAY,
            ],
        );
        for e in graph.edges() {
            for ev in &e.evidence {
                let file_id = file_ids
                    .get(ev.path.as_str())
                    .with_context(|| format!("evidence file {:?} is not a node", ev.path))?;
                w.write(&[
                    &snapshot_id,
                    &e.src,
                    &e.kind.as_str(),
                    &e.dst,
                    &e.provenance.as_str(),
                    file_id,
                    &ev.span.to_vec(),
                ])?;
            }
        }
        w.finish().context("copying edge evidence")?;

        let sink = tx.copy_in(
            "COPY rb.diagnostic (snapshot_id, kind, path, file_id, span, detail) FROM STDIN (FORMAT binary)",
        )?;
        let mut w = BinaryCopyInWriter::new(
            sink,
            &[
                Type::INT8,
                Type::TEXT,
                Type::TEXT,
                Type::INT8,
                Type::INT4_ARRAY,
                Type::JSONB,
            ],
        );
        for d in diagnostics {
            let file_id = d.path.as_deref().and_then(|p| file_ids.get(p));
            let span = d.span.map(|s| s.to_vec());
            w.write(&[&snapshot_id, &d.kind, &d.path, &file_id, &span, &d.detail])?;
        }
        w.finish().context("copying diagnostics")?;

        age::project(&mut tx, graph, graph_name, load_dir).with_context(|| {
            format!(
                "projecting snapshot into AGE (the server must see {} as /tmp/age/{})",
                load_dir.host.display(),
                load_dir.server_rel
            )
        })?;

        tx.execute(
            "UPDATE rb.snapshot SET status = 'retired' WHERE repo_id = $1 AND status = 'live'",
            &[&repo_id],
        )?;
        tx.execute(
            "UPDATE rb.snapshot SET status = 'live', age_graph = $2 WHERE id = $1",
            &[&snapshot_id, &graph_name],
        )?;
        tx.execute(
            "UPDATE rb.repo SET live_snapshot_id = $2 WHERE id = $1",
            &[&repo_id, &snapshot_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Drops snapshots beyond the live one plus [`KEEP_RETIRED`] retired ones, and all failed ones.
    fn apply_retention(&mut self, repo_id: i64) -> anyhow::Result<()> {
        let mut tx = self.client.transaction()?;
        let doomed = tx.query(
            "SELECT id, age_graph FROM rb.snapshot WHERE repo_id = $1 AND (
               status = 'failed'
               OR (status = 'retired' AND id NOT IN (
                     SELECT id FROM rb.snapshot WHERE repo_id = $1 AND status = 'retired'
                     ORDER BY id DESC LIMIT $2)))",
            &[&repo_id, &KEEP_RETIRED],
        )?;
        for row in doomed {
            let id: i64 = row.get(0);
            if let Some(g) = row.get::<_, Option<String>>(1) {
                age::drop_graph_if_exists(&mut tx, &g)?;
            }
            tx.execute("DELETE FROM rb.snapshot WHERE id = $1", &[&id])?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Cached syntax facts for `(blob, extractor)` rows; missing rows are simply absent.
    pub fn load_facts(
        &mut self,
        rows: &[(String, String)],
    ) -> anyhow::Result<HashMap<(String, String), Vec<u8>>> {
        let (blobs, extractors): (Vec<&str>, Vec<&str>) =
            rows.iter().map(|(b, e)| (b.as_str(), e.as_str())).unzip();
        Ok(self
            .client
            .query(
                "SELECT c.blob_sha, c.extractor, c.facts
                 FROM rb.file_facts_cache c
                 JOIN unnest($1::text[], $2::text[]) AS k(blob_sha, extractor)
                   USING (blob_sha, extractor)",
                &[&blobs, &extractors],
            )?
            .iter()
            .map(|r| ((r.get(0), r.get(1)), r.get(2)))
            .collect())
    }

    /// Stores syntax facts. Rows are content-addressed, so an existing row is already correct.
    pub fn save_facts(&mut self, rows: &[(String, String, Vec<u8>)]) -> anyhow::Result<()> {
        let blobs: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        let extractors: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
        let facts: Vec<&[u8]> = rows.iter().map(|r| r.2.as_slice()).collect();
        self.client.execute(
            "INSERT INTO rb.file_facts_cache (blob_sha, extractor, facts)
             SELECT * FROM unnest($1::text[], $2::text[], $3::bytea[])
             ON CONFLICT (blob_sha, extractor) DO NOTHING",
            &[&blobs, &extractors, &facts],
        )?;
        Ok(())
    }

    /// Reads a snapshot back into a [`Graph`] (used by `rb dump` and the determinism tests).
    pub fn load_graph(&mut self, snapshot_id: i64) -> anyhow::Result<Graph> {
        let nodes = self
            .client
            .query(
                "SELECT id, kind, key, name, attrs FROM rb.node WHERE snapshot_id = $1",
                &[&snapshot_id],
            )?
            .iter()
            .map(|r| {
                Ok(Node {
                    id: r.get(0),
                    kind: r.get::<_, String>(1).parse()?,
                    key: r.get(2),
                    name: r.get(3),
                    attrs: into_object(r.get(4))?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let edges = self
            .client
            .query(
                "SELECT src, dst, kind, provenance, certainty, attrs FROM rb.edge WHERE snapshot_id = $1",
                &[&snapshot_id],
            )?
            .iter()
            .map(|r| {
                Ok(Edge {
                    src: r.get(0),
                    dst: r.get(1),
                    kind: r.get::<_, String>(2).parse()?,
                    provenance: r.get::<_, String>(3).parse()?,
                    certainty: Certainty::from_rank(r.get(4))?,
                    attrs: into_object(r.get(5))?,
                    evidence: Vec::new(),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let mut evidence: HashMap<(i64, EdgeKind, i64, Provenance), Vec<Evidence>> = HashMap::new();
        for r in self.client.query(
            "SELECT ev.src, ev.kind, ev.dst, ev.provenance, f.key, ev.span
             FROM rb.edge_evidence ev
             JOIN rb.node f ON f.snapshot_id = ev.snapshot_id AND f.id = ev.file_id
             WHERE ev.snapshot_id = $1",
            &[&snapshot_id],
        )? {
            let key = (
                r.get(0),
                r.get::<_, String>(1).parse()?,
                r.get(2),
                r.get::<_, String>(3).parse()?,
            );
            evidence.entry(key).or_default().push(Evidence {
                path: r.get(4),
                span: span_from_sql(r.get(5))?,
            });
        }
        let mut edges = edges;
        for e in &mut edges {
            if let Some(ev) = evidence.remove(&(e.src, e.kind, e.dst, e.provenance)) {
                e.evidence = ev;
            }
        }
        anyhow::ensure!(evidence.is_empty(), "evidence rows without a matching edge");
        Graph::from_parts(nodes, edges)
    }

    /// Reads a snapshot's diagnostics, in canonical order.
    pub fn load_diagnostics(&mut self, snapshot_id: i64) -> anyhow::Result<Vec<Diagnostic>> {
        let mut out = self
            .client
            .query(
                "SELECT kind, path, span, detail FROM rb.diagnostic WHERE snapshot_id = $1",
                &[&snapshot_id],
            )?
            .iter()
            .map(|r| {
                Ok(Diagnostic {
                    kind: r.get(0),
                    path: r.get(1),
                    span: r
                        .get::<_, Option<Vec<i32>>>(2)
                        .map(span_from_sql)
                        .transpose()?,
                    detail: r.get(3),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        rb_model::sort_diagnostics(&mut out);
        Ok(out)
    }

    /// Runs a read-only Cypher query against the snapshot's AGE graph. Returns a header row followed
    /// by one row per result, each value rendered as agtype text.
    pub fn cypher(
        &mut self,
        snapshot_id: i64,
        query: &str,
        columns: &[String],
    ) -> anyhow::Result<Vec<Vec<String>>> {
        let snap = self.snapshot(snapshot_id)?;
        let Some(graph) = snap.age_graph else {
            bail!("snapshot {snapshot_id} has no AGE projection");
        };
        let sql = age::cypher_sql(&graph, query, columns)?;
        let mut tx = self.client.build_transaction().read_only(true).start()?;
        tx.batch_execute("SET LOCAL statement_timeout = '2s'")?;
        let rows = tx.query(sql.as_str(), &[])?;
        tx.commit()?;
        let mut out = vec![columns.to_vec()];
        for r in rows {
            out.push(
                (0..columns.len())
                    .map(|i| {
                        r.get::<_, Option<String>>(i)
                            .unwrap_or_else(|| "null".into())
                    })
                    .collect(),
            );
        }
        Ok(out)
    }

    pub fn status(&mut self, repo: &str) -> anyhow::Result<RepoStatus> {
        let live = self.live_snapshot(repo)?;
        let snapshots: i64 = self
            .client
            .query_one(
                "SELECT count(*) FROM rb.snapshot s JOIN rb.repo r ON r.id = s.repo_id WHERE r.name = $1",
                &[&repo],
            )?
            .get(0);
        let (mut nodes, mut edges, mut diagnostics) = (0, 0, 0);
        if let Some(s) = &live {
            let row = self.client.query_one(
                "SELECT (SELECT count(*) FROM rb.node WHERE snapshot_id = $1),
                        (SELECT count(*) FROM rb.edge WHERE snapshot_id = $1),
                        (SELECT count(*) FROM rb.diagnostic WHERE snapshot_id = $1)",
                &[&s.id],
            )?;
            (nodes, edges, diagnostics) = (row.get(0), row.get(1), row.get(2));
        }
        Ok(RepoStatus {
            name: repo.to_owned(),
            live,
            nodes,
            edges,
            diagnostics,
            snapshots,
        })
    }
}

const SNAPSHOT_SELECT: &str =
    "SELECT s.id, r.name, s.commit_sha, s.status, s.tool_versions, s.age_graph
                               FROM rb.snapshot s JOIN rb.repo r ON r.id = s.repo_id";

fn snapshot_from_row(r: &postgres::Row) -> SnapshotInfo {
    SnapshotInfo {
        id: r.get(0),
        repo: r.get(1),
        commit_sha: r.get(2),
        status: r.get(3),
        tool_versions: r.get(4),
        age_graph: r.get(5),
    }
}

fn span_from_sql(v: Vec<i32>) -> anyhow::Result<rb_model::Span> {
    v.try_into()
        .map_err(|v: Vec<i32>| anyhow::anyhow!("span must have 4 elements, got {}", v.len()))
}

fn into_object(v: Value) -> anyhow::Result<rb_model::Attrs> {
    match v {
        Value::Object(m) => Ok(m),
        other => bail!("attrs must be a JSON object, got {other}"),
    }
}

/// Hides the password in a `postgresql://user:pass@host/db` URL for error messages.
fn redact(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(s), Some(at)) if at > s => {
            let creds = &url[s + 3..at];
            match creds.split_once(':') {
                Some((user, _)) => format!("{}{user}:***{}", &url[..s + 3], &url[at..]),
                None => url.to_owned(),
            }
        }
        _ => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_passwords() {
        assert_eq!(
            redact("postgresql://rb:secret@h:1/db"),
            "postgresql://rb:***@h:1/db"
        );
        assert_eq!(redact("postgresql://rb@h/db"), "postgresql://rb@h/db");
        assert_eq!(redact("host=h user=rb"), "host=h user=rb");
    }
}
