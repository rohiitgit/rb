# M6d evaluator pass 2 — verification-only

Date: 2026-10-11. Rule: no new behavior; re-run every finding's repro +
controls, the suite, the mutants, invariant 1 and determinism. (Amendment 14
is the pass-1 record: 2 blocking + 9 non-blocking findings from 3 parallel
evaluators.)

## Finding repros (from pass 1, amendment 14)
- [x] F1 (E1-1) tool_versions identities: `tool_versions_record_image_identities_when_scip_is_on`
      absent/present rows + e2e `scip_image_identity_is_part_of_the_tool_configuration` green
      (a rebuilt image rebuilds instead of serving "up to date").
- [x] F2 (E2-1) absolute replace: `absolute_replace_paths_never_join_a_service` green
      (an absolute target stays a `?` package edge, never a service edge).
- [x] F3 (E3-1) chi Handle: `chi_verbs_nested_routes_and_middleware` (`ANY /raw`)
      + fixture `Handle` rows in `scip-callers` goldens green.
- [x] F4 (E3-2) double-include: `/v2/api/login` duplicated row in
      `fastapi_decorators_prefixes_and_includes` + fixture goldens green.
- [x] F5 (E3-3) dispatcher kwargs: `requests_httpx_reconfigured_calls_are_capped`
      (`method=`/`url=` native slots stay plain) green.
- [x] F6 (E2-2) gomod split (declined with reason): keys-apart behavior held
      (fail-closed on the unmeasured form; D3).
- [x] F7 (E1-5) fetch env: Go argv pins (`GOTOOLCHAIN`, `GOFLAGS`, `GOPROXY`) green.
- [x] Recorded-no-code items (E1-2/3/4, E2-3/4/5, E3-4/5/6): claims re-read
      against the tree, no drift.

## Suite (closing tree)
- [x] fmt clean.
- [x] `clippy --workspace --all-targets -D warnings` clean.
- [x] Workspace units: 765 passed + 25 cli-bin, 0 failed (outside e2e,
      counted from `var/m6d/logs/close-unit.log` with awk).
- [x] `rb-store` 66/66 + `rb-server` 12/12 with DB, no `skipping:`.
- [x] e2e 34/34 + api 1/1 (150 parameter sets byte-identical) with
      containers, no `skipping:`.
- [x] No listener, no stubs (only the known `rb-sql` TODO).

## Mutants re-killed (scripts `var/m6d/scripts/{p1,p2,p3,p12,eval1}-mutants.sh`)
- [x] 22 killed: p1 4, p2 4 (2 anchors repaired after later phases reflowed
      them), p3 2 (anchor repaired), p12 6 (sweep-scan, memo-dropped,
      cycle-always-memo, cycle-always-legacy, replace-direction-flipped,
      pip-timeout-dropped), eval1 6 (F1, F2, F3, F4, F5, F7).
- [x] Phases 4–11 mutants were ad hoc (no saved scripts); their killer tests
      remain green in the suite above.

## Invariant 1 (dumps vs `027a54f`, every row classified)
- [x] `--no-scip`: 4/7 fixtures byte-identical; polyglot +1 billing job row;
      go-chi +5 endpoints/+10 edges/+1 job row with the HANDLED_BY-keeps-entry
      orphan shift; python-fastapi +5 endpoints/+edges/+3 CONSUMES/+6 py pins
      + diagnostics +2 job rows; 0 removed rows; two HEAD names identical
      (`var/m6d/logs/inv1/`).
- [x] SCIP: fixture goldens green in the e2e loop above (only classified
      hunks differ).

## Determinism
- [x] Fresh == cached, two names, forced rebuilds byte-identical (e2e
      `warm_facts_cache`, cold/warm SCIP cycles, `inv1` two-name dumps green).

## VERDICT: PASS
