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
    "createnativequery",
];

/// Callees whose string argument is a query language other than SQL (JPA/Hibernate JPQL). Such
/// strings are left to the ORM packs (M2b) instead of being misread as SQL tables.
const NON_SQL_CALLEES: &[&str] = &["createquery"];

/// Text files of `lang` (e.g. `sql`, `prisma`) to read, and oversized ones reported (same cap and
/// skips as source files).
pub(crate) fn text_plan<'f>(
    files: &'f [FileEntry],
    lang: &str,
) -> (Vec<&'f FileEntry>, Vec<Diagnostic>) {
    let mut plan = Vec::new();
    let mut diagnostics = Vec::new();
    for f in files
        .iter()
        .filter(|f| f.lang == Some(lang) && !f.binary && !f.generated)
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

/// Adds `MIGRATES` edges for every statement of a `.sql` file. Statements that do not parse are
/// diagnostics; they never hide the ones that do.
pub(crate) fn add_migrations(
    b: &mut GraphBuilder,
    path: &str,
    text: &str,
) -> anyhow::Result<Vec<Diagnostic>> {
    // Byte ranges must refer to the raw blob, so a byte order mark is skipped, not removed.
    let offset = if text.starts_with('\u{feff}') {
        '\u{feff}'.len_utf8()
    } else {
        0
    };
    let analysis = rb_sql::analyze(&text[offset..]);
    let span = |(start, end): (usize, usize)| span_of(text, (start + offset, end + offset));
    let file = NodeKey::new(NodeKind::File, path);
    let mut access = Accesses::default();
    for statement in &analysis.statements {
        let at = Evidence {
            path: path.to_owned(),
            span: span(statement.range),
        };
        for table in &statement.tables {
            access.record(&file, table, Certainty::Exact, &at);
        }
    }
    access.emit(b, EdgeKind::Migrates, Provenance::SqlFile)?;
    Ok(analysis
        .errors
        .iter()
        .map(|e| unparsed(path, Some(span(e.range)), &e.error, None))
        .collect())
}

/// Adds `QUERIES` edges for a file's SQL candidates, from their enclosing symbol (or the file);
/// `keys` is parallel to `facts.definitions`.
///
/// A candidate passed to a known SQL sink is `exact`; any other candidate that parses is
/// `heuristic`. Statements that do not parse give a diagnostic only in a sink argument; otherwise
/// they are almost always prose ("Update failed") and are dropped.
pub(crate) fn add_queries(
    b: &mut GraphBuilder,
    path: &str,
    facts: &FileFacts,
    keys: &[String],
) -> anyhow::Result<Vec<Diagnostic>> {
    let file = NodeKey::new(NodeKind::File, path);
    let mut access = Accesses::default();
    let mut diagnostics = Vec::new();
    for candidate in &facts.sql {
        let callee = candidate.callee.as_deref().map(str::to_ascii_lowercase);
        if callee
            .as_deref()
            .is_some_and(|c| NON_SQL_CALLEES.contains(&c))
        {
            continue;
        }
        let sink = callee.as_deref().is_some_and(|c| SQL_SINKS.contains(&c));
        let analysis = rb_sql::analyze(&candidate.text);
        // One diagnostic per call site: the first error, and how many statements failed.
        if let (true, Some(first)) = (sink, analysis.errors.first()) {
            let mut d = unparsed(
                path,
                Some(candidate.span),
                &first.error,
                candidate.callee.as_deref(),
            );
            d.detail["failed_statements"] = analysis.errors.len().into();
            diagnostics.push(d);
        }
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
        for table in analysis.statements.iter().flat_map(|s| &s.tables) {
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
pub(crate) struct Accesses(BTreeMap<(NodeKey, String), Access>);

impl Accesses {
    pub(crate) fn record(
        &mut self,
        source: &NodeKey,
        table: &TableUse,
        certainty: Certainty,
        at: &Evidence,
    ) {
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

    pub(crate) fn emit(
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

pub(crate) fn unparsed(
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

    fn keys(path: &str, facts: &FileFacts) -> Vec<String> {
        rb_syntax::symbol_keys(path, facts)
    }

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

    #[test]
    fn byte_order_mark_and_bad_statements_do_not_hide_good_ones() {
        let sql = "\u{feff}CREATE TABLE widgets (id int);\nCREATE TABLE b (id INT AUTO_INCREMENT);\nCREATE TABLE gadgets (id int);\n";
        let (g, diagnostics) = graph_with("V1.sql", |b| add_migrations(b, "V1.sql", sql).unwrap());
        assert_eq!(
            edges(&g, EdgeKind::Migrates),
            [
                r#"V1.sql -> public.gadgets ops=["create"] Exact assumed @2"#,
                r#"V1.sql -> public.widgets ops=["create"] Exact assumed @0"#,
            ]
        );
        // Evidence spans count raw bytes: the statement starts after the 3-byte BOM.
        let widgets = g
            .edges()
            .iter()
            .find(|e| e.evidence[0].span[0] == 0)
            .unwrap();
        assert_eq!(widgets.evidence[0].span[1], 3);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].span.map(|s| s[0]), Some(1));
    }

    #[test]
    fn one_diagnostic_per_unparseable_sink_call() {
        // Two invalid statements the lexer can still split, and one valid one.
        let src = "db.query('SELECT FROM WHERE; SELECT * FORM x; SELECT * FROM good')\n";
        let facts = rb_syntax::extract(Language::JavaScript, src.as_bytes()).unwrap();
        let mut diagnostics = Vec::new();
        let (g, _) = graph_with("a.js", |b| {
            diagnostics = add_queries(b, "a.js", &facts, &keys("a.js", &facts)).unwrap();
            Vec::new()
        });
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].detail["failed_statements"], 2);
        // The statement that parses still counts.
        assert_eq!(
            edges(&g, EdgeKind::Queries),
            [r#"a.js -> public.good ops=["read"] Exact assumed @0"#]
        );
    }

    #[test]
    fn jpql_is_not_sql_but_native_queries_are() {
        let src = "class R {\n  void f() {\n    em.createQuery(\"SELECT c FROM Customer c\");\n    em.createNativeQuery(\"SELECT * FROM customer\");\n  }\n}\n";
        let facts = rb_syntax::extract(Language::Java, src.as_bytes()).unwrap();
        let (g, _) = graph_with("R.java", |b| {
            let keys = keys("R.java", &facts);
            crate::syntax::add_symbols(b, "R.java", &facts, &keys).unwrap();
            add_queries(b, "R.java", &facts, &keys).unwrap()
        });
        assert_eq!(
            edges(&g, EdgeKind::Queries),
            [r#"R#f(). -> public.customer ops=["read"] Exact assumed @3"#]
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
            let keys = keys("src/load.ts", &facts);
            crate::syntax::add_symbols(b, "src/load.ts", &facts, &keys).unwrap();
            diagnostics = add_queries(b, "src/load.ts", &facts, &keys).unwrap();
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
            let keys = keys("jobs/cleanup.py", &facts);
            crate::syntax::add_symbols(b, "jobs/cleanup.py", &facts, &keys).unwrap();
            add_queries(b, "jobs/cleanup.py", &facts, &keys).unwrap()
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
