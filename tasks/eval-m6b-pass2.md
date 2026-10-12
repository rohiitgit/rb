# M6b evaluator pass 2 (verification-only)

- **Date:** 2026-10-10 (wall clock)
- **Commit:** pass-1 fixes E1, E2 applied (tree otherwise identical to `4983f62` + `b74da03`); no code change during this pass.
- **Environment / images / database:** as pass 1 (Docker context `desktop-linux`; DB `postgresql://rb:rb@127.0.0.1:55433/rb`, 0 repos at start and end; disk 18–19G free, floor never approached).

## Re-runs (all green, no re-bless — every golden stands as classified)

| Check | Result | Log |
|---|---|---|
| `cargo fmt --all --check` | clean | — |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean (1 needless-borrow fixed during the pass: `plugin(&chain)` → `plugin(chain)`, then clean) | — |
| Workspace units (awk-summed, both streams) | **732 passed, 0 failed** | `var/m6b/logs/p11-unit.log` |
| `rb-store` + `rb-server` with DB | **78 passed** (66 + 12), no `skipping:` | `p11-store-db.log`, `p11-server-db.log` |
| e2e with containers | **32/32**, no `skipping:` | `p11-e2e.log` |
| API suite | **1/1** | `p11-api.log` |
| Finding repros (E1 unit, E2 reactor rows) + controls (same-package, non-binding-import, hand-rolled, literal-@, instance-destructure) | all green inside the 732 | — |
| Mutants | p8 24/24, p9 29/29, p11 5/5 killed, all restored identical (`p8-mutants.log`, `p9-mutants.log`, `p11-mutants.log`; one stale patch string repaired mid-pass: `carrier-cap-dropped` after the collapsible-if refactor, then killed) | logs |
| Invariant 1 (fixtures) | e2e green with no re-bless on the fixed tree: dumps identical to the classified phase-9 state, which is classified per hunk vs `3562b95` through phases 4–9 | — |
| Invariant 1 (repos) | E1/E2 trigger grep over re-cloned express/ftgo/microservices/piggymetrics at their pins (gin Go-only, mattermost all-capped/Go): no trigger shapes anywhere — phase-10 records stand byte-for-byte | pass 1 record |
| Determinism | e2e fresh==cached/forced rows green on all fixtures | `p11-e2e.log` |
| Growth | S4/S6/S8 linear tests green in the 732 | — |
| Stub scan | only the known `rb-sql/src/lib.rs:1111` hit | — |
| Listener / leftovers | no `rb` listener; 0 repos; clones removed; `VACUUM` done in phase 10 | — |

## Criteria (closing bar)

(a) No pass-1 fix regressed: E1/E2 units green, full suite green, goldens unchanged. (b) No regression: all suites green, measurements unaffected (trigger grep). (c) No false edge above its stated certainty: E1/E2 closed the two found shapes; the battery + measurements show nothing else. (d) No non-determinism. (e) No failed test/guard/step.

## VERDICT: PASS — M6b closes (amendment 11)
