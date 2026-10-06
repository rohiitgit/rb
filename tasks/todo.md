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
- **M4b, DI and dispatch** (Spring DI, Spring Data beans, RTA, entry points, orphans, Q3 roll-up, package versions and service dependencies, full Service Card): contract `tasks/contract-m4b.md` (S1–S28, soundness claims and closing bar up front, forks F1–F6) written 2026-10-06; not started. Of the M4a open items it takes 1, 5, 7, 9, 15 and 24; item 2 moves to M6.

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
- [ ] **5. RTA proof at index time** (S9).
  - *Derived:* proven types; `impl_instantiated: false` on their `IMPLEMENTS` edges; `rta_coverage`; the `rta:` line.
  - *Invariants:* the edge set and certainties are unchanged (dump diff against the phase-0 baseline shows attributes only); one keeping row per rule; subtype propagation tested at depth 1/2 and in reverse; a cycle test; counted growth test.
  - Files: new `rb-indexer/src/rta.rs`, `rb-indexer/src/lib.rs`, the `IMPLEMENTS` producers' attribute hook (`resolve.rs`/`scip.rs`, or one `GraphBuilder` call in `rb-model/src/graph.rs`: D4, the generator's choice recorded), `rb-cli/src/main.rs`.
  - **Verify:** units; e2e churn classified (expected: no fixture type proven before the fixture phases, so dumps change only in the new coverage row).
- [ ] **6. Precision at query time** (S10).
  - *Derived (query time):* filtered dispatch steps; `pruned_by_rta`; the `precision=` header.
  - *Invariants:* the defaults (Q1/Q4 `rta`, Q2/Q3 `cha`); every pruned step counted; same graph under both modes in one unit.
  - Files: `rb-store/src/code.rs`, `rb-store/src/path.rs`, `rb-cli/src/main.rs` (flags), `rb-cli/src/code.rs`, `rb-cli/src/path.rs`.
  - **Verify:** units; e2e (headers change in callers and path goldens: classified, blessed).
- [ ] **7. Entry kinds and orphan eligibility at index time** (S11 stored kinds, S12).
  - *Derived:* `Symbol` attributes `entry` and `orphan_eligible`; `orphan_coverage`; the uncached word scan.
  - *Invariants:* import-gated kinds with decoys; one keeping row per eligibility rule; scan cost measured on the public repositories in phase 15; counted growth test.
  - Files: new `rb-indexer/src/liveness.rs`, `rb-indexer/src/syntax.rs` (attributes), `rb-indexer/src/lib.rs`.
  - **Verify:** units; e2e churn (symbol attributes) classified.
- [ ] **8. Entry marks, orphans, tests and item 7 in the questions** (S11 query side, S13).
  - *Derived (query time):* `(entry: …)` marks; the orphan reachability; `tests (n)`; endpoints listed once; the router shown for prefix-unknown endpoints; `rb impact --json`.
  - *Invariants:* the greatest-fixpoint formulation (dead cycle orphaned; a cycle with an outside caller live); dispatch liveness; library services not computed; 5,000-deep chain; counted growth.
  - Files: `rb-store/src/code.rs`, `rb-store/src/path.rs` (header), `rb-cli/src/code.rs`, `rb-cli/src/main.rs`, `rb-cli/src/path.rs`.
  - **Verify:** units; e2e (impact and callers goldens re-blessed after classification).
- [ ] **9. Q3 roll-up** (S14).
  - *Derived (query time):* callers up to entry points; per-service ops, direct or indirect, endpoints (span-scoped), entry kinds; "through APIs" up to `--hops`; JSON `services`, `through_apis`.
  - *Invariants:* default `cha`; visited sets on nodes and endpoints; `CONSUMES` A ↔ B cycle test; counted growth.
  - Files: `rb-store/src/query.rs`, `rb-store/src/code.rs` (shared walk), `rb-store/src/path.rs` (upstream continuation reused, not duplicated), `rb-cli/src/main.rs`.
  - **Verify:** units; e2e (`table-deps-*` re-blessed; the "arrive with call resolution (M3)" line gone).
- [ ] **10. Service dependencies and the full card** (S17, S18).
  - *Derived (query time):* `via=calls`/`via=http`/`via=package`; depended on by; shares tables; tables; call-graph summary; entry points by kind; JSON fields.
  - *Invariants:* bytewise order; a self-call excluded; two vias between one pair both shown.
  - Files: `rb-store/src/query.rs`, `rb-cli/src/main.rs` (`render_card`).
  - **Verify:** units; e2e (every card re-blessed after classification).
- [ ] **11. M4a items 1, 9, 15** (S19–S21).
  - *Derived:* default-import mounts composed; catch-all candidates restricted to named hosts; DI-resolved client bases (`base_from_bean`).
  - *Invariants:* each can only lower or remove (`CONSUMES` diff against the baseline classified); M4a's `synthetic` bar untouched (the existing never-`synthetic` guards still green).
  - Files: `rb-frameworks/src/routes.rs`, `rb-indexer/src/routes.rs`, `rb-indexer/src/stitch.rs`, `rb-indexer/src/di.rs` (base lookup for S21).
  - **Verify:** units; e2e unchanged until the fixture phase.
- [ ] **12a. Fixtures: java-spring-jpa, part 1.** New `AuthApplication.java`, `Hashing.java`, `Tokens.java`, `AuditConfig.java`, `Notifiers.java`; `git add`.
  - **Verify:** e2e; predict every new `INJECTS`, `impl_instantiated` and coverage line before blessing.
- [ ] **12b. Fixtures: java-spring-jpa, part 2.** New `UserRepositoryCustomImpl.java`, `support/Service.java`, `support/ReportNotifier.java`; edits to `UserRepository.java` and `AuthController.java`.
  - **Verify:** as 12a.
- [ ] **12c. Fixtures: java-spring-jpa, part 3, and polyglot auth.** Edits to `DefaultAuthService.java` and `LegacyAuth.java`; polyglot new `AuthApplication.java`; edits to `UserDao.java` and `AuthController.java`.
  - **Verify:** as 12a; the `rb path` login goldens gain `QUERIES public.users`/`audit.users` (classified).
- [ ] **12d. Fixtures: polyglot billing and notification, ts-express-prisma.** Edits to `services/billing/internal/invoice.go` and `services/notification/src/selfcheck.ts`; new `src/routes/health.ts`; edit to `src/app.ts`.
  - **Verify:** as 12a.
- [ ] **13. Goldens and precision guards** (S22, S24, S26).
  - *State:* the guard files (data beside the goldens).
  - *Invariants:* every guard line and dump check bitten by a recorded mutation (item 5's own-service mutation included; the dynamic-URL check settled as equivalent or bitten, with the reasoning); producer coverage for `INJECTS` and both `DEPENDS_ON` forms; the invariant-1 check against the phase-0 baseline; AGE parity for `INJECTS` and Service → Service `DEPENDS_ON`.
  - Files: `rb-cli/tests/e2e.rs`, plus the six guard files.
  - **Verify:** e2e twice; determinism (two repos, forced rebuild, warm = cold).
- [ ] **14. SCIP mode** (S25).
  - Before the run: `df`, the images and the context.
  - *Derived:* `scip-injects.txt` (the `synthetic` row end to end).
  - *Invariants:* no `skipping:`; fresh = cached; every SCIP golden diff classified.
  - Files: `rb-cli/tests/e2e.rs`.
  - **Verify:** SCIP e2e with containers, twice.
- [ ] **15. Public repositories** (S27).
  - Clone into the scratchpad (or `var/m4b/` if the orchestrator says so).
  - Run `--no-scip` fresh and cached; run SCIP for (a) and (b).
  - Compute every count with a script from the dumps; identify one member per non-zero bucket.
  - Diff against the phase-0 baseline dumps and classify.
  - Hand-check the samples (`INJECTS` 20, all proven types or 20, 10 `rb impact` orphan runs, 20 package edges, 2 tables per Java repository).
  - Record the wall time and facts-cache size against `28e47ba`.
  - **Verify:** numbers recorded in the contract's measurement section, computed by a tool; clones and `m4b_*` repositories removed afterwards.
- [ ] **16. Records and review** (S28).
  - Records: contract amendments, measurement and open items; design §5.5/§6.2/§6.4/§8.5/§9.2–§9.5/§10.3/§13; README; IMPLEMENT.md; CHANGELOG; lessons.
  - Full verification loop and the stub scan.
  - Evaluator pass 1 (full review); fix its findings test-first, one amendment each.
  - Evaluator pass 2 (verification only, per the contract's closing bar); new attack forms in residue classes go to M5's adversarial review, not the verdict.
  - Commit after each verified phase.
