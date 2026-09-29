# IMPLEMENT

This file traces decisions from conversation to code. The design lives in `docs/repository-brain-design.md`, and task progress in `tasks/todo.md`.

## Working rules (agreed 2026-09-29)

- The `loop` rules take priority over the older "pause after each phase" rule: the agent decides and continues without waiting.
- The `engineer` rules apply to all code.
- Every batch runs plan → tests first → implement → separate evaluator agent → verify → commit.
- **Backups:** a project zip goes in `archive/` (gitignored) before each major change, and a git commit follows each verified batch. Per-file `.backup` copies are deliberately not used, because fixture tests copy whole directories and stray files would change the golden dumps.
- **Verification:** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55432/rb cargo test --workspace`.

## M0 Foundations: complete (commit a8f5ac2)

- **Decided:** Rust, Postgres 18 + AGE 1.8.0, single-transaction snapshot publish, canonical dump.
- **Built:** the rb-model, rb-git, rb-store and rb-cli crates, migration 0001, and 2 fixtures.

## M1a Structure: complete

- **Decided:**
  - every project is a `Service`;
  - the service key is the project directory;
  - CODEOWNERS follows GitHub semantics, with an in-house glob matcher;
  - edges carry evidence;
  - the new `rb find` and `rb service` commands.
- **Built:**
  - the rb-git modules `glob`, `codeowners` and `boundaries`;
  - the rb-indexer crate;
  - rb-store `query`;
  - migrations 0002 and 0003;
  - the `polyglot-monorepo` fixture and 7 goldens.

### Engineer declaration

**Canonical state**, the facts read from the commit:
- files, as (path, blob sha, size, flags);
- the deciding CODEOWNERS file and its rules;
- the projects detected from manifests, plus `rb.toml`.

**Derived** (computed, not stored separately):
- file → service: the innermost project;
- file → owners: the last matching rule;
- service → owners: the owners of the anchor file;
- card aggregates (language counts, owner shares, unowned count), computed at query time;
- owner type (`team`/`user`/`email`), from the owner string. It should not be stored.
- service path, which equals the service key. It should not be stored twice.

**Invariants:**
1. Every `File` has exactly one `IN_SERVICE` edge.
2. `OWNED_BY` evidence always points at the CODEOWNERS file that was used.
3. The synthetic `(root)` service exists iff some file lies outside every project.
4. Service keys are unique directories.
5. The same commit and tools give a byte-identical dump.

### Review findings (engineer self-review, 2026-09-29)

| # | Finding | Fix |
|---|---|---|
| 1 | Service `attrs.path` duplicates the node key | remove it |
| 2 | `deployable` + `synthetic` booleans, and the same 2×2 match written twice in the CLI | a single `role` attr: `service` \| `library` \| `unassigned` |
| 3 | Owner `attrs.type` is derivable from the key | remove it |
| 4 | `Boundaries::project_for` builds a map on every call (O(files × projects)) | binary search over the sorted projects |
| 5 | `rb.toml` names were stored under detector priority 0, with a comment explaining the hack | an explicit `override_name` field |
| 6 | Invariant 1 was only checked on a 4-file unit test | check it in the e2e test for every fixture |

**Kept on purpose:** `lang` on File nodes. It is extractor output (content-based detection is coming), and SQL aggregates over it.

### Status

- [x] Fixes 1–6, written tests first
- [x] Evaluator review of the full M1a diff: 9 confirmed defects, all fixed with regression tests. Also fixed 3 of its 5 unconfirmed suspicions (owner case, `--limit`, DOCTYPE) and proved one more by test (migration 0003). The last one, `/*` being root-only, was kept on purpose and documented.
- [x] Commit (see git log)

## M1b Syntax: complete

- **Decided:** see `tasks/contract-m1b.md`.
  - The extractor goes in a new `rb-syntax` crate.
  - Keys use the file path, not the project, so a key depends only on its own file.
  - References are cached but not emitted as edges.
  - Parallelism uses `std::thread::scope` (no rayon).
  - The facts cache stores JSON, not bincode.
  - `gix` change sets are dropped; the blob-keyed cache makes them redundant until M3.
- **Built:**
  - `rb-syntax`: queries, extraction, keys;
  - the `rb-indexer::syntax` stage;
  - `Store::load_facts`/`save_facts`;
  - `rb index --no-cache` and the parse count.
- **Measured (C18):** public repos at pinned commits, release build, 2026-09-29.

  | Repo @ commit | Files parsed | Cold | Warm | Symbols | Diagnostics |
  |---|---|---|---|---|---|
  | spring-projects/spring-petclinic @ 818c4136ea97 | 50 | 0.82 s | 0.10 s | 240 | 0 |
  | honojs/hono @ 18331a905e24 | 441 | 0.33 s | 0.23 s | 1,698 | 8 syntax errors |
  | gin-gonic/gin @ 1bd0ecf2d3e0 | 98 | 0.19 s | 0.21 s | 1,522 | 0 |
  | psf/requests @ 611c6162cbc4 | 37 | 0.17 s | 0.16 s | 911 | 2 skipped symlinks |

  Every warm run parsed 0 files, and each repo's cold and warm dumps are byte-identical.

  Sanity checks against `grep`:
  - **Java:** type declarations 50 = 50.
  - **Python:** def + class 807 = 807.
  - **TypeScript:** classes 59 = 59, and type-alias definition sites 915 of 926. The 11 missing are in files where the grammar reports syntax errors.
  - **Go:** functions + methods 1,315 = 1,345 − 25 (generated `test.pb.go`, skipped by design) − 5 (inside a `/* legacy tests */` block comment, which grep miscounts).

  All C18 repos were deleted from the Brain afterwards.
- **Known gaps:**
  - Java `@interface` annotation types are not captured.
  - tree-sitter-typescript 0.23.2 (the newest on crates.io, from November 2024) rejects some valid TypeScript 5.x, such as `export type * from`. In hono this caused 8 false syntax errors and lost definitions in those files. The TSX grammar also flags a bare `&` in JSX text.
  - Nesting deeper than about 32k levels takes Tree-sitter about 17 s per file. It is bounded and doesn't crash, but a timeout would make results depend on machine speed, so none is used.
  - JavaScript computed and private method names are not captured by the upstream query.
- **Found and fixed during review:** see CHANGELOG 2026-09-29 11:30.

### Status

- [x] Steps 1–3, committed: ce5290a, d97e645, eb70b51
- [x] Evaluator review of 54cb220..HEAD: 3 confirmed defects (a Go receiver cycle crashing the whole run, the wrong Go receiver parent, quadratic key growth), all fixed test-first.
- [x] Own C18 findings fixed:
  - TypeScript type aliases and enums were missing;
  - locals in anonymous scopes were merged into one symbol;
  - the cache served stale facts twice after extractor changes, now prevented by fingerprinting queries and code.
- [x] Final commit (see git log)

## M2a SQL core: complete

- **Decided:** see `tasks/contract-m2a.md` and its two amendment sections.
  - `pg_query` is the only new dependency.
  - `rb-sql` splits statements itself and walks the parse tree, instead of using pg_query's table helpers.
  - SQL candidates are cached extractor output; SQL analysis is recomputed each run.
  - Unparseable SQL is reported only for `.sql` files and sink calls.
  - Coverage is derived from the graph, never stored.
- **Built:**
  - the `rb-sql` crate;
  - `rb-syntax` SQL candidates (`strings.rs`);
  - the `rb-indexer` SQL stage (`sql.rs`);
  - `Store::resolve_table`, `table_deps` and `sql_coverage`;
  - `rb table-deps`;
  - SQL coverage in `rb status`;
  - SQL added to the polyglot fixture, plus goldens.
- **Measured (S16):** public repos at pinned commits, release build, after fixes, 2026-09-29.

  | Repo @ commit | Cold | Warm | Tables | Code sites | Migration statements | Unparsed |
  |---|---|---|---|---|---|---|
  | spring-projects/spring-petclinic @ 818c4136ea97 | 0.92 s | 0.12 s | 7 | 0 | 128 | 66 |
  | porsager/postgres @ 411429e7bd7a | 0.20 s | 0.18 s | 6 | 783 | 0 | 108 |
  | jackc/pgx @ ea996359befe | 0.42 s | 0.39 s | 31 | 359 | 2 | 10 |

  Every warm run parsed 0 files, and cold and warm dumps are byte-identical.
  - **petclinic:** 7 tables, equal to the distinct `CREATE TABLE` names across all its dialect files. All 66 unparsed statements are MySQL/H2/HSQLDB dialect.
  - **postgres.js:** all 108 unparsed are dynamic identifiers (`${sql(table)}`) in table position.
  - **pgx:** `drop_cols`, `foo` and `t` were verified against the source. System catalogs, composite types and prepared-statement names no longer appear as tables.
- **Found in S16 and fixed:** prefilter prefix match (`insert_t`), unqualified `pg_*` catalogs, `ALTER TYPE`, `COPY TO`, `RENAME`.
- **Found by the evaluator and fixed:**
  - retry ranges overran the text (a crash);
  - CTE shadowing;
  - nested DML operations;
  - dotted quoted names;
  - BOM in migrations;
  - `\u`/`\x`/octal escapes;
  - prose becoming tables;
  - coverage over-counting;
  - also: whole-file failure on one bad statement, `%s` placeholders, JPQL, case-insensitive table lookup.

### Status

- [x] Steps 1–4 committed: 7ac4f7e, 6d8c02e, 10ee960, 8855bbf
- [x] First evaluator pass: 8 defects; all fixed test-first (rb-sql rewrite)
- [x] Second evaluator pass: 9 issues, including the structural `nodes()` gap. The core was rebuilt on pg_query's JSON tree walk, and a new test exposed a hidden lexer-split drop.
- [x] Third evaluator pass: 6 defects, including a panic. Splitting is now token-only. Mutation checks cover every fix.
- [x] Commit (checkpoint, bd40ff3)
- [x] Fourth evaluator pass: 4 defects (lexer-error recovery, `pg_dump` COPY data, glued arithmetic placeholders, multi-action rules) and no regressions. All fixed and committed.
