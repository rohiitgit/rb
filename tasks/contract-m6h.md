# M6h contract: architecture rules as a CI gate (draft, 2026-10-12)

**Status:** draft written 2026-10-12 during the M6g phase-6 verify loop; forks F1–F4 decided
below; phase 0 not started. M6g must be closed before M6h phase 0 begins.

Written before any code. M6h is done only when every assertion below is backed by a passing test or a
recorded measurement. "Verified facts" are what probes read in the tree at the M6g close commit
(filled in at phase 0).

This contract follows the M6e/M6f/M6g format and applies the lessons M3b–M6g paid for (soundness bar +
closing bar up front; allow-lists; suppressions counted; growth by counted ops; no hand-counted
positions; invariant 1 vs previous close with classified rows; fixture plan is code; no builds during
container e2e; non-empty dumps).

## Soundness claims and closing bar (read first)

M6h's outputs whose failure would be silent: a rule that misses a real violation (a Cypher pattern
that does not mean what its name says); a rule that cries wolf in CI (a false violation blocking a
merge); a `rb check` run that writes to the database (a rule smuggling `CREATE`/`MERGE` through the
query path); a run that hangs CI (an unbounded traversal). **Everything not proven takes the safe
direction:** every rule's Cypher result must equal an independent relational re-computation on every
fixture (or the rule does not ship); the query path is read-only by construction (not by review);
every query carries a timeout and a row cap; an unreadable rules file is an error, never an empty
pass.

| Surface | Claim (exactly this, no more) | Mechanism | Residue (out of scope by construction) |
|---|---|---|---|
| Rule set (S1) | 3–5 built-in structural rules over service-level edges (dependency cycles, orphan endpoints, unowned table writes) with deterministic violation rows | Named rules; each a single read-only `MATCH` with `ORDER BY` | Cross-language semantic rules, threshold Metrics (churn, complexity) |
| Dual execution (S2) | Each rule's AGE/Cypher result equals its relational re-computation on every fixture | `rb-store` rule module with both engines + parity test | Ad-hoc user Cypher (no relational twin required) |
| CLI gate (S3) | `rb check` lists violations deterministically; exit 0 clean, 1 violations, 2 errors | New `rb-cli` subcommand; sorted text/json output | Auto-fix, baseline files, `--strict` levels beyond the exit code |
| Guard rails (S4) | Read-only transactions, statement timeouts, row caps on every rule query | Shared query path with `rb cypher`'s guards, tightened | Server-side streaming, cancellation mid-row |
| Fixtures + guards (S5) | Rule goldens on `topo-packs` + `polyglot-monorepo` (violations pinned, each rule biting) | Fixture plan is code; e2e fns + goldens | New fixtures only if a rule needs a shape no fixture has |
| Public repos (S6) | Rule results sampled correct; 0 false violations in the samples | Measurement plan below | New rule ideas recorded for their owners |

**What M6h must not change.** No new nodes/edges/diagnostics. `CONSUMES` never `exact` (reaffirmed).
`PIPELINE` becomes `m6h-1` at the first dump change (no dump change expected: rules read, never write).

This is invariant 1, checked against the M6g close dumps of every fixture (with and without SCIP) and
of the M6h measurement repositories at their pins, every differing row classified by script (dumps
asserted non-empty first).

**Closing bar.** Decided now:
1. **Evaluator pass 1 is a full review,** adversarial probing included (rule smuggling writes,
   timeout evasion, sort-order games, twin divergence).
2. **Evaluator pass 2 is verification-only.**
3. **Blocks closing only:** (a) a pass-1 fix that does not hold; (b) a regression; (c) a false
   violation or a missed planted violation on a fixture; (d) non-determinism; (e) a failed
   test/guard/step; (f) an AGE/relational twin mismatch on any fixture.
4. **Not blocking:** a new violation shape in a residue class (recorded); §timing (measured, not asserted).
5. **Timings are measured, not asserted.**

## Scope

**In (M6h):** the 6 items above (design §13 M6h row; design §8.4 division of labour; §14 open question 4).

**Out of M6h (each assigned):** Kotlin/C# → M6i; Guice/Dagger/NestJS/Spring-XML/.NET → M6j; custom
user rule files (decided F4); metric/threshold rules → residue.

## Engineer declaration

**Canonical state:** the built-in rule registry (name → Cypher + relational twin + description);
`rb check` flags. Nothing stored: violations are computed at query time from the live snapshot.
**Derived:** violation rows per rule.
**Invariants:**
1. **Dumps.** `--no-scip` and SCIP dumps differ from the M6g close only in classified rows; nothing
   unclassified; every compared dump asserted non-empty.
2. **Reads only.** No rule query writes (proven by a write-attempt test per engine, not by review).
3. **Twin equality.** Cypher result == relational result on every fixture, byte for byte after sorting.
4. **Egress closure.** Unchanged (M6h adds no fetches).
5. **Determinism.** Repeated `rb check` runs print byte-identically; fresh == cached snapshots agree.
6. **Bounded work.** Every rule query has a timeout and a row cap; the relational twin states
   complexity by counted operations.
7. **Exit codes.** 0/1/2 exactly as S3 states; CI can gate on the code alone.

## Decisions (made here, with reasons)

- **D1: rules read the AGE projection (F1).** Design §8.4 assigns pattern queries to AGE/Cypher;
  the relational twin is the check, not the product.
- **D2: twins, not reviews, prove the patterns (F2).** A Cypher pattern's meaning is slippery
  (path vs set semantics, label case, null properties); equality with an independent relational
  computation on every fixture is the only proof that survives contact with AGE quirks.
- **D3: built-ins only (F3/F4).** Custom rule files are a parsing/validation surface (injection,
  unbounded queries from untrusted text) with no demander; the registry is code, reviewed like code.
- **D4: violations sorted by (rule, src, dst) (F1).** Deterministic output without trusting AGE's
  row order; the CLI sorts regardless of engine.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **Rule set.** 3–5 named structural rules, each a single read-only MATCH with ORDER BY. | unit; mutation |
| S2 | **Twins.** Cypher == relational on every fixture. | e2e parity per rule; mutation (twin divergence fails) |
| S3 | **CLI gate.** `rb check` exit 0/1/2; sorted text/json. | unit + e2e goldens |
| S4 | **Guard rails.** Write attempts rejected per engine; timeouts + caps hold under a planted slow rule. | unit (write rejected); measured timeout |
| S5 | **Fixture + guards.** Rule goldens pinned; each rule's violations bite (planted violation fails). | e2e with containers; goldens |
| S6 | **Public repos.** Sampled violations correct; 0 false in samples. | recorded measurement |
| S7 | **Determinism + invariant 1.** Repeated runs identical; dumps vs M6g close classified. | e2e; recorded |
| S8 | **Verification loop + closing.** `var/m6h/scripts/verify.sh`: fmt, clippy, counted units, store/server with DB, e2e + api with containers, listener + stub checks. | verification loop |

## Verified facts (to be read at the M6g close commit, in phase 0)

1. `rb cypher`'s read-only guard and timeout path (file/flags to reuse).
2. `rb check` name free (no subcommand, no flag, no API route).
3. AGE label set at the M6g close (`Topic` present after M6g; edge labels per `EdgeKind::ALL`).
4. Fixture shapes available per rule (cycles? orphan endpoints? shared tables? — topo-packs + polyglot inventoried).
5. `rb serve` guard rails for a new endpoint (if S3 grows one; CLI-only is the default).

**Not verified (checked in preflight, before the step that needs it):** Cypher `ORDER BY` + agtype sorting stability; statement-timeout plumbing through the `cypher()` wrapper; row-cap mechanism; exit-code conventions for CI;

## Fixture plan

- `topo-packs` + `polyglot-monorepo` rule goldens (violations pinned per rule; planted-violation mutants per rule for bite).
- No new fixture unless a rule needs an absent shape (then: minimal addition to `topo-packs`, never a new repo).

## Public-repository measurement plan

| Step | Repository @ pin | Runs | Notes |
|---|---|---|---|
| Phase 0 | — | M6g close binary dumps (`--no-scip`); e2e green baseline | small |
| Phase 6 | reuse M6g pins + 1 services-rich repo | `rb check` both engines | twin equality + sampled violations |

Disk policy as M6b D9 (6 GiB floor, one repo at a time, remove + `VACUUM`, part-by-part figures).

## Determinism

No wall-clock/host-path/run-order in outputs (same rules as dumps). Repeated `rb check` runs print
byte-identically; twins agree on fixtures and measurement repos.

## Verification loop

`var/m6h/scripts/verify.sh <tag>` (copied from M6g's, M6h paths/logs): fmt, clippy `-D warnings`,
counted units, `rb-store`/`rb-server` with DB, e2e + api with containers, listener + stub checks,
mutation runs per M4a harness rules.

## Forks (decided here, with reasons)

- **F1: AGE projection is the rule engine.** §8.4's division of labour; relational is the twin.
- **F2: twins prove patterns.** AGE semantics are too slippery for review-only.
- **F3: built-in rules only.** No demander for custom rule files; avoids an injection surface.
- **F4: no auto-fix/baselines.** Gate only; remediation is the user's (and the model's) job.

## Amendments

(none yet)
