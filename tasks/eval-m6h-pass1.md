# M6h evaluator pass 1 (full review, adversarial probing included)

Ran 2026-10-12 on the phase-4 tree. VERDICT: PASS with 4 fixes (H1–H3 + R2/R3 live proofs),
all test-first, all mutants killed.

## Findings

- **H1 (fix: AGE rejects `MATCH` after `WITH`-alias).** R2's first Cypher bound `COALESCE(df, x)
  AS f` then `MATCH (f)` — live-fired on topo: `cypher: variable 'f' already exists`. Reworked to
  `OPTIONAL MATCH`es only (no rebinding anywhere in any rule). Lesson: run every rule query live
  before calling the set done — the unit twin cannot catch engine quirks.
- **H2 (fix: duplicate Cypher rows vs relational dedupe).** Two writers, one table: the relational
  twin dedupes (BTreeSet) but Cypher multi-matches. R2 gains `RETURN DISTINCT` (pinned by a
  targeted assertion — the first version matched `COLLECT(DISTINCT`, which the mutant survived),
  plus CLI-level sort+dedup as defense in depth, plus a duplicate-writer relational unit.
- **H3 (fix: silent truncation).** A capped row list would pass a violating snapshot silently.
  Truncation now surfaces three ways: stderr warning, `(truncated: rows)` summary mark, JSON
  `"truncated"` flag. Pure `format_check` renderer with units; mutant killed.
- **H4 (verified, no change): exit-code mapping.** anyhow failures exit 1, so check errors map to
  2 explicitly; e2e asserts 0/1/2 plus the unknown-rule and missing-repo paths.
- **H5 (live proofs for paths units cannot reach).** R2 non-empty Cypher path: scratch repo
  (owner + writer services) → `unowned_table_writes svc-b -> shared.events`, exit 1. R3:
  scratch npm workspace cycle → both links, exit 1. Twin agreement on these shapes holds by the
  e2e parity plus the pointed units.
- **H6 (measurement classification, not a code change).** piggy's 11 orphans: 9 true (UI-called
  only), 2 residue-misses (`authClient.createUser` → `POST /uaa/users`, statistics Feign `PUT
  /statistics/{name}`) sharing one root cause — context paths living only in config-server
  `shared/*.yml`, the declared M6g residue. The notification Feign `GET /accounts/{name}` DOES
  stitch (suffix), so the misses are exactly the prefix+context-path cases.
- **Reviewed, no change:** twin null-handling symmetry (A1 unit: ops-less `QUERIES` silent both
  sides); sort stability (CLI re-sorts bytewise; Cypher `ORDER BY` is belt-and-braces);
  `--rule` exact match; multi-repo bail; concurrent runs (read-only transactions); GRPC endpoints
  fall under the orphan rule by kind (gateway `HelloService` orphans intended).

## Mutants (this pass)

M-orphan-negation, M-write-gate, M-self-loop, DISTINCT-drop, truncation-flag: 5/5 killed, each
reverted with the tree re-verified. (One DISTINCT assertion needed a second attempt: it matched
`COLLECT(DISTINCT` instead of `RETURN DISTINCT`.)

## Verdict

PASS with the H1–H3 fixes. No blockers: every rule query runs live, twins agree on 4 fixtures
plus 2 scratch shapes, measurements sample clean, no writes possible (fixed strings + read-only
path + no-write unit).
