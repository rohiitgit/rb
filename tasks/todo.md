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
