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

/// Structure sections of the Service Card (design §9.5). APIs, tables, dependencies and the call
/// graph arrive with later milestones.
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

        let owners = self
            .client
            .query(
                "SELECT o.key FROM rb.edge e
                 JOIN rb.node o ON o.snapshot_id = e.snapshot_id AND o.id = e.dst
                 WHERE e.snapshot_id = $1 AND e.src = $2 AND e.kind = 'OWNED_BY'
                 ORDER BY o.key COLLATE \"C\"",
                &[&snapshot_id, &id],
            )?
            .iter()
            .map(|r| r.get(0))
            .collect();

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
        })
    }
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
