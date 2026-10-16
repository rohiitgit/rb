//! SQL stage (design §7): `.sql` files become `MIGRATES` edges and SQL candidates in code become
//! `QUERIES` edges, both to `Table` nodes. Edges are aggregated per (source, table) so each is
//! added once, with the union of operations as its attribute and every site as evidence.

use std::collections::{BTreeMap, BTreeSet};

use rb_frameworks::routes::FileScope;
use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, FileEntry, GraphBuilder, NewEdge, NodeKey,
    NodeKind, Provenance,
};
use rb_sql::{Op, TableUse};
use rb_syntax::{Arg, FileFacts, Init, Language, SqlCandidate, TemplatePart};
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

/// One MyBatis mapper statement: its `id`, folded SQL text and span from the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MapperStmt {
    pub id: String,
    pub sql: String,
    pub span: rb_model::Span,
}

/// A MyBatis mapper file read: its `namespace`, plain statements and dynamic-skipped
/// statement ids with spans and reasons.
pub(crate) struct MapperFile {
    pub namespace: String,
    pub stmts: Vec<MapperStmt>,
    pub dynamic: Vec<(String, rb_model::Span, &'static str)>,
}

/// Statement tags a mapper file may hold; anything else (dynamic tags like `<if>`, fragments)
/// is out of the allow-list.
const MAPPER_STATEMENTS: &[&str] = &["select", "insert", "update", "delete"];

/// Dynamic MyBatis tags: a statement holding any of these is skipped and counted, since its
/// SQL depends on runtime conditions (contract M6f S5 residue).
const MAPPER_DYNAMIC: &[&str] = &[
    "if",
    "choose",
    "when",
    "otherwise",
    "foreach",
    "where",
    "trim",
    "set",
    "include",
    "bind",
];

/// Reads a MyBatis mapper file: its `namespace`, plain statements and dynamic-skipped
/// statement ids with spans. `None` when the root is not `<mapper>` (poms and other XML pass
/// through untouched). `#{p}` and `${p}` placeholders fold to `?` (table extraction only).
pub(crate) fn mapper_file(text: &str) -> Option<MapperFile> {
    let root = crate::packages::parse_xml(text)?;
    if root.name != "mapper" {
        return None;
    }
    let namespace = root.attr("namespace").unwrap_or_default().to_owned();
    let mut stmts = Vec::new();
    let mut dynamic = Vec::new();
    for child in &root.children {
        if !MAPPER_STATEMENTS.contains(&child.name.as_str()) {
            continue;
        }
        let span = span_of(text, (child.start, child.end));
        let id = child.attr("id").unwrap_or_default().to_owned();
        // Any element child (dynamic tags, `<include>`, nested `<selectKey>`) takes the
        // statement out of the allow-list: pure-text statements only, skipped ones counted
        // with their reason.
        if !child.children.is_empty() {
            let reason = if child
                .children
                .iter()
                .any(|c| MAPPER_DYNAMIC.contains(&c.name.as_str()))
            {
                "dynamic"
            } else {
                "nested"
            };
            dynamic.push((id, span, reason));
            continue;
        }
        stmts.push(MapperStmt {
            id,
            sql: fold_placeholders(&child.text),
            span,
        });
    }
    Some(MapperFile {
        namespace,
        stmts,
        dynamic,
    })
}

/// `#{p}` and `${p}` placeholders become `?` (an unclosed placeholder ends the fold; the
/// statement still reads).
fn fold_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(['#', '$']) {
        let (head, tail) = rest.split_at(i);
        out.push_str(head);
        if tail.starts_with("#{") || tail.starts_with("${") {
            match tail.find('}') {
                Some(end) => {
                    out.push('?');
                    rest = &tail[end + 1..];
                }
                None => {
                    out.push_str(tail);
                    rest = "";
                }
            }
        } else {
            out.push_str(&tail[..1]);
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Adds exact `QUERIES` edges for a MyBatis mapper file's plain statements, from the file;
/// dynamic statements and parse failures become counted diagnostics, never edges.
pub(crate) fn add_mapper_queries(
    b: &mut GraphBuilder,
    path: &str,
    text: &str,
) -> anyhow::Result<Vec<Diagnostic>> {
    let Some(MapperFile {
        namespace,
        stmts,
        dynamic,
    }) = mapper_file(text)
    else {
        return Ok(Vec::new());
    };
    let file = NodeKey::new(NodeKind::File, path);
    let mut access = Accesses::default();
    let mut diagnostics = Vec::new();
    for stmt in &stmts {
        let analysis = rb_sql::analyze(&stmt.sql);
        let at = Evidence {
            path: path.to_owned(),
            span: stmt.span,
        };
        if let Some(first) = analysis.errors.first() {
            let callee = format!("{namespace}.{}", stmt.id);
            diagnostics.push(unparsed(path, Some(stmt.span), &first.error, Some(&callee)));
        }
        for table in analysis.statements.iter().flat_map(|s| &s.tables) {
            access.record(&file, table, Certainty::Exact, &at);
        }
    }
    access.emit(b, EdgeKind::Queries, Provenance::SqlFile)?;
    for (id, span, reason) in &dynamic {
        diagnostics.push(Diagnostic {
            kind: "mybatis_dynamic".into(),
            path: Some(path.to_owned()),
            span: Some(*span),
            detail: json!({ "namespace": namespace, "id": id, "reason": reason }),
        });
    }
    Ok(diagnostics)
}

/// Adds a `.sql` file's edges: `MIGRATES` for every statement of a migration, `QUERIES` (from
/// the file) for every statement of a query file ([`is_migration`], contract M7d S5).
/// Statements that do not parse are diagnostics; they never hide the ones that do.
pub(crate) fn add_sql_file(
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
    let kind = if is_migration(path, text, &analysis) {
        EdgeKind::Migrates
    } else {
        EdgeKind::Queries
    };
    access.emit(b, kind, Provenance::SqlFile)?;
    Ok(analysis
        .errors
        .iter()
        .map(|e| unparsed(path, Some(span(e.range)), &e.error, None))
        .collect())
}

/// Whether a `.sql` file is a migration (contract M7d S5): never a query file of named
/// statements (`-- name:` aiosql/yesql, `-- :name` HugSQL); else a migrations path convention (a
/// `migrations`/`migration`/`migrate`/`flyway`/`liquibase` directory, Flyway `V…__`/`U…__`/
/// `R__`, a numbered `001_…` name, `.up.sql`/`.down.sql`); else any DDL statement, or a file
/// of which nothing parsed (its diagnostics stand as before).
fn is_migration(path: &str, text: &str, analysis: &rb_sql::Analysis) -> bool {
    // aiosql/yesql write `-- name: q`, HugSQL `-- :name q`; pg_dump's `-- Name: t; Type: …`
    // comments are none of them.
    if text.lines().any(|l| {
        let l = l.trim_start();
        (l.starts_with("-- name:") && !l.contains("; Type:")) || l.starts_with("-- :name ")
    }) {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    let (dirs, file) = lower.rsplit_once('/').unwrap_or(("", lower.as_str()));
    let dir_named = dirs.split('/').any(|d| {
        matches!(
            d,
            "migrations" | "migration" | "migrate" | "flyway" | "liquibase"
        )
    });
    let versioned = |prefix: char| {
        file.strip_prefix(prefix)
            .and_then(|r| r.split_once("__"))
            .is_some_and(|(v, _)| {
                !v.is_empty()
                    && v.chars()
                        .all(|c| c.is_ascii_digit() || c == '_' || c == '.')
            })
    };
    let numbered = file
        .split(['_', '-', '.'])
        .next()
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
    if dir_named
        || versioned('v')
        || versioned('u')
        || file.starts_with("r__")
        || numbered
        || file.ends_with(".up.sql")
        || file.ends_with(".down.sql")
    {
        return true;
    }
    analysis.statements.is_empty()
        || analysis.statements.iter().any(|s| {
            s.tables.iter().any(|t| {
                t.ops.iter().any(|o| {
                    matches!(
                        o,
                        rb_sql::Op::Create
                            | rb_sql::Op::Alter
                            | rb_sql::Op::Drop
                            | rb_sql::Op::Truncate
                    )
                })
            })
        })
}

/// Go package-level string constants for folding `fmt.Sprintf` SQL (contract M9b D6): per
/// (package key, name), the declaring file, binding and value. A name enters only when its
/// package declares it exactly once at the top level (a `const`, a `var`, a function or a type)
/// and that declaration is an untyped or `string` `const` of a non-test file whose value is one
/// string literal.
#[derive(Default)]
pub(crate) struct GoConsts<'a> {
    table: BTreeMap<((&'a str, &'a str), &'a str), GoConst<'a>>,
}

#[derive(Clone, Copy)]
struct GoConst<'a> {
    file: &'a str,
    binding: usize,
    value: &'a str,
}

impl<'a> GoConsts<'a> {
    pub(crate) fn new(files: &BTreeMap<&'a str, (Language, &'a FileFacts)>) -> Self {
        let mut declared: BTreeMap<((&str, &str), &str), usize> = BTreeMap::new();
        let mut table = BTreeMap::new();
        for (&path, &(language, facts)) in files {
            if language != Language::Go {
                continue;
            }
            let Some(key) = crate::resolve::package_key(path, language, facts) else {
                continue;
            };
            for (i, b) in facts.bindings.iter().enumerate() {
                if b.scope.is_some() || b.member {
                    continue;
                }
                *declared.entry((key, b.name.as_str())).or_default() += 1;
                let value = match &b.init {
                    Some(Init::Template { template }) => match template.parts.as_slice() {
                        [TemplatePart::Lit { text }] => Some(text.as_str()),
                        _ => None,
                    },
                    _ => None,
                };
                // Untyped or `string` only: `%s`/`%v` print a named type through its `String()`,
                // `Error()` or `Format` method, not its literal (M9b pass 1, E3). `type_name` is
                // empty for both (it leaves built-in scalars out).
                let constant = b.constant && b.type_name.is_empty();
                let test_file = rb_model::is_test_path(path);
                if let (true, false, Some(value)) = (constant, test_file, value) {
                    let c = GoConst {
                        file: path,
                        binding: i,
                        value,
                    };
                    table.insert((key, b.name.as_str()), c);
                }
            }
            let top = facts
                .definitions
                .iter()
                .filter(|d| d.parent.is_none() && d.receiver.is_none());
            for d in top {
                *declared.entry((key, d.name.as_str())).or_default() += 1;
            }
        }
        table.retain(|k, _| declared.get(k) == Some(&1));
        GoConsts { table }
    }

    /// `candidate`'s text with its `fmt.Sprintf` holes folded, and the constants it used; `None`
    /// leaves the candidate as it is (contract M9b D6).
    fn fold(
        &self,
        path: &str,
        facts: &FileFacts,
        scope: &FileScope<'_>,
        candidate: &SqlCandidate,
    ) -> Option<(String, Vec<String>)> {
        if candidate.callee.as_deref() != Some("Sprintf") {
            return None;
        }
        // The call whose first argument is this literal.
        let chain = facts.chains.iter().find(|c| {
            c.links.first().is_some_and(|l| {
                matches!(l.args.first(), Some(Arg::Template { template }) if template.span == candidate.span)
            })
        })?;
        let link = &chain.links[0];
        if link.name != "Sprintf" || chain.root.as_deref() != Some("fmt") {
            return None;
        }
        if !fmt_is_the_fmt_package(facts, scope, chain.span) {
            return None;
        }
        let Some(Arg::Template { template }) = link.args.first() else {
            return None;
        };
        let [TemplatePart::Lit { text: format }] = template.parts.as_slice() else {
            return None;
        };
        let key = crate::resolve::package_key(path, Language::Go, facts)?;
        // Read at the file's lifetime (the map is covariant).
        let table: &BTreeMap<((&str, &str), &str), GoConst<'_>> = &self.table;
        let mut names = Vec::new();
        let mut values = Vec::new();
        for arg in &link.args[1..] {
            let Arg::Name { text, span } = arg else {
                return None;
            };
            let c = table.get(&(key, text.as_str()))?;
            // Another binding of the name in effect at the argument shadows the constant; the
            // constant's own top-level binding (a same-file constant) does not.
            let shadowed = scope
                .binding_index_at(text, *span)
                .is_some_and(|i| (path, i) != (c.file, c.binding));
            if shadowed {
                return None;
            }
            names.push(text.clone());
            values.push(c.value);
        }
        Some((sprintf(format, &values)?, names))
    }
}

/// Whether `fmt` at `at` is the standard `fmt` package: the file imports it under that name, no
/// import binds `fmt` to another module, and no binding of `fmt` is in effect there.
fn fmt_is_the_fmt_package(facts: &FileFacts, scope: &FileScope<'_>, at: rb_model::Span) -> bool {
    let binds_fmt: Vec<&str> = facts
        .imports
        .iter()
        .filter(|i| match i.alias.as_deref() {
            Some(alias) => alias == "fmt",
            None => i.module.rsplit('/').next() == Some("fmt"),
        })
        .map(|i| i.module.as_str())
        .collect();
    !binds_fmt.is_empty()
        && binds_fmt.iter().all(|m| *m == "fmt")
        && scope.binding_index_at("fmt", at).is_none()
}

/// `fmt.Sprintf(format, values…)` for `%s` and `%v` (each takes the next value) and `%%`; any
/// other verb, flag, width or index, or a count mismatch, is `None`.
fn sprintf(format: &str, values: &[&str]) -> Option<String> {
    let mut out = String::with_capacity(format.len());
    let mut next = values.iter();
    let mut chars = format.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            Some('s' | 'v') => out.push_str(next.next()?),
            _ => return None,
        }
    }
    next.next().is_none().then_some(out)
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
    consts: &GoConsts<'_>,
) -> anyhow::Result<Vec<Diagnostic>> {
    let file = NodeKey::new(NodeKind::File, path);
    let mut access = Accesses::default();
    let mut diagnostics = Vec::new();
    // Contract M9b D6: Go `fmt.Sprintf` candidates fold package constants into their holes.
    let scope = facts
        .sql
        .iter()
        .any(|c| c.callee.as_deref() == Some("Sprintf"))
        .then(|| FileScope::new(facts));
    let (mut folded_sites, mut folded_constants) = (0usize, BTreeSet::new());
    for candidate in &facts.sql {
        let callee = candidate.callee.as_deref().map(str::to_ascii_lowercase);
        if callee
            .as_deref()
            .is_some_and(|c| NON_SQL_CALLEES.contains(&c))
        {
            continue;
        }
        let sink = callee.as_deref().is_some_and(|c| SQL_SINKS.contains(&c));
        let folded = scope
            .as_ref()
            .and_then(|scope| consts.fold(path, facts, scope, candidate));
        if let Some((_, names)) = &folded {
            folded_sites += 1;
            folded_constants.extend(names.iter().cloned());
        }
        let text = folded
            .as_ref()
            .map_or(candidate.text.as_str(), |(t, _)| t.as_str());
        let analysis = rb_sql::analyze(text);
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
    if folded_sites > 0 {
        diagnostics.push(Diagnostic {
            kind: "sql_folded".into(),
            path: Some(path.to_owned()),
            span: None,
            detail: json!({ "sites": folded_sites, "constants": folded_constants.len() }),
        });
    }
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
            add_sql_file(b, "db/V1__init.sql", sql).unwrap()
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

    /// Contract M6f S5: mapper statements pin namespace, id, folded SQL and spans from the
    /// tree; dynamic statements are skipped and counted; non-mapper XML yields nothing.
    #[test]
    fn mybatis_mapper_statements_pin_namespace_id_and_sql() {
        let xml = "<?xml version=\"1.0\"?>\n<mapper namespace=\"com.acme.UserMapper\">\n  <select id=\"findAll\" resultType=\"User\">\n    SELECT id, name FROM users\n  </select>\n  <select id=\"findById\" resultMap=\"m\">\n    SELECT id FROM users WHERE id = #{id}\n  </select>\n  <insert id=\"insert\">INSERT INTO users (name) VALUES (#{name})</insert>\n  <select id=\"dyn\">SELECT a FROM t <if test=\"x != null\">AND x = #{x}</if></select>\n  <select id=\"sub\">SELECT a FROM t WHERE x IN ${xs}</select>\n</mapper>\n";
        let MapperFile {
            namespace,
            stmts,
            dynamic,
        } = mapper_file(xml).unwrap();
        assert_eq!(namespace, "com.acme.UserMapper");
        assert_eq!(
            stmts.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["findAll", "findById", "insert", "sub"]
        );
        assert!(
            stmts
                .iter()
                .all(|s| !s.sql.contains("#{") && !s.sql.contains("${"))
        );
        assert!(stmts[1].sql.contains('?'));
        // Spans come from the tree: the statement tags, never hand-counted.
        let row_of = |span: rb_model::Span| span[0];
        assert_eq!(
            stmts.iter().map(|s| row_of(s.span)).collect::<Vec<_>>(),
            [2, 5, 8, 10]
        );
        assert_eq!(dynamic.len(), 1);
        assert_eq!(dynamic[0].0, "dyn");
        // Anything else is not a mapper; a namespace-less mapper still reads.
        assert!(mapper_file("<project><a/></project>").is_none());
        assert!(mapper_file("))) garbage (((").is_none());
        let no_ns = mapper_file("<mapper><select id=\"s\">SELECT 1</select></mapper>").unwrap();
        assert_eq!(no_ns.namespace, "");
        assert_eq!(no_ns.stmts.len(), 1);
    }

    /// Contract M6f S5: mapper statements become exact `QUERIES` from their file; dynamic
    /// statements become counted diagnostics, never edges.
    #[test]
    fn mybatis_mapper_queries_reach_their_tables() {
        let xml = "<mapper namespace=\"com.acme.M\">\n  <select id=\"findAll\">\n    SELECT id FROM users\n  </select>\n  <select id=\"dyn\">SELECT a FROM t <if test=\"x\">AND x = 1</if></select>\n  <select id=\"bad\">SELECT FROM WHERE</select>\n</mapper>\n";
        let (g, diagnostics) = graph_with("db/UserMapper.xml", |b| {
            add_mapper_queries(b, "db/UserMapper.xml", xml).unwrap()
        });
        assert_eq!(
            edges(&g, EdgeKind::Queries),
            [r#"UserMapper.xml -> public.users ops=["read"] Exact assumed @1"#]
        );
        let kinds: Vec<&str> = diagnostics.iter().map(|d| d.kind.as_str()).collect();
        assert_eq!(kinds, ["unparsed_sql", "mybatis_dynamic"]);
        assert_eq!(diagnostics[1].detail["id"], json!("dyn"));
    }

    /// Contract M7d S5 (stress F23, fastapi-realworld): a `.sql` file is a migration only by
    /// evidence — a migrations path convention, or DDL; an aiosql/HugSQL query file
    /// (`-- name:`) or a DML-only file holds queries of that file (`QUERIES`).
    #[test]
    fn sql_files_are_migrations_only_by_evidence() {
        let named = "-- name: get-article^\nSELECT slug FROM articles WHERE id = 1;\n\n-- name: create-article!\nINSERT INTO articles (slug) VALUES ('x');\n";
        let select = "SELECT id FROM users;\n";
        let ddl = "CREATE TABLE users (id bigserial);\n";
        for (path, sql, migrates, queries) in [
            ("app/db/queries/sql/articles.sql", named, 0, 1),
            ("db/migrations/0001_seed.sql", select, 1, 0),
            ("db/migrations/seed.sql", select, 1, 0),
            ("sql/001_seed.sql", select, 1, 0),
            ("db/V3__seed.sql", select, 1, 0),
            ("db/seed.up.sql", select, 1, 0),
            ("schema.sql", ddl, 1, 0),
            ("reports/top_users.sql", select, 0, 1),
            // A `-- name:` file is queries even under a migrations directory.
            ("migrations/queries.sql", named, 0, 1),
            // HugSQL marks its queries `-- :name`.
            (
                "src/hug.sql",
                "-- :name create-users :!\nCREATE TABLE users (id int);\n",
                0,
                1,
            ),
            // A pg_dump schema's `-- Name: x; Type: TABLE` comments are no query markers
            // (found measuring mattermost: 87 dumps under `server/scripts`).
            (
                "server/scripts/dump.sql",
                "--\n-- Name: audits; Type: TABLE; Schema: public; Owner: -\n--\nCREATE TABLE public.audits (id int);\n",
                1,
                0,
            ),
        ] {
            let (g, _) = graph_with(path, |b| add_sql_file(b, path, sql).unwrap());
            assert_eq!(
                (
                    edges(&g, EdgeKind::Migrates).len(),
                    edges(&g, EdgeKind::Queries).len()
                ),
                (migrates, queries),
                "{path}"
            );
        }
    }

    #[test]
    fn unparsable_migrations_are_diagnostics_not_edges() {
        let sql = "CREATE TABLE t (id INT AUTO_INCREMENT PRIMARY KEY) ENGINE=InnoDB;";
        let (g, diagnostics) = graph_with("m.sql", |b| add_sql_file(b, "m.sql", sql).unwrap());
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
        let (g, diagnostics) = graph_with("V1.sql", |b| add_sql_file(b, "V1.sql", sql).unwrap());
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
            diagnostics = add_queries(
                b,
                "a.js",
                &facts,
                &keys("a.js", &facts),
                &GoConsts::default(),
            )
            .unwrap();
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
            add_queries(b, "R.java", &facts, &keys, &GoConsts::default()).unwrap()
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
            diagnostics =
                add_queries(b, "src/load.ts", &facts, &keys, &GoConsts::default()).unwrap();
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
            add_queries(b, "jobs/cleanup.py", &facts, &keys, &GoConsts::default()).unwrap()
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

    /// Contract M9b D6 (B10): `fmt.Sprintf` SQL folds `%s`/`%v` holes from package constants
    /// of sibling files, `%%` to `%`; every refusal keeps today's (unparsable) text, so it gives
    /// no edge. Each case is its own function: same-key edges would merge.
    #[test]
    fn sprintf_sql_folds_package_constants() {
        // `tStringer` has a named type whose `String()` method `%s` calls, so its literal is not
        // what Go prints (M9b pass 1, E3); `tTyped` is declared `string` and folds.
        let consts = "package repository\ntype Tbl string\nfunc (t Tbl) String() string { return \"stringer_\" + string(t) }\nconst (\n\tusersTable = \"users\"\n\tlistsTable = \"todo_lists\"\n\tdupTable = \"dup\"\n\tlocalTable = \"shadowed\"\n\ttStringer Tbl = \"t_typed\"\n\ttTyped string = \"t_typedstr\"\n)\nvar varTable = \"vars\"\n";
        // Row `H` refuses for its count (2 verbs, 1 value); row `P` refuses for `%d` alone, and
        // row `Q` for a local binding of `fmt` (M9b pass 1, E1).
        let use_go = "package repository\nimport (\n\t\"fmt\"\n\tother \"example.com/other\"\n)\nfunc A() { _ = fmt.Sprintf(\"SELECT id FROM %s WHERE id = $1\", usersTable) }\nfunc B() { _ = fmt.Sprintf(\"SELECT id FROM %v\", listsTable) }\nfunc C() { _ = fmt.Sprintf(\"SELECT id %% 2 FROM %s\", usersTable) }\nfunc D() {\n\tlocalTable := \"x\"\n\t_ = fmt.Sprintf(\"SELECT id FROM %s\", localTable)\n}\nfunc E() { _ = fmt.Sprintf(\"SELECT id FROM %s\", varTable) }\nfunc F() { _ = fmt.Sprintf(\"SELECT id FROM %s\", dupTable) }\nfunc G() { _ = fmt.Sprintf(\"SELECT id FROM %s\", testTable) }\nfunc H() { _ = fmt.Sprintf(\"SELECT id FROM %s WHERE note = '%d'\", usersTable) }\nfunc I() { _ = fmt.Sprintf(\"SELECT id FROM %[1]s\", usersTable) }\nfunc J() { _ = other.Sprintf(\"SELECT id FROM %s\", usersTable) }\nfunc K() { _ = fmt.Sprintf(\"SELECT id FROM %s\", tableName()) }\nfunc N() { _ = fmt.Sprintf(\"SELECT id FROM %s\", tStringer) }\nfunc O() { _ = fmt.Sprintf(\"SELECT id FROM %s\", tTyped) }\nfunc P() { _ = fmt.Sprintf(\"SELECT id FROM %d\", usersTable) }\nfunc Q() {\n\tfmt := struct{ Sprintf func(string, ...any) string }{}\n\t_ = fmt.Sprintf(\"SELECT id FROM %s\", usersTable)\n}\n";
        let files: Vec<(&str, &str)> = vec![
            ("repo/postgres.go", consts),
            // A second package-level declaration of `dupTable` (any kind).
            ("repo/dup.go", "package repository\nfunc dupTable() {}\n"),
            // A constant only a test file declares.
            (
                "repo/consts_test.go",
                "package repository\nconst testTable = \"tests\"\n",
            ),
            ("repo/use.go", use_go),
            // `fmt` is another module here.
            (
                "repo/alias.go",
                "package repository\nimport fmt \"example.com/notfmt\"\nfunc M() { _ = fmt.Sprintf(\"SELECT id FROM %s\", usersTable) }\n",
            ),
            // A same-file constant is the declaration itself, not a shadow.
            (
                "repo/same.go",
                "package repository\nimport \"fmt\"\nconst sameTable = \"same_t\"\nfunc S() { _ = fmt.Sprintf(\"SELECT id FROM %s\", sameTable) }\n",
            ),
            // Another package's constant of the same name is not visible.
            ("other/c.go", "package other\nconst otherTable = \"o\"\n"),
        ];
        let facts: Vec<(&str, FileFacts)> = files
            .iter()
            .map(|(p, src)| {
                (
                    *p,
                    rb_syntax::extract(Language::Go, src.as_bytes()).unwrap(),
                )
            })
            .collect();
        let by_path: BTreeMap<&str, (Language, &FileFacts)> =
            facts.iter().map(|(p, f)| (*p, (Language::Go, f))).collect();
        let go_consts = GoConsts::new(&by_path);
        let mut diagnostics = Vec::new();
        let mut b = GraphBuilder::new();
        for (path, f) in &facts {
            b.add_node(NodeKey::new(NodeKind::File, *path), *path, Attrs::new())
                .unwrap();
            let keys = keys(path, f);
            crate::syntax::add_symbols(&mut b, path, f, &keys).unwrap();
            diagnostics.extend(add_queries(&mut b, path, f, &keys, &go_consts).unwrap());
        }
        let g = b.finish().unwrap();
        let mut found: Vec<String> = edges(&g, EdgeKind::Queries)
            .into_iter()
            .map(|e| e.split(" @").next().unwrap().to_owned())
            .collect();
        found.sort();
        assert_eq!(
            found,
            [
                r#"A(). -> public.users ops=["read"] Heuristic assumed"#,
                r#"B(). -> public.todo_lists ops=["read"] Heuristic assumed"#,
                r#"C(). -> public.users ops=["read"] Heuristic assumed"#,
                r#"O(). -> public.t_typedstr ops=["read"] Heuristic assumed"#,
                r#"S(). -> public.same_t ops=["read"] Heuristic assumed"#,
            ]
        );
        // The verb reader `fold` applies: `%%` is one `%`; a missing or extra value refuses.
        assert_eq!(
            sprintf("SELECT id FROM %s WHERE name LIKE 'a%%'", &["users"]).as_deref(),
            Some("SELECT id FROM users WHERE name LIKE 'a%'")
        );
        assert_eq!(sprintf("SELECT id FROM %s", &[]), None);
        assert_eq!(sprintf("SELECT id FROM t", &["users"]), None);
        assert_eq!(sprintf("SELECT id FROM %5s", &["users"]), None);
        let folded: Vec<String> = diagnostics
            .iter()
            .filter(|d| d.kind == "sql_folded")
            .map(|d| format!("{} {}", d.path.as_deref().unwrap_or_default(), d.detail))
            .collect();
        assert_eq!(
            folded,
            [
                r#"repo/use.go {"constants":3,"sites":4}"#,
                r#"repo/same.go {"constants":1,"sites":1}"#,
            ]
        );
    }
}
