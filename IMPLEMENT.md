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
