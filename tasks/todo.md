# Repository Brain — design doc

Deliverable: `docs/repository-brain-design.md` (design only, no code).

- [x] Verify external facts (AGE PG versions + Cypher gaps, scip crate, SCIP indexers, pg_query.rs, tree-sitter crates, gix)
- [x] 1. Goals / non-goals
- [x] 2. Architecture overview + diagram
- [x] 3. Ingestion (git, CODEOWNERS, service boundaries)
- [x] 4. Syntactic extraction (Tree-sitter)
- [x] 5. Semantic resolution (SCIP)
- [x] 6. Call-graph construction + edge provenance
- [x] 7. SQL / data-access extraction
- [x] 8. Storage (Postgres + AGE)
- [x] 9. Query layer — four questions + service card
- [x] 10. Cross-service request paths
- [x] 11. Interfaces (CLI, REST)
- [x] 12. Testing strategy
- [x] 13. Phased roadmap
- [x] 14. Risks & open questions
- [x] ADR: implementation language
- [x] Self-review checklist

## Review
- Doc written: `docs/repository-brain-design.md` (~900 lines, 14 sections + 3 appendices).
- External facts verified by a research pass; sources are in Appendix C. Items still unverified are listed there.
- The §8.3 DDL was executed on local PG 14 inside a rolled-back transaction. The set-semantic `WITH RECURSIVE … UNION` reachability query terminates on a cycle.
- AGE-specific SQL (the `cypher()` prepared statement, the loaders) has **not** been executed, because there is no AGE instance locally. That is an M0 check.
- Self-review:
  - every edge kind in §8.5 has a producer;
  - Q1–Q4 map to §9.2, §9.3, §9.4 and §10.3;
  - the corrections to the blueprint are in §2.
- No tsc/eslint: there is no code in this deliverable.

---

# M0 Foundations — implementation (started 2026-09-29)

Exit criteria (design §13): determinism test green on 2 fixtures; `rb cypher` returns files per service.
Scope note: Owner projection needs CODEOWNERS, which is M1. M0 projects File + Service (single `(root)` service) + IN_SERVICE.

- [x] Batch 1 — infra: Cargo workspace, rust-toolchain, compose.yaml (AGE PG18 pinned by digest, port 55432), migration 0001, .gitignore. Verify: DB healthy, migration applies, AGE loader signatures probed.
- [x] Batch 2 — rb-model (kinds, snapshot graph, deterministic ids, canonical dump) + rb-git (gix manifest). Verify: unit tests, clippy -D warnings, fmt.
- [x] Batch 3 — rb-store (migrate, snapshot lifecycle, COPY load, AGE CSV projection, live flip, retention, dump) + rb-cli (`migrate`, `index`, `status`, `dump`, `cypher`). Verify: build + clippy.
- [x] Batch 4 — 2 fixtures + integration tests (determinism, AGE parity, cypher files-per-service), README, doc sync. Verify: full test run against the compose DB.

## M0 review (2026-09-29)

- **Exit criteria met.**
  - Determinism: the same commit indexed into two repos, and a forced rebuild, dump byte-identically on both fixtures. The stored dump also equals the in-memory pipeline's dump.
  - `rb cypher` returns files per service.
  - AGE parity: every relational `IN_SERVICE` edge appears in the projection.
  - `rb cypher` is read-only: a `CREATE` is rejected.
- **Checks:** 19 tests pass, 2 of them end to end against the compose database. `cargo clippy -D warnings` and `cargo fmt --check` are clean.
- **Real-repo smoke test:** A private repo indexed 403 files in 0.26 s, matching `git ls-tree`. Retention keeps the live snapshot plus 2 retired ones and drops their AGE graphs. A failed load rolls back fully (no graph), is marked `failed`, and is cleaned up by the next successful publish.
- **Deviations from the design, now synced into the doc:**
  - AGE loader paths must sit under `/tmp/age/`.
  - `rb.snapshot` is no longer unique on (repo, commit, tools); there is a partial unique index for one live snapshot per repo instead.
  - The Owner projection moved to M1.
- **Environment:**
  - Rust is installed via `brew install rustup`, toolchain pinned in `rust-toolchain.toml`. `~/.zshrc` is untouched, so PATH needs `/opt/homebrew/opt/rustup/bin`.
  - The database runs on port 55432, because 5432 and 5433 are already taken.
- **Not done:** the licence audit (`cargo deny`) listed in design §14. Nothing has been committed (newide is not its own git repo yet).

## Next: M1 Structure (awaiting confirmation)
- CODEOWNERS matcher (in-house on `globset`, last match wins, no `!`/`[]`) → Owner nodes + OWNED_BY
- Service boundary detection (package.json workspaces, Maven/Gradle modules, go.mod, Cargo, pyproject) + `rb.toml` overrides
- Tree-sitter defs/refs/scopes for TS, Java, Python, Go; facts cache keyed by blob SHA
- `rb find`, `rb service` (structure sections)

---

# M1a Structure — CODEOWNERS, boundaries, evidence, `rb find` / `rb service` (started 2026-09-29)

Design decisions made while planning (sync into the doc at the end):
- Every detected project becomes a `Service` node with `deployable: bool`. Libraries are services too; this matches the Service Card example ("shared-lib").
- Service key = the project directory (`.` for the repo root). The synthetic `(root)` service holds files outside every project. Names can collide, so `rb service <name>` returns candidates rather than picking one.
- CODEOWNERS uses an in-house segment matcher (GitHub semantics: `docs/*` is not recursive, and there is no `!`/`[]`). The same matcher handles `rb.toml` exclude globs, so there is one glob engine and no `globset`.
- Edges carry evidence (file path + span), stored in `rb.edge_evidence` and included in the dump (dump format v2).
- Assembly moves from rb-model into a new `rb-indexer` crate (git → structure → graph + diagnostics).

- [x] A — model: `NewEdge`, `Evidence`, `Diagnostic` in rb-model; dump v2; store persists and loads evidence. Verify: all existing tests.
- [x] B — rb-git `Repo` API (manifest + blob reads); `codeowners` module with GitHub-documented table tests.
- [x] C — `boundaries` module: package.json, pom.xml, gradle, go.mod (+ `package main` dirs), Cargo.toml, pyproject, Dockerfile, `rb.toml` overrides/excludes. Unit tests.
- [x] D — `rb-indexer` assembly (Service/Owner nodes, IN_SERVICE, OWNED_BY, diagnostics); store queries; `rb find`, `rb service`.
- [x] E — `polyglot-monorepo` fixture; golden tests for the service list and cards; doc sync. Verify: fmt, clippy, full test run on the DB.

## M1a review (2026-09-29)

- **Checks:** 43 tests pass, 5 of them end to end on the compose DB. Clippy with `-D warnings` and `fmt --check` are clean. No test repos are left in the DB.
- **Goldens:** 7 files for `polyglot-monorepo`, each reviewed by hand against the rules: services, cards, search, and the full dump with evidence and diagnostics. The test was shown to fail when a golden file is tampered with.
- **Real repos:**
  - A private TypeScript repo resolves to one service.
  - A private Java repo resolves to 4 projects, matching its manifests.
- **Bugs found and fixed:**
  - `pg_trgm` was installed into `ag_catalog`, because migrations inherited the AGE search_path. Migration 0003 moves it, and the runner now pins `search_path = public`.
  - A Go `package main // comment` line was not detected.
  - Card output said "1 files".
- **Design decisions,** synced into the doc (§4.2, §4.3, §8.2, §12.3, §13):
  - every project is a Service with `deployable`;
  - the Service key is the project directory;
  - the glob matcher is in-house and shared with `rb.toml`;
  - CODEOWNERS is tested with GitHub-example table tests instead of the planned property tests.
- **Not committed yet:** waiting for confirmation.

## Next: M1b Syntax (awaiting confirmation)
- Tree-sitter grammars (TS/TSX, Java, Python, Go) + `tags.scm` definitions/references + enclosing-scope index
- Symbol nodes (heuristic keys until SCIP lands in M3), `DEFINED_IN` / `CONTAINS` edges with evidence
- Facts cache keyed by (blob SHA, extractor version); `gix` change sets; rayon parallel extraction

---

# M1b Syntax (contract: tasks/contract-m1b.md)

Findings from the research step (2026-09-29):
- **Crates:** `tree-sitter` 0.27.0; grammar crates typescript 0.23.2, javascript 0.25.0, java 0.23.5, python 0.25.0 and go 0.25.0. Each exports `TAGS_QUERY`.
- **TypeScript:** `tags.scm` has only the TS additions (signatures, interfaces, abstract classes, modules). TS/TSX must run the JS query plus the TS query.
- **Java:** the stock tags have no enum, record or constructor definitions, so rb adds a small supplementary query.
- **Go:** method keys need the receiver type (`Circle#Area().`), otherwise two receivers' `Area` in one file collide. The receiver is read from the node in Rust.

- [x] 1. `rb-syntax` crate: the language registry (C14) and `extract(lang, bytes) -> FileFacts`, with tests first for C1–C4, C8, C13 and C17.
- [x] 2. Symbol keys + indexer assembly: Symbol / DEFINED_IN / CONTAINS (C2, C5, C6, C7); unit tests.
- [x] 3. Facts cache: store load/save + CLI orchestration + parse counter (C9–C12).
- [x] 4. Fixture goldens (C15, C16), real-repo measurement (C18), doc sync, evaluator pass, commit.

---

# M2a SQL core (contract: tasks/contract-m2a.md)

- [x] 1. `rb-sql` crate: statements, tables and operations via pg_query (S1–S5); 7 tests.
- [x] 2. rb-syntax: SQL candidates from string literals (template normalisation, callee name, prefilter) (S7, S8).
- [x] 3. Indexer: Table nodes, MIGRATES from `.sql` files, QUERIES from candidates, diagnostics (S6, S9–S12).
- [x] 4. `rb table-deps`, SQL coverage in `rb status`, fixture goldens (S13–S15).
- [x] 5. Public-repo measurement (S16), evaluator pass, fixes, docs, commit.

---

# M2b ORM mappings (contract: tasks/contract-m2b.md)

- [x] 1. rb-syntax: generic decorations, supertypes and Python class constants (O1).
- [x] 2. rb-frameworks crate: JPA entities and naming, Spring Data repositories and derived queries, `@Query` (native SQL and JPQL), JPQL resolution, Prisma schema and client calls, TypeORM, SQLAlchemy, Django (O2–O7).
- [x] 3. Indexer: MAPS_TO and ORM QUERIES edges, `jpql_unresolved_entity` diagnostics; table-deps "Mapped by" (O8).
- [x] 4a. Public repos (O9) measured; re-measured after fixes (Django 55, reconciled).
- [x] 4b. Evaluator pass 1: 13 defects fixed test-first (contract Amendment 1); design §7.4.
- [x] 4c. Evaluator passes 2–7 (Amendments 2–7), commit.

# M3a Tier B resolution, callers and impact (contract: tasks/contract-m3a.md)

- [x] 1. rb-syntax: imports, package clause, typed bindings (R1, R2).
- [x] 2. Indexer: module resolution (R3).
- [x] 3. Indexer: call, type and implements resolution; coverage (R4–R8).
- [x] 4. rb-store and CLI: `rb callers`, `rb impact` (R9, R10); fixtures and goldens (R11).
- [x] 5a. Public repos (R12): 80/80 sampled edges correct; stack overflow found and fixed.
- [x] 5b. Evaluator passes 1–6 (Amendments 1–6), docs, commit.

# M3b SCIP orchestration and exact edges (contract: tasks/contract-m3b.md)

- [x] 0. Research: indexer images/digests/invocations, `scip` crate API, fixture buildability; contract facts recorded.
- [x] 1. rb-scip: decode `index.scip`, position-encoding conversion (S1).
- [x] 2. rb-scip: project detection, container jobs, fetch/index phases, cache (S2–S4).
- [ ] 3. Indexer: the SCIP⋈Tree-sitter join, key replacement, exact edges, packages, coverage (S5–S9).
  - [x] 3a. Join: `Source` enum for file-level edges, targets limited to symbols the graph has, coverage.
  - [x] 3b. `index_commit` runs jobs (`ScipOptions`, `ScipCache`, Docker check), decodes, assembles with a precomputed keys map, excludes SCIP-ok files from Tier B, emits `scip_status`; CLI flags `--no-scip`, `--scip-work-dir`; `rb status` lines; `exact_calls`; e2e `scip_ts_express_prisma` with goldens; docs.
  - [x] 3c. scip-java through the slim local image (Maven only; Gradle stays `unsupported`):
    - [x] rb-scip: `SCIP_JAVA_IMAGE` tag, `Runner::image_id`, `resolve_image` (the image ID is the cache key for a local image; a missing image is `unsupported`), `java_runs` (fetch: go-offline + verify online with the m2 cache; index: `scip-java index --build-tool Maven -- --batch-mode --offline …` with `--network none`), `run_job` takes the resolved image; fake-runner tests.
    - [x] rb-indexer: `run_projects` resolves the image per job; join fallback for scip-java's annotation-anchored definitions (`definitions_matched_by_name`), unit test on a synthetic index.
    - [x] Fixtures: polyglot `services/auth/pom.xml` gets its dependencies; `services/broken` fails to build on purpose; `var/scip/cache/m2` seeded from the spike.
    - [x] e2e `scip_java_spring_jpa` (callers/impact/status goldens, cached re-index, byte-identical dump) and `scip_polyglot_monorepo` (status lines: failed + ok + TypeScript; `rb index` succeeds); `--no-scip` polyglot goldens re-blessed for the new files only.
    - [x] Docs: contract amendments 7–10, IMPLEMENT.md, CHANGELOG.md; verification loop; disk check.
    - Review: `rb callers` reports the dispatch step as `dispatch` (not `exact`) by the M3a CHA rule even though both edges are exact (amendment 9); `apps/web` fails under SCIP because `@acme/shared` is a workspace package (M6).
  - [x] 3d. Packages / `DEPENDS_ON` from external symbols (S8): `rb_scip::package` + key rule (unit), `Joined::packages` (unit), one edge per service and package with one evidence (unit), `ServiceCard.packages` + `rb service` rendering, e2e card goldens on all three SCIP fixtures. Lockfile versions: out of scope (amendment 12).
- [ ] 4. Public repos (S11), evaluator passes, docs, commit.
  - [x] spring-petclinic with scip-java: measured, 20/20 sample, callers compared with the heuristic twin, `CALLS` diff classified.
  - [x] hono → class-validator (amendment 13): measured, 20/20 sample, callers compared; found the "files without a document" gap and fixed the count (amendment 14).
  - [x] m3b repos deleted from the database, work dirs removed, caches kept; disk figures in the contract.
  - [x] Docs: contract (S8, S11, amendments 11–14, measurement section), IMPLEMENT.md, CHANGELOG.md, design §5.5/§13, lessons.
  - [ ] Evaluator passes, commit.
    - [x] Pass 1 fixes (test-first, one amendment each, 15–20):
      - [x] D1 ownership by document: `Joined.documented` → `exact_files`; `files_without_document` counts the indexer's languages only; unit (Python under Maven, excluded TS) + e2e (`scripts/tool.py` in the Java fixture; re-bless `scip-card.txt`, `scip-status.txt`).
      - [x] D2 `IMPLEMENTS` direction: only when A's type (or its method's enclosing type) declares a supertype named like B's last type descriptor; unit on a symmetric index; e2e golden `scip-impact-default-authenticate.txt`.
      - [x] D3 `tool_versions.scip` = `off` / `unavailable` / `on` (`ScipMode` decided by the caller); e2e with Docker hidden from the child's PATH.
      - [x] D4 `--name rb-scip-<hash16>-<phase>`, `docker kill` on timeout, `--scip-timeout-secs`; D7 `--no-cache` bypasses SCIP cache reads (`WriteOnlyScipCache`); unit (names, kill through a fake `docker` script) + e2e (`--no-cache --force` runs the container).
      - [x] D5 decode dedup through `BTreeSet`; timing test 40k occurrences < 1 s.
      - [x] D6 `unpinned` in the `scip:` lines; polyglot status golden re-blessed.
      - [x] Docs: contract (invariant 1, S5/S7, verified facts, S11 wording, amendments 15–20), IMPLEMENT.md, CHANGELOG.md, lessons.
      - [x] Verify: fmt, clippy, unit tests (awk-summed), e2e with containers, stub scan, no `t_*` repos, no work dirs.
    - [ ] Evaluator pass 2 (test-first, one amendment each, 21–24):
      - [x] E1 `IMPLEMENTS` through a hierarchy: `rb_scip::type_symbol_of` (replaces `last_type_name`); the join walks A's holder's declared supertypes (by simple name, visited set, depth cap 64) to B's type definition found exactly by symbol and compared by (document, index). Unit: abstract class + interface chain, two-class chain, the reverse still rejected, a same-named interface in another package, a generic supertype.
      - [x] E2 one run instance per `run_job`: work dir `<work_dir>/<hash16>-<pid>-<nanos>`, containers `rb-scip-<hash16>-<pid>-<nanos>-<phase>`; cache key unchanged. Unit: two runs of the same job share no directory and no name; each name is its instance's.
      - [x] E3 `references_undocumented`: an undefined symbol of the project's own package counts there, not as external; status line and diagnostic; the three status goldens changed, each moved reference identified in the cached index.
      - [x] E4 `resolve_target` accepts a key suffix after a space as well as after `/`. Unit in `rb_store::code`.
      - [x] Docs: contract (S5, amendments 21–24), IMPLEMENT.md, CHANGELOG.md, lessons.
      - [x] Verify without containers: fmt, clippy, unit tests (awk-summed), e2e against the database with Docker hidden, stub scan, no `t_*` repos, no work dirs, `df -h /`.
      - [x] Verify with containers, once a runtime with both images exists: `scip_ts_express_prisma` without `RB_BLESS` (it passed once under `RB_BLESS=1`, before the runtime was lost), `scip_java_spring_jpa` and `scip_polyglot_monorepo` (both skipped: no scip-java image; their status goldens were edited by hand), and two concurrent `rb index --no-cache --force` of one fixture (E2). Done on Docker Desktop (CHANGELOG 22:00 entry; the evaluator's pass 3 reproduced E2 as not reproducible).
    - [x] Evaluator pass 3 (test-first, one amendment each, 25–42; `tasks/eval-m3b-pass3.md`):
      - [x] Phase A (rb-syntax, rb-scip): F4 `extends_type_clause` supertypes; `Supertype.span`; F1 export skips `mvnw`/`mvnw.cmd`/`gradlew`/`gradlew.bat` at the build root; F3 `--ignore-scripts`; F12 `--` before scip-typescript projects; F6/F7 `environment` status and uncached timeouts with phase and budget; F5 `Images` resolved once; F18 dead decoded fields.
      - [x] Phase B (rb-indexer): F2 supertypes resolved through the index's own occurrences (`declared_supertypes`, `supertypes_unresolved`; J1 and T1 regression tests); F18 set lookup, PIPELINE `m3b-scip-1`, debugging aid deleted (F11); F8 no `elapsed_ms` in the diagnostic; F9 `occurrences_unconverted` in coverage; F14 `assemble` ownership test; F6/F7 cache rules in `run_projects`.
      - [x] Phase C (rb-store, rb-cli): F15 dotted form across the space and scip-java display; F17 `no heuristic references`; F5 `tool_versions` image identities; F8 `rb status` prints no time; F13 `--scip-work-dir` help; F18 `render_card` last line.
      - [x] Phase D (e2e, fixtures, goldens): F16 `copy_dir` from `git ls-files` + file-set assertion; F9 fixture line and span assertion; F10 `--no-cache` steps (polyglot: failed jobs' stderr tails excluded, amendment 34); F8 two fresh runs byte-identical, `mask_elapsed_ms` gone; F5 shim test; goldens re-blessed by container runs (14 e2e tests green on Docker Desktop).
      - [x] Phase E: public repos (petclinic with its wrapper, class-validator), precision samples, docs (contract, README, design §5.2/§5.3/§13/§14, IMPLEMENT.md, CHANGELOG.md, lessons), verification loop, cleanup of `var/fix-pass3/`, commit.
    - [x] Commit (pass 3).
    - [x] Evaluator pass 4 (test-first, one amendment each, 43–52; `tasks/eval-m3b-pass4.md`):
      - [x] G1/G3 name-span supertypes: `Supertype.span` is the type's name only (Java `generic_type` name child, `annotated_type` inner type; TS `extends_clause.value`, non-name values yield nothing); `declared_supertypes` takes the last type occurrence inside the name span; units `supertype_spans_cover_the_name_only`, `a_nested_type_argument_is_not_the_supertype`, `an_annotated_supertype_resolves_to_the_type`; J1/T1 kept.
      - [x] G2 unfinished jobs re-index: `Store::unfinished_scip_jobs`, `rebuild_reason` in `rb index` (`rebuilding: n SCIP jobs did not finish last time`); unit `rebuild_reason_requires_finished_scip_jobs`; e2e `scip_unfinished_jobs_are_not_up_to_date` (shim exit 125, then real docker; `--scip-timeout-secs 1`, then default budget).
      - [x] G4 `--logs-max=0` on the npm fetch (unit); S12 and invariant 5 restated (amendment 46); polyglot e2e asserts the npm tail is byte-identical between fresh runs.
      - [x] G5 `rb_scip::RECIPE` (`m3b-2`) mixed into `project_input_hash`; unit `the_recipe_is_part_of_the_key`.
      - [x] G6 scip-typescript identified by its digest reference; the e2e shim refuses `image inspect` for `rb-scip-java:0.13.1` only; mutation-checked (deleting `tools["scip_java"]` fails the test).
      - [x] G7 fixed `skipped` reason (`docker is not available`), `environment` reason redacted (`redact_host_paths`), detail on stderr only; units in rb-indexer and rb-scip.
      - [x] G8 `rb status` prints a timeout's detail once (`scip_status_line_prints_a_timeout_once`).
      - [x] G9 README "Container runtime" note, `--scip-work-dir` under the home directory; design §5.2.
      - [x] G10 `relationships_dropped` in coverage and on the `scip:` line (`dropped_relationships_are_counted`); goldens re-blessed by container runs, the Java fixture's 8 identified.
      - [x] Verify: fmt, clippy, unit tests (awk-summed), e2e twice with containers, stub scan, public repos (petclinic 13 `IMPLEMENTS` read in the source, class-validator), cleanup, commit.
    - [x] Evaluator pass 5 (test-first, one amendment each, 53–58; `tasks/eval-m3b-pass5.md`):
      - [x] Amendments 53–58 written first (S7 restated in the body and in 54; amendment 2 note).
      - [x] H2 comments skipped in heritage clauses (rb-syntax; `comments_in_heritage_clauses_are_not_supertypes`).
      - [x] H1 measured with the real scip-java on `var/fix-pass5/tabs` (68/68 occurrences explained by "tabs to multiples of 8, minus 7 per leading tab"); decoder `Units::Javac` (`javac_columns_expand_tabs_to_multiples_of_eight`, `javac_columns_inside_a_tab_map_to_the_tab`); join guard (`a_supertype_occurrence_must_name_the_written_type`).
      - [x] H3 local definitions bound per document (`definitions_local`), local → global forward, `relationships_dropped_in_project`; four join units, mutation-checked; e2e sweep restated.
      - [x] H4 `RunStatus::Failed(Phase)`, detail `phase`, `failed (fetch phase)`, migration 0005 `rb.scip_index.phase`; units in rb-scip, rb-indexer, rb-cli; polyglot e2e.
      - [x] H5/H6 docs (amendment 57, design §5.3/§13, amendment 2, singular "1 SCIP job").
      - [x] Verify: fmt, clippy, 294 unit tests, e2e twice with containers (no `skipping:`), stub scan; goldens re-blessed (3 status files) and read; public repos fresh == cached: petclinic 240/240, 256, 13; class-validator 417/424 + 6 local, 398, 2 (diff classified); jsoup 4,874 + 117 local, 14,013, 688, 0 in project; samples 20/20 and 10/10.
      - [x] Records: contract measurement, CHANGELOG, IMPLEMENT.md, lessons; cleanup of `var/fix-pass5/` and `fx5_*`; commit.

## Review (evaluator pass 2)
- The four defects are fixed test-first; each new test failed at HEAD 18970c7 for the reason the evaluator gave (the second E1 test, direct cases, passed before and after).
- Found while hand-checking E3: the Java fixture's two "external" references that moved are a record's compiler-generated accessors, so `references_undocumented` covers generated members as well as files without a document.
- Environment, not code: the container runtime (colima) disappeared during verification and Docker Desktop was started by someone else, so no SCIP e2e was re-run; an IDE build directory (`fixtures/java-spring-jpa/target/`) appeared in the Java fixture and breaks `orm_table_deps_goldens` while it is there, because `fixture_repo` copies untracked files.

## Review (evaluator pass 3)
- All 18 findings fixed test-first (amendments 25–42). The join resolves declared supertypes through the index's own heritage occurrences (J1 no longer yields the reverse exact edge; T1 chains through a TypeScript interface); the export leaves Maven/Gradle wrappers out, so spring-petclinic indexes with its `mvnw` present on Docker Desktop.
- Found while verifying: a failed job's `stderr_tail` is not a function of the commit (Maven's negative-cache wording, npm's timestamped log path), so the polyglot fresh-run comparison excludes those two fields (amendment 34). `--ignore-scripts` leaves Prisma's client ungenerated, which moves one TypeScript reference from `undocumented` to `local` (amendment 27).
- Environment: the Docker CLI's current context was `colima` (a dead socket); every container step ran with `DOCKER_CONTEXT=desktop-linux` and nothing touched the context setting.

## Review (evaluator pass 4)
- All ten findings fixed test-first (amendments 43–52). A declared supertype's span is now its name only and resolves to the last type occurrence inside it, so a nested type argument (`Marker<Marker.Disc>`) or a type-use annotation (`@Tag Shape`) no longer picks the wrong type; a snapshot whose SCIP jobs did not all finish is rebuilt by the next plain run; the fetch recipe is part of the index cache key.
- Found while verifying: a session slip — `git checkout -- <file>` to undo a temporary trace discarded the file's uncommitted work; it was restored from the diff recorded in the session transcript and verified by the file's test suite and the unchanged diff stat (lesson recorded).

## Review (evaluator pass 5)
- All six findings fixed test-first (amendments 53–58). The tab rule was measured before it was coded: every scip-java occurrence on the scratch project's tab lines is javac's tab-expanded column minus 7 per leading tab, so the decoder converts it and the join's name guard is a second line of defence. Local definitions are bound per document and keep heuristic keys; jsoup's join gaps dropped to 0 in project.
- Found while measuring: the local binding applies to scip-typescript too (class-validator +6 correct edges), and scip-typescript emits a reference, not a definition, at a named class expression, so one class-validator definition stays unanchored; S7 names that case. jsoup's 52 relationships from anonymous classes' methods stay dropped, because Tree-sitter has no definition for them (design §8.2); they are not counted as in-project gaps.
