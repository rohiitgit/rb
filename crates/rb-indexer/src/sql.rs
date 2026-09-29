//! SQL stage (design §7): `.sql` files become `MIGRATES` edges and SQL candidates in code become
//! `QUERIES` edges, both to `Table` nodes. Edges are aggregated per (source, table) so each is
//! added once, with the union of operations as its attribute and every site as evidence.

use std::collections::{BTreeMap, BTreeSet};

use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, FileEntry, GraphBuilder, NewEdge, NodeKey,
    NodeKind, Provenance,
};
use rb_sql::{Op, TableUse};
use rb_syntax::FileFacts;
use serde_json::json;

/// Callee names that mark a string argument as SQL executed by a database API (case-insensitive).
pub(crate) const SQL_SINKS: &[&str] = &[
    "query",
    "queryrow",
    "querycontext",
    "queryrowcontext",
    "exec",
    "execcontext",
    "execute",
    "executemany",
    "prepare",
    "preparecontext",
    "queryforobject",
    "queryforlist",
    "queryformap",
    "update",
    "batchupdate",
    "raw",
    "$queryraw",
    "$executeraw",
    "$queryrawunsafe",
    "$executerawunsafe",
    "sql",
    "text",
];

/// `.sql` files to read, and oversized ones reported (same cap and skips as source files).
pub(crate) fn sql_plan(files: &[FileEntry]) -> (Vec<&FileEntry>, Vec<Diagnostic>) {
    let mut plan = Vec::new();
    let mut diagnostics = Vec::new();
    for f in files
        .iter()
        .filter(|f| f.lang == Some("sql") && !f.binary && !f.generated)
    {
        if f.size > rb_syntax::MAX_PARSE_BYTES {
            diagnostics.push(Diagnostic {
                kind: "extraction_skipped".into(),
                path: Some(f.path.clone()),
                span: None,
                detail: json!({"reason": "too_large", "size": f.size}),
            });
        } else {
            plan.push(f);
        }
    }
    (plan, diagnostics)
}

/// Adds `MIGRATES` edges for every statement of a `.sql` file; a parse failure is a diagnostic.
pub(crate) fn add_migrations(
    b: &mut GraphBuilder,
    path: &str,
    text: &str,
) -> anyhow::Result<Vec<Diagnostic>> {
    let statements = match rb_sql::analyze(text) {
        Ok(statements) => statements,
        Err(error) => {
            return Ok(vec![unparsed(path, None, &error, None)]);
        }
    };
    let file = NodeKey::new(NodeKind::File, path);
    let mut access = Accesses::default();
    for statement in &statements {
        let at = Evidence {
            path: path.to_owned(),
            span: span_of(text, statement.range),
        };
        for table in &statement.tables {
            access.record(&file, table, Certainty::Exact, &at);
        }
    }
    access.emit(b, EdgeKind::Migrates, Provenance::SqlFile)?;
    Ok(Vec::new())
}

/// Adds `QUERIES` edges for a file's SQL candidates, from their enclosing symbol (or the file).
///
/// A candidate passed to a known SQL sink is `exact`; any other candidate that parses is
/// `heuristic`. A candidate that does not parse gives a diagnostic only when it is a sink
/// argument; otherwise it is almost always prose ("Update failed") and is dropped.
pub(crate) fn add_queries(
    b: &mut GraphBuilder,
    path: &str,
    facts: &FileFacts,
) -> anyhow::Result<Vec<Diagnostic>> {
    let keys = rb_syntax::symbol_keys(path, facts);
    let file = NodeKey::new(NodeKind::File, path);
    let mut access = Accesses::default();
    let mut diagnostics = Vec::new();
    for candidate in &facts.sql {
        let sink = candidate
            .callee
            .as_deref()
            .is_some_and(|c| SQL_SINKS.contains(&c.to_ascii_lowercase().as_str()));
        let statements = match rb_sql::analyze(&candidate.text) {
            Ok(statements) => statements,
            Err(error) => {
                if sink {
                    diagnostics.push(unparsed(
                        path,
                        Some(candidate.span),
                        &error,
                        candidate.callee.as_deref(),
                    ));
                }
                continue;
            }
        };
        let source = match candidate.enclosing {
            Some(i) => NodeKey::new(NodeKind::Symbol, keys[i].clone()),
            None => file.clone(),
        };
        let certainty = if sink {
            Certainty::Exact
        } else {
            Certainty::Heuristic
        };
        let at = Evidence {
            path: path.to_owned(),
            span: candidate.span,
        };
        for table in statements.iter().flat_map(|s| &s.tables) {
            access.record(&source, table, certainty, &at);
        }
    }
    access.emit(b, EdgeKind::Queries, Provenance::SqlLiteral)?;
    Ok(diagnostics)
}

/// How one source accesses one table, accumulated over every site.
struct Access {
    name: String,
    ops: BTreeSet<Op>,
    schema_assumed: bool,
    certainty: Certainty,
    evidence: BTreeSet<Evidence>,
}

#[derive(Default)]
struct Accesses(BTreeMap<(NodeKey, String), Access>);

impl Accesses {
    fn record(&mut self, source: &NodeKey, table: &TableUse, certainty: Certainty, at: &Evidence) {
        let access = self
            .0
            .entry((source.clone(), table.key()))
            .or_insert_with(|| Access {
                name: table.name.clone(),
                ops: BTreeSet::new(),
                schema_assumed: table.schema_assumed,
                certainty,
                evidence: BTreeSet::new(),
            });
        access.ops.extend(table.ops.iter().copied());
        access.schema_assumed &= table.schema_assumed;
        access.certainty = access.certainty.max(certainty);
        access.evidence.insert(at.clone());
    }

    fn emit(
        self,
        b: &mut GraphBuilder,
        kind: EdgeKind,
        provenance: Provenance,
    ) -> anyhow::Result<()> {
        for ((source, table_key), access) in self.0 {
            let table = NodeKey::new(NodeKind::Table, table_key);
            if !b.has_node(&table) {
                b.add_node(table.clone(), &access.name, Attrs::new())?;
            }
            let ops: Vec<&str> = access.ops.iter().map(|o| o.as_str()).collect();
            let mut attrs = Attrs::new();
            attrs.insert("ops".into(), json!(ops));
            if access.schema_assumed {
                attrs.insert("schema_assumed".into(), true.into());
            }
            let mut edge =
                NewEdge::new(source, kind, table, provenance, access.certainty).attrs(attrs);
            edge.evidence = access.evidence.into_iter().collect();
            b.add_edge(edge)?;
        }
        Ok(())
    }
}

fn unparsed(
    path: &str,
    span: Option<rb_model::Span>,
    error: &str,
    callee: Option<&str>,
) -> Diagnostic {
    let mut detail = json!({ "error": error });
    if let Some(callee) = callee {
        detail["callee"] = callee.into();
    }
    Diagnostic {
        kind: "unparsed_sql".into(),
        path: Some(path.to_owned()),
        span,
        detail,
    }
}

/// 0-based `[line, byte col, line, byte col]` of a byte range within `text`.
fn span_of(text: &str, (start, end): (usize, usize)) -> rb_model::Span {
    let point = |at: usize| {
        let before = &text.as_bytes()[..at];
        let line = before.iter().filter(|&&b| b == b'\n').count();
        let col = at
            - before
                .iter()
                .rposition(|&b| b == b'\n')
                .map_or(0, |p| p + 1);
        (
            i32::try_from(line).unwrap_or(i32::MAX),
            i32::try_from(col).unwrap_or(i32::MAX),
        )
    };
    let ((l0, c0), (l1, c1)) = (point(start), point(end));
    [l0, c0, l1, c1]
}

#[cfg(test)]
mod tests {
    use rb_model::Graph;
    use rb_syntax::Language;

    use super::*;

    fn graph_with(
        path: &str,
        build: impl FnOnce(&mut GraphBuilder) -> Vec<Diagnostic>,
    ) -> (Graph, Vec<Diagnostic>) {
        let mut b = GraphBuilder::new();
        b.add_node(NodeKey::new(NodeKind::File, path), path, Attrs::new())
            .unwrap();
        let diagnostics = build(&mut b);
        (b.finish().unwrap(), diagnostics)
    }

    /// `src -KIND-> dst ops=[..] certainty [assumed] @lines` per edge of `kind`.
    fn edges(g: &Graph, kind: EdgeKind) -> Vec<String> {
        let key = |id: i64| g.nodes().iter().find(|n| n.id == id).unwrap().key.clone();
        g.edges()
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| {
                let short = |k: String| k.rsplit('/').next().unwrap_or(&k).to_owned();
                let lines: Vec<String> =
                    e.evidence.iter().map(|ev| ev.span[0].to_string()).collect();
                let assumed = if e.attrs.contains_key("schema_assumed") {
                    " assumed"
                } else {
                    ""
                };
                format!(
                    "{} -> {} ops={} {:?}{assumed} @{}",
                    short(key(e.src)),
                    key(e.dst),
                    e.attrs["ops"],
                    e.certainty,
                    lines.join(",")
                )
            })
            .collect()
    }

    #[test]
    fn migrations_record_every_statement_with_its_line() {
        let sql = "-- users\nCREATE TABLE users (id bigserial);\nCREATE INDEX users_id ON users (id);\n\nINSERT INTO auth.roles VALUES (1);\n";
        let (g, diagnostics) = graph_with("db/V1__init.sql", |b| {
            add_migrations(b, "db/V1__init.sql", sql).unwrap()
        });
        assert!(diagnostics.is_empty());
        assert_eq!(
            edges(&g, EdgeKind::Migrates),
            [
                r#"V1__init.sql -> auth.roles ops=["insert"] Exact @4"#,
                r#"V1__init.sql -> public.users ops=["create","alter"] Exact assumed @1,2"#,
            ]
        );
        let tables: Vec<&str> = g
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::Table)
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(tables, ["roles", "users"]);
    }

    #[test]
    fn unparsable_migrations_are_diagnostics_not_edges() {
        let sql = "CREATE TABLE t (id INT AUTO_INCREMENT PRIMARY KEY) ENGINE=InnoDB;";
        let (g, diagnostics) = graph_with("m.sql", |b| add_migrations(b, "m.sql", sql).unwrap());
        assert!(edges(&g, EdgeKind::Migrates).is_empty());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].kind, "unparsed_sql");
        assert!(
            diagnostics[0].detail["error"]
                .as_str()
                .unwrap()
                .contains("syntax error")
        );
    }

    const TS: &str = "export async function load(id: string) {
  await db.query(`SELECT * FROM users WHERE id = ${id}`);
  const q = 'UPDATE users SET seen = now()';
  await db.query(`SELECT * FROM ${table}`);
  notify('Update failed');
}
const REPORT = 'SELECT * FROM public.orders';
";

    #[test]
    fn code_queries_aggregate_per_symbol_and_table() {
        let facts = rb_syntax::extract(Language::TypeScript, TS.as_bytes()).unwrap();
        let mut diagnostics = Vec::new();
        let (g, _) = graph_with("src/load.ts", |b| {
            crate::syntax::add_symbols(b, "src/load.ts", &facts).unwrap();
            diagnostics = add_queries(b, "src/load.ts", &facts).unwrap();
            Vec::new()
        });
        assert_eq!(
            edges(&g, EdgeKind::Queries),
            [
                // The sink literal is exact, so the merged edge is exact; both sites are evidence.
                r#"load(). -> public.users ops=["read","update"] Exact assumed @1,2"#,
                // JavaScript's tags do not make a string constant a definition: the file owns it.
                r#"load.ts -> public.orders ops=["read"] Heuristic @6"#,
            ]
        );
        // The sink call with a hole in table position is reported; the prose string is not.
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].kind, "unparsed_sql");
        assert_eq!(diagnostics[0].span.map(|s| s[0]), Some(3));
        assert_eq!(diagnostics[0].detail["callee"], "query");
    }

    #[test]
    fn literals_outside_definitions_are_attributed_to_the_file() {
        let src = "db.execute('DELETE FROM sessions')\n";
        let facts = rb_syntax::extract(Language::Python, src.as_bytes()).unwrap();
        let (g, _) = graph_with("jobs/cleanup.py", |b| {
            crate::syntax::add_symbols(b, "jobs/cleanup.py", &facts).unwrap();
            add_queries(b, "jobs/cleanup.py", &facts).unwrap()
        });
        assert_eq!(
            edges(&g, EdgeKind::Queries),
            [r#"cleanup.py -> public.sessions ops=["delete"] Exact assumed @0"#]
        );
        let e = g
            .edges()
            .iter()
            .find(|e| e.kind == EdgeKind::Queries)
            .unwrap();
        assert_eq!(e.certainty, Certainty::Exact);
    }
}
