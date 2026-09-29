# M2a contract: SQL core (tables, migrations, queries in code)

Written by the planner before any code, 2026-09-29. M2a is done only when every assertion below is backed by a passing test or a recorded measurement.

## Scope

Parse SQL with the real Postgres parser (`pg_query` 6.2, which embeds the PG17 parser). This yields:
- `Table` nodes;
- `MIGRATES` (File → Table) for statements in `.sql` files;
- `QUERIES` (Symbol → Table, or File → Table for module-level code) for SQL string literals in code;
- the new `rb table-deps <table>` command.

ORM mappings (JPA, Spring Data, Prisma, TypeORM, SQLAlchemy, JPQL) are M2b. The `sqlparser-rs` dialect fallback is out of scope: non-Postgres SQL becomes an `unparsed_sql` diagnostic, never a guess.

Research findings:
- `pg_query::parse` gives per-statement byte ranges, statement types, and `select_tables` / `dml_tables` / `ddl_tables`.
- CTE names are excluded from the table lists.
- A placeholder in table position (`FROM $1`) is a parse error.
- The table lists come back in hash order, so they must be sorted.

## Engineer declaration

**Canonical state:**
- a `.sql` blob's statements;
- a source file's SQL *candidates*: string literals that look like SQL, after template normalisation, with their span, enclosing definition and callee name. Candidates are extractor output, cached in `FileFacts`.

**Derived:**
- per statement, `(table, operations)`. The SQL analysis is a pure function and is not cached; it is cheap.
- `Table` nodes: the union of every table referenced.
- `QUERIES` / `MIGRATES` edges: aggregated per (source, table), with the union of operations as the attribute, so no edge is ever added twice with different attributes.
- SQL coverage (candidates, parsed, unparsed): computed at query time from diagnostics and evidence, never stored.

**Invariants:**
1. Every `Table` node has at least one `QUERIES`, `MIGRATES` or (later) `MAPS_TO` edge.
2. Table keys are `schema.table`, lowercase unless quoted, with unqualified names resolved to `public`. Resolution to the default schema is recorded as `schema_assumed` on the edge.
3. An SQL string that doesn't parse produces a diagnostic, never an edge.
4. The same commit yields the same dump; warm cache = cold run.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | For each statement, `rb-sql` reports the tables and operations (`read`, `insert`, `update`, `delete`, `merge`, `create`, `alter`, `drop`, `truncate`), sorted | unit tests |
| S2 | `INSERT … SELECT`: the target gets `insert`, the sources get `read`. `UPDATE … FROM`: the target gets `update` and the other table gets `read`. CTE names are never tables. | unit |
| S3 | Qualified names keep their schema. Unqualified names resolve to `public` and are flagged `schema_assumed`. Quoted identifiers keep their case; unquoted ones fold to lowercase (Postgres semantics). | unit |
| S4 | System schemas (`pg_catalog`, `information_schema`) are ignored | unit |
| S5 | Unparseable SQL returns an error with the parser message. JDBC `?` and named `:param` placeholders (outside quoted text, never `::` casts) are rewritten to `$n` in a single retry, accepted only if the retry parses. | unit |
| S6 | `.sql` files: every statement becomes `MIGRATES` (File → Table), with `ops` as an attribute and each statement's line span as evidence | indexer unit + dump golden |
| S7 | Code literals: TS/JS template strings with `${…}` holes, Python f-strings, and `+` concatenation in Java/JS/TS/Go/Python are normalised; each hole becomes `$n` | rb-syntax unit, per language |
| S8 | A candidate is only a string whose normalised text starts (case-insensitive, after whitespace and comments) with SELECT, INSERT, UPDATE, DELETE, WITH, MERGE, CREATE, ALTER, DROP or TRUNCATE. Other strings are not stored in facts. | unit |
| S9 | A literal that is an argument of a call whose callee name is a known SQL sink (case-insensitive: `query`, `queryrow`, `querycontext`, `queryrowcontext`, `exec`, `execcontext`, `execute`, `executemany`, `prepare`, `preparecontext`, `queryforobject`, `queryforlist`, `queryformap`, `update`, `batchupdate`, `raw`, `$queryraw`, `$executeraw`, `$queryrawunsafe`, `$executerawunsafe`, `sql`, `text`; direct and tagged-template calls) gives certainty `exact`; any other parsed candidate gives `heuristic` | indexer unit |
| S10 | A literal inside a definition gives `QUERIES` from that symbol, including a module-level constant such as Python `Q = 'SELECT …'`, whose assignment is the definition. A literal outside every definition gives `QUERIES` from the File. | indexer unit |
| S11 | A table referenced several times by one source gives one `QUERIES` edge with the union of operations, and every site as evidence | indexer unit |
| S12 | A candidate that does not parse never gives an edge. It gives an `unparsed_sql` diagnostic (span plus parser message) only when it is a sink argument; other failures are dropped silently, since they are almost always prose such as "Update failed". A hole in table position (``` `SELECT * FROM ${t}` ```) is the typical sink case. | indexer unit |
| S13 | `rb table-deps <table>` accepts `users`, `public.users` or `auth.users`. It lists the migrations (files, services, ops) and the accessing symbols grouped by service, each with ops, certainty and file:line. An ambiguous name lists the candidates and exits non-zero. | e2e golden on `polyglot-monorepo` |
| S14 | `rb status` shows SQL coverage (candidates parsed / unparsed), computed from the snapshot | e2e |
| S15 | AGE parity, determinism, and warm-cache = cold still hold with `Table` nodes and the new edges | existing e2e tests |
| S16 | Public repos at pinned commits index without panics. Their table counts are sanity-checked against `grep` for `CREATE TABLE` and against a hand-count of SQL literals in a sample. | recorded measurement |
| S17 | Only `pg_query` is added as a dependency | review |
| S18 | `fmt --check`, `clippy -D warnings`, the full test suite and the stub scan are all clean | verification loop |

## Amendments (planner, 2026-09-29, before step 2)

- **S5:** retry with placeholders rewritten, because JDBC and SQLAlchemy code would otherwise never parse.
- **S12:** noise control, because the prefilter admits prose.
- **Cache fingerprint:** the new candidate-extraction source file must be added to `rb_syntax::extractor_id`'s fingerprint.
