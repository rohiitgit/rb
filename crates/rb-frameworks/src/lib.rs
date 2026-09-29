//! Framework packs (design §4.5): pure functions that read framework-neutral syntax facts
//! (decorations, supertypes, class constants, call qualifiers, SQL candidates) and report which
//! classes map to tables and which code accesses them through an ORM.
//!
//! Packs never touch the graph; the indexer resolves entity and model names across files and
//! turns the results into `MAPS_TO` and `QUERIES` edges.

use std::collections::BTreeSet;

use rb_sql::Op;
use rb_syntax::{Definition, FileFacts, Language, RefKind, Span};

/// A table name as a framework states or implies it; `schema: None` means the database default.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TableName {
    pub schema: Option<String>,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Orm {
    Jpa,
    TypeOrm,
    SqlAlchemy,
    Django,
}

/// A class mapped to a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mapping {
    /// Index into `FileFacts::definitions`.
    pub class: usize,
    pub orm: Orm,
    /// The name queries use for it (JPA `@Entity(name)` or the class name).
    pub entity: String,
    pub table: TableName,
    /// The table name comes from a naming rule, not from the code; projects can override it.
    pub name_assumed: bool,
}

/// What an ORM access refers to, before cross-file resolution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// A JPA entity by name (resolved through `Mapping::entity`).
    Entity(String),
    /// A Prisma client model accessor, e.g. `user` in `prisma.user.findMany()`.
    PrismaAccessor(String),
    /// Native SQL, to be analysed by `rb-sql`.
    Sql(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// Stated in code (native `@Query`): the edge is exact.
    Stated,
    /// Implied by a framework convention (derived queries, inherited CRUD, client methods).
    Convention,
    /// Parsed from a non-SQL query language (JPQL).
    Parsed,
}

/// Data access through an ORM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Access {
    /// Index into `FileFacts::definitions`; `None` for code outside every definition.
    pub source: Option<usize>,
    pub target: Target,
    pub ops: BTreeSet<Op>,
    pub confidence: Confidence,
    pub span: Span,
}

/// A `model` block of a Prisma schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrismaModel {
    pub name: String,
    pub table: TableName,
    /// 0-based line of the `model` keyword.
    pub line: usize,
}

/// Supertypes that make an interface a Spring Data repository of its first type argument.
const REPOSITORY_BASES: &[&str] = &[
    "Repository",
    "CrudRepository",
    "ListCrudRepository",
    "PagingAndSortingRepository",
    "ListPagingAndSortingRepository",
    "JpaRepository",
];

/// Spring Data derived-query subjects and the operation each performs.
const DERIVED_QUERY_PREFIXES: &[(&str, Op)] = &[
    ("find", Op::Read),
    ("read", Op::Read),
    ("get", Op::Read),
    ("query", Op::Read),
    ("search", Op::Read),
    ("stream", Op::Read),
    ("count", Op::Read),
    ("exists", Op::Read),
    ("delete", Op::Delete),
    ("remove", Op::Delete),
];

/// The last `.`-separated segment of a possibly qualified name.
fn simple(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

fn decoration<'f>(def: &'f Definition, name: &str) -> Option<&'f rb_syntax::Decoration> {
    def.decorations.iter().find(|d| simple(&d.name) == name)
}

fn arg<'f>(decoration: &'f rb_syntax::Decoration, key: &str) -> Option<&'f str> {
    decoration
        .args
        .iter()
        .find(|a| a.key.as_deref() == Some(key))
        .map(|a| a.value.as_str())
}

fn positional(decoration: &rb_syntax::Decoration) -> Option<&str> {
    decoration
        .args
        .iter()
        .find(|a| a.key.is_none())
        .map(|a| a.value.as_str())
}

/// Classes in one file that map to tables.
pub fn mappings(path: &str, language: Language, facts: &FileFacts) -> Vec<Mapping> {
    let defs = &facts.definitions;
    let mut out = Vec::new();
    for (class, def) in defs.iter().enumerate() {
        let mapping = match language {
            Language::Java => jpa_mapping(def),
            Language::TypeScript | Language::Tsx | Language::JavaScript => typeorm_mapping(def),
            Language::Python => python_mapping(path, class, defs),
            Language::Go => None,
        };
        if let Some((orm, entity, table, name_assumed)) = mapping {
            out.push(Mapping {
                class,
                orm,
                entity,
                table,
                name_assumed,
            });
        }
    }
    out
}

type Mapped = (Orm, String, TableName, bool);

/// JPA: `@Entity(name?)` plus optional `@Table(name, schema)`. Without a table name the spec's
/// default is the entity name, which Spring Boot's physical naming turns into snake_case.
fn jpa_mapping(def: &Definition) -> Option<Mapped> {
    let entity_decoration = decoration(def, "Entity")?;
    let entity = arg(entity_decoration, "name")
        .unwrap_or(&def.name)
        .to_owned();
    let table = decoration(def, "Table");
    let explicit = table.and_then(|t| arg(t, "name")).filter(|n| !n.is_empty());
    let name = explicit.map_or_else(|| snake_case(&entity), str::to_owned);
    let schema = table
        .and_then(|t| arg(t, "schema"))
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    Some((
        Orm::Jpa,
        entity,
        TableName { schema, name },
        explicit.is_none(),
    ))
}

/// TypeORM: `@Entity('name')` or `@Entity({ name, schema })`; default is snake_case(class).
fn typeorm_mapping(def: &Definition) -> Option<Mapped> {
    let entity = decoration(def, "Entity")?;
    let explicit = positional(entity)
        .or_else(|| arg(entity, "name"))
        .filter(|n| !n.is_empty());
    let name = explicit.map_or_else(|| snake_case(&def.name), str::to_owned);
    let schema = arg(entity, "schema").map(str::to_owned);
    Some((
        Orm::TypeOrm,
        def.name.clone(),
        TableName { schema, name },
        explicit.is_none(),
    ))
}

/// SQLAlchemy `__tablename__`, else a Django model (`models.Model` subclass): `Meta.db_table`, or
/// Django's `<app_label>_<model>` with the app label taken from the directory (an assumption).
fn python_mapping(path: &str, class: usize, defs: &[Definition]) -> Option<Mapped> {
    let def = &defs[class];
    if let Some(t) = def
        .class_constants
        .iter()
        .find(|c| c.name == "__tablename__")
    {
        let table = TableName {
            schema: None,
            name: t.value.clone(),
        };
        return Some((Orm::SqlAlchemy, def.name.clone(), table, false));
    }
    if !def.supertypes.iter().any(|s| simple(&s.name) == "Model") {
        return None;
    }
    let meta = defs
        .iter()
        .filter(|d| d.parent == Some(class) && d.name == "Meta")
        .flat_map(|d| &d.class_constants)
        .find(|c| c.name == "db_table");
    let (name, assumed) = match meta {
        Some(t) => (t.value.clone(), false),
        None => (
            format!("{}_{}", django_app_label(path)?, def.name.to_lowercase()),
            true,
        ),
    };
    Some((
        Orm::Django,
        def.name.clone(),
        TableName { schema: None, name },
        assumed,
    ))
}

/// `blog/models.py` → `blog`; `blog/models/article.py` → `blog`.
fn django_app_label(path: &str) -> Option<&str> {
    let mut dirs = path.rsplit('/').skip(1);
    let first = dirs.next()?;
    if first == "models" {
        dirs.next()
    } else {
        Some(first)
    }
}

/// ORM data access in one file.
pub fn accesses(language: Language, facts: &FileFacts) -> Vec<Access> {
    let mut out = Vec::new();
    match language {
        Language::Java => {
            spring_data_accesses(facts, &mut out);
            for candidate in facts.sql.iter().filter(|c| {
                c.callee
                    .as_deref()
                    .is_some_and(|n| n.eq_ignore_ascii_case("createQuery"))
            }) {
                out.extend(jpql_accesses(
                    &candidate.text,
                    candidate.enclosing,
                    candidate.span,
                ));
            }
        }
        Language::TypeScript | Language::Tsx | Language::JavaScript => {
            prisma_accesses(facts, &mut out)
        }
        Language::Python | Language::Go => {}
    }
    out
}

fn spring_data_accesses(facts: &FileFacts, out: &mut Vec<Access>) {
    let defs = &facts.definitions;
    for (repo, def) in defs.iter().enumerate() {
        let Some(entity) = def
            .supertypes
            .iter()
            .find(|s| REPOSITORY_BASES.contains(&simple(&s.name)))
            .and_then(|s| s.type_args.first())
            .map(|e| simple(e).to_owned())
        else {
            continue;
        };
        // Inherited CRUD methods (save, findById, delete…) are not declared: the repository
        // itself carries them.
        out.push(Access {
            source: Some(repo),
            target: Target::Entity(entity.clone()),
            ops: [Op::Read, Op::Insert, Op::Update, Op::Delete]
                .into_iter()
                .collect(),
            confidence: Confidence::Convention,
            span: def.name_span,
        });
        for (method, m) in defs
            .iter()
            .enumerate()
            .filter(|(_, d)| d.parent == Some(repo))
        {
            if let Some(query) = decoration(m, "Query") {
                let text = arg(query, "value")
                    .or_else(|| positional(query))
                    .unwrap_or_default();
                if arg(query, "nativeQuery") == Some("true") {
                    out.push(Access {
                        source: Some(method),
                        target: Target::Sql(text.to_owned()),
                        ops: BTreeSet::new(),
                        confidence: Confidence::Stated,
                        span: m.name_span,
                    });
                } else {
                    out.extend(jpql_accesses(text, Some(method), m.name_span));
                }
                continue;
            }
            let derived = DERIVED_QUERY_PREFIXES.iter().find(|(prefix, _)| {
                m.name.strip_prefix(prefix).is_some_and(|rest| {
                    rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_uppercase())
                })
            });
            if let Some((_, op)) = derived {
                out.push(Access {
                    source: Some(method),
                    target: Target::Entity(entity.clone()),
                    ops: [*op].into_iter().collect(),
                    confidence: Confidence::Convention,
                    span: m.name_span,
                });
            }
        }
    }
}

fn jpql_accesses(text: &str, source: Option<usize>, span: Span) -> Vec<Access> {
    jpql(text)
        .into_iter()
        .map(|(entity, op)| Access {
            source,
            target: Target::Entity(entity),
            ops: [op].into_iter().collect(),
            confidence: Confidence::Parsed,
            span,
        })
        .collect()
}

fn prisma_accesses(facts: &FileFacts, out: &mut Vec<Access>) {
    for r in facts.references.iter().filter(|r| r.kind == RefKind::Call) {
        let (Some(qualifier), Some(ops)) = (&r.qualifier, prisma_ops(&r.name)) else {
            continue;
        };
        // `<client>.<model>.<method>()`: the model accessor is the qualifier's last segment.
        let Some((_, accessor)) = qualifier.rsplit_once('.') else {
            continue;
        };
        out.push(Access {
            source: r.enclosing,
            target: Target::PrismaAccessor(accessor.to_owned()),
            ops,
            confidence: Confidence::Convention,
            span: r.span,
        });
    }
}

/// Keywords that end a FROM item (so the next word is not an alias).
const JPQL_CLAUSE_WORDS: &[&str] = &[
    "WHERE", "JOIN", "INNER", "LEFT", "RIGHT", "OUTER", "FETCH", "GROUP", "ORDER", "HAVING", "ON",
    "UNION", "SET", "AS",
];

/// Entities named by a JPQL statement, with the operation on each, sorted. Association paths
/// (`JOIN o.lines l`) are not entities; qualified class names count by their simple name.
pub fn jpql(text: &str) -> Vec<(String, Op)> {
    let words = jpql_words(text);
    let upper: Vec<String> = words.iter().map(|w| w.to_ascii_uppercase()).collect();
    let mut out: BTreeSet<(String, Op)> = BTreeSet::new();
    let mut target_from = None; // index of a FROM that names the DELETE target
    match upper.first().map(String::as_str) {
        Some("UPDATE") => {
            if let Some(entity) = words.get(1) {
                out.insert((simple(entity).to_owned(), Op::Update));
            }
        }
        Some("DELETE") => {
            let at = if upper.get(1).map(String::as_str) == Some("FROM") {
                target_from = Some(1);
                2
            } else {
                1
            };
            if let Some(entity) = words.get(at) {
                out.insert((simple(entity).to_owned(), Op::Delete));
            }
        }
        Some("INSERT") => {
            if let Some(entity) = upper
                .iter()
                .position(|w| w == "INTO")
                .and_then(|i| words.get(i + 1))
            {
                out.insert((simple(entity).to_owned(), Op::Insert));
            }
        }
        _ => {}
    }
    for (i, word) in upper.iter().enumerate() {
        if word == "FROM" && Some(i) != target_from {
            from_items(&words, &upper, i + 1, &mut out);
        } else if word == "JOIN" {
            let at = if upper.get(i + 1).map(String::as_str) == Some("FETCH") {
                i + 2
            } else {
                i + 1
            };
            if let Some(entity) = words
                .get(at)
                .filter(|w| is_identifier(w) && !w.contains('.'))
            {
                out.insert((simple(entity).to_owned(), Op::Read));
            }
            // `JOIN o.lines l, Customer c`: a comma after the join continues the FROM list.
            let after = skip_alias(&words, &upper, at + 1);
            if words.get(after).map(String::as_str) == Some(",") {
                from_items(&words, &upper, after + 1, &mut out);
            }
        }
    }
    out.into_iter().collect()
}

/// Reads `Entity [AS] alias, Entity [AS] alias, …` starting at `j`.
fn from_items(words: &[String], upper: &[String], mut j: usize, out: &mut BTreeSet<(String, Op)>) {
    while let Some(entity) = words.get(j).filter(|w| is_identifier(w)) {
        out.insert((simple(entity).to_owned(), Op::Read));
        j = skip_alias(words, upper, j + 1);
        if words.get(j).map(String::as_str) != Some(",") {
            break;
        }
        j += 1;
    }
}

/// Index after an optional `[AS] alias` starting at `j`.
fn skip_alias(words: &[String], upper: &[String], mut j: usize) -> usize {
    if upper.get(j).map(String::as_str) == Some("AS") {
        j += 1;
    }
    let is_alias = words.get(j).is_some_and(|w| is_identifier(w))
        && !JPQL_CLAUSE_WORDS.contains(&upper[j].as_str());
    if is_alias { j + 1 } else { j }
}

fn is_identifier(word: &str) -> bool {
    word.starts_with(|c: char| c.is_alphabetic() || c == '_')
}

/// JPQL words: identifiers (with dots), and `(`, `)` and `,` as their own words. String literals
/// and named parameters are skipped.
fn jpql_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            for d in chars.by_ref() {
                if d == '\'' {
                    break;
                }
            }
        } else if matches!(c, '(' | ')' | ',') {
            words.push(c.to_string());
        } else if c.is_alphanumeric() || c == '_' || c == ':' {
            let mut word = c.to_string();
            while let Some(&d) = chars
                .peek()
                .filter(|d| d.is_alphanumeric() || matches!(d, '_' | '.' | '$'))
            {
                word.push(d);
                chars.next();
            }
            if !word.starts_with(':') {
                words.push(word);
            }
        }
    }
    words
}

/// The `model` blocks of a `schema.prisma` file. Prisma's default table name is the model name.
pub fn prisma_models(text: &str) -> Vec<PrismaModel> {
    let mut out = Vec::new();
    let mut current: Option<PrismaModel> = None;
    for (line_no, raw) in text.lines().enumerate() {
        let line = raw.split("//").next().unwrap_or_default().trim();
        if let Some(model) = &mut current {
            if line == "}" {
                out.extend(current.take());
            } else if let Some(name) = attribute_string(line, "@@map(") {
                model.table.name = name;
            } else if let Some(schema) = attribute_string(line, "@@schema(") {
                model.table.schema = Some(schema);
            }
        } else if let Some(rest) = line.strip_prefix("model ") {
            let name = rest.trim_end_matches('{').trim();
            if !name.is_empty() && line.ends_with('{') {
                current = Some(PrismaModel {
                    name: name.to_owned(),
                    table: TableName {
                        schema: None,
                        name: name.to_owned(),
                    },
                    line: line_no,
                });
            }
        }
    }
    out
}

/// The first string argument of a Prisma block attribute such as `@@map("users")`.
fn attribute_string(line: &str, attribute: &str) -> Option<String> {
    let rest = line.strip_prefix(attribute)?;
    let rest = rest.trim_start().strip_prefix('"')?;
    Some(rest.split('"').next()?.to_owned())
}

/// Operations a Prisma client method performs, if it is one.
pub fn prisma_ops(method: &str) -> Option<BTreeSet<Op>> {
    let ops: &[Op] = match method {
        "findUnique" | "findUniqueOrThrow" | "findFirst" | "findFirstOrThrow" | "findMany"
        | "count" | "aggregate" | "groupBy" => &[Op::Read],
        "create" | "createMany" | "createManyAndReturn" => &[Op::Insert],
        "update" | "updateMany" | "updateManyAndReturn" => &[Op::Update],
        "upsert" => &[Op::Insert, Op::Update],
        "delete" | "deleteMany" => &[Op::Delete],
        _ => return None,
    };
    Some(ops.iter().copied().collect())
}

/// `snakeCase` as TypeORM's `DefaultNamingStrategy` and Hibernate's
/// `PhysicalNamingStrategySnakeCaseImpl` (Spring Boot's default) apply it:
/// `ABc → A_Bc`, `aC → a_C`, then lowercase.
pub fn snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let next = chars.get(i + 1);
        let boundary = c.is_ascii_uppercase()
            && prev.is_some_and(|p| {
                p.is_ascii_lowercase()
                    || p.is_ascii_digit()
                    || (p.is_ascii_uppercase() && next.is_some_and(char::is_ascii_lowercase))
            });
        if boundary {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(xs: &[Op]) -> BTreeSet<Op> {
        xs.iter().copied().collect()
    }

    fn facts(language: Language, src: &str) -> FileFacts {
        rb_syntax::extract(language, src.as_bytes()).unwrap()
    }

    fn table(schema: Option<&str>, name: &str) -> TableName {
        TableName {
            schema: schema.map(Into::into),
            name: name.into(),
        }
    }

    #[test]
    fn snake_case_matches_typeorm_and_hibernate() {
        for (input, expected) in [
            ("User", "user"),
            ("UserAccount", "user_account"),
            ("HTTPServer", "http_server"),
            ("APIKey", "api_key"),
            ("order2Item", "order2_item"),
        ] {
            assert_eq!(snake_case(input), expected, "{input}");
        }
    }

    const JAVA: &str = r#"package com.acme;
@Entity
@Table(name = "users", schema = "auth")
public class User {}
@Entity(name = "Acct")
public class UserAccount {}
@Entity public class Order {}
interface UserRepository extends JpaRepository<User, Long> {
    Optional<User> findByEmail(String email);
    long countByActive(boolean active);
    void deleteByEmail(String email);
    @Query(value = "SELECT * FROM auth.users WHERE email LIKE ?1", nativeQuery = true)
    List<User> search(String q);
    @Modifying @Query("UPDATE User u SET u.active = false WHERE u.id = :id")
    void deactivate(long id);
    List<User> helper();
}
class Service { void f() { em.createQuery("SELECT o FROM Order o JOIN o.lines l"); } }
"#;

    #[test]
    fn jpa_entities_and_naming() {
        let f = facts(Language::Java, JAVA);
        let got: Vec<(String, String, TableName, bool)> =
            mappings("src/User.java", Language::Java, &f)
                .into_iter()
                .map(|m| {
                    (
                        f.definitions[m.class].name.clone(),
                        m.entity,
                        m.table,
                        m.name_assumed,
                    )
                })
                .collect();
        assert_eq!(
            got,
            [
                (
                    "User".into(),
                    "User".into(),
                    table(Some("auth"), "users"),
                    false
                ),
                // JPA: the default table name is the *entity* name (here set to "Acct").
                (
                    "UserAccount".into(),
                    "Acct".into(),
                    table(None, "acct"),
                    true
                ),
                ("Order".into(), "Order".into(), table(None, "order"), true),
            ]
        );
    }

    #[test]
    fn spring_data_repositories_query_methods_and_jpql() {
        let f = facts(Language::Java, JAVA);
        let got: Vec<(String, Target, BTreeSet<Op>, Confidence)> = accesses(Language::Java, &f)
            .into_iter()
            .map(|a| {
                let source = a
                    .source
                    .map_or("-".to_owned(), |i| f.definitions[i].name.clone());
                (source, a.target, a.ops, a.confidence)
            })
            .collect();
        let user = || Target::Entity("User".into());
        assert_eq!(
            got,
            [
                (
                    "UserRepository".into(),
                    user(),
                    ops(&[Op::Read, Op::Insert, Op::Update, Op::Delete]),
                    Confidence::Convention
                ),
                (
                    "findByEmail".into(),
                    user(),
                    ops(&[Op::Read]),
                    Confidence::Convention
                ),
                (
                    "countByActive".into(),
                    user(),
                    ops(&[Op::Read]),
                    Confidence::Convention
                ),
                (
                    "deleteByEmail".into(),
                    user(),
                    ops(&[Op::Delete]),
                    Confidence::Convention
                ),
                (
                    "search".into(),
                    Target::Sql("SELECT * FROM auth.users WHERE email LIKE ?1".into()),
                    ops(&[]),
                    Confidence::Stated
                ),
                (
                    "deactivate".into(),
                    user(),
                    ops(&[Op::Update]),
                    Confidence::Parsed
                ),
                (
                    "f".into(),
                    Target::Entity("Order".into()),
                    ops(&[Op::Read]),
                    Confidence::Parsed
                ),
            ]
        );
    }

    #[test]
    fn jpql_entities_and_operations() {
        let e = |name: &str, op: Op| (name.to_owned(), op);
        assert_eq!(
            jpql("SELECT u FROM User u WHERE u.email = :e"),
            [e("User", Op::Read)]
        );
        assert_eq!(
            jpql("select o from Order o join fetch o.lines l, com.acme.Customer c where o.c = c"),
            [e("Customer", Op::Read), e("Order", Op::Read)]
        );
        assert_eq!(
            jpql(
                "DELETE FROM Session s WHERE s.user IN (SELECT u FROM User u WHERE u.active = false)"
            ),
            [e("Session", Op::Delete), e("User", Op::Read)]
        );
        assert_eq!(
            jpql("UPDATE User u SET u.active = false"),
            [e("User", Op::Update)]
        );
        assert_eq!(jpql("FROM Event"), [e("Event", Op::Read)]);
    }

    const PRISMA: &str = r#"datasource db {
  provider = "postgresql"
}

// A user of the system
model User {
  id    Int    @id
  email String @unique // "model Fake {" in a comment is ignored
  @@map("users")
  @@schema("auth")
}

model Post {
  id Int @id
}
"#;

    #[test]
    fn prisma_schema_models() {
        let got: Vec<(String, TableName, usize)> = prisma_models(PRISMA)
            .into_iter()
            .map(|m| (m.name, m.table, m.line))
            .collect();
        assert_eq!(
            got,
            [
                ("User".into(), table(Some("auth"), "users"), 5),
                ("Post".into(), table(None, "Post"), 12),
            ]
        );
    }

    #[test]
    fn prisma_client_calls() {
        let src = "export async function load(id: number) {\n  const u = await prisma.user.findUnique({ where: { id } });\n  await this.db.post.upsert({});\n  await tx.post.deleteMany();\n  list.find();\n}\n";
        let f = facts(Language::TypeScript, src);
        let got: Vec<(Target, BTreeSet<Op>)> = accesses(Language::TypeScript, &f)
            .into_iter()
            .map(|a| (a.target, a.ops))
            .collect();
        let accessor = |a: &str| Target::PrismaAccessor(a.into());
        assert_eq!(
            got,
            [
                (accessor("user"), ops(&[Op::Read])),
                (accessor("post"), ops(&[Op::Insert, Op::Update])),
                (accessor("post"), ops(&[Op::Delete])),
            ]
        );
        assert_eq!(prisma_ops("findManyAndCount"), None);
    }

    #[test]
    fn typeorm_sqlalchemy_and_django() {
        let ts = "@Entity('orders') export class Order {}\n@Entity({ name: 'items', schema: 'shop' }) export class Item {}\n@Entity() export class LineItem {}\n";
        let f = facts(Language::TypeScript, ts);
        let got: Vec<(TableName, bool)> = mappings("src/entities.ts", Language::TypeScript, &f)
            .into_iter()
            .map(|m| (m.table, m.name_assumed))
            .collect();
        assert_eq!(
            got,
            [
                (table(None, "orders"), false),
                (table(Some("shop"), "items"), false),
                (table(None, "line_item"), true)
            ]
        );

        let py = "class User(Base):\n    __tablename__ = 'users'\n\nclass Article(models.Model):\n    title = 'x'\n\nclass Tag(models.Model):\n    class Meta:\n        db_table = 'blog_tags'\n\nclass Helper:\n    pass\n";
        let f = facts(Language::Python, py);
        let got: Vec<(Orm, String, TableName, bool)> =
            mappings("blog/models.py", Language::Python, &f)
                .into_iter()
                .map(|m| {
                    (
                        m.orm,
                        f.definitions[m.class].name.clone(),
                        m.table,
                        m.name_assumed,
                    )
                })
                .collect();
        assert_eq!(
            got,
            [
                (Orm::SqlAlchemy, "User".into(), table(None, "users"), false),
                (
                    Orm::Django,
                    "Article".into(),
                    table(None, "blog_article"),
                    true
                ),
                (Orm::Django, "Tag".into(), table(None, "blog_tags"), false),
            ]
        );
    }
}
