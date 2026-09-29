# Changelog

## [2026-09-29 17:20]
- M2a, third evaluator pass (6 defects, including a panic); all fixed test-first, and mutation checks confirm each fix is covered by a test.
  - **Splitting** is now done on Postgres lexer tokens alone. This removes the parser/scanner/coverage-check combination and the psql blanking pass.
    - Range start can no longer exceed its end (U+00A0).
    - Non-keyword statements (`USE`, `GO`, garbage) no longer hide the others.
    - `\gset` terminates a statement, and `\copy` is kept as `COPY`.
    - `BEGIN ATOMIC` bodies are tracked by keyword depth.
  - **Placeholders** glued to comparisons are rewritten (`id=?`, `>=?`, `=%(id)s`).
  - **Tree walk:** `FOR UPDATE OF alias` is no longer a table. `UNION` arms' `WITH` clauses are scoped. `PARTITION OF`/`INHERITS` parents, `CREATE FOREIGN TABLE` and `ALTER TABLE SET SCHEMA` are handled.
- Files affected: crates/rb-sql/src/lib.rs, docs/repository-brain-design.md.

## [2026-09-29 16:30]
- M2a, second evaluator pass (9 issues; all fixed test-first, plus one hidden bug found by a new test): `rb-sql` core rebuilt on Postgres' own tools. My hand-written splitter and character scanner are deleted.
  - **Splitting:** statements are split by `split_with_parser`, falling back to `split_with_scanner`. This fixes `BEGIN ATOMIC` bodies and `$` in identifiers.
  - **Coverage check:** if tokens are left uncovered, the whole text is reported. This fixes the lexer silently dropping text with unbalanced parentheses.
  - **Placeholders and meta-commands:** both are handled as lexer tokens. Comments and `E''` strings are safe, and meta-commands work anywhere on a line.
  - **Tables:** found by a full JSON walk of the protobuf tree, with proper CTE scoping. Nested scopes and sibling order fixed; `TABLESAMPLE`, `RETURNING`/`ON CONFLICT`/`MERGE` subqueries, `PREPARE`, cursors, `CREATE RULE` and `RENAME CONSTRAINT` now handled.
  - **Table keys:** parts containing `.` or `"` are quoted.
  - **Diagnostics:** one `unparsed_sql` per sink call site, with `failed_statements`.
- Mutation checks confirm each fix is covered by a test.
- Files affected: crates/rb-sql/{Cargo.toml,src/lib.rs}, crates/rb-indexer/src/sql.rs, docs/repository-brain-design.md, goldens.

## [2026-09-29 15:40]
- M2a fixes after the evaluator pass and the S16 measurement, each test-first.
- `rb-sql` rewritten:
  - **Own statement splitter:** handles quotes, `E''`, `$tag$`, nested comments and `psql` meta-commands.
  - **Per-statement parse:** one bad statement no longer hides the others, and ranges always refer to the original text. This fixes a crash.
  - **Parse-tree walk:** handles CTE shadowing, nested DML operations, dotted quoted names, `COPY` direction, `RENAME`, `SELECT INTO`, and `ALTER TYPE` as not a table. Unqualified `pg_*` names are system catalogs.
  - **Token-bounded placeholders:** covers `?`, `:name`, `%s` and `%(name)s`.
- `rb-syntax`: `\x`, `\u`, `\u{}`, `\U` and octal escapes are decoded, and keywords must be whole words.
- Indexer:
  - a BOM in `.sql` files is handled with raw-byte spans;
  - each bad statement becomes its own diagnostic;
  - `createQuery` (JPQL) is skipped and `createNativeQuery` is a sink.
- Store: distinct-site SQL coverage with an honest label, and case-insensitive table lookup.
- Files affected: crates/rb-sql/src/lib.rs, crates/rb-syntax/src/strings.rs, crates/rb-indexer/src/sql.rs, crates/rb-store/src/query.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, docs/repository-brain-design.md, tasks/contract-m2a.md, IMPLEMENT.md, README.md.

## [2026-09-29 14:20]
- M2a step 4: `rb table-deps <table>` (text or `--json`).
  - It lists migrations and accessing code grouped by service, with ops, certainty, schema assumption and file:line.
  - A bare name that matches several schemas lists the candidates and exits non-zero.
  - `rb status` shows SQL coverage (tables, query sites, migration statements, unparsed), derived from the snapshot.
  - The polyglot fixture gained Java `JdbcTemplate`, Node `pg`, Go `database/sql` and migrations.
  - New goldens: table-deps (3), status-sql.
- Files affected: crates/rb-store/src/{lib,query}.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-model/src/lib.rs, fixtures/polyglot-monorepo/*, crates/rb-cli/tests/golden/polyglot-monorepo/*.

## [2026-09-29 13:40]
- M2a step 3: the indexer adds `Table` nodes.
  - `MIGRATES` (File → Table, provenance `sql_file`) for every statement in `.sql` files.
  - `QUERIES` (Symbol or File → Table, provenance `sql_literal`) for SQL candidates. They are aggregated per (source, table), with the union of `ops`, the best certainty, and every site as evidence. Sink calls give `exact`; other candidates give `heuristic`.
  - `unparsed_sql` is reported only for unparseable `.sql` files and sink arguments.
  - Statement ranges now start at the first token, skipping leading comments.
  - New provenance: `sql_file`.
- Files affected: crates/rb-indexer/src/{lib,sql}.rs, crates/rb-sql/src/lib.rs, crates/rb-model/src/lib.rs, crates/rb-cli/tests/golden/polyglot-monorepo/dump.jsonl.

## [2026-09-29 13:05]
- M2a step 2: `rb-syntax` extracts SQL candidates, meaning SQL-looking string expressions. They are cached in `FileFacts.sql` with the normalised text, span, enclosing definition and callee name.
  - Covers TS/JS template holes and tagged templates, Java text blocks (dedented), Python f-strings and implicit concatenation, Go raw strings, and `+` concatenation.
  - Escapes are decoded.
  - Annotations and decorators are skipped.
  - `rb-sql` retries JDBC `?` and named `:param` placeholders as `$n`.
- Files affected: crates/rb-syntax/src/{lib,strings}.rs, crates/rb-sql/src/lib.rs, tasks/contract-m2a.md.

## [2026-09-29 12:20]
- M2a step 1: new `rb-sql` crate. It uses `pg_query` 6.2 (the real PG17 parser) to split SQL into statements and report each table with its operations: read, insert, update, delete, merge, create, alter, drop, truncate.
  - CTE names are excluded and system schemas are ignored.
  - Unqualified names resolve to `public` and are flagged `schema_assumed`.
  - Parse errors are returned, never guessed.
- Files affected: crates/rb-sql/*, Cargo.toml, tasks/contract-m2a.md.

## [2026-09-29 11:30]
- M1b review fixes, each test-first:
  - a Go method whose receiver type is declared in its own body crashed the process (a parent cycle). Methods now attach only to top-level types, and key building is iterative and cycle-safe;
  - a Go method no longer attaches to a same-named type local to another function;
  - nesting is capped at 64 levels (`definitions_too_deep` diagnostic), so key size stays linear;
  - TypeScript `namespace`, type aliases and enums are now definitions;
  - definitions inside anonymous functions (callbacks, lambdas, anonymous classes, Go func literals) are locals, not symbols. Previously unrelated test-local types merged into one symbol.
- Cache correctness: the facts cache key is now `rb_syntax::extractor_id()`, which combines the grammar versions with a fingerprint of the bundled queries and the extractor source, so stale facts can't be served after an edit.
- C18 moved to public repos (spring-petclinic, hono, gin, requests). The two private repos previously used for C18 were deleted from the Brain, and their references removed from IMPLEMENT.md and the M1b contract.
- Files affected: crates/rb-syntax/src/{lib,keys,queries}.rs, crates/rb-indexer/src/syntax.rs, crates/rb-cli/tests/e2e.rs, IMPLEMENT.md, tasks/contract-m1b.md, docs/repository-brain-design.md, README.md.

## [2026-09-29 10:25]
- M1b step 3: syntax facts are cached in `rb.file_facts_cache`, keyed by blob plus extractor version and language, through `Store::load_facts`/`save_facts` and a CLI `StoreCache` adapter.
  - `rb index` reports `parsed N source files`.
  - New `rb index --no-cache` bypasses the cache.
  - The index subcommand's arguments are now an `IndexArgs` struct.
  - New e2e test: a warm cache parses 0 files and dumps byte-identically to a cold run.
- Files affected: crates/rb-store/src/lib.rs, crates/rb-cli/src/main.rs, crates/rb-cli/Cargo.toml, crates/rb-cli/tests/e2e.rs.

## [2026-09-29 10:05]
- M1b step 2: the indexer turns definitions into `Symbol` nodes with `DEFINED_IN` (evidence: the name span) and `CONTAINS` edges.
  - Files are chosen for parsing by language, binary/generated flags and a 1 MiB cap (over the cap gives an `extraction_skipped` diagnostic).
  - Parsing runs in parallel on `std::thread::scope`, in batches of 512.
  - Syntax errors become diagnostics.
  - A `FactsCache` trait keeps the indexer I/O-free.
- The pipeline version is now `m1b-syntax-1`. New golden: `find-login-symbols.txt`. The dump golden includes symbols.
- Files affected: crates/rb-indexer/src/{lib,syntax}.rs, crates/rb-syntax/src/lib.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-cli/tests/golden/polyglot-monorepo/*.

## [2026-09-29 09:40]
- M1b step 1: new `rb-syntax` crate. It parses TypeScript, TSX, JavaScript, Java, Python and Go with Tree-sitter 0.27 and extracts definitions (with parents) and references (with their enclosing definition). It also derives heuristic symbol keys in SCIP shape.
  - Java gets an extra query for enums, records and constructors.
  - TS/TSX run both the JS and TS tag queries.
  - Go methods attach to their receiver type.
  - 15 tests cover these, and mutation checks confirm they catch regressions.
- Files affected: crates/rb-syntax/*, Cargo.toml.

## [2026-09-29 08:55]
- Engineer review of M1a:
  - service `role` (service, library, unassigned) replaces the deployable and synthetic flags;
  - stored `path` and owner `type` removed (both derivable);
  - `project_for` uses binary search, with sorting enforced by `Boundaries::new`;
  - explicit `rb.toml` name override.
- Evaluator fixes, each with a regression test:
  - `docs/*/` and `*/` now match;
  - a literal `*` in a filename no longer breaks wildcard matching;
  - a directory named `(root)` can no longer capture unassigned files (unassigned files now live on `.`);
  - CRLF and BOM handled in CODEOWNERS;
  - BOM handled in manifests;
  - non-UTF-8 paths are skipped instead of aborting the run;
  - `/* c */ package main` is detected;
  - all text ordering is bytewise (`COLLATE "C"`), not locale-dependent;
  - owners are lowercased;
  - `rb find --limit` is range-checked;
  - `pom.xml` with a DOCTYPE parses.
- Files affected: crates/rb-git/src/{glob,codeowners,boundaries,lib}.rs, crates/rb-indexer/src/lib.rs, crates/rb-model/src/{lib,manifest}.rs, crates/rb-store/src/{lib,query}.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, docs/repository-brain-design.md.

## [2026-09-29 08:10]
- M1a structure: CODEOWNERS owners with evidence, service boundary detection with `rb.toml`, and the `rb find` and `rb service` commands. Edges now carry evidence, and diagnostics keep their path (migration 0002).
- Fixed pg_trgm being installed into AGE's `ag_catalog` schema (migration 0003). Migrations now pin `search_path = public`.
- Files affected: crates/rb-git/src/{glob,codeowners,boundaries,lib}.rs, crates/rb-indexer/*, crates/rb-store/src/{lib,query}.rs, crates/rb-model/src/*, crates/rb-cli/*, migrations/0002-0003, fixtures/polyglot-monorepo.

## [2026-09-29 07:25]
- Initial commit a8f5ac2: the design doc and M0 foundations. This covers the Rust workspace, Postgres 18 + AGE 1.8.0 in compose, snapshot publishing in one transaction, and the `rb migrate`, `index`, `status`, `dump` and `cypher` commands.
