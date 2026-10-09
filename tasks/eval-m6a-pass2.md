# M6a evaluator pass 2 (verification only)

- **Date:** 2026-10-09 (wall clock)
- **Commit:** `5b33cc3` (HEAD; clean tree at start; only this file added by this pass).
  No code changed since the Z1 fix `cac1b84`: `git diff cac1b84..HEAD -- crates fixtures
  Cargo.toml Cargo.lock docker migrations` is empty. `git diff cac1b84^..HEAD -- crates/`
  is only `crates/rb-scip/src/jobs.rs` (+78/−2: the fix + 2 tests).
- **Scope:** closing bar 2 (verification only). Blocks only on 3(a–e). The barred
  shardingsphere/mattermost benchmarks were NOT re-run; nothing under
  `/Users/rohit/Public/alvora`. No eladmin fetch was needed (fixtures + units cover pass 2).
- **Environment:** Apple Silicon, `PATH=/opt/homebrew/opt/rustup/bin:$PATH`,
  `DOCKER_CONTEXT=desktop-linux`. Images verified by inspect before any container step:
  `rb-scip-java:0.13.1` (`sha256:cbd99634…`), scip-typescript
  (`sha256:028c00429913…`), `rb-egress-proxy:1.11.3` (`sha256:6ddb810b…`).
- **Database:** `postgresql://rb:rb@127.0.0.1:55433/rb`, 0 repos (`rb.repo`) at start and
  end. Only `ev6b_*` names used; every repo created was dropped (graphs + rows).
- **Disk:** 9.9 GiB free at start, 8.5 GiB at end; floor 6 GiB never approached
  (`df -m` before/after each container step). Parts: `target/` 8.0 → ~10 GiB (my fresh
  release build + debug runs), `/tmp` scratch (binaries, dumps, backups — all outside the
  repo). No checkouts or repos remain.
- **Binaries:** base `var/m6a/base/rb-cfa8ab9` (sha256 `69800d61…f009b`, matches recorded);
  HEAD release `/tmp/ev6b-rb-head` (sha256 `ede3e290…aad5c`) compiled by me from HEAD
  (`cargo build --release --bin rb`; `var/m6a/bench/bin/rb-m6a` was NOT reused — it hashes
  `17c4…`, built at `78e08da` pre-fix).

## 1. Regression suite (`var/m6a/scripts/verify.sh ev6b-pass2`, one run, logs `var/m6a/logs/`)

| Check | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean, 0 warnings |
| `cargo test --workspace` (both streams, awk per lesson) | exit 0; **676 passed, 0 failed** outside e2e/api = 674 + 2 new (both new tests present and `ok` in the log; total with e2e+api 709 = 676+32+1) |
| `rb-store` with DB | **66/66**, no `skipping:` |
| `rb-server` with DB | **12/12**, no `skipping:` |
| e2e with containers | **32/32** in 452.64 s, zero `skipping:` (`scip_java_lombok`, `scip_ts_workspaces`, `scip_java_spring_jpa`, `a_publish_killed_after_its_flip_self_heals` all `ok`) |
| api | **1/1** (150 parameter sets identical; statuses 200×100, 400×21, 404×18, 409×8, 410×1, 422×3) |
| listener / stub scan | no `rb` listener; 1 hit, the known test-input string `rb-sql/src/lib.rs:1111` |

Observation (non-blocking): `resolve::tests::field_chains_binding_chains_and_big_packages_stay_fast`
(a wall-clock `< 2/3/5 s` test, unrelated to the fix) failed once while a release build +
clippy + tests ran concurrently (load ~29); it passed alone (1.11 s), in the re-run
235/235 suite, and inside this verify run. Timing-only flake under extreme load, not a
regression: the file it lives in is untouched by the fix.

## 2. Z1 re-check (the fix holds end to end)

- **Repro fails on pre-fix code (stash-free):** with only the fix hunk reverted to
  `cac1b84^`'s `.take_while(|c| is_under(c, root))` (new tests kept), both new tests FAIL:
  `a_sibling_sorting_between_a_root_and_its_members_does_not_shadow_them` and
  `workspace_membership_with_shadowing_siblings_grows_linearly`. File restored
  byte-identical afterwards (sha256 `30284f3f…` before and after).
- **Passes now:** full jobs module 13/13 `ok`, including the repro, the shadowed-growth
  test, and all S8 allow-list / nested-root controls.
- **Diff:** `cac1b84^..HEAD -- crates/` is exactly the fix (string-prefix `take_while` +
  `is_under` filter + comment) and the 2 tests. Grouping goldens unchanged (e2e green
  without re-bless; no fixture file changed).

## 3. Controls

S8 allow-list units, `members_lockfiles_and_nested_roots`,
`typescript_jobs_group_tsconfigs_under_their_nearest_package_root`, S19 all three growth
tests, `scip_ts_workspaces` e2e — all pass; grouping goldens unchanged by the fix.

## 4. Invariant 1 re-check (both binaries: base `rb-cfa8ab9` vs fresh HEAD release)

- **`--no-scip`, all 5 fixtures:** 4 byte-identical with hashes equal to the recorded ones
  (`5d29a71a`, `3aeacacb`, `1fd30d5b`, `2dbce68c`); all 3 old fixtures also equal
  `var/m6a/base/noscip/` (phase-0 baseline). `ts-workspaces` differs in exactly the 3
  amendment-7 W rows (grouped root gains `workspace_members: 2`, two member rows fold in).
- **`--scip` spot:** HEAD hashes equal recorded (`696599b9`, 84 lines; `210f33fe`,
  116 lines); base hashes equal recorded (`a9338849`; `d0181cca`, 106 lines).
  `classify.py` re-run by me: java-lombok **+L 45 / −L 39, 5 files newly accepted**,
  6 unclassified removals = the recorded L-consequences (2 `(scip)` DEPENDS_ON package
  edges with generated-only evidence + 4 `unresolved_references` now exact);
  ts-workspaces **+T 4 / +W 26 / −W 19**, 5 unclassified = the recorded W-consequences.
  The 4 T edges are `Mid#m→Base#m`, `Deep#m→Mid#m`, `Deep#m→Base#m`, `Other#n→Base#n`,
  all sub→super (remaining new IMPLEMENTS rows are the recorded `Svc` class edges,
  classified W); no super→sub edge anywhere.
- **Fresh == cached:** covered by the green `scip_java_lombok` / `scip_ts_workspaces` e2e.

## 5. Determinism spot

`ts-workspaces` `--no-scip` into two repo names with the fresh HEAD binary: dumps `cmp`
identical. Repos dropped after (0 repos).

## 6. Mutations (each from a clean tree; restored byte-identical via `cmp`; `git diff --quiet` after each)

| Mutant | Killer | Result |
|---|---|---|
| M3-equiv: fixed code's `take_while`+`filter` → `filter` only | new repro alone | SURVIVES the repro (correct: bare `filter` implements the correct semantics — slower, not wrong); also survives both growth tests (the `tick(1)` sits in the loop body, so the walked-past siblings are unticked string comparisons). NOT a coverage gap for correctness: the Z1 regression proof is §2 (old logic fails both new tests). See observation O1 |
| M1 (D1: drop the enclosing-range clause) | `a_data_class_is_accepted_once_its_generated_members_are_removed` alone | KILLED (`test result: FAILED`) |
| M4 (D9: `-T` at n≥1) | `default_budget_argv_is_cfa8ab9s` alone | KILLED |
| M6 (D1: marker by descriptor, any package) | `the_lombok_marker_is_identified_by_package_and_descriptors` alone | KILLED |

## 7. Z2 (unchanged, kept-and-counted)

No code path touched by the fix alters link-target behavior (`jobs.rs` member scan only).
Link-target units pass explicitly: inside→exported, outside→`unsupported`, key coverage,
`documents_outside_the_root_are_ignored_and_counted`, plus `export_selection_grows_linearly`.

## 8. Residue

No new forms in any residue class found. O1 (below) is a measurement observation, not a
new attack form.

**O1 (non-blocking observation, for the record):** the S19 tick counter counts loop-body
executions, not `take_while`/`filter` walks (`is_under` is a pure string op). Hence a
`filter`-only variant is tick-identical to the fix while doing more unticked prefix
comparisons (linear per root here; adversarial nesting could do worse). Invariant 10's
letter (counted ops ≤ 5×) holds as tested; no correctness impact; no bar-3 clause applies.

## VERDICT: PASS

(a) Z1's fix holds with its repro (fails pre-fix, passes now) + controls + growth;
(b) no regression — suite green (676/66/12/32/1, zero `skipping:`), goldens unchanged,
invariant 1 holds as classified; (c) no new 3(a–e) issue; (d) deterministic (two-name
`cmp` identical; fresh == cached via e2e); (e) all steps green. O1 and the wall-clock
flake are recorded observations, not blockers. New attack forms in residue classes: none
found (none would block per bar 4).

**M6a is closed: pass 2 verifies Z1's test-first fix with its repro, controls and growth
bound, the full suite is green with goldens unchanged except classified L/T/W rows,
invariant 1 holds on fixtures with both binaries, and there is no new 3(a–e) issue.**
