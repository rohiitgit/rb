//! ORM stage (design §7.1): framework packs' mappings become `MAPS_TO` edges (class → table) and
//! their data accesses become `QUERIES` edges (provenance `orm`). Mappings from every file are
//! resolved together first (JPA inheritance, Spring Data base repositories, Prisma schemas),
//! so an access in one file resolves a class declared in another.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use rb_frameworks::{Confidence, Inheritance, Mapping, Orm, TableName, Target, Unmapped};
use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance,
};
use rb_sql::{Op, TableUse};
use rb_syntax::{DefKind, FileFacts, Language};
use serde_json::json;

use crate::sql::Accesses;

/// What accesses resolve against, across the whole snapshot.
#[derive(Debug, Default)]
pub(crate) struct OrmIndex {
    /// JPA entity name (as JPQL names it) → its tables by key. Empty when the class is mapped
    /// but has no static table name.
    entities: BTreeMap<String, BTreeMap<String, TableUse>>,
    /// Mapped class simple name (as repository type arguments name it) → its tables by key.
    classes: BTreeMap<String, BTreeMap<String, TableUse>>,
    /// Prisma client accessor → (schema directory, table) per model with that accessor.
    prisma: BTreeMap<String, Vec<(String, TableUse)>>,
    /// Spring Data repository interfaces → index of their entity type parameter.
    repository_bases: BTreeMap<String, usize>,
}

/// A class mapping found in one file.
struct Found<'a> {
    path: &'a str,
    symbol: NodeKey,
    class_name: &'a str,
    span: rb_model::Span,
    mapping: Mapping,
}

/// Adds `MAPS_TO` for every mapped class and Prisma model of the snapshot, and returns the index
/// that accesses resolve against. Needs all files at once: JPA inheritance and Spring Data base
/// repositories cross files.
pub(crate) fn add_mappings(
    b: &mut GraphBuilder,
    files: &[(&str, Language, &FileFacts)],
    prisma: &[(&str, &str)],
) -> anyhow::Result<(OrmIndex, Vec<Diagnostic>)> {
    let mut index = OrmIndex {
        repository_bases: rb_frameworks::repository_bases(
            files
                .iter()
                .filter(|(_, l, _)| *l == Language::Java)
                .flat_map(|(_, _, f)| &f.definitions),
        ),
        ..OrmIndex::default()
    };
    let mut found = Vec::new();
    for &(path, language, facts) in files {
        let keys = rb_syntax::symbol_keys(path, facts);
        for mapping in rb_frameworks::mappings(path, language, facts) {
            let def = &facts.definitions[mapping.class];
            found.push(Found {
                path,
                symbol: NodeKey::new(NodeKind::Symbol, keys[mapping.class].clone()),
                class_name: &def.name,
                span: def.name_span,
                mapping,
            });
        }
    }
    let roots = single_table_roots(files, &found);
    let mut maps = MapsTo::default();
    let mut diagnostics = Vec::new();
    for (f, root) in found.iter().zip(&roots) {
        let owner = root.map_or(f, |r| &found[r]);
        let by_entity = index.entities.entry(f.mapping.entity.clone()).or_default();
        let by_class = index.classes.entry(f.class_name.to_owned()).or_default();
        let table = match &owner.mapping.table {
            Ok(table) => table_use(table),
            // Reported once, on the class that lacks the name.
            Err(reason) => {
                if root.is_none() {
                    let kind = match reason {
                        Unmapped::DynamicName => "orm_dynamic_table_name",
                        Unmapped::UnknownProxyParent => "orm_unresolved_proxy_parent",
                    };
                    diagnostics.push(Diagnostic {
                        kind: kind.into(),
                        path: Some(f.path.to_owned()),
                        span: Some(f.span),
                        detail: json!({ "class": f.class_name }),
                    });
                }
                continue;
            }
        };
        by_entity.insert(table.key(), table.clone());
        by_class.insert(table.key(), table.clone());
        let certainty = if owner.mapping.name_assumed || root.is_some() {
            Certainty::Synthetic
        } else {
            Certainty::Exact
        };
        maps.record(
            &f.symbol,
            &table,
            MapInfo {
                orm: orm_name(f.mapping.orm),
                certainty,
                name_assumed: owner.mapping.name_assumed,
                inherits: root.map(|_| owner.class_name.to_owned()),
            },
            Evidence {
                path: f.path.to_owned(),
                span: f.span,
            },
        );
    }
    for &(path, text) in prisma {
        add_prisma_models(b, path, text, &mut maps, &mut index)?;
    }
    maps.emit(b)?;
    Ok((index, diagnostics))
}

/// For each JPA mapping in a SINGLE_TABLE hierarchy below another entity, the index (into
/// `found`) of the topmost entity, whose table it shares. The walk goes through every Java class,
/// so `@MappedSuperclass` and plain classes in between do not end it. A superclass resolves by
/// simple name: the only class with that name, else the only one in the subclass's directory
/// (package); otherwise the walk ends.
fn single_table_roots(
    files: &[(&str, Language, &FileFacts)],
    found: &[Found<'_>],
) -> Vec<Option<usize>> {
    struct Class<'a> {
        dir: &'a str,
        name: &'a str,
        superclass: Option<&'a str>,
        entity: Option<usize>,
    }
    let entity_at: BTreeMap<(&str, usize), usize> = found
        .iter()
        .enumerate()
        .filter(|(_, f)| f.mapping.orm == Orm::Jpa)
        .map(|(i, f)| ((f.path, f.mapping.class), i))
        .collect();
    let mut classes = Vec::new();
    let mut of_entity = BTreeMap::new();
    for &(path, _, facts) in files.iter().filter(|(_, l, _)| *l == Language::Java) {
        for (i, def) in facts.definitions.iter().enumerate() {
            if def.kind != DefKind::Class {
                continue;
            }
            let entity = entity_at.get(&(path, i)).copied();
            if let Some(e) = entity {
                of_entity.insert(e, classes.len());
            }
            classes.push(Class {
                dir: path.rsplit_once('/').map_or("", |(d, _)| d),
                name: &def.name,
                superclass: def
                    .supertypes
                    .iter()
                    .find(|s| !s.implements)
                    .map(|s| simple(&s.name)),
                entity,
            });
        }
    }
    let mut by_name: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, c) in classes.iter().enumerate() {
        by_name.entry(c.name).or_default().push(i);
    }
    let parent = |c: usize| -> Option<usize> {
        let candidates = by_name.get(classes[c].superclass?)?;
        let same_dir: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&p| classes[p].dir == classes[c].dir)
            .collect();
        match (candidates.as_slice(), same_dir.as_slice()) {
            ([p], _) | (_, [p]) => Some(*p),
            _ => None,
        }
    };
    (0..found.len())
        .map(|i| {
            let start = *of_entity.get(&i)?;
            let (mut at, mut root) = (start, i);
            let mut seen = BTreeSet::from([start]);
            while let Some(p) = parent(at).filter(|p| seen.insert(*p)) {
                at = p;
                root = classes[p].entity.unwrap_or(root);
            }
            let own_tables = matches!(
                found[root].mapping.inheritance,
                Some(Inheritance::Joined | Inheritance::TablePerClass)
            );
            (root != i && !own_tables).then_some(root)
        })
        .collect()
}

fn simple(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// Adds a `Symbol` per Prisma `model` (defined in `schema.prisma`) with its `MAPS_TO`. Prisma's
/// default table name is the model name itself, so it is never an assumption.
fn add_prisma_models(
    b: &mut GraphBuilder,
    path: &str,
    text: &str,
    maps: &mut MapsTo,
    index: &mut OrmIndex,
) -> anyhow::Result<()> {
    let lines: Vec<&str> = text.lines().collect();
    let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
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
        let table = table_use(&model.table);
        let info = MapInfo {
            orm: "prisma",
            certainty: Certainty::Exact,
            name_assumed: false,
            inherits: None,
        };
        maps.record(&symbol, &table, info, at);
        index
            .prisma
            .entry(lower_first(&model.name))
            .or_default()
            .push((dir.to_owned(), table));
    }
    Ok(())
}

impl OrmIndex {
    /// Tables a Prisma accessor can mean in `path`: the models of the schemas whose directory
    /// shares the most leading path segments with the file. Ties are all candidates.
    fn prisma_tables(&self, accessor: &str, path: &str) -> Vec<&TableUse> {
        let Some(models) = self.prisma.get(accessor) else {
            return Vec::new();
        };
        let shared = |dir: &str| {
            dir.split('/')
                .zip(path.split('/'))
                .take_while(|(a, b)| !a.is_empty() && a == b)
                .count()
        };
        let best = models.iter().map(|(dir, _)| shared(dir)).max().unwrap_or(0);
        let tables: BTreeMap<String, &TableUse> = models
            .iter()
            .filter(|(dir, _)| shared(dir) == best)
            .map(|(_, t)| (t.key(), t))
            .collect();
        tables.into_values().collect()
    }
}

/// How one declaration maps to a table.
struct MapInfo {
    orm: &'static str,
    certainty: Certainty,
    name_assumed: bool,
    /// The SINGLE_TABLE root entity whose table this class shares.
    inherits: Option<String>,
}

/// `MAPS_TO` edges aggregated per (symbol, table): declarations sharing a symbol key (a class
/// defined in both branches of an `if`) merge instead of conflicting.
#[derive(Default)]
struct MapsTo(BTreeMap<(NodeKey, String), (TableUse, MapInfo, BTreeSet<Evidence>)>);

impl MapsTo {
    /// Best certainty wins; a name or schema stays assumed only if every declaration assumes it.
    fn record(&mut self, symbol: &NodeKey, table: &TableUse, info: MapInfo, at: Evidence) {
        match self.0.entry((symbol.clone(), table.key())) {
            Entry::Vacant(e) => {
                e.insert((table.clone(), info, BTreeSet::from([at])));
            }
            Entry::Occupied(mut e) => {
                let (kept_table, kept, evidence) = e.get_mut();
                kept_table.schema_assumed &= table.schema_assumed;
                kept.certainty = kept.certainty.max(info.certainty);
                kept.name_assumed &= info.name_assumed;
                evidence.insert(at);
            }
        }
    }

    fn emit(self, b: &mut GraphBuilder) -> anyhow::Result<()> {
        for ((symbol, table_key), (table, info, evidence)) in self.0 {
            let key = NodeKey::new(NodeKind::Table, table_key);
            if !b.has_node(&key) {
                b.add_node(key.clone(), &table.name, Attrs::new())?;
            }
            let mut attrs = object(json!({ "orm": info.orm }));
            if info.name_assumed {
                attrs.insert("name_assumed".into(), true.into());
            }
            if table.schema_assumed {
                attrs.insert("schema_assumed".into(), true.into());
            }
            if let Some(root) = info.inherits {
                attrs.insert("inherits".into(), root.into());
            }
            let mut edge = NewEdge::new(
                symbol,
                EdgeKind::MapsTo,
                key,
                Provenance::Orm,
                info.certainty,
            )
            .attrs(attrs);
            edge.evidence = evidence.into_iter().collect();
            b.add_edge(edge)?;
        }
        Ok(())
    }
}

fn orm_name(orm: Orm) -> &'static str {
    match orm {
        Orm::Jpa => "jpa",
        Orm::TypeOrm => "typeorm",
        Orm::SqlAlchemy => "sqlalchemy",
        Orm::Django => "django",
    }
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
    for access in rb_frameworks::accesses(language, facts, &index.repository_bases) {
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
            Target::Sql { text, literal } => {
                let analysis = rb_sql::analyze(text);
                if let Some(first) = analysis.errors.first() {
                    let mut d =
                        crate::sql::unparsed(path, Some(access.span), &first.error, Some("Query"));
                    d.detail["failed_statements"] = analysis.errors.len().into();
                    diagnostics.push(d);
                }
                // Text built from constants is partly unknown: its static tables are a guess, and
                // a name containing a `$n` hole is no table at all.
                let certainty = if *literal {
                    certainty
                } else {
                    Certainty::Heuristic
                };
                for table in analysis.statements.iter().flat_map(|s| &s.tables) {
                    if !*literal && (has_hole(&table.name) || has_hole(&table.schema)) {
                        diagnostics.push(Diagnostic {
                            kind: "orm_dynamic_table_name".into(),
                            path: Some(path.to_owned()),
                            span: Some(access.span),
                            detail: json!({ "query": text }),
                        });
                    } else {
                        accesses.record(&source, table, certainty, &at);
                    }
                }
            }
            Target::Entity(name) | Target::EntityClass(name) => {
                let known = match &access.target {
                    Target::Entity(_) => &index.entities,
                    _ => &index.classes,
                };
                match known.get(name) {
                    Some(tables) => {
                        // The same entity name declared twice: every candidate, none asserted.
                        let certainty = if tables.len() > 1 {
                            Certainty::Ambiguous
                        } else {
                            certainty
                        };
                        for table in tables.values() {
                            accesses.record(&source, &with_ops(table, &access.ops), certainty, &at);
                        }
                    }
                    None => diagnostics.push(Diagnostic {
                        kind: "orm_unresolved_entity".into(),
                        path: Some(path.to_owned()),
                        span: Some(access.span),
                        detail: json!({ "entity": name }),
                    }),
                }
            }
            // Other libraries have `findMany` too: an unknown accessor is simply not Prisma.
            Target::PrismaAccessor(accessor) => {
                let tables = index.prisma_tables(accessor, path);
                let certainty = if tables.len() > 1 {
                    Certainty::Ambiguous
                } else {
                    certainty
                };
                for table in tables {
                    accesses.record(&source, &with_ops(table, &access.ops), certainty, &at);
                }
            }
        }
    }
    accesses.emit(b, EdgeKind::Queries, Provenance::Orm)?;
    Ok(diagnostics)
}

/// Whether a name contains a template hole (`$1`), i.e. was glued to a non-literal value.
fn has_hole(name: &str) -> bool {
    name.split('$')
        .skip(1)
        .any(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
}

/// Physical names come from the framework packs, so case is kept as given.
fn table_use(table: &TableName) -> TableUse {
    TableUse {
        schema: table
            .schema
            .clone()
            .unwrap_or_else(|| rb_sql::DEFAULT_SCHEMA.to_owned()),
        name: table.name.clone(),
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
        for (path, _, f) in &facts {
            crate::syntax::add_symbols(&mut b, path, f).unwrap();
        }
        let files: Vec<(&str, Language, &FileFacts)> =
            facts.iter().map(|(p, l, f)| (p.as_str(), *l, f)).collect();
        let (index, mut diagnostics) = add_mappings(&mut b, &files, prisma).unwrap();
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

    #[test]
    fn a_class_declared_twice_maps_once() {
        let src = "if X:\n    class Tag(models.Model):\n        class Meta:\n            db_table = 'blog_tag'\nelse:\n    class Tag(models.Model):\n        name = 'x'\n";
        let (g, _) = build(&[("blog/models.py", Language::Python, src)], &[]);
        assert_eq!(
            edges(&g, EdgeKind::MapsTo),
            [r#"Tag# -> public.blog_tag [exact] {"orm":"django","schema_assumed":true}"#]
        );
        let maps_to = g.edges().iter().find(|e| e.kind == EdgeKind::MapsTo);
        assert_eq!(maps_to.unwrap().evidence.len(), 2);
    }

    #[test]
    fn prisma_models_resolve_to_the_nearest_schema() {
        let a = "model User {\n  id Int @id\n  @@map(\"a_users\")\n}\n";
        let b = "model User {\n  id Int @id\n  @@map(\"b_users\")\n}\n";
        let call = "export function f() { return prisma.user.findMany(); }\n";
        let (g, _) = build(
            &[
                ("apps/a/src/a.ts", Language::TypeScript, call),
                ("tools/t.ts", Language::TypeScript, call),
            ],
            &[
                ("apps/a/prisma/schema.prisma", a),
                ("apps/b/prisma/schema.prisma", b),
            ],
        );
        let queries = edges(&g, EdgeKind::Queries);
        assert_eq!(
            queries,
            [
                r#"f(). -> public.a_users [synthetic] {"ops":["read"],"schema_assumed":true}"#,
                // Outside every schema's directory: both candidates, neither asserted.
                r#"f(). -> public.a_users [ambiguous] {"ops":["read"],"schema_assumed":true}"#,
                r#"f(). -> public.b_users [ambiguous] {"ops":["read"],"schema_assumed":true}"#,
            ]
        );
    }

    const PEOPLE: &str = "@Entity @Table(name = \"people\") public class Person {}\n@Entity @Inheritance(strategy = InheritanceType.JOINED) public class Vehicle {}\n";
    const SUBCLASSES: &str = "@Entity public class Admin extends Person {}\n@Entity @Table(name = \"ignored\") public class Guest extends Person {}\n@Entity public class Car extends Vehicle {}\n";

    #[test]
    fn jpa_single_table_subclasses_use_the_root_table() {
        let repo = "interface AdminRepo extends JpaRepository<Admin, Long> {}\n";
        let (g, diagnostics) = build(
            &[
                ("src/Person.java", Language::Java, PEOPLE),
                ("src/Sub.java", Language::Java, SUBCLASSES),
                ("src/AdminRepo.java", Language::Java, repo),
            ],
            &[],
        );
        assert_eq!(
            edges(&g, EdgeKind::MapsTo),
            [
                r#"Person# -> public.people [exact] {"orm":"jpa","schema_assumed":true}"#,
                r#"Vehicle# -> public.vehicle [synthetic] {"name_assumed":true,"orm":"jpa","schema_assumed":true}"#,
                r#"Admin# -> public.people [synthetic] {"inherits":"Person","orm":"jpa","schema_assumed":true}"#,
                // JOINED: each class has its own table.
                r#"Car# -> public.car [synthetic] {"name_assumed":true,"orm":"jpa","schema_assumed":true}"#,
                // Hibernate ignores @Table on a SINGLE_TABLE subclass (HHH000139).
                r#"Guest# -> public.people [synthetic] {"inherits":"Person","orm":"jpa","schema_assumed":true}"#,
            ]
        );
        assert_eq!(
            edges(&g, EdgeKind::Queries),
            [
                r#"AdminRepo# -> public.people [synthetic] {"ops":["read","insert","update","delete"],"schema_assumed":true}"#
            ]
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn generic_base_repositories_across_files_dynamic_names_and_bad_native_sql() {
        let base = "interface BaseRepo<T> extends JpaRepository<T, Long> {}\n";
        let repo = "interface PersonRepo extends BaseRepo<Person> {\n  @Query(value = \"SELEC oops\", nativeQuery = true) void broken();\n}\n";
        let dynamic = "@Entity @Table(name = Tables.ACCOUNTS) public class Account {}\ninterface AccountRepo extends JpaRepository<Account, Long> {}\n";
        let (g, diagnostics) = build(
            &[
                ("src/Person.java", Language::Java, PEOPLE),
                ("src/BaseRepo.java", Language::Java, base),
                ("src/PersonRepo.java", Language::Java, repo),
                ("src/Account.java", Language::Java, dynamic),
            ],
            &[],
        );
        assert_eq!(
            edges(&g, EdgeKind::Queries),
            [
                r#"PersonRepo# -> public.people [synthetic] {"ops":["read","insert","update","delete"],"schema_assumed":true}"#
            ]
        );
        assert!(
            !edges(&g, EdgeKind::MapsTo)
                .iter()
                .any(|e| e.starts_with("Account#"))
        );
        let kinds: Vec<(&str, Option<&str>)> = diagnostics
            .iter()
            .map(|d| (d.kind.as_str(), d.path.as_deref()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("orm_dynamic_table_name", Some("src/Account.java")),
                ("unparsed_sql", Some("src/PersonRepo.java")),
            ]
        );
        assert_eq!(diagnostics[0].detail["class"], "Account");
        assert_eq!(diagnostics[1].detail["callee"], "Query");
    }

    #[test]
    fn inheritance_walks_through_mapped_superclasses_and_prefers_the_same_package() {
        let (g, _) = build(
            &[
                (
                    "src/zoo/Animal.java",
                    Language::Java,
                    "@Entity public class Animal {}\n@MappedSuperclass public class Mammal extends Animal {}\n@Entity public class Dog extends Mammal {}\n",
                ),
                (
                    "src/a/Base.java",
                    Language::Java,
                    "@MappedSuperclass public class Base {}\n",
                ),
                (
                    "src/b/Base.java",
                    Language::Java,
                    "@Entity @Table(name = \"b_base\") public class Base {}\n",
                ),
                (
                    "src/c/Widget.java",
                    Language::Java,
                    "@Entity @Table(name = \"widgets\") public class Widget extends Base {}\n",
                ),
                (
                    "src/b/Gadget.java",
                    Language::Java,
                    "@Entity public class Gadget extends Base {}\n",
                ),
            ],
            &[],
        );
        let maps = edges(&g, EdgeKind::MapsTo);
        let of = |class: &str| {
            maps.iter()
                .find(|e| e.starts_with(&format!("{class}# ")))
                .cloned()
                .unwrap_or_default()
        };
        assert!(of("Dog").contains("-> public.animal "), "{maps:?}");
        // Two `Base` classes and none in Widget's package: no inheritance is guessed.
        assert!(of("Widget").contains("-> public.widgets "), "{maps:?}");
        // Gadget's own package has the entity `Base`.
        assert!(of("Gadget").contains("-> public.b_base "), "{maps:?}");
    }

    #[test]
    fn repositories_bind_class_names_and_jpql_binds_entity_names() {
        let (g, diagnostics) = build(
            &[
                (
                    "src/User.java",
                    Language::Java,
                    "@Entity(name = \"Usr\") public class User {}\n",
                ),
                (
                    "src/UserRepo.java",
                    Language::Java,
                    "interface UserRepo extends JpaRepository<User, Long> {\n  List<User> findByName(String n);\n  @Query(\"SELECT u FROM Usr u\") List<User> all();\n}\n",
                ),
            ],
            &[],
        );
        assert_eq!(
            edges(&g, EdgeKind::Queries),
            [
                r#"UserRepo# -> public.usr [synthetic] {"ops":["read","insert","update","delete"],"schema_assumed":true}"#,
                r#"UserRepo#all(). -> public.usr [heuristic] {"ops":["read"],"schema_assumed":true}"#,
                r#"UserRepo#findByName(). -> public.usr [synthetic] {"ops":["read"],"schema_assumed":true}"#,
            ]
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }

    #[test]
    fn prisma_prefers_the_schema_sharing_the_longest_directory_prefix() {
        let root = "model User {\n  id Int @id\n  @@map(\"root_users\")\n}\n";
        let api = "model User {\n  id Int @id\n  @@map(\"api_users\")\n}\n";
        let call = "export function f() { return prisma.user.findMany(); }\n";
        let (g, _) = build(
            &[("apps/api/src/a.ts", Language::TypeScript, call)],
            &[("schema.prisma", root), ("apps/api/db/schema.prisma", api)],
        );
        assert_eq!(
            edges(&g, EdgeKind::Queries),
            [r#"f(). -> public.api_users [synthetic] {"ops":["read"],"schema_assumed":true}"#]
        );
    }

    #[test]
    fn a_proxy_of_an_unknown_parent_is_reported() {
        let src = "class P(Article):\n    class Meta:\n        proxy = True\n";
        let (g, diagnostics) = build(&[("shop/models.py", Language::Python, src)], &[]);
        assert!(edges(&g, EdgeKind::MapsTo).is_empty());
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        assert_eq!(diagnostics[0].kind, "orm_unresolved_proxy_parent");
        assert_eq!(diagnostics[0].detail["class"], "P");
    }

    #[test]
    fn an_implemented_interface_is_not_a_superclass() {
        let (g, _) = build(
            &[
                (
                    "x/Animal.java",
                    Language::Java,
                    "@Entity @Table(name = \"animals\") class Animal {}\n",
                ),
                (
                    "y/Dog.java",
                    Language::Java,
                    "interface Animal {}\n@Entity class Dog implements Animal {}\n",
                ),
            ],
            &[],
        );
        let maps = edges(&g, EdgeKind::MapsTo);
        assert!(
            maps.iter().any(|e| e.starts_with("Dog# -> public.dog ")),
            "{maps:?}"
        );
    }

    #[test]
    fn dynamic_native_queries_never_name_tables() {
        let repo = "interface R extends JpaRepository<Person, Long> {\n  @Query(value = \"SELECT * FROM things_\" + K.SUFFIX + \" JOIN people p ON true\", nativeQuery = true) List<Object> q();\n}\n";
        let (g, diagnostics) = build(
            &[
                ("src/Person.java", Language::Java, PEOPLE),
                ("src/R.java", Language::Java, repo),
            ],
            &[],
        );
        let queries = edges(&g, EdgeKind::Queries);
        assert!(!queries.iter().any(|e| e.contains('$')), "{queries:?}");
        // The static part still counts, at the certainty of a guess.
        assert!(
            queries
                .iter()
                .any(|e| e.starts_with("R#q(). -> public.people [heuristic]")),
            "{queries:?}"
        );
        let kinds: Vec<&str> = diagnostics.iter().map(|d| d.kind.as_str()).collect();
        assert_eq!(kinds, ["orm_dynamic_table_name"], "{diagnostics:?}");
    }

    #[test]
    fn dollar_names_in_static_native_sql_are_tables() {
        let repo = "interface R extends JpaRepository<Person, Long> {\n  @Query(value = \"SELECT * FROM pg$1tab\", nativeQuery = true) List<Object> q();\n}\n";
        let (g, diagnostics) = build(
            &[
                ("src/Person.java", Language::Java, PEOPLE),
                ("src/R.java", Language::Java, repo),
            ],
            &[],
        );
        let queries = edges(&g, EdgeKind::Queries);
        assert!(
            queries
                .iter()
                .any(|e| e.starts_with("R#q(). -> public.pg$1tab [exact]")),
            "{queries:?}"
        );
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
    }
}
