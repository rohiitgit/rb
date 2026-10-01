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

## M3b SCIP orchestration: complete (pending the evaluator's pass-5 re-check)

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
- **Open:** Gradle (M6), workspace installs for nested npm packages (M6), tsconfig `include`/`exclude` awareness for excluded files (M6), lockfile-resolved package versions (M4 `DEPENDS_ON` work), a registry allow-list for the fetch phase (M5; design §14).

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
- [ ] Evaluator re-check of pass 5

### Environment incident (2026-09-30 evening, wall clock)

- During the pass-2 fix run the host disk reached 0 bytes free, `~/.colima` (the VM holding the brain database and both indexer images) was removed by something outside this project's commands, and Docker Desktop became the active context. The agent's transcript shows only reads of `~/.colima`; its deletions were `target/debug/incremental` and session scratch.
- The old VM was still running from the deleted files when this was written, so the database on port 55432 still answered. Everything in it is lost when that VM stops. The SCIP index cache rows were exported to `var/scip/cache/scip_index.copy` (11 rows).
- **Consequence for verification:** the pass-2 fixes (amendments 21–24) are verified by unit tests (276), fmt and clippy, and by the e2e suite with Docker hidden (SCIP tests skipped). `scip_java_spring_jpa` and `scip_polyglot_monorepo` were **not** re-run with containers; two of the three `scip-status.txt` goldens were edited by hand from a join over the cached indexes. They must be re-verified once a container runtime with `rb-scip-java:0.13.1` and the scip-typescript image exists again.
- **To restore:** a container runtime whose VM shares the home directory; `docker compose up -d --wait` and `rb migrate` for the database; `docker build -t rb-scip-java:0.13.1 docker/scip-java`; pull `sourcegraph/scip-typescript@sha256:028c0042…` for linux/amd64.
- Follow-up: the e2e fixture copy should skip ignored files (an IDE's `target/` inside a fixture broke `orm_table_deps_goldens`).
- **Restored on Docker Desktop (the user's choice, 2026-09-30 22:00 wall clock):** `compose.yaml` takes its host port from `RB_DB_PORT` (default 55432); the database runs on 55433 while the dead VM's forward holds 55432, freshly migrated (0001–0004). `--network none`, amd64 emulation and home-directory bind mounts were re-verified there.
