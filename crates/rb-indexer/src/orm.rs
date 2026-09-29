//! ORM stage (design §7.1): framework packs' mappings become `MAPS_TO` edges (class → table) and
//! their data accesses become `QUERIES` edges (provenance `orm`). Mappings from every file are
//! collected first, so a repository in one file resolves an entity declared in another.

use std::collections::{BTreeMap, BTreeSet};

use rb_frameworks::{Confidence, Orm, TableName, Target};
use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance,
};
use rb_sql::{Op, TableUse};
use rb_syntax::{FileFacts, Language};
use serde_json::json;

use crate::sql::Accesses;

/// Entity and Prisma model names → the tables they map to, across the whole snapshot.
#[derive(Debug, Default)]
pub(crate) struct OrmIndex {
    entities: BTreeMap<String, Vec<TableUse>>,
    prisma: BTreeMap<String, TableUse>,
}

/// Adds `MAPS_TO` for the classes of one file and records them in `index`.
pub(crate) fn add_mappings(
    b: &mut GraphBuilder,
    path: &str,
    language: Language,
    facts: &FileFacts,
    index: &mut OrmIndex,
) -> anyhow::Result<()> {
    let keys = rb_syntax::symbol_keys(path, facts);
    for m in rb_frameworks::mappings(path, language, facts) {
        // Hibernate emits unquoted identifiers, which Postgres folds to lowercase; the other ORMs
        // quote them, so their case is kept.
        let table = table_use(&m.table, m.orm == Orm::Jpa);
        let class = NodeKey::new(NodeKind::Symbol, keys[m.class].clone());
        let orm = match m.orm {
            Orm::Jpa => "jpa",
            Orm::TypeOrm => "typeorm",
            Orm::SqlAlchemy => "sqlalchemy",
            Orm::Django => "django",
        };
        let at = Evidence {
            path: path.to_owned(),
            span: facts.definitions[m.class].name_span,
        };
        map_to(b, class, &table, orm, m.name_assumed, at)?;
        index.entities.entry(m.entity).or_default().push(table);
    }
    Ok(())
}

/// Adds a `Symbol` per Prisma `model` (defined in `schema.prisma`) with its `MAPS_TO`. Prisma's
/// default table name is the model name itself, so it is never an assumption.
pub(crate) fn add_prisma_models(
    b: &mut GraphBuilder,
    path: &str,
    text: &str,
    index: &mut OrmIndex,
) -> anyhow::Result<()> {
    let lines: Vec<&str> = text.lines().collect();
    for model in rb_frameworks::prisma_models(text) {
        let symbol = NodeKey::new(
            NodeKind::Symbol,
            rb_syntax::type_symbol_key(path, &model.name),
        );
        let line = i32::try_from(model.line).unwrap_or(i32::MAX);
        let width = lines.get(model.line).map_or(0, |l| l.len());
        let at = Evidence {
            path: path.to_owned(),
            span: [line, 0, line, i32::try_from(width).unwrap_or(i32::MAX)],
        };
        b.add_node(
            symbol.clone(),
            &model.name,
            object(json!({"kind": "model"})),
        )?;
        b.add_edge(
            NewEdge::new(
                symbol.clone(),
                EdgeKind::DefinedIn,
                NodeKey::new(NodeKind::File, path),
                Provenance::Orm,
                Certainty::Exact,
            )
            .evidence(at.clone()),
        )?;
        let table = table_use(&model.table, false);
        map_to(b, symbol, &table, "prisma", false, at)?;
        index.prisma.insert(lower_first(&model.name), table);
    }
    Ok(())
}

/// Adds `QUERIES` (provenance `orm`) for the ORM accesses of one file.
pub(crate) fn add_accesses(
    b: &mut GraphBuilder,
    path: &str,
    language: Language,
    facts: &FileFacts,
    index: &OrmIndex,
) -> anyhow::Result<Vec<Diagnostic>> {
    let keys = rb_syntax::symbol_keys(path, facts);
    let mut accesses = Accesses::default();
    let mut diagnostics = Vec::new();
    for access in rb_frameworks::accesses(language, facts) {
        let source = match access.source {
            Some(i) => NodeKey::new(NodeKind::Symbol, keys[i].clone()),
            None => NodeKey::new(NodeKind::File, path),
        };
        let certainty = match access.confidence {
            Confidence::Stated => Certainty::Exact,
            Confidence::Convention => Certainty::Synthetic,
            Confidence::Parsed => Certainty::Heuristic,
        };
        let at = Evidence {
            path: path.to_owned(),
            span: access.span,
        };
        match &access.target {
            Target::Sql(text) => {
                for table in rb_sql::analyze(text)
                    .statements
                    .iter()
                    .flat_map(|s| &s.tables)
                {
                    accesses.record(&source, table, certainty, &at);
                }
            }
            Target::Entity(entity) => match index.entities.get(entity) {
                Some(tables) => {
                    // The same entity name declared twice: every candidate, none asserted.
                    let certainty = if tables.len() > 1 {
                        Certainty::Ambiguous
                    } else {
                        certainty
                    };
                    for table in tables {
                        accesses.record(&source, &with_ops(table, &access.ops), certainty, &at);
                    }
                }
                None => diagnostics.push(Diagnostic {
                    kind: "orm_unresolved_entity".into(),
                    path: Some(path.to_owned()),
                    span: Some(access.span),
                    detail: json!({ "entity": entity }),
                }),
            },
            // Other libraries have `findMany` too: an unknown accessor is simply not Prisma.
            Target::PrismaAccessor(accessor) => {
                if let Some(table) = index.prisma.get(accessor) {
                    accesses.record(&source, &with_ops(table, &access.ops), certainty, &at);
                }
            }
        }
    }
    accesses.emit(b, EdgeKind::Queries, Provenance::Orm)?;
    Ok(diagnostics)
}

fn map_to(
    b: &mut GraphBuilder,
    class: NodeKey,
    table: &TableUse,
    orm: &str,
    name_assumed: bool,
    at: Evidence,
) -> anyhow::Result<()> {
    let key = NodeKey::new(NodeKind::Table, table.key());
    if !b.has_node(&key) {
        b.add_node(key.clone(), &table.name, Attrs::new())?;
    }
    let mut attrs = object(json!({ "orm": orm }));
    if name_assumed {
        attrs.insert("name_assumed".into(), true.into());
    }
    if table.schema_assumed {
        attrs.insert("schema_assumed".into(), true.into());
    }
    let certainty = if name_assumed {
        Certainty::Synthetic
    } else {
        Certainty::Exact
    };
    b.add_edge(
        NewEdge::new(class, EdgeKind::MapsTo, key, Provenance::Orm, certainty)
            .attrs(attrs)
            .evidence(at),
    )
}

fn table_use(table: &TableName, fold_case: bool) -> TableUse {
    let fold = |s: &str| {
        if fold_case {
            s.to_lowercase()
        } else {
            s.to_owned()
        }
    };
    TableUse {
        schema: table
            .schema
            .as_deref()
            .map_or_else(|| rb_sql::DEFAULT_SCHEMA.to_owned(), fold),
        name: fold(&table.name),
        ops: BTreeSet::new(),
        schema_assumed: table.schema.is_none(),
    }
}

fn with_ops(table: &TableUse, ops: &BTreeSet<Op>) -> TableUse {
    TableUse {
        ops: ops.clone(),
        ..table.clone()
    }
}

/// Prisma client accessors are the model name with a lowercase first letter (`AuditLog` →
/// `auditLog`).
fn lower_first(name: &str) -> String {
    let mut chars = name.chars();
    chars
        .next()
        .map(|c| c.to_lowercase().chain(chars).collect())
        .unwrap_or_default()
}

fn object(v: serde_json::Value) -> Attrs {
    match v {
        serde_json::Value::Object(m) => m,
        _ => unreachable!("object() is only called with json!({{..}}) literals"),
    }
}

#[cfg(test)]
mod tests {
    use rb_model::{Attrs, EdgeKind, Graph, NodeKind};

    use super::*;

    /// Builds a graph from `(path, language, source)` files plus optional Prisma schemas.
    fn build(
        files: &[(&str, Language, &str)],
        prisma: &[(&str, &str)],
    ) -> (Graph, Vec<Diagnostic>) {
        let facts: Vec<(String, Language, FileFacts)> = files
            .iter()
            .map(|(p, l, src)| {
                (
                    (*p).to_owned(),
                    *l,
                    rb_syntax::extract(*l, src.as_bytes()).unwrap(),
                )
            })
            .collect();
        let mut b = GraphBuilder::new();
        for (p, _, _) in files
            .iter()
            .copied()
            .chain(prisma.iter().map(|(p, t)| (*p, Language::Go, *t)))
        {
            b.add_node(NodeKey::new(NodeKind::File, p), p, Attrs::new())
                .unwrap();
        }
        let mut index = OrmIndex::default();
        for (path, language, f) in &facts {
            crate::syntax::add_symbols(&mut b, path, f).unwrap();
            add_mappings(&mut b, path, *language, f, &mut index).unwrap();
        }
        for (path, text) in prisma {
            add_prisma_models(&mut b, path, text, &mut index).unwrap();
        }
        let mut diagnostics = Vec::new();
        for (path, language, f) in &facts {
            diagnostics.extend(add_accesses(&mut b, path, *language, f, &index).unwrap());
        }
        (b.finish().unwrap(), diagnostics)
    }

    /// `source -KIND-> table [certainty] attrs` with symbol keys shortened to their descriptors.
    fn edges(g: &Graph, kind: EdgeKind) -> Vec<String> {
        let key = |id: i64| {
            let k = g.nodes().iter().find(|n| n.id == id).unwrap().key.clone();
            k.rsplit_once("`/").map_or(k.clone(), |(_, d)| d.to_owned())
        };
        g.edges()
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| {
                format!(
                    "{} -> {} [{}] {}",
                    key(e.src),
                    key(e.dst),
                    e.certainty.as_str(),
                    serde_json::Value::Object(e.attrs.clone())
                )
            })
            .collect()
    }

    const ENTITY: &str = "@Entity @Table(name = \"Users\", schema = \"Auth\") public class User {}\n@Entity public class OrderLine {}\n";
    const REPO: &str = "interface UserRepository extends JpaRepository<User, Long> {\n  Optional<User> findByEmail(String e);\n  @Query(\"SELECT l FROM OrderLine l, Ghost g\") List<Object> weird();\n  @Query(value = \"DELETE FROM auth.sessions\", nativeQuery = true) void purge();\n}\n";

    #[test]
    fn jpa_mappings_resolve_across_files() {
        let (g, diagnostics) = build(
            &[
                ("src/User.java", Language::Java, ENTITY),
                ("src/UserRepository.java", Language::Java, REPO),
            ],
            &[],
        );
        assert_eq!(
            edges(&g, EdgeKind::MapsTo),
            [
                r#"OrderLine# -> public.order_line [synthetic] {"name_assumed":true,"orm":"jpa","schema_assumed":true}"#,
                // Hibernate emits unquoted names, which Postgres folds to lowercase.
                r#"User# -> auth.users [exact] {"orm":"jpa"}"#,
            ]
        );
        assert_eq!(
            edges(&g, EdgeKind::Queries),
            [
                r#"UserRepository# -> auth.users [synthetic] {"ops":["read","insert","update","delete"]}"#,
                r#"UserRepository#findByEmail(). -> auth.users [synthetic] {"ops":["read"]}"#,
                r#"UserRepository#purge(). -> auth.sessions [exact] {"ops":["delete"]}"#,
                r#"UserRepository#weird(). -> public.order_line [heuristic] {"ops":["read"],"schema_assumed":true}"#,
            ]
        );
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].kind, "orm_unresolved_entity");
        assert_eq!(diagnostics[0].detail["entity"], "Ghost");
    }

    #[test]
    fn entity_names_declared_twice_are_ambiguous() {
        let (g, _) = build(
            &[
                (
                    "a/User.java",
                    Language::Java,
                    "@Entity @Table(name = \"users\") class User {}\n",
                ),
                (
                    "b/User.java",
                    Language::Java,
                    "@Entity @Table(name = \"members\") class User {}\n",
                ),
                ("r.java", Language::Java, REPO.lines().next().unwrap()),
            ],
            &[],
        );
        let repo_edges: Vec<String> = edges(&g, EdgeKind::Queries);
        assert_eq!(repo_edges.len(), 2, "{repo_edges:?}");
        assert!(
            repo_edges.iter().all(|e| e.contains("[ambiguous]")),
            "{repo_edges:?}"
        );
    }

    const SCHEMA: &str =
        "model User {\n  id Int @id\n  @@map(\"users\")\n}\n\nmodel AuditLog {\n  id Int @id\n}\n";
    const CLIENT: &str = "export async function load(id: number) {\n  await prisma.user.findUnique({ where: { id } });\n  await prisma.auditLog.create({ data: {} });\n  await prisma.ghost.findMany();\n}\n";

    #[test]
    fn prisma_models_and_client_calls() {
        let (g, diagnostics) = build(
            &[("src/load.ts", Language::TypeScript, CLIENT)],
            &[("prisma/schema.prisma", SCHEMA)],
        );
        assert_eq!(
            edges(&g, EdgeKind::MapsTo),
            [
                r#"AuditLog# -> public.AuditLog [exact] {"orm":"prisma","schema_assumed":true}"#,
                r#"User# -> public.users [exact] {"orm":"prisma","schema_assumed":true}"#,
            ]
        );
        assert_eq!(
            edges(&g, EdgeKind::Queries),
            [
                r#"load(). -> public.AuditLog [synthetic] {"ops":["insert"],"schema_assumed":true}"#,
                r#"load(). -> public.users [synthetic] {"ops":["read"],"schema_assumed":true}"#,
            ]
        );
        // `prisma.ghost` matches no model: other libraries also have `findMany`, so no diagnostic.
        assert!(diagnostics.is_empty());
        let model = g
            .nodes()
            .iter()
            .find(|n| n.name == "User" && n.kind == NodeKind::Symbol)
            .unwrap();
        assert!(model.key.ends_with("`prisma/schema.prisma`/User#"));
    }
}
