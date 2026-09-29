# M2b contract: ORM mappings (draft, planner, 2026-09-29)

Written before any code. M2b is done only when every assertion below is backed by a passing test or a recorded measurement.

## Scope

Connect ORM code to tables.
- **Full vertical (mapping plus usage):**
  - JPA entities;
  - Spring Data repositories;
  - `@Query` (native SQL and JPQL);
  - JPQL in `createQuery`;
  - Prisma schema plus Prisma client calls.
- **Mapping only (usage is a later milestone):** TypeORM `@Entity`, SQLAlchemy `__tablename__`, and Django models.

Design (§4.5 framework packs):
- **Generic syntax facts:** `rb-syntax` extracts per definition its *decorations*, meaning Java annotations and Python/TypeScript decorators with their literal arguments, and its *supertypes*, the `extends`/`implements`/base-class text including type arguments. Both are cached in `FileFacts`.
- **Framework packs:** a new crate, `rb-frameworks`, holds pure functions that turn those generic facts (plus `schema.prisma` text) into framework facts.
- **Indexer:** turns framework facts into `MAPS_TO` and `QUERIES` edges (provenance `orm`).

## Engineer declaration

**Canonical state** (all cached extractor output):
- decorations and supertypes per definition;
- `schema.prisma` models.

**Derived:**
- entity → table (naming rules);
- repository → entity (from the type argument);
- method name → operations;
- edges.

**Invariants:**
1. Every `MAPS_TO` edge's source is a class-like `Symbol` and its target a `Table`.
2. A table name that comes from a naming rule rather than an explicit name is flagged `name_assumed`, and its certainty is at most `synthetic`.
3. JPQL never creates a table directly. It resolves entity names through `MAPS_TO`, and an unknown entity becomes a diagnostic.
4. Determinism and warm cache = cold still hold.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| O1 | `rb-syntax` extracts decorations (name and literal arguments, positional and named) and supertypes (text and type arguments) for Java, Python and TS, attached to the right definition | rb-syntax unit, per language |
| O2 | JPA `@Entity` plus `@Table(name, schema)` gives `MAPS_TO` class → `schema.name` (exact). Without `@Table`, the Spring Boot physical naming (camelCase → snake_case, lowercase) is used, flagged `name_assumed` (synthetic). | unit + fixture |
| O3 | Spring Data: an interface extending `Repository`/`CrudRepository`/`ListCrudRepository`/`PagingAndSortingRepository`/`JpaRepository` with `<E, ID>` binds to entity E. Declared derived-query methods give `QUERIES` method → table: `find/read/get/query/stream/count/exists…By` is read, `delete/remove…By` is delete. The interface itself gets `QUERIES` with the inherited CRUD operations (read, insert, update, delete). All synthetic. | unit + fixture |
| O4 | `@Query(value = "...", nativeQuery = true)` gives `QUERIES` from the method via `rb-sql` (exact). Plain `@Query` is JPQL (O5). | unit |
| O5 | JPQL (in `@Query` or `createQuery`): entity names after FROM/JOIN/UPDATE/DELETE FROM resolve through entity mappings to tables, with ops from the statement keyword, certainty heuristic. An unresolved entity gives an `orm_unresolved_entity` diagnostic (named `jpql_unresolved_entity` in the first draft). | unit |
| O6 | Prisma: `model X { … @@map("x") @@schema("s") }` gives a `Table` plus a `MAPS_TO` from a model `Symbol` defined in `schema.prisma`. Client calls `prisma.<model>.<op>(…)` give `QUERIES` from the enclosing symbol, with ops from the method (find/count/aggregate/groupBy = read, create = insert, update = update, upsert = insert+update, delete = delete), synthetic. | unit + fixture (`ts-express-prisma`) |
| O7 | TypeORM `@Entity('name')`, SQLAlchemy `__tablename__`, and Django `Meta.db_table` (or `<app>_<model>`, flagged `name_assumed`) give `MAPS_TO` | unit |
| O8 | `rb table-deps` shows a "Mapped by" section (entities/models) and includes ORM accesses in "Accessed by" with their certainty | e2e golden |
| O9 | Public repos at pinned commits (spring-petclinic: JPA; one Prisma app; one SQLAlchemy/Django app): no panics; every `@Entity` class maps to exactly one table; the entity count equals `grep @Entity` | recorded measurement |
| O10 | fmt, clippy, tests and the stub scan are clean; a separate evaluator pass finds no open defects; no new dependency without a stated reason | verification loop |

## Amendment 1: evaluator pass 1 (13 defects)

**Canonical state added** (cached syntax facts):
- `DecorationArg::literal` and `ClassConstant::literal`: whether the value is a static string. Concatenations of literals are flattened.
- `Definition::type_params`.

**Derived, and where each is derived:**
- *Physical names* are computed in `rb-frameworks`.
  - JPA uses Hibernate's `PhysicalNamingStrategySnakeCaseImpl`: it applies to explicit names as well; quoted names (`"x"` or `` `x` ``) are kept as written; unquoted names are snake_cased and lowercased.
  - The other ORMs quote their names, so their case is kept.
  - The indexer no longer folds case.
- *In-file Python resolution:*
  - Django: `abstract` classes have no table; a `proxy` uses its parent's table; `app_label` is honoured; subclasses of models in the same file are models, including multi-table inheritance.
  - `db.Model` is Flask-SQLAlchemy, snake_case, `name_assumed`.
- *Cross-file resolution* (in the indexer):
  - JPA SINGLE_TABLE inheritance: a subclass entity maps to its root entity's table unless the root declares `@Inheritance(strategy = JOINED | TABLE_PER_CLASS)`. Hibernate ignores `@Table` on a SINGLE_TABLE subclass (HHH000139).
  - Spring Data generic base repositories are resolved as a fixpoint of "interface → index of its entity type parameter".
  - Prisma accessors resolve against the nearest schema root. The root is the schema's directory, minus a trailing `schema` segment and then a trailing `prisma` segment. With no enclosing root, every schema is a candidate. Several distinct tables make the result `ambiguous`.

**Invariants added:**
5. A table name that is not a static string never becomes a table. It becomes an `orm_dynamic_table_name` diagnostic.
6. Indexing never fails because two declarations share a symbol key. `MAPS_TO` edges are aggregated per (symbol, table): best certainty, `name_assumed` only if every declaration assumes it, and every declaration's evidence.
7. JPQL entities come only from clause-level FROM and JOIN, at the top level or inside a subquery. Function arguments, `IN(…)` and `TREAT(…)` are never entities.
8. A Prisma client call counts only when the segment before the model accessor contains `prisma`, or is `db`, `tx`, `client`, `trx` or `database`.
9. A native `@Query` that `rb-sql` cannot parse gives an `unparsed_sql` diagnostic (callee `Query`).

## Amendment 2: evaluator pass 2 (the 13 earlier defects are verified fixed; 11 new code defects)

- **Python class constants:** keep any right-hand side. Anything other than a static string or boolean is non-literal, so `db_table = TABLE`, `.format(...)` and `%` give `orm_dynamic_table_name`. String values unwrap parentheses.
- **Spring Data type arguments** are *class* names (`Target::EntityClass`, resolved by the mapped class's simple name). JPQL names are *entity* names (`Target::Entity`).
- **JPA inheritance** walks through every Java class, so `@MappedSuperclass` and plain classes in between don't end it. The root is the topmost entity reached. A superclass name resolves to the only class with that name, else the only one in the subclass's directory; otherwise the walk stops.
- **Non-literal JPQL** is parsed from its flattened text; the `$n` holes never read as entities. JPQL skips `/* */` and `--` comments.
- **Generic base repositories:** interfaces that share a simple name but bind different parameters are dropped, not guessed.
- **JPA schema:** a snake-cased schema flags `name_assumed`.
- **Django:**
  - only `models.Model` and `django.db.models.Model` count; bare `Model` is not recognised until import facts exist;
  - a class without its own `Meta` inherits `app_label` from an abstract base;
  - a `proxy` without a concrete parent in the file gives `orm_unresolved_proxy_parent`.
- **Flask-SQLAlchemy** has its own naming (`flask_snake_case`, verified against `camel_to_snake_case`): `My_Table` becomes `my__table`. TypeORM's `snakeCase` (0.3.20) differs.
- **Prisma:** an accessor resolves to the schemas whose directory shares the most leading path segments with the file; ties are ambiguous. This replaces the root-stripping rule, which got a root schema plus `apps/api/db/schema.prisma` wrong.
- **Naming source:** Spring Boot `main` (4.x) defaults to `PhysicalNamingStrategySnakeCaseImpl`. Boot 3.x used `CamelCaseToUnderscoresNamingStrategy`, which also lowercases quoted names; that version difference is accepted.

## Amendment 3: evaluator pass 3 (the 11 pass-2 defects verified fixed; 3 contract violations and 8 lower-severity defects)

**Fixed:**
- **Generic base repositories:** bindings are re-derived from scratch every round, so same-named bases found in different rounds still conflict and are dropped. Before, the later one silently picked the wrong entity.
- **Native `@Query` built from non-literal text:** tables whose name contains a `$n` hole are never created (`orm_dynamic_table_name`). Its static tables drop to `heuristic`.
- **JPA inheritance** follows only `extends`. `Supertype::implements` marks Java `implements` and TypeScript `implements`.
- **Flask-SQLAlchemy:** `__abstract__ = True` gets no table.
- **SQLAlchemy:** `__tablename__ = None` (a single-table-inheritance subclass) gets no mapping and no diagnostic.
- **Django:**
  - `db_table` in the `'"schema"."table"'` / `'schema"."table'` idioms gives `schema.table`;
  - `class Meta(Base.Meta)` extends an abstract base's Meta; a class without a `Meta` inherits the whole abstract Meta (`app_label` and `db_table`, never `abstract`);
  - the app label is the directory above the last `models` package.
- **JPQL:** words holding a `$n` hole are not entities; SpEL (`#{#entityName}`) is skipped; HQL `UPDATE VERSIONED`.
- **Prisma** reads block attributes that share one line.

**Known gaps, accepted for M2b:**
- A native `@Query` declared on a generic base repository is not attributed to the concrete repositories.
- A `models.py` at the repository root has no app label, so it gets no mapping.
- Django model chains deeper than 16 classes stop resolving.
- Django doesn't truncate names longer than 63 characters (Django appends an md5 prefix; there is no hashing dependency yet).
- `#{#entityName}` is skipped rather than replaced by the repository's entity.
- **Prisma "most shared leading segments":** a file in `apps/api/src` prefers `apps/api/src/legacy/prisma` over `apps/api/prisma`. Revisit together with M3 module resolution.
- **Outside M2b:** the SQL-literal stage (M2a) still accepts a table name glued to a `$n` hole, at `heuristic` certainty. It is tracked for the next SQL pass.

## Amendment 4: evaluator pass 4 (D1–D3 verified fixed; 2 contract violations and 3 lower-severity defects)

**Fixed:**
- **Repository bases:** the round-based fixpoint could oscillate forever on legal code (same-named interfaces in a cycle). I had claimed it terminated, and that claim was wrong. It is replaced by memoised resolution of each name, with a cycle guard (a name on the current path resolves to nothing) and a depth cap of 512.
  - A name binds only when **every** declaration of it binds the same parameter; a built-in base counts as a declaration binding 0. So a project `CrudRepository<ID, E>` makes the bare name ambiguous, and it is dropped.
  - Supertypes written `org.springframework.data.…` always mean the built-in base.
- **`$n` hole filtering** applies only to non-literal native SQL. `pg$1tab` in a static query is a real table.
- **Python hierarchy walks** (Django model detection, `Meta` inheritance, proxies) visit each class once, using visited sets instead of the old depth cap and repeated recursion. Wide multiple inheritance is linear, not exponential.
- **Flask-SQLAlchemy:** a class reaching `db.Model` only through `__abstract__` bases gets `camel_to_snake_case(class)`. A subclass of a concrete model shares its table (single-table inheritance) and gets no mapping.

**Known limitation:** past the depth cap of 512, whether a name in the middle of a chain resolves depends on which name is resolved first. The order is fixed (sorted names), so results stay deterministic.

## Amendment 5: evaluator pass 5 (all 5 pass-4 fixes verified; 2 lower-severity defects)

**Fixed:**
- **Repository-base results no longer depend on interface names.** A name's result is cached only once no cycle it was computed around is still pending. Results computed inside an unfinished cycle are recomputed. A budget of 100,000 steps bounds recomputation in pathological cyclic code; past it, results are cached as computed.
- **Python mapping is linear.** The claim in Amendment 4 was false: the walks were cubic.
  - `PyClasses` builds each file's same-scope base links and `Meta` children once.
  - Django-model and Flask-model membership come from one reverse breadth-first walk each.
  - `Meta` inheritance is an iterative walk with a visited set.
  - Only class definitions are considered, for every language.
  - Cost: 3000-deep chains and 400-wide multiple inheritance each take under 1 s in a debug build; they took 198 s before.

## Amendment 6: evaluator pass 6 (no contract violations; 3 lower-severity defects, 2 minor)

**Fixed:**
- **E1:** Django table and `Meta` inheritance resolution are now one iterative, memoised pass per file (`first_along`). Proxy chains no longer recurse, so a 20,000-deep chain neither overflows nor slows down. N children of an N-deep `Meta(Base.Meta)` chain are linear. Both are pinned by a timing test.
- **E4 (Flask):** a Flask base must be `db.Model` or end in `.db.Model`; `mydb.Model` no longer matches.

**Accepted limitations (need import-aware name resolution, M3):**
- **E2:** base-repository results can still depend on which name is resolved first. This happens only for programs javac rejects, such as inheriting `JpaRepository` twice with different arguments. A randomized search over 2,294 legal hierarchies under every name permutation found no order dependence.
- **E3:** a legal cycle that exists only because two interfaces share a simple name (for example `p.A extends r.A`) drops every name in the cycle, even when a consistent binding exists. Nothing wrong is emitted; edges are lost.
- **Enums and records:** JPA `@Entity` on a Java `enum` or `record` still maps. JPA forbids both, but they are class-like, so invariant 1 holds.
- **Python bases:** only same-scope base classes are resolved. A nested class extending a module-level model is not mapped.

## Amendment 7: evaluator pass 7 (PASS with 2 lower-severity reservations, both fixed)

- **`first_along` revisits a base once it is resolved,** instead of skipping past it. Results no longer depend on whether a base is defined before or after the class (shadowed names). A 20,000-class children-first proxy chain is pinned by a test.
- **Django `Meta` inheritance:** a class without its own `Meta` inherits the **whole** `Meta` of its first abstract parent. Options are no longer merged across parents; this is Django's documented rule.

**M2b is closed.** Every O1–O10 assertion and every invariant 1–9 holds. The open items are the accepted limitations in Amendments 3, 4 and 6, which are carried into M3 (import-aware name resolution).
