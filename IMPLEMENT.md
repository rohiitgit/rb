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
