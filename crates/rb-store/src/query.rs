//! Read queries over a snapshot (design §9.1 node resolution, §9.5 Service Card structure sections).

use std::collections::BTreeMap;

use anyhow::bail;
use rb_model::{NodeKind, ServiceRole};
use serde::Serialize;
use serde_json::Value;

use crate::Store;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FoundNode {
    pub kind: String,
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServiceSummary {
    pub key: String,
    pub name: String,
    #[serde(serialize_with = "role_str")]
    pub role: ServiceRole,
    pub files: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OwnerShare {
    pub owner: String,
    pub files: usize,
}

/// The Service Card (design §9.5): structure, packages and the HTTP sections (contract M4a S20),
/// all computed at query time. Tables, service dependencies and the call graph arrive with M4b.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServiceCard {
    pub key: String,
    pub name: String,
    #[serde(serialize_with = "role_str")]
    pub role: ServiceRole,
    pub detected_by: Vec<String>,
    pub manifests: Vec<String>,
    pub files: usize,
    /// `(language or "other", count)`, sorted by count desc then language.
    pub languages: Vec<(String, usize)>,
    /// Owners of the service itself (via its anchor file), sorted.
    pub owners: Vec<String>,
    /// Per-owner file counts, sorted by count desc then owner. A file can have several owners.
    pub file_owners: Vec<OwnerShare>,
    pub unowned_files: usize,
    /// Keys of the packages the service depends on (`DEPENDS_ON`, design §5.5), sorted.
    pub packages: Vec<String>,
    /// Endpoints it exposes, by key.
    pub apis: Vec<Api>,
    /// Other services' endpoints its code consumes, by service key.
    pub consumes: Vec<HttpPeer>,
    /// Services whose code consumes its endpoints, by service key.
    pub consumed_by: Vec<HttpPeer>,
    /// Its client call sites that were not stitched (`http_*` site diagnostics).
    pub unstitched: i64,
}

/// An endpoint a service exposes, with its handlers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Api {
    pub key: String,
    pub name: String,
    /// `EXPOSES` certainty (the path's).
    pub certainty: String,
    pub prefix_unknown: bool,
    /// By key; empty when the handler was not resolved.
    pub handlers: Vec<Handler>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Handler {
    /// `symbol` or `file` (an inline handler in top-level code).
    pub kind: String,
    pub key: String,
    /// `HANDLED_BY` certainty.
    pub certainty: String,
}

/// A service on the other side of `CONSUMES` edges, and the endpoints involved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HttpPeer {
    pub service_key: String,
    pub service_name: String,
    /// By endpoint key.
    pub endpoints: Vec<HttpUse>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HttpUse {
    pub key: String,
    pub name: String,
    /// The best certainty among the edges.
    pub certainty: String,
    /// Client call sites, summed over the edges.
    pub sites: i64,
}

/// One source's access to a table: a migration file (`MIGRATES`) or code (`QUERIES`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableAccess {
    /// `file` or `symbol`.
    pub source_kind: String,
    pub source_key: String,
    pub source_name: String,
    pub ops: Vec<String>,
    pub certainty: String,
    pub schema_assumed: bool,
    pub file: String,
    /// 1-based line of the first evidence site.
    pub line: i32,
    pub service_key: String,
    pub service_name: String,
}

/// Which code and migrations touch a table (design §9.4, symbol level until M3 adds callers).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TableDeps {
    pub key: String,
    pub name: String,
    pub migrations: Vec<TableAccess>,
    /// ORM classes and models mapped to the table (`MAPS_TO`).
    pub mappings: Vec<TableAccess>,
    pub accesses: Vec<TableAccess>,
}

/// How many call and type references were resolved (design §12.4, contract M3a R8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolutionCoverage {
    /// Distinct reference sites with exactly one target.
    pub resolved: i64,
    /// Distinct reference sites with several candidate targets.
    pub ambiguous: i64,
    /// References to the snapshot's own code that found no target.
    pub unresolved: i64,
    /// References into packages outside the snapshot.
    pub external: i64,
    /// `CALLS` edges with provenance `scip` (exact, from an indexer; contract M3b S9).
    pub exact_calls: i64,
}

impl ResolutionCoverage {
    /// Share of references to the snapshot's own code that found at least one target.
    pub fn resolved_share(&self) -> f64 {
        let found = self.resolved + self.ambiguous;
        let total = found + self.unresolved;
        if total == 0 {
            1.0
        } else {
            found as f64 / total as f64
        }
    }
}

/// SQL coverage of a snapshot (design §12.4), derived from edges and diagnostics. Sites are
/// distinct source locations: a literal touching two tables counts once, and SQL touching no
/// table (e.g. `SET search_path`) leaves no trace in the graph and is not counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct SqlCoverage {
    /// Distinct code sites whose SQL touches at least one table.
    pub query_sites: i64,
    /// Distinct migration statements that touch at least one table.
    pub migration_statements: i64,
    pub unparsed: i64,
    pub tables: i64,
}

/// HTTP coverage of a snapshot (contract M4a S14): its endpoints, the services exposing them,
/// and its client call sites per bucket, as the indexer counted them (`http_coverage`). A site's
/// bucket cannot be read back from the edges: a `CONSUMES` edge carries its best site's
/// certainty.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct HttpCoverage {
    pub routes: i64,
    pub services: i64,
    pub sites: i64,
    /// Bucket → sites; empty for a snapshot without client calls.
    pub buckets: BTreeMap<String, i64>,
}

/// DI coverage of a snapshot (contract M4b S7, amendment 2): injection points per bucket and
/// the gates that capped them, as the indexer counted them (`di_coverage`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct DiCoverage {
    pub points: i64,
    /// Bucket → points; empty for a snapshot indexed before DI.
    pub buckets: BTreeMap<String, i64>,
    pub gates: BTreeMap<String, i64>,
}

/// RTA coverage of a snapshot (contract M4b S9): implementing types considered, those proven
/// never instantiated, the first failing rule of the others, and the `IMPLEMENTS` edges
/// flagged (`rta_coverage`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct RtaCoverage {
    pub types: i64,
    pub proven: i64,
    pub prunable_edges: i64,
    pub kept: BTreeMap<String, i64>,
}

/// Outcome of resolving user input to exactly one node (design §9.1: never pick silently).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    One(FoundNode),
    None,
    Ambiguous(Vec<FoundNode>),
}

impl Store {
    /// Search nodes by key or name (design §9.1): exact matches first, then substring matches ranked
    /// by trigram similarity. Deterministic: ties break on kind and key.
    pub fn find(
        &mut self,
        snapshot_id: i64,
        text: &str,
        kind: Option<NodeKind>,
        limit: i64,
    ) -> anyhow::Result<Vec<FoundNode>> {
        let like = format!("%{}%", escape_like(text));
        let kind = kind.map(NodeKind::as_str);
        let rows = self.client.query(
            "SELECT kind, key, name FROM rb.node
             WHERE snapshot_id = $1 AND ($2::text IS NULL OR kind = $2)
               AND (key ILIKE $3 ESCAPE '\\' OR name ILIKE $3 ESCAPE '\\')
             ORDER BY (key = $4) DESC, (name = $4) DESC, similarity(name, $4) DESC,
                      length(key), kind COLLATE \"C\", key COLLATE \"C\"
             LIMIT $5",
            &[&snapshot_id, &kind, &like, &text, &limit],
        )?;
        Ok(rows.iter().map(found_from_row).collect())
    }

    /// Resolves a service by exact key (its directory) or exact name.
    pub fn resolve_service(&mut self, snapshot_id: i64, text: &str) -> anyhow::Result<Resolved> {
        let rows = self.client.query(
            "SELECT kind, key, name FROM rb.node
             WHERE snapshot_id = $1 AND kind = 'service' AND (key = $2 OR name = $2)
             ORDER BY (key = $2) DESC, key COLLATE \"C\"",
            &[&snapshot_id, &text],
        )?;
        let mut found: Vec<FoundNode> = rows.iter().map(found_from_row).collect();
        // An exact key match is unambiguous even if other services share the name.
        Ok(match found.len() {
            0 => Resolved::None,
            1 => Resolved::One(found.remove(0)),
            _ if found[0].key == text => Resolved::One(found.remove(0)),
            _ => Resolved::Ambiguous(found),
        })
    }

    /// Resolves a table by exact key (`schema.table`) or bare name (`table`). Input that matches
    /// nothing as typed is retried in lowercase, as Postgres folds unquoted names.
    pub fn resolve_table(&mut self, snapshot_id: i64, text: &str) -> anyhow::Result<Resolved> {
        let query = "SELECT kind, key, name FROM rb.node
                     WHERE snapshot_id = $1 AND kind = 'table' AND (key = $2 OR name = $2)
                     ORDER BY (key = $2) DESC, key COLLATE \"C\"";
        let mut rows = self.client.query(query, &[&snapshot_id, &text])?;
        let lower = text.to_lowercase();
        if rows.is_empty() && lower != text {
            rows = self.client.query(query, &[&snapshot_id, &lower])?;
        }
        let text = if rows.iter().any(|r| r.get::<_, String>(1) == text) {
            text
        } else {
            lower.as_str()
        };
        let mut found: Vec<FoundNode> = rows.iter().map(found_from_row).collect();
        Ok(match found.len() {
            0 => Resolved::None,
            1 => Resolved::One(found.remove(0)),
            _ if found[0].key == text => Resolved::One(found.remove(0)),
            _ => Resolved::Ambiguous(found),
        })
    }

    pub fn table_deps(&mut self, snapshot_id: i64, key: &str) -> anyhow::Result<TableDeps> {
        let Some(table) = self.client.query_opt(
            "SELECT id, name FROM rb.node WHERE snapshot_id = $1 AND kind = 'table' AND key = $2",
            &[&snapshot_id, &key],
        )?
        else {
            bail!("no table with key {key:?}");
        };
        let table_id: i64 = table.get(0);
        // Each access with the file it lives in (a symbol's DEFINED_IN file, or the file itself),
        // that file's service, and the first evidence line.
        let rows = self.client.query(
            "SELECT e.kind, s.kind, s.key, s.name, e.attrs, e.certainty, f.key,
                    (SELECT min(ev.span[1]) FROM rb.edge_evidence ev
                     WHERE ev.snapshot_id = e.snapshot_id AND ev.src = e.src AND ev.dst = e.dst
                       AND ev.kind = e.kind AND ev.provenance = e.provenance),
                    svc.key, svc.name
             FROM rb.edge e
             JOIN rb.node s ON s.snapshot_id = e.snapshot_id AND s.id = e.src
             LEFT JOIN rb.edge d ON d.snapshot_id = e.snapshot_id AND d.src = s.id AND d.kind = 'DEFINED_IN'
             JOIN rb.node f ON f.snapshot_id = e.snapshot_id
                           AND f.id = CASE WHEN s.kind = 'file' THEN s.id ELSE d.dst END
             JOIN rb.edge m ON m.snapshot_id = e.snapshot_id AND m.src = f.id AND m.kind = 'IN_SERVICE'
             JOIN rb.node svc ON svc.snapshot_id = e.snapshot_id AND svc.id = m.dst
             WHERE e.snapshot_id = $1 AND e.dst = $2 AND e.kind IN ('MIGRATES', 'QUERIES', 'MAPS_TO')",
            &[&snapshot_id, &table_id],
        )?;
        let mut migrations = Vec::new();
        let mut mappings = Vec::new();
        let mut accesses = Vec::new();
        for r in &rows {
            let attrs: Value = r.get(4);
            let access = TableAccess {
                source_kind: r.get(1),
                source_key: r.get(2),
                source_name: r.get(3),
                ops: attrs["ops"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|o| o.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                certainty: rb_model::Certainty::from_rank(r.get(5))?
                    .as_str()
                    .to_owned(),
                schema_assumed: attrs["schema_assumed"].as_bool().unwrap_or(false),
                file: r.get(6),
                line: r.get::<_, Option<i32>>(7).map_or(0, |l| l + 1),
                service_key: r.get(8),
                service_name: r.get(9),
            };
            match r.get::<_, String>(0).as_str() {
                "MIGRATES" => migrations.push(access),
                "MAPS_TO" => mappings.push(access),
                _ => accesses.push(access),
            }
        }
        // Bytewise order, independent of the database collation.
        let order = |a: &TableAccess, b: &TableAccess| {
            (&a.service_key, &a.file, a.line, &a.source_key).cmp(&(
                &b.service_key,
                &b.file,
                b.line,
                &b.source_key,
            ))
        };
        migrations.sort_by(order);
        mappings.sort_by(order);
        accesses.sort_by(order);
        Ok(TableDeps {
            key: key.to_owned(),
            name: table.get(1),
            migrations,
            mappings,
            accesses,
        })
    }

    pub fn sql_coverage(&mut self, snapshot_id: i64) -> anyhow::Result<SqlCoverage> {
        let r = self.client.query_one(
            "SELECT (SELECT count(*) FROM (SELECT DISTINCT file_id, span FROM rb.edge_evidence
                                            WHERE snapshot_id = $1 AND kind = 'QUERIES') q),
                    (SELECT count(*) FROM (SELECT DISTINCT file_id, span FROM rb.edge_evidence
                                            WHERE snapshot_id = $1 AND kind = 'MIGRATES') m),
                    (SELECT count(*) FROM rb.diagnostic WHERE snapshot_id = $1 AND kind = 'unparsed_sql'),
                    (SELECT count(*) FROM rb.node WHERE snapshot_id = $1 AND kind = 'table')",
            &[&snapshot_id],
        )?;
        Ok(SqlCoverage {
            query_sites: r.get(0),
            migration_statements: r.get(1),
            unparsed: r.get(2),
            tables: r.get(3),
        })
    }

    pub fn di_coverage(&mut self, snapshot_id: i64) -> anyhow::Result<DiCoverage> {
        let detail: Option<Value> = self
            .client
            .query_opt(
                "SELECT detail FROM rb.diagnostic WHERE snapshot_id = $1 AND kind = 'di_coverage'",
                &[&snapshot_id],
            )?
            .map(|r| r.get(0));
        let Some(detail) = detail else {
            return Ok(DiCoverage::default());
        };
        let counts = |field: &str| -> anyhow::Result<BTreeMap<String, i64>> {
            detail[field]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| {
                    let n = v
                        .as_i64()
                        .ok_or_else(|| anyhow::anyhow!("di_coverage {field}.{k}: {v}"))?;
                    Ok((k.clone(), n))
                })
                .collect()
        };
        Ok(DiCoverage {
            points: detail["points"].as_i64().unwrap_or_default(),
            buckets: counts("buckets")?,
            gates: counts("gates")?,
        })
    }

    pub fn rta_coverage(&mut self, snapshot_id: i64) -> anyhow::Result<RtaCoverage> {
        let detail: Option<Value> = self
            .client
            .query_opt(
                "SELECT detail FROM rb.diagnostic WHERE snapshot_id = $1 AND kind = 'rta_coverage'",
                &[&snapshot_id],
            )?
            .map(|r| r.get(0));
        let Some(detail) = detail else {
            return Ok(RtaCoverage::default());
        };
        let n = |v: &Value| -> anyhow::Result<i64> {
            v.as_i64()
                .ok_or_else(|| anyhow::anyhow!("rta_coverage: {v}"))
        };
        Ok(RtaCoverage {
            types: n(&detail["types"])?,
            proven: n(&detail["proven"])?,
            prunable_edges: n(&detail["prunable_edges"])?,
            kept: detail["kept"]
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| Ok((k.clone(), n(v)?)))
                .collect::<anyhow::Result<_>>()?,
        })
    }

    pub fn http_coverage(&mut self, snapshot_id: i64) -> anyhow::Result<HttpCoverage> {
        let r = self.client.query_one(
            "SELECT (SELECT count(*) FROM rb.node WHERE snapshot_id = $1 AND kind = 'endpoint'),
                    (SELECT count(DISTINCT src) FROM rb.edge
                     WHERE snapshot_id = $1 AND kind = 'EXPOSES'),
                    (SELECT detail FROM rb.diagnostic
                     WHERE snapshot_id = $1 AND kind = 'http_coverage')",
            &[&snapshot_id],
        )?;
        let detail: Option<Value> = r.get(2);
        let mut buckets = BTreeMap::new();
        let mut sites = 0;
        if let Some(Value::Object(counts)) = detail {
            for (bucket, n) in counts {
                // Gate A's counts of verified and unverified instance bases (contract M4a
                // amendment 13) are not a bucket.
                if bucket == "bases" {
                    continue;
                }
                let n = n
                    .as_i64()
                    .ok_or_else(|| anyhow::anyhow!("http_coverage {bucket}: {n}"))?;
                if bucket == "sites" {
                    sites = n;
                } else {
                    buckets.insert(bucket, n);
                }
            }
        }
        Ok(HttpCoverage {
            routes: r.get(0),
            services: r.get(1),
            sites,
            buckets,
        })
    }

    pub fn resolution_coverage(&mut self, snapshot_id: i64) -> anyhow::Result<ResolutionCoverage> {
        let r = self.client.query_one(
            "WITH sites AS (
                 -- A site is ambiguous when it has several targets. An edge's own certainty
                 -- is its best site's, so it cannot classify a site.
                 SELECT ev.file_id, ev.span, count(DISTINCT (ev.kind, ev.dst)) AS targets
                 FROM rb.edge_evidence ev
                 WHERE ev.snapshot_id = $1 AND ev.kind IN ('CALLS', 'REFERENCES')
                   AND ev.provenance = 'heuristic'
                 GROUP BY ev.file_id, ev.span)
             SELECT (SELECT count(*) FROM sites WHERE targets = 1),
                    (SELECT count(*) FROM sites WHERE targets > 1),
                    (SELECT coalesce(sum((detail->>'calls')::bigint + (detail->>'types')::bigint), 0)::bigint
                     FROM rb.diagnostic WHERE snapshot_id = $1 AND kind = 'unresolved_references'),
                    (SELECT coalesce(sum((detail->>'external')::bigint), 0)::bigint
                     FROM rb.diagnostic WHERE snapshot_id = $1 AND kind = 'unresolved_references'),
                    (SELECT count(*) FROM rb.edge
                     WHERE snapshot_id = $1 AND kind = 'CALLS' AND provenance = 'scip')",
            &[&snapshot_id],
        )?;
        Ok(ResolutionCoverage {
            resolved: r.get(0),
            ambiguous: r.get(1),
            unresolved: r.get(2),
            external: r.get(3),
            exact_calls: r.get(4),
        })
    }

    /// The snapshot's SCIP jobs that did not finish (`environment`, `timeout`, `skipped` or
    /// `unsupported`; contract M3b amendment 44), counted over its stored `scip_status`
    /// diagnostics: such a snapshot is heuristic for a reason the next run may not have, so
    /// `rb index` does not take it for "up to date" while SCIP is on.
    pub fn unfinished_scip_jobs(&mut self, snapshot_id: i64) -> anyhow::Result<i64> {
        let row = self.client.query_one(
            "SELECT count(*) FROM rb.diagnostic
             WHERE snapshot_id = $1 AND kind = 'scip_status'
               AND detail->>'status' IN ('environment', 'timeout', 'skipped', 'unsupported')",
            &[&snapshot_id],
        )?;
        Ok(row.get(0))
    }

    pub fn services(&mut self, snapshot_id: i64) -> anyhow::Result<Vec<ServiceSummary>> {
        let rows = self.client.query(
            "SELECT s.key, s.name, s.attrs,
                    (SELECT count(*) FROM rb.edge e
                     WHERE e.snapshot_id = s.snapshot_id AND e.dst = s.id AND e.kind = 'IN_SERVICE')
             FROM rb.node s
             WHERE s.snapshot_id = $1 AND s.kind = 'service'
             ORDER BY s.key COLLATE \"C\"",
            &[&snapshot_id],
        )?;
        rows.iter()
            .map(|r| {
                let attrs: Value = r.get(2);
                Ok(ServiceSummary {
                    key: r.get(0),
                    name: r.get(1),
                    role: role_of(&attrs)?,
                    files: usize::try_from(r.get::<_, i64>(3))?,
                })
            })
            .collect()
    }

    pub fn service_card(&mut self, snapshot_id: i64, key: &str) -> anyhow::Result<ServiceCard> {
        let Some(row) = self.client.query_opt(
            "SELECT id, name, attrs FROM rb.node WHERE snapshot_id = $1 AND kind = 'service' AND key = $2",
            &[&snapshot_id, &key],
        )?
        else {
            bail!("no service with key {key:?}");
        };
        let id: i64 = row.get(0);
        let attrs: Value = row.get(2);
        let strings = |v: &Value| -> Vec<String> {
            v.as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        };

        // Files of the service with their language and owners (NULL owner = unowned).
        let rows = self.client.query(
            "SELECT f.key, f.attrs->>'lang', o.key
             FROM rb.edge m
             JOIN rb.node f ON f.snapshot_id = m.snapshot_id AND f.id = m.src
             LEFT JOIN rb.edge ob ON ob.snapshot_id = f.snapshot_id AND ob.src = f.id AND ob.kind = 'OWNED_BY'
             LEFT JOIN rb.node o ON o.snapshot_id = ob.snapshot_id AND o.id = ob.dst
             WHERE m.snapshot_id = $1 AND m.dst = $2 AND m.kind = 'IN_SERVICE'",
            &[&snapshot_id, &id],
        )?;
        let mut files: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();
        for r in &rows {
            let entry = files.entry(r.get(0)).or_insert_with(|| {
                let lang: Option<String> = r.get(1);
                (lang.unwrap_or_else(|| "other".into()), Vec::new())
            });
            if let Some(owner) = r.get::<_, Option<String>>(2) {
                entry.1.push(owner);
            }
        }
        let mut languages: BTreeMap<&str, usize> = BTreeMap::new();
        let mut per_owner: BTreeMap<&str, usize> = BTreeMap::new();
        let mut unowned_files = 0;
        for (lang, owners) in files.values() {
            *languages.entry(lang).or_default() += 1;
            if owners.is_empty() {
                unowned_files += 1;
            }
            for o in owners {
                *per_owner.entry(o).or_default() += 1;
            }
        }
        let mut languages: Vec<(String, usize)> = languages
            .into_iter()
            .map(|(l, n)| (l.to_owned(), n))
            .collect();
        languages.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let mut file_owners: Vec<OwnerShare> = per_owner
            .into_iter()
            .map(|(owner, files)| OwnerShare {
                owner: owner.to_owned(),
                files,
            })
            .collect();
        file_owners.sort_by(|a, b| b.files.cmp(&a.files).then_with(|| a.owner.cmp(&b.owner)));

        // Targets of the service's edges of one kind, by key.
        let mut targets = |kind: &str| -> anyhow::Result<Vec<String>> {
            Ok(self
                .client
                .query(
                    "SELECT o.key FROM rb.edge e
                     JOIN rb.node o ON o.snapshot_id = e.snapshot_id AND o.id = e.dst
                     WHERE e.snapshot_id = $1 AND e.src = $2 AND e.kind = $3
                     ORDER BY o.key COLLATE \"C\"",
                    &[&snapshot_id, &id, &kind],
                )?
                .iter()
                .map(|r| r.get(0))
                .collect())
        };
        let owners = targets("OWNED_BY")?;
        // `Package` targets once each (a `scip` and a `manifest` edge may share one; contract
        // M4b S15); services a manifest names are dependencies, not packages.
        let packages: Vec<String> = self
            .client
            .query(
                "SELECT o.key FROM rb.edge e
                 JOIN rb.node o ON o.snapshot_id = e.snapshot_id AND o.id = e.dst
                 WHERE e.snapshot_id = $1 AND e.src = $2 AND e.kind = 'DEPENDS_ON'
                   AND o.kind = 'package'
                 GROUP BY o.key
                 ORDER BY o.key COLLATE \"C\"",
                &[&snapshot_id, &id],
            )?
            .iter()
            .map(|r| r.get(0))
            .collect();
        let apis = self.apis(snapshot_id, id)?;
        let (consumes, consumed_by) = self.http_peers(snapshot_id, id, key)?;
        let unstitched: i64 = self
            .client
            .query_one(
                "SELECT count(*) FROM rb.diagnostic dg
                 JOIN rb.edge m ON m.snapshot_id = dg.snapshot_id AND m.src = dg.file_id
                               AND m.kind = 'IN_SERVICE'
                 WHERE dg.snapshot_id = $1 AND m.dst = $2
                   AND dg.kind IN ('http_unmatched', 'http_self', 'http_external',
                                   'http_url_dynamic', 'http_too_ambiguous')",
                &[&snapshot_id, &id],
            )?
            .get(0);

        Ok(ServiceCard {
            key: key.to_owned(),
            name: row.get(1),
            role: role_of(&attrs)?,
            detected_by: strings(&attrs["detected_by"]),
            manifests: strings(&attrs["manifests"]),
            files: files.len(),
            languages,
            owners,
            file_owners,
            unowned_files,
            packages,
            apis,
            consumes,
            consumed_by,
            unstitched,
        })
    }

    /// The endpoints service `id` exposes, with their handlers, by key (bytewise).
    fn apis(&mut self, snapshot_id: i64, id: i64) -> anyhow::Result<Vec<Api>> {
        let rows = self.client.query(
            "SELECT ep.key, ep.name, ep.attrs, x.certainty, h.kind, h.key, hb.certainty
             FROM rb.edge x
             JOIN rb.node ep ON ep.snapshot_id = x.snapshot_id AND ep.id = x.dst
             LEFT JOIN rb.edge hb ON hb.snapshot_id = x.snapshot_id AND hb.src = ep.id
                                 AND hb.kind = 'HANDLED_BY'
             LEFT JOIN rb.node h ON h.snapshot_id = hb.snapshot_id AND h.id = hb.dst
             WHERE x.snapshot_id = $1 AND x.src = $2 AND x.kind = 'EXPOSES'",
            &[&snapshot_id, &id],
        )?;
        let mut apis: BTreeMap<String, Api> = BTreeMap::new();
        for r in &rows {
            let key: String = r.get(0);
            let attrs: Value = r.get(2);
            let api = apis.entry(key.clone()).or_insert(Api {
                key,
                name: r.get(1),
                certainty: certainty_str(r.get(3))?,
                prefix_unknown: attrs["prefix_unknown"].as_bool().unwrap_or(false),
                handlers: Vec::new(),
            });
            if let Some(kind) = r.get::<_, Option<String>>(4) {
                api.handlers.push(Handler {
                    kind,
                    key: r.get(5),
                    certainty: certainty_str(r.get(6))?,
                });
            }
        }
        let mut apis: Vec<Api> = apis.into_values().collect();
        for a in &mut apis {
            a.handlers.sort_by(|x, y| x.key.cmp(&y.key));
        }
        Ok(apis)
    }

    /// The service's outbound and inbound `CONSUMES`, grouped by the other service, then by
    /// endpoint, each with the best certainty and the summed sites.
    fn http_peers(
        &mut self,
        snapshot_id: i64,
        id: i64,
        key: &str,
    ) -> anyhow::Result<(Vec<HttpPeer>, Vec<HttpPeer>)> {
        // Each edge with the service of its source's file and the service exposing its target.
        let rows = self.client.query(
            "SELECT ss.key, ss.name, ts.key, ts.name, ep.key, ep.name, c.certainty, c.attrs
             FROM rb.edge c
             JOIN rb.node s ON s.snapshot_id = c.snapshot_id AND s.id = c.src
             LEFT JOIN rb.edge d ON d.snapshot_id = c.snapshot_id AND d.src = s.id
                                AND d.kind = 'DEFINED_IN'
             JOIN rb.edge m ON m.snapshot_id = c.snapshot_id AND m.kind = 'IN_SERVICE'
                           AND m.src = CASE WHEN s.kind = 'file' THEN s.id ELSE d.dst END
             JOIN rb.node ss ON ss.snapshot_id = c.snapshot_id AND ss.id = m.dst
             JOIN rb.node ep ON ep.snapshot_id = c.snapshot_id AND ep.id = c.dst
             JOIN rb.node ts ON ts.snapshot_id = c.snapshot_id AND ts.kind = 'service'
                            AND ts.key = ep.attrs->>'service'
             WHERE c.snapshot_id = $1 AND c.kind = 'CONSUMES' AND (ss.id = $2 OR ts.key = $3)",
            &[&snapshot_id, &id, &key],
        )?;
        let (mut out, mut inbound) = (Peers::new(), Peers::new());
        for r in &rows {
            let (source, target): (String, String) = (r.get(0), r.get(2));
            let attrs: Value = r.get(7);
            let edge = (
                r.get::<_, String>(4),
                r.get::<_, String>(5),
                rb_model::Certainty::from_rank(r.get(6))?,
                attrs["sites"].as_i64().unwrap_or(1),
            );
            if source == key {
                record(&mut out, target.clone(), r.get(3), edge.clone());
            }
            if target == key {
                record(&mut inbound, source, r.get(1), edge);
            }
        }
        let flatten = |peers: Peers| -> Vec<HttpPeer> {
            peers
                .into_iter()
                .map(|(service_key, (service_name, uses))| HttpPeer {
                    service_key,
                    service_name,
                    endpoints: uses
                        .into_iter()
                        .map(|(key, (name, certainty, sites))| HttpUse {
                            key,
                            name,
                            certainty: certainty.as_str().to_owned(),
                            sites,
                        })
                        .collect(),
                })
                .collect()
        };
        Ok((flatten(out), flatten(inbound)))
    }
}

fn certainty_str(rank: i16) -> anyhow::Result<String> {
    Ok(rb_model::Certainty::from_rank(rank)?.as_str().to_owned())
}

/// Service key → (its name, endpoint key → (endpoint name, best certainty, sites)).
type Peers = BTreeMap<String, (String, BTreeMap<String, (String, rb_model::Certainty, i64)>)>;

/// One `CONSUMES` edge `(endpoint key, name, certainty, sites)` with the service on its other
/// side.
fn record(
    peers: &mut Peers,
    peer: String,
    peer_name: String,
    (key, name, certainty, sites): (String, String, rb_model::Certainty, i64),
) {
    let (_, uses) = peers.entry(peer).or_insert((peer_name, BTreeMap::new()));
    let u = uses.entry(key).or_insert((name, certainty, 0));
    u.1 = u.1.max(certainty);
    u.2 += sites;
}

/// Every `Service` node carries a `role`; a missing or unknown one is corrupt data, not a default.
fn role_of(attrs: &Value) -> anyhow::Result<ServiceRole> {
    attrs["role"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("service node without a role: {attrs}"))?
        .parse()
}

fn role_str<S: serde::Serializer>(role: &ServiceRole, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(role.as_str())
}

fn found_from_row(r: &postgres::Row) -> FoundNode {
    FoundNode {
        kind: r.get(0),
        key: r.get(1),
        name: r.get(2),
    }
}

/// Escapes `%`, `_` and `\` for use inside an ILIKE pattern with `ESCAPE '\'`.
fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_escaping() {
        assert_eq!(escape_like("a_b%c\\d"), "a\\_b\\%c\\\\d");
        assert_eq!(escape_like("plain"), "plain");
    }
}
