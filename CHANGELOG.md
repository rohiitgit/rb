# Changelog

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
