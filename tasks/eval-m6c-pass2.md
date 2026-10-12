# M6c evaluator pass 2 — verification-only

Date: 2026-10-11. Rule: no new behavior; re-run every finding's repro +
controls, the suite, the mutants, invariant 1 and determinism.

## Finding repros (from pass 1, `tasks/eval-m6c-pass1.md`)
- [x] E1 inline comments: `lombok_configs_read_nearest_chain` commented row green.
- [x] E2 shape gates: deployable test with `abs`/`nest`/`iface` rows green.
- [x] E1/E2 trigger grep over re-cloned D13 pins: no `lombok.config`
      anywhere; no abstract/nested/interface `@SpringBootApplication`
      carriers (all app classes top-level concrete, incl. a live C4 instance:
      petclinic's test-source application stays library).

## Suite (closing tree)
- [x] fmt clean.
- [x] `clippy --workspace --all-targets -D warnings` clean.
- [x] Workspace units: 753 passed, 0 failed (outside e2e, counted from
      `var/m6c/logs/p11-unit.log`).
- [x] `rb-store` 66/66 + `rb-server` 12/12 with DB, no `skipping:`.
- [x] e2e 32/32 + api 1/1 with containers, no `skipping:`.
- [x] No listener, no stubs (only the known `rb-sql` TODO).

## Mutants re-killed (scripts `var/m6c/scripts/{p1..p9,eval1}-mutants.sh`)
- [x] 57 killed: p1 7 (fixed `array-items` anchor), p2 6, p3 5 (kind-gate
      discriminated by new `iface` row), p4 6, p5 5, p6 6, p7 5, p8 8, p9a 6
      (fixed `nested-zero` anchor; cross-crate), eval1 fixes 3.
- [x] 3 documented, not killed: `broad-candidates` (joint coverage, as in
      phase 2), `build-dropped` + `exact-gate-dropped` (e2e-killed: S4 wiring
      proven by golden mismatch, S7 gate by the M3b-S7 assert failure).

## Invariant 1 (dumps vs `10a9ad5`, every row classified)
- [x] Fixture goldens green both tiers (e2e): only classified hunks differ.
- [x] Public-repo diffs classified (`var/m6c/bench/phase11.md`): every new
      edge S6/S7/S4; role flips consequence-free; RTA yield 0; orphans,
      buckets and gates otherwise identical.

## Determinism
- [x] Fresh == cached, two names, forced rebuilds byte-identical (e2e
      `warm_facts_cache`, cold/warm SCIP cycles green).

## VERDICT: PASS
