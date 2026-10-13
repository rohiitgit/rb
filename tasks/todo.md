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
- [x] 3. Indexer: the SCIP⋈Tree-sitter join, key replacement, exact edges, packages, coverage (S5–S9).
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
- [x] 4. Public repos (S11), evaluator passes, docs, commit.
  - [x] spring-petclinic with scip-java: measured, 20/20 sample, callers compared with the heuristic twin, `CALLS` diff classified.
  - [x] hono → class-validator (amendment 13): measured, 20/20 sample, callers compared; found the "files without a document" gap and fixed the count (amendment 14).
  - [x] m3b repos deleted from the database, work dirs removed, caches kept; disk figures in the contract.
  - [x] Docs: contract (S8, S11, amendments 11–14, measurement section), IMPLEMENT.md, CHANGELOG.md, design §5.5/§13, lessons.
  - [x] Evaluator passes, commit.
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
    - [x] Evaluator pass 6 (test-first, one amendment each, 59–63; `tasks/eval-m3b-pass6.md`):
      - [x] I1 measured first with both real images on `var/fix-pass6/eol` (javac: `\n`, lone `\r`, `\r\n`; TypeScript: those and U+2028/U+2029; neither U+0085; 72/72 occurrences explained); `rb_scip::decode` converts every position through its byte offset by the indexer's own line rule (`Lines::position`, one path with the column rules); name guard on exact-span definitions and references (`names_token`, `rb_scip::may_name`; strict-equality rejections 2,137 enumerated into constructors, `validate0:`, accessors, default-import aliases; 0 with the rule on four repos, fixtures, scratch). Units in rb-scip and rb-indexer (through the real decoder), mutation-checked.
      - [x] I2 bound locals are in project by binding (`typescript_local_to_local_relationships_are_counted_in_project`).
      - [x] I3 qualified supertype names built from identifiers (`comments_inside_a_qualified_supertype_name_are_skipped`).
      - [x] I4 `definitions_without_occurrence`, `relationships_anonymous` stored and printed; amendment 54's local-class sentence corrected; `a_call_to_a_local_definition_is_exact` in scip-typescript's layout; goldens re-blessed by a container run (two new zero fields per `ok` line).
      - [x] I5 design §13/§5.3, `PIPELINE` `m3b-scip-2` with the bump rule.
      - [x] spring-retry tests without documents: cause found (Develocity local build cache serves `testCompile`), recorded as an open item.
      - [x] Verify: fmt, clippy, 300 unit tests, e2e twice with containers (no `skipping:`), stub scan; public repos fresh == cached, edge sets identical to `79d2c33`; records; cleanup; commit.
    - [x] Evaluator pass 7 (test-first, one amendment each, 64–67; `tasks/eval-m3b-pass7.md`; decision: per-document agreement check, whole-file Tier B fallback):
      - [x] Amendment skeletons 64–67 and invariant 1 restated.
      - [x] Measured with the real images on `var/fix-pass7/scratch` (no `Document.text` from either indexer; javac reads an undeclared build as UTF-8; ISO-8859-1 builds shift non-ASCII lines only; scip-typescript strips the BOM; default imports carry the export's own symbol).
      - [x] Phase 1 (rb-scip): `Rejection` (`not_utf8`, `text_differs`, `comment_terminator` in the decoder; `source_encoding` from the Maven poms); TypeScript BOM on line 0; units.
      - [x] Phase 2 (rb-indexer): `definition_disagreement` in the join (calibrated to 0 on four repos + fixtures), rejected documents fall back whole-file, `documents_rejected(_reasons)` in coverage; `source_encoding` applied in `run_projects`; J4 name guard (import-site symbol, local names from the definition occurrence); `PIPELINE` `m3b-scip-3`; units, mutations M9/M14/M15/M22.
      - [x] Phase 3 (rb-cli, e2e): status line (`documents rejected`, old snapshots omit `definitions without occurrence`); goldens re-blessed by container runs; growth-ratio timing test.
      - [x] Phase 4: four public repos fresh == cached, edge sets == `82f34d4`; scratch J1–J4 cases; records; cleanup; commit.
      - [x] Resumed after an interruption: phases 1–3 were in the tree uncommitted; reviewed, `in_root`'s displaced doc comment restored, amendment 64's reason order corrected to the pipeline's, every mutation (M9, M14, M15, M22, the agreement check's clauses, quadratic `add_edges`) re-run and failing, scratch cases re-run with `--no-cache` on the real images, design §5.3/§13 updated.
    - [x] Evaluator pass 8 (test-first, one amendment each, 68–71; `tasks/eval-m3b-pass8.md`; decision: two structural checks, not more per-quirk rules):
      - [x] Amendment skeletons 68–71, invariant 1's reason list restated.
      - [x] Measured with the real images on `var/fix-pass8` (`esc`, `ann`, `ts`, `tsd`, `cfg`, `arg`, `mod`, `utf`): scip-java enclosing ranges on every definition kind except an implicit `<init>`, raw columns; scip-typescript partial and starting at decorators; javac's ISO-8859-1 columns are byte offsets in all three K2 layouts.
      - [x] Phase 1 (rb-scip): `encoding_ambiguous` (`Units::JavacLatin1`, both readings checked against the symbol), `Rejection` gains it and `structure_disagreement`; unit with the measured ranges.
      - [x] Phase 2 (rb-indexer): `Anchors` per document, `structure_disagreement` (Java only) before acceptance; `source_encoding` wins over `encoding_ambiguous`; K3 units (N24, N25, N27); `PIPELINE` `m3b-scip-4`.
      - [x] Mutations N24, N25, N27, S1–S7, E1–E6: each fails a unit.
      - [x] Verify: fmt, clippy, 318 unit tests, e2e 15/15 twice with containers (goldens unchanged), stub scan; five public repos fresh == cached, 0 rejections, dumps identical to `11ae499`'s apart from the header; scratch K1/K2 cases have no false exact edge.
      - [x] Records (contract 68–71 and corrections to 64, open items, CHANGELOG, IMPLEMENT.md, design §5.3/§13, lessons); cleanup; commit.
    - [x] Evaluator pass 9 (test-first; amendments 72–74; `tasks/eval-m3b-pass9.md`; decision: two-way definition agreement for Java documents, folded into `structure_disagreement`):
      - [x] Rebuilt the evaluator's layouts on the real image (`var/fix-pass9/x9esc` Hid1–3, 5–7, 9, 10; `x9lat` Wxyz, Abcd; `x9sjis` Sj3) and a forms project (`x9forms`); the `5502422` binary reproduces every L1–L4 false exact edge.
      - [x] Calibrated with a temporary trace (removed): unpaired SCIP definitions are implicit `<init>` (no range) and annotation forms only (jsoup 1, spring-retry 48, petclinic-rest 4); Tree-sitter-only definitions 0; syntax errors 0; 0 rejections after the annotation exemption.
      - [x] Phase 1 (rb-scip): tie rule `differing > 0 && iso >= utf8`; unit `a_tie_between_the_readings_is_ambiguous`.
      - [x] Phase 2 (rb-indexer): `definitions_unpaired` ((a), (b), syntax error), clause 2 over every method/type range; units for Hid1–10, each side alone, the annotation exemption, bound locals (S9, S10); two amended units; `PIPELINE` `m3b-scip-5`.
      - [x] Mutations A1, B1, X1, U1, C1, U1C1, R1, EX1–EX3, S8, S9, S10, E3, E3b, E3c: each fails a unit.
      - [x] Verify: fmt, clippy, 322 unit tests, e2e 15/15 twice with containers (48.07 s, 40.89 s; goldens unchanged), stub scan; six public repos fresh == cached, 0 rejections, dumps identical to `5502422`'s; scratch projects as recorded.
      - [x] Records (contract 72–74, corrections to 71, open items, CHANGELOG, IMPLEMENT.md, design §5.3/§13, lessons); cleanup; commit.
    - [x] Evaluator pass 10 (test-first; amendments 75–76; `tasks/eval-m3b-pass10.md`; decisions: kind-aware pairing (M1), syntax-error rule for TypeScript (M2), a type anchored by its type symbol (M3), compact constructors brought in):
      - [x] Rebuilt the evaluator's layouts on the real image (`var/fix-pass10/x10esc` Anon1, Anon2, Loc1, Fld1, plus Anon3, Init1, Enm1, Enm2, Fut, Rec, Rec2; `x10ts` conflict.ts); the `bf43351` binary emits every false exact edge (and three more: Anon3, Init1, Enm2).
      - [x] Measured with a probe (real decoder and extractor): jsoup's 117 bound locals are all kind `Method` inside a kindless enum constant of the Tree-sitter parent; the decision's literal rule (ii) would pass Anon3 and Init1, so (ii) is the enum-constant-body shape.
      - [x] Phase 1 (rb-indexer `scip.rs`, rb-syntax `queries.rs`/`lib.rs`, `PIPELINE` `m3b-scip-6`): failing units first (5), then the fix; 5 existing units given the measured constant and local kind.
      - [x] Mutations P1–P7, T1, S1, C1, B1, X1, S9, S10: each fails a unit.
      - [x] Verify: fmt, clippy, 328 unit tests, e2e 15/15 twice with containers, stub scan; eight public repos fresh == cached; six dumps byte-identical to `bf43351`'s; vavr and TheAlgorithms diffs confined to the four previously rejected files.
      - [x] Records (contract 75–76, corrections to 71, open items, CHANGELOG, IMPLEMENT.md, design §5.3/§13, lessons); cleanup; commit.
    - [x] Evaluator pass 11 (test-first; amendments 77–79; `tasks/eval-m3b-pass11.md`; decisions: `structural_escape` for N1 and the `*/` class, index-confirmed supertypes for N2, N3 doc, N4 units):
      - [x] Measure: structural-escape scan over the seven public Java repos (json-schema-validator's two `case '\u002E':` → `.`/`@`/`:` dropped, then 0); rebuilt `t11esc` (Enm3, Enm4, Enm7, Sup1, Plain, Chain) on the real image, the `2406e57` binary emits all six false exact edges; `t11ts` shows scip-typescript relates interfaces to nothing.
      - [x] Phase 1 (rb-scip `decode.rs`): `Rejection::StructuralEscape`, JLS backslash rule; unit first.
      - [x] Phase 2 (rb-indexer `scip.rs`, `lib.rs`; rb-cli `main.rs`): walk steps confirmed by the origin's relationships (deviation recorded in amendment 78), `supertypes_unconfirmed`; N2/N4 units first; `PIPELINE` `m3b-scip-7`.
      - [x] Mutations SE1–SE8, SC1–SC4, MK2, MK13: each fails a unit.
      - [x] Verify: fmt, clippy, 333 unit tests, e2e 15/15 twice, stub scan; nine public repos fresh == cached, rejections 0, `supertypes_unconfirmed` 0, edge sets identical to `2406e57`.
      - [x] Records (contract 77–79, amendment 71 table, invariant 1, CHANGELOG, IMPLEMENT.md, design §5.3/§13, lessons); cleanup; commit.
    - [x] Evaluator pass 12 (amendment 80; `tasks/eval-m3b-pass12.md`; O1: an escaped backslash is structural unless `u` follows; O2: the produced-backslash rule's unit):
      - [x] Phase 1 (rb-scip `decode.rs`, rb-indexer `lib.rs`): backslash clause with the `u` exemption; units `escaped_backslashes_reject_a_scip_java_document`, `a_produced_backslash_leaves_the_next_escape_eligible`; the "agree" unit corrected; `PIPELINE` `m3b-scip-8`.
      - [x] Mutations: clause removed, exemption removed, R1 on the pass-11 rule: each fails a unit; R1 on the new rule is equivalent (recorded).
      - [x] Real image `var/close-m3b/t12bs`: `Bs7`, `Bs4` rejected `structural_escape`, no false exact edge; seven Java repos `documents rejected 0`, counts unchanged, fresh == cached.
      - [x] Verify: fmt, clippy, 335 unit tests, e2e 15/15 twice (no `skipping:`), stub scan.
      - [x] Records (contract 80, amendments 71/77 in place, design §5.3, CHANGELOG); cleanup; commit.

- [x] 5. M3b closed 2026-10-06 (contract amendment 81: final status, review history, open items carried forward).

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

## Review (evaluator pass 6)
- All five findings fixed test-first (amendments 59–63). Line terminators were measured on both real images before any code: scip-java counts a lone CR, scip-typescript also U+2028/U+2029, Tree-sitter only `\n`; the decoder now converts each position through its byte offset, and a name guard on exact-span matches rejects an occurrence that does not name its token (0 rejections on four public repositories, the fixtures and the scratch project after the measured naming forms were enumerated).
- Found while measuring: a U+2029 inside a TypeScript `//` comment ends the comment for TypeScript but not for Tree-sitter (scip-typescript indexed the rest of the line as a class property); spring-retry's test sources are skipped by scip-java because a Develocity build cache serves `testCompile` (open item). The edge sets of all four public repositories are unchanged; only the coverage diagnostic gains two fields.

## Review (evaluator pass 7)
- All five findings fixed test-first (amendments 64–67). A document is SCIP-owned only when the indexer and Tree-sitter read it alike (`not_utf8`, `text_differs`, `comment_terminator`, `source_encoding`, `definition_disagreement`); otherwise the whole file is Tier B and counted. The J2 layout is rejected by `comment_terminator` and, with the decoder bypassed, by `definition_disagreement`. Calibration: 0 rejected documents on the four public repositories and the fixtures; edge sets identical to `82f34d4` on every edge kind.
- Found while resuming: amendment 64 said the reasons are tested in listed order, but the decoder's `comment_terminator` comes before `source_encoding` (applied after decoding); the amendment now states the pipeline's order. `text_differs` cannot fire with today's indexers (no `Document.text` emitted, measured) and stays as a guard.

## Review (evaluator pass 8)
- K1–K4 fixed test-first (amendments 68–71). The edge source is now checked against the index's own enclosing ranges (Java), and javac's encoding is decided from the positions when the build cannot be read; both calibrated to 0 rejections on five public repositories with edge sets unchanged.
- Found while measuring: scip-typescript's class and method ranges include decorators, which Tree-sitter-typescript places outside them, so the structure check is Java-only. The evaluator's K2 layout is genuinely ambiguous even in a UTF-8 build (its `é` run equals the distance between the two `z` calls), so `Utf.java` is rejected too; recorded.
- `field_chains_binding_chains_and_big_packages_stay_fast` (a wall-clock M3a test, 2 s/3 s bounds) failed in some mutation runs while a fresh build competed for CPU; it passed in every clean run. Not changed here (no defect found); a candidate for the growth-ratio treatment given to `many_declarations_of_one_name_stay_fast`.

## Review (evaluator pass 9)
- L1–L5 fixed test-first (amendments 72–74). A Java document is SCIP-owned only when its definitions agree both ways (every ranged method, constructor or type the index defines is a Tree-sitter definition, every Tree-sitter definition has an occurrence, no Tree-sitter syntax error), and the structure check's clause 2 sees every method or type range; `encoding_ambiguous` rejects a tie. Calibrated to 0 rejections on six public repositories with edge sets byte-identical to `5502422`.
- Found while measuring: scip-java marks every annotation interface by its relation to `java/lang/annotation/Annotation#`, which identifies the only legitimate ranged unpaired form without the source text; a record's compact constructor is a member Tree-sitter-java does not capture, so its calls had a wrong exact source before and the file is now Tier B (open item). The two-way rule makes `definitions_without_occurrence` 0 by construction for accepted Java documents; that counter now matters for TypeScript only.
- Session slip: a scratch-repository commit landed in the project repository through a relative `cd`; undone at once without touching the working tree (lesson recorded).


## Review (evaluator pass 10)
- M1–M3 fixed test-first and compact constructors brought in (amendments 75–76). A Tree-sitter definition is paired only by an occurrence of its own kind, or (a method) by an enum constant body's local method; a type is anchored by its type symbol, never its implicit constructor; a TypeScript document with a syntax error is rejected. Calibrated: 0 rejections on eight public repositories (vavr and TheAlgorithms 2 → 0), six dumps byte-identical to `bf43351`.
- Found while measuring: the decided rule (ii) ("innermost method/type range is the parent's") still paired an anonymous class's method in a field initializer and a local variable of an initializer block (both reproduced as false exact edges on the real image), so the binding was narrowed to the measured enum-constant-body shape (recorded as a deviation in amendment 75). scip-java leaves enum constants without a `SymbolInformation.kind`, which is what identifies them.

## Review (evaluator pass 11)
- N1–N4 fixed test-first (amendments 77–79). A scip-java document holding a Unicode escape of structural ASCII is rejected at the decoder (`structural_escape`), which closes the `*/` class of passes 8–11 at its input; the `IMPLEMENTS` walk steps only onto supertypes the origin type's symbol information relates it to. Calibrated: 0 rejections and 0 unconfirmed supertypes on nine public repositories, edge sets identical to `2406e57`.
- Found while measuring: json-schema-validator writes `case '\u002E':`, so `.`, `@`, `:` left the set as the decision prescribed; scip-typescript 0.4.0 relates an interface to no supertype, so the per-step confirmation the decision worded would have dropped true edges through TypeScript interfaces; the walk is confirmed by its origin's transitive relationships instead (recorded deviation).

## Review (evaluator pass 12 and closing)
- O1 and O2 fixed (amendment 80, `263a20f`): an escape decoding to a backslash is structural unless `u` follows it; the produced-backslash rule has its unit. Calibrated: 0 rejections on seven Java repositories, counts unchanged, fresh == cached; the closing binary reproduces all nine public repositories' numbers.
- Found while fixing: under the new clause the evaluator's mutant R1 is equivalent (a produced backslash reaches the parity count only before a `u`, which resets it), so its unit kills R1 only against the pass-11 rule; recorded rather than contorting the code. vavr writes the escape as upper-case `\u005C` followed by `u`, which the unit now covers.
- M3b closed without a pass-13 re-check (amendment 81 says so). Open items carried forward are listed there.

# M4 Frameworks (split into M4a and M4b; design §13)

- **M4a, endpoints and request paths:** contract `tasks/contract-m4a.md` (S1–S28). **Closed 2026-10-06** (contract amendment 18) after eight evaluator passes; pass 8 (`tasks/eval-m4a-pass8.md`, verification only) VERDICT PASS.
- **M4b, DI and dispatch** (Spring DI, Spring Data beans, RTA, entry points, orphans, Q3 roll-up, package versions and service dependencies, full Service Card): contract `tasks/contract-m4b.md` (S1–S28, soundness claims and closing bar up front, forks F1–F6) written 2026-10-06; built 2026-10-06 (phases 0–16). **Closed 2026-10-07** (contract amendment 22) after two evaluator passes; pass 2 (`tasks/eval-m4b-pass2.md`, verification only) VERDICT PASS. Of the M4a open items it takes 1, 5, 7, 9, 15 and 24; item 2 moves to M6.

## M4a phases

Each phase has at most 5 hand-edited files (blessed goldens are reviewed separately and listed in the changelog), is test-first, and ends with its verification step. The checks run in every phase are fmt, `clippy --workspace --all-targets -D warnings` and the unit tests of the crates touched; the e2e suite runs with `RB_TEST_DATABASE_URL`. Any golden that changes gets a classified line diff before `RB_BLESS=1`.

- [x] **0. Preflight** (no code). Done 2026-10-06: findings under the contract's "Checked in phase 0".
  - Read the contract, the lessons and M3b amendment 81.
  - Run `df`, `docker context show` and `docker image inspect` for both indexer images.
  - Confirm from primary docs: Spring 6's stereotype requirement for handler detection (D12); gin's route and group API, and Go 1.22 `ServeMux` patterns (S6).
  - Record the findings under "Verified facts" in the contract.
  - **Verify:** the contract states each finding with its source.
- [x] **1. Neutral facts** (S1, S2). Done 2026-10-06 (contract amendment 1).
  - *State:* `FileFacts.chains` (new), `Binding.{init, constant, decorations}`, `Decoration.span`, `DecorationArg.items`; all per blob in the facts cache. *Derived:* nothing yet (no graph change). *Invariants:* facts stay framework-neutral; every new file is fingerprinted (test); existing goldens unchanged. Files: `rb-syntax/src/lib.rs` (new fields, fingerprint list), new `rb-syntax/src/calls.rs` (call chains, argument summaries), `bindings.rs` (`init`, `constant`, `decorations`), `decorations.rs` (`span`), `strings.rs` (string templates with hole expressions, reusing the SQL decoding).
  - Tests per language; positions derived from the source text; one test that every extractor module is fingerprinted.
  - **Verify:** rb-syntax unit tests. The full e2e must pass **without re-blessing any golden**: these facts do not reach the graph yet. A golden diff here is a bug.
- [x] **2. Normaliser and route packs, pure functions** (S3–S6, the per-file parts). Done 2026-10-06 (contract amendment 2).
  - *State:* none (pure functions of `FileFacts`). *Derived:* per-file `FileRoutes` (routes with raw in-file paths, Express routers and mounts, route diagnostics); normalised paths and client URLs. *Invariants:* `norm_path` total and idempotent (property test); every framework gate is an import or a qualified name (negative test each); gin group walk has a visited set and a cap of 32 (cycle and 5,000-deep tests). Files: `rb-frameworks/src/lib.rs` (module declarations), new `rb-frameworks/src/endpoint.rs` (normaliser, matching order D9), new `rb-frameworks/src/routes.rs` (Spring, Express, gin, `net/http` route facts; Feign exclusion; `route_on_interface`).
  - Table tests per syntax and framework; a property test for idempotence; import-gated detection, each gate with a negative test.
  - **Verify:** rb-frameworks unit tests; e2e unchanged.
- [x] **3. Client packs and folding, pure functions** (S9–S12, the per-file parts). Done 2026-10-06 (contract amendment 3).
  - *State:* none. *Derived:* per-file `FileClients` (call sites with method, URL value, base, axios instance candidates); `Folded` values with an `Origin` label; parsed `application.properties`. *Invariants:* mutable bindings never fold; folding terminates by an on-path guard, depth cap 16 and a 10,000-step budget, and memoises only clean results; every client gate is an import or qualified name. Files: new `rb-frameworks/src/clients.rs` (`fetch`, axios, `RestTemplate`, `WebClient`/`RestClient`, Feign, Go `net/http`, `Sprintf`), new `rb-frameworks/src/fold.rs` (holes, constants, env defaults, `@Value`, placeholders; visited set and depth cap), `rb-frameworks/src/lib.rs`.
  - Cycle test (`A = B; B = A`); 1,000-link depth test; mutable bindings never fold.
  - **Verify:** rb-frameworks unit tests; e2e unchanged.
- [x] **4. Endpoints in the graph** (S4 properties, S5 cross-file mounts, S6 groups, S7, S8). Done 2026-10-06 (contract amendment 4).
  - *State:* `Content.config` (Spring config texts by path). *Derived:* `Endpoint` nodes, `EXPOSES`, `HANDLED_BY`, route diagnostics; context paths per service; exact handler targets from the join. *Invariants:* one node per (service, method, path) with one `EXPOSES`; edges merged per key before `add_edge`; mount composition guarded (on-path set, depth 32, 64 prefixes) and linear (growth test); handler names never resolved by bare name across the repository. Files: new `rb-indexer/src/routes.rs` (composition, aggregation per key, `EXPOSES`/`HANDLED_BY`, route diagnostics), `rb-indexer/src/lib.rs` (`Content.config` read by path, stage wiring, `PIPELINE = m4a-http-1`), `rb-indexer/src/resolve.rs` (expose the M3a lookup for handler names), `rb-indexer/src/scip.rs` (occurrence at a span, for exact handlers).
  - Unit tests: mount cycle, 5,000-deep chain, composition growth ratio, a same-named unrelated handler that must not be linked, aggregation of duplicate registrations.
  - **Verify:** unit tests, then e2e. Expected golden churn: new `Endpoint` nodes, `EXPOSES`, `HANDLED_BY` and route diagnostics in `dump.jsonl`, and the `tool_versions` pipeline. Classify each line, then bless.
- [x] **5. Stitching and coverage** (S12 across files, S13, S14). Done 2026-10-06 (contract amendment 5). Files: new `rb-indexer/src/stitch.rs` (the indexer's `Lookup`, sites, candidate index, D6 host rule, D9 specificity, certainty table, buckets, merge per edge), `rb-indexer/src/routes.rs` (return the endpoints it added), `rb-indexer/src/resolve.rs` (where an imported or qualified constant is declared), `rb-frameworks/src/fold.rs` (`Lookup::external` gets the hole's position, so a local variable shadows a package or type qualifier), `rb-indexer/src/lib.rs`, `rb-store/src/query.rs` (HTTP coverage), `rb-cli/src/main.rs` (`http:` line in `rb status`). Seven files: the endpoint list must come back from `routes.rs` (the builder cannot be iterated) and the position from `fold.rs`; each edit is a few lines.
  - *State:* none new (client facts are `FileFacts`; `application.properties` is `Content.config`). *Derived:* per client site its folded URL and bucket; `CONSUMES` edges (provenance `http_stitch`), `http_*` site diagnostics and one `http_coverage` count per snapshot; the `rb status` line at query time. *Invariants:* never `exact` (`synthetic` at most, and only by the S13 row with F1); certainty is the minimum of match kind, method, host, URL origin, the endpoint's path certainty; own-service and loopback sites are `self`, never linked; a literal host naming no service is `external`, never path-matched; > 16 candidates give no edge; every site is in exactly one bucket (sum test); candidate lookup is bucketed by first/last segment and verified by the reference matcher (`match_path`), so it agrees with D9 by construction (property test) and is linear on distinct prefixes (growth test); constant lookup across files goes through M3a resolution, unique declarations only.
  - Table-driven unit test per certainty row and per D6/D9 clause; a test that the buckets sum to the site count; stitcher growth ratio plus the pathological all-`{}` input.
  - **Verify:** unit tests, e2e (classified churn: `CONSUMES` edges, `http_*` diagnostics, `status-*.txt`).
- [x] **6. `rb path`** (S15–S19). Done 2026-10-06 (contract amendment 6). Files: new `rb-store/src/path.rs` (input resolution, downstream and upstream walks, collapse, caps, histogram, JSON model), `rb-store/src/lib.rs`, `rb-store/src/code.rs` (the graph `View` shared, not duplicated), new `rb-cli/src/path.rs` (text/JSON rendering, `--pick`), `rb-cli/src/main.rs` (subcommand), `rb-cli/Cargo.toml` (`rb-frameworks`, so `<METHOD> <path>` input is normalised by the same function as routes).
  - *State:* none (query time over the loaded snapshot graph). *Derived:* the endpoint candidates for an input (S15 tiers, first non-empty wins, sorted by key); a downstream tree per hop (handler roots, `CALLS` + CHA dispatch within the handler's service and library services, sinks `QUERIES → Table` and `CONSUMES → Endpoint` recursing as `⇒ <service>`, other deployable services shown as "crosses into … (not followed)", helpers collapsed unless `--full`); an upstream tree (client sites, their callers with CHA, continuation through a handled node or an inline handler's spans); the histogram and the limits hit. *Invariants:* every walk has a visited set (symbols per output, endpoints per direction), a per-hop depth cap (`--depth` 12, max 25), a hop cap (`--hops` 4, max 8) and a node cap (5,000), and `truncated` names each limit hit; children are ordered by step certainty (best first), then via, then natural key, so the output has a total order; inline handlers are followed only through edges whose evidence lies in their spans; nothing is picked silently among several endpoints.
  - Unit tests on built graphs: `CALLS` cycle, self-recursion, `CONSUMES` A ↔ B, 5,000-deep chain cut at `--depth`, hop cap, node cap with `truncated`, inline-handler span scoping, an upstream continuation through an inline handler; `rb path` growth ratio.
  - **Verify:** unit tests; e2e smoke on the current fixtures (goldens arrive in phase 8).
- [x] **7. Service Card API section and `rb impact` roll-up** (S20, S21). Done 2026-10-06 (contract amendment 7). Files: `rb-store/src/query.rs` (card sections at query time), `rb-store/src/code.rs` (endpoint roll-up), `rb-store/src/path.rs` (its `handler_spans` reader shared, not duplicated), `rb-cli/src/main.rs` (`render_card`), `rb-cli/src/code.rs` (impact rendering).
  - *State:* none. *Derived (query time):* the card's `APIs` (each `EXPOSES` endpoint with its `HANDLED_BY` handlers and certainty, `prefix unknown` flagged), `Calls APIs of` (the service's `CONSUMES`, grouped by target service, best certainty and sites per endpoint), `APIs called by` (`CONSUMES` into its endpoints, grouped by source service), and the count of its unstitched client sites (`http_*` site diagnostics in its files); the impact's `endpoints` (handled by a removed, breaking or affected node; distance one more than the handler's, certainty the minimum of the handler's and `HANDLED_BY`). *Invariants:* sections are ordered by natural key (bytewise), never by database collation; an inline handler (`handler_spans`) counts only when the edge that put it in the impacted set has evidence in its spans (invariant 8); certainty is never raised (minimum along the way, best per endpoint only among equal distances).
  - **Verify:** unit tests; e2e. Card and impact goldens re-blessed after a classified diff; the "not indexed yet (M4)" line is gone from all five goldens.
- [x] **8a. Fixture plan, part 1** (contract "Fixture plan"). Done 2026-10-06. Files: `polyglot-monorepo/services/auth/.../AuthController.java`, `services/notification/src/server.ts`, new `services/notification/src/selfcheck.ts`, `services/billing/cmd/billing/main.go`, `services/billing/go.mod`. `git add` them, since `fixture_repo` copies tracked files only.
  - *State:* the fixture's tracked source only (no code change). *Derived:* 4 new endpoints (auth `GET /api/v1/login`, notification `POST /internal/mfa/sms`, billing `POST /billing/invoices/{}/pay` and the same-path decoy `POST /internal/mfa/email`), `notifyPaid` → notification `synthetic`, selfcheck in bucket `self`. *Invariants:* commented routes give nothing; the GET endpoint and the billing decoy receive no existing client; every new line of every golden is predicted before blessing.
  - **Verify:** e2e; classify the churn.
- [x] **8b. Fixture plan, part 2.** Done 2026-10-06. Files: new `polyglot-monorepo/apps/web/src/api/{billing,mfa,http}.ts`, new `java-spring-jpa/src/main/resources/application.properties`. `git add` them.
  - *State:* fixture source only. *Derived:* web `payInvoice` → billing pay (`heuristic`, `match: suffix`), web `resend` → both `POST /internal/mfa/email` (`ambiguous`, `candidates: 2`), `http.ts` in buckets `dynamic` and `external`; the Java endpoint becomes `POST /auth/api/v1/login` (`synthetic` path, amendment 4). *Invariants:* the external URL's path equals auth's login path and is still not linked; every site lands in one bucket (8 sites: 2 synthetic, 2 heuristic, 1 ambiguous, 1 self, 1 external, 1 dynamic).
  - **Verify:** e2e; classify the churn.
- [x] **8c. Goldens and precision guards** (S22, S23, S25, S26). Done 2026-10-06 (contract amendment 8).
  - *State:* the guard files (data beside the goldens); two fixture lines (`LegacyAuth` mappings, notification `/health`). *Derived (query time):* `rb path` stitch notes (`stitch` in JSON). *Invariants:* every guard line is bitten by a recorded mutation; a must-not-exist source key must name a node (no vacuous line); guards and invariants 1, 3, 5, 12 are checked on every fixture dump in one collected assertion; AGE `Endpoint` vertices and 1-hop neighbours equal the relational ones. Files: `rb-cli/tests/e2e.rs` (path goldens, guard runner, dump invariants, AGE parity for the new kinds), plus new `must-exist.txt` and `must-not-exist.txt` for each of the three fixtures.
  - Mutations, each shown to fail a guard and then restored and checked with `cmp`: host restriction off, own-service exclusion off, Feign-route exclusion off, loopback rule off, external-host rule off.
  - **Verify:** e2e twice; determinism (two repos, forced rebuild, warm = cold).
- [x] **9. SCIP mode** (S24). Done 2026-10-06.
  - *State:* none. *Derived:* the SCIP-mode login path (golden). *Invariants:* the Java hop is `exact` only through scip-java's occurrences; the named Express handler is `exact` only through the join's exact-handler map (mutation shown); fresh = cached. Files: `rb-cli/tests/e2e.rs` (`scip-path-login.txt`), plus the re-blessed SCIP goldens.
  - Before the run: `df`, the images and the context (lessons 2026-10-01).
  - **Verify:** SCIP e2e with containers; no `skipping:`; fresh = cached.
- [x] **10. Public repositories** (S27). Done 2026-10-06 (contract amendment 9, fork F3, "Measurement").
  - *State:* none new. *Derived:* suffix candidates ranked by the matched tail's literals (amendment 9). *Invariants:* certainty unchanged by the ranking fix; every measured number computed by a script from the dumps; clones and `m4a_*` repositories removed. Clones went to `var/m4a/` (the orchestrator's instruction) rather than the scratchpad.
  - Clone the candidates into the scratchpad. Verify that their routes and clients match the contract's description; record any substitution with its reason. Pin the commits.
  - Run `--no-scip` fresh and cached, plus SCIP for spring-petclinic-microservices.
  - Record the counts and buckets (one member per non-zero bucket), the time and the facts-cache size against the M3b binary.
  - Sample 20 endpoints and the `CONSUMES` edges by the contract's rule and hand-check each against the source.
  - **Verify:** the numbers are recorded in the contract's measurement section, computed by a tool, not summed by hand.
- [x] **F3. Router identity in prefix-unknown endpoint keys** (orchestrator decision on fork F3). Done 2026-10-06 (contract amendment 10, `73a111b`).
  - *State:* none new (`RouterDecl.binding`, `RouteBase::Unknown(binding)` are per-file route facts). *Derived:* prefix-unknown keys `HTTP <M> <tail> @<service> ?router=<file>#<binding>` and the `router` attribute; `rb path` candidates name the router. *Invariants:* known-prefix keys byte-identical (unit); the identity holds no position (unit); invariant 1 restated. Files: `rb-frameworks/src/{routes,endpoint}.rs`, `rb-indexer/src/{routes,lib}.rs`, `rb-cli/src/path.rs`.
  - **Verify:** units test-first, two mutations; e2e with no golden change; gin realworld re-measured (22 endpoints, 20/20 sampled, fresh == cached).
- [ ] **11. Records and review** (S28).
  - [x] Records (2026-10-06): contract (amendment 10, the measurement table with the gin re-measure, **Open items**), design §8.2/§9.6/§10/§13 (M4a row: built, pending the evaluator), README (status, `rb path`, the `http:` line, card sections), IMPLEMENT.md, CHANGELOG, lessons.
  - [x] Full verification loop and the stub scan.
  - [ ] A separate evaluator pass; fix its findings test-first (each round adds contract amendments). M4a is not marked done until the evaluator decides.
  - Commit after each verified phase.

## Review (M4a evaluator pass 1)
- [x] Plan (orchestrator's decisions, contract amendment 11): P1 hosts past holes; P2 suffix evidence and own-service ranking; P3 opaque tails; P4 `rb path` scopes; P5 biting tests; P6 axios shadowing; P7 `config_unread`; P8 `ServeMux` hosts; P9 residue; P10 dead code; the two suspected items; honest open items; `PIPELINE = m4a-http-5`.
- [x] Baseline: `d2aee54` release binary built and the four public repositories dumped before any change.
- [x] P1–P3 (`rb-frameworks/src/endpoint.rs`, `clients.rs`) and the stitcher (`rb-indexer/src/stitch.rs`), test-first.
- [x] P8 route host, P7 configuration (`routes.rs` ×2, `lib.rs`, `fold.rs`), test-first.
- [x] P4 and suspected item 2 (`rb-store/src/path.rs`), test-first; P5 tests; both mutation batches (20 runs, all killed).
- [x] Public repositories re-measured and diffed (only the expected rows changed; piggymetrics' false edge gone); fresh == cached == second repository.
- [x] Records: contract, design §10, CHANGELOG, IMPLEMENT.md, lessons.
- [x] Verification loop (fmt, clippy, 437 unit tests, e2e twice, stub scan); clean-up of `fx4a1_*` and `var/fix-m4a-1/`.
- Found while fixing: the P4 drop also happens upstream (a file reached from two inline handlers); config *servers* first read as config clients; a hole in user information (`http://u:${p}@h`) would have been read as a port hole. Each fixed test-first.
- [x] Evaluator pass 2 (`tasks/eval-m4a-pass2.md`, VERDICT FAIL on `7d12001`): fixed below.

## Review (M4a evaluator pass 2)
- [x] Plan (orchestrator's decisions, contract amendment 12): Q1 bases stated at the call or on the instance; Q2 partial hosts; Q3 `config_unread` forms; Q4 counted growth tests; Q5 biting tests (M02, M21, M23); Q6 a hole after a literal port; Q7 called registrars in `rb path`; Q8 `node-fetch` and honest open items; `PIPELINE` `m4a-http-6`.
- [x] Facts (`rb-syntax`): spreads in object literals, Java `new` arguments (`Arg::New`), assignment targets (`CallChain.assigned`); unit first.
- [x] Q1, Q8 (`clients.rs`), Q2, Q6 (`endpoint.rs`), the stitcher (`stitch.rs`: instance override, external partial hosts, `base_from_bean` cap), test-first.
- [x] Q3 (`fold.rs::yaml_entries`, `lib.rs`), Q7 and M21 (`rb-store/src/path.rs`), M02/M23 rows, test-first.
- [x] Q4: `work::tick` counters (`rb-frameworks` with the `work-count` feature for rb-indexer's tests, `rb-store` `cfg(test)`); five growth tests counted; 30 runs of each of three crate suites under background load: 0 failures.
- [x] Mutations: 23 mutants (every new rule, the three Q5 survivors, four quadratic loops), all killed.
- [x] Public repositories against a `7d12001` build: four byte-identical, ftgo +4 `dynamic` sites; fresh == cached == second name; the evaluator's TS and Java repros end to end.
- [x] Records: contract (amendment 12, amendment-11 correction, invariant 11, open items 11, 13–20), CHANGELOG, IMPLEMENT.md, lessons.
- [x] Verification loop; clean-up of `fx4a2_*` and `var/fix-m4a-2/`.
- [x] Evaluator pass 3 (`tasks/eval-m4a-pass3.md`, VERDICT FAIL on `e06e590`): fixed below.

## Review (M4a evaluator pass 3)
- [x] Plan (orchestrator's decisions, contract amendment 13): Gate A (verified instance bases, R1), Gate B (a path starts with a literal, R2, R4), R3 base-document disable, R5 upstream registrar, R6 counted scans, R7 biting tests (C, G), R8 client forms and README, R9 open item; `PIPELINE` `m4a-http-7`.
- [x] Baseline: `e06e590` release binary built (own target directory) before any change.
- [x] Facts (`rb-syntax/src/uses.rs`): `NameUse` uses of candidate names; unit first.
- [x] Gate A in the packs (`clients.rs`: axios instances, inline instances, Spring bases, builders, imported names) and the stitcher (`stitch.rs`: importer taint, cap, `base: unverified`, coverage counts), test-first.
- [x] Gate B (`stitch::join`, `endpoint::client_url` port holes), R3 (`fold.rs` documents, `lib.rs`), R5 (`rb-store/src/path.rs`), R8, test-first; R6 `work::Counted` with mutants T and U killed.
- [x] Mutations: every new rule plus C and G (see CHANGELOG).
- [x] Public repositories against `e06e590`: no edge changed; fresh == cached == second name; the evaluator's shapes end to end in a scratch repository.
- [x] Records: contract (amendment 13, corrections in 12, invariant 11, open items 11, 14, 21–23), README, CHANGELOG, IMPLEMENT.md, lessons.
- [x] Verification loop; clean-up of `fx4a3_*` and `var/fix-m4a-3/`.
- Found while fixing: a namespace import used otherwise cannot be attributed to one export, so it unverifies every export of its module (recorded, open item 22); `resolve::tests::field_chains_binding_chains_and_big_packages_stay_fast` (an M3a wall-clock bound) fails under the machine's current load at `e06e590` and at this change alike (3.04 s and 3.1 s against 3 s).
- [x] Evaluator pass 4 (`tasks/eval-m4a-pass4.md`, VERDICT FAIL on `b75b6fc`): fixed below.

## Review (M4a evaluator pass 4)
- [x] Plan (orchestrator's decision, contract amendment 14): narrow Gate A to a conservative, scope-insensitive token scan (T1–T4), conditional documents (T5), open item 14 (T6), records (T7); `PIPELINE` `m4a-http-8`.
- [x] Baseline: `b75b6fc` release binary built from `git archive` in its own target directory before any change.
- [x] Facts (`rb-syntax/src/tokens.rs`, replacing `uses.rs`): `NameToken` per token of a candidate name, role declaration / receiver / other; unit first.
- [x] Gate A (`clients.rs` `Gate`, `plain_construction`, `chain_base` precedence; `stitch.rs` importer taint deleted), test-first: the new units failed on the `b75b6fc` tree.
- [x] T5 (`fold::conditional_document`), test-first.
- [x] Mutations: 16 mutants, one per rule condition, all killed (see CHANGELOG).
- [x] Public repositories against `b75b6fc`; fresh == cached == second name.
- [x] Records: contract (amendment 14, correction in 13, open items 14 and 22), CHANGELOG, IMPLEMENT.md, lessons (rewritten).
- [x] Verification loop; clean-up of `fx4a4_*` and `var/fix-m4a-4/`.
- Found while fixing: once the scan is name-based, several guards were redundant with the declaration role (a member check, a function-kind check on the scope, a Java/TS switch for top-level constants: none can fire without the role already failing), and excluding fields from the recorded names made the role's field rule unbitten (mutant M9 survived). All four were deleted, so the declaration role is the one guard that keeps fields, parameters, `var`s and exported bindings unverified, and M9 now kills it. An aliased import of another module's export with the instance's name (`import { api as shared }`) is a token of the name, so it unverifies the local `api`: by design (the scan is name-based).
- [x] Evaluator pass 5 (`tasks/eval-m4a-pass5.md`, VERDICT FAIL on `2a20746`): fixed below.

## Review (M4a evaluator pass 5)
- [x] Plan (orchestrator's decision, contract amendment 15): the gate covers every request — a `synthetic` stitch needs a provably plain carrier (verified instance, bare module client with no global hook, Feign, inline fresh construction with plain links), whatever the URL's source (U1, U2); axios transport keys are not plain (U3); `fresh_root` pinned in `chain_base` (M9); bk8 ruled; `PIPELINE` `m4a-http-9`.
- [x] Baseline: `2a20746` release binary built from `git archive` in its own target directory before any change.
- [x] Facts (`rb-syntax`): `TokenRole::Member`, tokens of module objects (TS/JS package default/namespace imports, Go imports) and JS global objects, `Init::New` for Java object creations; units first.
- [x] Carriers (`clients.rs`: `verdict`, `plain_config`, `SpringBases::carrier`, `plain_links`, `ClientCall.module_call`, `global_hooks`; `stitch.rs`: hooked module clients capped, imported instances never plain), test-first: `every_carrier_is_gated_whatever_the_url` failed on the `2a20746` tree (13 of 14 repro sites `synthetic`).
- [x] Mutations: 40 mutants, one per new condition plus M9, all killed (see CHANGELOG).
- [x] Public repositories against `2a20746`; fresh == cached; scratch repositories end to end.
- [x] Records: contract (amendment 15, corrections in 13 and 14, open item 22 with the M5 follow-up), CHANGELOG, IMPLEMENT.md, lessons.
- [x] Verification loop; clean-up of `fx4a5_*` and `var/fix-m4a-5/`.
- Found while fixing: a method-local `RestTemplate rt = new RestTemplate()` had no initialiser fact (a Java `new` is no call chain), so the scan could not see it and it would have been capped; `Init::New` makes it a verifiable carrier (it also stops a same-file `@Bean` base from being taken for such a local). Removing `mutate` from the request calls and judging the request chain's links each stop U1 alone, so each has its own biting row (`m1`, `l9`). On node-express-realworld the e2e setup's `axios.defaults.baseURL` is a real global hook: its one bare `axios.get` is now capped.
- [x] Evaluator pass 5 re-check: pass 6 (`tasks/eval-m4a-pass6.md`, VERDICT FAIL, V1–V5 blockers, V6 process).

## Review (M4a evaluator pass 6)
- [x] Plan (orchestrator's decision, contract amendment 16): replace every carrier deny-list by an allow-list of token roles (V1 Java fresh constructions, V2 Feign, V3 `fetch`, V4 axios, V5 Go), a tainted kind caps every site of the kind; fix the mutation harness (V6); write the closing bar; rewrite open item 22; `PIPELINE` `m4a-http-10`.
- [x] Baseline: `d4010fa` release binary built from `git archive` in its own target directory before any change.
- [x] Test first: `carriers_are_plain_only_by_allow_listed_roles` (rb-indexer) on the unchanged code: every V1–V5 repro `synthetic` (J 7/7, Feign 3/3 forms, `fetch` 4/4 hooks, axios instances under `defaults` and a function-local `require`, Go dot import).
- [x] Facts (`rb-syntax`): surface names (`fetch`; `DefaultTransport`, `DefaultClient`; Feign extension points and `feign`) recorded as property, field and type names too, judged as the member access they name; `TokenRole::Callee`; `Init::New.body`; global-object and Go import tokens deleted.
- [x] Carriers (`clients.rs`): `fresh_root`/`url_arg` allow-list, `PLAIN_LINKS` without `create`/`builder`, `global_hooks` by allow-list for all four kinds plus Feign, `feign_config_key`, `module_names` at any scope and subpaths; `ClientCall.module_call` deleted. Stitcher: a hooked kind caps every site; Feign keys in any `.properties`/`.yml`/`.yaml` file (`lib.rs`).
- [x] Beyond the decision, same principle: Feign's other extension points (`Capability`, `FeignBuilderCustomizer`, `Targeter`, `Feign`, `feign.Client` and any non-plain Feign/OpenFeign import), `defaultConfiguration` on `@EnableFeignClients`, axios/fetch module subpaths.
- [x] Mutations (V6): harness restores with a fresh mtime and checks bytes; full list of passes 5 and 6 plus amendment 16 on a clean build: 63/63 killed on the final code (CHANGELOG). The first run's two survivors were fixed first: P29 (a redundant check in `SpringBases::carrier`, deleted) and N02 (a builder of another type, row added).
- [x] Public repositories against `d4010fa`; fresh == cached; scratch repositories end to end (12, both binaries).
- [x] Records: contract (amendment 16, corrections in 14 and 15, items 14 and 22), CHANGELOG, IMPLEMENT.md, lessons (2 new).
- [x] Verification loop; clean-up of `fx4a6_*` and `var/fix-m4a-6/`.
- Found while fixing: ftgo's integration-test client hands `node-fetch` to Apollo as `{ fetch }`, which the allow-list counts as a taint (4 `dynamic` sites capped); the harness kill (lesson) — SIGTERM and a pattern that did not match left a mutant applied for a minute, restored and checked before any further edit.
- [x] Evaluator pass 6 re-check: pass 7 (`tasks/eval-m4a-pass7.md`, VERDICT FAIL, W1–W4 blockers, W5 minor, W6 doc).

## Review (M4a evaluator pass 7)
- [x] Plan (orchestrator's decision, contract amendment 17): raw-text taint for every file without token facts (W1/W2), axios binding forms and a string-literal backstop (W3, W6's `"fetch"` literals), discovery configuration caps host-by-name `synthetic` (W4), a `require` subpath row (W5), item 22's residue list (W6), the closing decision; `PIPELINE` `m4a-http-11`.
- [x] Baseline: `95e1fbb` release binary built from `git archive` in its own target directory before any change.
- [x] Test first: rb-frameworks `global_hooks` literal rows and raw-scan rows, rb-indexer `unparsed_files_and_discovery_cap_carriers` on the unchanged code.
- [x] Facts (`rb-syntax`): string literals `"fetch"`, `"axios"`, `"axios/…"` as `TokenRole::Literal` tokens.
- [x] Carriers (`clients.rs`): literal backstop, `raw_scan`, discovery keys; stitcher: unparsed hooks, discovery cap.
- [x] Mutations from a clean build; public repositories against `95e1fbb`; raw-scan cost; scratch repros end to end.
- [x] Records: contract (amendment 17, item 22, closing decision), CHANGELOG, IMPLEMENT.md, lessons.
- [x] Verification loop; clean-up of `fx4a7_*` and `var/fix-m4a-7/`.
- Found while fixing: the decided "any text file" scan would have capped the fixture's own hops (a `package.json` dependency, a `pom.xml` comment) and every axios repository (lock files), so inert classes were added with rows; the first mutation run showed the W5 hook row masked by the new literal backstop (clean row added) and a redundant language guard (deleted); Spring binds environment variables, so discovery words are matched with `_` read as `.`; an e2e attempt timed out on cypher under load with 87 scratch graphs in the database (dropped; two clean runs after).
- [x] Evaluator pass 7 re-check: pass 8 (`tasks/eval-m4a-pass8.md`, verification only, VERDICT PASS; X1 minor test gap carried as open item 24).

## M4a closing
- [x] Evaluator pass 8 (verification only): W1–W6 repros and controls end to end, inert-text exemptions, 19 mutants from a clean build (18 killed; X1), seven public repositories byte-identical to `95e1fbb`, regression suite, invariant 13, docs vs code.
- [x] M4a closed 2026-10-06 (contract amendment 18: final S1–S28/invariant table, review history of passes 1–8, closing measurement, open items by milestone with the M5 adversarial carrier review list); design §13, README, IMPLEMENT.md, CHANGELOG updated.
- [x] M4b: contract written (`tasks/contract-m4b.md`, 2026-10-06); phases below.

## M4b phases (contract `tasks/contract-m4b.md`)

Same rules as M4a:
- Each phase has at most 5 hand-edited files. Blessed goldens and guard data are reviewed separately and listed in the changelog.
- Each phase is test-first: the new units fail, or do not compile, before the change.
- Each phase ends with its verification step.
- Every phase runs fmt, `clippy --workspace --all-targets -D warnings` and the touched crates' unit tests, counted from a log holding both streams.
- The e2e suite runs with `RB_TEST_DATABASE_URL`.
- Any golden that changes gets a classified line diff before `RB_BLESS=1`.
- Mutation runs follow M4a's harness rules: fresh mtime, restored bytes checked, a `test result:` line per mutant, stopped with SIGINT.

`PIPELINE` becomes `m4b-di-1` at the first output change and is bumped on each later one.

- [x] **0. Preflight** (no code). Done 2026-10-06: contract "Checked in phase 0" and amendments 1–3 (Lombok with a declared constructor; F2's `plain`/`capped` buckets; gate D main sources only); pins checked out under `var/m4b/`; invariant-1 baseline binary `var/m4b/bin/rb-1f81c8f` and fixture dumps `var/m4b/base/`.
  - Read the contract (soundness section first), the lessons, and M4a amendments 14–18.
  - Run `df`, `docker context show`, and `docker image inspect` for both indexer images.
  - Confirm from primary docs the "not verified" list:
    - D9's Spring rules: implicit single-constructor injection, `@Autowired` constructors, `@Resource` by name, `@Qualifier`/`@Primary`/name fallback and `-parameters`, `Introspector.decapitalize`, wrappers;
    - Spring Data fragment naming;
    - `@EnableFeignClients` default packages;
    - Boot's JPA repository base package;
    - Lombok's generated constructors;
    - the npm v2/v3 lockfile layout for workspaces.
  - Clone the S27 candidates, check the layout of each by reading, and pin commits.
  - Build the M4a closing binary (`28e47ba`, own target directory) and dump the three fixtures and the S27 repositories. These are the invariant-1 baseline and S27's comparison.
  - Run S6's and S9's matchers as a throwaway script over the fixtures and the public repositories, and read every hit (lesson 2026-10-06, "measure a conservative fallback on the fixtures"). Record the expected cost, or amend a list with its reason.
  - Check the `.m2` cache for `spring-context` and `spring-boot-autoconfigure` (the new fixture classes).
  - **Verify:** the contract gains "Checked in phase 0" with each finding and its source; any deviation from a decision is written as an amendment before code.
- [x] **1a. X1 and parameter, type and modifier facts** (S1). Done: X1's HTML row kills mutant B (HTML stripped like XML: the row fails, restored bytes compared); `Binding.parameter`/`type_args`/`array` (amendment 4), `Definition.return_type`/`modifiers`; 8 new units (failed to compile first); e2e 17/17 with no golden change.
  - *State:* `Binding.parameter`, `Binding.type_args`, `Definition.return_type`, `Definition.modifiers`.
  - *Derived:* none.
  - *Invariants:* facts stay neutral; fingerprint test; no golden change.
  - Files: `rb-frameworks/src/clients.rs` (the open-item-24 row only, shown killing mutant B), `rb-syntax/src/lib.rs`, `bindings.rs`, `decorations.rs`.
  - **Verify:** units per language (positions from `src.find`); e2e passes with **no** re-bless.
- [x] **1b. Default exports and mentions** (S1). Done: `FileFacts.default_export`, `FileFacts.mentions` (`MentionRole`, 12 roles; amendment 5: aliased specifiers and JS heritage); 5 new units seen failing first (stub; `default_export` disabled); e2e 17/17, no golden change.
  - *State:* `FileFacts.default_export` and `FileFacts.mentions` (roles per name; no spans).
  - *Invariants:* one unit row per role, plus `getLogger(X.class)`, `@Import(X.class)`, a qualified name and a string; fingerprinted.
  - Files: `rb-syntax/src/lib.rs`, `imports.rs`, new `rb-syntax/src/mentions.rs`.
  - **Verify:** units; e2e unchanged.
- [x] **2. Spring facts, pure** (S2, S3). Done: `rb_frameworks::di` (beans, points, scans, ambiguous constructors, unread classes, gate D findings in facts and in text); `Binding.modifiers`/`initialised` (amendment 6); 11 + 1 new units, seen failing first (empty `di`; missing fields); e2e unchanged.
  - *Derived:* per file: beans (stereotypes, `@Bean` methods with return types, Spring Data incl. store bases, Feign), bean names, qualifiers, `@Primary`, conditionals, annotations outside the inert list, scan and enable roots, `scan_unread`, injection points (forms, Lombok, wrappers, exclusions), and gate D's surface findings for the file.
  - *Invariants:* every Spring name is import-gated (negative test each); decoys give nothing.
  - Files: new `rb-frameworks/src/di.rs`, `rb-frameworks/src/lib.rs`.
  - **Verify:** rb-frameworks units; e2e unchanged.
- [x] **3. Manifest packages and `via=package`** (S15, S16). Done: `rb-indexer/src/packages.rs` (npm with lockfile walk, go.mod, pom; Service → Service; classpath closure); `PIPELINE = m4b-di-1`; card packages deduped (amendment 7); 6 units; fixture dumps vs the `1f81c8f` baseline: only added `Package` nodes and `manifest` `DEPENDS_ON` lines; six goldens re-blessed after classification.
  - *Derived:* direct dependencies with versions (lockfile walk, go.mod, pom literal or property); `Package` nodes in M3b's key form (`@?`); `DEPENDS_ON` with provenance `manifest`, evidence at the entry; Service → Service `via=package`; `lockfile_*` diagnostics.
  - *Invariants:* M3b's `scip` package edges unchanged; classpath closure has a cycle test.
  - Files: new `rb-indexer/src/packages.rs`, `rb-indexer/src/lib.rs` (wiring, `PIPELINE = m4b-di-1`).
  - **Verify:** units; e2e churn (manifest edges on all three fixtures) classified line by line, then blessed.
- [x] **4a. DI resolution in the indexer** (S4–S8). Done: `rb-indexer/src/di.rs` (candidates, roots, classpath, narrowing, gates D and M, `INJECTS` capped at `heuristic`, `di_*`, `di_coverage`), resolver accessors, `RawScan` gate inputs; `PIPELINE = m4b-di-2`; amendment 8; 8 + 1 units, 28 mutants all killed; fixture dumps only add lines vs `1f81c8f`.
- [x] **4b. `di:` status line** (S7): `Store::di_coverage`, `rb status` `di:` line, `polyglot-monorepo/status-di.txt` (`di: 1 injection points: 1 unresolved`; the member is `AuthController`'s `MfaClient` point, `di_unresolved` `no_bean`).
- (original phase 4 entry, split as above) **4. DI resolution in the indexer** (S4–S8).
  - *Derived:* candidates per point; narrowing; gates D and M; `INJECTS` with `capped_by`; `di_*` diagnostics; `di_coverage`; the `di:` status line.
  - *Invariants:* never `exact`; the minimum over resolution, path and rule; merge per edge; buckets sum (unit); each gate has its own biting row; counted growth test.
  - Files: new `rb-indexer/src/di.rs`, `rb-indexer/src/lib.rs`, `rb-indexer/src/resolve.rs` (type resolution exposed), `rb-store/src/query.rs` (coverage read), `rb-cli/src/main.rs` (status line).
  - **Verify:** units including S5's table and the S6 mutations; e2e churn classified (current fixtures: `java-spring-jpa`'s two points capped by `scan_roots_unknown`; polyglot `MfaClient` `unresolved` until `@EnableFeignClients` arrives in 12c).
- [x] **5a. RTA proof at index time** (S9). Done: `rb-indexer/src/rta.rs`, `GraphBuilder::{edge_sources, set_edge_attr}`, fragment names from DI; `PIPELINE = m4b-di-3`; amendment 9; 5 + 1 units, 20 mutants all killed; fixture dumps only add `rta_coverage` (no type proven before the fixture phases; java-spring-jpa's one considered type `DefaultAuthService` kept by `annotation`).
- [x] **5b. `rta:` status line** (S9): `Store::rta_coverage`, `rb status` `rta:` line, `polyglot-monorepo/status-rta.txt`. Invariant 1 on two public repositories (spring-petclinic, routing-controllers, `--no-scip`, `1f81c8f` vs this binary): 0 lines removed or changed; added lines are only `Package`/`manifest`, `INJECTS`, `di_coverage`, `rta_coverage`.
- (original phase 5 entry, split as above) **5. RTA proof at index time** (S9).
  - *Derived:* proven types; `impl_instantiated: false` on their `IMPLEMENTS` edges; `rta_coverage`; the `rta:` line.
  - *Invariants:* the edge set and certainties are unchanged (dump diff against the phase-0 baseline shows attributes only); one keeping row per rule; subtype propagation tested at depth 1/2 and in reverse; a cycle test; counted growth test.
  - Files: new `rb-indexer/src/rta.rs`, `rb-indexer/src/lib.rs`, the `IMPLEMENTS` producers' attribute hook (`resolve.rs`/`scip.rs`, or one `GraphBuilder` call in `rb-model/src/graph.rs`: D4, the generator's choice recorded), `rb-cli/src/main.rs`.
  - **Verify:** units; e2e churn classified (expected: no fixture type proven before the fixture phases, so dumps change only in the new coverage row).
- [x] **6. Precision at query time** (S10). Done: `rb_store::code::Precision`, `View::dispatches` (skips and records an `IMPLEMENTS` step with `impl_instantiated: false` under `rta`), used by `overridden` (Q1, Q2, upstream Q4) and `path::Walk::implementations` (downstream Q4); `CallerTree`/`Impact`/`RequestPath` carry `precision` and `pruned_by_rta`; `--precision` on `rb callers` (default `rta`), `rb impact` (`cha`), `rb path` (`rta`); `precision=<p>` header and the `rta: <k> dispatch step(s) pruned …` line; `rb path --format json` gains `precision`, `pruned_by_rta`. `rb table-deps` gets its flag with its caller walk in phase 9 (no dispatch is expanded there before; a flag that changes nothing would be a stub). 3 units first (did not compile), 6 mutants killed; 21 goldens gained only the header (classified).
  - *Derived (query time):* filtered dispatch steps; `pruned_by_rta`; the `precision=` header.
  - *Invariants:* the defaults (Q1/Q4 `rta`, Q2/Q3 `cha`); every pruned step counted; same graph under both modes in one unit.
  - Files: `rb-store/src/code.rs`, `rb-store/src/path.rs`, `rb-cli/src/main.rs` (flags), `rb-cli/src/code.rs`, `rb-cli/src/path.rs`.
  - **Verify:** units; e2e (headers change in callers and path goldens: classified, blessed).
- [x] **7. Entry kinds and orphan eligibility at index time** (S11 stored kinds, S12). Done: `rb-indexer/src/liveness.rs` (entry kinds import-gated; rules (a)–(f) plus `default_export`; the uncached whole-word scan of every non-binary file streamed blob by blob in `index_commit`), `GraphBuilder::{set_node_attr, inbound}`; `PIPELINE = m4b-di-4`; amendment 10; 4 + 1 units, 35 mutants all killed (5 survivors of the first run given rows); fixture dumps vs `1f81c8f`: only `Symbol` lines gaining `entry`/`orphan_eligible` (2/6/1) and the added lines of phases 3–7. Files: `liveness.rs`, `lib.rs`, `rb-model/src/graph.rs` (the builder methods replace the planned `syntax.rs` edit: eligibility needs the edges, so attributes are set after every producer).
  - *Derived:* `Symbol` attributes `entry` and `orphan_eligible`; `orphan_coverage`; the uncached word scan.
  - *Invariants:* import-gated kinds with decoys; one keeping row per eligibility rule; scan cost measured on the public repositories in phase 15; counted growth test.
  - Files: new `rb-indexer/src/liveness.rs`, `rb-indexer/src/syntax.rs` (attributes), `rb-indexer/src/lib.rs`.
  - **Verify:** units; e2e churn (symbol attributes) classified.
- [x] **8a. `rb impact`: orphans, tests, item 7, `--json`** (S13). Done: `rb_store::code::{Orphans, Orphan, entry_kinds}`, the orphan walk (candidates BFS, liveness BFS, both with visited sets), `tests`, endpoints out of `breaks`; `rb impact --json`; `rb_model::is_test_path` (moved from rb-indexer); amendment 11; 5 + 1 units, 15 mutants killed (one survivor given a row). Files: `rb-store/src/code.rs`, `rb-cli/src/{code,main}.rs`, `rb-model/src/lib.rs`, `rb-indexer/src/di.rs`.
- [x] **8b. Entry marks and routers in the other answers** (S11 query side, S13 item 7): `(entry: …)` in `rb callers` (stored kinds, `endpoint <name>`, `top-level`), the router in the `rb path` header and the card's APIs. Done: `Caller.entry` from `View::entry_marks` (stored kinds; `top-level` and `test` for files; `endpoint <name>`, an inline handler's only for a step inside its spans), `code::endpoint_display`; `rb path` header and JSON `router`; `Api.router` and the card line; 1 + 2 units extended, 8 mutants killed. `rb callers` has no JSON output, so S11's "JSON gains `entry`" has nothing to extend yet (recorded; amendment 11's phase split). Files: `rb-store/src/{code,query}.rs`, `rb-cli/src/{code,path,main}.rs`.
- (original phase 8 entry, split as above) **8. Entry marks, orphans, tests and item 7 in the questions** (S11 query side, S13).
  - *Derived (query time):* `(entry: …)` marks; the orphan reachability; `tests (n)`; endpoints listed once; the router shown for prefix-unknown endpoints; `rb impact --json`.
  - *Invariants:* the greatest-fixpoint formulation (dead cycle orphaned; a cycle with an outside caller live); dispatch liveness; library services not computed; 5,000-deep chain; counted growth.
  - Files: `rb-store/src/code.rs`, `rb-store/src/path.rs` (header), `rb-cli/src/code.rs`, `rb-cli/src/main.rs`, `rb-cli/src/path.rs`.
  - **Verify:** units; e2e (impact and callers goldens re-blessed after classification).
- [x] **9. Q3 roll-up** (S14). Done: `rb-store/src/rollup.rs` (caller BFS with depth 25 and node cap, label propagation, per-service roll-up, API hops through `path::upstream_of`), `Store::table_deps` with `RollupOptions`, `rb table-deps --precision/--hops`, the text sections and JSON fields; amendment 12; 4 units, 12 mutants killed (four survivors of the first runs given rows or a ticking variant). Files: `rb-store/src/{rollup,lib,query,path,code}.rs` (code.rs: two visibility changes and `View::graph`), `rb-cli/src/main.rs`.
  - *Derived (query time):* callers up to entry points; per-service ops, direct or indirect, endpoints (span-scoped), entry kinds; "through APIs" up to `--hops`; JSON `services`, `through_apis`.
  - *Invariants:* default `cha`; visited sets on nodes and endpoints; `CONSUMES` A ↔ B cycle test; counted growth.
  - Files: `rb-store/src/query.rs`, `rb-store/src/code.rs` (shared walk), `rb-store/src/path.rs` (upstream continuation reused, not duplicated), `rb-cli/src/main.rs`.
  - **Verify:** units; e2e (`table-deps-*` re-blessed; the "arrive with call resolution (M3)" line gone).
- [x] **10. Service dependencies and the full card** (S17, S18). Done: `rb-store/src/card.rs` (`card_sections`), `ServiceCard.derived`, `render_card`'s derived sections; amendment 13; 2 units (+ the card unit extended), 15 mutants killed (three survivors given rows; a redundant self filter deleted as equivalent). Files: `rb-store/src/{card,lib,query}.rs`, `rb-cli/src/main.rs`, `rb-cli/tests/e2e.rs` (the failed service's package line).
  - *Derived (query time):* `via=calls`/`via=http`/`via=package`; depended on by; shares tables; tables; call-graph summary; entry points by kind; JSON fields.
  - *Invariants:* bytewise order; a self-call excluded; two vias between one pair both shown.
  - Files: `rb-store/src/query.rs`, `rb-cli/src/main.rs` (`render_card`).
  - **Verify:** units; e2e (every card re-blessed after classification).
- [x] **11a. M4a items 1 and 9** (S19, S20). Done: default-import mounts through `default_export` (`rb-indexer/src/routes.rs`), catch-alls only for named hosts with `catch_all_only` (`stitch.rs`); `PIPELINE = m4b-di-5`; amendment 14 (S19 measured on node-express-realworld: 0 of 19 resolved, layout recorded as an open item); 2 units + 1 changed row, 7 mutants killed (one equivalent check deleted). Invariant 1: fixture dumps unchanged apart from the phase-7 lines; node-express-realworld vs `1f81c8f`: additions and the six `Symbol` attribute lines only.
- [x] **11b. M4a item 15** (S21): DI-resolved client bases. Done: `ClientCall.injected`, `clients::bean_base` (rb-frameworks), `di::Points` (per-owner points, unique beans), `stitch::injected_base`; `PIPELINE = m4b-di-6`; amendment 15; 1 new unit with five rows + the pack units updated, 10 mutants killed (two survivors given a row). Invariant 1: fixture dumps identical to 11a's; petclinic-microservices and piggymetrics `CONSUMES` unchanged vs `1f81c8f`. Files: `rb-frameworks/src/clients.rs`, `rb-indexer/src/{di,stitch,lib}.rs`.
- (original phase 11 entry, split as above) **11. M4a items 1, 9, 15** (S19–S21).
  - *Derived:* default-import mounts composed; catch-all candidates restricted to named hosts; DI-resolved client bases (`base_from_bean`).
  - *Invariants:* each can only lower or remove (`CONSUMES` diff against the baseline classified); M4a's `synthetic` bar untouched (the existing never-`synthetic` guards still green).
  - Files: `rb-frameworks/src/routes.rs`, `rb-indexer/src/routes.rs`, `rb-indexer/src/stitch.rs`, `rb-indexer/src/di.rs` (base lookup for S21).
  - **Verify:** units; e2e unchanged until the fixture phase.
- [x] **12a. Fixtures: java-spring-jpa, part 1.** New `AuthApplication.java`, `Hashing.java`, `Tokens.java`, `AuditConfig.java`, `Notifiers.java`; `git add`. Done: package-private types and `Notifier.java` (amendment 16); `INJECTS` lose `scan_roots_unknown` (`plain 2`), `KafkaAuditSink` proven (2 prunable edges), `main`/`lifecycle` entries; SCIP card and status re-blessed through container runs.
  - **Verify:** e2e; predict every new `INJECTS`, `impl_instantiated` and coverage line before blessing.
- [x] **12b. Fixtures: java-spring-jpa, part 2.** New `UserRepositoryCustomImpl.java`, `support/Service.java`, `support/ReportNotifier.java`; edits to `UserRepository.java` and `AuthController.java`. Done: qualifier, collection (capped by the decoy, `annotation_unread`), conditional ambiguous setter edges; fragment kept by RTA; eight goldens moved by the line shift, SCIP card and status re-blessed.
  - **Verify:** as 12a.
- [x] **12c. Fixtures: java-spring-jpa, part 3, and polyglot auth.** Edits to `DefaultAuthService.java` and `LegacyAuth.java`; polyglot new `AuthApplication.java`; edits to `UserDao.java` and `AuthController.java`. Done: `@Primary`, `@Bean` and name-fallback points; `LegacyDigest` helper for the orphan case (amendment 16); Feign and `@Repository` beans in polyglot; `rb path` login gains both `QUERIES`; 26 goldens classified.
  - **Verify:** as 12a; the `rb path` login goldens gain `QUERIES public.users`/`audit.users` (classified).
- [x] **12d. Fixtures: polyglot billing and notification, ts-express-prisma.** Edits to `services/billing/internal/invoice.go` and `services/notification/src/selfcheck.ts`; new `src/routes/health.ts`; edit to `src/app.ts`. Done: shared `public.users` (coupling, Q3 second service), `smsCheck` bucket `self`, `GET /ops/health` known prefix `heuristic`.
  - **Verify:** as 12a.
- [x] **13. Goldens and precision guards** (S22, S24, S26). Done: guard lines on the three fixtures, `m4b_invariant_failures`, labelled AGE parity, producer coverage, `spring_di_rta_and_orphan_goldens` (7 new goldens); 27 code + 5 other mutants killed; S22: own-service bitten by `smsCheck`, the dynamic check not equivalent to P2 (unit row); invariant 1 by `inv1.py` (amendment 17).
  - *State:* the guard files (data beside the goldens).
  - *Invariants:* every guard line and dump check bitten by a recorded mutation (item 5's own-service mutation included; the dynamic-URL check settled as equivalent or bitten, with the reasoning); producer coverage for `INJECTS` and both `DEPENDS_ON` forms; the invariant-1 check against the phase-0 baseline; AGE parity for `INJECTS` and Service → Service `DEPENDS_ON`.
  - Files: `rb-cli/tests/e2e.rs`, plus the six guard files.
  - **Verify:** e2e twice; determinism (two repos, forced rebuild, warm = cold).
- [x] **14. SCIP mode** (S25). Done: `scip-injects.txt` from a container run (F2: the plain rows `heuristic` with `resolution`, the issuers `ambiguous`/`conditional`), equal to the `--no-scip` injections; SCIP tests twice (amendment 18).
  - Before the run: `df`, the images and the context.
  - *Derived:* `scip-injects.txt` (the `synthetic` row end to end).
  - *Invariants:* no `skipping:`; fresh = cached; every SCIP golden diff classified.
  - Files: `rb-cli/tests/e2e.rs`.
  - **Verify:** SCIP e2e with containers, twice.
- [x] **15. Public repositories** (S27). Done: eight repositories (realworld-spring added for Lombok, amendment 20), counts by script, every sample read; amendment 19 fixes the one false `heuristic` `INJECTS` found (`m4b-di-7`); invariant 1 and determinism hold; results in the contract's measurement section.
  - Clone into the scratchpad (or `var/m4b/` if the orchestrator says so).
  - Run `--no-scip` fresh and cached; run SCIP for (a) and (b).
  - Compute every count with a script from the dumps; identify one member per non-zero bucket.
  - Diff against the phase-0 baseline dumps and classify.
  - Hand-check the samples (`INJECTS` 20, all proven types or 20, 10 `rb impact` orphan runs, 20 package edges, 2 tables per Java repository).
  - Record the wall time and facts-cache size against `28e47ba`.
  - **Verify:** numbers recorded in the contract's measurement section, computed by a tool; clones and `m4b_*` repositories removed afterwards.
- [x] **16. Records and review** (S28).
  - [x] Records: contract amendments (16–20), measurement and open items (15, with milestones); design §5.5/§6.2/§6.4/§8.5/§9.2–§9.5/§10.3/§13 (M4b: built, pending the evaluator); README (status, `--precision`, the `di:`/`rta:` lines, orphans, Q3's roll-up, the card's sections); IMPLEMENT.md; CHANGELOG; lessons.
  - [x] Full verification loop and the stub scan.
  - [x] Evaluator pass 1 (full review, `tasks/eval-m4b-pass1.md`, VERDICT FAIL on `531e14e`: Y1–Y3 blockers, Y4–Y10 major, Y11–Y14 minor, Y15 doc); fixed test-first as amendment 21, below.
  - [x] Evaluator pass 2 (verification only, per the contract's closing bar); new attack forms in residue classes go to M5's adversarial review, not the verdict.
  - Commit after each verified phase.

## Review (M4b evaluator pass 1)
- [x] RTA opt-in (D5, S10): every question defaults to `cha`; `--precision rta` says `(opt-in, may hide callers reached through forms the scan cannot see)`; 12 goldens change only their header, `callers-kafka-record.txt` passes `--precision rta`; the default equals `cha` (e2e row).
- [x] Y1 binary names (`$` segments in string and text words), Y14 every non-binary file for S9 (d), Y2 supertype role only for definitions (local and expression classes `other`), Y7 TS/JS module objects (new fact `module_objects`), Y12 unconsidered instantiated subtype row.
- [x] Y3 orphan rule (e) transitive through repository types (visited set, depth 64).
- [x] Y4 gate M over every assignable class and `@Bean`-declaring class; Y5/Y6/Y13 no `@Primary`/name pick for generic (`nested_type_args`) or annotated points, `@Named` as qualifier and stereotype, two primaries ambiguous.
- [x] Y8/Y11 Maven duplicates and duplicate properties → `@?` `heuristic` with `versions`; Y9 npm aliases keyed by the real package; Y10 `go.mod` `replace`.
- [x] Y15 open items 16–22 and the weaker-false-results list; contract amendment 21, design, README.
- [x] Verification: fmt, clippy, unit tests, e2e twice with the database and containers (no `skipping:`), stub scan; 33 mutants (31 killed, 2 equivalent with their code deleted); four public repositories against `531e14e` (three byte-identical, jhipster: one RTA proof withdrawn, orphan first-rule shift only), fresh == cached; the evaluator's repros end to end.
- [x] Evaluator pass 2 (verification only): `tasks/eval-m4b-pass2.md`, VERDICT PASS.

## M4b closing
- [x] Evaluator pass 2 (verification only, against `304fe4a`): Y1–Y15 repros and controls end to end with runtime confirmations, 21 mutants from a clean build (21 killed), four public repositories against `531e14e` (three byte-identical, jhipster's claimed 8 lines), fresh == cached, samples (`INJECTS` 20/20, orphans 9/9, versions 20/20), regression suite, invariant 1, docs vs code (N1, N2 doc notes).
- [x] M4b closed 2026-10-07 (contract amendment 22: final S1–S28/invariant table, review history of passes 1–2, closing measurement, open items by milestone with the M5 adversarial-review list, item 23); design §13, README, IMPLEMENT.md, CHANGELOG updated.

# M5 Scale & API (design §13; contract `tasks/contract-m5.md`)

- [x] Contract written (2026-10-07): soundness claims and closing bar up front; S1–S28, invariants 1–11, D1–D18, forks F1–F6; every M3b/M4a/M4b item assigned to M5 dispositioned (D16). Not started.

## M5 phases (contract `tasks/contract-m5.md`)

Same rules as M4b:
- Each phase has at most 5 hand-edited files. Blessed goldens and guard data are reviewed separately and listed in the changelog.
- Each phase is test-first: the new units fail, or do not compile, before the change.
- Each phase ends with its verification step: fmt, `clippy --workspace --all-targets -D warnings`, the touched crates' unit tests counted from a log holding both streams (`> log 2>&1`), and the e2e suite with `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb`.
- Any golden that changes gets a classified line diff before `RB_BLESS=1`.
- Mutation runs follow M4a's harness rules: fresh mtime, restored bytes checked, a `test result:` line per mutant, stopped with SIGINT.
- Environment: never `npm`/`npx` on the host; Docker context `desktop-linux`; `df` before every build, clone and container step, floor 6 GiB for benchmark steps and 2 GiB otherwise; one-off builds (baselines, scratch crates) use a `CARGO_TARGET_DIR` under the scratchpad and are removed afterwards.

`PIPELINE` becomes `m5-1` at the first dump change (phase 12) and is bumped on each later one. `RECIPE` is bumped in phase 15.

- [x] **0. Preflight** (no code). Done 2026-10-07: contract "Checked in phase 0" and amendment 1. Baseline `var/m5/base/` (binary `rb-afae548`, three fixture dumps; the M4b repositories' dumps in `var/fix-m4b-1/dumps/`); reader-role grants confirmed; lock limit measured (40 graphs per transaction pass, 80 fail with `out of shared memory`); S2 not reproduced on `afae548`; axum 21 crates / +0.68 MiB (under D1's threshold); `--internal` network and DNS probed; tinyproxy sized; fetch hosts deferred to phase 15 and Maven duplicates to phase 14 (each needs that phase's tooling); benchmark pins by `git ls-remote` and the GitHub trees API (no clone).
  - Read the contract (soundness section first), the lessons, contract-m4a amendment 18 and contract-m4b amendment 22.
  - `df`, `docker context show`, `docker image inspect` for both indexer images.
  - Build `afae548` (scratch target directory) and dump the three fixtures (`--no-scip`, from `git ls-files` copies) and the four M4b closing repositories at their pins: the invariant-1 baseline.
  - Check the contract's "not verified" list, each with its command and output recorded: `cypher()` under `SET LOCAL ROLE rb_reader` with draft grants; dropping 10 graphs in one transaction (error text, `SHOW max_locks_per_transaction`); S2's concurrent-publisher load on the `afae548` binary (crash count); a scratch crate with D1's axum features (new crates in its lockfile, licences, release size delta); Docker Desktop `--internal` network: no route out, DNS behaviour, `--dns 127.0.0.1`; tinyproxy image size before building; the fixtures' fetch hosts from a proxy log; Maven's duplicate choice with the scip-java image (S20).
  - Benchmark candidates (S23): GitHub API size, shallow fetch of a pinned commit into the scratchpad, line counts per language by script, build system and SCIP project detection read; choose and pin; remove the clones.
  - **Verify:** the contract gains "Checked in phase 0" with every finding and its source; any deviation from a decision is an amendment before code; F1's threshold checked.
- [x] **1. Whole-snapshot reads and the AGE DDL lock** (S1, S2). Done 2026-10-07: `Store::read_tx`/`read_snapshot` (one `REPEATABLE READ READ ONLY` transaction; nested calls join it; writes refused inside); `load_graph`, `load_diagnostics`, `status` and every CLI read command (`status`, `dump`, `find`, `service`, `table-deps`, `callers`, `impact`, `path`: the live pointer and the reads together) run in one; `RB_AGE_DDL_LOCK` before `create_graph` and every `drop_graph`, after the repository row lock (`delete_repo` and retention now lock the row first). Tests in `rb-store/src/db_tests.rs` (database; the S1 test lives there rather than in `e2e.rs` because its interleaving point is a test-only hook): S1 failed first (nodes, no edges), S2 failed first (publish finished under the held lock); mutations 4/4 killed; S2 load on the M5 binary 0 crashes; fixture dumps byte-identical to `afae548`; e2e unchanged (18/18).
  - *Derived:* `Store::read_snapshot` (one `REPEATABLE READ READ ONLY` transaction); `RB_AGE_DDL_LOCK` taken before `create_graph`/`drop_graph`.
  - *Invariants:* 4, 5 (lock part).
  - Files: `rb-store/src/lib.rs`, `rb-store/src/age.rs`, `rb-cli/tests/e2e.rs` (the two-connection test).
  - **Verify:** the S1 test fails first (no edges read after the concurrent delete), then passes; S2's load measurement with the M5 binary (crash count recorded); e2e unchanged.
- [x] **2. Retention and the graph lifecycle** (S3). Done 2026-10-07: the snapshot row is inserted inside the publish transaction after the repository row lock (the parent read under it); a failure writes a separate `failed` row after the rollback (`recorded as failed snapshot n`); retention (`Store::retain`, public `apply_retention`) drops every non-live snapshot's graph one per transaction (`drop_one_graph`: row lock, DDL lock, `age_graph = NULL`), then deletes `failed` rows and retired rows beyond `--keep` in one transaction, never the pointer's row (reported as `live_marked_retired`, a warning on stderr) nor a row still naming a graph; `delete_repo` drops one graph per transaction too. `rb index --keep N` (0-20, `RB_KEEP_RETIRED`, default 2; parse-time bound). `publish_snapshot` returns `Published { snapshot, retention }`. Tests (database): the random-sequence property (48 seeded steps over two repositories, each operation ≥ 5 times), the killed publisher (no row, no graph), the failed row, the pointer never selected (each guard with its own row); a CLI parse test. Mutations 9/9 killed, two of them the pre-M5 behaviours (row committed before the transaction; retired graphs kept). No status golden prints snapshot counts: none changed; fixture dumps byte-identical.
  - *State:* snapshot rows inserted inside the publish transaction; `age_graph` cleared at retirement.
  - *Derived:* the previous live graph dropped after the flip, one per transaction; `--keep N` / `RB_KEEP_RETIRED`.
  - *Invariants:* 5; the random-sequence property test with injected failures; a killed publisher leaves no row.
  - Files: `rb-store/src/lib.rs`, `rb-store/src/age.rs`, `rb-cli/src/main.rs` (`--keep`, the status line's graph count), `rb-cli/tests/e2e.rs`.
  - **Verify:** property test (database) and units; e2e (status goldens that print snapshot counts classified).
- [x] **3. `rb gc`, `rb cache prune`, `--no-cache` writes** (S4, S5). Done 2026-10-07: `Store::gc_plan(prefix)`/`gc_apply` (graphs `rb_<prefix>…` that no live snapshot names, one per transaction under the row and DDL locks with a re-check; `building` rows re-checked); `rb gc [--dry-run] [--prefix P]` (`--prefix` added so the e2e clean-up and tests stay inside their own repositories; `--egress` arrives with phase 15's network and proxy); `Store::prune_facts`/`prune_scip` and `rb cache prune --facts|--scip [--dry-run]` (rows and bytes; `--scip` refuses when Docker or the scip-java image cannot be identified); `--no-cache` uses `WriteOnlyFactsCache` (in `rb-cli`, beside `StoreCache`, so `rb-indexer` is untouched), help text and README updated. The S5 e2e failed first (a plain run after `--no-cache` parsed 3 of 3). Prune is tested in a scratch database (`t_store_db_*`, dropped after), since it is database-wide. Six hand-edited files (`age.rs` only made `lock_ddl` crate-visible). Dry runs on the shared database: `rb gc` would drop `rb_fp3_class_validator_274` and `rb_fp3_petclinic_272` (retired `fp3_*` snapshots' graphs from before M5) — not run, reported; `rb cache prune --facts` would remove 22,913 rows (248,136,074 bytes), `--scip` 0 — not run.
  - *Derived:* orphan graphs and stale `building` rows listed and removed; facts/SCIP rows of other versions pruned; `WriteOnlyFactsCache`.
  - Files: `rb-store/src/lib.rs`, `rb-indexer/src/syntax.rs`, `rb-cli/src/main.rs`, `rb-cli/tests/e2e.rs`, `README.md`.
  - **Verify:** the S5 test fails first (a plain run after `--no-cache` parses every file); planted orphan graph and stale row removed; a live graph never listed.
- [x] **4. Cypher guard rails and e2e robustness** (S7, S8). Done 2026-10-07: migration `0006_reader_role.sql` (idempotent: role created if absent, grants on `rb`, `ag_catalog.ag_graph`/`ag_label` and every existing `rb_*` graph; applied twice in one transaction cleanly; `rb migrate` on the existing database: `schema up to date (0006_reader_role)`); the publish transaction grants each new graph's schema; `Store::cypher(snapshot, query, cols, CypherLimits)` in one `REPEATABLE READ READ ONLY` transaction with `SET LOCAL statement_timeout` and `SET LOCAL ROLE rb_reader`, `LIMIT cap + 1`, returning `CypherRows { columns, rows, truncated }`; typed `CypherError` (`NoProjection`, `Timeout { limit_ms }`, `Refused`); `rb cypher --timeout-ms` (1-30,000, `RB_CYPHER_TIMEOUT_MS`), `truncated: rows` printed after the rows; e2e start-up removes `t_*` repositories and `rb_t_*` graphs through `gc_plan`/`gc_apply` (once, under the mutex); parity and per-service queries pass `--timeout-ms 30000`. Contract amendment 2 (test forms, S8 measurement: 3 × 21/21 under 8 burners). Mutations 8/8 killed (one survived the first version of the timeout row and was fixed with a coarse elapsed bound). Six hand-edited files (`db_tests.rs` beside the plan's five).
  - *State:* `migrations/0006_reader_role.sql`; the publish transaction's grants.
  - *Derived:* `--timeout-ms`, row cap with `truncated: rows`, `SET LOCAL ROLE rb_reader`; the e2e start-up clean-up through `rb gc`'s code; parity queries with a 30 s timeout.
  - Files: `migrations/0006_reader_role.sql`, `rb-store/src/lib.rs`, `rb-store/src/age.rs`, `rb-cli/src/main.rs`, `rb-cli/tests/e2e.rs`.
  - **Verify:** timeout, read-only, permission-denied, row-cap and retired-snapshot rows; e2e three times under 8 CPU burners, graph counts before and after recorded.
- [x] **5. AGE projection parity and `op`** (S6). Done 2026-10-07 (amendment 3: the attribute is `ops`, an array): edges with `attrs.ops` carry `ops` (separate `<LABEL>.ops.csv` files); vertices load with `nid` as the id column (the redundant `id` property is gone; AGE's `__id__` stays, = `nid`); `rb index --profile` prints each loaded file's label, rows, CSV bytes and load time and the totals (`Published.projection`, printed only); the e2e parity (`age_parity_failures`) compares the whole vertex set by `(label, properties(n))` and the whole edge multiset by `(label, start nid, end nid, properties(e))` as typed JSON, replacing the provenance-less edge check and the endpoint-only vertex check. Failed first (QUERIES without `ops`; then every vertex's extra `id`/`__id__`). Mutations 5/5 killed (a skipped edge label, a skipped vertex label, `certainty` as text, `ops` dropped, `nid` as text). Fixture dumps byte-identical.
  - *Derived:* `op` on edges with `attrs.op`; per-label load times under `rb index --profile` (printed only).
  - *Invariants:* 6: whole vertex set and edge multiset.
  - Files: `rb-store/src/age.rs`, `rb-cli/src/main.rs` (`--profile`), `rb-cli/tests/e2e.rs`.
  - **Verify:** parity on every fixture; mutations (a skipped label, `certainty` as text) fail it; no dump change.
- [x] **6a. Prebuilt snapshot index** (S9). Done 2026-10-07: `rb_store::code::SnapshotIndex` (owns the `Graph`; node positions; inbound and outbound CSR arrays of edge indices in the graph's edge order; `Send + Sync`; build counted) and per-question `View` (the index, precision, the pruned set); `resolve_target`, `Describer`, `callers`, `impact`, `resolve_endpoints`, `request_path`, `upstream_of`, `table_rollup`, `card_sections` take `&SnapshotIndex`; `Store::table_deps` and `Store::service_card` take the index instead of loading the graph; the CLI builds one index per invocation. The counted test failed first (depth-1 callers: 3,009 → 12,009 operations for n → 4n, the per-question adjacency build) and now costs the same at n and 4n, with the build ≤ 5×. Every golden unchanged (no re-bless). Mutations: 2/2 killed by units (a question rebuilding the index; reversed adjacency order); the reversed order leaves the goldens unchanged (outputs are sorted), so the order unit is that guard. Eight hand-edited files: the plan's five plus `rb-cli`'s `main.rs`, `code.rs`, `path.rs`, whose calls the new signatures change.
  - *Derived:* `SnapshotIndex` (immutable, `Send + Sync`) and per-question state; every question, the card's sections and Q3's roll-up take it; `table_deps`/`service_card` stop reloading the graph.
  - *Invariants:* every golden unchanged; counted test (depth-1 callers independent of graph size).
  - Files: `rb-store/src/code.rs`, `rb-store/src/path.rs`, `rb-store/src/rollup.rs`, `rb-store/src/card.rs`, `rb-store/src/query.rs`.
  - **Verify:** units; e2e with **no** re-bless.
- [x] **6b. Guard rails complete** (S10). Done 2026-10-07 (amendment 4): `rb_store::limits` (D6's table, `Limit`, `Caps`); a real per-request work budget (`work::metered`/`spend`, thread-local, a nested question joins its caller's, exact: a budget of n admits n elements) spent by every adjacency read; Q1 `tree` (5,000 entries) and `work`; Q2 `truncated` is a set of limit names (`nodes` at every insertion site, `work`; orphans not computed after a `work` cut); Q4 `tree` (was `nodes`) and `work`; Q3 `nodes` and `work` (the API hops share the budget); results carry `work`; the CLI prints `truncated: …` for callers and impact, impact JSON lists names. Tests: the seeded property over random graphs with cycles for Q1–Q4 (an answer that differs from the uncapped one names the lowered cap; untruncated equals uncapped; `tree` (Q1) and `work` strict), boundary rows per cap (Q1 tree/work, Q2 nodes incl. direct dependants/work, Q3 nodes/work, Q4 tree), work-count determinism, budget unit, CLI line unit, the work-cap calibration e2e. Golden: `impact-legacy-orphans.json` `"truncated": false` → `[]` (the named S10 change, read and blessed alone). Mutations 13/13 killed (one survivor, Q3's nodes cap removed, killed after its boundary row was added). Fixture dumps byte-identical.
  - *Derived:* Q1 `tree` cap; Q2's `truncated` as limit names; the `work` cap counted in every build; D6's limits as one table of constants.
  - Files: `rb-store/src/lib.rs` (`work`), `rb-store/src/code.rs`, `rb-store/src/path.rs`, `rb-store/src/rollup.rs`, `rb-cli/src/code.rs` (rendering).
  - **Verify:** the random-graph property test (caps lowered and lifted); a hit row and a one-below row per limit; e2e (impact goldens' `truncated` line only if any golden truncates: classified).
- [x] **7a. `rb-api`: envelope, errors, moved resolution and renderers** (S11, S12). Done 2026-10-07 (amendment 5): new crate `rb-api` (`error`: D5's table, `ApiError` with `param`/`candidates`, `cli_text`; `params`: the string map, unknown/duplicate names refused, D6 bounds never clamped; `render`: `pick_target`, `pick_endpoint`, `impact_json`, the path JSON and `PathView` moved from `rb-cli`, `callers_json` new; `lib`: `Endpoint`, `parse` → `Request`/`Query` with the normalised query, `run` in one read transaction (Cypher after it), `Answer`, `Output::body` (D4 envelope, not yet printed) and `result_json`, `Indexes`/`LoadEach`). The CLI's question commands (`find`, `service`, `table-deps`, `callers`, `impact`, `path`, `cypher`) build `Params` from their flags (`asked`) and render text from the same `Answer`; errors exit with D5's codes. Every golden unchanged; dumps byte-identical; mutations 7/7 killed.
  - *State:* none (no table, no dump change).
  - *Derived:* request parameters from a string map; D4 envelope; D5 error table; `pick_target`, `pick_endpoint`, `impact_json`, `render_json` moved from `rb-cli`.
  - *Plan:* `rb-api` = `error` (ErrorKind × exit code × status, `ApiError` with `param` and `candidates`), `params` (string map, unknown/duplicate parameters refused, integers with D6 bounds never clamped, booleans `true|false`, enums), `render` (moved JSON renderers and resolution), `lib` (`Endpoint`, `Request`/`Query` with the normalised query, `Output`/`Answer`, `Envelope`, `run` in one read transaction, `Indexes` with the CLI's load-per-invocation). The CLI builds a `Params` from its flags (question flags become strings so `rb-api` is the only validator) and renders text from the same `Answer`; existing `--json` outputs print the bare `result` until 7b; errors exit with D5's codes, text keeps today's messages (an `invalid_param` names the CLI flag).
  - *Invariants:* 2 (one request layer); every text golden and JSON golden unchanged; dumps unchanged.
  - Files: new `crates/rb-api/Cargo.toml`, new `crates/rb-api/src/lib.rs`, `rb-cli/src/code.rs`, `rb-cli/src/path.rs`, `rb-cli/src/main.rs`.
  - **Verify:** rb-api units (bounds, error kinds, candidate order); e2e text goldens unchanged.
- [x] **7b. JSON everywhere and `--snapshot`** (S11, D17). Done 2026-10-07 (amendment 6): every question's `--json` prints the D4 envelope (`rb callers --json` with `entry`, `rb find --json`, `rb status --json` with the coverage lines as data, new `rb snapshots [--json]`, `rb cypher --json`); `--snapshot` on every question (kept retired snapshots answer from their rows; deleted → `snapshot_not_retained`, foreign or unknown → `not_found`; Cypher on a retired one → `invalid_param`); JSON errors print their body on stdout; the card spends from the work budget (`card.rs`); `RB_TEST_CAPS`. Goldens: four JSON goldens wrapped (result lines identical, re-indented by two: classified by script), three new (`callers-send-code.json`, `find-auth.json`, `status.json`, each read against its text golden). Mutations 11/11 killed (one survivor fixed by pinning the card's element count). Dumps byte-identical.
  - *Derived:* `rb callers --json` (with `entry`), `rb status --json`, `rb find --json`, `rb snapshots`; existing JSON outputs inside the envelope; `--snapshot` on every question.
  - Files: `crates/rb-api/src/lib.rs`, `rb-cli/src/main.rs`, `rb-store/src/query.rs` (status as data), `rb-cli/tests/e2e.rs`.
  - **Verify:** new JSON goldens; existing JSON goldens re-blessed after a diff showing only the envelope (classified); a question on a retired snapshot and on a deleted one.
- [x] **8. Pagination** (S15, D18). Done 2026-10-07: `limit`/`offset` on `find` (one `LIMIT/OFFSET` query and one count: `Store::find` → `FoundPage`), `services` and `snapshots` (default and max 1,000, paged by `Paged::of` taking at most `offset + limit` elements); results `{total, offset, limit, next_offset, items}`; CLI `--limit`/`--offset` (`rb service` list, `rb snapshots`, `rb find --offset`), next page on stderr. Golden `find-auth.json` (items equal the old result; `offset: 0` in the query; classified). Mutations 6/6 killed (the CLI-offset mutant first hung the e2e page loop: the loop now asserts progress and is bounded; lesson recorded). Dumps byte-identical.
  - *State:* none.
  - *Derived:* `limit`/`offset` on `find` (default 20, max 1,000), `services` and `snapshots` (default and max 1,000); results `{total, offset, limit, next_offset, items}`; `find` as one `LIMIT/OFFSET` query plus one count; in-memory lists paged by skipping and taking from an iterator; the text lists the page's rows on stdout and, when more remain, the next offset on stderr (stdout stays rows only).
  - *Invariants:* 9/10: a page examines at most `offset + limit` list elements (counted); concatenated pages equal the whole list; `next_offset` null exactly on the last page; orders total (find ends on kind and key bytewise; services by key; snapshots by id).
  - Files: `crates/rb-api/src/lib.rs`, `rb-store/src/query.rs` (`find` count and offset), `rb-cli/src/main.rs` (`--limit`/`--offset` where listed), `rb-cli/tests/e2e.rs`.
  - **Verify:** concatenated pages equal the full list at limits 1, 7 and the maximum; counted test.
- [x] **9. `rb serve`** (S13; D1, D3, D7, D8; after F1 is confirmed). Done 2026-10-07 (amendment 7): new crate `rb-server` (one fallback handler: Host allow-list → 421, path, method → 405 with `Allow`, Cypher's JSON → 415, `rb-api` parameters → 400, all before the database; semaphore of `--workers` with `--queue` waiting, else 503 `busy` at once; `spawn_blocking` holding the slot and a pooled read-only session; `--deadline-ms` → 503 `deadline_exceeded`; shared index cache: live per repository + one other; `Server-Timing`; no CORS); `rb serve [--bind] [--workers] [--queue] [--deadline-ms] [--cypher-timeout-ms]`, non-loopback bind refused; `Store::connect_read_only`. axum 0.8.9: 21 crates, +1.17 MiB of the release `rb`, under D1's guard. Mutations 8/8 killed. Dumps byte-identical.
  - *State:* none (dependencies only: axum 0.8 with `http1`, `tokio`, `query`; tokio's `rt-multi-thread`, `net`, `signal`, `time`, `sync`).
  - *Plan:* one fallback handler (no router state to drift): Host allow-list (`127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>` of the bound port) → 421; method per endpoint (GET, POST only for cypher) → 405, `OPTIONS` included; cypher's `Content-Type: application/json` → 415; path → endpoint or 404; parameters parsed by `rb-api` before any database work (unknown → 400); then a worker slot (semaphore of `--workers`, at most `--queue` waiting, else 503 `busy` at once); the work in `spawn_blocking` holding the slot and a pooled read-only connection (`SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY`), under `--deadline-ms` (503 `deadline_exceeded`, no body of results; the abandoned work keeps its slot); the index cache (live per repository + one other snapshot) shared by the workers; `Server-Timing` header, nothing timed in bodies; no CORS header anywhere. Startup refuses a non-loopback `--bind`. Test hook `RB_TEST_HOLD_MS` (each request's work waits first).
  - *Invariants:* 7 (loopback, Host, no CORS, read-only sessions), 8 (bodies equal `rb-api`'s; timing only in a header), 9 (slot, queue, deadline, work cap).
  - *Derived:* axum binding; pool of read-only connections; semaphore and queue; deadline; Host allow-list; snapshot cache with live-pointer check; `Server-Timing`.
  - Files: `Cargo.toml` (workspace dependencies), new `crates/rb-server/Cargo.toml`, new `crates/rb-server/src/lib.rs`, `rb-cli/Cargo.toml`, `rb-cli/src/main.rs` (`rb serve`).
  - **Verify:** rb-server units per rule (non-loopback bind, foreign `Host` → 421, no CORS, `OPTIONS` → 405, `POST` without JSON → 415, unknown parameter, busy, deadline via a test hook, read-only sessions, live-pointer change); `Cargo.lock` delta and licences recorded.
- [x] **10. API ≡ CLI** (S14). Done 2026-10-07: `tests/api.rs` `api_bodies_equal_cli_json`: per fixture 44 parameter sets (targets found from each fixture's own answers) through `rb … --json` and a running `rb serve`, 132 in all plus a deleted snapshot (410), a repository never indexed, every repository's status, and five capped sets per fixture with `RB_TEST_CAPS` on both sides (at least three cut per fixture); bodies byte-identical; statuses 200 ×91, 400 ×15, 404 ×15, 409 ×8, 410 ×1, 422 ×3, each the exit code's by D5; a second server answers with the same bytes. Helpers moved to `tests/common/mod.rs`. Mutations 4/4 killed (a server-only renderer change, a server-only error body change, a status remapped, the server ignoring the cap override).
  - *Plan:* `tests/api.rs` runs, per fixture (`--no-scip`), a parameter table per endpoint (defaults; maxima; `rta`; `pick`; an ambiguous and a missing target; a value out of bounds; `snapshot=`; a truncating cap through `RB_TEST_CAPS`, given to both the CLI and a second server) through `rb … --json` and through a running `rb serve`; bodies equal byte for byte, and each status matches the exit code by D5's table (200 ↔ 0). Targets are discovered from each fixture's own `find` answers (an endpoint, a table, a name two symbols share), so every fixture exercises the ambiguous rows. Shared helpers move to `tests/common/mod.rs`.
  - *Invariants:* 2 (byte identity), 8 (two servers, same bodies).
  - Files: new `crates/rb-cli/tests/api.rs` (shares the e2e helpers through a small `tests/common/mod.rs`), new `crates/rb-cli/tests/common/mod.rs`, `rb-cli/tests/e2e.rs` (helpers moved).
  - **Verify:** byte equality per endpoint × fixture × parameter set; status ↔ exit-code table; the server-only renderer mutation fails the test.
- [x] **11. `rb diff`: exact sections** (S16). Done 2026-10-07: `rb_model::diff` (`NodeRow`/`EdgeRow` with the dump's identities and lines, `diff` by merge-join, `GraphDiff` with nodes added/removed/changed and edges added/removed/changed/evidence-only, `invert`, `apply` over dump lines with conflict checks, `diagnostics_diff` as a multiset of `D` lines with counts per kind); `canonical_dump` renders through the same rows (dumps byte-identical). Properties on 200 seeded random graph pairs (every node and edge kind, self-loops, cycles, duplicate names; every section exercised); counted growth 5,916 → 23,770 steps for 3,549 → 14,227 rows (4.0×). Mutations 5/5 killed.
  - *Derived:* `rb_model::diff` (`GraphDiff`, `apply`, `invert`), canonical order.
  - Files: new `rb-model/src/diff.rs`, `rb-model/src/lib.rs`.
  - **Verify:** units per section; the apply, self-empty and inverse properties on generated graphs; counted growth.
- [x] **12. `rb diff`: summaries, CLI and API; M4a item 6** (S17, S18). Done 2026-10-07 (amendment 8): S18 `base: unresolved` on suffix edges without a resolved host (`PIPELINE` `m5-1`; the fixture's only changed dump line is web `payInvoice`, classified; `dump.jsonl` re-blessed for it, the `must-exist` guard now states it); `rb_store::diff` summaries (endpoints, table access, service dependencies with the card's `Membership`, certainty shifts); `rb-api` `diff` (selectors, default parent, `tools_differ`, paged sections); `rb diff [<from>] [<to>] [--json] [--diagnostics] [--evidence] [--limit] [--offset]`; `GET …/diff` through the server (API ≡ CLI now 150 sets). Every summary row of the two-commit history predicted before blessing `diff-polyglot.{txt,json}`; apply property on all nine ordered fixture pairs; empty diff of each forced rebuild. Mutations 7/7 killed.
  - *Derived:* endpoint, table-access and service-dependency summaries from the index functions; `rb diff` text and JSON; `GET …/diff`; `base: unresolved` on suffix edges (`PIPELINE = m5-1`).
  - Files: new `rb-store/src/diff.rs`, `crates/rb-api/src/lib.rs`, `rb-cli/src/main.rs`, `rb-indexer/src/stitch.rs`, `rb-cli/tests/e2e.rs`.
  - **Verify:** the two-commit polyglot history in the test, each summary row predicted before blessing `diff-polyglot.{txt,json}`; apply property on every fixture pair; the `base` units fail first; invariant-1 diff shows only `base` on suffix edges (classified).
- [x] **13a. Review fixes: carriers** (S19 a, b, f). Done 2026-10-07 (amendment 9, `PIPELINE` `m5-2`): `clients::discovery_text` (amendment-17 words + the LoadBalancer/Ribbon package names + structure-free JSON key paths) on every non-binary file, parsed ones in `syntax::scan_unparsed` (not counted as scanned, no hooks); XML comments read; `UrlValue::Contested` with candidates from resolvable spreads (new fact `Init::Object`) and disagreeing Spring bases; the stitcher makes a site external when a joined base's candidate names an external host. Tests failed first (did not compile: new function, variant, field) and their mutations bite: 12/12 killed (`var/m5/mut/p13a.json`). Invariant 1: fixtures — `polyglot-monorepo`'s `http_coverage` `bases.unverified` 0 → 5 (S19 b, pom comment "fetch") besides S18's line; the two-commit diff's `loginCheck` `fetch` edge `synthetic` → `heuristic` (S19 b); the four M4b repositories: only S18's 5 lines on petclinic-microservices. Seven source files (the spread needs a syntax fact).
  - *State:* `rb_syntax::Init::Object` (a fact; the extractor fingerprint changes, so cached facts are re-extracted); `PIPELINE` `m5-2`.
  - *Derived:* discovery over every file's text; `UrlValue::Contested`; `Site::base_candidates`; `http_external` with detail `base_candidate_external`.
  - *Invariants:* 1 (can only cap or remove: a discovery hit lowers `synthetic`, an XML-comment hook caps, an external candidate removes a suffix edge).
  - Files: `rb-syntax/src/{lib,calls}.rs` (+ `bindings.rs` test), `rb-frameworks/src/{clients,fold}.rs`, `rb-indexer/src/{stitch,syntax,lib}.rs`.
  - **Verify:** each row fails first, its control stays, its mutation bites; cost on the fixtures and the four M4b repositories against the phase-0 baseline (classified).
- [x] **13b. Review fixes: orphans and DI** (S19 c, d, e, g). Done 2026-10-07 (amendment 10, `PIPELINE` `m5-3`): (c) recorded (X1 row present); (d) fragment implementations and their members are not orphan-eligible (counted under `convention`); (e) rule (e) counts a head external unless its Tier B resolution is unique and no import may bind it from outside (`Index::imported_from_outside`); (g) class-literal registrants join a point's scanned candidates (`ambiguous`, `origin: registrant`, no `@Primary`/name pick). Tests failed first (`spring_data_fragment_implementations_are_not_orphans`, `supertype_heads_resolved_by_name_are_external_unless_certain`, `registrants_named_by_a_class_literal_are_candidates`); mutations 11/11 killed after two equivalent mutants were removed by simplifying the code (`var/m5/mut/p13b.json`, `p13b-2.json`). Invariant 1: `java-spring-jpa` 2 lines and jhipster 2 lines (S19 d), petclinic 1 counter line; rule (e) and (g) cost nothing measured.
  - *State:* `PIPELINE` `m5-3`.
  - *Derived:* `liveness::Inputs::fragments`; `Index::imported_from_outside`; `di::literal_registrants`.
  - *Invariants:* 1 (keeps symbols alive; `ambiguous` instead of a single wrong `heuristic`); depth 2 and cycle rows for rule (e).
  - Files: `rb-indexer/src/{liveness,di,resolve,lib}.rs`.
  - **Verify:** as 13a; depth and cycle rows for rule (e); `ambiguous` over the registrant and the scanned bean.
- [x] **14. Residue records** (S20). Done 2026-10-07 (amendment 11): M4b 18 and 20 restated with examples and their weaker false results; M4a 22's residue list unchanged (three scan-visible forms removed by S19 a, b); Maven 3.9.16 in the scip-java image, offline: duplicate `<dependency>` → last wins (warning), duplicate property → last wins (no warning), duplicate `<dependencyManagement>` entry → first governs (warning); undocumented and inconsistent, so `@?` stays; five new forms listed as M6 items. No code.
  - **Verify:** each record names its example and the claim's residue class.
- [x] **15. Fetch-phase egress and per-repository caches** (S21, S22; F2 "in"). Done 2026-10-07 (amendment 12, `RECIPE` `m5-egress-1`): `rb-egress-proxy:1.11.3` built locally (Alpine digest base, `tinyproxy=1.11.3-r0`; ID `sha256:9e2f31c4…`, 13.8 MB); `rb_scip::egress` (allow-list, tinyproxy config and filter `host:443` with `FilterURLs On`/`FilterDefaultDeny Yes`, Maven `settings.xml`, proxy environment, refusals from the log); one proxy per fetch run on the internal `rb-scip-fetch` + bridge `rb-scip-egress` (`Runner::start_proxy`/`stop_proxy`; a non-internal `rb-scip-fetch` refused); `Network::Fetch` (`--network rb-scip-fetch --dns 127.0.0.1`), read-only client mounts; the allow-list in the cache key; proxy image absent → `unsupported`; `--scip-registries`/`RB_SCIP_REGISTRIES` (host-side, amended from `rb.toml`); per-repository caches `cache/<repo>`; Develocity/Gradle Enterprise build caches off in both Maven phases; npm's update notifier off (found by the per-repository cache); `rb gc --egress`. Probe suite green; SCIP e2e 7/7 through the proxy, no `skipping:`, goldens unchanged; spring-retry 68 → 0 files without a document, fresh == cached; mutations 16/16 killed.
  - *State:* `RECIPE` `m5-egress-1`; no `rb.toml` change (amended: extra registries are host configuration).
  - *Derived:* the egress networks (created when absent, removed by `rb gc --egress`), a proxy per fetch run, the proxy variables and a generated `settings.xml`; caches per repository; build-cache flags.
  - *Invariants:* 11 (the fetch container's only network is the internal one, its only peer the run's proxy, which allows `CONNECT host:443` to the allow-list only); 8 (dumps identical across cold and warm per-repository caches).
  - Files (deviation: nine hand-edited, the per-repository cache, the host-side registries and `rb gc --egress` need the CLI, and the tests' clean-up and probe need the e2e helpers): new `docker/egress-proxy/{Dockerfile,README.md}`, new `rb-scip/src/egress.rs`, `rb-scip/src/{runner,orchestrate,hash,lib}.rs`, `rb-indexer/src/projects.rs`, `rb-cli/src/main.rs`, `rb-cli/tests/{e2e.rs,common/mod.rs}`, `README.md`.
  - Before the run: `df`, the image sizes, `docker context show`.
  - **Verify:** the probe suite from a real fetch container; SCIP e2e through the proxy, no `skipping:`; spring-retry's `files without a document` before and after; fresh == cached.
- [x] **16. Benchmark set-up** (S23). Done 2026-10-07 (amendment 13): shallow fetches of both pins under `var/m5/bench/` (one at a time; mattermost counted and removed until its turn); counted lines (`var/m5/scripts/loc.py`): shardingsphere 720,005 (Java 708,960), mattermost 1,698,802 (Go 818,222, TS 793,519, JS 85,891); clones 335 and 782 MiB; release binary of `17410d7` as `var/m5/bench/bin/rb-m5`; the benchmark harness `var/m5/scripts/bench_index.sh` (`/usr/bin/time -l`, a `/usr/bin/sample` call tree of the whole run, load and disk before and after, the 6 GiB floor checked). *Original plan:* Clone the two pinned repositories shallowly under the scratchpad (or `var/m5/` if the orchestrator says so), after `df` and the measured sizes; `cargo clean` of stale worktree builds if needed; release build of the phase-15 code.
  - **Verify:** pins, line counts and disk recorded in the contract.
- [x] **17. Benchmark: indexing and storage** (S24). *17a done 2026-10-07 (amendment 14):* a publish now ANALYZEs the snapshot tables inside its transaction (found here: a read right after a publish planned a nested loop on stale statistics and did not finish in 10 minutes; test first, mutation killed); shardingsphere measured: `--no-scip` fresh 35.0 s, cached 23.4 s, incremental 18.5/19.2 s; SCIP fresh 1,878.9 s (fetch ≈ 641 s; ≈ 1,238 s without it: §12.5's 15 min **missed**, the 2-CPU scip-java index phase ≈ 1,086 s dominating), cached 49.5 s; fresh == cached both ways, empty self-diffs, AGE parity OK; 173 MiB + 64 MiB AGE per snapshot; retention delete 1.4 s; facts cache 9 % of the repository's snapshots; no superlinear stage (half vs whole). *17b done:* mattermost: `--no-scip` 29.2 s fresh, 19.6 s cached, incremental 19.6/17.9 s; SCIP 610.6 s fresh (4 of 11 TS projects index; 7 fetches fail: workspace packages and `file:` links outside the build root, M6), 24.0 s cached; deterministic, parity OK; 121 MiB + 44 MiB AGE per snapshot. *Plan:* Full `--no-scip` (fresh, cached), SCIP (fetch and index timed apart), two incremental commits, `--profile`, `/usr/bin/time -l`, database and facts-cache sizes, AGE load and parity, retention delete time, fresh == cached, empty self-diff; `/usr/bin/sample` for any stage over its target.
  - **Verify:** every number computed by a script; hardware and load recorded; gaps written with their profile.
- [x] **18. Benchmark: latency** (S25). Done 2026-10-07: two runs per repository through a warm `rb serve`, D14's sampling rule; every §12.5 latency target met (Q1 p95 ≤ 3.3 ms, Q2 p95 ≤ 152 ms, Q4 p95 3.6 ms on mattermost's 66 endpoints; shardingsphere has none); no truncation, so the work cap stands; index build 0.57/0.63 s, server RSS 284/249 MiB; CLI cold 0.4–0.6 s. *Plan:* `rb serve` warm; D14's sampling rule; p50/p95/max from a script; two runs; CLI cold times; truncations by limit; the work cap re-calibrated as an amendment if a default question hits it.
  - **Verify:** numbers recorded with load averages; any miss has a `sample` profile.
- [x] **19. Benchmark: precision and carried measurements** (S26). Done 2026-10-07: no heuristic or `synthetic` `CONSUMES` on either repository; mattermost's 8 `ambiguous` read, all false (Go tests building requests for in-process handlers, path-matched by the stated relative-URL convention; two M6 forms recorded); port-hole, constructor-member, user-information and config-by-name forms counted (none reaches an edge); Gate A capped all 306 sites but lowered 0 edges; no DI point, `INJECTS` or proven RTA type on either; mattermost orphan share 1.08 %, 10 impact runs, 1 orphan listed, true; S19's cost 0 on both. Clones, caches, `m5b_*` repositories, SCIP rows and egress networks removed, `VACUUM`. *Plan:* `CONSUMES` samples, port-hole and other form counts, Gate A's cost by carrier, DI/RTA/orphan counts and samples, TS interface-method `IMPLEMENTS`; S19's cost there. Then remove clones, `m5_*` repositories, SCIP work directories and run `VACUUM`.
  - **Verify:** samples read in the source; counts by script; disk back above its starting level.
- [x] **20. Records and review** (S27, S28). *Records done 2026-10-07 (amendment 15; no code); the evaluator passes below are open.*
  - [x] Determinism and invariant 1 (fixtures and the four M4b repositories) against the phase-0 baseline, classified: with the HEAD binary, 12 rows differ on seven repositories, each S18 or S19 (b, d) as amendments 8–10 predicted; every dump equals phase 13b's (`var/m5/p20/`). Determinism: the e2e and API suites (amendment 15 (b)).
  - [x] Records: contract amendment 15, the measurement section's phase-19 disk figures corrected, "Open items" (21, with targets and the weaker false results still possible); design §8.3/§8.4/§11/§12.5/§13/§14 as decided and measured; README (`rb serve`, `rb diff`, `rb gc`, `rb cache prune`, `--keep`, `--timeout-ms`, `--json` everywhere, `--snapshot`, pages, `--profile`, egress; ports 55432 default and 55433 here), every example run as printed (`m5r_polyglot`, removed); IMPLEMENT.md; CHANGELOG; lessons (disk accounting).
  - [x] Full verification loop and the stub scan (`var/m5/scripts/verify.sh p20`), then `cargo clean`.
  - [x] Evaluator pass 1 (full review, `tasks/eval-m5-pass1.md`: FAIL on Z1, Z2; Z3–Z6 minor, Z7 doc), fixed test-first as amendment 16:
    - [x] Z1: `status` of a never-indexed repository is `not_found` (`rb-api`; `rb-server` unit on the database; API rows `status`, `snapshots`).
    - [x] Z2 (a): Cypher reads a graph only for a `live` row; `SnapshotInfo` names a graph only for a `live` row. (b): retention drops every non-live graph (`drop_non_live_graphs`); `rb serve` sweeps every repository at startup (`sweep_graphs`). DB test with a stop-after-flip hook; property test with the stopped publish; e2e with the evaluator's lock + `SIGKILL`.
    - [x] Z3: path segments percent-decoded once (unit; API rows `%5F`, `%5f`, encoded letter, unicode).
    - [x] Z4: two `Host` → 400; foreign absolute-form authority → 421; authority ≠ `Host` → 400 (unit).
    - [x] Z5, Z6: `failed` snapshot rows (API); two and three non-live graphs (DB tests). m16, m17 killed.
    - [x] Z7: hyper's bare refusals recorded as residue (soundness table, open item 23) and pinned by a test.
    - [x] Open items 22 (adversarial egress review → M6), 23 (Z7); Z2's window closed for answers.
    - [x] Mutations 16/16 killed; invariant 1: fixture dumps equal phase 20's; `rb cache prune --facts` (19,783 rows, 649,322,998 bytes).
    - [x] Verification (`var/m5/scripts/verify.sh ev1`): fmt, clippy; 639 unit; store 65/65; server 12/12; e2e 30/30; api 1/1; no `skipping`, no listener, stub scan clean.
  - [x] Evaluator pass 2 (verification only, per the contract's closing bar; `tasks/eval-m5-pass2.md`: PASS).
  - Commit after each verified phase.

## M5 closing
- [x] Evaluator pass 2 (verification only, against `5cb1a2c`): Z1–Z7 repros and controls end to end (Z2's lock + `kill -9` twice, healed by `rb serve` and by the next publish), 12 mutants of amendment 16 from a clean build (12 killed), `verify.sh` twice green, invariant 1 (fixtures = phase 20; spring-petclinic: the one S19 (d) row against `afae548`), fresh == cached on spring-petclinic and both benchmark repositories (each equal to its phase-17 dump), API ≡ CLI on 29 own sets, no host paths, docs vs code (N1, N2: non-blocking, open items 24, 25).
- [x] M5 closed 2026-10-07 (contract amendment 17: final S1–S28/invariant table, review history of passes 1–2, §12.5 summary, open items by milestone with the M6 review list and item 22); design §13, README, IMPLEMENT.md, CHANGELOG updated.

# M6 Breadth (design §13: split into M6a–M6j)

- [x] Split planned (2026-10-07): design §13 has one row per sub-milestone (scope and exit criteria) and a "deferred beyond M6, or dropped" row; every item design §13's M6 row, contract-m4b and contract-m5 (amendment 17) moved to M6 is assigned there (contract-m6a D12 restates contract-m5's items).
- [x] M6a contract written (2026-10-07): `tasks/contract-m6a.md`: soundness claims and closing bar up front; S1–S20, invariants 1–11, D1–D12, forks F1–F7; planner probes P1 (scip-typescript interface relationships) and P2 (scip-java on Lombok) recorded. Not started.

## M6a phases (contract `tasks/contract-m6a.md`)

Same rules as M5:
- Each phase has at most 5 hand-edited source files. Fixture sources, blessed goldens and guard data are listed separately in the changelog and reviewed on their own.
- Each phase is test-first: the new units fail, or do not compile, before the change (the failure recorded).
- Each phase ends with its verification step: fmt, `clippy --workspace --all-targets -D warnings`, the touched crates' unit tests counted from a log holding both streams (`> log 2>&1`), and the e2e suite with `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb` (with containers wherever the phase touches SCIP: `docker image inspect` first, no `skipping:`).
- Any golden that changes gets a classified line diff before `RB_BLESS=1`; any dump change is classified L, T or W (contract invariant 1).
- Mutation runs follow M4a's harness rules: fresh mtime, restored bytes checked, a `test result:` line per mutant, stopped with SIGINT, `"$@"`, `command -v` for every tool.
- Environment: never `npm`/`npx` on the host (lockfiles are generated inside the pinned scip-typescript image); Docker context `desktop-linux`; no image pull (none is needed); `df` before every build, clone and container step, floor 6 GiB for large-repository steps and 2 GiB otherwise; one-off builds use a `CARGO_TARGET_DIR` under the scratchpad, removed afterwards; disk figures accounted part by part in MiB.

`PIPELINE` becomes `m6a-1` in phase 2, `m6a-2` in phase 4, `m6a-3` in phase 6. `RECIPE` is split in phase 6 (`RECIPE_JAVA` unchanged, `RECIPE_TS` `m6a-ts-1`).

- [x] **0. Preflight** (no code). Done 2026-10-07: contract "Checked in phase 0" and amendment 1; outputs `var/m6a/phase0/`, baselines `var/m6a/base/`; verify baseline `var/m6a/logs/p0-*` (639 unit, store 65, server 12, e2e 30, api 1).
  - Read the contract (soundness section first), `tasks/lessons.md`, contract-m5 amendment 17 and its open items.
  - `df`, `docker context show`, `docker image inspect` for `rb-scip-java:0.13.1`, the scip-typescript digest and `rb-egress-proxy:1.11.3`; `var/m6a/{scripts,phase0,base,bench,logs}`; `verify.sh` from M5's with M6a paths.
  - Build the `cfa8ab9` release binary in a scratch target directory (`var/m6a/base/rb-cfa8ab9`, sha256 recorded); dump the three fixtures `--no-scip` and with SCIP (the e2e's fixed identity and date): the invariant-1 baseline.
  - P1b (D4's form table) and P2b (the Lombok annotation matrix, S2's shadow file, `lombok.addLombokGeneratedAnnotation = false`) with the images present, `--network none` for P1b and a scratch `.m2` for P2b; raw relationships and positions recorded; P1b's super→sub count (0 or stop: F3).
  - The layout check of S10 (a scratch build with only the repository-shaped export; `ts-express-prisma` and `polyglot-monorepo` SCIP dumps vs the baseline) and the `npm ci` link-target check (F2's fallback).
  - The second Lombok repository by D11's rule (GitHub API sizes, pom read, one fetch attempt); D5's glob forms counted on mattermost's and the candidates' manifests; the largest `--scip-memory` the VM holds beside Postgres.
  - **Verify:** the contract gains "Checked in phase 0" with every finding and its source; a deviation from a decision is an amendment before code.
- [x] **1. `rb serve` start-up order and the two-repository sweep** (S14, S15; D10). Done 2026-10-07: *State* the bind address and the repository list; *Invariant* 11 (no write before the bind check). S14's e2e row failed first (`a refused start leaves the leftover`); mutant "sweep only the first name" killed by `rb-store` alone. Verify `var/m6a/logs/p1-*`.
  - Files: `rb-cli/src/main.rs`, `rb-store/src/db_tests.rs`, `rb-cli/tests/e2e.rs`.
  - **Verify:** S14's e2e fails first (the refused start sweeps); the "first repository only" mutant is killed by the `rb-store` test alone; loop green.
- [x] **2. Lombok removal and counters** (S1, S2 units, S3; D1–D3; `PIPELINE` `m6a-1`). Done 2026-10-07: 7 of the 8 new join units failed first (the 8th, (j)/(k), is a control that held); 5 mutants killed; fixture dumps (both modes) byte-identical to `var/m6a/base`; amendments 2 (`undocumented`) and 3 (`@FieldNameConstants` residue).
  - *Derived:* marker spans per scip-java document; removed occurrences; `lombok_removed`, `documents_lombok` (written only when non-zero).
  - *Invariants:* 2, 3; S1 (i) over every fixture document.
  - Files: `rb-indexer/src/scip.rs`, `rb-indexer/src/lib.rs` (coverage JSON), `rb-cli/src/main.rs` (status line).
  - **Verify:** S1 (a)–(e) fail first; the four S1 mutations killed; fixture dumps and SCIP goldens unchanged.
- [x] **3. `java-lombok` fixture** (S2 end to end, S4). Done 2026-10-07: fixture checked against Java's file rules (one public type per file; `Named`/`Explicit` package-private in `Shadow.java`); `scip_java_lombok` with containers (Lombok fetched through the proxy), no `skipping`; `lombok-generated 234 in 6 documents` equals `var/m6a/scripts/p2b_check.py` on the real index; removal-off mutant fails the guards and the status golden; fresh == cached == `--no-cache` == a second name.
  - Fixture sources per the contract's fixture plan (checked against Java's file rules first); goldens predicted from P2b before blessing.
  - Files: `rb-cli/tests/e2e.rs`, `fixtures/README.md` (plus the fixture sources and goldens).
  - **Verify:** `scip_java_lombok` with containers, no `skipping:`; the removal-off mutant fails the status golden and the must-exist list; fresh == cached.
- [x] **4. TypeScript interface-method `IMPLEMENTS`** (S5–S7; D4; `PIPELINE` `m6a-2`). Only if P1b found no super→sub relationship. Done 2026-10-07 (P1b: 0): S5's two positive units failed first; the waiver-on-Java mutant survived the amendment-78 unit (classes) and is killed by a new Java-interface unit, the waiver-on-every-TS-type mutant by a TS-class unit (amendment 4); fixture dumps byte-identical; Java SCIP goldens unchanged.
  - Files: `rb-indexer/src/scip.rs`.
  - **Verify:** S5's positive rows fail first; the waiver-on-Java mutant killed by the amendment-78 unit; Java SCIP goldens unchanged.
- [x] **5. Workspace membership** (S8; D5). Done 2026-10-07: `rb_scip::jobs(files, read) -> Plan` (jobs, members per grouped root, ungrouped reasons); 6 new units (did not compile before); mutants partial `*`, no-lockfile grouping and a quadratic trie walk killed; `polyglot-monorepo` prints `workspace . not grouped (workspace_without_lockfile)` and dumps byte-identically; amendment 5.
  - *Derived:* the job list from the file list and the root manifests; ungrouped reasons.
  - Files: `rb-scip/src/jobs.rs`, `rb-scip/src/lib.rs`, `rb-indexer/src/projects.rs` (manifest reader).
  - **Verify:** S8's rows; the two mutations killed; `polyglot-monorepo`'s jobs unchanged (unit and SCIP golden).
- [x] **6. Workspace fetch and index; per-indexer recipes** (S9; D6, D8; `PIPELINE` `m6a-3`, `RECIPE_TS` `m6a-ts-1`). Done 2026-10-07: `RECIPE_JAVA`/`RECIPE_TS`/`recipe(indexer)`; the Java key of a fixed input pinned to `cfa8ab9`'s value (`2798679d…bb0c`, printed by that code before the split); grouped-job argv unit; 2 mutants killed; fixture dumps byte-identical with every TypeScript job re-run under its new key (no W rows).
  - Files: `rb-scip/src/orchestrate.rs`, `rb-scip/src/hash.rs`, `rb-scip/src/jobs.rs`, `rb-indexer/src/projects.rs`.
  - **Verify:** the pinned Java hash equals `cfa8ab9`'s; argv units; e2e SCIP goldens unchanged except rows classified W.
- [x] **7. Repository-shaped export and link targets** (S10; D7). Split in two commits to keep each within five hand-edited files: **7a** (done 2026-10-07: `ContainerRun::cwd`, repository-shaped scip-typescript exports starting in the root, `job_files`/`link_targets` with `unsupported` outside the repository, `project_input_hash` over exactly the exported files; layout check of amendment 1 passed: fixture SCIP dumps byte-identical with fresh TypeScript runs; 6 mutants killed) and **7b** (done 2026-10-07: `ScipProject::link_targets` → `scip_status` `link_targets` and `, <n> link targets`; `Coverage::documents_outside_root` → coverage key and `, <n> documents outside the root`, each only when non-zero; 4 new units, 1 mutant killed; fixture dumps byte-identical).
  - Files: `rb-scip/src/orchestrate.rs`, `rb-scip/src/runner.rs` (working directory apart from the mount), `rb-scip/src/hash.rs`, `rb-indexer/src/projects.rs`, `rb-indexer/src/scip.rs` (`documents_outside_root`).
  - **Verify:** S10's units; the layout leaves the TypeScript fixtures' SCIP dumps unchanged (phase 0's check repeated on the real change).
- [x] **8. `ts-workspaces` fixture** (S11). Done 2026-10-07: fixture with lockfiles generated in the scip-typescript image (phase 0); `scip_ts_workspaces` with containers (no `skipping`): `workspace 2 members`, app→core exact, 4 interface-method `IMPLEMENTS` (T), `tools/e2e` `1 link targets` (its call into core counted external, recorded), `legacy` its own unpinned job; cold and warm package cache, cached, `--no-cache` and a second name dump identically; the D4-off mutant fails 4 guards; `cfa8ab9`'s binary on the fixture: `packages/app` and `tools/e2e` fail their fetch. The verify loop's e2e first failed (the cold-cache step assumed a fetch had run; fixed, lesson recorded, e2e re-run 32/32). Amendment 7.
  - Lockfiles generated inside the scip-typescript image (`--network none`), the command recorded in `fixtures/README.md`.
  - Files: `rb-cli/tests/e2e.rs`, `fixtures/README.md` (plus the fixture sources and goldens).
  - **Verify:** `scip_ts_workspaces` with containers; the fetch from an emptied per-repository cache twice gives the same stored output; fresh == cached.
- [x] **9. Budget flags** (S12; D9). Done 2026-10-07: `--scip-cpus`/`--scip-memory`/`--scip-maven-threads` (env `RB_SCIP_*`), `Budget::maven_threads`, `-T n` on every Maven invocation of both phases only for n > 1, Java recipe `+T<n>`; default argv of the Java phases and a TypeScript root job pinned to `cfa8ab9`'s (recorded from that code); 4 mutants killed; README; fixture dumps byte-identical.
  - Files: `rb-cli/src/main.rs`, `rb-scip/src/orchestrate.rs`, `rb-scip/src/hash.rs`, `README.md`.
  - **Verify:** default argv equals `cfa8ab9`'s for every phase (unit); bounds rows; the threaded key differs.
- [x] **10. shardingsphere: Lombok and the reactor** (S13, S16; no code). Done 2026-10-09: base SCIP fresh wall 3062.77s (`ok` 2950.5s) → M6a cached 72.7s (rejected 0, 107,101 removals in 3,951 docs); newly accepted 3,889 / rejected 0; 40/40 sample read in source. Threaded ×2 fresh (`-T4`): both `failed (index phase)` identically modulo the allowed `stderr_tail` row → flag stays (D9). `--no-scip` pair byte-identical (74214/233242). Oct-7 attempt died at the floor under load 43; retry under load ~4 completed.
  - **Verify:** S13 three dumps identical modulo `stderr_tail` (or D9 withdrawal — not needed); S16 records complete.
- [x] **11. Second Lombok repository and mattermost** (S16, S17; no code). Done 2026-10-09.
  - 11a eladmin: Lombok 1.18.30 unmarked (residue); base fresh == M6a cached == M6a fresh, byte-identical.
  - 11b mattermost: base 11 jobs/4 ok → M6a 5 jobs/3 ok, 4 ok with `--scip-registries` (F7 both outcomes; cypress git+ssh ok 57.4s); webapp `EBADENGINE` (new residue → M6e); interface `IMPLEMENTS` sets identical (T = 0; 55 clauses all in unindexed webapp); full row accounting UNACCOUNTED 0.
  - **Verify:** fresh == cached on each (eladmin 3-way, mattermost `--no-cache --force`); records complete.
- [x] **12. Records** (S18–S20; no code except classifier scripts under `var/`). Done 2026-10-09.
  - Invariant 1: fixtures `--no-scip` identical except `ts-workspaces` (3 amendment-7 W rows); SCIP identical except `java-lombok` (L-only) and `ts-workspaces` (T+W-only); measurement repos per S16–S18 notes; D7 quadratic mutant bites (D1/D5 logged).
  - Contract amendments 8–11 + measurement section; CHANGELOG; IMPLEMENT.md; design §13 (M6a measured, webapp-engines → M6e); lessons (4); this todo.
  - **Verify:** `verify.sh p10-12` green (674/66/12/32/1, no `skipping:`); evaluator pass 1 PASS (Z1 fixed as amendment 12, `z1-fix` loop green 676/66/12/32/1); evaluator pass 2 PASS. **M6a closed 2026-10-09** (contract amendment 13). Next: M6b (contract to be written when it starts).

## M6b Request-path precision (contract `tasks/contract-m6b.md`, written 2026-10-09; forks F1–F7 confirmed; started 2026-10-09)

- [x] **0. Preflight** (no code). Done 2026-10-09 (amendment 1): baseline binary `var/m6b/base/rb-3562b95` (sha256 `ede3e290…55aad5c`); fixture dumps with it, `--no-scip` and SCIP, equal M6a's closing hashes on all 5 fixtures; `var/m6b/{scripts,base,bench,logs}`; `verify.sh` from M6a's with M6b paths. Shape checks on shallow checkouts (removed after reading): gin-realworld — `Register(v1.Group("/users"))` literal args into `*gin.RouterGroup` params, nested through `v1 := r.Group("/api")` (composition must fold the receiver chain too); node-express — prefix-less chain links over default imports plus `export default Router().use('/api', api)` over a local const; ftgo — `this.consumerService = \`${options.baseUrl}/consumers\`` in the constructor, used as `${this.consumerService}/${consumerId}`; full ftgo pin `558dfc53b11d30a5f1d995c0c6d58d5106c28189`. `HttpClient`: no occurrence in any measured repo → synthetic probe in the fixture phase. Images verified by inspect; DB 0 repos after the dumps.
- [x] **1. Facts** (S1). Done 2026-10-09 (amendment 2): `MemberWrite` facts carry `ctor: bool` from the nearest function-like ancestor (`method_definition` named `constructor`, `constructor_declaration`; a nested function is not a constructor write); compound assignments (`+=`, `||=`, `??=`) count as writes; bare-field writes are not members. New-library gates and sink patterns need no new facts (imports, calls with args, `Binding.type_name` carry them). Fingerprinted; e2e passed without re-blessing any golden. Files: `rb-syntax/src/{bindings,lib}.rs`.
- [x] **2. gin parameter groups** (S2; `PIPELINE m6b-1`). Done 2026-10-09 (amendment 3): a composed endpoint's key is the normal `endpoint_key(method, path, service)` (no `?router` suffix), so identical compositions merge and literal registrations join them; the (router site, call site) identity is one `EXPOSES`/`HANDLED_BY` evidence row per site (D1); the `prefix_unknown` node stays iff no call site was found or some site did not resolve-or-fold (safe direction); one level only (a group passed onward stays unknown); `Arg::Other` folds through `chain_at(span)`, `Arg::Name` through its initialiser's call chain; the 64-prefix mount cap removed during the phase (it broke per-site identity). Files: `rb-frameworks/src/routes.rs`, `rb-indexer/src/{routes,lib}.rs`.
- [x] **3. Express chains** (S3; `PIPELINE m6b-2`). Done 2026-10-09 (amendment 4): `RouterDecl.binding` is `Option<usize>` (one consumer: the unknown-fallback identity, `{path}#default` when `None`); `router_kind` reads the first link only, so `const api = Router().use(…)` binds; every `use` link on a bound router's chains is a mount (single-link behaviour preserved); the anonymous default is matched by the new `default_export_expr` span fact against a Router-starting chain (exactly one default per module); unexported inline constructions bind nothing (no false endpoints from dead code); the default-import fallback resolves `None` to the `"default"`-named decl. Mutants M-c/M-d/M-e bite at unit and end-to-end level. Files: `rb-frameworks/src/routes.rs`, `rb-indexer/src/{routes,lib}.rs`, `rb-syntax/src/{imports,lib}.rs`.
- [x] **4. In-process + empty URL** (S4; `PIPELINE m6b-3`). Done 2026-10-10 (amendment 5): pack-side sink walk (`FileClients.in_process`: seeds, `ServeHTTP`/recorder-co-pass/mux-handler sinks, `Do` carve-out, depth 8 + visited, scope-indexed resolution); stitcher buckets (`unmatched` split into plain + non-zero-only `http_in_process`/`http_empty_url`, own diagnostic kinds, sum test extended); `rb status` labels; Service Card `unstitched`. Fixture per D10 (billing `main_test.go` + 3 must-not-exist guards; notifyPaid's edge intact). Mutants killed (visited gate, `Do`, `ServeHTTP`/recorder/mux, seed siblings, kind override, coverage split, empty check). Files: `rb-frameworks/src/clients.rs`, `rb-indexer/src/{stitch,lib}.rs`, `rb-cli/src/main.rs`, `rb-store/src/query.rs`.
- [x] **5. axios folding** (S5; `PIPELINE m6b-4`). Done 2026-10-10 (amendment 6): `create` contested via spreads (+ by-name object/string, Name kept otherwise); single-arg objects parsed as configs; request-config Names resolved (entries/string-kept/dynamic-forced); spread Names emitted for cross-file resolution; `fold::object_values` (unique M3a, visited + shared `MAX_SPREADS`); stitcher expands Names into `base_candidates` (D5 externality); Spring `of()` later-wins (span-ordered, M5 fallback on unreadable, heuristic-capped by carrier scan). Fixture per D10 (web `opts.ts` + `resendViaCfg`/`resendExternal`; 1 must-exist + 3 must-not-exist, all biting). Mutants killed (create-contested, config-entries, dynamic-forcing, spread-resolve/names, later-wins, contested-fallback, fold/stitch expansion, request-object). Files: `rb-frameworks/src/{clients,fold}.rs`, `rb-indexer/src/{stitch,lib}.rs`.
- [x] **6. Constructor members** (S6; `PIPELINE m6b-5`). Done 2026-10-10 (amendment 7): member-write facts for every form on any object (`on_this` flag; bare fields still out); per-file single-assignment proofs (class + same-file subclasses, assign-kill, shared `fold_init` allow-list, value folded at the write); reads via `this.`/member bindings (TS/JS/Java only); one shared `Folder` per `sites()` run (fresh step budget per fold). Fixture per D10 (web `NotifyClient`; 1 must-exist guard, biting). Mutants killed (single-count, on_this, ctor, assign-kill, subclass merge, no-memo growth, update arm). Files: `rb-syntax/src/{bindings,lib}.rs`, `rb-frameworks/src/fold.rs`, `rb-indexer/src/{stitch,lib}.rs`.
- [ ] **5. axios folding** (S5). Same-file spreads, unique cross-file constants, by-name configs, contested-external, ordered Spring statements.
- [ ] **6. Constructor members** (S6). Single-assignment proof + folding (TS/JS + Java).
- [x] **7. userinfo holes** (S7; `PIPELINE m6b-6`). Done 2026-10-10 (amendment 8): `strip_userinfo_hole` pre-pass in `client_url` (scheme-gated, last-`@`, hole-before-`@` only; credentials never stored); post-`@` FQDN is a normal host (stitcher host rule decides). Mutants killed (strip call at unit + e2e level; gate-drop equivalent, recorded). No fixture/userinfo shapes elsewhere: e2e byte-identical. Files: `rb-frameworks/src/endpoint.rs`, `rb-indexer/src/{stitch,lib}.rs` (stitch: one unit).
- [x] **8. Discovery narrowing** (S8; `PIPELINE m6b-7`). Done 2026-10-10 (amendment 9): `discovery_flags` (broad, narrowed); `discovery_resolved` (5-type allow-list, annotation/supertype/declared-type/return positions, explicit/on-demand/FQ, Java-gated); `single_literal_discovery` (linear brace stack + propagation); stitcher `discovery_broad` + `discovery_lifted` marker + non-zero-only coverage count. Tests first (3 compile-failures recorded); 24 mutants + quadratic rescan killed (693,100 → 11,503,300 vs 5×). Fixtures trigger-free: e2e byte-identical (0 lifts); repo calibration with phase 10. Files: `rb-frameworks/src/clients.rs`, `rb-indexer/src/{syntax,stitch,lib}.rs`.
- [x] **9. Client breadth** (S9; `PIPELINE m6b-8`). Done 2026-10-10 (amendment 10): got/ky/superagent/undici fetch-class (verbs, direct, namespace, carriers, members); HttpClient import-gated + always capped; axios member forms (new `Init::Member` fact, fold-hole) capped; imported-fallback exclusion; hooks (per-lib allow-lists, mentions taints, destructure allowances) + RAW_SURFACE words. Tests first (7 pack/hook/raw units + 1 syntax + 1 fold, all failed first); 29 mutants killed (2 survivors forced new rows). Fixture `clients.ts` (10 sites, 10+10 guards, all biting); goldens classified per hunk (25 sites; commit-sha-only downstream). Files: `rb-frameworks/src/{clients,fold}.rs`, `rb-syntax/src/{bindings,lib}.rs`.
- [x] **10. Measurements** (S11). Done 2026-10-10 (`var/m6b/bench/phase10.md`): `--no-scip --no-cache --force` with base `rb-3562b95` and head `rb-m6b-p9` (release, built this phase), one repo at a time, clones removed each, DB batch-removed + `VACUUM` (0 repos). gin: 19 unknown→26 composed, 44 unmatched+2 dynamic→33 in-process+18 self (sampled in source); express: 19 unknown→19 composed `/api` keys; ftgo: 4 dynamic→3 heuristic+1 ambiguous-over-3 (all read, genuine); mattermost: 8 ambiguous edges removed, 145 unmatched→106+32 in-process+7 empty, 123 dynamic→119+4 in-process (imageproxy prod sink read), empty 8/8 CSRF `""` (read); microservices + piggymetrics byte-identical. S8 calibration: `discovery_lifted` 0 on all six + fixtures (0 unintended lifts).
- [x] **11. Records + evaluator.** S12–S14; fixtures/goldens (S10) with the fixture phase; pass 1 (full), fixes, pass 2 (verification-only). Done 2026-10-10: pass 1 (`tasks/eval-m6b-pass1.md`, VERDICT PASS with fixes E1 reconfigured-caps + E2 ReactorLoadBalancer); pass 2 (`tasks/eval-m6b-pass2.md`, VERDICT PASS — 732/66/12/32/1 green, 58 mutants re-killed, no re-bless, E1/E2 trigger grep confirms phase-10 records). Contract amendment 11: **M6b closed.**

## M6c DI, RTA and orphan precision (contract `tasks/contract-m6c.md`, written 2026-10-10; forks F1–F7 recommendations for the orchestrator, confirmation at phase 0 start; not started)

Same rules as M6b:
- Each phase has at most 5 hand-edited source files. Fixture sources, blessed goldens and guard data are listed separately in the changelog and reviewed on their own.
- Each phase is test-first: the new units fail, or do not compile, before the change (the failure recorded).
- Each phase ends with its verification step: fmt, `clippy --workspace --all-targets -D warnings`, the touched crates' unit tests counted from a log holding both streams (`> log 2>&1`), and the e2e suite with `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb` (with containers wherever the phase touches SCIP: `docker image inspect` first, no `skipping:`).
- Any golden that changes gets a classified line diff before `RB_BLESS=1`; any dump change is classified L, T or W (contract invariant 1).
- Mutation runs follow M4a's harness rules: fresh mtime, restored bytes checked, a `test result:` line per mutant, stopped with SIGINT, `"$@"`, `command -v` for every tool.
- Environment: never `npm`/`npx` on the host (lockfiles are generated inside the pinned scip-typescript image); Docker context `desktop-linux`; no image pull (none is needed); `df` before every build, clone and container step, floor 6 GiB for large-repository steps and 2 GiB otherwise; one-off builds use a `CARGO_TARGET_DIR` under the scratchpad, removed afterwards; disk figures accounted part by part in MiB.

`PIPELINE` becomes `m6c-1` at the first dump change and is bumped on each later one.

- [x] **0. Preflight** (no code). Done 2026-10-10 (amendment 1): baseline binary `var/m6c/base/rb-10a9ad5` (sha256 `264fec11…`; HEAD code == `10a9ad5`); fixture dumps `--no-scip` identical to goldens where pinned (polyglot/lombok/workspaces), others recorded; SCIP hashes equal M6a's recorded values (e2e drives SCIP goldens); full-SHA pins by API (petclinic org corrected); shape checks on shallow checkouts, removed after reading (realworld Gradle → app-class signal; jhipster @Import/@Bean/@Qualifier, no tsconfig paths; microservices plugin + 8 app classes; eladmin/lombok.config vacuous; fixtures have no test dirs/paths/configs). Forks F1–F7 confirmed. Images verified; DB 0 repos; disk accounted.
- [x] **1. Registrants** (S1). Done 2026-10-10 (amendment 2): `bean_method_registrants` (import-named configs' assignable `@Bean` methods join as `origin: registrant`; Y4's residue closed, its test updated as predicted). Tests first (9 shape rows, failed first); 7 mutants killed. No fixture `@Import`-named configs: e2e byte-identical, no `PIPELINE` bump. Files: `rb-indexer/src/di.rs`.
- [x] **2. Gate M main-only** (S2). Done 2026-10-10 (amendment 3): test-path `ClassLiteral` mentions exempted from `mentioned` (D2); `mentioned_broad` + `gate_m_broad` mirror for the count; `mention_lifted` iff broad-caps-but-plain, non-zero-only in `di_coverage`. Tests first (4 rows incl. coverage asserts, failed first); S19g + S1 test-source rows updated to plain as predicted. 6 mutants killed + 1 documented arm-overlap. No fixture test dirs: e2e byte-identical, no `PIPELINE` bump. Files: `rb-indexer/src/di.rs`.
- [x] **3. Deployable roles** (S3). Done 2026-10-10 (amendment 4): `app_services` + single `deployable_services` predicate (roles, DI classpath, RTA). Tests first (flip + 4 controls, growth); 6 mutants killed. No fixture flips: e2e byte-identical. Files: `rb-indexer/src/lib.rs`.
- [x] **4. Module objects** (S4). Done 2026-10-10 (amendment 5): `rta::AliasMaps` (`paths` + `exports`/`main`, exact-beats-pattern, longest-wins, multi-target keep-all) shared by Tier B and RTA; `tsconfig.json` is a manifest input. Tests (Tier B unit, RTA exact/keep-all rows, precedence, growth, input pin); 6 lib mutants + build wiring e2e-killed. Fixture `legacy/` + 2 guards; goldens classified per hunk. Verify: fmt, clippy clean; 739 unit, 66 store, 12 server, 32/32 e2e (no `skipping:`), 1/1 api; final tree 256 + targeted e2e green. `PIPELINE m6c-1`. Files: `rb-indexer/src/rta.rs`, `modules.rs`, `resolve.rs`, `packages.rs`, `lib.rs`.
- [x] **5. Rule (e) levels** (S5). Done 2026-10-10 (amendment 6): `levels()` returns the winning `TypeLevel`; external only on ambiguous/miss, dotted-presence (unchanged), or unique on-demand + outside wildcard. Tests first (failed first); 4 predicted flips; growth test. 5 mutants killed (1 survivor gained its shape). No fixture flips: e2e byte-identical, no `PIPELINE` bump. Verify: fmt, clippy clean; 743 unit, 66 store, 12 server, 32/32 e2e (no `skipping:`), 1/1 api. Files: `rb-indexer/src/resolve.rs`, `liveness.rs`.
- [x] **6. Non-bean consumers** (S6). Done 2026-10-10 (amendment 7): `@Bean`-returned classes resolve per returning method, validators per class, always `heuristic`. Tests first; multi-candidate heuristic pin; fan-out bug fixed. 6 mutants killed. Fixture D10 (`JwtFilter`, `EmailValidator`, factory, validation dep) + 4 guards; goldens classified per hunk. Verify: fmt, clippy clean; 744 unit, 66 store, 12 server, 32/32 e2e (no `skipping:`), 1/1 api. `PIPELINE m6c-2`. Files: `rb-indexer/src/di.rs`, `lib.rs` + fixture/goldens.
- [x] **7. Inherited Spring Data** (S7). Done 2026-10-10 (amendment 8): allow-listed names on repository receivers → interface node + `inherited`, heuristic; Tier B doubles never; SCIP-exact files skipped. Tests + growth; 5 lib mutants + exact-gate e2e-killed. Fixture D10 (`SignupService`) + 3 guards; wiring bite proven. Goldens classified per hunk (impact-users affects 2→4 — the feature). `PIPELINE m6c-3`. Verify: fmt, clippy clean; 746 unit, 66 store, 12 server, 31/32 e2e + impact-users bless (no other fixture moved), 1/1 api. Files: `rb-indexer/src/di.rs`, `resolve.rs`, `lib.rs` + fixture/goldens.
- [x] **8. Lombok qualifiers** (S8). Done 2026-10-11 (amendment 9): `LombokConfigs` nearest-chain reader; listed qualifiers ride the parameter (`copied_qualifier`), counted in `lombok_narrowed`. Tests + growth (8 mutants killed; stop survivor forced `+=` shape). Fixture D10 (lombok dep, config, `MailService`) + 2 guards; goldens classified per hunk. Verify: fmt, clippy clean; 751 unit, 66 store, 12 server, 32/32 e2e (no `skipping:`), 1/1 api. `PIPELINE m6c-4`. Files: `rb-frameworks/src/di.rs`, `rb-indexer/src/di.rs`, `packages.rs`, `lib.rs` + fixture/goldens.
- [x] **9. Generics + custom qualifiers** (S9). Done 2026-10-11 (amendments 10–11): exact type-arg agreement picks single (capped, counted); custom-qualifier meta leg deferred as residue (nil incidence, needs syntax-visible annotation declarations; non-blocking per closing bar). Tests + growth (6 mutants killed; 2 redundant checks removed). Fixture D10 (`Channel.java`) + 2 guards; goldens classified per hunk. Verify: fmt, clippy clean; 753 unit, 66 store, 12 server, 32/32 e2e (no `skipping:`), 1/1 api. `PIPELINE m6c-5`. Files: `rb-syntax/src/bindings.rs`, `rb-frameworks/src/di.rs` + `clients.rs`, `rb-indexer/src/di.rs`, `lib.rs` + fixture/goldens.
- [x] **10. Measurements** (S11). Done 2026-10-11 (amendment 13): `--no-scip` base vs head on all six D13 repos; realworld +6 S6 INJECTS with S2/S6 bucket shifts, jhipster +119 S7/+14 S4 (root-tsconfig correction), petclinic/microservices/piggymetrics/eladmin S7-only; microservices 8 S3 role flips with no blast radius; RTA yield 0, `discovery_lifted` 0, S8 vacuous everywhere. Full record `var/m6c/bench/phase11.md`. DB 0 repos.
- [x] **11. Records + evaluator.** S12–S14; fixtures/goldens (S10) with the fixture phases; pass 1 DONE (amendment 14: VERDICT PASS, E1+E2 fixed test-first), pass 2 DONE (amendment 15: VERDICT PASS). **M6c closed 2026-10-11.**

## M6d Go and Python SCIP (contract `tasks/contract-m6d.md`, written 2026-10-11; forks F1–F6 recommendations for the orchestrator; not started)

Same rules as M6b/M6c:
- Each phase has at most 5 hand-edited source files. Fixture sources, blessed goldens and guard data are listed separately in the changelog and reviewed on their own.
- Each phase is test-first: the new units fail, or do not compile, before the change (the failure recorded).
- Each phase ends with its verification step: fmt, `clippy --workspace --all-targets -D warnings`, the touched crates' unit tests counted from a log holding both streams (`> log 2>&1`), and the e2e suite with `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb` (with containers wherever the phase touches SCIP: `docker image inspect` first, no `skipping:`).
- Any golden that changes gets a classified line diff before `RB_BLESS=1`; any dump change is classified L, T or W (contract invariant 1).
- Mutation runs follow M4a's harness rules: fresh mtime, restored bytes checked, a `test result:` line per mutant, stopped with SIGINT, `"$@"`, `command -v` for every tool.
- Environment: never `npm`/`npx`/`pip`/`go` on the host (lockfiles and environments are generated inside the pinned images); Docker context `desktop-linux`; no image pull except the measured scip-python pin (none is needed otherwise); `df` before every build, clone and container step, floor 6 GiB for large-repository steps and 2 GiB otherwise; one-off builds use a `CARGO_TARGET_DIR` under the scratchpad, removed afterwards; disk figures accounted part by part in MiB.

`PIPELINE` becomes `m6d-1` at the first dump change and is bumped on each later one.

- [x] **0. Preflight** (no code). Done 2026-10-11 (amendment 1): baseline binary `var/m6d/base/rb-027a54f`; full-SHA pins (mattermost Go fresh HEAD, Python `full-stack-fastapi-template` chosen with `uv.lock`); shape checks (go.mod collected, poetry/uv absent, no Go/Python images/jobs, egress npm/yarn/maven-only, grammars pinned, no client/route packs or fixtures); scip-python 188.8 MB compressed (3× bound fits); fixture dep versions read; Go stable go1.27.2; egress baseline 61/61; e2e baseline 8/8. DB 0 repos.
- [x] **1. scip-go image** (S1). Done 2026-10-11 (amendment 2): `docker/scip-go/` (Ubuntu pinned, Go 1.27.2 checksummed, scip-go v0.2.7 source-built at pinned commit); `rb-scip-go:0.2.7` built locally (644 MB, versions verified); jobs/wiring/`m6d-go-1` recipe; tests first + 4 mutants killed; stdlib smoke indexes offline. Files: `docker/scip-go/`, `rb-scip/src/jobs.rs`, `orchestrate.rs`, `rb-indexer/src/projects.rs`.
- [x] **2. scip-python pin** (S2). Done 2026-10-11 (amendment 3): v0.6.6 digest-pinned (188.8 MB compressed, 731.6 MB actual, fits); `Indexer::ScipPython`, per-manifest jobs (`Fetch::Python`, unpinned), `m6d-py-1`, empty fetch (S4 fills), offline index, emulated platform. Tests first + 4 mutants killed; smoke indexes exit 0 (needs project metadata — planning stays gated). Files: `rb-scip/src/jobs.rs`, `orchestrate.rs`, `rb-indexer/src/projects.rs`.
- [x] **3. Go fetch** (S3). Done 2026-10-11 (amendment 4): `proxy.golang.org` allow-listed; fetch pins `GOSUMDB=off`; e2e probes extended (gomod allowed, gosum refused). Tests first + 2 mutants killed; chi module fetches + indexes end-to-end. No dump change. Files: `rb-scip/src/egress.rs`, `orchestrate.rs`, `hash.rs`, `rb-cli/tests/e2e.rs`.
- [x] **4. Python fetch** (S4). Done 2026-10-11 (amendment 5): `Fetch::Python` carries `PythonLock` (poetry.lock → locked tree, requirements.txt → pins only, else project only; `RECIPE_PY m6d-py-2`); `pypi.org` + `files.pythonhosted.org` allow-listed; e2e probes extended; tests first + 8 mutants killed; scratch repo fetches + indexes `scip-python ok` end-to-end, refetch byte-identical. No Python golden moves. Repaired pre-existing phase-1 debt found in verification: `--all-targets` build break (`Images.go` in `main.rs` test) + red polyglot goldens (billing Go job never re-blessed; 4 goldens re-blessed, hunks classified). Verify: fmt/clippy clean; 733 unit + 25 cli-bin; 32/32 e2e (no `skipping:`) + 1/1 api; DB 0 repos. Files: `rb-scip/src/egress.rs`, `jobs.rs`, `orchestrate.rs`, `hash.rs`, `rb-cli/tests/e2e.rs` (+ `rb-cli/src/main.rs` repair, + 4 blessed goldens).
- [x] **5. Go package keys** (S5). Done 2026-10-11 (amendment 6): `package()` normalises scip-go `gomod`→`go` (measured gin/stdlib/own forms; names/versions pass through; `?`/mismatch keys apart per D3); export evidence review closes amendments 2–3 (no code change); tests first + 5 mutants killed; e2e pin in `scip_polyglot_monorepo` (shared gin node, both provenances; no golden moved). Verify: fmt/clippy clean; 735 unit + 25 cli-bin; polyglot + 28 e2e green; DB 0 repos. Files: `rb-scip/src/symbol.rs`, `rb-cli/tests/e2e.rs`.
- [x] **6. Lockfile versions** (S6). Done 2026-10-11 (amendment 7): PEP 508 + poetry-table readers, poetry/uv lock readers (registry sources pin, rest `?`), PEP 503 lookup, workspace-aware governance; manager `py`. Tests first + 10 mutants killed; scratch end-to-end (exact pins, diagnostics, spans) + green structure goldens; no golden moves. Files: `rb-indexer/src/packages.rs`, `rb-indexer/Cargo.toml` (+ lock edge).
- [x] **7. requests/httpx** (S7). Done 2026-10-11 (amendment 8): fetch-class sites (module/dispatcher/session/inline, shared verbs, base_url fold, carrier caps, per-lib kwarg allow-lists); kwarg extractor support + lockfile inertness. Tests first + 10 mutants killed; mixed scratch stitches 4 sites into 2 CONSUMES edges; no golden moves. Files: `rb-frameworks/src/clients.rs`, `rb-syntax/src/calls.rs`.
- [x] **8. Python member idioms** (S8). Done 2026-10-11 (amendment 9): self/cls member receivers (construction-first, annotation fallback, conflict-dynamic) + session-typed locals; all capped. Tests first + 9 mutants killed; mixed scratch stitches member/typed sites; no golden moves. Files: `rb-frameworks/src/clients.rs`.
- [x] **9. Fixtures + guards** (S9). Done 2026-10-11 (amendments 10–11): chi/FastAPI packs (9a) + fixtures with per-class guards, IMPLEMENTS join branch, e2e fns + goldens (9b). Tests first + 15 mutants killed; full suite green. Files: `rb-frameworks/src/routes.rs`, `rb-indexer/src/scip.rs`, `rb-cli/tests/e2e.rs` + fixtures/goldens.
  - [x] **9a. chi + FastAPI route packs.** Done 2026-10-11 (amendment 10): chi verbs/nested-Route/Group/With/Method/Handle + FastAPI decorators/prefixes/includes; tests first + 11 mutants killed. Files: `rb-frameworks/src/routes.rs` (+ 1-line `kwarg` share in `clients.rs`).
- [x] **10. Egress review** (S10). Done 2026-10-11 (amendment 12): probe suite re-run green on the final 7-host allow-list; per-tool posture recorded; verdict no finding, no code change.
- [x] **11. Measurements** (S11). Done 2026-10-11 (amendment 13, record `var/m6d/bench/phase11.md`): --no-scip both binaries both repos (identical graphs, classified +W-only diffs, read samples); scip-go 6/9 ok + scip-python ok with two measurement-driven fetch fixes (GCS redirect host, requires-python flag + recipe bump); classified SCIP diffs + samples; deterministic. Files: `rb-scip/src/egress.rs`, `hash.rs`, `orchestrate.rs`, `rb-cli/tests/e2e.rs`.
- [x] **12. Records + evaluator.** S12–S14; pass 1 (full), fixes, pass 2 (verification-only).
  - [x] **Pass 1 + fixes.** Done 2026-10-11 (amendment 14): 3 parallel evaluators, 2 blocking + 9 non-blocking findings; blockers fixed test-first (tool_versions identities, absolute replace) + claim-breaking hardening (chi Handle, double-include expansion, dispatcher kwargs, gomod split pin, GOTOOLCHAIN env); rest recorded with reasons.
  - [x] **Phase 12 (records + pass 2).** Done 2026-10-11 (amendment 15): S13 growth audit found chi (15.85×) and FastAPI (18×) superlinear — fixed test-first (per-file function sweep `FuncCache`/`ChiState`; adjacency + memoised levels + cycle gate; 4 ratio tests + 1 cycle pin, 6/6 mutants); requests/httpx + member packs linear as built; same-name shadowing cost recorded. `PIP_TIMEOUT=60` with `RECIPE_PY m6d-py-4` (measured pip read-timeout flake). Contention incident recovered (2 p12 e2e failures from parallel builds; poisoned scip row overwritten uncontended; both re-green). Invariant 1 vs `027a54f`: 4/7 identical, rest added-only W + 1 explained shift; two-name determinism. Pass 2 (`tasks/eval-m6d-pass2.md`): VERDICT PASS (suite 765+25/66/12/34/150, 22/22 mutants, no `skipping:`). **M6d closed 2026-10-11.**

## M6e Build and lockfile breadth (contract `tasks/contract-m6e.md`, written 2026-10-11; forks F1–F6 decided with reasons; started 2026-10-11)

Same rules as M6d:
- Each phase has at most 5 hand-edited source files. Fixture sources, blessed goldens and guard data are listed separately in the changelog and reviewed on their own.
- Each phase is test-first: the new units fail, or do not compile, before the change (the failure recorded).
- Each phase ends with its verification step: fmt, `clippy --workspace --all-targets -D warnings`, the touched crates' unit tests counted from a log holding both streams (`> log 2>&1`), and the e2e suite with `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb` (with containers wherever the phase touches SCIP: `docker image inspect` first, no `skipping:`).
- Any golden that changes gets a classified line diff before `RB_BLESS=1`; any dump change is classified L, T or W (contract invariant 1).
- Mutation runs follow M4a's harness rules: fresh mtime, restored bytes checked, a `test result:` line per mutant, stopped with SIGINT, `"$@"`, `command -v` for every tool.
- Environment: never `npm`/`npx`/`pip`/`go`/`gradle` on the host (lockfiles and environments are generated inside the pinned images); Docker context `desktop-linux`; no image pull except the measured Gradle distribution (none is needed otherwise); `df` before every build, clone and container step, floor 6 GiB for large-repository steps and 2 GiB otherwise; one-off builds use a `CARGO_TARGET_DIR` under the scratchpad, removed afterwards; disk figures accounted part by part in MiB; no builds during e2e container phases.

`PIPELINE` becomes `m6e-1` at the first dump change and is bumped on each later one.

- [x] **0. Preflight** (no code). Done 2026-10-11 (amendment 1): baseline binary `var/m6e/base/rb-de4f5c1` (sha256 recorded; HEAD code == `de4f5c1`); fixture dumps with it, `--no-scip`, all non-empty; full-SHA pins (jcommander Gradle-KTS, ofetch pnpm-lock); yarn-classic/go.work public samples: none in reach (recorded gap); shape checks (Gradle detection/unsupported, Maven-only image, 8-host egress, npm/go/pom-only versions, npm+Yarn-v1 grouping, no exclude/go.work/classifier/include readers; fixtures N3-clean); scip-java Gradle interface from primary docs + `--help` (`--build-tool=<gradle>`, default `clean compileTestJava …`, detection files); Gradle 8.14.6 pin + dist sha (8.x line over 9.x, recorded reason); image tooling (yarn 1.22.22 classic → Berry versions-only; pnpm 10.17.1); fixture dep versions read (commons-lang3 3.20.0, semver 7.8.5, is-odd 3.0.1); Maven encoding precedence from plugin docs with 2 points flagged for S11 measurement; images verified; DB 3 pre-existing + 7 `t_p0_*` (auto-cleaned next e2e); disk 11G free.
- [x] **1. Gradle image** (S1). Done 2026-10-11 (amendment 2): Dockerfile Gradle 8.14.6 (pinned dist + sha, `jar xf` + `chmod 0755`, `GRADLE_HOME`/PATH); image rebuilt locally as `rb-scip-java:0.13.1` (ID `cbd99634…`→`f9612a94…`, ≈1.23 GB); in-image `gradle --version` 8.14.6 + `mvn` unchanged; no code change, no mutants (image-only). Files: `docker/scip-java/`.
- [x] **2. Gradle fetch + index** (S2). Done 2026-10-11 (amendment 3): `RECIPE_GRADLE m6e-gradle-1`, job-keyed `recipe()`, `gradle_runs()` (resolve + online build behind the proxy, offline `--build-tool Gradle` index), per-repo `gradle` cache, per-run `GRADLE_OPTS` proxy properties, `plugins.gradle.org` allow-listed (`services` refused by design); smoke proves exact edges + determinism end-to-end. Tests first + 6 mutants killed; e2e 34/34, api 1/1. Files: `rb-scip/src/{orchestrate.rs,egress.rs,hash.rs}`, `rb-indexer/src/projects.rs`, `rb-cli/tests/e2e.rs` (probes).
- [x] **3. Gradle versions** (S3). Done 2026-10-11 (amendment 4): catalog + both-DSL literal readers, ancestor governance, Maven-mirroring certainty/duplicates/scopes; project refs + multi-line + buildSrc recorded out; trailing-boundary check proven redundant and removed. Tests first + 12 mutants killed; no dump change, no PIPELINE bump. Files: `rb-indexer/src/packages.rs`.
- [x] **4. Gradle fixture + guards** (S4). Done 2026-10-12 (amendment 5): `java-gradle-catalog` (Kotlin DSL + catalog with `version.ref`, literal gson, test junit; call graph with exact edges), per-class guards, e2e fn + goldens (bless-2 green after fixing dotted-`version.ref` resolution the units missed); 2 e2e guard-bite mutants. Files: `fixtures/java-gradle-catalog/` + goldens/guards, `rb-cli/tests/e2e.rs` (+ `fixtures/README.md`).
- [x] **5. pnpm/yarn lockfile readers** (S5). Done 2026-10-12 (amendment 6): hand-rolled yarn v1/Berry + pnpm v9 readers, `LockKind` governance (npm > yarn > pnpm), Exact pins + `@?` duplicates + missing-entry diagnostics; `lockfile_unread` producer retired (kind survives). Tests first + 12 mutants killed; no dump change, no PIPELINE bump. Files: `rb-indexer/src/packages.rs`.
- [x] **6. pnpm workspaces + yarn links** (S6). Done 2026-10-12 (amendment 7): pnpm-workspace.yaml globs into shared grouping (Pnpm lock), yarn file:/link: targets via shared resolve_link, Berry jobs unsupported; workspace_lockfile_unsupported retired. Tests + 6 mutants killed; no dump change. Files: `rb-scip/src/{jobs,orchestrate}.rs`, `rb-indexer/src/projects.rs`.
- [x] **7. TS lockfile fixture** (S7). Done 2026-10-12 (amendment 8): `ts-pnpm-yarn` (pnpm workspace + classic yarn, caret ranges, generated lockfiles), per-format version guards, e2e fn + goldens (blessed + reviewed, green unblessed); 2 e2e guard-bite mutants. Files: `fixtures/ts-pnpm-yarn/` + goldens/guards, `rb-cli/tests/e2e.rs` (+ `fixtures/README.md`).
- [x] **8. go.work + exclude** (S8). Done 2026-10-12 (amendment 9): `go.work` reader (single/block `use` + `replace` + `exclude`, `go`/`toolchain`/garbage ignored) + `exclude` in `go.mod`; nearest-`go.work` governance; workspace replaces win over module replaces (joined at the workspace root, not the member dir); workspace `use` members join services by module path (merged with project modules, deduped); excluded required versions fail safe to `@?` exact `unresolved` + `go_excluded` diagnostic. Tests first (did not compile before the readers); 2 targeted mutants killed (workspace-win dropped, exclude dropped). No fixture carries `go.work`: 36/36 e2e green unblessed, no dump change, no `PIPELINE` bump. Files: `rb-indexer/src/packages.rs`.
- [x] **9. Maven classifiers** (S9). Done 2026-10-12 (amendment 10): `<classifier>` rides the key, duplicate rule per full key, `<type>` recorded; tests first + mutants; full verify green unblessed, no dump change.
- [x] **10. tsconfig scoping + encodings + N3** (S10–S12). Done 2026-10-12 (amendment 11): `TsScopes` reader (allow-list globs, bounded extends, unread counted, fail-open) partitioning `files_without_document` with no edge moves; per-document five-source effective encodings (D9 corrected by measurement: pom beats compilerArgs both directions); N3 `restructured` rejection after 0-calibration. Tests first + mutants + growth + scratch measurement; full verify green unblessed, no dump change.
- [x] **11. Measurements** (S13). Done 2026-10-12 (amendment 12): jcommander --no-scip identical + deterministic, Gradle SCIP ok 45.3s (1,218/1,218 defs, 1,355 exact CALLS, 20/20 samples, cached identical) after allow-listing plugins-artifacts (probed 200, key moved by design); ofetch 12 pnpm pins classified + sampled + deterministic; class-validator S10 live-fire 42 = 38 + 4; cleanup done.
- [x] **12. Records + evaluator.** Done 2026-10-12 (amendment 13): pass 1 full review (12 probes, 5 new tests, VERDICT PASS, no blockers); pass 2 verification-only (VERDICT PASS, close verify green). **M6e closed.**

## Later M6 sub-milestones (not started; scope and exit criteria in design §13)

- [x] M6b Request-path precision (closed 2026-10-10, amendment 11; passes 1–2 PASS).
- [x] M6c DI, RTA and orphan precision (closed 2026-10-11, amendments 1–15; passes 1–2 PASS).
- [x] M6d Go and Python SCIP (scip-go image built locally, scip-python pinned by digest after measuring, fetch phases behind the proxy, Go/Python fixtures, the adversarial egress review).
- [x] M6e Build and lockfile breadth (closed 2026-10-12, amendment 13; passes 1–2 PASS).
- [x] **0. Preflight** (M6f). Done 2026-10-12 (amendment 1): baseline binary `var/m6f/base/rb-376dbcb` (HEAD code == `376dbcb`); pins grpc-go + OAI spec repo (ls-remote + API listings); tree-sitter-proto binds (node kinds confirmed, stream needs text check); shape checks; baseline verify green (fmt, clippy, 822 units, store 66, server 12, e2e 36/36, api 1/1).
- [x] **1. OpenAPI reader** (S1). Done (amendment 2): JSON + YAML shapes, basenames, growth; mutants.
- [x] **2. Proto reader** (S2). Done (amendment 3): tree-sitter-proto services/RPCs/streams/spans; mutants; growth.
- [x] **3. Spec endpoints in graph** (S3). Done (amendment 4): shared-key registrations, GRPC keys, diagnostics; mutants.
- [x] **4. MyBatis mapper XML** (S5). Done (amendment 5): Element attrs/ends, statements→tables, dynamics counted; mutants.
- [x] **5. Generated-client packs** (S6 packs). Done (amendment 6): operationId/RPC candidates + Endpoint.operations; mutants; growth.
- [x] **6. Generated-call stitching** (S6 stitch). Done (amendment 7): operation/RPC matching, own-service rule, synthetic cap; mutants.
- [x] **S4b. Interface handler roll-down** (S4). Done (amendment 8): interface_routes recorded, spec-matched roll-down via IMPLEMENTS; mutants.
- [x] **7. Fixture + guards** (S7). Done (amendment 9): contract-packs fixture (Maven + nested webapp), e2e + 5 goldens + 12/3 guards, every line read, 4 guard-bite mutants, unblessed green.
- [x] **8. Measurements** (S8). Done (amendment 10): grpc-go classified (18 endpoints, 52 synthetic CONSUMES sampled, 869 correctly-unmatched residue) + deterministic; OAI identical + reader breadth on real specs; cleanup done.
- [x] **9. Records + evaluator.** Done 2026-10-12 (amendment 11): pass 1 (12 probes, dispatch fix + churn fix, VERDICT PASS); pass 2 (VERDICT PASS, close verify green). **M6f closed.**
- [x] **0. Preflight** (M6g). Done 2026-10-12 (amendment 1): baseline binary `var/m6g/base/rb-f80714d` (HEAD code == `f80714d`); pins apache/kafka + spring-cloud-gateway (ls-remote); shape checks; baseline verify green (fmt, clippy, 842 units, store 66, server 12, e2e 37/37, api 1/1).
- [x] **1. Topic packs** (S1). Done (amendment 2): Kafka/Rabbit listeners + sends, literals/placeholders/dynamics; mutants; growth.
- [x] **2. Topic graph** (S2). Done (amendment 3): Topic/PUBLISHES/SUBSCRIBES kinds, stage wired, placeholder resolution; mutants.
- [ ] M6g phases 3+ (gateway packs, host names, Spring YAML, fixture, measurements, evaluator).
- [ ] M6h Architecture rules as a CI gate (`rb check` in Cypher).
- [x] M6i Kotlin and C# syntax (closed 2026-10-12, amendments 1–8; passes 1–2 PASS).
- [x] M6j DI breadth (closed 2026-10-12, amendments 1–11; passes 1–2 PASS).
- [x] M6k DI recall (closed 2026-10-12, amendments 1–10; passes 1–2 PASS).
- [x] M6l XML diagnostic siting (closed 2026-10-13, amendments 1–3; passes 1–2 PASS).
- Deferred beyond M6: scip-clang, `rust-analyzer scip`, scip-dotnet, the SCIP incremental below a single-root reactor, authentication/remote binding/`POST …/index` and move detection (only if asked for). Dropped: Groovy registrants.

---

# M7a: `exact`-label correctness (stress batch A), started 2026-10-13

Contract: `tasks/contract-m7a.md`. Findings: `tasks/stress-findings.md` (F17, F18, F19).

- [x] Phase 1 (S1/F19): Go/TS gRPC client typing. Constructor must be the last link; the `New*Client(c).Rpc` chain read as an RPC. The test failed first (getters reported, real calls missed). Mutants m1–m3 killed.
- [x] Phase 2 (S2/F18): FastAPI through the shared router composer (`RouterDecl`/`Mount`), plus Python submodule mount targets and Python top-level `Constant` routers. The indexer test failed first (`DELETE /{}` merged, all exact). The 3 pack tests now pin per-file facts, and in-file composition is pinned at indexer level. Mutants m4–m7 killed.
- [x] Phase 3 (S3/F17): gRPC server registrations (5 languages), registration-based attribution, `grpc_implementer_unknown` and `grpc_registration_ambiguous`. Tests failed first (copies exposed everything). Mutants m8–m12 killed; m9 first survived, and the test gained a one-copy-implemented-elsewhere case.
- [x] Phase 4: PIPELINE `m7a-1`; fmt and clippy clean. DB suites: 980 pass, 2 fail (pre-existing F1). Invariant: 29/31 identical, with 2 classified (microservices-demo, fastapi-realworld). The mattermost false positives were caught and fixed by tightening to the two-link shape. Repros re-run; DB cleaned.

## Review (M7a)
- The three S1 `exact`-label errors from the stress campaign are fixed, tests-first, with 14 mutants killed. All 18 fixture dumps are byte-identical.
- Residue recorded in the contract: inner prefixes are lost under an unknown top prefix; gRPC handler methods aren't linked; helper-registered health isn't read.
- Not done: commit, and an evaluator pass.

---

# M7b: regressions and hangs (stress batch B), started 2026-10-13

Contract: `tasks/contract-m7b.md`. Branch `m7b-regressions-and-hangs` (stacked on M7a).

- [x] Phase 1 (S1/F1): `rb status` without `--repo` lists all repositories outside an indexed checkout. The two red tests (e2e `json_answers…`, api) now pass. README and testing guide updated.
- [x] Phase 2 (S2/F2): `bounded_output` for every Docker CLI call (probe 10 s, others 60 s), killed and reaped. Test with a hanging fake (bounds 1 s, an order of magnitude above process start, after a flake at 300 ms). Mutants m15/m16 killed. Against the real wedged Docker Desktop, `rb index` finishes in 12 s (was an indefinite hang).
- [x] Phase 3 (S3/F9/F10): a `Parents` map (one walk) replaces `Node::parent()` in the profiled hot spots only: `mentions` (es/java role), `call_qualifier`, `is_inner_link`, `assignment_target`. Each has a killing mutant (m17–m20, m23). `local_by_ancestry` and `type_qualifier` were converted, then reverted when their mutants (m21, m22) showed they are not hot. The timing test (7 pathological cases) runs in about 4.7 s in a debug build. The 1,500-link JS chain took 365 s in debug before. A probe also found a 3,000-segment member path at 85 s (release), now under 1 s.
- [x] Phase 4 (S4/F11): `Composer::prefixes` explored every simple path of a mount cycle (unclean results are never memoised). It now stops once the known prefixes are capped and the result is unknown. Counted work at 6→12 routers went from 123× growth to within 4×, with endpoints pinned (365/725). On the F11 file: 44 s → 1 s, dumps byte-identical. Mutant m24 killed.
- [x] Phase 5 (S5/F15): `find_header` for the size, and the head is streamed through `git cat-file` for blobs over 64 MiB (full load as fallback). The manifest is identical with streaming forced on and off (sniff-edge fixture). Mutant m25 killed. 1 GiB blob: 2,063 MiB → 16 MiB peak RSS, 2.66 s → 1.30 s.
- [x] Phase 6: fmt and clippy clean. DB suites 986/0, no skips. Invariant against M7a 31/31 identical. Repros: F1 lists all repos; F2 12 s on real wedged Docker; F9 chain 4k 3 s (was >300 s); F10 nested 20k under 1 s; F11 1 s (was 44 s); F15 16 MiB (was 2 GiB). DB cleaned.

## Review (M7b)
- Batch B is complete. Both suites are green with the DB and without skips. Every hang and blow-up found by the stress campaign is now bounded.
- Residue: role walks are O(n·depth) on one long chain (3 s at 4k links); blobs of 64 MiB and under are still loaded to sniff.
