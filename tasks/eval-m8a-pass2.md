VERDICT: FAIL (2 blocking; 6 should-fix, 2 notes)

# M8a close: evaluator pass 2 (2026-10-13)

Scope: contract-m8a, pass 1, bench-findings, `bench/`, `crates/rb-bench/src`, the scoreboard, usage §5.5 and README. Tools: the prebuilt `target/debug/rb-bench`, `python3 -I` over `var/bench/out/a038653b8478/`, and scratch copies. No cargo was run. The workspace suite is pending and not judged here.

**What reproduces:**
- Re-scoring gives a scoreboard and perf file `cmp`-identical to the committed ones (`45ae7d44…`).
- An independent Python location matcher reproduces every SCIP cell. It also reproduces mattermost's **6,108** cross-module fp, 0 cross-module fn, 5,819 `_test.go` fn, 10,599 and 2,871 cross calls.
- An independent name-based PyCG scorer matches **all 238 cells**, as well as "44 of 105", the category recalls, the certainty table, the perf of record and the 3,199 s cold SCIP total.

## Findings

1. **Blocking: pass-1 finding 2 was fixed in form only.**
   - `Locator::canonical` is called only by tests (identity.rs:159, 302-304), never by `scip_ref::agreement`.
   - SCIP merges the static and instance `User#hashPassword` (express User.ts:11 and :62). Tier B's ambiguous 62→11 and 62→62 therefore score fp 2 + fn 1, and the site counts as a miss. Yet 62→11 is the true call (line 63).
   - The Result's "Off mattermost, Tier B adds 2 edges that SCIP does not have" is entirely this artifact. Mattermost has 4 more such pairs (`ensureFeatureFlag` overloads).
   - Fix: map found endpoints through the reference locator's `canonical` before scoring. Add a unit, re-score, and update the Result, B3 and the certainty table.
2. **Blocking: "S1–S11 met" while S4's Java and Python legs are vacuous.**
   - Every Java job (spring_boot, kotlin, adservice) and every Python one (fastapi failed, microblog has no job) has 0 covered files.
   - Lessons L429 and 2026-10-12 say a vacuous leg proves nothing.
   - Fix: an amendment narrowing S4 to Go and TS (Java and Python move to residue), or one passing Java or Python SCIP job.
3. **Should-fix: the gate passes certainty downgrades of correct edges.**
   - Each of these scratch edits makes `compare` exit **0**:
     - go_gin: tp heuristic→ambiguous (sites and hits 0→1);
     - mattermost: 50 tp the same way;
     - go_gin: 152 tp `exact`→`heuristic`.
   - Fix: fail when tp at certainty ≥ c falls, for any c. This is needed before M8b's `exact` keys.
4. **Should-fix: 8 of the 48 mutant specs are stale.**
   - m153, m156–m160 and m162 target pre-amendment-1 code; m193's code was deleted.
   - The rewritten Locator and the two-pass ambiguous logic have no live mutants, so "all killed" can't be re-run.
   - Fix: re-target the specs, re-run them, and mark m193 retired.
5. **Should-fix: the scorer of record predates the last source edits.**
   - `dump.rs` and `pycg.rs` were edited at 21:43:55–58. The rb-bench binary is from 21:39 and the scoreboard from 21:42 (lesson 2026-10-13, "binary of record").
   - Fix: after the suite, re-score with the rebuilt rb-bench and `cmp` the result.
6. **Should-fix: `run.sh` has safety and reporting gaps.**
   - A failed index or dump (`|| continue`, :86-99, :123-128) leaves the previous dump in OUT, and `score` uses it.
   - The script always exits 0, because its last command is `rm -f`.
   - `exit 9` inside `stress()` skips `cleanup.sh` and leaves `run.$$.sh`.
   - `to` sends SIGKILL to `/usr/bin/time` only, which orphans `rb`.
   - `DOCKER_CONTEXT` is not exported, unlike `bench_index.sh`, against D3 and L557.
   - Fix: delete each target's outputs first, count failures and exit non-zero, use `trap` for clean-up, kill the process group, and export `DOCKER_CONTEXT`.
7. **Should-fix: S5's units hold one real case (pycg.rs:329), not three, and none scores `pkg/__init__.py`.**
   - `a_root_package…` tests a root `__init__`, not a package.
   - The behaviour itself is right.
   - Fix: add the real `imports/init_import` and `decorators/return_different_func` JSON.
8. **Should-fix: S11 and the records overclaim.**
   - (a) The S11 verification-template line exists in no file.
   - (b) The README omits these `exact` producers:
     - Tree-sitter `DEFINED_IN` and `CONTAINS` (syntax.rs:203, 214);
     - manifest `IN_SERVICE` (lib.rs:864);
     - CODEOWNERS (lib.rs:868).
   - (c) README:22 says "How often each label is right is measured", which is false for `exact` and `synthetic` until M8b.
   - (d) B6 cites run 1's RSS. The perf of record (run 3) is 1.87 GiB, 1.24× M5, above the 1.15× warn line.
   - (e) CHANGELOG, IMPLEMENT and `contract-m8b.md` were being written during this pass. Re-check them against (a)–(d) and findings 1–2.
9. **Note: the PyCG mapping is sound.**
   - Seven cases checked by hand were all classified correctly: `imports/init_import`, `imports/import_all`, `classes/base_class_calls_child`, `decorators/return_different_func`, `mro/two_parents`, `exceptions/raise` and `lambdas/calls_parameter`.
   - In all 119 cases, every gold caller is a module, function or lambda, and every in-case callee maps to an rb callable. No systematic bias was found.
   - "Recall < 50 %" is measured per case. Per cell, 57 cases qualify. State the unit.
10. **Note: SCIP identity holds apart from finding 1.**
    - Covered-file locations are identical across modes (go_gin 153/153, bookcars 2,482/2,482).
    - SCIP mode sources no heuristic `CALLS` in covered files.
    - The sampled fn (go_gin: `Generate`, `appG.Response` ×2, `CheckMergedImage`, `OpenMergedImage`; bookcars: `checkToken`, `helper.error`, `getCountry`, `helper.info`, `useStore`) are real calls Tier B lacks.
    - Bookcars fn: 610 of 694 are namespace imports behind an `@/` alias, so B3 holds.
    - Accept and stale lines, `exact` fp under accept (exit 1), a malformed accept line (exit 2) and the `cleanup.sh` guard all work.
    - Microblog's note says "no status", but the file exists. It should say "no SCIP job".
