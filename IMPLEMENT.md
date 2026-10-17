# IMPLEMENT

This file traces decisions from conversation to code. The design lives in `docs/repository-brain-design.md`, and task progress in `tasks/todo.md`.

## Working rules (agreed 2026-09-29)

- The `loop` rules take priority over the older "pause after each phase" rule: the agent decides and continues without waiting.
- The `engineer` rules apply to all code.
- Every batch runs plan → tests first → implement → separate evaluator agent → verify → commit.
- **Backups:** a project zip goes in `archive/` (gitignored) before each major change, and a git commit follows each verified batch. Per-file `.backup` copies are deliberately not used, because fixture tests copy whole directories and stray files would change the golden dumps.
- **Verification:** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:<port>/rb cargo test --workspace`, where `<port>` is the database's: 55432 by default (`compose.yaml`), 55433 on this machine since 2026-09-30 (`RB_DB_PORT=55433`; see the Docker Desktop restoration below). In M5 the loop is `var/m5/scripts/verify.sh <tag>`.

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

## M2b ORM mappings: complete

- **Decided:** see `tasks/contract-m2b.md`.
  - `rb-syntax` extracts framework-neutral decorations, supertypes, class constants and call qualifiers.
  - A new `rb-frameworks` crate holds the packs.
  - Naming rules are checked against TypeORM and Hibernate/Spring Boot sources.
  - JPA names follow Hibernate's physical naming: snake_case plus lowercase, with quoted names kept exactly. The other ORMs keep case. The frameworks crate computes physical names; the indexer does not fold case.
  - Certainty by kind: stated exact, convention synthetic, JPQL heuristic, duplicates ambiguous.
- **Built:** `decorations.rs`, `rb-frameworks`, indexer `orm.rs` (`MAPS_TO` and ORM `QUERIES`), `Content` input struct, and "Mapped by" in `table-deps`, plus goldens for the JPA and Prisma fixtures.
- **Measured (O9):** public repos at pinned commits, release build, 2026-09-29.

  | Repo @ commit | Time | Mappings | ORM query edges | Unresolved entities |
  |---|---|---|---|---|
  | spring-projects/spring-petclinic @ 818c4136ea97 | 1.07 s | 6 JPA (= 6 `@Entity`) | 8 | 0 |
  | prisma/prisma-examples @ caa902a819be | 1.02 s | 143 Prisma (= 143 `model` blocks), 2 TypeORM | 341 | 0 |
  | django/djangoproject.com @ 901dd6b014c5 | 0.34 s | 48 Django (= 48 `models.Model` classes) | 0 | 0 |

  Re-measured after the evaluator fixes (same commits): petclinic 6 JPA with 8 ORM query edges; prisma-examples 143 Prisma + 2 TypeORM with 341; djangoproject.com **55** Django. Every run had 0 ambiguous edges and 0 ORM diagnostics. The Django count reconciles exactly: 48 direct `models.Model` classes, minus 3 abstract bases (`Metric`, `ReleaseChecklist`, `FundraisingModel`), plus 10 concrete subclasses of those bases.

  - **petclinic:** all 6 entities map exactly to the same `Table` nodes that its 4 dialect schema files create, so the ORM and SQL views join.
  - **prisma-examples:** the TypeORM mappings come from its real TypeORM example.
  - **djangoproject.com:** usage (querysets) is out of M2b's scope, so 0 ORM query edges is expected.

### Status

- [x] Steps 1–3 committed: c2ce362, 2982c96, 4dd764e
- [x] First evaluator pass: 13 confirmed defects (the report's summary line said 11, but its list has 13). All were fixed test-first in three layers: syntax facts (`literal`, `type_params`, flattened concatenation), framework rules, then indexer resolution (JPA inheritance, generic repositories, Prisma roots, `MAPS_TO` aggregation). See contract Amendment 1.
- [x] Second evaluator pass: the 13 earlier fixes verified; 11 new code defects, plus a stale O9 record that was already updated. All fixed test-first, with mutation checks on the syntax fixes (contract Amendment 2). The O9 numbers did not change.
- [x] Third evaluator pass: the 11 pass-2 fixes verified; 3 contract violations (base-repository clashes across rounds, `$n` hole tables from dynamic native SQL, `implements` taken for `extends`) plus lower-severity defects. Fixed with tests; D1–D3 were mutation-checked. Accepted gaps are listed in contract Amendment 3. The O9 numbers did not change. Tests: 165 unit, 8 e2e.
- [x] Fourth evaluator pass: D1–D3 verified; a non-terminating repository-base fixpoint (my termination argument was wrong), a project interface overriding a Spring base, the hole filter hitting literal SQL, exponential Python walks, Flask abstract bases. All fixed with tests, and the fixpoint was replaced by a memoised resolver (Amendment 4). Tests: 170 unit, 8 e2e; O9 unchanged.
- [x] Fifth evaluator pass: pass-4 fixes verified; 2 lower-severity defects (name-dependent base resolution around cycles; cubic Python walks, which contradicted my "linear" claim). Both fixed test-first, and the tests failed before the fix (Amendment 5). Tests: 172 unit, 8 e2e; O9 unchanged.
- [x] Sixth evaluator pass (interrupted by a session end, then resumed): no contract violations. Quadratic Django walks and a proxy stack overflow were fixed test-first; the tests overflowed before the fix. The Flask match was tightened. Name-resolution limitations are deferred to M3 (Amendment 6). Tests: 174 unit, 8 e2e; O9 unchanged.
- [x] Seventh evaluator pass: PASS with 2 lower-severity reservations (source-order dependence in `first_along`; `Meta` options merged across abstract parents). Both were fixed with the evaluator's repros as failing-first tests (Amendment 7). These two fixes were not re-reviewed; M3's first evaluator pass re-covers `rb-frameworks`. Tests: 176 unit, 8 e2e; O9 unchanged.
- [x] Commit

## M3a Tier B resolution: complete

- **Decided:** see `tasks/contract-m3a.md`. M3 is split: M3a resolves calls, types and implementations heuristically from Tree-sitter facts and ships `rb callers` / `rb impact`; M3b adds SCIP in containers. M3a adds import, package and typed-binding facts to `rb-syntax`. Those facts are also what the M2b limitations deferred to M3 need.

- **Built:** step 1 (f272cda) imports, packages and bindings; steps 2–3 (73574a4) module and call resolution with coverage; step 4 (4d481aa) `rb callers` / `rb impact` with fixture goldens.
- **Measured (R12):** public repos at pinned commits, release build, 2026-09-30.

  | Repo @ commit | Index time | Resolved share (own code) | Resolved / ambiguous / unresolved / external | Hand-checked sample |
  |---|---|---|---|---|
  | spring-projects/spring-petclinic @ 818c4136ea97 | 0.89 s | 37.8% | 329 / 27 / 585 / 656 | 20/20 correct |
  | honojs/hono @ 18331a905e24 | 0.46 s | 14.8% | 6,310 / 56 / 36,556 / 1,750 | 20/20 correct |
  | gin-gonic/gin @ 1bd0ecf2d3e0 | 0.33 s | 70.3% | 4,860 / 72 / 2,086 / 5,162 | 20/20 correct |
  | psf/requests @ 611c6162cbc4 | 0.18 s | 30.4% | 511 / 26 / 1,231 / 488 | 20/20 correct |

  - **How the sample was drawn:** 20 `heuristic` edges per repo, in md5 order of their keys, each checked against its source line. All 80 were correct, including inherited targets, class-qualified and static calls, constructor calls and typed parameters.
  - **Why unresolved is high:** calls on receivers without a declared type stay unresolved by design (invariant 3). The hono gap (untyped callback parameters such as `(c) => c.json()`, and test globals) is a hypothesis, not a measurement.
- **Found in R12 and fixed:**
  - Indexing psf/requests overflowed the stack. A binding such as `session = session.Session()` made type resolution recurse through itself. Type resolution now has an in-progress guard and a depth cap of 64, with a cycle test.
- **Known imprecision (for the evaluator and M3b):**
  - A memoised supertype list can be cut short by the in-progress guard, but only when a supertype name resolves through a cyclic binding. The evaluator found no reachable case.
  - The earlier anonymous-parameter leak is gone since Amendment 2: declarations carry extents.

### Status

- [x] Steps 1–4 committed; R12 measured.
- [x] Evaluator pass 1: 12 defects (three performance blowups; receivers linked through same-named types; untyped names not shadowing; impact missing dispatch callers; Java lookup order; rebinding; Go test packages; status split; Python stdlib matching; self-calls). All fixed test-first (contract Amendment 1). The independent 80-edge sample was also 80/80.
- [x] Evaluator pass 2: 5 defects, two of them regressions of pass-1 fixes (position-based rebinding ignored block scope; a declaration referenced itself). Fixed by giving every declaration a visibility extent (contract Amendment 2). Resolved share after the fixes: petclinic 39.2%, hono 14.8%, gin 63.4%, requests 30.4%.
- [x] Evaluator pass 3: all pass-2 fixes verified (88/88 precision); 5 defects in the extent rules and bare-call resolution, all fixed test-first (contract Amendment 3). Resolved share after the fixes: petclinic 39.2%, hono 13.5%, gin 63.4%, requests 30.4%.
- [x] Evaluator pass 4: C1–C5 verified; one regression (function-valued `const`s treated as variables, which caused the hono drop I had misattributed) plus Java namespaces, `var` hoisting and missing Python built-ins. All fixed test-first (contract Amendment 4). hono is back at 14.8% (the 329 suppressed edges regained, verified by an edge diff); the other repos are unchanged.
- [x] Evaluator pass 5: D1–D5 verified; one regression (function-valued constants inside callbacks fell through to same-named imports) plus generator/class expressions and two `var` hoisting gaps. Fixed test-first by moving the "is a definition" decision into the resolver (contract Amendment 5). Edge diff: hono lost exactly the one wrong edge; the other repos are identical.
- [x] Evaluator pass 6: PASS. All Amendment 5 repros hold, 96/96 sampled edges correct, invariants 1–6 hold, no database FATALs across three serial e2e runs. Its two low-severity notes on `defines` were fixed afterwards with unit tests (Amendment 6).
- [x] e2e flake: AGE 1.8 on PG 18 kills backends under concurrent graph DDL; tests serialized, risk recorded in design §14.
- [x] Commit

## M3b SCIP orchestration: complete (closed 2026-10-06, contract amendment 81)

- **Decided so far:** see `tasks/contract-m3b.md` (draft). Indexers run in pinned containers with the network disabled during indexing; an index is cached by `(project_input_hash, indexer_digest)`; a project is fully SCIP or fully heuristic. scip-java and scip-typescript first.
- **Environment checked 2026-10-01:** Docker 29.5.2 on colima (arm64); `--network none` verified to block egress; about 4.8 GiB free on the host disk, so image sizes matter.
- **Research done (2026-10-01, primary sources, recorded in the contract):** `scip` 0.10.0 with `protobuf` pinned to 3.7.2; scip-java 0.13.1 (`ghcr.io/scip-code/scip-java`, arm64 and amd64 digests recorded) runs the whole Maven/Gradle build; scip-typescript 0.4.0 (`sourcegraph/scip-typescript`, **amd64 only**, so it runs under emulation here, which was verified to work). Both indexers mean UTF-16 columns and leave the encoding unset; scip-java emits typed ranges, scip-typescript the deprecated ones. Offline indexing after a fetch step is undocumented for both and is verified empirically in step 2.
- **Built:** step 1, `rb-scip::decode` (typed and deprecated ranges, column conversion with the document text or the repo blob, duplicate-document merge, symbol information). Two new dependencies with stated reasons: `scip`, `protobuf`.

- **Incident (2026-10-01):** pulling the official scip-java 0.13.1 image (6.75 GB uncompressed) filled the host disk; `cargo clean` (16.4 GiB, `target/` had reached 12 GB) and removing the image recovered it. The published image is not usable here: the Java path needs a slim image from a pinned JDK base plus the pinned scip-java release (research in progress). scip-typescript's slim image proceeds.
- **Built (step 2):** job detection (`rb_scip::jobs`), the container runner (`rb_scip::runner`, `docker run` with digest, platform, `--network none`, budgets), the cache key (`project_input_hash`, SHA-256; new dependency `sha2`), the `rb.scip_index` cache table (migration 0004) with store methods, and `rb_scip::run_job` (export, fetch with the network, index without it). Verified empirically on the TypeScript fixture: offline indexing after a fetch step works; bind mounts must sit under a colima-shared path, so work directories live under `var/scip/`.
- **Built (step 3a–3b):** the SCIP⋈Tree-sitter join (`rb_indexer::scip::join`: keys by definition-name range, `CALLS`/`REFERENCES`/`IMPLEMENTS` from matched references and `is_implementation`, coverage incl. `references_unanchored`); orchestration in `rb_indexer::projects` (`ScipOptions`, `ScipCache` mirroring `FactsCache`, cache hit → no container, `unsupported` never cached, `--no-scip` or no Docker → every job `skipped`); `assemble` builds one symbol-keys map (heuristic, overridden by SCIP symbols) that every stage takes, adds exact edges, excludes SCIP-ok files from Tier B and emits one `scip_status` diagnostic per job (no `cached` flag, so warm re-indexes dump identically). CLI: `--no-scip`, `--scip-work-dir`, per-job `scip:` lines in `rb index` and `rb status`, `exact calls (scip)` in the `code:` line. e2e `scip_ts_express_prisma` runs the real container (skips without Docker) and proves S3, S4, S6, S7, S9 and S12 on the TypeScript fixture.
- **Built (step 3c, contract amendments 7–10):** the Java path through the slim local image `rb-scip-java:0.13.1` (Maven only; Gradle stays `unsupported`). `Runner::image_id` resolves a local image's ID, `rb_scip::resolve_image` makes it the cache key and the reference the containers run by, and a missing image is `unsupported` with a reason pointing at the README (never built implicitly). `java_runs`: `go-offline` plus an online `verify` into the shared `var/scip/cache/m2`, then `scip-java index --build-tool Maven -- --batch-mode --offline …` under `--network none`, with Maven's logger moved to stderr so build errors reach the stderr tail. The join gained the bounded by-name fallback for scip-java's annotation-anchored definition occurrences (`definitions_matched_by_name` in coverage and `rb status`). Fixtures: the polyglot `services/auth` pom now compiles (web, jdbc, openfeign), and `services/broken` fails to build on purpose. e2e `scip_java_spring_jpa` (goldens for callers, impact and status; cached re-index byte-identical; fresh 6.1 s / cached 0.15 s) and `scip_polyglot_monorepo` (`rb index` succeeds with one failed Maven project and one failed npm package; the ok projects are exact). Status-line goldens mask run times.
- **Built (step 4, S8, contract amendments 11–12):** `Package` nodes and `Service -DEPENDS_ON-> Package` from every occurrence of a global symbol the index does not define (`rb_scip::package` parses the symbol's `<manager> <name> <version>`; key `<manager>/<name>@<version>`, scip-java's `maven/` artifact prefix dropped, `@?` for no version; own-package and namespace-only symbols skipped; symbol relationships not used). One edge per service and package, `scip`/`exact`, with the first site as its one evidence. `rb service <name>` prints `Packages (n)` or `Packages: none`; no `Symbol` stubs for external symbols. Versions are the indexer's (a `@types/*` package stands in for its untyped runtime package; scip-typescript can give a subpath import its own version); lockfile resolution is out of scope.
- **Measured (step 4, S11, contract "Measurement" section):** spring-petclinic @ 818c4136ea97 with scip-java: 71.1 s in the container (Spring Boot 4.1.0 fetch + index), 806 KB index, 240/240 definitions, 256 exact `CALLS` (324 sites) against the heuristic twin's 283 edges — the difference is 34 `ambiguous` overload fan-outs the heuristic emits and 7 inherited-accessor calls only SCIP resolves; 36 packages; 20/20 sampled edges correct. hono was **substituted by typestack/class-validator @ 2e1a5c27dbd6** (amendment 13: hono's pnpm lock lists 1,085 packages with native binaries, too large for the emulated install on 2.8 GiB free): 8.7 s (npm ci of 518 packages + index), 959 KB index, 417/424 definitions, 394 exact `CALLS` against 373 heuristic edges within `src/`; 5 packages; 20/20 sampled edges correct. `rb callers` on `VetRepository.findAll` separates the two overloads (6 and 2 exact callers) where the heuristic listed the same 7 ambiguous callers for both.
- **Found in S11 and fixed (amendment 14):** files under a SCIP-ok root that the indexer emitted no document for (class-validator's tsconfig excludes `test/**` and `sample`: 42 files, 927 heuristic call sites) got no edges and no count. The join now reports `files_without_document`, printed by `rb status` when non-zero.
- **Evaluator pass 1 (amendments 15–20):** ownership is by document (a file is exact only when its `ok` index has a document for it; the rest keep Tier B; `files_without_document` counts the indexer's languages only); `IMPLEMENTS` follows the syntax's supertype declaration, since scip-java sets `is_implementation` on both sides of an override; `tool_versions.scip` distinguishes `off`, `unavailable` and `on`; containers are named `rb-scip-<hash16>-<phase>` and killed by name on a budget overrun (`--scip-timeout-secs`); the decoder's document merge is set-based; `unpinned` is printed; `--no-cache` bypasses SCIP cache reads. New goldens `scip-impact-default-authenticate.txt`; re-blessed `scip-card.txt`, `scip-status.txt` (Java), `scip-status.txt` (polyglot).
- **Evaluator pass 2 (amendments 21–24):** `IMPLEMENTS` is accepted through the declared hierarchy, not only a direct supertype: the target's type definition is found exactly by symbol (`rb_scip::type_symbol_of`, which replaces `last_type_name`) and the source's type holder must reach it through declared supertypes (`scip::reaches_supertype`: simple names, visited set, depth cap 64), so `Svc#m() IMPLEMENTS Base#m()` through an abstract class and an interface chain is kept and the reverse is still rejected. Each `run_job` call is its own instance: the export lives in `<work_dir>/<hash16>-<pid>-<nanos>` and the containers are `rb-scip-<hash16>-<pid>-<nanos>-<phase>`, so two runs of one commit at once share nothing; the cache key is unchanged. Coverage gained `references_undocumented` (an undefined symbol of the project's own package: a file without a document, a generated client, a record's accessors), printed between `external` and `local`; it took 1 reference from `external` on the TypeScript fixture and 2 on the Java one. `resolve_target` accepts a key suffix that starts after a space, so a scip-java key's whole descriptor (`com/acme/auth/AuthService#authenticate().`) resolves. Status goldens changed on all three fixtures; the two that need the scip-java image were edited by hand from the join of the cached indexes.
- **Evaluator pass 3 (amendments 25–42, `tasks/eval-m3b-pass3.md`):** the export leaves `mvnw`/`mvnw.cmd`/`gradlew`/`gradlew.bat` at the build root out, since Docker Desktop's mounts answer `access(X_OK)` for every file and scip-java ran the repository's wrapper (spring-petclinic now indexes with its wrapper present). Declared supertypes resolve through the index's own heritage-clause occurrences (`rb_syntax::Supertype.span`, `scip::declared_supertypes`: the earliest type occurrence in the span, a longer symbol extending it for nested types; `types_by_symbol` replaces the simple-name lookup), so a same-named type elsewhere no longer turns scip-java's reverse override link into an exact edge; unresolved ones are counted (`supertypes_unresolved`). TypeScript `interface X extends Y` declares supertypes, so `IMPLEMENTS` chains through interfaces. The fetch phase is stated honestly (default bridge network, the project's build, `--ignore-scripts`; allow-listing is M5), scip-typescript projects follow `--`. `rb_scip::Images` resolves both images once per `rb index`; `tool_versions` records `scip_java`/`scip_typescript` identities. `RunStatus::Environment` (docker exit 125–127, client errors) and `Timeout { phase, budget_secs }` are never cached (`RunStatus::is_cacheable`; a stale `timeout` row is a miss). `elapsed_ms` left the `scip_status` detail (two fresh runs dump byte-identically; `rb status` prints no time); `occurrences_unconverted` joined the coverage and the TypeScript fixture gained a non-ASCII call line that fails when the column conversion is removed. `PIPELINE` is `m3b-scip-1`; dead decoded fields and the debugging aid are gone; `BTreeMap` lookup replaces the per-symbol scan. CLI: the dotted form crosses the space of a scip-java key, keys display as their descriptors, `code: no heuristic references to own code (n external)` for all-SCIP repositories, the card's last line names M4. e2e: fixtures are copied from `git ls-files`, the Java tests run a container on `--no-cache` (polyglot compares without the failed jobs' stderr tails, amendment 34), a `docker` shim proves the image identity is part of the tool configuration. S11 refreshed on Docker Desktop: petclinic 22.1 s / 256 exact calls / 36 packages, class-validator 10.1 s / 394 / 5, both samples 20/20.
- **Evaluator pass 4 (amendments 43–52, `tasks/eval-m3b-pass4.md`):** a declared supertype's span is its *name* only (`rb_syntax::Supertype.span`: Java `generic_type` → its name child, `annotated_type` → the type after its annotations; TypeScript `extends_clause` → its `value`, and a value that is not a name such as `mixin(Base)` declares nothing), and the join resolves it to the last type occurrence inside that span, so a nested type argument (`Face extends Marker<Marker.Disc>`) or a type-use annotation (`implements @Tag Shape`) no longer picks the wrong type: the false `Face#area() IMPLEMENTS Marker#Disc#area()` is gone and the lost `Cfg#run() → Base#run()`, `Sq# → Shape#` edges are back. A snapshot whose SCIP jobs did not all finish (`environment`, `timeout`, `skipped`, `unsupported`, none of them cached) is never "up to date" while SCIP is on (`Store::unfinished_scip_jobs`, `rebuild_reason` in `rb index`: `rebuilding: n SCIP jobs did not finish last time`). `rb_scip::RECIPE` (`m3b-2`) is part of `project_input_hash`, so an index from an older fetch recipe is a miss. The npm fetch runs with `--logs-max=0` (no timestamped log path in a failed tail); S12 and invariant 5 name the one field that may vary between fresh runs (a failed job's `stderr_tail`). scip-typescript's recorded identity is its digest reference; only the local scip-java image is an ID or `absent`. Stored reasons name no host (`skipped` → `docker is not available`; an `environment` reason is the client's first line with absolute paths redacted; the detail goes to stderr at index time). `rb status` prints a timeout's detail once. Coverage gains `relationships_dropped` (an `is_implementation` relationship whose source or target has no project node), printed on the `scip:` line. README documents the container runtime of record (Docker Desktop, `DOCKER_CONTEXT=desktop-linux`) and the work directory's home-directory requirement. Public repos re-measured with the release binary: petclinic `scip-java ok 15.0s`, 240/240, 256 exact `CALLS`, 13 `IMPLEMENTS` (all 13 read in the source, correct direction), `relationships dropped 42` (every one an external supertype), `supertypes_unresolved 0`; class-validator `ok 9.1s`, 417/424, 394 exact `CALLS`, `relationships dropped 2` (the `local` class expression's), `supertypes_unresolved 0`.
- **Evaluator pass 5 (amendments 53–58, `tasks/eval-m3b-pass5.md`):** scip-java's columns are javac's (a tab advances to the next multiple of 8, scip-java corrects leading tabs only), measured on a scratch project with the real image and converted in `rb_scip::decode` (`Units::Javac`); a declared supertype's occurrence must also name the written type, so a shifted occurrence can never pick another type (the false `Wide#act() IMPLEMENTS Doer#act()` is gone). Document-local definition occurrences (scip-java's enum constant bodies, scip-typescript's nested functions and classes) bind their Tree-sitter definition within the document: the definition keeps its heuristic key (`definitions_local`) and references and relationships naming the local give exact edges; `relationships_dropped_in_project` counts join gaps (0 on all three public repos). Comments are no longer supertypes. A `failed` job names its phase (`failed (fetch phase)`), stored in `rb.scip_index.phase` (migration 0005). jsoup @ 088614f4a145 is the third public repository: 4,874/4,991 + 117 local definitions, 14,013 exact `CALLS` (20/20 sampled), 688 `IMPLEMENTS` (10/10 sampled from local definitions); petclinic unchanged (256 / 13); class-validator 398 exact `CALLS` and 2 `IMPLEMENTS` (+6 edges, classified).
- **Evaluator pass 6 (amendments 59–63, `tasks/eval-m3b-pass6.md`):** an indexer's line numbers are its own (javac: `\n`, a lone `\r`, `\r\n`; TypeScript: also U+2028/U+2029; measured on both real images), so `rb_scip::decode` converts every position through its byte offset in the text (`Lines::position`: the indexer's line, the column rule within it, then Tree-sitter's `\n` rows); the join accepts an exact-span definition or reference occurrence only when it names the token (`scip::names_token`, `rb_scip::may_name`: the last descriptor, a constructor at its type's name, `validate0:`-style properties, `<get>`/`<set>` accessors, import aliases; 0 rejections on the four public repositories). Bound locals count in project by binding (TypeScript locals have no display name); qualified supertype names skip comments (`name_and_span`); coverage gains `definitions_without_occurrence` and `relationships_anonymous` (printed on the `scip:` line); `PIPELINE` is `m3b-scip-2`.
- **Evaluator pass 7 (amendments 64–67, `tasks/eval-m3b-pass7.md`):** instead of mirroring one more lexing difference, the join checks per document that the indexer and Tree-sitter read the file alike and otherwise leaves the whole file to Tier B, counted as `documents rejected` with a reason: `not_utf8`, `text_differs` (neither indexer emits `Document.text`, measured), `source_encoding` (a Maven pom declaring a non-UTF-8 source encoding, for non-ASCII files), `comment_terminator` (scip-java: a lone CR or a `\u000a`/`\u000d` escape, which end a `//` comment for javac only) and `definition_disagreement` (a method or type definition occurrence no Tree-sitter definition accounts for, on another token or inside a method; calibrated to 0 rejections on the four public repositories and the fixtures). scip-typescript's line 0 starts after a BOM; the name guard's default-import and local forms check the import's own symbol and the local's name or binding; `rb status` omits coverage fields an older snapshot lacks; the M3a timing test is a growth ratio. `PIPELINE` `m3b-scip-3`.
- **Evaluator pass 8 (amendments 68–71, `tasks/eval-m3b-pass8.md`):** two structural checks instead of more per-quirk rules. `structure_disagreement` (join, Java only): a reference that would become an edge must lie inside the index's enclosing range of the definition anchoring its Tree-sitter source, and the innermost anchored range around it must be that source's or an ancestor's (scip-java's ranges measured on every definition kind; scip-typescript's include decorators, which Tree-sitter places elsewhere, and TypeScript has no escape translation, so it is not checked). `encoding_ambiguous` (decoder, scip-java): every position on a non-ASCII line is read as UTF-8 and as one column per byte; a document is rejected when an occurrence names its token both ways or the single-byte reading names more (`.mvn/maven.config`, `compilerArgs -encoding`, a module override are no longer unseen); the pom reading stays first. Tests for the evaluator's surviving mutants N24, N25, N27; amendment 64's "impossible" replaced by a per-class coverage list with uncovered vectors. 0 rejections on five public repositories, edge sets unchanged. `PIPELINE` `m3b-scip-4`.
- **Evaluator pass 9 (amendments 72–74, `tasks/eval-m3b-pass9.md`):** a Java document's definitions must agree both ways (`structure_disagreement`, join): every global method, constructor or type definition occurrence with a range anchors a Tree-sitter definition (annotation interfaces and their elements excepted, identified by `kind` `Interface` and the relation to `java/lang/annotation/Annotation#`), every Tree-sitter definition is anchored, and Tree-sitter parsed the file without a syntax error; clause 2 of the structure check considers every method, constructor or type range, anchored or not. Members only javac sees (a `\u002a/` that ends a comment for javac only) or only Tree-sitter sees no longer give exact edges from the wrong source. `encoding_ambiguous` rejects a tie between the readings. Calibrated on five public Java repositories: the only unpaired forms are implicit constructors (no range) and annotation forms; 0 rejections, edge sets unchanged. `PIPELINE` `m3b-scip-5`.
- **Evaluator pass 10 (amendments 75–76, `tasks/eval-m3b-pass10.md`):** pairing is kind-aware (`definitions_unpaired`, Java): a Tree-sitter type pairs only with a type symbol, a method or constructor only with a method or constructor symbol, and a bound local only when it is a method of an enum constant's body (kind `Method`, innermost enclosing definition an enum constant of the Tree-sitter parent), so a field, a local variable or an anonymous class's method at the name of a method only Tree-sitter sees no longer makes its calls exact (`Anon1`–`Anon3`, `Loc1`, `Fld1`, `Init1`, `Enm2` rejected on the real image); a type is anchored at its name only by a type symbol, so a class whose own occurrence sits in its annotation is found by the amendment-7 fallback instead of its implicit constructor (vavr 2 → 0 rejected, +1,274 exact edges); a TypeScript document with a Tree-sitter syntax error is rejected (merge-conflict markers); compact canonical constructors are captured as constructors (TheAlgorithms/Java 2 → 0 rejected). `PIPELINE` `m3b-scip-6`.
- **Evaluator pass 11 (amendments 77–79, `tasks/eval-m3b-pass11.md`):** the escape trick is rejected at the input: the decoder rejects a scip-java document holding a Unicode escape that decodes to `/ * { } ( ) ; , " ' = < >` or a line terminator (`structural_escape`, JLS backslash-parity rule; `.`, `@`, `:` left out after json-schema-validator's `case '\u002E':`), 0 documents on seven public Java repositories, the evaluator's `Enm3`/`Enm4`/`Enm7`/`Sup1` rejected on the real image; the `IMPLEMENTS` walk steps onto a declared supertype only when the walk's origin type's symbol information relates it to that supertype (both indexers relate a class to every supertype transitively; scip-typescript relates an interface to nothing, measured), counted `supertypes_unconfirmed` (0 on nine public repositories, edge sets unchanged). `PIPELINE` `m3b-scip-7`.
- **Evaluator pass 12 (amendment 80, `tasks/eval-m3b-pass12.md`):** an escape decoding to a backslash is structural unless `u` immediately follows it (javac's produced backslash escapes the next quote inside a literal; Tree-sitter reads `\u005c` as complete), which closes the evaluator's `Bs7`/`Bs4`; vavr's Javadoc `\u005Cu…` stays accepted; 0 documents on seven Java repositories, counts unchanged. `PIPELINE` `m3b-scip-8`.
- **Closed (amendment 81):** final status S1–S13 and invariants 1–6, the review history of passes 1–12 and the consolidated open items are in the contract.
- **Open:** Gradle (M6), workspace installs for nested npm packages (M6), tsconfig `include`/`exclude` awareness for excluded files (M6), lockfile-resolved package versions (M4 `DEPENDS_ON` work), a registry allow-list for the fetch phase (M5; design §14). spring-retry's test sources get no scip-java document because a Develocity build cache serves `testCompile` (contract open items).

### Status

- [x] Steps 0–2
- [x] Step 3a–3b: join, orchestration, key replacement, exact edges, coverage, CLI
- [x] Step 3c: scip-java (Maven) in the local image, join fallback, Java and mixed-monorepo goldens
- [x] Step 3d (S8 packages) and step 4 (S11 measured on petclinic and class-validator; docs)
- [x] Evaluator pass 1 fixes (amendments 15–20)
- [x] Evaluator pass 2 fixes (amendments 21–24); SCIP e2e re-run with containers on Docker Desktop
- [x] Evaluator pass 3 fixes (amendments 25–42), S11 refreshed, 14 e2e tests with containers
- [x] Evaluator pass 4 fixes (amendments 43–52), S11 refreshed, 15 e2e tests with containers
- [x] Evaluator pass 5 fixes (amendments 53–58), jsoup measured, 15 e2e tests with containers
- [x] Evaluator pass 6 fixes (amendments 59–63), spring-retry measured, 15 e2e tests with containers
- [x] Evaluator pass 7 fixes (amendments 64–67): per-document agreement check, 15 e2e tests with containers
- [x] Evaluator pass 8 fixes (amendments 68–71): enclosing-range and encoding agreement, 15 e2e tests with containers
- [x] Evaluator pass 9 fixes (amendments 72–74): two-way definition agreement, encoding tie, 15 e2e tests with containers
- [x] Evaluator pass 10 fixes (amendments 75–76): kind-aware pairing, type anchoring, TypeScript syntax errors, compact constructors, 15 e2e tests with containers
- [x] Evaluator pass 11 fixes (amendments 77–79): structural escapes, index-confirmed supertypes, 15 e2e tests with containers
- [x] Evaluator pass 12 fixes (amendment 80): escaped backslash as a structural escape, 15 e2e tests with containers
- [x] M3b closed (amendment 81): final status, review history, open items carried forward; nine public repositories re-measured with the closing binary

### Environment incident (2026-09-30 evening, wall clock)

- During the pass-2 fix run the host disk reached 0 bytes free, `~/.colima` (the VM holding the brain database and both indexer images) was removed by something outside this project's commands, and Docker Desktop became the active context. The agent's transcript shows only reads of `~/.colima`; its deletions were `target/debug/incremental` and session scratch.
- The old VM was still running from the deleted files when this was written, so the database on port 55432 still answered. Everything in it is lost when that VM stops. The SCIP index cache rows were exported to `var/scip/cache/scip_index.copy` (11 rows).
- **Consequence for verification:** the pass-2 fixes (amendments 21–24) are verified by unit tests (276), fmt and clippy, and by the e2e suite with Docker hidden (SCIP tests skipped). `scip_java_spring_jpa` and `scip_polyglot_monorepo` were **not** re-run with containers; two of the three `scip-status.txt` goldens were edited by hand from a join over the cached indexes. They must be re-verified once a container runtime with `rb-scip-java:0.13.1` and the scip-typescript image exists again.
- **To restore:** a container runtime whose VM shares the home directory; `docker compose up -d --wait` and `rb migrate` for the database; `docker build -t rb-scip-java:0.13.1 docker/scip-java`; pull `sourcegraph/scip-typescript@sha256:028c0042…` for linux/amd64.
- Follow-up: the e2e fixture copy should skip ignored files (an IDE's `target/` inside a fixture broke `orm_table_deps_goldens`).
- **Restored on Docker Desktop (the user's choice, 2026-09-30 22:00 wall clock):** `compose.yaml` takes its host port from `RB_DB_PORT` (default 55432); the database runs on 55433 while the dead VM's forward holds 55432, freshly migrated (0001–0004). `--network none`, amd64 emulation and home-directory bind mounts were re-verified there.

## M4a Endpoints and request paths: complete (closed 2026-10-06, contract amendment 18)

- [x] Phase 0: preflight (D12 confirmed in the Spring 6.0 release notes; gin and Go 1.22 `ServeMux` grammars recorded in the contract)
- [x] Phase 1: neutral facts (call chains, string templates, binding initialisers and constants, decoration spans; amendment 1)
- [x] Phase 2: normaliser (`endpoint.rs`) and route packs (`routes.rs`: Spring, Express, gin, `net/http`; amendment 2)
- [x] Phase 3: client packs (`clients.rs`: fetch, axios, RestTemplate, WebClient/RestClient, Feign, Go `net/http`) and URL folding (`fold.rs`; amendment 3)
- [x] Phase 4: endpoints in the graph (`rb-indexer/src/routes.rs`; `EXPOSES`, `HANDLED_BY`, route diagnostics; `PIPELINE = m4a-http-1`; amendment 4)
- [x] Phase 5: stitching and coverage (`rb-indexer/src/stitch.rs`; `CONSUMES`, `http_*` diagnostics, `http_coverage`, the `rb status` `http:` line; `PIPELINE = m4a-http-2`; amendment 5)
- [x] Phase 6: `rb path` (`rb-store/src/path.rs` walks, `rb-cli/src/path.rs` text and JSON, `--pick`, `--depth`, `--hops`, `--full`; amendment 6)
- [x] Phase 7: Service Card `APIs` / `Calls APIs of` / `APIs called by` / unstitched count, `rb impact` endpoint roll-up (amendment 7).
- [x] Phase 8a: fixture plan part 1 (auth `GET /login` and a commented route, notification `/sms` and a commented route, `selfcheck.ts` loopback client, billing gin service with a same-path decoy and a Go client).
- [x] Phase 8b: fixture plan part 2 (web suffix, ambiguous, dynamic and external clients; Java context path `/auth`). 
- [x] Phase 8c: S22 path goldens (text, JSON, candidates), `status-http`, notification card, `impact-send-code`; precision guards for the three fixtures with a biting mutation per line; HTTP invariants and AGE endpoint parity on every fixture dump; `rb path` shows how each `CONSUMES` step was stitched (amendment 8). 
- [x] Phase 9: SCIP mode (`scip-path-login.txt`: the controller → Feign hop `exact`; notification's named `health` handler `exact`). 
- [x] Phase 10: public repositories measured (contract "Measurement"): petclinic-microservices 16/16 endpoints and 5/5 `CONSUMES` after amendment 9 (suffix ranking by the matched tail); node-express 20/20 as labelled (19 `prefix_unknown`); gin 13/17 (fork F3: `prefix_unknown` endpoints with different unknown prefixes share a key).
- [x] Fork F3 (orchestrator: option b; amendment 10, `73a111b`): a prefix-unknown endpoint's key ends in ` ?router=<file>#<binding>` (the router binding's file and scope-qualified name, no position); known-prefix keys unchanged; `rb path` candidates name the router; `PIPELINE = m4a-http-4`. gin re-measured: 22 endpoints, the 4 merged nodes split into 9, 20/20 sampled.
- [x] Evaluator pass 1 fixed (contract amendment 11, `PIPELINE = m4a-http-5`): hosts kept past port holes; suffix matches need a literal in the matched tail and own candidates rank with the others; glued trailing holes are opaque tails; `rb path` walks each inline handler's scope; `config_unread` and `prefix_maybe_incomplete` for Spring config M4a does not read; `ServeMux` hosts; axios shadowing; unknown bases for server-side Java clients; D8 crosses into any service exposing endpoints. piggymetrics' false `CONSUMES` gone; petclinic's 5 unchanged; express and gin byte-identical. Open items rewritten (13).
- [x] Evaluator pass 2 fixed (contract amendment 12, `PIPELINE = m4a-http-6`): a base stated in an axios request config or configured on a Spring client instance (initialiser, constructor assignment, `setUriTemplateHandler`, same-file `@Bean`, capped below `synthetic`) decides the host, an unreadable one is unknown; a host hole next to a dotted fragment is external, any other host hole is suffix-only; a hole after a literal port is an unknown prefix; YAML flow mappings, `config/` files and disabled config clients; growth tests count operations (5× for 4× input); `rb path` walks a called registrar outside other endpoints' inline handlers; `node-fetch` is `fetch`. Four public repositories byte-identical, ftgo +4 `dynamic` sites. Open items 11 and 13 rewritten, 14–20 added.
- [x] Evaluator pass 3 fixed (contract amendment 13, `PIPELINE = m4a-http-7`): two gates. Gate A: a client instance's (or builder's) constructed base reaches `synthetic` only when the instance is provably unchanged (a constant or never-written binding used only as the receiver of request calls, from the new `FileFacts.uses` facts; a mutable builder used only to build it; no importer using an exported instance otherwise); otherwise it keeps its host, is capped at `heuristic` and says `base: unverified`; `http_coverage` counts verified and unverified bases. Gate B: a URL whose path starts with a hole (a leading unknown under a base, a second hole after a port hole) is matched by suffix and never joined to a base. The config client is turned off only by a base document; `rb path` upstream no longer enters a registrar through its inline handler; growth tests count scans of bindings, chains and endpoints themselves (`work::Counted`); `cross-fetch`, `axios.create(…).get(…)` and inline `new RestTemplate()` are sites. Six public repositories: no edge changed (only the coverage row's `bases`; ftgo 2 verified). Open items 11 and 14 rewritten, 21–23 added.
- [x] Evaluator pass 4 fixed (contract amendment 14, `PIPELINE = m4a-http-8`): Gate A narrowed to a conservative, scope-insensitive token scan (`FileFacts.name_tokens` replaces `uses`): a constructed base is verified only for a function local or an unexported top-level `const` whose every token in its whole scope is its one declaration or a request call's receiver, built by a plain chain from a new client or builder; Java fields, exported TS instances and anything else stay `heuristic`, `base: unverified`. A `uriBuilderFactory` overrides `baseUrl`; any `spring.config.activate.*` document is conditional. The importer taint and the use-resolution machinery are deleted. Seven public repositories: dumps byte-identical to `b75b6fc` (ftgo 2 and Store 19 verified bases, all method locals). Open items 14 and 22 rewritten.
- [x] Evaluator pass 5 fixed (contract amendment 15, `PIPELINE = m4a-http-9`): Gate A covers every request: a site reaches `synthetic` only through a provably plain carrier — a verified instance whose construction and request chain have only plain links (`mutate()` is neither a request nor plain), a bare module client (`fetch`, axios's default export, Go's `net/http` package) in a repository with no global hook (`globalThis.fetch = …`, `axios.defaults…`/`axios.interceptors…`, `http.DefaultTransport`), a Feign interface, or an inline fresh construction with plain links — whatever the URL's source; axios configs are plain only with allow-listed keys. Seven public repositories: no edge added, removed or re-certified; 7 field-carried edges say `base: unverified`. Open item 22 rewritten with the M5 cost measurement.
- [x] Evaluator pass 6 fixed (contract amendment 16, `PIPELINE = m4a-http-10`): every carrier deny-list replaced by an allow-list of token roles — a carrier is plain only if every token naming its configuration surface anywhere in the repository is in an allowed role, and a tainted client kind caps all its sites (bare calls, axios instances, Feign). Java fresh constructions are allow-listed (`new RestTemplate()`, `create()`/`create(<url>)`, no-argument `builder()`; no client, factory or customizer argument, no class body); Feign is plain only with no interceptor-capable token or import, no `configuration`/`defaultConfiguration` and no Feign client key in any settings file; `fetch` tokens are recorded as properties too and must be callees; axios bindings include `require` at any scope and subpaths; a Go dot import taints. Closing bar written into the contract: no false `synthetic` from any form a repository-wide name-token scan can see; open item 22 lists the residue. Seven public repositories: no edge changed; piggymetrics' 4 Feign and ftgo's 4 `node-fetch` sites capped.
- [x] Evaluator pass 7 fixed (contract amendment 17, `PIPELINE = m4a-http-11`): every non-binary file without facts (no grammar: Kotlin, Groovy, Vue, HTML, …; generated; too large to parse) is scanned as raw text for the carriers' surface names (whole words; Go's only in `.go` files; prose, npm lock files, plain `package.json` dependency entries and XML comments inert; a file over 4 MiB taints every kind it is scanned for), a hit capping the kind repository-wide; TS/JS string literals `"fetch"`, `"axios"`, `"axios/…"` are tokens (`TokenRole::Literal`), so `jest.spyOn(global, "fetch")`, `vi.stubGlobal`, `globalThis["fetch"]` taint `fetch` and any axios module string outside an import or `require` declaration (import-equals, `require("axios").default`, a destructured `require`, `import()`, a re-export, `jest.mock`) taints axios; a service-discovery or load-balancer key (settings files, nested YAML, environment-variable spellings, raw text, Java naming Spring Cloud LoadBalancer or Ribbon by an import or a fully qualified annotation) caps every host-by-name `synthetic` stitch (`discovery: configured`). `rb index` prints the files scanned as text. Seven public repositories byte-identical to `95e1fbb`; raw scan ≤ 11 ms per repository. Closing decision written: M4a closes on a verification-only re-check; open-ended carrier probing moves to M5 ("adversarial carrier review").
- [x] M4a closed (contract amendment 18): evaluator pass 8 (`tasks/eval-m4a-pass8.md`, verification only, against `28e47ba`) VERDICT PASS — W1–W6 fixed end to end, 18 of 19 mutants killed (X1, a missing unit row for the "HTML is not stripped" rule, minor, carried as open item 24 to M4b), seven public repositories byte-identical to `95e1fbb` with 0 `synthetic` `CONSUMES`, e2e 17/17 twice, invariant 13 held; final S1–S28/invariant table, review history of passes 1–8, closing measurement and the open items by milestone (M4b; M5 with the adversarial carrier review; M6) recorded in the contract. Next: M4b.
- [x] Phase 11 records: contract measurement table and **Open items** (8 items, each with its number and milestone: default-import Express mounts and gin parameter groups → M4b, YAML discovery names → M6, `--no-cache` facts writes → M5, unbitten guard mutations → M4b, `base: literal` on suffix edges → M5, endpoints listed twice in `rb impact` → M4b, facts-cache growth → M5); design, README, todo and changelog synced. Next: the evaluator's pass; M4a is not marked done until it decides.

- **Split (2026-10-06):** M4 becomes M4a (this entry) and M4b. M4b covers Spring DI and Spring Data synthesis, RTA, derived `DEPENDS_ON`, the full Service Card, Q3's roll-up and orphan detection; it is not started. Design §13 has both rows.
- **Contract:** `tasks/contract-m4a.md`, S1–S28 and invariants 1–13. Phases are in `tasks/todo.md`.
- **Decided in the contract (D1–D12):**
  - Endpoint keys carry the exposing service (`HTTP POST /api/v1/login @services/auth`; design §8.2 annotated).
  - `CONSUMES` is never `exact`; `synthetic` is its maximum (design §10.2 annotated).
  - Frameworks: Spring MVC, Express, gin and `net/http`. Clients: `fetch`, axios, Feign, `RestTemplate`, `WebClient`/`RestClient` and Go `net/http`. Python, NestJS, YAML config, gateway rewrites, external endpoint nodes and Mermaid/DOT are deferred, each with its reason.
  - Config is read only from `application.properties`. Hosts match a service by name or directory, uniquely; a service without endpoints is a proxy; loopback means the caller's own service; any other unmatched literal host is external.
  - Inline handlers are span-scoped. `rb path` sinks are tables and endpoints.
  - Precision guards are `must-exist.txt` and `must-not-exist.txt` beside the goldens.
  - No new dependency and no migration.
- **Forks for the orchestrator:** F1 (may a name-matched host reach `synthetic`? decided yes), F2 (Python in M6, decided as recommended) and F3 (identity of a prefix-unknown endpoint, raised by the gin measurement; decided: the router's identity in the key, amendment 10).

## M4b DI and dispatch: complete (closed 2026-10-07, contract amendment 22)

- [x] Phase 0 (preflight): primary-source checks, pins, invariant-1 baseline, gate matchers; contract amendments 1–3.
- [x] Phase 1a: X1's HTML row; parameter positions, type arguments, array flag, return types and modifiers as facts (amendment 4); no golden change.
- [x] Phase 1b: default exports and per-file mention roles as facts (amendment 5); no golden change.
- [x] Phase 2: Spring DI facts per file (`rb_frameworks::di`: beans, injection points, scans, gate D; amendment 6); no golden change.
- [x] Phase 3: manifest packages with versions (npm lockfile, go.mod, pom) and Service → Service `via=package`; `PIPELINE = m4b-di-1` (amendment 7).
- [x] Phase 4a: Spring DI resolution, `INJECTS` (never above `heuristic`, F2), `di_*` diagnostics and `di_coverage`; `PIPELINE = m4b-di-2` (amendment 8).
- [x] Phase 4b: the `rb status` `di:` line and `status-di.txt`.
- [x] Phase 5a: RTA proof of absence, `impl_instantiated: false` on `IMPLEMENTS`, `rta_coverage`; `PIPELINE = m4b-di-3` (amendment 9).
- [x] Phase 5b: the `rb status` `rta:` line; invariant 1 held on spring-petclinic and routing-controllers (only added lines).
- [x] Phase 6: `--precision rta|cha` on `rb callers` (default `rta`), `rb impact` (`cha`) and `rb path` (`rta`); pruned dispatch steps counted in every answer (`precision=` header, `rta:` line, JSON `precision`/`pruned_by_rta`); `rb table-deps` takes it with its walk in phase 9.
- [x] Phase 7: entry kinds (`main`, `test`, `scheduled`, `listener`, `lifecycle`) and `orphan_eligible` as `Symbol` attributes, `orphan_coverage`; the whole-word scan of every non-binary file; `PIPELINE = m4b-di-4` (amendment 10).
- [x] Phase 8a: `rb impact` lists orphans (greatest-fixpoint reachability in deployable services), impacted tests, each endpoint once (`(handler removed)`, routers of prefix-unknown endpoints), and gains `--json` (amendment 11).
- [x] Phase 8b: `rb callers` marks entry points (`(entry: …)`: stored kinds, `endpoint <name>`, `top-level`) and keeps expanding past them; prefix-unknown endpoints show their router in the `rb path` header and the card.
- [x] Phase 9: `rb table-deps` rolls up callers to entry points per service (ops, direct/indirect, endpoints, entry kinds, best certainty) and lists services reaching the table only through APIs, up to `--hops` (amendment 12).
- [x] Phase 10: the full Service Card: packages with versions, database tables, service dependencies `via=calls|http|package` and their inverse, shared tables, the call-graph summary and entry points by kind (amendment 13).
- [x] Phase 11a: Express mounts of default imports (S19; node-express-realworld's layout needs more, amendment 14) and catch-all endpoints only for named hosts (S20); `PIPELINE = m4b-di-5`.
- [x] Phase 11b: an injected Spring client's base comes from the `@Bean` method its injection point resolves to, any file (S21, `base_from_bean`, never `synthetic`); `PIPELINE = m4b-di-6` (amendment 15).
- [x] Phase 12a: java-spring-jpa gains its Spring application, a `@Primary` hasher, profile issuers, a `@Bean` audit sink with the never-instantiated `KafkaAuditSink`, and two notifiers (amendment 16).
- [x] Phase 12b: a Spring Data fragment, the non-Spring `@Service` decoy, and `AuthController`'s qualified field, collection constructor parameter and setter.
- [x] Phase 12c: `DefaultAuthService`'s `@Primary`, `@Bean` and name-fallback points; `LegacyAuth`'s not-a-bean field and orphan helper; polyglot auth's application, Feign and `@Repository` beans (login now reaches `public.users` and `audit.users`).
- [x] Phase 12d: billing shares `public.users` with auth, notification's relative `smsCheck` stays unstitched (`self`), ts-express-prisma mounts a default-imported router (`GET /ops/health`).
- [x] Phase 13: precision guards and dump checks for `INJECTS`, `DEPENDS_ON` and `impl_instantiated`, each bitten by a recorded mutation; DI/RTA/orphan goldens; S22 settled (amendment 17).
- [x] Phase 14: SCIP mode end to end: `scip-injects.txt` (amendment 18).
- [x] Phase 15: eight public repositories measured (contract measurement section; amendments 19–20: a by-simple-name `@Bean` match fixed, `PIPELINE = m4b-di-7`).
- [x] Phase 16 (records): contract measurement section and Open items (15, each with its milestone, plus the weaker false results still possible); design, README, todo and changelog synced. Next: evaluator pass 1 (full review), then pass 2 (verification only). M4b is not done until the evaluator decides.
- [x] Evaluator pass 1 (`tasks/eval-m4b-pass1.md`, VERDICT FAIL, Y1–Y15) fixed as amendment 21 (`PIPELINE = m4b-di-8`): RTA opt-in (every question defaults to `cha`); RTA reads binary names, every text file and TS/JS module objects, and a supertype of a non-definition keeps its base; orphan rule (e) is transitive; gate M covers every assignable registration; `@Primary`/name never pick past generics or unread annotations; `@Named` read; Maven duplicates, npm aliases and `go.mod` replaces. Open items 16–22. Next: evaluator pass 2 (verification only).
- [x] M4b closed (contract amendment 22): evaluator pass 2 (`tasks/eval-m4b-pass2.md`, verification only, against `304fe4a`) VERDICT PASS — Y1–Y15 hold end to end (JDK 21 and Spring 6.1.13 at run time, `go list` for `replace`), 21/21 mutants killed from a clean build, three public repositories byte-identical to `531e14e` and jhipster's claimed 8 lines, fresh == cached, fmt/clippy/559 unit tests/e2e 18/18 twice green, invariant 1 held; two doc notes (N1, N2) corrected in the records. Open items 1–23 by milestone (M5, M6) in amendment 22.

- **Contract:** `tasks/contract-m4b.md` (2026-10-06), S1–S28 and invariants 1–11. Phases 0–16 are in `tasks/todo.md`.
- **Soundness claims and closing bar, stated up front** (lesson 2026-10-06):
  - `INJECTS` never above `synthetic` as drafted; the orchestrator capped it at `heuristic` (F2), with the narrowing in `resolution` and the failed gates in `capped_by`;
  - RTA prunes only by a proof of absence, and every pruned step is counted;
  - orphans only for symbols every mention of which is accounted for;
  - `manifest` versions only from a lockfile or a literal.

  The residue is listed per claim. Pass 1 is a full review; pass 2 is verification-only, and new attack forms in residue classes go to M5's adversarial review.
- **Decided in the contract (D1–D13):**
  - `INJECTS` goes from the injection point's owner to the bean definition (class, `@Bean` method, Spring Data or Feign interface).
  - Spring Data repositories are beans of their own interface type: no synthetic node.
  - RTA is stored as `impl_instantiated: false` only.
  - Precision defaults were `rta` for Q1/Q4 and `cha` for Q2/Q3; amendment 21 made `cha` the default everywhere (`rta` opt-in).
  - Service → Service `via=calls`/`via=http` are derived at query time; `via=package` is stored with provenance `manifest`.
  - Package versions come from `package-lock.json`, `go.mod` and pom literals or properties, in M3b's key form, with `@?` when unresolved.
  - DI scope is the service's classpath and literal scan roots, main sources only.
  - Spring's resolution rules are as modelled in D9, to be confirmed in phase 0.
  - Entry points are an allow-list.
  - Orphans are a greatest-fixpoint reachability computed only in deployable services.
  - Q3 has an API hop up to `--hops`.
- **M4a open items:** 1, 5, 7, 9, 15 and 24 are in M4b. Item 2 (gin parameter groups) moves to M6.
- **Forks for the orchestrator:**
  - F1: service dependencies derived or stored;
  - F2: may `INJECTS` reach `synthetic`;
  - F3: gin groups to M6;
  - F4: orphan eligibility by a whole-word mention scan;
  - F5: Spring Data as a bean rather than a synthetic node;
  - F6: no active-profile evaluation.

  Each has a recommendation.
- No new dependency and no migration are planned.

## M5 Scale & API: complete (closed 2026-10-07, contract amendment 17)

- **Contract:** `tasks/contract-m5.md` (2026-10-07), S1–S28 and invariants 1–11. Phases 0–20 are in `tasks/todo.md`.
- **Soundness claims and closing bar, stated up front** (lesson 2026-10-06):
  - API bodies byte-identical to the matching `rb … --json` output (one request layer, `rb-api`);
  - no silently partial answer: every cap names itself in `truncated`, the per-request budget is a deterministic operation count, and a wall-clock deadline gives an error, never a body;
  - every snapshot read is one `REPEATABLE READ READ ONLY` transaction (today `load_graph` is three separate statements);
  - retention never removes the live snapshot, keeps the `N` newest retired ones, and leaves one AGE graph per repository;
  - `rb diff` reproduces B's dump lines from A's;
  - the AGE projection equals the relational rows (whole vertex set and edge multiset);
  - the server is loopback-only, Host-checked, CORS-free and read-only, with Cypher as the role `rb_reader`;
  - the fetch phase reaches only allow-listed registries (fork F2).

  The residue is listed per claim. Pass 1 is a full review; pass 2 is verification-only; §12.5 misses with profiles do not block (design §13's exit criterion).
- **Decided in the contract (D1–D18):** axum on the existing `tokio` (fork F1); `rb-api` and `rb-server` crates behind `rb serve`; one prebuilt index per live snapshot in the server; the D4 envelope and D5 error/status table; limits from design §11.2 plus `tree`, `work` and `rows`; loopback only, no authentication in M5; read-only pooled connections; Cypher as `rb_reader` (migration 0006); retention with the AGE graph for the live snapshot only, one graph drop per transaction under a database-wide DDL lock, snapshot rows written inside the publish transaction; `rb diff` by natural identity with no move inference and no re-indexing; `--no-cache` writes the facts it extracted, `rb cache prune` and `rb gc` explicit; the benchmark protocol (hardware of record: Apple M4, 10 cores; Docker VM 10 CPUs, 7.75 GiB); the egress proxy design; `?snapshot=` on every question; offset pagination over immutable snapshots.
- **Not in M5:** `POST /v1/repos/{repo}/index`, the `RB_SERVER` thin client, authentication, partition-per-snapshot, edge-delta loading, move detection, and the precision items M4a 16/19/20 and M4b 5/6/7/19 (measured on the benchmark, changed in M6, fork F5).
- **Forks for the orchestrator:** F1 HTTP crate (axum vs `tiny_http`), F2 egress in M5 vs M6, F3 drop retired snapshots' AGE graphs, F4 no move detection, F5 precision items to M6, F6 benchmark repositories (apache/shardingsphere and mattermost/mattermost recommended). Each has a recommendation.
- **New dependencies planned:** the axum stack only (D1), with a measured threshold before phase 9. One migration (0006, a read-only role).
- **Progress:**
  - [x] Phase 0, preflight (contract "Checked in phase 0", amendment 1): the `afae548` binary and fixture dumps as the invariant-1 baseline; `cypher()` works as a reader role once the graph's schema is granted; 80 graphs in one drop transaction exhaust the lock table (40 do not); S2's lock crash not reproduced on `afae548` (0 in 120 concurrent publishes); axum adds 21 crates and 0.68 MiB; Docker Desktop's `--internal` network has no route out and no external DNS; tinyproxy 1.11.3 (144 KiB); benchmark pins recorded (mattermost ≈ 2.2 M lines by estimate).
  - [x] Phase 1 (S1, S2): every read of a snapshot is one `REPEATABLE READ READ ONLY` transaction (`Store::read_tx`/`read_snapshot`, CLI commands read pointer and snapshot together); graph DDL under `RB_AGE_DDL_LOCK`, after the repository row lock.
  - [x] Phase 2 (S3): snapshot rows written inside the publish transaction (a failure records a separate `failed` row); retention keeps the live snapshot plus `--keep` retired ones, drops every other graph one per transaction, never selects the pointer's row.
  - [x] Phase 3 (S4, S5): `rb gc` (scoped by `--prefix`, dry run), `rb cache prune --facts|--scip`, `--no-cache` writes the facts it extracts. On the shared database `rb gc` would drop two retired `fp3_*` graphs: not run.
  - [x] Phase 4 (S7, S8): migration 0006 (`rb_reader`), Cypher in a read-only transaction as `rb_reader` with `--timeout-ms` and a 10,000-row cap (`truncated: rows`); e2e start-up clean-up through `rb gc`'s code; 3 green e2e runs under 8 CPU burners.
  - [x] Phase 5 (S6): `ops` on `QUERIES`/`MIGRATES` edges in AGE (amendment 3), vertices without the redundant `id` property, `rb index --profile` per-label load lines, whole-property parity on every fixture.
  - [x] Phase 6a (S9): `SnapshotIndex` (CSR adjacency, `Send + Sync`) built once per snapshot; every question, Q3's roll-up and the card take it; a depth-1 caller question costs the same operations at n and 4n.
  - [x] Phase 6b (S10): D6's limits in `rb_store::limits`; a deterministic per-request work budget in every build; Q1 `tree`/`work`, Q2 limit names (`nodes`, `work`), Q4 `tree`/`work`, Q3 `nodes`/`work`; property test over random graphs; work cap 20,000,000 (amendment 4).
  - [x] Phase 7a (S12): the `rb-api` crate (D5 errors with exit codes and statuses, the parameter string map with D6 bounds, the envelope, resolution and JSON renderers moved from `rb-cli`); every CLI question runs through it and renders text from its answer; goldens unchanged (amendment 5).
  - [x] Phase 7b (S11, D17): every question's `--json` is the D4 envelope (new: `callers`, `find`, `status`, `snapshots`, `cypher`); `--snapshot` on every question; JSON errors on stdout with D5's exit codes; the card metered; `RB_TEST_CAPS` (amendment 6).
  - [x] Phase 8 (S15, D18): `limit`/`offset` with `total` and `next_offset` on `find`, `services` and `snapshots`; `find` as one `LIMIT/OFFSET` query and one count; a page examines at most `offset + limit` elements (counted).
  - [x] Phase 9 (S13): `rb serve` (crate `rb-server`, axum 0.8.9 over the existing tokio: 21 crates, +1.17 MiB): loopback bind only, Host allow-list (421), no CORS, `OPTIONS`/wrong methods 405, Cypher `POST` needs JSON (415), unknown parameters 400; 4 workers + 16 queued, then 503 `busy`; a deadline gives 503 `deadline_exceeded`, never a body; pooled read-only sessions; one index per live snapshot shared by the workers; `Server-Timing` only (amendment 7). 
  - [x] Phase 10 (S14): the API ≡ CLI suite: 132 parameter sets over the three fixtures (defaults, maxima, `rta`, `pick`, ambiguous, missing, out of bounds, `snapshot=`), a deleted snapshot, capped answers on both sides: byte-identical bodies, statuses matching exit codes by D5, a second server identical. 
  - [x] Phase 11 (S16): `rb_model::diff`: nodes and edges by natural identity (added, removed, changed; evidence-only edges apart), `apply` (a checked patch of dump lines), `invert`, the diagnostics' multiset difference; the properties hold on random graphs; the dump renders through the same rows. 
  - [x] Phase 12 (S17, S18): `rb diff [<from>] [<to>]` and `GET …/diff`: summaries (endpoints, table access, service dependencies, certainty shifts) by the card's rule, the exact sections paged, the tool versions that differ; a two-commit polyglot history whose summary rows were predicted; `base: unresolved` on suffix edges without a host (`PIPELINE` `m5-1`, one fixture line changed, amendment 8).
  - [x] Phase 13a (S19 a, b, f; `PIPELINE` `m5-2`, amendment 9): discovery and load-balancer words and JSON key paths are read in every file, parsed Java included; XML comments are read; a contested base (a resolvable spread in an axios request config, disagreeing Spring bases) whose candidates name an external host makes the site external. Fixtures: the polyglot `fetch` sites are capped by a pom comment (counter only; one diff-golden edge `synthetic` → `heuristic`); the four M4b repositories unchanged by S19.
  - [x] Phase 13b (S19 c, d, e, g; `PIPELINE` `m5-3`, amendment 10): Spring Data fragment implementations are not orphan-eligible (jhipster's `OperationRepositoryWithBagRelationshipsImpl` was a false one); rule (e) trusts a supertype head's name resolution only when unique and not importable from outside; a class registered by a class literal is a DI candidate, so the point is `ambiguous` and `@Primary` never picks past it.
  - [x] Phase 14 (S20, amendment 11; no code): residue records for M4b 18 and 20 and M4a 22 under this contract; Maven's duplicate handling observed with the scip-java image's Maven offline (dependencies and properties: last wins; dependency management: first), undocumented, so `@?` stays; S19's new forms listed for M6.
  - [x] Phase 15 (S21, S22; `RECIPE` `m5-egress-1`, amendment 12): every SCIP fetch runs behind its own tinyproxy on an `--internal` Docker network, allowing only `CONNECT` on 443 to Maven Central and the npm/Yarn registries plus host-configured extras (`--scip-registries`; never the repository's `rb.toml`, since the repository is the adversary); the proxy image is built locally from the pinned Alpine base with `tinyproxy=1.11.3-r0`, and without it no fetch runs; package caches are per repository and the Develocity build caches are off (spring-retry: 68 → 0 files without a document); the probe suite proves each forbidden path closed; `rb gc --egress`. Next: phase 16 (benchmark set-up).
  - [x] Phase 16 (S23, amendment 13): both pins fetched shallowly and counted: shardingsphere 720,005 supported lines (Java 708,960; Maven, 446 poms), mattermost 1,698,802 (Go 818,222, TypeScript 793,519, JavaScript 85,891; npm and Go); the release binary of `17410d7` and the benchmark harness (`time -l`, `sample`, load and disk around every run).
  - [x] Phase 17a (S24 on shardingsphere; amendment 14): a publish ANALYZEs `rb.node`, `rb.edge`, `rb.edge_evidence` and `rb.diagnostic` before its commit, so the first reads of a new snapshot no longer get a nested-loop plan on stale statistics (a dump after a publish: >10 min before, 1.3–2.6 s after). shardingsphere: `--no-scip` 35.0 s fresh / 23.4 s cached / 18.5–19.2 s incremental; SCIP 1,878.9 s fresh (≈ 20.6 min without the fetch: §12.5's 15 min missed, the single 2-CPU scip-java job dominating) and 49.5 s cached; deterministic, parity OK; 173 MiB relational + 64 MiB AGE per snapshot.
  - [x] Phase 17b (S24 on mattermost): `--no-scip` 29.2 s fresh / 19.6 s cached / 17.9–19.6 s incremental; SCIP 610.6 s fresh (4 of 11 TypeScript projects index; the webapp's workspace packages and two e2e projects with `file:` links outside their root fail their fetch, M6), 24.0 s cached; deterministic, parity OK; 121 MiB + 44 MiB AGE per snapshot; S18/S19 change nothing there.
  - [x] Phase 18 (S25): latency through a warm `rb serve`, two runs per repository: Q1 depth 1 p95 1.6–3.3 ms, Q2 on the 20 hubs p95 94–152 ms, Q4 p95 3.6 ms (mattermost's 66 endpoints; shardingsphere has none), all within §12.5; no `truncated` answer; snapshot index built in 0.57–0.63 s, server RSS 249–284 MiB; cold CLI 0.4–0.6 s per question.
  - [x] Phase 19 (S26): no heuristic or `synthetic` `CONSUMES` on either benchmark repository; mattermost's 8 `ambiguous` are all false (in-process Go test requests matched by the relative-URL convention; M6 items); the M4a forms counted (none reaches an edge); Gate A lowers 0 edges; no DI, `INJECTS` or RTA proof to sample; mattermost's one sampled orphan is true; S19 costs nothing there. Benchmark artifacts removed and the database vacuumed.
  - [x] Phase 20, records (S27, S28 except the evaluator passes; no code): invariant 1 re-checked with the HEAD binary against `afae548` — fixtures and the four M4b repositories differ only in the rows S18 and S19 (c–g) named (12 changed rows in all, each classified), and every dump equals phase 13b's byte for byte; the contract's open items consolidated with target milestones (M6 unless stated), its measurement section corrected (phase 19's disk figures); design §8.3, §8.4, §11, §12.5, §13 and §14 updated as decided and measured; README: `rb serve`, `rb diff`, the JSON envelope, `--snapshot`, pages, `rb gc`, `rb cache prune`, `--keep`, `--timeout-ms`, `--profile`, the egress proxy and per-repository caches, each example run as printed against the database (repository `m5r_polyglot`, removed after); the port references say both 55432 (default) and 55433 (this machine). Next: evaluator pass 1 (full review), then pass 2 (verification only).
  - [x] Evaluator pass 1 fixed (amendment 16): `status` of a never-indexed repository is `not_found`; a publisher killed between its flip and its retention can no longer make Cypher answer on a retired snapshot (reads decide from the status row), and the leftover graph is dropped by the next publish or `rb serve`'s startup sweep; path segments percent-decoded once; duplicate `Host` and foreign absolute-form authorities refused; `failed`-snapshot and multi-graph retention rows (mutants m16, m17 killed); hyper's bare transport refusals recorded as residue; adversarial egress review carried to M6. 16 mutants killed; invariant 1 holds; stale facts rows pruned.
  - [x] M5 closed (contract amendment 17): evaluator pass 2 (`tasks/eval-m5-pass2.md`, verification only, against `5cb1a2c`) VERDICT PASS — Z1–Z7 hold end to end (the lock + `kill -9` repro healed once by `rb serve`, once by the next publish), 12/12 mutants of amendment 16 killed from a clean build, the verification loop green twice (639 unit, store 65/65, server 12/12, e2e 30/30 with containers, api 1/1), invariant 1 held (fixtures equal phase 20's; spring-petclinic the one S19 (d) row against `afae548`), fresh == cached on spring-petclinic, shardingsphere and mattermost (both equal to their phase-17 dumps), API ≡ CLI on 29 own parameter sets. Notes N1, N2 (non-blocking) are open items 24, 25 (M6).

## M6 Breadth: split into M6a–M6j (planned 2026-10-07)

- **Split** (design §13, one row per sub-milestone with scope and exit criteria; each gets its own contract when it starts, soundness claims and closing bar up front, evaluator pass 1 full and pass 2 verification-only). Ordered by value and dependency: harden what exists first, then new languages, then packs, then the rules engine, then the remaining languages and DI frameworks.
  - **M6a SCIP coverage on Java and TypeScript:** contract-m5 open items 1, 3, 4, 5, 8, 24, 25.
  - **M6b Request-path precision:** gin groups as parameters, Express construction chains, contract-m5 6, 7, 10 (part), 11, 12, contract-m4a 14 (part), 16, 19, 20, 21 (rest).
  - **M6c DI, RTA and orphan precision:** contract-m5 9 (part), 10 (part), 13, 17 (contract-m4b 5, 6, 7, 19); contract-m4b 4, 8, 14, 16, 17.
  - **M6d Go and Python SCIP:** scip-go (image built locally), scip-python (pinned by digest after measuring), contract-m4b 13, Python lockfiles, contract-m5 22 (egress review) and 9 (Go at scale).
  - **M6e Build and lockfile breadth:** Gradle, pnpm/Yarn lockfiles and workspaces, contract-m4b 22, contract-m5 18.
  - **M6f Contract packs:** OpenAPI, gRPC, contract-m4b 9.
  - **M6g Topology and configuration packs:** message topics, gateway configs, compose/k8s hosts, Spring YAML and active profiles (contract-m4a 3, 10, 13, 18).
  - **M6h Architecture rules as a CI gate.**
  - **M6i Kotlin and C# syntax** (contract-m5 14, Kotlin part).
  - **M6j DI breadth:** Guice, Dagger, NestJS, Spring XML and Kotlin beans, .NET over Tier B.
  - **Deferred beyond M6:** scip-clang, `rust-analyzer scip`, scip-dotnet (1,253 MB compressed image), the SCIP incremental below a single-root reactor (contract-m5 2), authentication/remote binding/`POST …/index` and move detection (contract-m5 19, 20: only if asked for). **Dropped:** Groovy registrants (contract-m5 14, Groovy part). Residue items 15, 16, 23 and the not-planned 21 stay recorded.

## M6a SCIP coverage on Java and TypeScript: in progress (phases 0–9 by the generator; 10–12 later)

- **Contract:** `tasks/contract-m6a.md` (2026-10-07), S1–S20, invariants 1–11, D1–D12, forks F1–F7. Phases 0–12 are in `tasks/todo.md`.
- **Soundness claims and closing bar, stated up front:**
  - a scip-java document is accepted only when every existing agreement check passes on what remains after removing the occurrences Lombok marks as generated (`@lombok.Generated`'s span, measured in probe P2), so no exact edge has a generated member as its source, target or anchor;
  - a TypeScript interface-method `IMPLEMENTS` needs scip-typescript's own member relationship and the declared-clause walk (probe P1: scip-typescript relates interface methods, sub to super only, and interface types to nothing);
  - workspace grouping (a root lockfile and an allow-list of glob forms) changes only which job indexes a project, and a job exports exactly its root plus in-repository link targets, all in its cache key;
  - the budget flags never change an index (measured), and a threaded Maven build has its own cache key;
  - `rb serve` writes nothing before its bind check.

  Residue per claim (generated members' callers stay unanchored; builds without the marker stay rejected; pnpm, Yarn ≥ 2 and Yarn links stay as today). Pass 1 full, pass 2 verification-only.
- **Decided in the contract:** D1 Lombok's marker as the only identification, removal before the unchanged checks; D3 new counters only when non-zero (invariant 1 on every unrelated dump); D4 the TypeScript interface waiver gated by a phase-0 directionality measurement; D5 workspace membership; D6 root fetch; D7 repository-shaped exports with link targets; D8 per-indexer recipes (Java's key unchanged, so cached Java indexes stay valid); D9 host-side budget flags; D10 the start-up order; D11 measurement repositories (apache/shardingsphere and mattermost/mattermost at M5's pins, a second Lombok repository by rule); D12 every M5 open item's target.
- **Forks for the orchestrator:** F1 gin groups in M6b (recommended) vs M6a; F2 open item 5 in M6a with a decided fallback; F3 the TypeScript interface waiver; F4 threads and flags vs per-module jobs; F5 no nodes for generated members; F6 the M6 split; F7 git hosts for mattermost's measurement from the indexing host only.
- **New dependencies:** none. No migration.

### Status
- **Phase 0 (preflight, 2026-10-07):** done. Baseline binary `var/m6a/base/rb-cfa8ab9` (HEAD's code = `cfa8ab9`'s); invariant-1 baselines in `var/m6a/base/{noscip,scip}/`; P1b: 0 super→sub member relationships over 13 forms (D4 is built); P2b: every generated definition is enclosed exactly by a `lombok/Generated#` marker span, S2's shadow placement confirmed, 11 generated references not at a marker (multi-line `<init>` spans of builders, `@UtilityClass`'s exception), none on a Tree-sitter token; F2's `npm ci` link check passes (item 5 stays); `ts-workspaces` lockfiles generated in the scip-typescript image offline; mattermost's workspace patterns all literal; second Lombok repository elunez/eladmin @ `55fbf705`; `--scip-memory 6g` fits beside Postgres. Verify loop at baseline: 639 unit, `rb-store` 65/65, `rb-server` 12/12, e2e 30/30, api 1/1. Amendment 1 (layout check moved to phase 7).
- **Phase 1 (`rb serve` start-up order, two-repository sweep; S14, S15):** done. `check_bind` before `sweep_leftover_graphs`; e2e row in `a_publish_killed_after_its_flip_self_heals` (failed first), `rb-store` `the_startup_sweep_covers_every_repository_it_is_given` (kills the first-name-only mutant alone). Verify: 640 unit, `rb-store` 66/66, `rb-server` 12/12, e2e 30/30, api 1/1.
- **Phase 2 (Lombok removal and counters; S1, S2 units, S3):** done. `rb_scip::symbol::is_lombok_generated_marker`; `remove_lombok_generated` before every check in `join` (scip-java only); `lombok_removed`/`documents_lombok` in the coverage JSON and the status line only when non-zero; `PIPELINE` `m6a-1`. P2b re-indexed: 21 of 22 documents accepted (509 removals); `Consts.java` stays rejected (amendment 3). Verify: 651 unit, `rb-store` 66/66, `rb-server` 12/12, e2e 30/30, api 1/1.
- **Phase 3 (`java-lombok` fixture; S2 end to end, S4):** done. `fixtures/java-lombok` (7 sources, Lombok 1.18.34 `provided`), goldens `scip-status.txt` (`documents rejected 0`, `lombok-generated 234 in 6 documents`, 9 exact calls), `scip-callers-greet.txt`, `dump.jsonl`, SCIP guards; e2e `scip_java_lombok`. Verify: 651 unit, `rb-store` 66/66, `rb-server` 12/12, e2e 31/31, api 1/1.
- **Phase 4 (TypeScript interface-method `IMPLEMENTS`; S5–S7):** done. `reaches_supertype` takes the origin's relations as an option; a scip-typescript interface origin walks its declared clauses alone. `PIPELINE` `m6a-2`. Amendment 4 (S5's mutation needs its own killers). Verify: 656 unit, `rb-store` 66/66, `rb-server` 12/12, e2e 31/31, api 1/1.
- **Phase 5 (workspace membership; S8):** done. D5 in `rb_scip::jobs` (manifest reader in `projects.rs`; `serde_json` added to `rb-scip` from the workspace, no new crate); `workspace_members` in `scip_status` and the status line only when non-zero; ungrouped reasons printed by `rb index` only (amendment 5). Verify: 662 unit, `rb-store` 66/66, `rb-server` 12/12, e2e 31/31, api 1/1; clippy fixed after the loop (two `.0` statements in tests), re-checked.
- **Phase 6 (workspace fetch/index, per-indexer recipes; S9):** done. `RECIPE_JAVA` `m5-egress-1` (pinned key), `RECIPE_TS` `m6a-ts-1`; `project_input_hash` takes the job's recipe; `PIPELINE` `m6a-3`. A grouped job fetches at its root with today's commands and lists every member project (D6, already the behaviour of phase 5's job list; pinned by a unit). Verify: 664 unit, `rb-store` 66/66, `rb-server` 12/12, e2e 31/31, api 1/1.
- **Phase 7a (repository-shaped export, link targets; S10 part 1):** done. `ContainerRun::cwd`; scip-typescript exports at repository paths, phases in `/src/<root>`; `projects::job_files`/`link_targets`; `unsupported` for a link outside the repository; the key over exactly the exported files (amendment 6). Layout check passed. Verify: 669 unit, `rb-store` 66/66, `rb-server` 12/12, e2e 31/31, api 1/1.
- **Phase 7b (link-target and outside-root counters; S10 part 2):** done. Verify: 671 unit, `rb-store` 66/66, `rb-server` 12/12, e2e 31/31, api 1/1.
- **Phase 8 (`ts-workspaces` fixture; S11):** done. Verify: 671 unit, `rb-store` 66/66, `rb-server` 12/12, api 1/1; e2e 32/32 on the re-run after the cold-cache fix (the loop's run: 31/32). Amendment 7 (`--no-scip` W rows on grouped workspaces).
- **Phase 9 (budget flags; S12):** done. Verify: 674 unit, `rb-store` 66/66, `rb-server` 12/12, e2e 32/32, api 1/1.
- **Phase 10 (shardingsphere; S13, S16):** done 2026-10-09. Base fresh (wall 3062.77s, `ok` 2950.5s; Oct-7 attempt died at the disk floor under load 43, retry from 16.1G under load ~4 completed, floor never below 10.5G): rejected 3889 as hypothesised. M6a cached (72.7s): rejected 0, `lombok-generated 107101 in 3951 documents`, exact CALLS 27364 → 68500. Newly accepted 3889 / rejected 0; 40/40 sample read in source (`var/m6a/bench/out/ss-s16-sample.txt`). S13: two fresh threaded runs (`--scip-cpus 8 --scip-memory 6g --scip-maven-threads 4`) both `failed (index phase)` at 1297.8s/1295.7s (maven-compiler-plugin on `parser-sql-engine-mysql` under `-T4`; default compiles it): dumps identical modulo the one allowed `stderr_tail` row, so the flag stays (D9). §12.5 900s missed with profile (amendments 8–11).
- **Phase 11a (eladmin; S16):** done 2026-10-09. Lombok 1.18.30 emits no `lombok/Generated#` (measured bytewise): base fresh == M6a cached == M6a `--no-cache --force` fresh, byte-identical; residue case, vacuous 40-sample.
- **Phase 11b (mattermost; S17):** done 2026-10-09. Base 11 jobs/4 ok → M6a 5 jobs/3 ok, 4 ok with `--scip-registries github.com,codeload.github.com` (F7: cypress git+ssh dep installs; webapp `EBADENGINE` — image node v24.6.0 vs required 24.11.1, new residue → M6e). Interface `IMPLEMENTS` sets identical (T = 0; all 55 extends clauses in unindexed webapp). `mm_account.py`: 19,213 differing rows, UNACCOUNTED 0. Fresh == cached byte-identical.
- **Phase 12 (records; S18–S20):** done 2026-10-09. Fixtures `--no-scip` identical except `ts-workspaces` (3 amendment-7 W rows); SCIP identical except `java-lombok` (L-only) and `ts-workspaces` (T+W-only). D7 quadratic mutant bites (restored identical). Verify `var/m6a/logs/p10-12-*`: fmt, clippy, 674 unit, 66 store, 12 server, 32 e2e, 1 api, no `skipping:`.
- **Evaluator pass 2 (`tasks/eval-m6a-pass2.md`, 2026-10-09): VERDICT PASS** — Z1 holds (repro fails pre-fix, passes now), suite green (676/66/12/32/1), invariant 1 re-derived with a fresh HEAD release, determinism spot identical, M1/M4/M6 killed; O1 + wall-clock flake recorded non-blocking. **M6a closed.** Next: M6b.

## M6b Request-path precision: complete (closed 2026-10-10, contract amendment 11; evaluator passes 1–2 PASS)

- **Phase 0 (preflight, 2026-10-09):** done. Baseline binary `var/m6b/base/rb-3562b95` (sha `ede3e290`, == evaluator's independent build); fixture dumps `--no-scip` + `--scip` equal M6a's closing hashes on all 5. Shape checks (shallow checkouts, removed): gin literal args + nested `v1.Group` composition; express `routes.ts` prefix-less chain + anonymous default; ftgo constructor-member shapes; full ftgo pin recorded. Contract amendment 1. DB 0 repos.

- **Phase 1 (neutral facts; S1):** done. `FileFacts.member_writes` (`member`, `value: Option<Init>`, `enclosing`, `ctor`, `span`; TS/JS `this.x`, Java `this.x`, compound assignments included, bare fields excluded); gates/sinks verified to need no new facts. Tests first (4: every RHS shape, non-member shapes, Java shapes, Go/Python empty; positions source-derived). Verify: rb-syntax 96/96, e2e 32/32 with unmodified goldens. Contract amendment 2.

- **Phase 2 (gin parameter groups; S2):** done. `gin_group_prefix` (frameworks) folds a `Group()` chain through caller bindings; the indexer composes param-group registrations from uniquely resolved call sites (one identity per site in evidence, path-based keys, heuristic max; unknown stays iff no site or some site fails). Tests first (framework fold unit, same-file/cross-package/dynamic-fallback/growth indexer units). Verify: frameworks 98, indexer 239, e2e 32/32 with unmodified goldens. Contract amendment 3. `PIPELINE m6b-1`.

- **Phase 3 (Express chains; S3):** done. Chain-built bindings, per-link `use`/verb handling, anonymous default-exported routers (`default_export_expr` span fact + `{path}#default` identity + default-import fallback). Tests first (fold/shape units, chain/growth units, node-express-shaped composition e2e-unit). Mutants M-c/M-d/M-e bite. Verify: syntax 97, frameworks 101, indexer 240, e2e 32/32 with unmodified goldens. Contract amendment 4. `PIPELINE m6b-2`.

- **Phase 4 (in-process + empty URL; S4):** done. Pack-side sink walk (`go_in_process`: seeds, three positive sink patterns, `Do` carve-out, depth 8 + visited, scope-indexed resolution) feeding `FileClients.in_process`; stitcher buckets (`unmatched` split into plain + non-zero-only `http_in_process`/`http_empty_url`, own diagnostic kinds, sum test extended); `rb status` labels; Service Card `unstitched`. Tests first (9 pack units incl. growth + cycle bound, 4 stitcher units). Fixture per D10 (billing `main_test.go` + 3 must-not-exist guards; notifyPaid's edge intact). Mutants killed per amendment 5. Verify: fmt, clippy clean; frameworks 110, indexer 244, store 66, cli-bin 25, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos. Contract amendment 5. `PIPELINE m6b-3`.

- **Phase 5 (axios/certainty folding; S5):** done. `create` contested via spreads (+ by-name object/string); single-arg objects parsed as configs; request-config Names resolved (entries/string-kept/dynamic-forced); spread Names emitted for cross-file resolution; `fold::object_values` (unique M3a, visited + shared `MAX_SPREADS`); stitcher expands Names into `base_candidates` (D5 externality); Spring `of()` later-wins (span-ordered, M5 fallback on unreadable, heuristic-capped by carrier scan). Tests first (4 pack units incl. growth, 2 fold units incl. growth, 1 stitcher unit; 5 existing pack/stitch expectations updated as predicted). Fixture per D10 (web `opts.ts` + `resendViaCfg`/`resendExternal` in `mfa.ts`; 1 must-exist + 3 must-not-exist guards, all biting). Mutants killed per amendment 6. Verify: fmt, clippy clean; frameworks 116, indexer 245, store 66, cli-bin 25, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos. Contract amendment 6. `PIPELINE m6b-4`.

- **Phase 6 (constructor members; S6):** done. Member-write facts for every form on any object (`on_this` flag; bare fields still out); per-file single-assignment proofs (class + same-file subclasses, assign-kill, shared `fold_init` allow-list, value folded at the write); reads via `this.`/member bindings (TS/JS/Java only); one shared `Folder` per `sites()` run (fresh step budget per fold). Tests first (2 extraction units, 3 fold units incl. growth; 3 existing expectations updated as predicted). Fixture per D10 (web `NotifyClient`; 1 must-exist guard, biting). Mutants killed per amendment 7. Verify: fmt, clippy clean; syntax 98, frameworks 119, indexer 245, store 66, cli-bin 25, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos. Contract amendment 7. `PIPELINE m6b-5`.

- **Phase 7 (userinfo holes; S7):** done. `strip_userinfo_hole` pre-pass in `client_url` (scheme-gated, last-`@`, hole-before-`@` only). Tests first (dedicated unit + 2 updated M4a rows + 1 stitcher unit). Mutants killed per amendment 8. Verify: fmt, clippy clean; frameworks 120, indexer 246; polyglot e2e byte-identical (no fixture/userinfo shapes elsewhere). Contract amendment 8. `PIPELINE m6b-6`.
- **Phase 8 (discovery narrowing; S8):** done. `clients::discovery_flags` (broad, narrowed); `discovery_resolved` (5-type allow-list over annotation/supertype/declared-type/return positions, explicit/on-demand/FQ import resolution, Java-gated); `single_literal_discovery` (linear brace-stack scan + nested propagation); stitcher `discovery_broad` + `discovery_lifted` marker + non-zero-only `http_coverage.discovery_lifted`. Tests first (4 pack/fold units incl. growth + cycle-shape, 1 flags unit, 1 stitcher unit, 1 syntax update — all failed first as compile errors/assertions). Mutants killed: 24 targeted (every arm, the Java gate, both scan arms, marker/count/coverage) + quadratic rescan (693,100 → 11,503,300 vs 5×). Verify: fmt, clippy clean; frameworks 125, indexer 247, store 66, server 12, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos; fixtures trigger-free so every dump byte-identical (0 lifts; invariant 1 transitive vs phase 7). Contract amendment 9. `PIPELINE m6b-7`.
- **Phase 9 (client breadth; S9):** done. got/ky/superagent/undici fetch-class sites (verbs incl. `del`, direct with opts-method, undici namespace, extend/create/agent carriers, destructured members — all reads capped except plain module verbs/directs); HttpClient import-gated, this./local typed receivers, always capped; axios member forms via new `Init::Member` fact (fold-hole) + Name-dot + inline require + `.default`, all capped; imported-fallback exclusion (no doubles); hooks per-lib allow-lists + mentions taints + destructure allowances; RAW_SURFACE +4 words. Tests first (6 pack/hook/raw units + 1 syntax + 1 fold — assertion/compile failures recorded). Mutants killed: 29 (2 survivors forced new pinning rows). Fixture `clients.ts` (10 sites, 10 must-exist + 10 must-not-exist guards, all biting first try); goldens classified per hunk (25 sites incl. 10 new heuristic; orphan pattern-consistent; scip/diff/path/callers commit-sha-only +1 scoped unresolved-ref). Verify: fmt, clippy clean; frameworks 132, indexer 247, syntax 99, store 66, server 12, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos. Contract amendment 10. `PIPELINE m6b-8`.
- **Phase 10 (measurements; S11):** done 2026-10-10 (`var/m6b/bench/phase10.md`, pins verified by API). `--no-scip --no-cache --force`, base `rb-3562b95` vs head `rb-m6b-p9` (release): gin 19 unknown→26 composed + 44 unmatched/2 dynamic→33 in-process/18 self (sampled); express 19 unknown→19 composed; ftgo 4 dynamic→3 heuristic + 1 genuine 3-way ambiguous (all read); mattermost 8 ambiguous edges removed + 39 unmatched/4 dynamic→53 in-process/8 empty (sinks read, incl. production imageproxy); microservices + piggymetrics byte-identical. S8 calibration: `discovery_lifted` 0 everywhere (0 unintended lifts). DB 0 repos after batch `delete_repo` + `VACUUM`; disk 19G free.
- **Phase 11 (records + evaluator; S12–S14):** done 2026-10-10. Pass 1 (`tasks/eval-m6b-pass1.md`): adversarial battery — E1 (superagent `.use`, got/ky `hooks`/`agent`/by-name/spread options, undici `dispatcher` now cap per site) + E2 (`ReactorLoadBalancer` allow-listed), both test-first with 5 fix mutants killed; 4 green confirmations. Pass 2 (`tasks/eval-m6b-pass2.md`): verification-only — fmt, clippy, 732/66/12/32/1 green, 58 mutants re-killed (24+29+5), invariant 1 + determinism with no re-bless, E1/E2 trigger grep over re-cloned repos. Contract amendment 11: **M6b closed.** `PIPELINE m6b-8`.

## M6c DI, RTA and orphan precision: in progress (contract `tasks/contract-m6c.md`, draft 2026-10-10; forks F1–F7 all as recommended)

- **Phase 0 (preflight, 2026-10-10):** done. Baseline binary `var/m6c/base/rb-10a9ad5` (sha `264fec11`); fixture dumps `--no-scip` identical to goldens where pinned, SCIP hashes equal M6a's recorded values; full-SHA pins by API (petclinic org corrected); shape checks on shallow checkouts, removed. Contract amendment 1.
- **Phase 1 (registrants; S1):** done. `di::bean_method_registrants` (`@Import`-named configs' assignable `@Bean` methods join as `origin: registrant`; Y4's residue closed, its test updated as predicted). Tests first (9 shape rows, failed first). Mutants killed per amendment 2 (7: drop, import-name, class-suffix, test-filter ×2, bean-check, shared dedup — needed a candidates-count assertion since the merge hides duplicates — array-items). Verify: fmt, clippy clean; workspace 733, store 66, server 12, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos. No fixture `@Import`-named configs: e2e byte-identical, no `PIPELINE` bump. Contract amendment 2.
- **Phase 2 (gate M main-only; S2):** done. Test-path `ClassLiteral` mentions exempted from the mention set (D2); `mentioned_broad` + `gate_m_broad` mirror arm-for-arm; `mention_lifted` counted iff broad-caps-but-plain, non-zero-only in `di_coverage`. Tests first (registrant/bean/type arms + Other control + coverage asserts, failed first). S19g's and S1's test-source rows updated to plain as predicted (M4b item 5 closed). Mutants killed per amendment 3 (6 + 1 documented arm-overlap). Verify: fmt, clippy clean; workspace 734, store 66, server 12, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos. No fixture test dirs: e2e byte-identical, no `PIPELINE` bump. Contract amendment 3.
- **Phase 3 (deployable roles; S3):** done. `app_services` (main-source `@SpringBootApplication` classes by module) feeds one `deployable_services` predicate for roles, DI classpath and RTA. Tests first (flip + library/test/TS/method controls, linear growth). Mutants killed per amendment 4 (6 incl. quadratic rescan). Verify: fmt, clippy clean; workspace 736, store 66, server 12, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos. No fixture flips: e2e byte-identical. Contract amendment 4.
- **Phase 4 (module objects; S4):** done. `rta::AliasMaps` (`tsconfig.json` `paths` + `baseUrl`, `package.json` `exports`/`main`; nearest scope wins; exact-beats-pattern, longest-pattern wins; multi-target/multi-star/computed/`extends`/non-string resolve nothing) shared by Tier B (`Modules::with_aliases` + `probe`) and RTA (`module_may_name` exact branch, else approximate keep-all); `tsconfig.json` is a manifest input; per-pattern `work::tick`. Tests (Tier B bare-specifier unit, RTA exact/nowhere/unknown/multi e2e rows, precedence unit incl. `./a`-vs-`./a*`, input pin, linear growth). Mutants killed per amendment 5 (6 lib + wiring e2e-killed; one survivor strengthened the test). Fixture `legacy/` (`main.ts`, `models/user.ts`, `ext.ts`; 1 must-exist + 1 must-not-exist guard, SCIP basename-quoted key). Verify: fmt, clippy clean; workspace 739, store 66, server 12, e2e 32/32 (no `skipping:`), api 1/1; final tree indexer 256 + `scip_ts_workspaces` green; no listener; DB 0 repos, 0 graphs. Contract amendment 5. `PIPELINE m6c-1`.
- **Phase 5 (rule (e) levels; S5):** done. `levels()` returns the winning `TypeLevel`; `external_supertype` counts external only on ambiguous/miss, dotted-presence (unchanged), or unique on-demand + outside wildcard — unique local/single/same-package resolutions count in-repo (D5). Tests first (failed first); 4 predicted flips in the old rule-(e) test; linear growth test. Mutants killed per amendment 6 (5; one survivor gained the on-demand + outside-single shape). No fixture flips: e2e byte-identical, no `PIPELINE` bump. Verify: fmt, clippy clean; workspace 743, store 66, server 12, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos. Contract amendment 6.
- **Phase 6 (non-bean consumers; S6):** done. `bean_method_consumers` (assignable closure + created types, symbol-level) + `ConstraintValidator` allow-list; one bucket count per point, one edge per (consumer, candidate), certainty forced `heuristic`. Tests first; multi-candidate pin; accumulator-aliasing fan-out bug caught and fixed (`split_off`). Mutants killed per amendment 7 (6). Fixture D10 (`JwtFilter`, `EmailValidator` with real annotation + `isValid`, factory method, managed validation dep) + 4 guards; goldens classified per hunk (status-di +2 plain; card/scip-card deltas; scip-injects byte-identical edges; kafka line-shift; impact sha). `PIPELINE m6c-2`. Verify: fmt, clippy clean; workspace 744, store 66, server 12, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos, 0 graphs. Contract amendment 7.
- **Phase 7 (inherited Spring Data; S7):** done. Post-`INJECTS` producer `add_inherited_calls` (allow-list `save/find/get/delete/count/exists`, DI Spring Data receivers, Tier B no-double, SCIP-exact skip); `Resolution.spring_data` carries the beans. Tests (+growth); harness runs Tier B in pipeline order. Mutants killed per amendment 8 (5 lib + exact-gate e2e-killed; one survivor isolated the non-listed call in its own method — same-key edges merge). Fixture D10 (`SignupService`) + 3 guards; wiring bite proven. Goldens classified per hunk. `PIPELINE m6c-3`. Verify: fmt, clippy clean; workspace 746, store 66, server 12, e2e 31/32 + impact-users bless, api 1/1; no listener; DB 0 repos. Contract amendment 8.
- **Phase 8 (Lombok qualifiers; S8):** done. `LombokConfigs` nearest-chain reader (`=`/`+=`/bare-`!`, root-first, `stopBubbling` shield, unread-on-garbage) through `di()` into `lombok_point` (listed qualifiers ride with `copied_qualifier`, else old row); indexer reads `lombok.config` manifests, counts kept qualifier filters in non-zero-only `lombok_narrowed`. Tests (+growth); 8 mutants killed (stop survivor forced the `+=` discriminator). Fixture D10 (managed lombok dep, config, `MailService`) + 2 guards; goldens classified per hunk (scip edge mirrors no-scip). `PIPELINE m6c-4`. Verify: fmt, clippy clean; workspace 751, store 66, server 12, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos. Contract amendment 9.
- **Phase 9a (exact type-arg agreement; S9):** done. Wildcard marks in extraction (behavior-preserving strips at resolution uses); clean-or-nothing point `type_args`; per-bean supertype clauses; single pick at heuristic capped by the generic gate, counted in `type_args_narrowed`. Tests first (+growth); Y5 flipped as predicted; 6 mutants killed (2 redundant checks removed after survivors proved them unobservable). Fixture D10 (`Channel.java`) + 2 guards; goldens classified per hunk (scip edge mirrors no-scip). **Phase 9b (custom-qualifier metas) deferred as residue** (amendment 11): needs syntax-visible annotation declarations (verified absent); nil incidence in measurement repos; non-blocking per closing bar; mechanism specified. `PIPELINE m6c-5`. Verify: fmt, clippy clean; workspace 753, store 66, server 12, e2e 32/32 (no `skipping:`), api 1/1; no listener; DB 0 repos. Contract amendments 10–11.
- **Phase 10 (fixture proofs for S1/S2; S10):** done. `AppImports` + out-of-roots `SmsConfig` + consumer `Alerts` + test source `AlertsTest`. E2E: Alerts ambiguous over 2 scanned + registrant (listed, never single); name fallback falls to ambiguous (predicted); collections scanned-only with mention cap; test bucket +1 with no lifts (S2 calibration). Goldens classified per hunk on both tiers. No source change. Contract amendment 12.
- **Phase 11 (evaluator pass 2 + closeout; S12–S14):** done 2026-10-11. Pass 2 verification-only (`tasks/eval-m6c-pass2.md`): VERDICT PASS — fmt, clippy clean; 753 unit, 66 store, 12 server, 32/32 e2e (no `skipping:`), 1/1 api; 57 mutants re-killed (2 anchor fixes, 3 documented); invariant 1 + determinism hold; E1/E2 repros + trigger grep clean. Contract amendment 15: **M6c closed.** Design §13 row updated. DB 0 repos.
- **M6d phase 1 (scip-go image; S1):** done 2026-10-11. `docker/scip-go/` (pinned Ubuntu/Go/scip-go-source, checksums recorded); `rb-scip-go:0.2.7` built locally, versions verified, stdlib smoke indexes offline. Wiring: image const, `Indexer::ScipGo`, `Fetch::Go` (go.sum-gated pinned), per-module jobs, `m6d-go-1` recipe. Tests first; 4 mutants killed. Contract amendment 2.
- **M6d phase 2 (scip-python pin; S2):** done 2026-10-11. v0.6.6 digest-pinned after measuring (188.8 MB compressed, 731.6 MB actual); `Indexer::ScipPython`, per-manifest jobs, empty fetch vec, offline `scip-python index`, emulated platform. Tests first; 4 mutants killed; smoke green (bare projects crash it — planning stays gated). Contract amendment 3.
- **M6d phase 3 (Go fetch; S3):** done 2026-10-11. `proxy.golang.org` allow-listed; Go fetch pins `GOSUMDB=off`; e2e egress probes extended (gomod 200, gosum refused, refusals array). Tests first; 2 mutants killed; chi-bearing module fetches + indexes `scip-go ok` end-to-end. No dump change. Contract amendment 4.
- **M6d phase 4 (Python fetch; S4):** done 2026-10-11. `Fetch::Python{PythonLock}` (poetry.lock → `poetry install --no-root --no-interaction` pinned, requirements.txt → `pip install --no-deps -r`, else `pip install --no-deps -e .`; `PIP_DISABLE_PIP_VERSION_CHECK=1`, per-variant pip/pypoetry caches, `RECIPE_PY m6d-py-2`); `pypi.org` + `files.pythonhosted.org` allow-listed (F3 complete); e2e probes extended (pypi 200, files 404, testpypi refused). Tests first (did not compile before); 8 mutants killed; scratch repo fetches + indexes `scip-python ok unpinned` end-to-end with exact scip CALLS, refetch byte-identical. No Python golden moves. Repaired pre-existing phase-1 debt found in verification: `--all-targets` build break (`Images.go`) + red `scip_polyglot_monorepo` (billing Go goldens never re-blessed; `scip-status.txt` re-blessed, hunks classified W). Contract amendment 5.
- **M6d phase 5 (Go package keys; S5):** done 2026-10-11. Measured scip-go's `gomod` manager (deps versioned, stdlib pseudo-module, own `.`) against manifest `go/` keys; `package()` normalises `gomod`→`go`, rest passes through, mismatches key apart (D3). Export evidence review closes the S1/S2 follow-ups (Go root-relative, Python repository-shaped; no change). Tests first (failed on the manager); 5 mutants killed (one survivor indicted its test); e2e pin in `scip_polyglot_monorepo` (shared gin node + both provenances; no golden moved). Verify: fmt/clippy clean; 735 unit + 25 cli-bin; polyglot + 28 e2e green; DB 0 repos. Contract amendment 6.
- **M6d phase 6 (Lockfile versions; S6):** done 2026-10-11. `pyproject.toml` (PEP 508 + poetry-table readers, `python`/groups/extras out) with `poetry.lock`/`uv.lock` pins (registry sources only, PEP 503 lookup, workspace-aware governance with poetry-wins ties); manager `py`; npm-mirroring diagnostics incl. `lockfile_unread` for requirements/Pipfile. Tests first (failed on missing fns); 10 mutants killed (one survivor grew its test); scratch end-to-end verified (exact pins, `?` rows, spans, all three diagnostics); structure goldens green, no golden moves. Verify: fmt/clippy clean; 744 unit + 25 cli-bin; DB 0 repos. Contract amendment 7.
- **M6d phase 7 (requests/httpx; S7):** done 2026-10-11. Fetch-class `requests`/`httpx` sites as `ClientKind::Fetch` (module/dispatcher/session/inline, shared verbs, `base_url` fold, uniform carrier caps, per-lib kwarg allow-lists with `requests`-hooks-plain reasoning); Python kwargs read as single-entry objects + `**` keeps `...`; poetry/uv locks inert. Tests first (failed pre-impl); 10 mutants killed (one survivor grew its test); mixed scratch stitches 4 sites into 2 CONSUMES edges with correct buckets. No golden moves. Verify: fmt/clippy clean; 750 unit + 25 cli-bin; structure + code-question goldens green; DB 0 repos. Contract amendment 8.
- **M6d phase 8 (Python member idioms; S8):** done 2026-10-11. `self.`/`cls.` member receivers (construction-first incl. bare annotations, conflict-dynamic, enclosing-class walk, precomputed map) + session-typed locals (Depends shape); all capped. Tests first (2 failed); 9 mutants killed (one survivor grew its test); mixed scratch stitches member/typed sites with correct buckets. No golden moves. Verify: fmt/clippy clean; 753 unit + 25 cli-bin; structure + code-question goldens green; DB 0 repos. Contract amendment 9.
- **M6d phase 9a (chi + FastAPI route packs):** done 2026-10-11. chi (verbs, nested Route closures via containment+depth cap, Group, With incl. bound sub-routers, Method/Handle) + FastAPI (decorators, APIRouter prefixes, recursive include_router, api_route spreads); new Framework::Chi/FastAPI. Tests first (3 failed); 11 mutants killed (2 survivors grew tests). Out: chi Mount/named-registers, FastAPI mount/websockets. Verify: fmt/clippy clean; 757 unit + 25 cli-bin; DB 0 repos. Contract amendment 10.
- **M6d phase 9b (fixtures + guards):** done 2026-10-11. `go-chi-sqlx` (chi/SQL/implicit-Payer, pinned go.sum) + `python-fastapi-sqlalchemy` (two services, decorators/pins/imports/clients); join IMPLEMENTS branch for origin-attested interfaces (method+type edges; dispatch stays unmatched — RTA future); per-class biting guards; e2e fns + classified goldens. Tests first; 4 join mutants killed (1 redundant check removed); full 34 e2e + api green. DB 0 repos. Contract amendment 11.
- **M6d phase 10 (egress review; S10):** done 2026-10-11. Probe suite re-run green (allowed 200s, refusals named incl. sumdb/test-pypi, transport closure verified); per-tool fetch posture recorded (GOSUMDB off, lifecycle scripts off, build-code-runs-filtered equivalence). Verdict: no finding, no code change. DB 0 repos. Contract amendment 12.
- **M6d phase 11 (measurements; S11):** done 2026-10-11. --no-scip base-vs-head on mattermost + full-stack-fastapi-template (identical graphs; +14/+60 classified W-only rows incl. Go jobs, S7 sites, FastAPI endpoints, uv pins — all read in source); scip-go 6/9 ok (143 s server) + scip-python ok (9.5 s), deterministic; two fetch fixes from measurement (storage.googleapis.com + GOPROXY pin; --ignore-requires-python + RECIPE_PY m6d-py-3). Full record var/m6d/bench/phase11.md. Verify: fmt/clippy clean; 759 unit + 25 cli-bin; fixture e2e re-green under final tree. DB 0 repos. Contract amendment 13.
- **M6d phase 12 (growth, pip hardening, invariant 1, evaluator, close):** done 2026-10-11. S13 audit measured chi 15.85× and FastAPI 18× (counted ops, 100 vs 400 inputs) — both superlinear by construction; fixed test-first with 4 ratio tests + 1 cycle pin (chi per-file function sweep `FuncCache`/`ChiState`; FastAPI adjacency index + per-router memoised levels + cycle gate with legacy fallback; diamonds fan out by design). requests/httpx + member packs linear as built. Same-name shadowing cost recorded (shared machinery, not asserted). PIP_TIMEOUT=60 (`RECIPE_PY m6d-py-4`): measured 19/87 s ok vs 33/54 s pip read-timeout failures through the emulated stack. Contention incident recovered (2 p12 e2e failures from parallel builds starving the fetch; poisoned scip row diagnosed from `stderr_tail` and overwritten uncontended; both re-green). Invariant 1 (`--no-scip` vs `027a54f`): 4/7 identical, rest added-only W rows + 1 explained diagnostic shift; two-name determinism everywhere. Evaluator passes 1 (amendment 14) + 2 (verification-only, `tasks/eval-m6d-pass2.md`, VERDICT PASS). Verify (`var/m6d/logs/close-*`): fmt, clippy clean; 765 unit + 25 cli-bin; `rb-store` 66/66, `rb-server` 12/12; e2e 34/34, api 1/1 (150 sets), no `skipping:`; no listener; stub scan clean. The M3a wall-clock test flaked twice under load 7-10 and passes alone (M3b-cited). DB holds only 3 pre-existing evaluator repos; disk 9→11 GiB. Contract amendment 15. **M6d closed 2026-10-11.**

## M6e Build and lockfile breadth: complete (closed 2026-10-12, contract amendment 13)

- **Contract:** `tasks/contract-m6e.md` (2026-10-11), S1–S16 and invariants 1–7. Phases 0–12 in `tasks/todo.md`.
- **Soundness claims and closing bar, stated up front** (lesson 2026-10-06): package edges never pinned to the wrong version; Gradle builds never exfiltrate outside the allow-list and never use the wrapper; TS documents never wrongly rejected/kept; encodings never misread with a wrong precedence. Everything unproven takes the safe direction (unreadable pins nothing, unresolvable stays external, unlisted hosts fail closed, cross-classifier duplicates never merge, ambiguous encodings keep today's reading, unrecorded images never used).
- **Decided in the contract (D1–D13):** Gradle joins the scip-java image (pinned 8.14.6 + dist sha, `jar xf` + `chmod 0755`); wrapper never used/exported; egress starts at plugin portal + distribution host (additions only for measured traffic); hand-rolled lockfile YAML (no new dependency); Gradle version precedence mirrors Maven; go.work replaces win, excludes fail safe; classifiers ride the key; tsconfig scoping explains only; five-source encoding precedence; N3 rejects at input; fixture plan is code; measurement reuses public pins; M-numbered gaps keep their numbers.
- **Forks:** F1 single image, F2 Gradle pinned in preflight, F3 portal-first egress, F4 hand-rolled YAML, F5 bounded scoping/Berry, F6 preflight-chosen pins.
- **Progress:**
  - [x] Phase 0, preflight (amendment 1): baseline binary, full-SHA pins (jcommander Gradle-KTS, ofetch pnpm-lock), shape checks, scip-java Gradle interface, Gradle 8.14.6 pin + sha, image tooling, fixture dep versions, encoding precedence hypothesized, N3-clean fixtures.
  - [x] Phase 1, Gradle image (S1, amendment 2): `docker/scip-java` + Gradle 8.14.6, ID `cbd99634…`→`f9612a94…` (≈1.23 GB), in-image `gradle --version` verified offline.
  - [x] Phase 2, Gradle fetch + index (S2, amendment 3): `RECIPE_GRADLE m6e-gradle-1`, job-keyed `recipe()`, `gradle_runs()` (resolve + build behind proxy, offline `--build-tool Gradle` index), per-run `GRADLE_OPTS` proxy properties, `plugins.gradle.org` allow-listed; smoke proves exact edges + determinism; 6 mutants.
  - [x] Phase 3, Gradle versions (S3, amendment 4): catalog + both-DSL readers, ancestor governance, Maven-mirroring certainty/duplicates/scopes; 12 mutants; no dump change.
  - [x] Phase 4, Gradle fixture + guards (S4, amendment 5): `java-gradle-catalog` (bless-2 after fixing dotted-`version.ref` nesting the units missed — positive-rows lesson); 2 e2e guard-bite mutants.
  - [x] Phase 5, pnpm/yarn readers (S5, amendment 6): yarn v1/Berry + pnpm v9 hand-rolled readers, `LockKind` governance; 12 mutants; no dump change.
  - [x] Phase 6, pnpm workspaces + yarn links (S6, amendment 7): workspace globs into shared grouping, `file:`/`link:` via shared `resolve_link`, Berry unsupported; 6 mutants; no dump change.
  - [x] Phase 7, TS lockfile fixture (S7, amendment 8): `ts-pnpm-yarn` (generated lockfiles, per-format guards); 2 e2e guard-bite mutants.
  - [x] Phase 8, go.work + exclude (S8, amendment 9): `go.work` reader, nearest-governance, workspace replaces win (joined at workspace root), `use` members join by module path, excluded→`@?` + `go_excluded`; 2 mutants; no dump change.
  - [x] Phase 9, classifiers (S9, amendment 10): `<classifier>` rides the key, duplicate rule per full key, `<type>` recorded; 2 mutants; no dump change.
  - [x] Phase 10, tsconfig + encodings + N3 (S10–S12, amendment 11): `TsScopes` (allow-list globs, subtree rule, bounded extends, unread counted, fail-open) partitioning `files_without_document` with no edge moves; per-document five-source `effective_encoding` (D9 corrected by measurement: pom beats compilerArgs both ways; mixed-build corner → per-doc); `Restructured` rejection after 0/618 calibration; growth ratio; all mutants; no dump change.
  - [x] Phase 11, measurements (S13, amendment 12): jcommander `--no-scip` identical + deterministic, Gradle SCIP ok 45.3 s (1,218/1,218 defs, 1,355 exact CALLS, 20/20 samples, cached identical) after allow-listing `plugins-artifacts.gradle.org` (probed 200, key moved by design); ofetch 12 pnpm pins classified + sampled + deterministic; class-validator S10 live-fire 42 = 38 + 4; cleanup done.
  - [x] Phase 12, evaluator + close (amendment 13): pass 1 full review (12 probes, 5 new tests, VERDICT PASS, no blockers), pass 2 verification-only (VERDICT PASS, close verify green).
- **Verification** (`var/m6e/logs/p12-close-*`): fmt, clippy `-D warnings`, 822 units + 25 cli-bin, store 66, server 12, e2e 36/36, api 1/1 (150 sets), no `skipping:`, no listener, stub scan clean. `PIPELINE` unbumped (`m6c-5`). DB holds only the 3 pre-existing `fx_*` repos.

## M6f Contract packs: complete (closed 2026-10-12, contract amendment 11)

- **Contract:** `tasks/contract-m6f.md` (2026-10-12), S1–S11 and invariants 1–6. Phases 0–9 in `tasks/todo.md`.
- **Soundness claims and closing bar, stated up front** (lesson 2026-10-06): no misread paths/methods/versions; Gradle-style exfiltration discipline reused (no new fetches); no wrongly rejected/kept TS... (M6f: specs/protos) documents; no encoding-style misreads. Everything unproven takes the safe direction (unreadable pins nothing, unresolvable stays external/unhandled, duplicates never merge silently, `info.version` never a package version, `CONSUMES` never `exact` — deviating from design §10.2 with reasons in D5).
- **Decided in the contract (D1–D7):** hand-rolled YAML (no new dependency); proto prefers the grammar; specs authoritative for existence only; MyBatis rides rb-sql; generated clients cap at synthetic; fixture plan is code; measurement reuses pins.
- **Forks:** F1 hand-rolled YAML, F2 grammar-first proto, F3 existence-only authority, F4 MyBatis-on-rb-sql, F5 synthetic cap.
- **Progress:**
  - [x] Phase 0, preflight (amendment 1): baseline binary, grpc-go + OAI pins, tree-sitter-proto binds (stream needs text check), shape checks, baseline green.
  - [x] Phase 1, OpenAPI reading (S1, amendment 2): JSON + YAML shapes, basenames, growth; mutants.
  - [x] Phase 2, proto reading (S2, amendment 3): services/RPCs/streams/spans; mutants; growth.
  - [x] Phase 3, spec endpoints in graph (S3, amendment 4): shared-key registrations, GRPC keys, diagnostics; mutants.
  - [x] Phase 4, MyBatis mapper XML (S5, amendment 5): Element attrs/ends, statements→tables, dynamics counted; mutants.
  - [x] Phase 5, generated-client packs (S6 packs, amendment 6): operationId/RPC candidates + Endpoint.operations; mutants; growth.
  - [x] Phase 6, generated-call stitching (S6 stitch, amendment 7): operation/RPC matching, own-service rule, synthetic cap; mutants.
  - [x] S4b, interface handler roll-down (S4, amendment 8): interface_routes recorded, spec-matched roll-down via IMPLEMENTS; mutants.
  - [x] Phase 7, fixture + guards (S7, amendment 9): contract-packs fixture (Maven + nested webapp), e2e + 5 goldens + 12/3 guards, every line read, 4 guard-bite mutants.
  - [x] Phase 8, measurements (S8, amendment 10): grpc-go classified (18 endpoints, 52 synthetic CONSUMES sampled, 869 correctly-unmatched residue) + deterministic; OAI identical + reader breadth on real specs; cleanup.
  - [x] Phase 9, evaluator + close (amendment 11): pass 1 full review (12 probes, 1 fix + churn fix, VERDICT PASS), pass 2 verification-only (VERDICT PASS, close verify green).
- **Verification** (`var/m6f/logs/p9-close3-*`): fmt, clippy `-D warnings`, 842 units + 25 cli-bin, store 66, server 12, e2e 37/37, api 1/1 (150 sets), no `skipping:`, no listener, stub scan clean. `PIPELINE` `m6c-5` → `m6f-1`. DB holds only the 3 pre-existing `fx_*` repos.

## M6g Topology and configuration packs: complete (closed 2026-10-12, contract amendment 11)

- **Contract:** `tasks/contract-m6g.md` (2026-10-12), S1–S10 and invariants 1–7, D1–D7, forks F1–F5.
- **Soundness claims and closing bar, stated up front:** silent-failure outputs (wrong topic, misroute, merged hosts, wrong profile, YAML misread) all take the safe direction (unreadable contributes nothing counted; dynamics never edge; ambiguous rewrites never applied; unknown profiles keep today's reading; duplicates never merge). Pass 1 full, pass 2 verification-only.
- **Decided in the contract (D1–D7):** topic edges originate at services; literals only; gateway rules rewrite (no new node kind); hosts extend D6; YAML hand-rolled with documented precedence; fixture plan is code; measurement pins.
- **Progress:**
  - [x] Phase 0, preflight (amendment 1): baseline binary `var/m6g/base/rb-f80714d`, kafka + gateway pins, shape checks.
  - [x] Phase 1, topic packs (S1, amendment 2): Kafka/Rabbit listeners + template sends; dynamics counted; mutants.
  - [x] Phase 2, topic graph (S2, amendment 3): `Topic` + `PUBLISHES`/`SUBSCRIBES`, placeholder resolution; mutants.
  - [x] Phase 3, gateway packs (S3, amendment 4): nginx + Spring Gateway readers + stitch rewrites (`via_gateway`); mutants.
  - [x] Phase 4, host names (S4, amendment 5): compose/k8s/`rb.toml` readers + stitch aliases + `RbToml.hosts`; mutants.
  - [x] Phase 5, Spring YAML (S5, amendment 6): multi-doc profiles evaluated in documented precedence; diagnostics stay conservative; mutants.
  - [x] Phase 6, fixture + guards (S6, amendment 7): topo-packs fixture, e2e + dump golden + 11/4 guards each biting (8 mutant runs); `PIPELINE` `m6g-1`.
  - [x] S9, growth (amendment 8): counted ops for the config walk (4x→5x); readers ride existing growth tests.
  - [x] Close verify + invariant 1 (amendment 8): 10 old fixtures `--no-scip` identical + polyglot/ts-express SCIP identical.
  - [x] Phase 7, measurements (amendment 9): kafka 625k identical (vacuous, proven); gateway +8 correct unreads; spring-kafka +808 (0 false in samples).
  - [x] Evaluator pass 1 (amendment 10, `tasks/eval-m6g-pass1.md`): VERDICT PASS — 6 fixes (P9/P18/P30/P31/P32/P2), 10/10 mutants killed.
  - [x] Evaluator pass 2 (`tasks/eval-m6g-pass2.md`): verification-only — VERDICT PASS (repros re-run, mutants cited, suite green, invariant 1 re-derived).
- **Verification** (`var/m6g/logs/p6-close2-*`): fmt, clippy `-D warnings` (clean on the final tree; the loop's clippy step ran pre-fix), 866 units, `rb-store` 66/66, `rb-server` 12/12, e2e 38/38, api 1/1, no `skipping:`, no listener, stub scan clean.

## M6h Architecture rules as a CI gate: complete (closed 2026-10-12, contract amendment 7)

- **Contract:** `tasks/contract-m6h.md` (2026-10-12), S1–S8 and invariants 1–7, D1–D4, forks F1–F4.
- **Soundness claims and closing bar, stated up front:** unreadable rules impossible (registry is code); twins prove patterns; read-only by construction; timeouts + caps on every query.
- **Decided in the contract (D1–D4):** AGE projection is the engine; twins prove patterns; built-ins only; violations sorted.
- **Progress:**
  - [x] Phase 0, preflight (amendment 1): baseline binary, `rb check` free, guard rails, AGE labels, fixture inventory.
  - [x] Phase 1, rule module (amendment 2): orphans, unowned writes, service cycles + Cypher twins; 3/3 mutants.
  - [x] Phase 2, CLI gate (amendment 3): read-only path, sorted/JSON, exit 0/1/2; live-fired; AGE rebinding fix.
  - [x] Phase 3, goldens + parity (amendment 4): twin parity on 4 fixtures; check.txt goldens; topo orphan endpoint.
  - [x] Phase 4, public legs (amendment 5): piggy/spring-kafka/gateway sampled, 0 false; residue-miss root cause.
  - [x] Evaluator pass 1 (amendment 6, `tasks/eval-m6h-pass1.md`): VERDICT PASS — 4 fixes, 5/5 mutants.
  - [x] Evaluator pass 2 (amendment 7, `tasks/eval-m6h-pass2.md`): VERDICT PASS — repros re-run, 3/3 re-killed, suite green.
- **Verification** (`var/m6h/logs/p6-final-*`): fmt, clippy `-D warnings`, 874 units, `rb-store` 72/72, `rb-server` 12/12, e2e 39/39, api 1/1, no `skipping:`, no listener, stub scan clean.

## M6i Kotlin and C# syntax: complete (closed 2026-10-12, contract amendment 8)

- **Contract:** `tasks/contract-m6i.md` (2026-10-12), S1–S8 and invariants 1–7, D1–D4, forks F1–F4.
- **Soundness claims and closing bar, stated up front:** wrong-parent definitions, wrong-scope references, wrong exact edges, grammar panics; safe direction everywhere (partial + `syntax_error`, references stay references, old facts frozen, SCIP owns Kotlin only under the unchanged checks).
- **Decided in the contract (D1–D4):** stock queries first; Tier B reuse; unchanged agreement checks; fixture plan is code.
- **Progress:**
  - [x] Phase 0, preflight (amendment 1): baseline binary, grammar-crate pins (`kotlin-ng 1.1.0`, `c-sharp 0.23.5`), scip-java Kotlin support, coroutines + automapper pins.
  - [x] Phase 1, grammars (amendment 2): Kotlin hand-written tags (first-pattern-wins) + C# stock tags with invocation supplement; `EXTRACTOR_VERSION` + pin test; old languages byte-identical.
  - [x] Phase 2, Tier B (amendment 3): imports/bindings/calls/modules/qualifiers/supertypes per language; fixtures + 11/4 biting guards; 12/12 mutants.
  - [x] Phase 3, Kotlin exact (amendment 4): fixture builds at Kotlin 2.2.0 (scip-java's own version); 0 new calibrations; `scip_kotlin_gradle` e2e.
  - [x] Phase 5, public measurement (amendment 5): coroutines +30,496 / automapper +30,239 rows, every row classified; 7 Java→Kotlin interop edges source-verified; 0 false in samples.
  - [x] Phase-5 follow-up (amendment 6): Kotlin wildcard miss → 1-line arm fix + unit; mutant bites.
  - [x] `PIPELINE m6i-1` (amendment 7): metadata-only, no golden pins it.
  - [x] Golden re-bless (amendment 8): `java-gradle-catalog` +1 classified row + status recompute; `.kts`/`.cs` census proves it the only stale golden.
  - [x] Evaluator pass 1 (`tasks/eval-m6i-pass1.md`): VERDICT PASS — H1 in-tree, 8 probes, mutant killed.
  - [x] Evaluator pass 2 (`tasks/eval-m6i-pass2.md`): VERDICT PASS — repros re-run, suite stands.
- **Verification** (`var/m6i/logs/p6-close2-*`): fmt, clippy `-D warnings`, 888 units, `rb-store` 72/72, `rb-server` 12/12, e2e 42/42 (after the reviewed re-bless), api 1/1 (150 sets identical), no `skipping:`, no listener, stub scan clean.

## M6j DI breadth: complete (closed 2026-10-12, contract amendment 11)

- **Contract:** `tasks/contract-m6j.md` (2026-10-12), S1–S11 and invariants 1–7, D1–D4, forks F1–F5 (F5 decided in phase 4: no new nodes).
- **Soundness claims and closing bar, stated up front:** wrong beans, wrong injection targets, XML misreads, taint loss, decorator misattachment; safe direction everywhere (unread stays taint, unresolvable stays unresolved, old packs frozen, XML parse failure taints, .NET heuristic max).
- **Decided in the contract (D1–D4, F1–F5):** packs reuse M4b; stock node shapes first; XML reuses `parse_xml`; fixture plan is code (4 new + 2 extensions, Dagger split amended in).
- **Progress:**
  - [x] Phase 0, preflight (amendment 1): baseline binary, six pins (guice/dagger/nest/jpetstore/spring-fu/automapper-reuse), grammar shapes, green baseline.
  - [x] Phase 1, decorations (amendment 2): Kotlin annotations + C# attributes with literal args, string literals with interpolation holes, `class_parameter` bindings, binding decorations for Kotlin/C#/TS; 12/12 mutants.
  - [x] Phase 2, Guice/Dagger (amendment 3): import-gated packs (`Singleton`/`Injectable`/`Binds`/`Provides`/bind-chains with qualifier joins), `Arg::Type` class literals, engine `Binds` arm; fixtures + 8/5 guards, every guard biting; 12/12 + T1/T2 mutants.
  - [x] Phase 3, NestJS (amendment 4): decorator packs + `resolve_nest` lane (qualifier-only tokens); `ts-nest` fixture + 3/3 guards; 6/6 mutants + lane bites.
  - [x] Phase 4, Spring XML (amendment 5): `xml_beans.rs` reader + synthesis (beans target resolved classes, refs become points, scans union roots, findings retire); `Inputs.xml`; `spring-xml` fixture + 4/3 guards; 7/7 + X1–X5 bites.
  - [x] Phase 5, Kotlin beans (amendment 6): `CallLink.type_args` + `value_argument` decoding; structural `beans`-span packs reusing `Binds`; shared lane; plain `kotlin-beans` fixture + 4/3 guards; 8/8 + K1/K2 bites.
  - [x] Phase 6, .NET (amendment 7): generic call args + file-scoped modules; `Add/TryAdd` packs with owner-registration rule; lane shared; `csharp/` extended + guards; 5/5 + D6/D7/V1/V2 bites.
  - [x] Phase 8, public measurement (amendment 8): guice +40, nest +168, jpetstore gate-lift, springfu QUERIES, automapper +693 — every row classified, samples read in source, 0 false.
  - [x] S10 growth (amendment 9): shared `bean_start`, indexed class resolution, ticks; two lane ratio tests green.
  - [x] Evaluator pass 1 (amendment 10, `tasks/eval-m6j-pass1.md`): VERDICT PASS — H1–H4 in-tree, 9 probes, 52/52 mutants.
  - [x] Evaluator pass 2 (`tasks/eval-m6j-pass2.md`): VERDICT PASS — repros re-run, suite stands.
- **Verification** (`var/m6j/logs/p6-close-*`): fmt, clippy `-D warnings`, 913 units, `rb-store` 72/72, `rb-server` 12/12, e2e 47/47, api 1/1 (150 sets identical), no `skipping:`, no listener, stub scan clean.

## M6k DI recall: complete (closed 2026-10-12, contract amendment 10)

- **Contract:** `tasks/contract-m6k.md` (2026-10-12), S1–S11 and invariants 1–7, D1–D5, forks F1–F5.
- **Soundness claims and closing bar, stated up front:** misanchored beans, assisted leakage, unmarked-property points, key confusion; safe direction everywhere (unresolvable anchors skip, assisted excluded import-gated, properties opt-in, literal-only keys, heuristic max).
- **Progress:**
  - [x] Phase 0, preflight (amendment 1): baseline binary `eb2242be`, M6j pins reused, green baseline.
  - [x] Phase 1, anchor at type (amendment 2): `FileDi::unanchored` + shared `resolve_lane_point` + unique-or-skip (XML-bean precedent); val-form + minimal-hosting engine proofs.
  - [x] Phase 2, packs (amendments 3–4): `@Assisted` exclusion, `toProvider`, C# properties, keyed services, C# `argument` unwrap, global-namespace key; pack + engine units.
  - [x] Phase 3, fixtures (amendment 4): extensions with reviewed purely-additive goldens; every new guard bites with byte-identical restores.
  - [x] Phase 4, units/mutants/PIPELINE (amendment 5): anchor-skip unit, 7/7 manual kills (two vacuous-test repairs), `PIPELINE m6k-1`, lint clean.
  - [x] Phase 5, invariant + measurement (amendments 6, 8–9): 15/16 identical + 2 SCIP identical; guice/springfu/automapper classified, 0 false, 0 lost, 0 new exact Tier-B.
  - [x] Pass-1 probing (amendments 7–8, `tasks/eval-m6k-pass1.md`): VERDICT PASS — H1–H6 in-tree (dotted namespaces, cross-namespace merge, enclosing scope, diagnostic siting, static properties, vacuous tests), 20+ source-read samples.
  - [x] Evaluator pass 2 (`tasks/eval-m6k-pass2.md`): VERDICT PASS — repros re-run, suite stands.
- **Verification** (`var/m6k/logs/p6-close-*`): fmt, clippy `-D warnings`, 926 units, `rb-store` 72/72, `rb-server` 12/12, e2e 47/47, api 1/1, no `skipping:`, no listener, stub scan clean.

## M6l XML diagnostic siting: complete (closed 2026-10-13, contract amendment 3)

- **Contract:** `tasks/contract-m6l.md` (2026-10-12), S1–S4, D1–D2, fork F1.
- **Progress:**
  - [x] Phase 0, baselines (amendment 1): baseline binary `41e7d632` (M6k final-tree build, no code delta at copy).
  - [x] Phase 1, siting fix (amendment 2): `owner_path` → `xml_path` in the one call + unit (mutant kills) + spring-xml `missing` ref (no new nodes/edges, guards hold) + reviewed re-bless (coverage +1/+1, newly-sited diagnostic only) + clippy fix in the new test.
  - [x] Invariant 1: 17/18 identical + 2 SCIP identical; the single spring-xml row moves the diagnostic to the document.
  - [x] Evaluator pass 1 (short review): the fix surface, one mutant kill, no other rows shift.
  - [x] Evaluator pass 2 (`tasks/eval-m6l-pass2.md`): VERDICT PASS — repros re-run, suite stands.
  - [x] `PIPELINE m6l-1`: metadata-only, no golden pins it.
- **Verification** (`var/m6l/logs/p6-pipe-*`): fmt, clippy `-D warnings`, 927 units, `rb-store` 72/72, `rb-server` 12/12, e2e 47/47, api 1/1, no `skipping:`, no listener, stub scan clean.

## M7a `exact`-label correctness (stress batch A): implemented 2026-10-13, uncommitted

- **Contract:** `tasks/contract-m7a.md` (S1–S3, amendment 1). Source: `tasks/stress-findings.md` F17–F19.
- **S1:** Go/TS gRPC clients are typed only by a trailing `New*Client` constructor. `pb.NewXClient(c).Rpc(…)` (exactly two links) is the RPC.
- **S2:** FastAPI goes through the shared router composer (`RouterDecl`/`Mount`). `module.router` resolves through Python submodules, and an unknown prefix stays unknown.
- **S3:** gRPC endpoints come from server registrations in 5 languages, counted with `grpc_implementer_unknown` / `grpc_registration_ambiguous`.
- **Verification:** 14 mutants killed. DB suites 980/2, with the 2 failures pre-existing (F1). Invariant: 29/31 targets byte-identical (all 18 fixtures), and microservices-demo plus fastapi-realworld diffs classified. `PIPELINE` `m7a-1`.

## M7b Regressions and hangs (stress batch B): implemented 2026-10-13

- **Contract:** `tasks/contract-m7b.md` (S1–S5, amendment 1). Source: `tasks/stress-findings.md` F1, F2, F9–F11, F15.
- **S1:** `rb status` lists every repository outside an indexed checkout. The two red DB suites are green again.
- **S2:** every Docker CLI call is bounded, killed and reaped. A wedged daemon now costs about 10 s, not a hang.
- **S3:** a `Parents` map replaces `Node::parent()` in the profiled rb-syntax hot spots. Cubic and quadratic pathological inputs now take seconds or less.
- **S4:** the mount composer is pruned once its result can't change. F11 went from 44 s to 1 s with identical output.
- **S5:** huge blobs are sniffed by streaming. A 1 GiB blob now peaks at 16 MiB, down from 2 GiB.
- **Verification:**
  - 11 mutants killed (m15–m25, 2 reverted conversions recorded).
  - DB suites 986/0, with no skips.
  - Invariant against M7a: 31/31 byte-identical.

## M7c HTTP recall on supported stacks (stress batch C, part 1): implemented 2026-10-13

- **Contract:** `tasks/contract-m7c.md` (S1–S4, amendment 1). Fixes F20, F21, F12, F27.
- **Changes:** Express paths written as names fold across files through a shared constant lookup (`rb-indexer/src/constants.rs`), with object-member and default-import folding. Default-imported axios instances resolve. Inline `require('express')()` is recognised. TS gRPC stubs need a generated-code import.
- **Verification:**
  - 14 mutants killed. Real-Docker suites 990/0/0.
  - Invariant 29/31 identical. bookcars: +107 endpoints and +175 stitched calls. mattermost: −29 false gRPC sites.
  - `PIPELINE` `m7c-1`.

## M7d DI and SQL roles (stress batch C, part 2): implemented 2026-10-13

- **Contract:** `tasks/contract-m7d.md` (amendment 1). Fixes F28 (except open generics), F26, F23.
- **Changes:** .NET implicit and global usings import DI; implementation-registered owners; constructor parameters only. Framework-provided lane points are `external`. `.sql` files are migrations only by evidence.
- **Verification:**
  - 18 mutants killed (m40–m57). Real-Docker suites 995/0/0.
  - Invariant 26/31 identical, diffs classified.
  - `PIPELINE` `m7d-1`.

## M7e Input hygiene (stress batch D): implemented 2026-10-13

- **Contract and design:** `tasks/contract-m7e.md` (diagram, estimates, trade-offs, algorithm). Fixes F16, F22, F13, F29.
- **Mechanism:** `exclude::decide` runs once from the full tree; analysis stages see only non-excluded files; `File` nodes are kept and marked; counts are reported per reason, including in `rb status`.
- **Verification:**
  - 15 mutants killed. Real-Docker 1,004/0/0. Invariant 28/31 identical, classified.
  - go-gin-example: 4× faster, 4× less memory, 8× smaller graph.

## M7f The error contract and targets (stress batch E, part 1): implemented 2026-10-13

- **Contract:** `tasks/contract-m7f.md` (error contract table and diagram, S1–S7, amendment 1). Fixes F4–F8, F14.
- **Verification:**
  - 14 mutants killed. Real-Docker 1,009/0/0.
  - Invariant 31/31 byte-identical. The exit-code table gains `unavailable` (6 / 503).

## M7g Honesty about what was not analysed (stress batch E, part 2): implemented 2026-10-13

- **Contract:** `tasks/contract-m7g.md` (design diagram, S1–S3, amendment 1). Addresses F30, F25 and F24, each partly (residue recorded).
- **Implemented:** `language_unsupported` and `framework_unsupported` diagnostics with `rb status` lines; `orphan_endpoints` requires a non-ambiguous consumer; `rb check --baseline` and the unlinked-client note.
- **Verification:**
  - Mutants m87–m98 killed (m93 exposed dead code, now deleted). Real-Docker 1,013/0/0.
  - Invariant 27/31 byte-identical, 4 classified. `PIPELINE` `m7g-1`.

## M7h The CLI remainder (stress batch F, part 1): implemented 2026-10-13

- **Contract:** `tasks/contract-m7h.md` (decision diagram, S1–S5, amendment 1). Fixes F3, U1, U4, U7, U9 and the M7f residue.
- **Implemented:**
  - Checkouts recorded per repository, used for `--repo` inference and by `rb index .`.
  - `find --kind` alone, the `find` no-match note, the card page wording.
  - Cypher read-only and `--cols` errors; the environment-named timeout.
  - `rb doctor`.
- **Verification:** 24 mutants killed. Real-Docker 1,028/0/0. Invariant 31/31 identical.

## M7i Precision and cost remainder (stress batch F, part 2): implemented 2026-10-13

- **Contract:** `tasks/contract-m7i.md` (diagram, S1–S6, amendment 1). Fixes F31, F32, U5, O3, O4; O7 declined.
- **Verification:**
  - 21 mutants killed. Real-Docker 1,041/0/0.
  - Invariant 29/31 identical, 2 classified.
  - mattermost cached RSS 2,148 → 1,658 MiB.

## M7j The M7 close fixes (evaluator pass 1): implemented 2026-10-13

- **Contract:** `tasks/contract-m7j.md` (E1–E8, amendment 1). Input: `tasks/eval-m7-pass1.md` (17 findings, 0 S1).
- **Verification:**
  - 7 mutants killed. Real-Docker 1,047/0/0, with the log kept.
  - Invariant 31/31 identical. E5 10.8 s (M7i 45.4 s).

## M7 closed (evaluator passes 1–2): 2026-10-13

- **The series:** M7a–M7j fixed the stress campaign's findings (`tasks/stress-findings.md`).
- **Pass 1:** 17 findings, answered by M7j.
- **Pass 2:** 1 blocking records error (U2), corrected at the close, plus 4 S4 items (fixed or recorded as residue).
- **Next:** M8a (accuracy benchmark), whose contract is drafted separately.

## M8a An accuracy benchmark with answer keys, part 1: implemented 2026-10-13

- **Discussed and decided:** the benchmark contract was written in plan mode, its draft reviewed adversarially (4 blockers), and the work split into M8a (harness, gate, SCIP agreement, PyCG) and M8b (microSecEnD, blind hand keys, `exact` sample). Contract: `tasks/contract-m8a.md`, S1–S11 with amendments 1–3.
- **Built:**
  - `crates/rb-bench` (dump reader, location identity, scoring, scoreboard, gate, render, SCIP and PyCG adapters, `score|render|compare` CLI).
  - `bench/` (manifest, fetch, run, cleanup, mutant battery).
  - The first scoreboard `bench/scoreboard/m8a.json`, and the findings `tasks/bench-findings.md` (B1–B6).
- **Verification:**
  - Evaluator passes 1 and 2, both FAIL with 2 blocking items each, all answered (amendments 1 and 4).
  - Mutants m151–m200 (49 live, all killed).
  - Scoreboard determinism across runs and scorings.
  - Real-Docker 1,094/0/0.
- **Status:** complete. The M8b contract is written (`tasks/contract-m8b.md`).

## M8b An accuracy benchmark with answer keys, part 2: implemented 2026-10-14

- **Contract:** `tasks/contract-m8b.md`, T1–T7 with amendment 1.
- **Keys added:**
  - **microSecEnD:** 12 apps pinned, scoring endpoints and service flows.
  - **Blind hand keys:** spring_boot_microservices, hk_polls, hk_todo. Labelled and adjudicated without sight of rb's output, with audited transcripts.
  - **The `exact` sample:** 249 judged edges.
- **Built:** `structural.rs` (identity for endpoints, handlers, tables and services), `msecend.rs`, `handkey.rs`, `sample.rs`; the `statements` and `sample` commands; `bench/pin_microsecend.py` and `bench/adjudicate.py`.
- **Results:**
  - `bench/scoreboard/m8b.json` (amendment 2 re-score `d7346ffb…`: microsecend endpoints P 88.7 % R 75.8 % after 23 removals; `exact` 122/130);
  - findings B7–B14 in `tasks/bench-findings.md` (B14: config-server context paths).
- **Verification:**
  - Mutants m201–m229 (29 specs, full battery re-run, all killed).
  - Determinism (`cmp`-identical twice, accuracy and perf).
  - Real-Docker 1,122/0/0.
  - Evaluator pass 1 (`tasks/eval-m8b-pass1.md`) FAIL answered by amendment 2 (6 override files, 3 second adjudications, corrected Result/B11/B12/CHANGELOG); close after a verification-only re-check.
- **Status:** closed 2026-10-16 on evaluator pass 2 (`tasks/eval-m8b-pass2.md`) VERDICT PASS (1 prose should-fix, 4 notes, all answered). M8 (benchmark) is done; the rb fixes it found (B1, B4, B7–B10, B12–B14) are input for the next rb milestone.

## M9a `exact` precision from the M8 benchmark: implemented 2026-10-16

- **Discussed and decided:** the plan (`tasks/plan-m9.md`) was checked against the code and the M8 dumps before the contract (`tasks/contract-m9a.md`, C1–C12, amendments 1–2). Seven plan corrections, among them: B8 comes from generated OpenAPI files, not `@Router` comments; re-sourcing B13's method edges would duplicate 1,695 type edges and add 2 false ones, so they are dropped and counted; B14 reads only what a `native` config server provably serves.
- **Built:**
  - B7: the endpoint stage reads no test file (`routes_in_test_code` diagnostic).
  - B8: spec-only endpoints `synthetic`.
  - B12a: literal `"{…}"` Spring paths.
  - B13: no `IMPLEMENTS` from a Go method under the bypass.
  - B14: context paths from a `native` config server's shared files (`config_server_context_path`, `config_server_ambiguous`).
  - B11: documented only.
  - The mutant runner names its crate and counts build errors as INVALID. `PIPELINE` `m9a-1`.
- **Files:** `crates/rb-frameworks/src/routes.rs`, `crates/rb-indexer/src/{lib,routes,scip}.rs`, `crates/rb-cli/tests/e2e.rs`, the contract-packs and go-chi-sqlx goldens, `bench/mutants/{run.py,m9a.json}`, `bench/samples/exact.tsv`, `bench/scoreboard/m9a{,-perf}.json`, `bench/accept-m9a.tsv`, `README.md`, `docs/{usage-and-testing,repository-brain-design}.md`.
- **Results:**
  - `exact` 100.0 % of 126 (M8b 93.8 % of 130); sample `routes` and `scip` 60/60 after 80 new verdicts, all TRUE.
  - microSecEnD endpoints P 100.0 %, R 91.9 %.
  - 8 of 314 cells changed, one accepted (a correct out-of-scope Feign flow). Invariant: 0 unclassified rows over 157 dumps.
  - B13b recorded: 277 `IMPLEMENTS` from 5 `*Suite#` types on mattermost, deferred to M9b.
- **Verification:**
  - Mutants m230–m247 and m8b all KILLED.
  - Real-Docker 1,129/0/0.
  - fmt and clippy clean; `rb.repo` unchanged.
  - Two scorings `cmp`-identical.
  - Perf inside the NFR after a re-measure (amendment 2).
- **Evaluator pass 1 (PASS, 4 should-fix, 4 notes), answered in amendment 3 (`624da45`):**
  - E3: an empty remote context path now clears a lower one (`PIPELINE` `m9a-2`).
  - E1: the B14 and B7 tests cover every rule, with mutants m248–m260.
  - E2: a mutant kill needs a failing test that matches the spec's filter.
  - E4: every citation of the 80 new verdicts is script-checked (258 of 259 OK); the disk figures are logged.
  - E5 and E6 are residue.
  - Re-close: binary `2e36cd60…`, dumps identical by row to the first run, scoreboard cells unchanged, `compare` exit 0, perf warnings re-measured without persisting.
- **Status:** closed 2026-10-16 on evaluator pass 2 (`tasks/eval-m9a-pass2.md`) VERDICT PASS (5 notes; N1 and N2 carried to M9b P5).

## M9b Recall from the M8 benchmark: implemented 2026-10-16

- **Discussed and decided:** the contract (`tasks/contract-m9b.md`, C1–C12, amendments 0–6) checked the plan against the code first, with ten plan corrections. Among them: B10 can reach 21 of 26, not 26, because the `Update` functions build `SET %s` at run time; B12b's resources include ones the model does not list, which need an adjudication. P0 read the raw scip-go indexes before any B1 code and found no member occurrence of server/public in server's index, so B1 moved to M9c (D4; C5 and C6 withdrawn, P4 not built).
- **Built:**
  - B9: Go methods declared outside their receiver type's file are its members, and never bare package-level names (`resolve.rs`).
  - B10: `fmt.Sprintf` SQL folds package `const` strings (`sql_folded`).
  - B12b: Spring Data REST repositories expose `synthetic` endpoints (`data_rest_base_unread`).
  - N1: the remote context-path trim is pinned by a test. N2: the mutant runner refuses filters that select wall-clock tests.
  - `PIPELINE` `m9b-1`. The P0 probe `bench/probe/scip_cross.py` records B1 and B13b from the raw index.
- **Files:** `crates/rb-frameworks/src/routes.rs`, `crates/rb-indexer/src/{lib,resolve,routes,sql}.rs`, `bench/probe/scip_cross.py`, `bench/mutants/{run.py,timing_tests.py,timing_tests.txt,m9a.json,m9b.json}`, `bench/keys/microsecend/mse_jferrater.md`, `bench/scoreboard/m9b{,-perf}.json`, `bench/accept-m9b.tsv`, `README.md`, `docs/{usage-and-testing,repository-brain-design}.md`.
- **Results:**
  - hk_todo `handler_binding` 0 → 12 of 13, `table_queries` and `table_ops` 0 → 21 of 26 each, all `heuristic`.
  - mse_jferrater endpoints 0 → 17 of 18 after a blind adjudication added 12 entries (17 of 17 against the key, which lacks the served `/customers/search/findById` that rb also misses; corrected after pass 2, N3); microSecEnD endpoints R 77.0 % → 100.0 % on the re-scored base (74 of 75, 98.7 %, with that endpoint).
  - scip calls agreement P 74.3 % → 78.4 %, R 49.7 % → 62.4 %.
  - 10 of 314 cells changed; `compare` exits 0 with 2 accepted cells. Invariant: 0 unclassified rows over 157 dumps, 0 new `exact`.
  - `exact` 100.0 % of 126, unchanged; `synthetic` 83.0 % of 47.
- **Verification:**
  - Mutants m9b 28/28, m9a 31/31, m8b 29/29 KILLED.
  - Real-Docker 1,135/0/0.
  - fmt and clippy clean; `rb.repo` unchanged.
  - Two scorings `cmp`-identical.
  - One perf warning (mattermost `scip` RSS 1.18×), re-measured A/B as noise.
- **Evaluator pass 1 (PASS, 6 should-fix, 4 notes), answered in amendment 7 (`b651523`):**
  - E3: a Go constant of a named type no longer folds into `Sprintf` SQL (`PIPELINE` `m9b-2`).
  - E2: a configurer registered by a `@Bean` method, by `withConfig` or by a Kotlin class caps data-rest endpoints.
  - E1: rows and mutants for each evaluator survivor and a `data_rest_services` test; mutants m279–m284 and m295–m299.
  - E4: the runner refuses a filter that any timing test's full path contains.
  - E5: the ADD split is 5 item, 2 collection and 5 search; `/customers/search/findById` is a known gold omission, so jferrater is 17 of 18.
  - E6: the full `compare` command is quoted; `compare-r2.txt` has exit 0; `GetClusterId` is an M9c open item.
  - The `@RestResource(exported = false)` CRUD rule and E8–E9 are residue.
  - Re-close: binary `36ed5ea4…`, dumps identical by row to the first run, scoreboard cells unchanged, `compare` exit 0, 24 load-driven perf warnings re-measured A/B as noise.
- **Evaluator pass 2 (PASS, 0 blocking, 3 should-fix, 4 notes), answered in amendment 8 (`15baa12`):**
  - S1: six of the 24 warnings had a median CPU ratio over amendment 0's 1.1× on 3 runs, and the summary script had an exemption the rule lacks. Re-measured with 20 runs per side in ABBA order: median CPU 0.932–1.097×, wall 0.944–1.029×, instructions retired 0.978–1.003×. All six are noise under the rule as written.
  - S2: the Result's perf table shows m9a, r1 and r2 with their loads.
  - S3: the three load-sensitive tests passed on a quieter host (try 3, `var/m9b/logs/r3/s3-try3.log`), and the full suite at `15baa12` is 1,136 passed, 0 failed, 0 skipped (`test-r3b.log`). The e2e statement timeout is `rb-store`'s `unowned_table_writes` query under a `LIMIT` (about 1.8 s on this host). The M9a binary hits it identically. Open item in `tasks/todo.md`.
  - N1: a fully qualified configurer (return type and supertype) caps; the evaluator's x1 is KILLED. N2, N3: records corrected ("E4"; "17 of 18"). N4: `t_*` probes are deleted by the e2e start-up clean-up (lesson).
- **Status:** closed 2026-10-16 on evaluator pass 2 (`tasks/eval-m9b-pass2.md`) VERDICT PASS (0 blocking; 3 should-fix and 4 notes answered in amendment 8).

## M9c Tier B call recall, the scip-go image, cross-module scip-go joins: built 2026-10-17

- **Discussed and decided:** the contract (`tasks/contract-m9c.md`, C1–C16, amendments 1–2 and phase notes PI–P7b) checked the plan against the code first. B3's cause is strict JSON parsing of tsconfig `paths`, not namespace imports. B2's misses are five syntactic forms (call results, call segments, aliases, embedding, package vars), not single-implementer interfaces (0 measured misses). B5 adds inherited `__init__`. P0 found the 3 `App#GetClusterId` fp came from scip-go recording no occurrence where an imported package failed to load, and P0b traced that to rb's image.
- **The scip-go image, root cause (amendment 2, PI):**
  - `docker/scip-go/Dockerfile` built scip-go v0.2.7 with Go 1.27.2, whose compiler writes unified export data version 5 (`uirVersion = pkgbits.V5`, a 1.27.2 backport for golang/go#81188 and #81306). scip-go loads imports from export data through x/tools v0.45.0, whose decoder refuses anything above version 4.
  - So go/types made every import, the standard library included, a fake package with no members. scip-go swallows package errors and exits 0. Since the image was created (M6d, `1f155c7`), every scip-go index rb made came from that degraded type check: 119,977 of server's 202,385 locals `invalid type`, no member occurrence of any dependency (M9b's "nothing to join"), and go/types' "assume implements" for invalid types (B13b's 322 false-or-unverifiable `IMPLEMENTS`).
  - Fixed by building v0.2.7 against x/tools v0.50.0, the first release that reads version 5. `go.mod` and `go.sum` are pinned by sha256, `go build` runs `-mod=readonly`, `go version -m` must list x/tools v0.50.0, and a build-time self-test fails the build unless scip-go writes `os/Getenv().` for `os.Getenv(…)`; its control fails on the old image. Tag **`rb-scip-go:0.2.7-xtools0.50.0`** (`SCIP_GO_IMAGE`), image ID `sha256:47eab865…`; a new image ID misses every cached Go index. A newer scip-go (none released after v0.2.7) and a Go that writes ≤ V4 were rejected.
  - D7 (a method-set check on scip-go type relationships) was withdrawn: on the fixed image it catches 0 false rows and drops 3 true ones. D8 became D8′, a symbol join, because imports now load and the references are `external`, not `unmatched`.
- **Built:**
  - PI: the image, the tag constant, a unit test tying the tag to the Dockerfile's pins, both READMEs; 2 e2e golden rows re-blessed (`external` up, `unmatched` down by the same).
  - B3 (P1): `rb_scip::jobs::parse_jsonc` for tsconfig `paths` (`AliasMaps::build` only); TS/JS default imports through `default_export`.
  - B2 (P2, P3): Go facts (`return_type`, `alias_of`, embedded member bindings, untyped `var_spec` literals, the `:=` call pair) and resolution through call results, call segments, aliases, embedding and package vars; `tier_b_forms` (D6).
  - B5 (P2, P4): the `raise` query; `super()`, star imports, re-exports, module variables, inherited `__init__`.
  - B1 (P5, D8′): external scip-go references into another ok job's `go.mod` module are joined after the job loop as `synthetic` `scip` edges; `scip_cross_project`; `exact calls (scip)` counts `exact` only.
  - P6: `PIPELINE` `m9c-1`; design notes §5.2–§5.4; usage guide.
  - P7b: a per-join memo so the SCIP join parses each distinct symbol once (`NameForms`, `NameMemo`); output unchanged.
- **Files:** `crates/rb-scip/src/{jobs,lib,symbol}.rs`, `crates/rb-indexer/src/{lib,resolve,rta,scip}.rs`, `crates/rb-syntax/src/{lib,bindings,decorations,queries,keys}.rs`, `crates/rb-store/src/{query,db_tests}.rs`, `crates/rb-cli/src/doctor.rs`, `crates/rb-cli/tests/golden/{go-chi-sqlx,polyglot-monorepo}/scip-status.txt`, `docker/scip-go/{Dockerfile,README.md}`, `bench/probe/m9c_forms.py`, `bench/mutants/m9c.json`, `bench/samples/exact.tsv`, `bench/scoreboard/m9c{,-perf}.json`, `bench/accept-m9c.tsv`, `README.md`, `docs/{usage-and-testing,repository-brain-design}.md`.
- **Results** (binary `f340b267…06c9` from `55592a1`, run `var/bench/out/f340b267d529/`):
  - scip `calls` agreement P 78.4 % → 99.5 %, R 62.4 % → 96.3 %; mattermost gold 33,312 → 43,840, fp 6,141 → 203, fn 12,672 → 1,659. 23,186 `synthetic` cross-module scip edges. The precision gain is the gold growing: the `synthetic` joins are scip gold, 5,944 of the 6,141 base fp became tp through them, and against `exact` gold alone P reads 78.2 % (amendment 3, E5).
  - bookcars `calls` R 62.5 % → 99.6 % (fn 694 → 8); go_clean_arch and go_gin_example 100 %; PyCG `calls` 12 → 14 of 48, `calls_file_scope` 94 → 105 of 186.
  - 17 of 314 cells changed; `compare` exits 0 with 6 accepted lines over 5 cells. Invariant: 0 unclassified rows over 157 dumps; no Tier B edge `exact`.
  - `exact` 100.0 % of 126; the `scip` sample's 54 redrawn edges all TRUE.
- **Verification:**
  - Mutants m9c 38/38 (m300–m330, m334–m338, m340, m341), m9b 39/39, m9a 31/31, m8b 29/29 KILLED, all four re-run on the final code after P7b.
  - Real-Docker 1,154/0/0 (0 `skipping:`) on the final code after P7b; 1,153/0/0 at P6.
  - fmt and clippy clean; `rb.repo` unchanged; two scorings `cmp`-identical.
  - Perf: mattermost `noscip-fresh` A/B noise (CPU 1.010×); SCIP mode 1.297× CPU at P7, 0.975× after P7b's memo.
- **Open, carried to M9d:** version skew (185 of the 203 mattermost fp; every D8′ join `synthetic`), `(T, *model.AppError)` results, the `scip` crate parser's remaining `Debug` cost, and the 4,092 interface-method references no server/public definition anchors.
- **Evaluator pass 1 (`tasks/eval-m9c-pass1.md`: PASS, 0 blocking, 6 should-fix, 6 notes; contract amendment 3; code `e58f6ee`, `PIPELINE` `m9c-2`):**
  - E1: a Go result named by a type parameter types nothing (Go facts record type parameters); `select` receives declare their names.
  - E2: Python member lookup and inherited `__init__` follow the C3 MRO; `super().m()` is the MRO's next class (N4); the later of a definition, named import and star import wins.
  - E3: tests for the evaluator's surviving mutants; e1, e2, e4, e8 adopted as m356–m359; e5 equivalent, its redundant check removed.
  - E4: tsconfig BOM and `"baseUrl": "."`.
  - E5: 60 seeded D8′ `synthetic` `CALLS` judged blind, 59 TRUE (`bench/samples/synthetic-d8.tsv`); the FALSE class (a Go conversion labelled `CALLS`, 77 rows) is now `REFERENCES`. Against `exact` gold alone the scip `calls` agreement P is 78.2 %.
  - E6: r2 benched on a quiet host; mattermost walls 26.10 / 16.63 / 20.34 s, within 1.15× of a standalone timing; M9d's base.
  - r2: binary `3b94ded4…9db6`, run `var/bench/out/3b94ded449f8/`, 156 rows changed against r1 (154 conversions, 2 PyCG), 0 unclassified; 53 redrawn `exact` edges, 53 TRUE; `compare` exit 0; RSS A/B noise.
  - Verification: real-Docker 1,156/0/0; mutants m9c 56/56, m9b 39/39, m9a 31/31, m8b 29/29 KILLED.
- **Status:** built; evaluator pass 1 answered and re-closed; pass 2 pending.
