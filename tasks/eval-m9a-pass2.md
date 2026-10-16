VERDICT: PASS (0 blocking; 0 should-fix, 5 notes)

# M9a close: evaluator pass 2 (2026-10-16, verification of amendment 3)

Scope: the pass 1 answers E1–E8 (`tasks/eval-m9a-pass1.md`) in `624da45` (code, tests, runner, docs) and `7b18f12` (re-close records). This covers `tasks/contract-m9a.md` amendment 3, Result and C8/C10/C11/C12; the scoreboards `bench/scoreboard/m9a{,-perf}.json`; the run `var/bench/out/2e36cd607eab/`; the binary `var/m9a/bin/rb`; the evidence in `var/m9a/logs/`; and the CHANGELOG, IMPLEMENT, todo, bench-findings and lessons entries.
Method: tools at HEAD `7b18f12` (branch `m9`), with `target/debug/{rb,rb-bench}` rebuilt. Fixtures were indexed into `:55433` and deleted afterwards. 3 mutants are my own. Scratch is in `var/m9a/eval2/`. Read-only apart from this report.

## 1. E3: an empty remote context path now clears a lower one (verified)

- **The fix.** `lib.rs:907-909` now records the highest value found, empty included (`if let Some(value) = entries.get(CONTEXT_PATH_KEY)`). Then `lib.rs:924-928` removes the client's context path when that value is empty and inserts it otherwise. The diagnostic still names the file, with `context_path` `""`. `PIPELINE` is `m9a-2` (`lib.rs:142`).
- **The test.** `config_server_shared_files_set_client_context_paths` passes. Its rows include:
  - `eps`: remote `""` over local `/local-eps` (and over the shared `application.yml` `/app`) gives `GET /x [exact]`.
  - `zeta`: remote `<name>.yml` null over remote `application.yml` `/app` gives `GET /x [exact]`.
  - `theta`: an absent key overrides nothing (`/app`).
  - Local rows: `lyml`, `lprof`, `lnull` and `lbase`.
  - `e3-red.log` shows exactly `eps` and `zeta` red before the fix (both `/app/x [synthetic]`), as claimed.
- **Pass 1's own fixture** (`var/m9a/eval/fx/b14`, re-indexed as `t_eval2_m9a_b14`).
  - `svc-e`, which pass 1 reported as `/local-eps/x`, is now `GET /x` (certainty 5, `exact`), with diagnostic `context_path ""`.
  - The other services are unchanged from pass 1: `/alpha`, `/beta-props`, `/gamma-common`, `/eta`, `svc-z` `/alpha`, and `svc-d` `delta`→`/delta/x` (E6 residue).
- **My own fixture** (`var/m9a/eval2/fx/e3`, `t_eval2_m9a_e3`, `--no-scip`). It has a native server at `classpath:/shared, classpath:/common` with shared `application.yml` `/app`. Snapshot `tool_versions.pipeline` = `m9a-2`.

  | svc | input | got | Spring |
  |---|---|---|---|
  | e | remote `""` over local `/local-eps` | `/x` exact, diag `""` | root |
  | f | remote `<name>.yml` null over remote `application.yml` `/app` | `/x` exact, diag `""` | root (YAML null → `""`) |
  | g | no `<name>` file, local `/local-g` | `/app/x` synthetic | remote over local |
  | h | shared `""`, later location `/later` | `/later/x` | later location wins |
  | i | shared `/india`, later location `""` | `/x` exact | root |
  | j | `juliet.properties` `key=` over `juliet.yml` `/yml` | `/x` exact | `.properties` first |
  | k | `<name>` file without the key, local `/local-k` | `/app/x` | absent ≠ empty |
  | l | `""` only in a `spring.profiles: docker` document | `/lima/x` | profile document ignored |
  | m | not a client; local `application.yml` `""` over `bootstrap.yml` `/boot` | `/x` exact, no diag | root |
  | n | remote `"   "` over local | `/x` exact, diag `""` | root (Spring trims) |
  | o | remote `.properties` `/oscar   ` (trailing spaces) | `/oscar/x` | `/oscar` |

  Rows n and o were indexed with the binary of record, `var/m9a/bin/rb` (`t_eval2_m9a_bin`), and every row a–m gave the same result there. The binary is not just the debug build.
- **Spring's documented behaviour holds.**
  - Config Server remote sources rank above local files by default.
  - `<application>.yml` ranks above `application.yml`.
  - The YAML processor flattens a null to `""`.
  - `ServerProperties.Servlet` trims the context path and drops a trailing `/`, and `""` is the root context.
  - The rule follows local files: their merge also takes the highest value and then drops an empty one (`lib.rs:1184-1190`).
- **An extra edge.** `context-path: /`, remote (`svc-p`) or local (`svc-q`), gives `GET /x [synthetic]`. The path is right, the certainty is conservative, and local and remote agree, so there is no finding.
- All three repositories were deleted with `rb gc`; their AGE graphs were dropped.

## 2. E1: the battery and the surviving specs (verified)

- `python3 -I bench/mutants/run.py bench/mutants/m9a.json --jobs 4` (fresh, 4 min 09 s; `var/m9a/eval2/mutants-m9a.log`): **31/31 KILLED**, m230–m260, none STALE or INVALID.
- Each new spec (m248–m260) mutates non-test code, its `old` text occurs exactly once, and each names a filter.
- Pass 1's specs re-run (`var/m9a/eval2/mutants-pass1.log`): e01, e02 and e04–e12 KILLED; e03 STALE (0 matches).
  - **e03 is validly STALE.** Its `old` was `if !value.is_empty() {`, the line E3 replaced. Its meaning, "an empty value counts as a path", is now m249 (`if false {`), and m259 drops the clear. Both are KILLED.
- The unfiltered wide specs e02w–e12w are all INVALID `(no test filter)` under the new rule (`mutants-pass1-wide.log`), as E2 intends. Their filtered twins above are KILLED.
- **My mutants** (`var/m9a/eval2/mutants-eval2.json`, log `mutants-eval2.log`):
  - v01 KILLED: E3 reverted another way, skipping empty values while searching (`.filter(|v| !v.trim().is_empty())`).
  - v02 KILLED: the empty value clears only when no local path exists, and otherwise inserts `""`.
  - v03 SURVIVED (extra probe): the remote value is not trimmed. See N1. Today's behaviour is right (rows n and o above).
- `git status` after every battery shows only the pre-existing `bench/__pycache__/`. No `rbmut` worktree is left.

## 3. E2: the runner's kill rule (verified; one residual gap)

- **The change.** `verdict()`:
  - Without the filter, a non-rb-bench spec is INVALID before anything runs.
  - With a filter, KILLED needs a `---- <name> stdout ----` whose name contains the filter.
  - Failures only outside the filter are INVALID.
  - A non-zero exit with no failure section (build error, abort) is INVALID.
- **Checks.**
  - `var/m9a/eval2/verdict_check.py` feeds `verdict()` 9 synthetic outputs, and all give the expected result (`verdict-check.txt`).
  - When only the timing test `resolve::…_stay_fast` fails under the `config_server…` filter, the result is INVALID.
  - `e2-runner-check.log` reproduces the same rule through real runs: t1 INVALID (no filter), t2 INVALID (failure only outside `wilson_intervals`), t3 KILLED (control).
- **Where a false KILLED can still arise.** For a non-rb-bench crate, cargo runs only the tests that match the filter, and the KILLED check uses the same substring rule. So a false KILLED needs a filter that itself selects a flaky test.
  - The 7 wall-clock tests in these crates are `field_chains_…_stay_fast`, `many_beans_of_one_type_are_too_ambiguous`, `an_all_parameter_input_is_too_ambiguous_within_a_bound`, `wide_python_hierarchies_resolve_quickly`, `deep_and_wide_python_hierarchies_stay_fast`, `long_proxy_and_meta_chains_are_linear_and_do_not_recurse` and `reversed_proxy_chains_resolve_every_class` (`timing-tests.txt`).
  - None of the 6 filters in `m9a.json` is a substring of any of them. `context_path` matches only `spring_context_paths_are_synthetic` and `config_server_shared_files_set_client_context_paths`.
  - The rule does not refuse such a filter. `stay_fast` or `tests::` would KILLED on a timing flake: see `verdict-check.txt` rows 8 and 9, and `e2-runner-check.log`'s own `filter stay_fast -> KILLED`. See N2.

## 4. E4: the citation audit (verified)

- A copy of `var/m9a/scripts/check_citations.py`, with only `OUT` changed to `var/m9a/eval2/`, writes `citations.txt` **byte-identical** to `var/m9a/logs/citations.txt`:
  - totals: OK 258, MISMATCH 1, UNRESOLVED 0, over 259 citations in 80 verdicts;
  - control (every cited line + 7): OK 64, MISMATCH 190, UNRESOLVED 5.
  - The input check prints `same lines as verdicts-*.tsv: True`, and all 3 checkouts are at their pins.
- **The MISMATCH is real and harmless.** `exact.tsv:276` cites `(:41)` for "overrides EmptyCart". At pin `38e7348e`, `CartService.cs:41` is the `{` of `GetCart` and `:45` is `public async override Task<Empty> EmptyCart(…)`. The verdict TRUE stands.
- **The rule is loose but meaningful.** The control still scores 25% OK. So I read every OK row that matched only one identifier (42 rows). Each holds the named symbol or a plausible import or declaration line. For example, `:676` in row 296 is the test function the evidence names.
- **`disk.txt`.** It exists and gives the amendment 2 figures (2.4 GiB, 48.0 GiB, 28 GiB after) with their source, "orchestrator session tool output", plus a logged `df` from 13:45. Contract amendment 3 (`:298`), the Result (`:407`) and CHANGELOG (`:9`) point to it.

## 5. Re-close evidence (verified)

- **Re-score.** `rb-bench score var/bench/out/2e36cd607eab --milestone m9a --rb-commit 624da45a…` is `cmp`-identical to `bench/scoreboard/m9a.json` and `m9a-perf.json`. The committed `m9a.json` differs from its parent only in `rb.commit` and `rb.binary_sha256` (the `7b18f12` diff is 2 lines).
- **Gate.** `compare var/m9a/m8b-rescored.json bench/scoreboard/m9a.json --accept bench/accept-m9a.tsv` exits 0, with 14 `changed` lines and 1 `accepted` line. These are identical, outside the perf lines, to `compare-r2.txt` and to r1's `compare.txt`. With `--perf var/m9a/m8b-rescored-perf.json bench/scoreboard/m9a-perf.json`, the output plus exit line is byte-identical to `compare-r2.txt`, with 27 warnings.
- **r2 diff.**
  - `r2diff.py 05a600c9ff2d 2e36cd607eab` is `cmp`-identical to `r2-diff.txt` (`dumps 157: identical 157, differing 0; differing rows 0`). An independent per-file `cmp` agrees: 157 dumps, 0 differ.
  - **The script is not vacuous.** Over `a038653b8478 → 05a600c9ff2d` it reports 10 differing dumps and 1,866 rows (= C1 22 + C2 6 + C3 1,740 + C5 78 + C6 20), in the same 10 dumps as the invariant.
- **Sample.** `sample-missing-r2.txt`: total missing 0.
- **Binary of record.**
  - sha256 of `var/m9a/bin/rb` = `2e36cd607eab2ac472886238d18e27ba3d3d0340d5835aa480e9a5fb4b047f17` (`rb.sha256` agrees); `rb.r1` = `05a600c9…`.
  - `git diff 624da45 HEAD -- crates migrations Cargo.toml Cargo.lock` is empty, and the tree status for those paths is clean. `cargo build --release -p rb-cli` on that tree finds the artifact fresh, and `target/release/rb` hashes to `2e36cd60…`. So the binary corresponds to `624da45`'s clean sources.
  - `build-r2.log` itself records only `HEAD 624da45` before and after the build, with no status line; the check above supplies the missing proof.
  - **`PIPELINE`.** A snapshot written by `var/m9a/bin/rb` records `"pipeline": "m9a-2"` (§1). The release binary inlines the short string, so `grep` of the binary is not evidence; indexing is.
- **Bench run** (`bench-run-r2.log`): 27 "fresh and cached dumps identical", `failures: 0`, `repositories left like bench\_%: 0`.

## 6. Perf (verified; claims computed right, NFR met)

- `var/m9a/eval2/perf_ratios.py` recomputes everything from `perf-ab3.txt` and `perf-ab4.txt` (`perf-ratios.txt`).
- **`perf-ab3.txt`.**
  - 234 runs exit 0; spring_boot_microservices exits 1 ×9 in 0.01 s (`bench_m9a_ab3_spring_boot_microservices_m8` is 42 characters, over the 40 limit).
  - r2/M8 mean wall is 0.68–1.19 on the other 24 targets, go_gin_example 1.27 (r2 runs 0.91/1.55/0.84; r1 also had a 1.94 s run), and mattermost 32.09 / 31.61 s (1.02×), with runs of 48.58, 50.99 and 49.81 s, one per binary. All of these match amendment 3.
- **`perf-ab4.txt`.**
  - 26 runs exit 0.
  - go_gin_example: means 0.49 / 0.49 s (n=5).
  - spring_boot_microservices: medians 0.41 / 0.42 s; the means of 0.86 / 0.82 s are inflated by one 2.5 s run per binary.
  - mattermost: 22.88 / 23.82 s (1.04×); max RSS M8 1,990,934,528 B, r2 1,951,055,872 B. All match.
- **CPU time**, which load does not inflate, confirms there is no regression: mattermost r2/M8 is 1.019 (ab3) and 1.031 (ab4), go_gin_example 1.021 and 0.962.
- **NFR.**
  - mattermost r2 wall: 23.82 s ≤ 30.9 s (ab4).
  - r2 peak RSS: at most 1,959,968,768 B (ab3), 1,951,055,872 B (ab4), and 2,041,479,168 B in the bench run (1.133× of 1,801,732,096). All are ≤ 2,072,321,720 B (1.93 GiB), and also ≤ the exact 1.15× limit, 2,071,991,910 B.
  - "No warning persists" holds on the ab4 numbers. Two of the three targets needed a second re-measure: see N4.

## 7. Records (verified)

These match their logs:
- unit 357/204/79 (`unit-*-r2.log`);
- e2e 52 passed, 0 failed, 0 `skipping:` (`e2e-r2.log`, `e2e-r2-exit.txt` `exit=0`); try1 50/2 with `scip-java failed 70.2s (fetch phase)`; the re-run 2 passed;
- batteries: m9a 31 KILLED and m8b 29 KILLED, 0 cached (`mutants-*-r2.log`);
- `fmt-clippy-r2.log` CLEAN;
- `repo-{before,after}-r2.txt` identical;
- scoring 8.51 s (`score/r2-time1.txt`);
- mattermost fresh 39.35 s with RSS 2,041,479,168 (`m9a-perf.json`, load 3.95→6.0);
- citations 258/1/0 and the control.

The answers are consistent across the contract (amendment 3, Result, C8/C10/C11/C12, residue E5/E6), CHANGELOG, IMPLEMENT, todo, bench-findings B14 and lessons. Every Status line says "pass 1 answered, pass 2 pending": contract `:3`, IMPLEMENT, CHANGELOG, todo `[ ] Evaluator pass 2`. The design-doc table ("evaluator pending") and the README ("under evaluation") agree. E8: `perf-ab3.txt` and `perf-ab4.txt` contain no `bad option`.

## 8. Hygiene

- `cargo fmt --all --check` exits 0. `cargo clippy --workspace --all-targets -- -D warnings` exits 0.
- `cargo test -p rb-indexer --lib`: 357 passed, 0 failed. `cargo test -p rb-frameworks --lib`: 204 passed, 0 failed.
- These ran after the batteries had finished. The runner works in its own worktrees, and `git status` was checked after each battery.
- Repositories created: `t_eval2_m9a_e3`, `t_eval2_m9a_bin`, `t_eval2_m9a_b14` and `t_eval2_m9a_slash`. All were deleted and their graphs dropped by `rb gc`. `SELECT name FROM rb.repo` returns `alvorabackend, fx_date, fx_full, fx_man`.
- `git status` shows only `?? bench/__pycache__/`.
- My full m9a run rewrote `var/bench/mutants-cache.json`, which is ignored. It holds the same 31 KILLED for the same tree and specs.

## Findings

- **N1 (note): no test pins the trimming of a remote context-path value.**
  - Repro: `python3 -I bench/mutants/run.py var/m9a/eval2/mutants-eval2.json v03` gives SURVIVED. The mutation is `found = Some((file, value.trim().to_owned()))` → `value.to_owned()`.
  - Today's behaviour is right, because Spring trims: `"   "` gives the root (svc-n), and `/oscar   ` gives `/oscar` (svc-o).
  - Under the mutant, a whitespace-only remote value would become a path and would not clear. The E3 clear depends on this trim.
  - Fix: add one row (for example a `.properties` `server.servlet.context-path=/p   ` or a YAML `"  "`), and add v03 to `m9a.json`.
- **N2 (note): E2 still allows a filter that selects a wall-clock test.**
  - The kill rule holds only if the filter excludes the 7 timing tests (§3). No current spec breaks it.
  - The usage guide's "so a wall-clock test that flakes under load cannot score a kill" is unconditional.
  - Fix, either of:
    - make `run.py` mark a spec INVALID when its filter matches a known timing test (`stay_fast`, `quickly`, `are_linear`, `resolve_every_class`, `too_ambiguous`), or
    - add "with a filter that selects no wall-clock test" to the doc and lesson.
- **N3 (note, records): two amendment 3 details have no log.**
  - Amendment 3 says try1's `--no-cache` step "hit `repo.maven.apache.org:443 failed to respond`", and that the row was cleared by a `t_scipfix` index (scip-java ok). Neither appears under `var/m9a/`. `e2e-r2-try1.log` shows only `scip-java failed 70.2s (fetch phase)`, and the cleared `rb.scip_index` row is gone.
  - This is the class the new lesson "a spot check is a file, or it did not happen" covers. The outcome that matters, 52/0/0, is logged.
  - Fix: record both in a log with their source ("session tool output"), as `disk.txt` does, or reword to what the log shows.
- **N4 (note): perf "re-measured once" was applied twice for 2 targets.**
  - The NFR (contract `:75`) says a warning is re-measured once and blocks if it persists.
  - On the first re-measure (ab3), go_gin_example's mean was 1.27× of M8, and mattermost's 32.09 s mean was above 30.9 s, though 1.02× of the same-session M8. Both were re-measured again (ab4) and passed.
  - Amendment 3 discloses both numbers. ab3's medians (1.05×, 1.03×) and CPU times (1.02×) show noise, not a regression, so the conclusion stands.
  - Suggest: fix the re-measure statistic before the next close (for example, the median of ≥ 5 alternating runs, plus CPU time), so a second sample is never chosen after the fact.
- **N5 (note, prose).**
  - "The run's load was 3.95–6.0" (amendment 3) is mattermost's own run. Over all 27 warned `noscip-fresh` runs, `load_start`/`load_end` span 2.98–6.0; C11 says "4–6".
  - The lesson's "about 5 GiB each" for `/tmp/rbmut-target-*` is not logged. They are 2.0–2.7 GiB now.
  - None of these figures affects a gate.
