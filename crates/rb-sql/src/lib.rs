//! SQL analysis (design §7.2): which tables each statement touches, and how.
//!
//! Pure functions over SQL text, backed by the real PostgreSQL parser (`pg_query`, PG17). Anything
//! the parser rejects is an error for the caller to report; nothing is guessed.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// Schema assumed for unqualified table names (Postgres' default `search_path`).
pub const DEFAULT_SCHEMA: &str = "public";

/// Schemas whose tables are the database's own, never application tables.
const SYSTEM_SCHEMAS: &[&str] = &["pg_catalog", "information_schema"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Op {
    Read,
    Insert,
    Update,
    Delete,
    Merge,
    Create,
    Alter,
    Drop,
    Truncate,
}

impl Op {
    pub fn as_str(self) -> &'static str {
        match self {
            Op::Read => "read",
            Op::Insert => "insert",
            Op::Update => "update",
            Op::Delete => "delete",
            Op::Merge => "merge",
            Op::Create => "create",
            Op::Alter => "alter",
            Op::Drop => "drop",
            Op::Truncate => "truncate",
        }
    }
}

/// One table as used by one statement.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableUse {
    pub schema: String,
    pub name: String,
    pub ops: BTreeSet<Op>,
    /// Every reference was unqualified, so the schema is [`DEFAULT_SCHEMA`] by assumption.
    pub schema_assumed: bool,
}

impl TableUse {
    /// Natural key of the `Table` node (design §8.2).
    pub fn key(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// Byte range of the statement within the analysed text.
    pub range: (usize, usize),
    /// Sorted by (schema, name).
    pub tables: Vec<TableUse>,
}

/// Splits `sql` into statements and reports the tables each one touches.
pub fn analyze(sql: &str) -> Result<Vec<Statement>, String> {
    let parsed = pg_query::parse(sql).map_err(|e| e.to_string())?;
    parsed
        .protobuf
        .stmts
        .iter()
        .map(|raw| {
            let start = usize::try_from(raw.stmt_location).map_err(|e| e.to_string())?;
            // Postgres reports length 0 for "until the end of the input".
            let end = match usize::try_from(raw.stmt_len).map_err(|e| e.to_string())? {
                0 => sql.len(),
                len => start + len,
            };
            let text = &sql[start..end];
            let range = (
                start + (text.len() - text.trim_start().len()),
                start + text.trim_end().len(),
            );
            Ok(Statement {
                range,
                tables: tables_of(text)?,
            })
        })
        .collect()
}

/// Tables of a single statement, with the operation each one undergoes.
fn tables_of(statement: &str) -> Result<Vec<TableUse>, String> {
    let parsed = pg_query::parse(statement).map_err(|e| e.to_string())?;
    let kind = parsed
        .statement_types()
        .first()
        .copied()
        .unwrap_or_default();
    let mut uses: BTreeMap<(String, String), (BTreeSet<Op>, bool)> = BTreeMap::new();
    let groups = [
        (parsed.select_tables(), Op::Read),
        (parsed.dml_tables(), dml_op(kind)),
        (parsed.ddl_tables(), ddl_op(kind)),
    ];
    for (tables, op) in groups {
        for qualified in tables {
            let Some((schema, name, assumed)) = split_name(&qualified) else {
                continue;
            };
            let entry = uses
                .entry((schema, name))
                .or_insert_with(|| (BTreeSet::new(), assumed));
            entry.0.insert(op);
            entry.1 &= assumed; // one qualified reference makes the schema known
        }
    }
    Ok(uses
        .into_iter()
        .map(|((schema, name), (ops, schema_assumed))| TableUse {
            schema,
            name,
            ops,
            schema_assumed,
        })
        .collect())
}

/// The write a DML statement performs on its target. Statement kinds the parser reports as DML
/// but that are not classified here (e.g. `COPY`) are recorded as `update`, a generic write:
/// dropping the dependency or calling it a read would understate the impact.
fn dml_op(kind: &str) -> Op {
    match kind {
        "InsertStmt" => Op::Insert,
        "DeleteStmt" => Op::Delete,
        "MergeStmt" => Op::Merge,
        _ => Op::Update,
    }
}

fn ddl_op(kind: &str) -> Op {
    match kind {
        "CreateStmt" | "CreateTableAsStmt" | "ViewStmt" => Op::Create,
        "DropStmt" => Op::Drop,
        "TruncateStmt" => Op::Truncate,
        _ => Op::Alter,
    }
}

/// `[catalog.][schema.]name` → (schema, name, schema_assumed); `None` for system schemas.
fn split_name(qualified: &str) -> Option<(String, String, bool)> {
    let parts: Vec<&str> = qualified.split('.').collect();
    let (schema, name, assumed) = match parts.as_slice() {
        [name] => (DEFAULT_SCHEMA, *name, true),
        [.., schema, name] => (*schema, *name, false),
        [] => return None,
    };
    if SYSTEM_SCHEMAS.contains(&schema) {
        return None;
    }
    Some((schema.to_owned(), name.to_owned(), assumed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `schema.name:op+op` per table, per statement; `*` marks an assumed schema.
    fn summary(sql: &str) -> Vec<Vec<String>> {
        analyze(sql)
            .unwrap()
            .iter()
            .map(|s| {
                s.tables
                    .iter()
                    .map(|t| {
                        let ops: Vec<&str> = t.ops.iter().map(|o| o.as_str()).collect();
                        let star = if t.schema_assumed { "*" } else { "" };
                        format!("{}{star}:{}", t.key(), ops.join("+"))
                    })
                    .collect()
            })
            .collect()
    }

    #[test]
    fn dml_operations_per_statement() {
        assert_eq!(
            summary(
                "SELECT * FROM users u JOIN orders o ON o.uid = u.id; DELETE FROM sessions WHERE id = $1"
            ),
            [
                vec!["public.orders*:read", "public.users*:read"],
                vec!["public.sessions*:delete"]
            ]
        );
        assert_eq!(
            summary("UPDATE users SET n = $1 FROM teams t WHERE t.id = users.team"),
            [vec!["public.teams*:read", "public.users*:update"]]
        );
        assert_eq!(
            summary(
                "MERGE INTO stock s USING deliveries d ON s.id = d.id WHEN MATCHED THEN UPDATE SET n = d.n"
            ),
            [vec!["public.deliveries*:read", "public.stock*:merge"]]
        );
    }

    #[test]
    fn insert_select_and_ctes() {
        assert_eq!(
            summary(
                "WITH recent AS (SELECT * FROM orders) INSERT INTO audit.log (id) SELECT id FROM recent"
            ),
            [vec!["audit.log:insert", "public.orders*:read"]]
        );
    }

    #[test]
    fn ddl_operations() {
        let sql = "CREATE TABLE IF NOT EXISTS users (id bigserial primary key);
                   ALTER TABLE users ADD COLUMN email text;
                   CREATE INDEX users_email ON users (email);
                   CREATE VIEW active AS SELECT * FROM users;
                   DROP TABLE old_users;
                   TRUNCATE big;";
        assert_eq!(
            summary(sql),
            [
                vec!["public.users*:create"],
                vec!["public.users*:alter"],
                vec!["public.users*:alter"],
                vec!["public.active*:create", "public.users*:read"],
                vec!["public.old_users*:drop"],
                vec!["public.big*:truncate"],
            ]
        );
    }

    #[test]
    fn schemas_case_and_system_catalogs() {
        assert_eq!(
            summary(r#"SELECT * FROM Auth.Users, "Mixed"."Case", users, db.sales.orders"#),
            [vec![
                "Mixed.Case:read",
                "auth.users:read",
                "public.users*:read",
                "sales.orders:read"
            ]]
        );
        assert_eq!(
            summary("SELECT * FROM pg_catalog.pg_class, information_schema.tables, t"),
            [vec!["public.t*:read"]]
        );
        // A qualified reference anywhere in the statement makes the schema known.
        assert_eq!(
            summary("SELECT * FROM public.users JOIN users u2 ON true"),
            [vec!["public.users:read"]]
        );
    }

    #[test]
    fn statement_ranges_cover_the_text() {
        let sql = "SELECT 1 FROM a;\n  SELECT 2 FROM b";
        let ranges: Vec<&str> = analyze(sql)
            .unwrap()
            .iter()
            .map(|s| &sql[s.range.0..s.range.1])
            .collect();
        assert_eq!(ranges, ["SELECT 1 FROM a", "SELECT 2 FROM b"]);
    }

    #[test]
    fn parse_errors_are_reported_not_guessed() {
        let err = analyze("SELECT * FROM $1").unwrap_err();
        assert!(err.contains("syntax error"), "{err}");
        assert!(analyze("SELEC oops").is_err());
        assert_eq!(analyze("").unwrap(), []);
        assert_eq!(analyze("-- only a comment\n").unwrap(), []);
    }

    #[test]
    fn output_is_sorted_and_deterministic() {
        let sql = "SELECT * FROM z, y, x, w, v";
        let a = summary(sql);
        assert_eq!(a, summary(sql));
        assert_eq!(
            a[0],
            [
                "public.v*:read",
                "public.w*:read",
                "public.x*:read",
                "public.y*:read",
                "public.z*:read"
            ]
        );
    }
}
