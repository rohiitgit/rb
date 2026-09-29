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
| O5 | JPQL (in `@Query` or `createQuery`): entity names after FROM/JOIN/UPDATE/DELETE FROM resolve through entity mappings to tables, with ops from the statement keyword, certainty heuristic. An unresolved entity gives a `jpql_unresolved_entity` diagnostic. | unit |
| O6 | Prisma: `model X { … @@map("x") @@schema("s") }` gives a `Table` plus a `MAPS_TO` from a model `Symbol` defined in `schema.prisma`. Client calls `prisma.<model>.<op>(…)` give `QUERIES` from the enclosing symbol, with ops from the method (find/count/aggregate/groupBy = read, create = insert, update = update, upsert = insert+update, delete = delete), synthetic. | unit + fixture (`ts-express-prisma`) |
| O7 | TypeORM `@Entity('name')`, SQLAlchemy `__tablename__`, and Django `Meta.db_table` (or `<app>_<model>`, flagged `name_assumed`) give `MAPS_TO` | unit |
| O8 | `rb table-deps` shows a "Mapped by" section (entities/models) and includes ORM accesses in "Accessed by" with their certainty | e2e golden |
| O9 | Public repos at pinned commits (spring-petclinic: JPA; one Prisma app; one SQLAlchemy/Django app): no panics; every `@Entity` class maps to exactly one table; the entity count equals `grep @Entity` | recorded measurement |
| O10 | fmt, clippy, tests and the stub scan are clean; a separate evaluator pass finds no open defects; no new dependency without a stated reason | verification loop |
