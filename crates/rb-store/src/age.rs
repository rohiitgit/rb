//! Apache AGE projection of a snapshot (design §8.4).
//!
//! Vertices carry only `{nid, key, name}` and edges `{provenance, certainty}`; everything else is
//! looked up relationally by `nid`. Loading goes through AGE's CSV loaders, which (in AGE 1.8) only
//! read files under the server's `/tmp/age/`. The indexer writes the CSVs into a host directory that
//! is mounted there (see `compose.yaml`).

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, bail, ensure};
use postgres::Transaction;
use rb_model::{EdgeKind, Graph, NodeKind};
use serde_json::Value;

/// Where projection CSVs are written, as seen from the indexer and from the Postgres server.
#[derive(Debug, Clone)]
pub struct LoadDir {
    /// Host directory, mounted into the server under `/tmp/age/<server_rel>`.
    pub host: PathBuf,
    /// Path relative to the server's `/tmp/age/`.
    pub server_rel: String,
}

impl LoadDir {
    pub fn new(host: impl Into<PathBuf>) -> Self {
        Self {
            host: host.into(),
            server_rel: "rb-load".into(),
        }
    }
}

/// AGE graph name for a snapshot. Repo names are constrained to `[a-z0-9_]{1,40}` by the schema.
pub fn graph_name(repo: &str, snapshot_id: i64) -> String {
    format!("rb_{repo}_{snapshot_id}")
}

/// Creates the graph with every label (so the schema is stable even when a label has no rows),
/// then bulk-loads the non-empty ones. Runs inside the caller's transaction.
pub(crate) fn project(
    tx: &mut Transaction<'_>,
    graph: &Graph,
    name: &str,
    dir: &LoadDir,
) -> anyhow::Result<()> {
    let host_dir = dir.host.join(name);
    fs::create_dir_all(&host_dir).with_context(|| format!("creating {}", host_dir.display()))?;
    let result = write_and_load(tx, graph, name, dir, &host_dir);
    // The server has read the files by the time the load calls return; always clean up.
    let cleanup =
        fs::remove_dir_all(&host_dir).with_context(|| format!("removing {}", host_dir.display()));
    result.and(cleanup)
}

fn write_and_load(
    tx: &mut Transaction<'_>,
    graph: &Graph,
    name: &str,
    dir: &LoadDir,
    host_dir: &Path,
) -> anyhow::Result<()> {
    lock_ddl(tx)?;
    tx.execute("SELECT ag_catalog.create_graph($1::text::name)", &[&name])?;
    for kind in NodeKind::ALL {
        tx.execute(
            "SELECT ag_catalog.create_vlabel($1::text::cstring, $2::text::cstring)",
            &[&name, &kind.age_label()],
        )?;
    }
    for kind in EdgeKind::ALL {
        tx.execute(
            "SELECT ag_catalog.create_elabel($1::text::cstring, $2::text::cstring)",
            &[&name, &kind.as_str()],
        )?;
    }

    let server_path = |file: &str| format!("{}/{name}/{file}", dir.server_rel);

    let mut vertices: BTreeMap<&str, csv::Writer<fs::File>> = BTreeMap::new();
    let mut label_of = HashMap::with_capacity(graph.nodes().len());
    for n in graph.nodes() {
        let label = n.kind.age_label();
        label_of.insert(n.id, label);
        let w = match vertices.entry(label) {
            std::collections::btree_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::btree_map::Entry::Vacant(v) => {
                let mut w = csv::Writer::from_path(host_dir.join(format!("{label}.csv")))?;
                w.write_record(["id", "nid", "key", "name"])?;
                v.insert(w)
            }
        };
        // `load_as_agtype` mode: each property cell is a JSON value (strings stay quoted).
        w.write_record([
            n.id.to_string(),
            n.id.to_string(),
            Value::from(n.key.as_str()).to_string(),
            Value::from(n.name.as_str()).to_string(),
        ])?;
    }
    for (label, mut w) in vertices {
        w.flush()?;
        tx.execute(
            "SELECT ag_catalog.load_labels_from_file($1::text::name, $2::text::name, $3, true, true)",
            &[&name, &label, &server_path(&format!("{label}.csv"))],
        )
        .with_context(|| format!("loading {label} vertices"))?;
    }

    let mut edges: BTreeMap<&str, csv::Writer<fs::File>> = BTreeMap::new();
    for e in graph.edges() {
        let label = e.kind.as_str();
        let w = match edges.entry(label) {
            std::collections::btree_map::Entry::Occupied(o) => o.into_mut(),
            std::collections::btree_map::Entry::Vacant(v) => {
                let mut w = csv::Writer::from_path(host_dir.join(format!("{label}.csv")))?;
                w.write_record([
                    "start_id",
                    "start_vertex_type",
                    "end_id",
                    "end_vertex_type",
                    "provenance",
                    "certainty",
                ])?;
                v.insert(w)
            }
        };
        w.write_record([
            e.src.to_string(),
            label_of[&e.src].to_string(),
            e.dst.to_string(),
            label_of[&e.dst].to_string(),
            Value::from(e.provenance.as_str()).to_string(),
            e.certainty.rank().to_string(),
        ])?;
    }
    for (label, mut w) in edges {
        w.flush()?;
        tx.execute(
            "SELECT ag_catalog.load_edges_from_file($1::text::name, $2::text::name, $3, true)",
            &[&name, &label, &server_path(&format!("{label}.csv"))],
        )
        .with_context(|| format!("loading {label} edges"))?;
    }
    Ok(())
}

/// Takes [`crate::RB_AGE_DDL_LOCK`] for the rest of the transaction (contract M5 S2).
fn lock_ddl(tx: &mut Transaction<'_>) -> anyhow::Result<()> {
    tx.execute(
        "SELECT pg_advisory_xact_lock($1)",
        &[&crate::RB_AGE_DDL_LOCK],
    )?;
    Ok(())
}

pub(crate) fn drop_graph_if_exists(tx: &mut Transaction<'_>, name: &str) -> anyhow::Result<()> {
    lock_ddl(tx)?;
    let exists = tx
        .query_opt(
            "SELECT 1 FROM ag_catalog.ag_graph WHERE name = $1::text::name",
            &[&name],
        )?
        .is_some();
    if exists {
        tx.execute(
            "SELECT ag_catalog.drop_graph($1::text::name, true)",
            &[&name],
        )?;
    }
    Ok(())
}

/// Builds the SQL wrapper for an ad-hoc Cypher query. Each returned column is cast to text.
///
/// The query is embedded with a dollar-quote tag it does not contain, so it cannot escape into SQL;
/// the caller additionally runs it in a READ ONLY transaction with a statement timeout.
pub(crate) fn cypher_sql(graph: &str, query: &str, columns: &[String]) -> anyhow::Result<String> {
    ensure!(
        !columns.is_empty(),
        "at least one result column is required"
    );
    for c in columns {
        ensure!(
            is_identifier(c),
            "invalid column name {c:?} (use [a-z_][a-z0-9_]*)"
        );
    }
    ensure!(is_graph_name(graph), "invalid graph name {graph:?}");
    let mut tag = String::from("$rbq$");
    let mut n = 0;
    while query.contains(&tag) {
        n += 1;
        tag = format!("$rbq{n}$");
        if n > 100 {
            bail!("could not find a safe dollar-quote tag for this query");
        }
    }
    let select: Vec<String> = columns.iter().map(|c| format!("(t.{c})::text")).collect();
    let defs: Vec<String> = columns
        .iter()
        .map(|c| format!("{c} ag_catalog.agtype"))
        .collect();
    Ok(format!(
        "SELECT {} FROM ag_catalog.cypher('{graph}', {tag}{query}{tag}) AS t({})",
        select.join(", "),
        defs.join(", ")
    ))
}

fn is_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && s.len() <= 63
}

fn is_graph_name(s: &str) -> bool {
    is_identifier(s) && s.starts_with("rb_")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cypher_sql_quotes_safely() {
        let cols = vec!["svc".to_string(), "n".to_string()];
        let sql = cypher_sql("rb_x_1", "MATCH (n) RETURN n.key, 1", &cols).unwrap();
        assert_eq!(
            sql,
            "SELECT (t.svc)::text, (t.n)::text FROM ag_catalog.cypher('rb_x_1', $rbq$MATCH (n) RETURN n.key, 1$rbq$) \
             AS t(svc ag_catalog.agtype, n ag_catalog.agtype)"
        );
        let sneaky = cypher_sql("rb_x_1", "RETURN 1 $rbq$; DROP TABLE x; --", &cols[..1]).unwrap();
        assert!(sneaky.contains("$rbq1$RETURN 1 $rbq$; DROP TABLE x; --$rbq1$"));
    }

    #[test]
    fn cypher_sql_rejects_bad_identifiers() {
        assert!(cypher_sql("rb_x_1", "RETURN 1", &["a b".into()]).is_err());
        assert!(cypher_sql("rb_x_1", "RETURN 1", &[]).is_err());
        assert!(cypher_sql("x'; --", "RETURN 1", &["a".into()]).is_err());
    }
}
