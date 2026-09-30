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
