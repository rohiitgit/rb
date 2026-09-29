//! SQL analysis (design §7.2): which tables each statement touches, and how.
//!
//! Pure functions over SQL text, built on PostgreSQL's own lexer and parser (`pg_query`, PG17):
//!
//! 1. Statements are split on Postgres' own lexer tokens (see [`split`]): `;` outside
//!    `BEGIN ATOMIC` bodies and `psql` meta-commands end a statement, so one bad statement never
//!    hides the rest. Every range refers to the original text.
//! 2. Each statement is parsed on its own. JDBC `?`, named `:param` and DB-API `%s`/`%(name)s`
//!    placeholders are not PostgreSQL syntax; a statement that fails is retried once with them
//!    rewritten to `$n`, found as tokens (so comments, strings and `?|` operators are safe).
//! 3. Tables come from walking the *whole* parse tree (serialised to JSON, so no node type is
//!    skipped). Each statement node contributes its own target and operation, including DML nested
//!    in CTEs or rule and function bodies; other relations are reads inside query-bearing
//!    statements only. CTE names are scoped: a CTE hides same-named tables only where it is
//!    visible (later siblings and the main query; itself too when `RECURSIVE`).
//!
//! Nothing is guessed: whatever the parser rejects is returned as an error with its range.

use std::collections::{BTreeMap, BTreeSet};

use pg_query::protobuf::{ObjectType, Token};
use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    /// Natural key of the `Table` node (design §8.2): `schema.table`, with a part double-quoted
    /// (SQL style) when it contains `.` or `"`, so `"a.b".c` and `a."b.c"` never collide.
    pub fn key(&self) -> String {
        format!("{}.{}", quote_part(&self.schema), quote_part(&self.name))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// Byte range of the statement within the analysed text.
    pub range: (usize, usize),
    /// Sorted by (schema, name).
    pub tables: Vec<TableUse>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementError {
    /// Byte range of the statement within the analysed text.
    pub range: (usize, usize),
    pub error: String,
}

/// The statements of a text: those that parsed, and those that did not.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Analysis {
    pub statements: Vec<Statement>,
    pub errors: Vec<StatementError>,
}

/// Splits `sql` into statements and reports the tables each one touches.
pub fn analyze(sql: &str) -> Analysis {
    let mut analysis = Analysis::default();
    for range in split(sql) {
        match tables_of(&sql[range.0..range.1]) {
            Ok(tables) => analysis.statements.push(Statement { range, tables }),
            Err(error) => analysis.errors.push(StatementError { range, error }),
        }
    }
    analysis
}

fn quote_part(part: &str) -> String {
    if part.contains(['.', '"']) {
        format!("\"{}\"", part.replace('"', "\"\""))
    } else {
        part.to_owned()
    }
}

fn token_kind(token: &pg_query::protobuf::ScanToken) -> Option<Token> {
    Token::try_from(token.token).ok()
}

/// Statement ranges from Postgres' own lexer, so quotes, dollar quotes, `E''` strings and
/// comments are single tokens and can never be mistaken for boundaries:
/// - a `;` ends a statement, except inside a `BEGIN ATOMIC … END` function body;
/// - a `psql` meta-command (a `\\` token, to the end of its line) ends one too, as `\\gset` does;
///   `\\copy …` is kept as a `COPY …` statement;
/// - a range runs from a statement's first token to its last, so it is never empty or inverted;
/// - text the lexer cannot tokenise (e.g. an unterminated quote) is one statement, so its error
///   is reported rather than lost.
fn split(sql: &str) -> Vec<(usize, usize)> {
    let Ok(scan) = pg_query::scan(sql) else {
        let (start, end) = (sql.len() - sql.trim_start().len(), sql.trim_end().len());
        return if start < end {
            vec![(start, end)]
        } else {
            Vec::new()
        };
    };
    let tokens: Vec<(Option<Token>, usize, usize)> = scan
        .tokens
        .iter()
        .filter(|t| !matches!(token_kind(t), Some(Token::SqlComment | Token::CComment)))
        .filter_map(|t| {
            Some((
                token_kind(t),
                usize::try_from(t.start).ok()?,
                usize::try_from(t.end).ok()?,
            ))
        })
        .collect();
    let mut out = Vec::new();
    let mut current: Option<(usize, usize)> = None;
    let mut atomic_depth = 0usize;
    let mut i = 0;
    while i < tokens.len() {
        let (kind, start, end) = tokens[i];
        match kind {
            Some(Token::Ascii92) => {
                out.extend(current.take());
                let line_end = sql[start..].find('\n').map_or(sql.len(), |n| start + n);
                let on_line: Vec<_> = tokens[i + 1..]
                    .iter()
                    .take_while(|t| t.1 < line_end)
                    .collect();
                let is_copy = on_line
                    .first()
                    .is_some_and(|t| t.1 == end && sql[t.1..t.2].eq_ignore_ascii_case("copy"));
                if let (true, Some(first), Some(last)) = (is_copy, on_line.first(), on_line.last())
                {
                    out.push((first.1, last.2));
                }
                i += 1 + on_line.len();
                continue;
            }
            Some(Token::Ascii59) if atomic_depth == 0 => out.extend(current.take()),
            _ => {
                let after_begin = i > 0 && tokens[i - 1].0 == Some(Token::BeginP);
                match kind {
                    Some(Token::Atomic) if after_begin => atomic_depth += 1,
                    Some(Token::Case) if atomic_depth > 0 => atomic_depth += 1,
                    Some(Token::EndP) if atomic_depth > 0 => atomic_depth -= 1,
                    _ => {}
                }
                current = Some((current.map_or(start, |c| c.0), end));
            }
        }
        i += 1;
    }
    out.extend(current);
    out
}

/// Tables of one statement; retried once with placeholders rewritten if it does not parse.
fn tables_of(statement: &str) -> Result<Vec<TableUse>, String> {
    let parsed = match pg_query::parse(statement) {
        Ok(parsed) => parsed,
        Err(first) => {
            let rewritten = positional(statement);
            if rewritten == statement {
                return Err(first.to_string());
            }
            pg_query::parse(&rewritten).map_err(|_| first.to_string())?
        }
    };
    let tree = serde_json::to_value(&parsed.protobuf).map_err(|e| e.to_string())?;
    let mut walker = Walker::default();
    walker.walk(&tree, &[], false);
    Ok(walker.uses.into_tables())
}

/// Comparison operators the lexer may glue to a following `?` or `%` (`id=?`, `a>=%s`).
const COMPARISONS: &[&str] = &["=", "<>", "!=", "<", ">", "<=", ">="];

/// Rewrites placeholder tokens to `$1`, `$2`, …: a `?` operator, `:name`, `%s` and `%(name)s`
/// (each part adjacent), also when glued to a comparison (`id=?` lexes as one `=?` token). A `?`
/// glued to a preceding word (`cache?`) is prose; real operators such as `?|` or `@?` stay.
fn positional(sql: &str) -> String {
    let Ok(scan) = pg_query::scan(sql) else {
        return sql.to_owned();
    };
    let tokens: Vec<(Option<Token>, usize, usize, &str)> = scan
        .tokens
        .iter()
        .filter_map(|t| {
            let (start, end) = (usize::try_from(t.start).ok()?, usize::try_from(t.end).ok()?);
            Some((token_kind(t), start, end, sql.get(start..end)?))
        })
        .collect();
    let is_word = |i: usize| {
        tokens.get(i).is_some_and(|t| {
            !t.3.is_empty() && t.3.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
    };
    let adjacent = |i: usize| i > 0 && tokens.get(i).is_some_and(|t| t.1 == tokens[i - 1].2);
    // Tokens after a `%` that complete `%s` or `%(name)s`.
    let percent_tail = |i: usize| -> usize {
        if adjacent(i + 1) && tokens.get(i + 1).is_some_and(|t| t.3 == "s") {
            1
        } else if tokens.get(i + 1).is_some_and(|t| t.3 == "(")
            && (1..=4).all(|k| adjacent(i + k))
            && is_word(i + 2)
            && tokens.get(i + 3).is_some_and(|t| t.3 == ")")
            && tokens.get(i + 4).is_some_and(|t| t.3 == "s")
        {
            4
        } else {
            0
        }
    };
    let mut out = String::with_capacity(sql.len() + 8);
    let mut copied = 0;
    let mut n = 0;
    let mut i = 0;
    while i < tokens.len() {
        let (kind, start, _, text) = tokens[i];
        let glued_to_word = adjacent(i) && is_word(i - 1);
        let (prefix, marker) = match (kind, text) {
            (Some(Token::Op), "?") | (Some(Token::Ascii37), "%") | (Some(Token::Ascii58), ":") => {
                ("", text.chars().next())
            }
            (Some(Token::Op), t) if t.len() > 1 && COMPARISONS.contains(&&t[..t.len() - 1]) => {
                (&t[..t.len() - 1], t.chars().last())
            }
            _ => ("", None),
        };
        // Number of tokens after `i` that belong to the placeholder, if it is one.
        let tail = match marker {
            Some('?') if !prefix.is_empty() || !glued_to_word => Some(0),
            Some(':') if !glued_to_word && adjacent(i + 1) && is_word(i + 1) => Some(1),
            Some('%') => Some(percent_tail(i)).filter(|&t| t > 0),
            _ => None,
        };
        let Some(tail) = tail else {
            i += 1;
            continue;
        };
        n += 1;
        out.push_str(&sql[copied..start]);
        out.push_str(prefix);
        out.push_str(&format!("${n}"));
        copied = tokens[i + tail].2;
        i += tail + 1;
    }
    out.push_str(&sql[copied..]);
    out
}

/// Accumulates `(schema, name) → (ops, schema_assumed)`.
#[derive(Default)]
struct Uses(BTreeMap<(String, String), (BTreeSet<Op>, bool)>);

impl Uses {
    fn add(&mut self, schema: &str, name: &str, op: Op) {
        if name.is_empty()
            || SYSTEM_SCHEMAS.contains(&schema)
            || (schema.is_empty() && name.starts_with("pg_"))
        {
            // Unqualified `pg_*` relations resolve to pg_catalog, which is always searched first.
            return;
        }
        let assumed = schema.is_empty();
        let schema = if assumed { DEFAULT_SCHEMA } else { schema };
        let entry = self
            .0
            .entry((schema.to_owned(), name.to_owned()))
            .or_insert_with(|| (BTreeSet::new(), assumed));
        entry.0.insert(op);
        entry.1 &= assumed; // one qualified reference makes the schema known
    }

    fn into_tables(self) -> Vec<TableUse> {
        self.0
            .into_iter()
            .map(|((schema, name), (ops, schema_assumed))| TableUse {
                schema,
                name,
                ops,
                schema_assumed,
            })
            .collect()
    }
}

fn is_table_like(object: Option<i64>) -> bool {
    [
        ObjectType::ObjectTable,
        ObjectType::ObjectView,
        ObjectType::ObjectMatview,
        ObjectType::ObjectForeignTable,
    ]
    .iter()
    .any(|t| Some(*t as i64) == object)
}

fn str_at<'v>(v: &'v Value, key: &str) -> &'v str {
    v.get(key).and_then(Value::as_str).unwrap_or_default()
}

/// Walks the JSON form of a parse tree (prost structs serialised by serde: node wrappers are
/// `{"node": {"Kind": {…}}}`, typed fields such as `relation` hold the struct directly).
#[derive(Default)]
struct Walker {
    uses: Uses,
}

impl Walker {
    fn walk(&mut self, v: &Value, scope: &[String], reads: bool) {
        match v {
            Value::Array(items) => items.iter().for_each(|x| self.walk(x, scope, reads)),
            Value::Object(fields) => fields
                .iter()
                .for_each(|(k, x)| self.visit(k, x, scope, reads)),
            _ => {}
        }
    }

    /// Walks every field except `skip`.
    fn walk_fields(&mut self, v: &Value, skip: &[&str], scope: &[String], reads: bool) {
        if let Value::Object(fields) = v {
            for (k, x) in fields.iter().filter(|(k, _)| !skip.contains(&k.as_str())) {
                self.visit(k, x, scope, reads);
            }
        }
    }

    fn target(&mut self, rv: Option<&Value>, op: Op) {
        if let Some(rv) = rv.filter(|r| r.is_object()) {
            self.uses
                .add(str_at(rv, "schemaname"), str_at(rv, "relname"), op);
        }
    }

    fn visit(&mut self, key: &str, v: &Value, scope: &[String], reads: bool) {
        match key {
            "RangeVar" => {
                let (schema, name) = (str_at(v, "schemaname"), str_at(v, "relname"));
                let names_a_cte = schema.is_empty() && scope.iter().any(|c| c == name);
                if reads && !names_a_cte {
                    self.uses.add(schema, name, Op::Read);
                }
            }
            "SelectStmt" | "InsertStmt" | "UpdateStmt" | "DeleteStmt" | "MergeStmt" => {
                let scope = self.with_scope(v, scope);
                let op = match key {
                    "InsertStmt" => Some(Op::Insert),
                    "UpdateStmt" => Some(Op::Update),
                    "DeleteStmt" => Some(Op::Delete),
                    "MergeStmt" => Some(Op::Merge),
                    _ => None,
                };
                if let Some(op) = op {
                    self.target(v.get("relation"), op);
                }
                self.target(v.pointer("/into_clause/rel"), Op::Create);
                // UNION arms are typed SelectStmt fields (no node wrapper): visit them as such so
                // their own WITH clauses are scoped.
                for arm in ["larg", "rarg"] {
                    if let Some(select) = v.get(arm).filter(|x| x.is_object()) {
                        self.visit("SelectStmt", select, &scope, true);
                    }
                }
                // `FOR UPDATE OF u` names an alias from FROM, not a table.
                let skip = [
                    "with_clause",
                    "relation",
                    "into_clause",
                    "larg",
                    "rarg",
                    "locking_clause",
                ];
                self.walk_fields(v, &skip, &scope, true);
            }
            "CopyStmt" => {
                let from = v.get("is_from").and_then(Value::as_bool).unwrap_or(false);
                self.target(v.get("relation"), if from { Op::Insert } else { Op::Read });
                self.walk_fields(v, &["relation"], scope, true);
            }
            // The created table, and the parents it partitions or inherits from (which change);
            // constraints (`REFERENCES t`) are not reads.
            "CreateStmt" => {
                self.target(v.get("relation"), Op::Create);
                for parent in v
                    .get("inh_relations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    self.target(parent.pointer("/node/RangeVar"), Op::Alter);
                }
            }
            "CreateForeignTableStmt" => {
                if let Some(base) = v.get("base_stmt").filter(|b| b.is_object()) {
                    self.visit("CreateStmt", base, scope, reads);
                }
            }
            "AlterObjectSchemaStmt" => {
                if is_table_like(v.get("object_type").and_then(Value::as_i64)) {
                    self.target(v.get("relation"), Op::Alter);
                }
            }
            "CreateTableAsStmt" => {
                self.target(v.pointer("/into/rel"), Op::Create);
                self.walk_fields(v, &["into"], scope, true);
            }
            "ViewStmt" => {
                self.target(v.get("view"), Op::Create);
                self.walk_fields(v, &["view"], scope, true);
            }
            "IndexStmt" | "CreateTrigStmt" | "RefreshMatViewStmt" => {
                self.target(v.get("relation"), Op::Alter);
            }
            "RuleStmt" => {
                self.target(v.get("relation"), Op::Alter);
                self.walk_fields(v, &["relation", "where_clause"], scope, reads);
            }
            "AlterTableStmt" => {
                // `ALTER TYPE t …` is an AlterTableStmt too, on a composite type: not a table.
                if is_table_like(v.get("objtype").and_then(Value::as_i64)) {
                    self.target(v.get("relation"), Op::Alter);
                }
            }
            "RenameStmt" => {
                let rename = v.get("rename_type").and_then(Value::as_i64);
                let on_table_part = [ObjectType::ObjectColumn, ObjectType::ObjectTabconstraint]
                    .iter()
                    .any(|t| Some(*t as i64) == rename)
                    && v.get("relation_type").and_then(Value::as_i64)
                        != Some(ObjectType::ObjectType as i64);
                if is_table_like(rename) || on_table_part {
                    self.target(v.get("relation"), Op::Alter);
                }
            }
            "DropStmt" => {
                if is_table_like(v.get("remove_type").and_then(Value::as_i64)) {
                    for object in v
                        .get("objects")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                    {
                        let parts: Vec<&str> = object
                            .pointer("/node/List/items")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|n| n.pointer("/node/String/sval").and_then(Value::as_str))
                            .collect();
                        match parts.as_slice() {
                            [name] => self.uses.add("", name, Op::Drop),
                            [.., schema, name] => self.uses.add(schema, name, Op::Drop),
                            [] => {}
                        }
                    }
                }
            }
            "TruncateStmt" => {
                for rel in v
                    .get("relations")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    self.target(rel.pointer("/node/RangeVar"), Op::Truncate);
                }
            }
            // Privileges, locks and comments name tables without reading or writing them.
            "GrantStmt" | "LockStmt" | "CommentStmt" | "SecLabelStmt" | "AlterOwnerStmt" => {}
            _ => self.walk(v, scope, reads),
        }
    }

    /// Walks a statement's CTE bodies with the names visible to each (earlier siblings; all of
    /// them when `RECURSIVE`) and returns the scope for the rest of the statement.
    fn with_scope(&mut self, stmt: &Value, scope: &[String]) -> Vec<String> {
        let mut outer = scope.to_vec();
        let Some(with) = stmt.get("with_clause").filter(|w| w.is_object()) else {
            return outer;
        };
        let recursive = with
            .get("recursive")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let ctes: Vec<(&str, Option<&Value>)> = with
            .get("ctes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| c.pointer("/node/CommonTableExpr"))
            .map(|c| (str_at(c, "ctename"), c.get("ctequery")))
            .collect();
        for (k, (_, query)) in ctes.iter().enumerate() {
            let visible = if recursive { ctes.len() } else { k };
            let mut inner = outer.clone();
            inner.extend(ctes[..visible].iter().map(|(n, _)| (*n).to_owned()));
            if let Some(query) = query {
                self.walk(query, &inner, true);
            }
        }
        outer.extend(ctes.iter().map(|(n, _)| (*n).to_owned()));
        outer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `schema.name:op+op` per table, per statement; `*` marks an assumed schema. Panics on errors.
    fn summary(sql: &str) -> Vec<Vec<String>> {
        let analysis = analyze(sql);
        assert!(analysis.errors.is_empty(), "{:?}", analysis.errors);
        summary_of(&analysis)
    }

    fn summary_of(analysis: &Analysis) -> Vec<Vec<String>> {
        analysis
            .statements
            .iter()
            .map(|s| {
                s.tables
                    .iter()
                    .map(|t| {
                        format!(
                            "{}{}:{}",
                            t.key(),
                            if t.schema_assumed { "*" } else { "" },
                            t.ops
                                .iter()
                                .map(|o| o.as_str())
                                .collect::<Vec<_>>()
                                .join("+")
                        )
                    })
                    .collect()
            })
            .collect()
    }

    fn texts(sql: &str) -> Vec<&str> {
        analyze(sql)
            .statements
            .iter()
            .map(|s| &sql[s.range.0..s.range.1])
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
        assert_eq!(
            summary("INSERT INTO t SELECT * FROM u ON CONFLICT (id) DO NOTHING RETURNING id"),
            [vec!["public.t*:insert", "public.u*:read"]]
        );
    }

    #[test]
    fn ctes_nested_dml_and_shadowing() {
        assert_eq!(
            summary(
                "WITH recent AS (SELECT * FROM orders) INSERT INTO audit.log (id) SELECT id FROM recent"
            ),
            [vec!["audit.log:insert", "public.orders*:read"]]
        );
        // Data-modifying CTEs keep their own operation.
        assert_eq!(
            summary(
                "WITH d AS (DELETE FROM sessions RETURNING *) INSERT INTO archive SELECT * FROM d"
            ),
            [vec!["public.archive*:insert", "public.sessions*:delete"]]
        );
        assert_eq!(
            summary("WITH x AS (INSERT INTO a VALUES (1) RETURNING 1) DELETE FROM b"),
            [vec!["public.a*:insert", "public.b*:delete"]]
        );
        // A CTE named like a real table: inside its own body the name is the real table.
        assert_eq!(
            summary("WITH users AS (SELECT * FROM users WHERE active) SELECT * FROM users"),
            [vec!["public.users*:read"]]
        );
        // In a recursive CTE, its own name inside the body is the CTE.
        assert_eq!(
            summary(
                "WITH RECURSIVE t AS (SELECT 1 AS n FROM seed UNION ALL SELECT n + 1 FROM t) SELECT * FROM t"
            ),
            [vec!["public.seed*:read"]]
        );
    }

    #[test]
    fn ddl_operations() {
        let sql = "CREATE TABLE IF NOT EXISTS users (id bigserial primary key);
                   ALTER TABLE users ADD COLUMN email text;
                   CREATE INDEX users_email ON users (email);
                   CREATE VIEW active AS SELECT * FROM users;
                   DROP TABLE old_users, archive.old;
                   TRUNCATE big;
                   ALTER TABLE users RENAME TO members;
                   SELECT * INTO snapshot FROM members;";
        assert_eq!(
            summary(sql),
            [
                vec!["public.users*:create"],
                vec!["public.users*:alter"],
                vec!["public.users*:alter"],
                vec!["public.active*:create", "public.users*:read"],
                vec!["archive.old:drop", "public.old_users*:drop"],
                vec!["public.big*:truncate"],
                vec!["public.users*:alter"],
                vec!["public.members*:read", "public.snapshot*:create"],
            ]
        );
    }

    #[test]
    fn non_table_objects_and_privileges_are_not_tables() {
        let sql = "CREATE TYPE ct AS (a int); ALTER TYPE ct DROP ATTRIBUTE a; CREATE SEQUENCE s;
                   GRANT SELECT ON t TO r; LOCK TABLE t; COMMENT ON TABLE t IS 'x';
                   CREATE TABLE child (id int REFERENCES parent (id))";
        assert_eq!(
            summary(sql),
            [
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec!["public.child*:create"]
            ]
        );
    }

    #[test]
    fn relations_outside_query_statements_are_not_reads() {
        // Reached by the generic walk, but naming a table here is not reading it.
        assert_eq!(
            summary("CREATE STATISTICS st ON a, b FROM t"),
            [Vec::<String>::new()]
        );
    }

    #[test]
    fn copy_direction() {
        assert_eq!(
            summary("COPY t FROM STDIN; COPY t TO STDOUT"),
            [vec!["public.t*:insert"], vec!["public.t*:read"]]
        );
    }

    #[test]
    fn schemas_case_quoting_and_system_catalogs() {
        assert_eq!(
            summary(r#"SELECT * FROM Auth.Users, "Mixed"."Case", users, db.sales.orders"#),
            [vec![
                "Mixed.Case:read",
                "auth.users:read",
                "public.users*:read",
                "sales.orders:read"
            ]]
        );
        // Quoted identifiers keep their dots.
        let dotted = analyze(r#"SELECT * FROM "a.b", "My.Schema"."T""#);
        let keys: Vec<String> = dotted.statements[0]
            .tables
            .iter()
            .map(|t| format!("{}|{}", t.schema, t.name))
            .collect();
        assert_eq!(keys, ["My.Schema|T", "public|a.b"]);
        // System catalogs, qualified or not (pg_catalog is always searched first).
        assert_eq!(
            summary(
                "SELECT * FROM pg_catalog.pg_class, information_schema.tables, pg_stat_activity, t"
            ),
            [vec!["public.t*:read"]]
        );
        assert_eq!(
            summary("SELECT * FROM public.users JOIN users u2 ON true"),
            [vec!["public.users:read"]]
        );
    }

    #[test]
    fn splitting_respects_quotes_comments_dollar_quotes_and_meta_commands() {
        assert_eq!(
            texts("-- header\n/* block */ SELECT 1 FROM a;\n-- note\nSELECT 2 FROM b"),
            ["SELECT 1 FROM a", "SELECT 2 FROM b"]
        );
        assert_eq!(
            texts("SELECT ';' FROM a; SELECT E'it\\'s;' FROM b"),
            ["SELECT ';' FROM a", "SELECT E'it\\'s;' FROM b"]
        );
        let func = "CREATE FUNCTION f() RETURNS void AS $body$ BEGIN DELETE FROM x; END $body$ LANGUAGE plpgsql;\nSELECT $1 FROM y";
        assert_eq!(texts(func).len(), 2);
        assert_eq!(
            texts("\\c mydb\n\\set x 1\nCREATE TABLE t (id int);"),
            ["CREATE TABLE t (id int)"]
        );
        assert_eq!(
            texts("/* a /* nested */ b */ SELECT 1 FROM z"),
            ["SELECT 1 FROM z"]
        );
    }

    #[test]
    fn one_bad_statement_does_not_hide_the_others() {
        let sql = "CREATE TABLE a (id int);\nCREATE TABLE b (id INT AUTO_INCREMENT);\nCREATE TABLE c (id int);";
        let analysis = analyze(sql);
        let good: Vec<&str> = analysis
            .statements
            .iter()
            .map(|s| &sql[s.range.0..s.range.1])
            .collect();
        assert_eq!(good, ["CREATE TABLE a (id int)", "CREATE TABLE c (id int)"]);
        assert_eq!(analysis.errors.len(), 1);
        assert_eq!(
            &sql[analysis.errors[0].range.0..analysis.errors[0].range.1],
            "CREATE TABLE b (id INT AUTO_INCREMENT)"
        );
        assert!(analysis.errors[0].error.contains("syntax error"));
    }

    #[test]
    fn placeholders_are_retried_as_positional_parameters() {
        assert_eq!(
            summary("SELECT name FROM users WHERE id = ? AND org = ?"),
            [vec!["public.users*:read"]]
        );
        assert_eq!(
            summary("UPDATE t SET a = :a, b = :b_2 WHERE id = :id AND x::int = 1"),
            [vec!["public.t*:update"]]
        );
        assert_eq!(
            summary("SELECT * FROM t WHERE a = %s AND b = %(name)s"),
            [vec!["public.t*:read"]]
        );
        assert_eq!(
            positional("SELECT '?', \"a:b\", c::text, 'x %s' FROM t WHERE d = ? AND e = %(e)s"),
            "SELECT '?', \"a:b\", c::text, 'x %s' FROM t WHERE d = $1 AND e = $2"
        );
        assert_eq!(
            summary("SELECT * FROM t WHERE data ? 'k'"),
            [vec!["public.t*:read"]]
        );
        // Ranges refer to the original text even when the retry rewrote it.
        let sql = "SELECT * FROM t WHERE a = ? AND b = ?\n";
        assert_eq!(texts(sql), ["SELECT * FROM t WHERE a = ? AND b = ?"]);
    }

    #[test]
    fn prose_is_not_turned_into_sql() {
        // `?` glued to a word is not a placeholder, so this stays unparseable.
        assert_eq!(analyze("Delete from cache?").statements, []);
        assert!(analyze("SELECT * FROM ?").errors.len() == 1);
    }

    #[test]
    fn cte_names_are_scoped() {
        // A CTE inside a subquery does not hide the outer table.
        assert_eq!(
            summary(
                "SELECT * FROM users WHERE id IN (WITH users AS (SELECT 1 AS id) SELECT id FROM users)"
            ),
            [vec!["public.users*:read"]]
        );
        // A later sibling is not visible to an earlier CTE's body.
        assert_eq!(
            summary("WITH a AS (SELECT * FROM b), b AS (SELECT 1) SELECT * FROM a"),
            [vec!["public.b*:read"]]
        );
        // A CTE nested inside another CTE's body does not leak to the main query.
        assert_eq!(
            summary(
                "WITH x AS (WITH orders AS (SELECT 1) SELECT * FROM orders) SELECT * FROM x, orders"
            ),
            [vec!["public.orders*:read"]]
        );
    }

    #[test]
    fn lexer_and_parser_split_hard_cases() {
        let sql = "CREATE TABLE sys$x$y (id int); DROP TABLE victim; CREATE TABLE bad (id INT AUTO_INCREMENT);";
        let analysis = analyze(sql);
        assert_eq!(analysis.statements.len(), 2);
        assert_eq!(analysis.errors.len(), 1);
        let atomic = "CREATE FUNCTION g() RETURNS void LANGUAGE sql BEGIN ATOMIC INSERT INTO atom VALUES (1); SELECT CASE WHEN true THEN 1 END; END; SELECT 1 FROM z";
        assert_eq!(
            summary(atomic),
            [vec!["public.atom*:insert"], vec!["public.z*:read"]]
        );
        assert_eq!(
            texts("SELECT 1 FROM a; \\c db\nSELECT 2 FROM b; \\gset\n"),
            ["SELECT 1 FROM a", "SELECT 2 FROM b"]
        );
    }

    #[test]
    fn placeholders_after_comments_and_escape_strings() {
        assert_eq!(
            summary("UPDATE x SET a = 1 -- don't\n WHERE id = ?"),
            [vec!["public.x*:update"]]
        );
        assert_eq!(
            summary("SELECT * FROM t WHERE a = E'it\\'s ?' AND b = ?"),
            [vec!["public.t*:read"]]
        );
    }

    #[test]
    fn every_node_type_is_walked() {
        let cases = [
            (
                "SELECT * FROM pets TABLESAMPLE SYSTEM (10)",
                vec!["public.pets*:read"],
            ),
            (
                "UPDATE t SET (a, b) = (SELECT a, b FROM src1) RETURNING (SELECT 1 FROM r1)",
                vec!["public.r1*:read", "public.src1*:read", "public.t*:update"],
            ),
            (
                "INSERT INTO t VALUES (1) ON CONFLICT (id) DO UPDATE SET n = (SELECT n FROM c1)",
                vec!["public.c1*:read", "public.t*:insert"],
            ),
            (
                "MERGE INTO t USING s ON t.id = s.id WHEN MATCHED THEN UPDATE SET n = (SELECT n FROM m1)",
                vec!["public.m1*:read", "public.s*:read", "public.t*:merge"],
            ),
            (
                "PREPARE p AS INSERT INTO pt SELECT * FROM ps",
                vec!["public.ps*:read", "public.pt*:insert"],
            ),
            (
                "DECLARE c CURSOR FOR SELECT * FROM cur",
                vec!["public.cur*:read"],
            ),
            (
                "CREATE RULE r AS ON INSERT TO rt DO INSTEAD INSERT INTO rt2 VALUES (1)",
                vec!["public.rt*:alter", "public.rt2*:insert"],
            ),
            (
                "ALTER TABLE s.u RENAME CONSTRAINT c TO d",
                vec!["s.u:alter"],
            ),
        ];
        for (sql, expected) in cases {
            assert_eq!(summary(sql), [expected], "{sql}");
        }
    }

    #[test]
    fn keys_quote_parts_with_dots() {
        let a = analyze(r#"SELECT * FROM "public.a".b, public."a.b""#);
        let keys: Vec<String> = a.statements[0].tables.iter().map(TableUse::key).collect();
        // Sorted by raw (schema, name): ("public", "a.b") < ("public.a", "b").
        assert_eq!(keys, ["public.\"a.b\"", "\"public.a\".b"]);
    }

    #[test]
    fn unbalanced_parentheses_are_still_split_and_nothing_is_lost() {
        let sql = "SELECT bad(; SELECT bad2(; SELECT * FROM good";
        let a = analyze(sql);
        assert_eq!(a.errors.len(), 2);
        assert_eq!(summary_of(&a), [vec!["public.good*:read"]]);
    }

    #[test]
    fn whitespace_the_lexer_and_rust_disagree_on_cannot_panic() {
        for sql in [
            "-- c\n\u{a0}\n",
            " \u{a0}",
            "\u{3000}",
            "SELECT 1 FROM t\u{a0};",
        ] {
            let a = analyze(sql);
            for r in a
                .statements
                .iter()
                .map(|s| s.range)
                .chain(a.errors.iter().map(|e| e.range))
            {
                assert!(r.0 < r.1 && r.1 <= sql.len(), "{sql:?}: {r:?}");
            }
        }
    }

    #[test]
    fn non_sql_statements_do_not_hide_the_others() {
        let a = analyze("USE mydb;\nCREATE TABLE a (id int);\nINSERT INTO a SELECT * FROM b;");
        assert_eq!(a.statements.len(), 2, "{a:?}");
        assert_eq!(a.errors.len(), 1);
        let a = analyze("CREATE TABLE a (id int); garbage here; SELECT 1 FROM ok");
        assert_eq!((a.statements.len(), a.errors.len()), (2, 1));
    }

    #[test]
    fn psql_meta_commands_terminate_statements_and_copy_is_kept() {
        assert_eq!(
            summary("SELECT * FROM a \\gset\nSELECT * FROM b;"),
            [vec!["public.a*:read"], vec!["public.b*:read"]]
        );
        assert_eq!(
            summary("\\copy seeds FROM 'f.csv' WITH (FORMAT csv)\nSELECT 1 FROM x;"),
            [vec!["public.seeds*:insert"], vec!["public.x*:read"]]
        );
    }

    #[test]
    fn placeholders_glued_to_operators() {
        assert_eq!(
            summary("UPDATE t SET a=?, b=? WHERE id=?"),
            [vec!["public.t*:update"]]
        );
        assert_eq!(
            summary("SELECT * FROM t WHERE id>=? AND x<>? AND y = ?"),
            [vec!["public.t*:read"]]
        );
        assert_eq!(
            summary("SELECT * FROM t WHERE id=%(id)s AND n=%s"),
            [vec!["public.t*:read"]]
        );
        // Real `?` operators are left alone.
        assert_eq!(
            summary("SELECT * FROM t WHERE d ?| array['a'] AND e @? '$.x'"),
            [vec!["public.t*:read"]]
        );
    }

    #[test]
    fn locking_clauses_union_arms_and_more_ddl() {
        assert_eq!(
            summary("SELECT * FROM users u FOR UPDATE OF u"),
            [vec!["public.users*:read"]]
        );
        assert_eq!(
            summary("(WITH x AS (SELECT * FROM src) SELECT * FROM x) UNION SELECT * FROM y"),
            [vec!["public.src*:read", "public.y*:read"]]
        );
        assert_eq!(
            summary("SELECT * FROM a UNION (WITH c AS (SELECT 1 FROM d) SELECT * FROM c)"),
            [vec!["public.a*:read", "public.d*:read"]]
        );
        assert_eq!(
            summary("CREATE FOREIGN TABLE ft (id int) SERVER s"),
            [vec!["public.ft*:create"]]
        );
        assert_eq!(
            summary("ALTER TABLE t SET SCHEMA s2"),
            [vec!["public.t*:alter"]]
        );
        assert_eq!(
            summary("CREATE TABLE p1 PARTITION OF parent FOR VALUES IN (1)"),
            [vec!["public.p1*:create", "public.parent*:alter"]]
        );
    }

    #[test]
    fn empty_input_and_determinism() {
        assert_eq!(analyze(""), Analysis::default());
        assert_eq!(analyze("-- only a comment\n"), Analysis::default());
        let sql = "SELECT * FROM z, y, x, w, v";
        assert_eq!(summary(sql), summary(sql));
        assert_eq!(
            summary(sql)[0],
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
