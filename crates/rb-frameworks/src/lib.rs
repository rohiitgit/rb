//! Framework packs (design §4.5): pure functions that read framework-neutral syntax facts
//! (decorations, supertypes, class constants, call qualifiers, SQL candidates) and report which
//! classes map to tables and which code accesses them through an ORM.
//!
//! Packs never touch the graph; the indexer resolves entity and model names across files and
//! turns the results into `MAPS_TO` and `QUERIES` edges.

pub mod clients;
pub mod endpoint;
pub mod fold;
pub mod routes;

/// Operation counts for the growth tests (contract M4a invariant 11; amendment 12, Q4): the
/// measured loops call [`work::tick`] with the elements they examine, and a test compares the
/// counts for an input and one four times as large, which no machine load can change. The
/// counter exists only in test builds (this crate's, or a dependant's through the `work-count`
/// feature its dev-dependency enables); elsewhere `tick` is empty.
pub mod work {
    #[cfg(any(test, feature = "work-count"))]
    thread_local! {
        static COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    /// Counts `n` elements examined by a measured loop.
    #[inline(always)]
    pub fn tick(n: usize) {
        #[cfg(any(test, feature = "work-count"))]
        COUNT.with(|c| c.set(c.get() + n as u64));
        let _ = n;
    }

    /// The count since the last call, on this thread.
    #[cfg(any(test, feature = "work-count"))]
    pub fn take() -> u64 {
        COUNT.with(|c| c.replace(0))
    }

    /// A slice whose iteration counts every element it yields, so any scan over it is
    /// measured, not only the helpers that tick explicitly (amendment 13; evaluator pass 3, R6:
    /// an added scan of every binding per call, or of every endpoint per site, escaped the
    /// counters). Indexing is O(1) and not counted.
    #[derive(Debug)]
    pub struct Counted<'a, T>(&'a [T]);

    impl<T> Clone for Counted<'_, T> {
        fn clone(&self) -> Self {
            *self
        }
    }

    impl<T> Copy for Counted<'_, T> {}

    impl<'a, T> Counted<'a, T> {
        pub fn new(items: &'a [T]) -> Self {
            Counted(items)
        }

        pub fn iter(&self) -> impl DoubleEndedIterator<Item = &'a T> + Clone + 'a {
            self.0.iter().inspect(|_| tick(1))
        }

        pub fn get(&self, i: usize) -> Option<&'a T> {
            self.0.get(i)
        }

        pub fn len(&self) -> usize {
            self.0.len()
        }

        pub fn is_empty(&self) -> bool {
            self.0.is_empty()
        }
    }

    impl<T> std::ops::Index<usize> for Counted<'_, T> {
        type Output = T;
        fn index(&self, i: usize) -> &T {
            &self.0[i]
        }
    }

    impl<'a, T> IntoIterator for &Counted<'a, T> {
        type Item = &'a T;
        type IntoIter = std::iter::Inspect<std::slice::Iter<'a, T>, fn(&&'a T)>;
        fn into_iter(self) -> Self::IntoIter {
            self.0.iter().inspect(|_| tick(1))
        }
    }
}

use std::collections::{BTreeMap, BTreeSet};

use rb_sql::Op;
use rb_syntax::{DecorationArg, DefKind, Definition, FileFacts, Language, RefKind, Span};

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
    pub table: Result<TableName, Unmapped>,
    /// The table name (or schema) comes from a naming rule, not from the code; projects can
    /// override it.
    pub name_assumed: bool,
    /// JPA: the strategy declared by `@Inheritance` on this class.
    pub inheritance: Option<Inheritance>,
}

/// Why a mapped class has no table name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmapped {
    /// The code names the table with something other than a static string.
    DynamicName,
    /// A Django proxy whose concrete parent is not declared in the same file.
    UnknownProxyParent,
}

/// JPA `InheritanceType`. Without `@Inheritance` a hierarchy is `SINGLE_TABLE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Inheritance {
    SingleTable,
    Joined,
    TablePerClass,
}

/// What an ORM access refers to, before cross-file resolution.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    /// A JPA entity by entity name, as JPQL names it (resolved through `Mapping::entity`).
    Entity(String),
    /// A JPA entity by class name, as a Spring Data repository's type argument names it.
    EntityClass(String),
    /// A Prisma client model accessor, e.g. `user` in `prisma.user.findMany()`.
    PrismaAccessor(String),
    /// Native SQL, to be analysed by `rb-sql`. `literal: false` when the text has `$n` holes
    /// or is not a string at all.
    Sql { text: String, literal: bool },
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

fn arg<'f>(decoration: &'f rb_syntax::Decoration, key: &str) -> Option<&'f DecorationArg> {
    decoration
        .args
        .iter()
        .find(|a| a.key.as_deref() == Some(key))
}

fn positional(decoration: &rb_syntax::Decoration) -> Option<&DecorationArg> {
    decoration.args.iter().find(|a| a.key.is_none())
}

/// A non-empty argument: `Some(Some(text))` for a static string, `Some(None)` for anything else.
fn name_arg(a: Option<&DecorationArg>) -> Option<Option<&str>> {
    a.filter(|a| !a.value.is_empty())
        .map(|a| a.literal.then_some(a.value.as_str()))
}

/// Classes in one file that map to tables.
pub fn mappings(path: &str, language: Language, facts: &FileFacts) -> Vec<Mapping> {
    let defs = &facts.definitions;
    let py = (language == Language::Python).then(|| PyClasses::new(path, defs));
    (0..defs.len())
        .filter(|&class| defs[class].kind == DefKind::Class)
        .filter_map(|class| match &py {
            Some(py) => python_mapping(class, py),
            None => match language {
                Language::Java => jpa_mapping(class, &defs[class]),
                Language::TypeScript | Language::Tsx | Language::JavaScript => {
                    typeorm_mapping(class, &defs[class])
                }
                Language::Python | Language::Go => None,
            },
        })
        .collect()
}

fn mapping(
    class: usize,
    orm: Orm,
    entity: &str,
    table: Result<TableName, Unmapped>,
    assumed: bool,
) -> Mapping {
    Mapping {
        class,
        orm,
        entity: entity.to_owned(),
        name_assumed: assumed && table.is_ok(),
        table,
        inheritance: None,
    }
}

/// JPA: `@Entity(name?)` plus optional `@Table(name, schema)`. Without a table name the spec's
/// default is the entity name. Spring Boot's physical naming then applies to every name.
fn jpa_mapping(class: usize, def: &Definition) -> Option<Mapping> {
    let entity_decoration = decoration(def, "Entity")?;
    let entity = match name_arg(arg(entity_decoration, "name")) {
        Some(Some(name)) => Some(name),
        Some(None) => None,
        None => Some(def.name.as_str()),
    };
    let table = decoration(def, "Table");
    let name = match name_arg(table.and_then(|t| arg(t, "name"))) {
        Some(explicit) => explicit.map(|n| (n, true)),
        None => entity.map(|n| (n, false)),
    };
    let schema = name_arg(table.and_then(|t| arg(t, "schema")));
    let physical = match (name, schema) {
        (Some((name, explicit)), None | Some(Some(_))) => {
            let schema = schema.flatten();
            // Only inserted underscores are an assumption: case folding is what Postgres does.
            let rewritten = |written: &str, physical: &str| {
                physical.to_lowercase() != written.trim_matches(['"', '`']).to_lowercase()
            };
            let physical = hibernate_physical_name(name);
            let physical_schema = schema.map(hibernate_physical_name);
            let assumed = !explicit
                || rewritten(name, &physical)
                || schema
                    .zip(physical_schema.as_deref())
                    .is_some_and(|(w, p)| rewritten(w, p));
            let table = TableName {
                schema: physical_schema,
                name: physical,
            };
            Some((table, assumed))
        }
        _ => None,
    };
    let inheritance = decoration(def, "Inheritance")
        .and_then(|i| arg(i, "strategy").or_else(|| positional(i)))
        .and_then(|a| match simple(&a.value) {
            "SINGLE_TABLE" => Some(Inheritance::SingleTable),
            "JOINED" => Some(Inheritance::Joined),
            "TABLE_PER_CLASS" => Some(Inheritance::TablePerClass),
            _ => None,
        });
    let (table, assumed) =
        physical.map_or((Err(Unmapped::DynamicName), false), |(t, a)| (Ok(t), a));
    Some(Mapping {
        inheritance,
        ..mapping(class, Orm::Jpa, entity.unwrap_or(&def.name), table, assumed)
    })
}

/// TypeORM: `@Entity('name')` or `@Entity({ name, schema })`; default is snake_case(class).
fn typeorm_mapping(class: usize, def: &Definition) -> Option<Mapping> {
    let entity = decoration(def, "Entity")?;
    let explicit = name_arg(positional(entity).or_else(|| arg(entity, "name")));
    let schema = name_arg(arg(entity, "schema"));
    let (name, assumed) = match explicit {
        Some(n) => (n.map(str::to_owned), false),
        None => (Some(snake_case(&def.name)), true),
    };
    let table = match (name, schema) {
        (Some(name), None) => Ok(TableName { schema: None, name }),
        (Some(name), Some(Some(schema))) => Ok(TableName {
            schema: Some(schema.to_owned()),
            name,
        }),
        _ => Err(Unmapped::DynamicName),
    };
    Some(mapping(class, Orm::TypeOrm, &def.name, table, assumed))
}

/// SQLAlchemy `__tablename__`; Flask-SQLAlchemy `db.Model` (default `camel_to_snake_case(class)`);
/// else a Django model, directly or through models in the same file.
fn python_mapping(class: usize, py: &PyClasses<'_>) -> Option<Mapping> {
    let def = &py.defs[class];
    if py.is_true(class, "__abstract__") {
        return None;
    }
    if let Some(t) = constant(def, "__tablename__") {
        // `None`: a single-table-inheritance subclass, stored in its parent's table.
        if !t.literal && t.value == "None" {
            return None;
        }
        return Some(mapping(
            class,
            Orm::SqlAlchemy,
            &def.name,
            static_name(t),
            false,
        ));
    }
    if py.flask[class] {
        let table = TableName {
            schema: None,
            name: flask_snake_case(&def.name),
        };
        return Some(mapping(class, Orm::SqlAlchemy, &def.name, Ok(table), true));
    }
    let (table, assumed) = py.tables[class].clone()?;
    Some(mapping(class, Orm::Django, &def.name, table, assumed))
}

/// The classes of one Python file and how they inherit from each other (same-scope bases only,
/// as there are no import facts), computed once per file so every walk is cheap.
struct PyClasses<'d> {
    defs: &'d [Definition],
    /// Per definition: its same-scope base classes, in source order.
    bases: Vec<Vec<usize>>,
    /// Per definition: its nested `Meta` class.
    meta: Vec<Option<usize>>,
    /// Per definition: a Django model (abstract or not), directly or through its bases.
    django: Vec<bool>,
    /// Per definition: a Flask-SQLAlchemy model with its own table: it extends `db.Model`
    /// directly or through `__abstract__` bases only. A subclass of a concrete model shares its
    /// table (single-table inheritance).
    flask: Vec<bool>,
    /// Per definition: its Django table, when it is a concrete model or a proxy.
    tables: Vec<Option<DjangoTable>>,
}

/// A Django model's table and whether its name is assumed.
type DjangoTable = (Result<TableName, Unmapped>, bool);

/// Per class `c`: `own(c)` if it has a value, else the first base in `deps(c)` (in order) that
/// has one, else `fallback(c)`. A base still being resolved (a cycle) counts as having none.
/// Iterative and memoised: linear in classes plus links, whatever the depth.
fn first_along<T: Clone>(
    n: usize,
    own: impl Fn(usize) -> Option<T>,
    deps: impl Fn(usize) -> Vec<usize>,
    fallback: impl Fn(usize) -> Option<T>,
) -> Vec<Option<T>> {
    #[derive(Clone, Copy, PartialEq)]
    enum State {
        New,
        Open,
        Done,
    }
    let mut state = vec![State::New; n];
    let mut value: Vec<Option<T>> = vec![None; n];
    for start in 0..n {
        if state[start] != State::New {
            continue;
        }
        // Frames: (class, its deps, index of the next dep to try).
        let mut stack: Vec<(usize, Vec<usize>, usize)> = Vec::new();
        state[start] = State::Open;
        match own(start) {
            Some(v) => {
                value[start] = Some(v);
                state[start] = State::Done;
                continue;
            }
            None => stack.push((start, deps(start), 0)),
        }
        while let Some((c, ds, i)) = stack.last_mut() {
            let c = *c;
            if *i == ds.len() {
                value[c] = fallback(c);
                state[c] = State::Done;
                stack.pop();
                continue;
            }
            let d = ds[*i];
            match state[d] {
                State::Done if value[d].is_some() => {
                    value[c] = value[d].clone();
                    state[c] = State::Done;
                    stack.pop();
                }
                State::Done | State::Open => *i += 1,
                // Resolve `d` first; this frame looks at it again once it is done.
                State::New => {
                    state[d] = State::Open;
                    match own(d) {
                        Some(v) => {
                            value[d] = Some(v);
                            state[d] = State::Done;
                        }
                        None => stack.push((d, deps(d), 0)),
                    }
                }
            }
        }
    }
    value
}

impl<'d> PyClasses<'d> {
    fn new(path: &str, defs: &'d [Definition]) -> Self {
        let mut first: BTreeMap<(Option<usize>, &str), usize> = BTreeMap::new();
        let mut meta = vec![None; defs.len()];
        for (i, d) in defs.iter().enumerate() {
            if d.kind == DefKind::Class {
                first.entry((d.parent, d.name.as_str())).or_insert(i);
            }
            if let Some(p) = d.parent.filter(|_| d.name == "Meta") {
                meta[p].get_or_insert(i);
            }
        }
        let bases: Vec<Vec<usize>> = defs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                d.supertypes
                    .iter()
                    .filter_map(|s| first.get(&(d.parent, simple(&s.name))).copied())
                    .filter(|&b| b != i)
                    .collect()
            })
            .collect();
        let mut py = PyClasses {
            defs,
            bases,
            meta,
            django: Vec::new(),
            flask: Vec::new(),
            tables: Vec::new(),
        };
        let direct = |names: &[&str], d: &Definition| {
            d.supertypes
                .iter()
                .any(|s| names.iter().any(|n| s.name == *n))
        };
        py.django = py.descendants(
            |c| direct(&["models.Model", "django.db.models.Model"], &defs[c]),
            |_| true,
        );
        let is_flask_root = |c: usize| {
            defs[c]
                .supertypes
                .iter()
                .any(|s| s.name == "db.Model" || s.name.ends_with(".db.Model"))
        };
        py.flask = py.descendants(is_flask_root, |c| {
            constant(&defs[c], "__abstract__").is_some_and(|a| a.value == "True")
        });
        py.tables = py.resolve_django_tables(path);
        py
    }

    /// Classes that are `root`s, or inherit from one through bases passing `through`: one
    /// reverse breadth-first walk, linear in classes plus base links.
    fn descendants(
        &self,
        root: impl Fn(usize) -> bool,
        through: impl Fn(usize) -> bool,
    ) -> Vec<bool> {
        let mut children: Vec<Vec<usize>> = vec![Vec::new(); self.defs.len()];
        for (c, bases) in self.bases.iter().enumerate() {
            for &b in bases {
                children[b].push(c);
            }
        }
        let mut marked: Vec<bool> = (0..self.defs.len()).map(&root).collect();
        let mut queue: Vec<usize> = (0..self.defs.len()).filter(|&c| marked[c]).collect();
        while let Some(b) = queue.pop() {
            if !through(b) {
                continue;
            }
            for &c in &children[b] {
                if !marked[c] {
                    marked[c] = true;
                    queue.push(c);
                }
            }
        }
        marked
    }

    /// A constant of the class's own `Meta`.
    fn own_meta(&self, class: usize, name: &str) -> Option<&'d rb_syntax::ClassConstant> {
        constant(&self.defs[self.meta[class]?], name)
    }

    fn is_true(&self, class: usize, name: &str) -> bool {
        let value = if name.starts_with("__") {
            constant(&self.defs[class], name)
        } else {
            self.own_meta(class, name)
        };
        value.is_some_and(|c| c.literal && c.value == "True")
    }

    /// Per class, a `Meta` constant as Django sees it: a class without its own `Meta` inherits
    /// the `Meta` of an abstract base, and `class Meta(Base.Meta)` extends it. Use only for
    /// options Django inherits (`abstract` is reset on children).
    fn inherited_meta(&self, name: &str) -> Vec<Option<&'d rb_syntax::ClassConstant>> {
        first_along(
            self.defs.len(),
            |c| self.own_meta(c, name),
            |c| match self.meta[c] {
                // `class Meta(Base.Meta)`: the bases whose `Meta` it extends.
                Some(m) => self.defs[m]
                    .supertypes
                    .iter()
                    .filter_map(|s| {
                        let base = s.name.strip_suffix(".Meta")?;
                        self.bases[c]
                            .iter()
                            .copied()
                            .find(|&b| self.defs[b].name == simple(base))
                    })
                    .collect(),
                // Without a `Meta` of its own, a class inherits the whole `Meta` of its first
                // abstract parent; other parents' options are ignored.
                None => self.bases[c]
                    .iter()
                    .copied()
                    .find(|&b| self.is_true(b, "abstract"))
                    .into_iter()
                    .collect(),
            },
            |_| None,
        )
    }

    /// Every class's Django table. Abstract models have no table, proxies use their parent's,
    /// and every other model (including a multi-table-inheritance child) has its own.
    fn resolve_django_tables(&self, path: &str) -> Vec<Option<DjangoTable>> {
        let db_table = self.inherited_meta("db_table");
        let app_label = self.inherited_meta("app_label");
        let concrete = |c: usize| -> Option<DjangoTable> {
            if self.is_true(c, "proxy") || !self.django[c] || self.is_true(c, "abstract") {
                return None;
            }
            if let Some(t) = db_table[c] {
                return Some((django_db_table(t), false));
            }
            let label = match app_label[c] {
                Some(l) if l.literal => l.value.as_str(),
                Some(_) => return Some((Err(Unmapped::DynamicName), false)),
                None => django_app_label(path)?,
            };
            let table = TableName {
                schema: None,
                name: format!("{label}_{}", self.defs[c].name.to_lowercase()),
            };
            Some((Ok(table), true))
        };
        // `Meta.proxy` marks a Django model even when its parent is declared in another file.
        first_along(
            self.defs.len(),
            concrete,
            |c| {
                if self.is_true(c, "proxy") {
                    self.bases[c].clone()
                } else {
                    Vec::new()
                }
            },
            |c| {
                self.is_true(c, "proxy")
                    .then_some((Err(Unmapped::UnknownProxyParent), false))
            },
        )
    }
}

fn constant<'d>(def: &'d Definition, name: &str) -> Option<&'d rb_syntax::ClassConstant> {
    def.class_constants.iter().find(|c| c.name == name)
}

fn static_name(c: &rb_syntax::ClassConstant) -> Result<TableName, Unmapped> {
    if c.literal {
        Ok(TableName {
            schema: None,
            name: c.value.clone(),
        })
    } else {
        Err(Unmapped::DynamicName)
    }
}

/// `Meta.db_table`, including the Postgres schema idioms `'"auth"."users"'` and
/// `'auth"."users'`, which Django's quoting turns into `"auth"."users"`.
fn django_db_table(c: &rb_syntax::ClassConstant) -> Result<TableName, Unmapped> {
    let table = static_name(c)?;
    let unquoted = table
        .name
        .strip_prefix('"')
        .and_then(|n| n.strip_suffix('"'))
        .unwrap_or(&table.name);
    Ok(match unquoted.split_once("\".\"") {
        Some((schema, name)) => TableName {
            schema: Some(schema.to_owned()),
            name: name.to_owned(),
        },
        None => table,
    })
}

/// `blog/models.py` → `blog`; `blog/models/article.py` and `blog/models/sub/x.py` → `blog` (the
/// directory above a `models` package).
fn django_app_label(path: &str) -> Option<&str> {
    let dirs: Vec<&str> = path.split('/').collect();
    let dirs = &dirs[..dirs.len() - 1];
    match dirs.iter().rposition(|d| *d == "models") {
        Some(i) => i.checked_sub(1).map(|p| dirs[p]),
        None => dirs.last().copied(),
    }
}

/// Spring Data repository interfaces, by simple name, with the index of the type parameter that
/// is their entity: the built-in bases, plus every interface in `definitions` that passes one of
/// its own type parameters to a known base (`interface BaseRepo<T> extends JpaRepository<T, Long>`).
///
/// Names are resolved without imports, so every declaration of a name must agree (including a
/// project interface named like a built-in base); otherwise the name is dropped rather than
/// guessed. Supertypes written as `org.springframework.data.…` always mean the built-in base.
pub fn repository_bases<'d>(
    definitions: impl IntoIterator<Item = &'d Definition>,
) -> BTreeMap<String, usize> {
    let mut declared: BTreeMap<&str, Vec<&Definition>> = BTreeMap::new();
    for def in definitions
        .into_iter()
        .filter(|d| !d.type_params.is_empty())
    {
        declared.entry(def.name.as_str()).or_default().push(def);
    }
    let mut resolver = BaseResolver {
        declared: &declared,
        memo: BTreeMap::new(),
        resolving: BTreeSet::new(),
        steps: 0,
    };
    let names: BTreeSet<&str> = declared
        .keys()
        .copied()
        .chain(REPOSITORY_BASES.iter().copied())
        .collect();
    names
        .into_iter()
        .filter_map(|n| Some((n.to_owned(), resolver.resolve(n)?)))
        .collect()
}

/// Deepest chain of generic base interfaces followed; deeper ones stay unresolved.
const MAX_BASE_DEPTH: usize = 512;

/// Resolution steps after which results are cached even when they depend on a cycle still being
/// resolved: bounds recomputation in pathological cyclic declarations.
const MAX_BASE_STEPS: usize = 100_000;

struct BaseResolver<'a, 'd> {
    declared: &'a BTreeMap<&'d str, Vec<&'d Definition>>,
    memo: BTreeMap<&'d str, Option<usize>>,
    /// Names on the current resolution path: meeting one again contributes no binding.
    resolving: BTreeSet<&'d str>,
    steps: usize,
}

impl<'d> BaseResolver<'_, 'd> {
    fn resolve(&mut self, name: &'d str) -> Option<usize> {
        self.resolve_tracked(name).0
    }

    /// The binding of `name`, and the names on the current path that it was computed without
    /// (cycles cut short). A result is cached only once none of those remain pending, so it never
    /// depends on which name resolution started from.
    fn resolve_tracked(&mut self, name: &'d str) -> (Option<usize>, BTreeSet<&'d str>) {
        if let Some(known) = self.memo.get(name) {
            return (*known, BTreeSet::new());
        }
        if self.resolving.contains(name) {
            return (None, BTreeSet::from([name]));
        }
        if self.resolving.len() >= MAX_BASE_DEPTH {
            return (None, BTreeSet::new());
        }
        self.steps += 1;
        self.resolving.insert(name);
        let mut pending: BTreeSet<&'d str> = BTreeSet::new();
        // One entry per declaration (a built-in base counts as one): the parameter it binds, or
        // `None` when it binds none.
        let mut bindings: BTreeSet<Option<usize>> = BTreeSet::new();
        if REPOSITORY_BASES.contains(&name) {
            bindings.insert(Some(0));
        }
        for def in self.declared.get(name).into_iter().flatten() {
            let mut params: BTreeSet<usize> = BTreeSet::new();
            for s in &def.supertypes {
                let base = simple(&s.name);
                let index = if s.name.starts_with("org.springframework.data.") {
                    REPOSITORY_BASES.contains(&base).then_some(0)
                } else if let Some(known) = self.known_name(base) {
                    let (index, cut) = self.resolve_tracked(known);
                    pending.extend(cut);
                    index
                } else {
                    None
                };
                let param = index
                    .and_then(|k| s.type_args.get(k))
                    .and_then(|arg| def.type_params.iter().position(|p| p == arg));
                params.extend(param);
            }
            let mut params = params.into_iter();
            bindings.insert(match (params.next(), params.next()) {
                (Some(p), None) => Some(p),
                _ => None,
            });
        }
        self.resolving.remove(name);
        pending.remove(name);
        let result = match bindings.into_iter().collect::<Vec<_>>().as_slice() {
            [Some(param)] => Some(*param),
            _ => None,
        };
        if pending.is_empty() || self.steps > MAX_BASE_STEPS {
            self.memo.insert(name, result);
        }
        (result, pending)
    }

    /// The interned form of a supertype name: only declared or built-in names can bind.
    fn known_name(&self, name: &str) -> Option<&'d str> {
        self.declared
            .get_key_value(name)
            .map(|(k, _)| *k)
            .or_else(|| REPOSITORY_BASES.iter().copied().find(|b| *b == name))
    }
}

/// ORM data access in one file. `repository_bases` comes from [`repository_bases`] over every
/// Java file of the snapshot.
pub fn accesses(
    language: Language,
    facts: &FileFacts,
    repository_bases: &BTreeMap<String, usize>,
) -> Vec<Access> {
    let mut out = Vec::new();
    match language {
        Language::Java => {
            spring_data_accesses(facts, repository_bases, &mut out);
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

fn spring_data_accesses(facts: &FileFacts, bases: &BTreeMap<String, usize>, out: &mut Vec<Access>) {
    let defs = &facts.definitions;
    for (repo, def) in defs.iter().enumerate() {
        let entity = def
            .supertypes
            .iter()
            .find_map(|s| {
                let base = simple(&s.name);
                let index = if s.name.starts_with("org.springframework.data.") {
                    REPOSITORY_BASES.contains(&base).then_some(0)
                } else {
                    bases.get(base).copied()
                };
                s.type_args.get(index?)
            })
            .map(|e| simple(e).to_owned());
        // A generic base repository's entity is a type parameter: its subinterfaces bind it.
        let Some(entity) = entity.filter(|e| !def.type_params.contains(e)) else {
            continue;
        };
        // Inherited CRUD methods (save, findById, delete…) are not declared: the repository
        // itself carries them.
        out.push(Access {
            source: Some(repo),
            target: Target::EntityClass(entity.clone()),
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
                let Some(text) = arg(query, "value").or_else(|| positional(query)) else {
                    continue;
                };
                if arg(query, "nativeQuery").is_some_and(|a| a.value == "true") {
                    // Not a static string: the indexer reports it as SQL it could not parse.
                    out.push(Access {
                        source: Some(method),
                        target: Target::Sql {
                            text: text.value.clone(),
                            literal: text.literal,
                        },
                        ops: BTreeSet::new(),
                        confidence: Confidence::Stated,
                        span: m.name_span,
                    });
                } else {
                    // Non-literal parts are `$n` holes, which never read as entity names.
                    out.extend(jpql_accesses(&text.value, Some(method), m.name_span));
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
                    target: Target::EntityClass(entity.clone()),
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
        // `<client>.<model>.<method>()`: the model accessor is the qualifier's last segment, and
        // other libraries' `delete`/`update` are told apart by how the client is named.
        let Some((client, accessor)) = qualifier.rsplit_once('.') else {
            continue;
        };
        let client = simple(client);
        if !(client.to_ascii_lowercase().contains("prisma") || PRISMA_CLIENTS.contains(&client)) {
            continue;
        }
        out.push(Access {
            source: r.enclosing,
            target: Target::PrismaAccessor(accessor.to_owned()),
            ops,
            confidence: Confidence::Convention,
            span: r.span,
        });
    }
}

/// Receiver names that conventionally hold a Prisma client or transaction.
const PRISMA_CLIENTS: &[&str] = &["db", "tx", "client", "trx", "database"];

/// Keywords that end a FROM item (so the next word is not an alias).
const JPQL_CLAUSE_WORDS: &[&str] = &[
    "WHERE", "JOIN", "INNER", "LEFT", "RIGHT", "OUTER", "FETCH", "GROUP", "ORDER", "HAVING", "ON",
    "UNION", "SET", "AS",
];

/// Entities named by a JPQL statement, with the operation on each, sorted. Association paths
/// (`JOIN o.lines l`) are not entities; qualified class names count by their simple name. Only
/// FROM and JOIN at clause level count, in the statement or a subquery: `TRIM(… FROM x)`,
/// `IN(c.orders)` and `TREAT(… AS E)` name no entity.
pub fn jpql(text: &str) -> Vec<(String, Op)> {
    let words = jpql_words(text);
    let upper: Vec<String> = words.iter().map(|w| w.to_ascii_uppercase()).collect();
    let clause_level = clause_levels(&upper);
    let mut out: BTreeSet<(String, Op)> = BTreeSet::new();
    let mut target_from = None; // index of a FROM that names the DELETE target
    match upper.first().map(String::as_str) {
        Some("UPDATE") => {
            // HQL `UPDATE VERSIONED Entity`.
            let at = if upper.get(1).map(String::as_str) == Some("VERSIONED") {
                2
            } else {
                1
            };
            if let Some(entity) = words.get(at).filter(|w| is_identifier(w)) {
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
            if let Some(entity) = words.get(at).filter(|w| is_identifier(w)) {
                out.insert((simple(entity).to_owned(), Op::Delete));
            }
        }
        Some("INSERT") => {
            if let Some(entity) = upper
                .iter()
                .position(|w| w == "INTO")
                .and_then(|i| words.get(i + 1))
                .filter(|w| is_identifier(w))
            {
                out.insert((simple(entity).to_owned(), Op::Insert));
            }
        }
        _ => {}
    }
    for (i, word) in upper.iter().enumerate() {
        if !clause_level[i] {
            continue;
        }
        if word == "FROM" && Some(i) != target_from {
            from_items(&words, &upper, i + 1, &mut out);
        } else if word == "JOIN" {
            let at = if upper.get(i + 1).map(String::as_str) == Some("FETCH") {
                i + 2
            } else {
                i + 1
            };
            let after = if words.get(at + 1).map(String::as_str) == Some("(") {
                // `JOIN TREAT(…) a`: a function, not an entity.
                skip_alias(&words, &upper, close_paren(&words, at + 1))
            } else {
                if let Some(entity) = words
                    .get(at)
                    .filter(|w| is_identifier(w) && !w.contains('.'))
                {
                    out.insert((simple(entity).to_owned(), Op::Read));
                }
                skip_alias(&words, &upper, at + 1)
            };
            // `JOIN o.lines l, Customer c`: a comma after the join continues the FROM list.
            if words.get(after).map(String::as_str) == Some(",") {
                from_items(&words, &upper, after + 1, &mut out);
            }
        }
    }
    out.into_iter().collect()
}

/// For each word, whether it is at clause level: outside parentheses, or directly inside a
/// parenthesised subquery (`(SELECT …`).
fn clause_levels(upper: &[String]) -> Vec<bool> {
    let mut subquery: Vec<bool> = Vec::new();
    upper
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let level = subquery.last().copied().unwrap_or(true);
            match w.as_str() {
                "(" => subquery.push(upper.get(i + 1).map(String::as_str) == Some("SELECT")),
                ")" => {
                    subquery.pop();
                }
                _ => {}
            }
            level
        })
        .collect()
}

/// Index after the `)` matching the `(` at `open` (or the end of the words).
fn close_paren(words: &[String], open: usize) -> usize {
    let mut depth = 0usize;
    for (i, w) in words.iter().enumerate().skip(open) {
        match w.as_str() {
            "(" => depth += 1,
            ")" => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
    }
    words.len()
}

/// Reads `Entity [AS] alias, Entity [AS] alias, …` starting at `j`. Function-like items
/// (`IN(c.orders) o`) are skipped.
fn from_items(words: &[String], upper: &[String], mut j: usize, out: &mut BTreeSet<(String, Op)>) {
    while let Some(entity) = words.get(j).filter(|w| is_identifier(w)) {
        if words.get(j + 1).map(String::as_str) == Some("(") {
            j = skip_alias(words, upper, close_paren(words, j + 1));
        } else {
            out.insert((simple(entity).to_owned(), Op::Read));
            j = skip_alias(words, upper, j + 1);
        }
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

/// A name: `Thing$1` (a template hole glued to text) is not one.
fn is_identifier(word: &str) -> bool {
    let hole = word
        .split('$')
        .skip(1)
        .any(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
    word.starts_with(|c: char| c.is_alphabetic() || c == '_') && !hole
}

/// JPQL words: identifiers (with dots), and `(`, `)` and `,` as their own words. String literals,
/// named parameters, (HQL) comments and SpEL expressions (`#{#entityName}`) are skipped.
fn jpql_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '#' && chars.peek() == Some(&'{') {
            for d in chars.by_ref() {
                if d == '}' {
                    break;
                }
            }
            // Keep the item's position so a following alias is not read as an entity.
            words.push("#".to_owned());
        } else if c == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut prev = ' ';
            for d in chars.by_ref() {
                if prev == '*' && d == '/' {
                    break;
                }
                prev = d;
            }
        } else if c == '-' && chars.peek() == Some(&'-') {
            for d in chars.by_ref() {
                if d == '\n' {
                    break;
                }
            }
        } else if c == '\'' {
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
            } else {
                // Block attributes may share a line: `@@map("users") @@schema("auth")`.
                if let Some(name) = attribute_string(line, "@@map(") {
                    model.table.name = name;
                }
                if let Some(schema) = attribute_string(line, "@@schema(") {
                    model.table.schema = Some(schema);
                }
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

/// The first string argument of a Prisma block attribute such as `@@map("users")` or
/// `@@map(name: "users")`.
fn attribute_string(line: &str, attribute: &str) -> Option<String> {
    let at = line.find(attribute)?;
    let rest = line[at + attribute.len()..].trim_start();
    let rest = rest.strip_prefix("name").map_or(rest, |r| {
        r.trim_start().strip_prefix(':').unwrap_or(r).trim_start()
    });
    let rest = rest.strip_prefix('"')?;
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

/// `snakeCase` as TypeORM's `DefaultNamingStrategy` applies it: `ABc → A_Bc`, `aC → a_C` (a
/// digit counts as lowercase), then lowercase.
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

/// Flask-SQLAlchemy's `camel_to_snake_case`: an underscore before a capital that follows a
/// lowercase letter or digit, or that is not first and precedes a lowercase letter; then lowercase
/// and strip leading underscores (`My_Table → my__table`).
pub fn flask_snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        let prev = i.checked_sub(1).map(|p| chars[p]);
        let next = chars.get(i + 1);
        let boundary = c.is_ascii_uppercase()
            && (prev.is_some_and(|p| p.is_ascii_lowercase() || p.is_ascii_digit())
                || (i > 0 && next.is_some_and(char::is_ascii_lowercase)));
        if boundary {
            out.push('_');
        }
        out.extend(c.to_lowercase());
    }
    out.trim_start_matches('_').to_owned()
}

/// A JPA name as Hibernate's `PhysicalNamingStrategySnakeCaseImpl` (Spring Boot's default)
/// writes it to the database. Quoted names (`"x"`, `` `x` ``) are kept exactly. Otherwise `.`
/// becomes `_`, an underscore goes before an interior capital that has a lowercase letter or digit
/// on both sides, and the result is lowercased (as Postgres folds unquoted names).
pub fn hibernate_physical_name(name: &str) -> String {
    for quote in ['"', '`'] {
        if let Some(inner) = name.strip_prefix(quote).and_then(|n| n.strip_suffix(quote)) {
            return inner.to_owned();
        }
    }
    let chars: Vec<char> = name.replace('.', "_").chars().collect();
    let lower_or_digit = |c: char| c.is_lowercase() || c.is_ascii_digit();
    let mut out = String::with_capacity(name.len() + 4);
    for (i, &c) in chars.iter().enumerate() {
        let interior = i > 0 && i + 1 < chars.len();
        if interior
            && c.is_uppercase()
            && lower_or_digit(chars[i - 1])
            && lower_or_digit(chars[i + 1])
        {
            out.push('_');
        }
        out.extend(c.to_lowercase());
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
    fn snake_case_matches_typeorm_and_flask_sqlalchemy() {
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

    #[test]
    fn hibernate_physical_names() {
        for (input, expected) in [
            ("User", "user"),
            ("UserAccount", "user_account"),
            // An underscore needs a lowercase letter or digit on both sides of the capital.
            ("HTTPLog", "httplog"),
            ("APIKey", "apikey"),
            ("UserA", "usera"),
            ("order2Item", "order2_item"),
            ("user_accounts", "user_accounts"),
            ("auth.Users", "auth_users"),
            // Quoted names are kept exactly.
            ("\"UserAccount\"", "UserAccount"),
            ("`Users`", "Users"),
        ] {
            assert_eq!(hibernate_physical_name(input), expected, "{input}");
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
                        m.table.unwrap(),
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
        let bases = repository_bases(&f.definitions);
        let got: Vec<(String, Target, BTreeSet<Op>, Confidence)> =
            accesses(Language::Java, &f, &bases)
                .into_iter()
                .map(|a| {
                    let source = a
                        .source
                        .map_or("-".to_owned(), |i| f.definitions[i].name.clone());
                    (source, a.target, a.ops, a.confidence)
                })
                .collect();
        let user = || Target::EntityClass("User".into());
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
                    Target::Sql {
                        text: "SELECT * FROM auth.users WHERE email LIKE ?1".into(),
                        literal: true
                    },
                    ops(&[]),
                    Confidence::Stated
                ),
                (
                    "deactivate".into(),
                    Target::Entity("User".into()),
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
        let got: Vec<(Target, BTreeSet<Op>)> = accesses(Language::TypeScript, &f, &BTreeMap::new())
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
            .map(|m| (m.table.unwrap(), m.name_assumed))
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
                        m.table.unwrap(),
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

    fn mapped(
        path: &str,
        language: Language,
        src: &str,
    ) -> Vec<(String, Orm, Option<TableName>, bool)> {
        let f = facts(language, src);
        mappings(path, language, &f)
            .into_iter()
            .map(|m| {
                (
                    f.definitions[m.class].name.clone(),
                    m.orm,
                    m.table.ok(),
                    m.name_assumed,
                )
            })
            .collect()
    }

    #[test]
    fn jpa_quoted_explicit_dynamic_names_and_inheritance_facts() {
        let src = r#"@Entity @Table(name = "\"Users\"") class A {}
@Entity @Table(name = "UserAccounts") class B {}
@Entity @Table(name = "user_accounts") class B2 {}
@Entity @Table(name = Tables.ACCOUNTS) class C {}
@Entity @Table(name = "people") @Inheritance(strategy = InheritanceType.JOINED) class Person {}
@Entity class Admin extends Person implements Serializable {}
"#;
        assert_eq!(
            mapped("m/A.java", Language::Java, src),
            [
                ("A".into(), Orm::Jpa, Some(table(None, "Users")), false),
                // Spring Boot's naming strategy rewrites explicit names too.
                (
                    "B".into(),
                    Orm::Jpa,
                    Some(table(None, "user_accounts")),
                    true
                ),
                (
                    "B2".into(),
                    Orm::Jpa,
                    Some(table(None, "user_accounts")),
                    false
                ),
                ("C".into(), Orm::Jpa, None, false),
                (
                    "Person".into(),
                    Orm::Jpa,
                    Some(table(None, "people")),
                    false
                ),
                ("Admin".into(), Orm::Jpa, Some(table(None, "admin")), true),
            ]
        );
        let f = facts(Language::Java, src);
        let inheritance: Vec<Option<Inheritance>> = mappings("m/A.java", Language::Java, &f)
            .into_iter()
            .map(|m| m.inheritance)
            .collect();
        assert_eq!(inheritance[4], Some(Inheritance::Joined));
        assert_eq!(inheritance[5], None);

        // A snake-cased schema is a naming rule too.
        assert_eq!(
            mapped(
                "m/B.java",
                Language::Java,
                "@Entity @Table(name = \"users\", schema = \"BillingData\") class U {}\n"
            ),
            [(
                "U".into(),
                Orm::Jpa,
                Some(table(Some("billing_data"), "users")),
                true
            )]
        );
    }

    #[test]
    fn django_abstract_proxy_app_label_subclasses_and_flask() {
        let src = r#"class Base(models.Model):
    class Meta:
        abstract = True

class Post(Base):
    pass

class Article(models.Model):
    class Meta:
        db_table = 'articles'

class ArticleProxy(Article):
    class Meta:
        proxy = True

class Featured(Article):
    pass

class Labeled(models.Model):
    class Meta:
        app_label = 'news'

class Dyn(models.Model):
    class Meta:
        db_table = f"{P}_x"

class FlaskUser(db.Model):
    pass

class Legacy(db.Model):
    __tablename__ = 'legacy'

class Dyn2(Base2):
    __tablename__ = f"{P}_users"

class DynName(models.Model):
    class Meta:
        db_table = TABLE

class Net(tf.keras.models.Model):
    pass

class LabeledBase(models.Model):
    class Meta:
        abstract = True
        app_label = 'inherited'

class LabeledChild(LabeledBase):
    pass

class OwnMeta(LabeledBase):
    class Meta:
        ordering = ['x']

class My_Table(db.Model):
    pass

class Helper:
    pass
"#;
        assert_eq!(
            mapped("blog/models.py", Language::Python, src),
            [
                (
                    "Post".into(),
                    Orm::Django,
                    Some(table(None, "blog_post")),
                    true
                ),
                (
                    "Article".into(),
                    Orm::Django,
                    Some(table(None, "articles")),
                    false
                ),
                // A proxy has no table of its own.
                (
                    "ArticleProxy".into(),
                    Orm::Django,
                    Some(table(None, "articles")),
                    false
                ),
                // Multi-table inheritance: the child has its own table.
                (
                    "Featured".into(),
                    Orm::Django,
                    Some(table(None, "blog_featured")),
                    true
                ),
                (
                    "Labeled".into(),
                    Orm::Django,
                    Some(table(None, "news_labeled")),
                    true
                ),
                ("Dyn".into(), Orm::Django, None, false),
                (
                    "FlaskUser".into(),
                    Orm::SqlAlchemy,
                    Some(table(None, "flask_user")),
                    true
                ),
                (
                    "Legacy".into(),
                    Orm::SqlAlchemy,
                    Some(table(None, "legacy")),
                    false
                ),
                ("Dyn2".into(), Orm::SqlAlchemy, None, false),
                ("DynName".into(), Orm::Django, None, false),
                // An abstract base's Meta is inherited only by children without their own Meta.
                (
                    "LabeledChild".into(),
                    Orm::Django,
                    Some(table(None, "inherited_labeledchild")),
                    true
                ),
                (
                    "OwnMeta".into(),
                    Orm::Django,
                    Some(table(None, "blog_ownmeta")),
                    true
                ),
                (
                    "My_Table".into(),
                    Orm::SqlAlchemy,
                    Some(table(None, "my__table")),
                    true
                ),
            ]
        );
    }

    #[test]
    fn typeorm_dynamic_names_are_not_tables() {
        let src = "@Entity(TABLE) export class A {}\n@Entity(`orders_${s}`) export class B {}\n@Entity({ name: 'c', schema: SCHEMA }) export class C {}\n";
        assert_eq!(
            mapped("src/e.ts", Language::TypeScript, src),
            [
                ("A".into(), Orm::TypeOrm, None, false),
                ("B".into(), Orm::TypeOrm, None, false),
                ("C".into(), Orm::TypeOrm, None, false),
            ]
        );
    }

    #[test]
    fn generic_base_repositories_and_native_concatenation() {
        let src = r#"interface BaseRepo<T, ID> extends JpaRepository<T, ID> { List<T> findByName(String n); }
interface MidRepo<X> extends BaseRepo<X, Long> {}
interface PersonRepo extends MidRepo<Person> {
    List<Person> findByAge(int a);
    @Query(value = "SELECT * " + "FROM people", nativeQuery = true) List<Person> all();
    @Query(value = QUERY, nativeQuery = true) List<Person> dynamic();
}
"#;
        let f = facts(Language::Java, src);
        let bases = repository_bases(&f.definitions);
        assert_eq!(bases.get("BaseRepo"), Some(&0));
        assert_eq!(bases.get("MidRepo"), Some(&0));
        let got: Vec<(String, Target)> = accesses(Language::Java, &f, &bases)
            .into_iter()
            .map(|a| (f.definitions[a.source.unwrap()].name.clone(), a.target))
            .collect();
        let person = || Target::EntityClass("Person".into());
        assert_eq!(
            got,
            [
                ("PersonRepo".into(), person()),
                ("findByAge".into(), person()),
                (
                    "all".into(),
                    Target::Sql {
                        text: "SELECT * FROM people".into(),
                        literal: true
                    }
                ),
                // Not a static string: the indexer reports it as unparsed.
                (
                    "dynamic".into(),
                    Target::Sql {
                        text: "QUERY".into(),
                        literal: false
                    }
                ),
            ]
        );
    }

    #[test]
    fn jpql_ignores_function_arguments_and_collection_members() {
        let e = |name: &str| (name.to_owned(), Op::Read);
        assert_eq!(
            jpql("SELECT TRIM(BOTH ' ' FROM p.name) FROM Person p"),
            [e("Person")]
        );
        assert_eq!(
            jpql("SELECT EXTRACT(YEAR FROM o.created) FROM Order o"),
            [e("Order")]
        );
        assert_eq!(
            jpql("SELECT o FROM Customer c, IN(c.orders) o"),
            [e("Customer")]
        );
        assert_eq!(
            jpql("SELECT a FROM Shelter s JOIN TREAT(s.owner AS Admin) a"),
            [e("Shelter")]
        );
        assert_eq!(
            jpql("SELECT COALESCE((SELECT MAX(o.total) FROM Order o), 0) FROM Customer c"),
            [e("Customer"), e("Order")]
        );
        assert_eq!(
            jpql("SELECT c FROM Customer c WHERE EXISTS (SELECT o FROM Order o WHERE o.c = c)"),
            [e("Customer"), e("Order")]
        );
    }

    #[test]
    fn prisma_receivers_and_named_map() {
        let src = "async function f() {\n  this.cache.delete(k);\n  req.user.update({});\n  m.session.delete(1);\n  await this.prisma.user.findMany();\n  await ctx.prismaService.post.count();\n  await trx.post.create({});\n}\n";
        let f = facts(Language::TypeScript, src);
        let got: Vec<Target> = accesses(Language::TypeScript, &f, &BTreeMap::new())
            .into_iter()
            .map(|a| a.target)
            .collect();
        let accessor = |a: &str| Target::PrismaAccessor(a.into());
        assert_eq!(got, [accessor("user"), accessor("post"), accessor("post")]);

        let models = prisma_models(
            "model User {\n  @@map(name: \"users\")\n}\nmodel Post {\n  @@map(name:\"posts\")\n}\n",
        );
        let tables: Vec<TableName> = models.into_iter().map(|m| m.table).collect();
        assert_eq!(tables, [table(None, "users"), table(None, "posts")]);
    }

    #[test]
    fn unmapped_reasons_flask_naming_jpql_comments_and_conflicting_bases() {
        let f = facts(
            Language::Python,
            "class P(Remote):\n    class Meta:\n        proxy = True\n\nclass Q(models.Model):\n    pass\n\nclass QP(Q):\n    class Meta:\n        proxy = True\n",
        );
        let got: Vec<(String, Result<TableName, Unmapped>)> =
            mappings("shop/models.py", Language::Python, &f)
                .into_iter()
                .map(|m| (f.definitions[m.class].name.clone(), m.table))
                .collect();
        assert_eq!(
            got,
            [
                ("P".into(), Err(Unmapped::UnknownProxyParent)),
                ("Q".into(), Ok(table(None, "shop_q"))),
                ("QP".into(), Ok(table(None, "shop_q"))),
            ]
        );
        let f = facts(
            Language::Python,
            "class Base(models.Model):\n    class Meta:\n        abstract = True\n\nclass P(Base):\n    class Meta:\n        proxy = True\n",
        );
        let got: Vec<Result<TableName, Unmapped>> =
            mappings("shop/models.py", Language::Python, &f)
                .into_iter()
                .map(|m| m.table)
                .collect();
        assert_eq!(got, [Err(Unmapped::UnknownProxyParent)]);

        for (input, expected) in [
            ("FlaskUser", "flask_user"),
            ("HTTPServer", "http_server"),
            ("My_Table", "my__table"),
            ("_Private", "private"),
            ("order2Item", "order2_item"),
        ] {
            assert_eq!(flask_snake_case(input), expected, "{input}");
        }

        assert_eq!(
            jpql("/* hint */ DELETE FROM Gizmo g WHERE g.x = 1 -- trailing\n"),
            [("Gizmo".to_owned(), Op::Delete)]
        );

        let src = r#"interface R extends JpaRepository<Animal, Long> {
    @Query("SELECT a FROM Animal a WHERE a.kind = " + K.KIND) List<Animal> byKind();
}
package a; interface Gen<X, Y> extends JpaRepository<X, Y> {}
interface Gen<Y, X> extends JpaRepository<X, Y> {}
interface WidgetRepo extends Gen<Long, Widget> {}
"#;
        let f = facts(Language::Java, src);
        let bases = repository_bases(&f.definitions);
        assert_eq!(bases.get("Gen"), None);
        let got: Vec<(String, Target)> = accesses(Language::Java, &f, &bases)
            .into_iter()
            .map(|a| (f.definitions[a.source.unwrap()].name.clone(), a.target))
            .collect();
        assert_eq!(
            got,
            [
                ("R".into(), Target::EntityClass("Animal".into())),
                ("byKind".into(), Target::Entity("Animal".into())),
            ]
        );
    }

    #[test]
    fn same_named_bases_resolved_in_different_rounds_conflict() {
        let src = r#"interface BaseRepo<T, ID> extends JpaRepository<T, ID> {}
interface Mid<ID, T> extends JpaRepository<T, ID> {}
interface BaseRepo<ID, T> extends Mid<ID, T> {}
interface R extends BaseRepo<Customer, Order> { void deleteByX(String x); }
"#;
        let f = facts(Language::Java, src);
        let bases = repository_bases(&f.definitions);
        assert_eq!(bases.get("Mid"), Some(&1));
        assert_eq!(bases.get("BaseRepo"), None);
        assert!(accesses(Language::Java, &f, &bases).is_empty());
    }

    #[test]
    fn python_and_django_table_name_idioms() {
        let src = r#"class Base(db.Model):
    __abstract__ = True

class Employee(Base):
    __tablename__ = 'employees'

class Manager(Employee):
    __tablename__ = None

class AuthUser(models.Model):
    class Meta:
        db_table = '"auth"."users"'

class Legacy(models.Model):
    class Meta:
        db_table = 'billing"."invoices'

class CommonInfo(models.Model):
    class Meta:
        abstract = True
        app_label = 'store'

class Student(CommonInfo):
    class Meta(CommonInfo.Meta):
        ordering = ['x']

class Tabled(models.Model):
    class Meta:
        abstract = True
        db_table = 'shared_tbl'

class Child(Tabled):
    pass
"#;
        assert_eq!(
            mapped("school/models/people.py", Language::Python, src),
            [
                (
                    "Employee".into(),
                    Orm::SqlAlchemy,
                    Some(table(None, "employees")),
                    false
                ),
                (
                    "AuthUser".into(),
                    Orm::Django,
                    Some(table(Some("auth"), "users")),
                    false
                ),
                (
                    "Legacy".into(),
                    Orm::Django,
                    Some(table(Some("billing"), "invoices")),
                    false
                ),
                // `Meta(CommonInfo.Meta)` extends the abstract base's Meta, so app_label carries over.
                (
                    "Student".into(),
                    Orm::Django,
                    Some(table(None, "store_student")),
                    true
                ),
                // Without a Meta of its own, the abstract base's Meta is inherited entirely.
                (
                    "Child".into(),
                    Orm::Django,
                    Some(table(None, "shared_tbl")),
                    false
                ),
            ]
        );
        assert_eq!(django_app_label("deep/models/sub/x.py"), Some("deep"));
        assert_eq!(django_app_label("shop/models.py"), Some("shop"));
    }

    #[test]
    fn jpql_holes_spel_and_versioned_updates() {
        assert_eq!(jpql("SELECT t FROM Thing$1 t"), []);
        assert_eq!(jpql("SELECT e FROM #{#entityName} e"), []);
        assert_eq!(
            jpql("update versioned Thing t set t.x = 1"),
            [("Thing".to_owned(), Op::Update)]
        );
        let models =
            prisma_models("model User {\n  id Int @id\n  @@map(\"users\") @@schema(\"auth\")\n}\n");
        assert_eq!(models[0].table, table(Some("auth"), "users"));
    }

    #[test]
    fn repository_base_cycles_terminate_and_spring_names_are_not_overridden() {
        let src = "interface X<A, B> extends q.Foo<A, B>, JpaRepository<B, Long> {}\ninterface Foo<P, Q> extends r.X<Q, P> {}\ninterface X<C, D> {}\ninterface Loop<T> extends Loop<T> {}\n";
        let f = facts(Language::Java, src);
        let bases = repository_bases(&f.definitions);
        assert_eq!(bases.get("X"), None);
        assert_eq!(bases.get("Loop"), None);

        let src = "interface CrudRepository<ID, E> extends Repository<E, ID> {}\ninterface OwnerRepo extends org.springframework.data.repository.CrudRepository<Owner, Integer> {}\ninterface PetRepo extends CrudRepository<Pet, Integer> {}\n";
        let f = facts(Language::Java, src);
        let bases = repository_bases(&f.definitions);
        // The project's CrudRepository clashes with Spring's: the bare name is dropped.
        assert_eq!(bases.get("CrudRepository"), None);
        let got: Vec<(String, Target)> = accesses(Language::Java, &f, &bases)
            .into_iter()
            .map(|a| (f.definitions[a.source.unwrap()].name.clone(), a.target))
            .collect();
        assert_eq!(
            got,
            [("OwnerRepo".into(), Target::EntityClass("Owner".into()))]
        );
    }

    #[test]
    fn wide_python_hierarchies_resolve_quickly() {
        let mut src = String::from("class C0(models.Model):\n    pass\n");
        for i in 1..40 {
            let bases: Vec<String> = (0..i).rev().map(|j| format!("C{j}")).collect();
            src.push_str(&format!("class C{i}({}):\n    pass\n", bases.join(", ")));
        }
        let started = std::time::Instant::now();
        let f = facts(Language::Python, &src);
        assert_eq!(mappings("app/models.py", Language::Python, &f).len(), 40);
        assert!(started.elapsed().as_secs() < 5, "{:?}", started.elapsed());
    }

    #[test]
    fn flask_models_under_abstract_bases_get_tables() {
        let src = "class Mixin(db.Model):\n    __abstract__ = True\n\nclass User(Mixin):\n    pass\n\nclass Admin(User):\n    pass\n";
        assert_eq!(
            mapped("app/models.py", Language::Python, src),
            // Admin extends a concrete model: single-table inheritance, no table of its own.
            [(
                "User".into(),
                Orm::SqlAlchemy,
                Some(table(None, "user")),
                true
            )]
        );
    }

    #[test]
    fn deep_base_repository_chains_neither_overflow_nor_hang() {
        let mut src = String::from("interface R0<T> extends JpaRepository<T, Long> {}\n");
        for i in 1..3000 {
            src.push_str(&format!("interface R{i}<T> extends R{}<T> {{}}\n", i - 1));
        }
        let f = facts(Language::Java, &src);
        let bases = repository_bases(&f.definitions);
        assert_eq!(bases.get("R400"), Some(&0));
        // Past MAX_BASE_DEPTH a chain is left unresolved rather than recursing further.
        assert_eq!(bases.get("R2999"), None);
    }

    #[test]
    fn base_resolution_does_not_depend_on_names() {
        for (a, b) in [("A", "B"), ("Z", "B")] {
            let src = format!(
                "interface {a}<T> extends {b}<T>, JpaRepository<T, Long> {{}}\ninterface {b}<U> extends {a}<U> {{}}\ninterface {a}<V> extends JpaRepository<V, Long> {{}}\ninterface OwnerViaB extends {b}<Owner> {{}}\n"
            );
            let f = facts(Language::Java, &src);
            let bases = repository_bases(&f.definitions);
            assert_eq!(bases.get(a), Some(&0), "{a}");
            assert_eq!(bases.get(b), Some(&0), "{a}");
        }
    }

    #[test]
    fn deep_and_wide_python_hierarchies_stay_fast() {
        let mut deep = String::from("class C0(Base):\n    pass\n");
        for i in 1..3000 {
            deep.push_str(&format!("class C{i}(C{}):\n    pass\n", i - 1));
        }
        let mut wide = String::from("class W0(Base):\n    pass\n");
        for i in 1..400 {
            let bases: Vec<String> = (0..i).rev().map(|j| format!("W{j}")).collect();
            wide.push_str(&format!("class W{i}({}):\n    pass\n", bases.join(", ")));
        }
        for src in [deep, wide] {
            let f = facts(Language::Python, &src);
            let started = std::time::Instant::now();
            assert!(mappings("app/models.py", Language::Python, &f).is_empty());
            assert!(
                started.elapsed().as_millis() < 1000,
                "{:?}",
                started.elapsed()
            );
        }
    }

    #[test]
    fn long_proxy_and_meta_chains_are_linear_and_do_not_recurse() {
        let mut proxies = String::from("class P0(models.Model):\n    pass\n");
        for i in 1..20000 {
            proxies.push_str(&format!(
                "class P{i}(P{}):\n    class Meta:\n        proxy = True\n",
                i - 1
            ));
        }
        let mut metas = String::from(
            "class M0(models.Model):\n    class Meta:\n        abstract = True\n        app_label = 'deep'\n",
        );
        for i in 1..3000 {
            metas.push_str(&format!(
                "class M{i}(M{p}):\n    class Meta(M{p}.Meta):\n        abstract = True\n",
                p = i - 1
            ));
        }
        for i in 0..3000 {
            metas.push_str(&format!("class K{i}(M2999):\n    pass\n"));
        }
        for (src, expected) in [(proxies, 20000), (metas, 3000)] {
            let f = facts(Language::Python, &src);
            let started = std::time::Instant::now();
            let got = mappings("app/models.py", Language::Python, &f);
            assert!(
                started.elapsed().as_millis() < 1000,
                "{:?}",
                started.elapsed()
            );
            assert_eq!(got.len(), expected);
            assert!(got.iter().all(|m| {
                m.table
                    .as_ref()
                    .is_ok_and(|t| t.name == "app_p0" || t.name.starts_with("deep_k"))
            }));
        }
    }

    #[test]
    fn flask_base_must_be_db_model() {
        let src = "class NotDb(mydb.Model):\n    pass\n\nclass Yes(app.db.Model):\n    pass\n";
        assert_eq!(
            mapped("app/models.py", Language::Python, src),
            [(
                "Yes".into(),
                Orm::SqlAlchemy,
                Some(table(None, "yes")),
                true
            )]
        );
    }

    #[test]
    fn resolution_does_not_depend_on_source_order_and_meta_comes_from_one_base() {
        // Bases defined later in the file (legal when the name is shadowed).
        let src = r#"class C(B):
    class Meta:
        proxy = True

class B(A):
    class Meta:
        proxy = True

class A(models.Model):
    pass

class K(M2):
    pass

class M2(M1):
    class Meta(M1.Meta):
        abstract = True

class M1(models.Model):
    class Meta:
        abstract = True
        db_table = 'shared'

class TblA(models.Model):
    class Meta:
        abstract = True
        ordering = ['x']

class TblB(models.Model):
    class Meta:
        abstract = True
        db_table = 'b_table'

class TwoTbl(TblA, TblB):
    pass
"#;
        assert_eq!(
            mapped("blog/models.py", Language::Python, src),
            [
                ("C".into(), Orm::Django, Some(table(None, "blog_a")), true),
                ("B".into(), Orm::Django, Some(table(None, "blog_a")), true),
                ("A".into(), Orm::Django, Some(table(None, "blog_a")), true),
                ("K".into(), Orm::Django, Some(table(None, "shared")), false),
                // Only the first abstract parent's Meta is inherited, as a whole.
                (
                    "TwoTbl".into(),
                    Orm::Django,
                    Some(table(None, "blog_twotbl")),
                    true
                ),
            ]
        );
    }

    #[test]
    fn reversed_proxy_chains_resolve_every_class() {
        let mut src = String::new();
        for i in (1..20000).rev() {
            src.push_str(&format!(
                "class P{i}(P{}):\n    class Meta:\n        proxy = True\n",
                i - 1
            ));
        }
        src.push_str("class P0(models.Model):\n    pass\n");
        let f = facts(Language::Python, &src);
        let started = std::time::Instant::now();
        let got = mappings("app/models.py", Language::Python, &f);
        assert!(
            started.elapsed().as_millis() < 1000,
            "{:?}",
            started.elapsed()
        );
        assert_eq!(got.len(), 20000);
        assert!(
            got.iter()
                .all(|m| m.table.as_ref().is_ok_and(|t| t.name == "app_p0"))
        );
    }
}
