VERDICT: PASS (0 blocking; 3 should-fix, 4 notes). The workspace suite did not come back green on this machine: 3 failures under external load (S3)

# M9b close: evaluator pass 2 (2026-10-16, verification of amendment 7)

Scope:
- Pass 1's findings E1–E6 (`tasks/eval-m9b-pass1.md`), as answered in `b651523` (code, tests, runner, docs) and `2a9c7d6` (re-close records).
- In the contract: `tasks/contract-m9b.md` amendment 7 (`:319-370`), the Result (`:372-488`), and amendments 5 and 6 as amended.
- The binary `var/m9b/bin/rb`; the run `var/bench/out/36ed5ea46ddf/`; `bench/scoreboard/m9b{,-perf}.json`; `bench/accept-m9b.tsv`; the logs in `var/m9b/logs/` and `var/m9b/logs/r2/`.
- The CHANGELOG, IMPLEMENT, todo, bench-findings, lessons, README, design §13 and `tasks/contract-m9c.md` entries.

Method:
- Tools were at HEAD `2a9c7d6` (branch `m9`). `target/{debug,release}/{rb,rb-bench}` were fresh against HEAD.
- Two fixtures of my own were indexed into `:55433` with the binary of record: `t_eval2_m9b_e3` (Go) and `t_eval2_m9b_e2` (Java/Kotlin). A third, `t_eval2_m9b_chk`, was used for one timing probe.
- 8 mutant specs are my own.
- Scratch is `var/m9b/eval2/`. I edited no file except this report.

## 1. E3: a named-type constant no longer folds (verified)

**The fix.** `crates/rb-indexer/src/sql.rs:370` reads `let constant = b.constant && b.type_name.is_empty();`.

**The test.** `sprintf_sql_folds_package_constants` (`sql.rs:980`) adds:
- `type Tbl string` with a `String()` method;
- row `N` (`tStringer Tbl`), which refuses;
- row `O` (`tTyped string`), which folds to `public.t_typedstr`.

`r2/e3-red.log` shows `N(). -> public.t_typed` emitted before the fix, and the assertion red.

**My own fixture** (`var/m9b/eval2/fx/e3`, binary of record, `--no-scip`, evidence in `fx/summary.txt`). Package `store` declares:
- `type Ident string` with `String()`;
- `type Oops string` with `Error()`;
- `type Bare string` with no method;
- a constant of the other package's `other.Name`, which has `String()`;
- a `string` constant and an untyped constant, each once at top level and once inside a `const (…)` group (`grpStr string`, `grpUntyped`, `grpIdent Ident`).

| site | constant | result |
|---|---|---|
| `QStr`, `QGrpStr` | declared `string` | `QUERIES → public.ev_str` / `ev_grp_str`, `heuristic` |
| `QUntyped` (`%v`), `QGrpUnt` | untyped | `QUERIES → public.ev_untyped` / `ev_grp_untyped`, `heuristic` |
| `QIdent` (`%s`), `QIdentV` (`%v`), `QGrpIdent` | `Ident` (has `String()`) | no edge |
| `QOops` | `Oops` (has `Error()`) | no edge |
| `QBare` | `Bare` (no method) | no edge (conservative) |
| `QOther` | `other.Name` (has `String()`) | no edge |
| `QDigit` | `%d` with a `string` constant | no edge |

- `sql_folded store/q.go {constants 4, sites 4}`.
- Snapshot `tool_versions.pipeline` = `m9b-2` for both fixtures (`rb.snapshot`).

**Mutants.**
- m296 (`b.constant && …` → `b.constant`) is **KILLED** in my run (`mut/m9b-sample.log`).
- My t5 (named types whose name starts with `T` fold) is **KILLED** (`mut/eval2.log`).
- m267, re-anchored on the same line, is **KILLED**.

## 2. E2: every configurer form caps (verified)

**The fix.** `crates/rb-indexer/src/routes.rs:520-540`, for Java or Kotlin files, treats any of these as a configurer:
- a supertype named `RepositoryRestConfigurer(Adapter)`;
- a definition whose `return_type` names one of them;
- a chain rooted at `RepositoryRestConfigurer` whose first link is `withConfig`.

The test `data_rest_endpoints_are_synthetic` (`routes.rs:2006`) has rows `adpt`, `bean`, `kcfg`, `kwith` and `neg`. `r2/e2-red.log` shows `bean`, `kcfg` and `kwith` `synthetic` before the fix.

**My own fixture** (`var/m9b/eval2/fx/e2`). It has 8 services, each with a data-rest pom and one `@RepositoryRestResource` `CrudRepository` with one finder: 7 endpoints. Evidence is in `fx/summary.txt`.

| svc | configurer | EXPOSES / HANDLED_BY | `data_rest_base_unread` |
|---|---|---|---|
| sbeanad | `@Bean public RepositoryRestConfigurerAdapter restAdapter()` returning an anonymous Adapter (return type is the Adapter) | 7 / 7 `heuristic` | 1 |
| sbeanfq | `@Bean` returning a fully qualified `org.springframework…RepositoryRestConfigurer` | 7 / 7 `heuristic` | 1 |
| swithj | Java `@Bean Object cfg() { return RepositoryRestConfigurer.withConfig(…); }` (`withConfig` alone) | 7 / 7 `heuristic` | 1 |
| sktad | Kotlin `class RestConf : RepositoryRestConfigurerAdapter()` | 7 / 7 `heuristic` | 1 |
| sktimpl | Kotlin `class RestConf : RepositoryRestConfigurer` | 7 / 7 `heuristic` | 1 |
| smulti | Java subclass + two `@Bean` methods + `withConfig` + Kotlin class, in one service | 7 / 7 `heuristic` | **1** (not 4) |
| sneg | returns `RepositoryRestConfiguration`, `Builder.withConfig(…)`, returns `RepositoryRestConfigurerFactory` | 7 / 7 `synthetic` | 0 |
| splain | none | 7 / 7 `synthetic` | 0 |

So each configured service is capped and counted once, and the uncapped cases stay `synthetic`.

**Mutants.**
- m297, m298 and m299 are **KILLED** (`mut/m9b-sample.log`).
- My x2 (the `withConfig` root on the wrong type) is **KILLED**.
- My extra probe x1 **SURVIVED**. It drops the `rsplit('.')` in `configurer`, so a qualified name no longer caps. See N1. Today's behaviour is right (`sbeanfq` above).

## 3. E1: the evaluator's survivors are killed (verified)

**Pass 1's specs.** I re-ran all seven: `python3 -I bench/mutants/run.py var/m9b/eval/mut/eval-m9b.json e1 … e7 --jobs 2`. They gave **7/7 KILLED**, none STALE or INVALID (`var/m9b/eval2/mut/pass1-specs.log`).

**The m9b sample.** I re-ran the 12 specs added or re-anchored after pass 1 (`--jobs 2`; `mut/m9b-sample.log`): m267, m279, m280, m281, m282, m283, m284, m295, m296, m297, m298 and m299. All 12 are **KILLED**.

**The tests named in E1 are there:**
- `%d` alone: row `P` (`sql.rs:986`, `func P() … "SELECT id FROM %d", usersTable`). It has one verb and one value. m280 (`'s' | 'v'` → `'s' | 'v' | 'd'`) is KILLED under the exact test filter, and my t6 (the same mutation, filter `sql::tests`) is KILLED. So the `%d` refusal is now bitten on its own; row `H` still covers the count mismatch.
- A local `fmt`: row `Q`; m279 is KILLED.
- `data_rest_services` has its own test: `lib.rs:3212`, `data_rest_services_need_the_dependency_and_read_the_base_path`. It covers a pom, a `build.gradle` and a `build.gradle.kts` with the dependency, a pom without it but with a base path, `basePath=/rest/`, `base-path: /api/`, and a blank `"  /  "`. m281 (the gate) and m282 (the `basePath` key) are KILLED.
- A function-local type (`resolve.rs:1878`, `svc/loc.go`): m284 is KILLED.
- `findAllById` (`rb-frameworks/src/routes.rs:699`, test row `:1994`): m295 is KILLED.

**Ids.** `bench/mutants/m9b.json` holds 39 specs, m261–m299, with no gap and no duplicate. m8a is 151–200, m8b 201–229, m9a 230–260. No file has an id ≥ m300.

## 4. E4: the runner matches cargo's full path (verified)

**The file.** `bench/mutants/timing_tests.txt` holds `crate path file` rows.
- 17 rows: rb-cli 3, rb-frameworks 4, rb-indexer 4, rb-scip 3, rb-server 2, rb-syntax 1. That is **17 tests in 6 crates**.
- All 14 lib paths occur exactly in `cargo test -p <crate> --lib -- --list` (`var/m9b/eval2/lib-test-list.txt`). The 3 rb-cli rows are integration tests (`tests/e2e.rs`), listed bare.
- A copy of `timing_tests.py` with only `root` and the output path changed regenerates the file **byte-identical** (`var/m9b/eval2/regen/`).

**The rule.** `run.py:59-65`, `selects_timing`, returns `p for c, p in timing if c == crate and (test is None or test in p)`, which is cargo's substring rule. My specs (`mut/eval2-m9b.json`, log `mut/eval2.log`):

| spec | filter | result |
|---|---|---|
| t1 | `resolve` (rb-indexer) | INVALID, names both `resolve::tests::…_stay_fast` |
| t2 | `tests` (rb-indexer) | INVALID, names `di::…`, both `resolve::…` and `stitch::…` (4) |
| t3 | `quickly` (rb-frameworks) | INVALID, names `tests::wide_python_hierarchies_resolve_quickly` |
| t4 | `tests::a_` (rb-server) | INVALID, names both rb-server timing tests |
| t5 | `sql::tests::sprintf_sql_folds_package_constants` (precise) | ran, KILLED |
| t6 | `sql::tests` (a module with no timing test) | ran, KILLED |

t1–t4 have no matching `old` text, so they would have been STALE had the timing check not come first (`run.py:153-160`). They were reported INVALID, so the check runs before anything else.

**Records.** "17 … in 6 crates" appears in amendment 5 (`contract-m9b.md:293`), amendment 7 (`:339`), the usage guide (`docs/usage-and-testing.md:355`), CHANGELOG `:7` and todo `:1191`. No "7 crates" remains outside the sentences that name the correction. See N2 for a wrong cross-reference.

## 5. E5 and E6: records (verified)

**E5.**
- `var/m9b/logs/adjudicate.txt` has 12 ADD lines. By my own reading they are:
  - 5 item resources: customers, prices, stores, accounts, items;
  - 2 collection resources: `/prices`, `/items`;
  - 5 search resources: `findByLastName`, `findByFirstName`, `findByStoreName`, `findByName`, `findByEmail`.
- That equals `r2/e5-split.txt`, amendment 6 (`:306`, corrected in place with its reason), amendment 7 (`:344`), `bench-findings.md:153`, CHANGELOG `:9`, IMPLEMENT `:870` and todo `:1192`.
- jferrater "17 of 18" with `/customers/search/findById` missed is stated in:
  - amendment 7 `:345-346`;
  - the Result cell table (`:404`) and aggregates (`:452`, "74 of 75 (98.7 %)");
  - `bench-findings.md:154` ("Known gold omission");
  - Out of scope (`:248`);
  - CHANGELOG `:10` and IMPLEMENT `:870`.
- The key file stays unedited (`mse_jferrater.md:27` still says "5 item … and 7 search"). The records name this sentence as wrong and give the reason (blind keys stay blind). That is a reasoned departure from pass 1's suggested correction line, and it is disclosed.

**E6.**
- C9 (`:118`) and the Result (`:412`) quote the full command, `--perf var/m9b/m9a-rescored-perf.json bench/scoreboard/m9b-perf.json`.
- `compare-r2.txt` opens with HEAD `c85835a`, the accept file's sha256 `74f6fc77…` and `exit=0`. My `shasum` of `bench/accept-m9b.tsv` is `74f6fc77…`.
- Without its 2 header lines and its `exit=` line, `compare-r2.txt` is identical to `compare-acceptfix.txt`.
- `compare-r3.txt` cites `--perf bench/scoreboard/m9a-perf.json …`. That file is `cmp`-identical to `var/m9b/m9a-rescored-perf.json`, so both commands are the same gate.
- `GetClusterId` is an open item in `tasks/contract-m9c.md:271`, naming the 3 call sites and the two evidence files.

**E10.** `e2e-r2.log` opens with HEAD `c85835a` and the working-tree diff sha256 `42adc898…`. My `git diff c85835a b651523 -- crates migrations | shasum -a 256` is `42adc898…`, so the r2 e2e ran on the committed code.

## 6. Binary, bench and gate (verified)

**Binary.**
- `shasum -a 256 var/m9b/bin/rb` is `36ed5ea46ddf03410f0bec53062072becc6a3dbed63e2383afd62f5791ab2903`, and `rb.sha256` agrees. `rb.r1` is `3fc09844…`.
- `b651523:crates/rb-indexer/src/lib.rs:145` has `PIPELINE = "m9b-2"`. Snapshots written by the binary record `tool_versions.pipeline = m9b-2` (§1).
- `build-r2.log` records HEAD `b651523` before and after the build, "crates/ and migrations/ clean", `build exit=0`, and `target/release/rb` = `36ed5ea4…`.
- **The rebuild.** Free disk was 10 GiB, so I ran `cargo build --release -p rb-cli` at HEAD (`var/m9b/eval2/rebuild.log`). Before it, `git diff b651523 HEAD -- crates migrations Cargo.toml Cargo.lock` was empty and the status of those paths was clean. The build was fresh (1.36 s), and `target/release/rb` hashes to `36ed5ea4…`. This is a fingerprint-fresh rebuild, not one from scratch: there was not enough disk for a clean target.

**Re-score.**
- `target/release/rb-bench score var/bench/out/36ed5ea46ddf --milestone m9b --rb-commit b651523… --write … --perf …`, run twice (`var/m9b/eval2/score/`).
- The two outputs are **`cmp`-identical** to each other and to the committed `bench/scoreboard/m9b.json` and `m9b-perf.json`. They took 10.68 s and 10.56 s.
- A leaf-by-leaf diff of `b651523:bench/scoreboard/m9b.json` (r1) against my re-score gives exactly 2 leaves: `rb.commit` and `rb.binary_sha256`.
- `var/m9b/score/m9b-r2-second{,-perf}.json` are also `cmp`-identical to the committed files.

**Gate.**
- `rb-bench compare var/m9b/m9a-rescored.json bench/scoreboard/m9b.json --accept bench/accept-m9b.tsv --perf var/m9b/m9a-rescored-perf.json bench/scoreboard/m9b-perf.json` gives **exit 0**: 25 `changed`, 3 `accepted` and 24 `warning` lines (`score/compare.txt`).
- The 3 `accepted` lines are exactly the 2 cells of `accept-m9b.tsv`: `mse_jferrater … service_flows` (1 line) and `scip:mattermost:noscip:calls` (fp and out of scope, 2 lines).
- The output is identical to `compare-r3.txt` without its header.
- Outside the warnings, it is identical to `compare-r2.txt`, which is r1's scoreboard. So the r2 binary changes no gated line.
- All 24 warnings are `noscip-fresh` walls. r1's mattermost `scip` RSS warning is gone.

**r2 against r1.**
- My own per-file `cmp` of the 157 `*.jsonl` dumps in `var/bench/out/3fc098445243/` and `36ed5ea46ddf/`: same file set, **157 byte-identical, 0 differ**, none empty. That is stronger than the record's "0 differing rows".
- `r2-diff-control.txt` (base → r1) finds 7,713 differing rows in 5 dumps, which equals the invariant's 7,568 + 28 + 117.
- The run's `.meta` files name `binary_sha256 36ed5ea4…`.
- `bench-run-r2.log`: 27 "fresh and cached dumps identical", `failures: 0`, `repositories left like bench\_%: 0`. `.meta` load spans 5.15–8.74 (65 files), and the outputs span 18:30:17–18:44:25.
- `r2/sample-missing-r2.txt`: total missing 0.

**Perf.** I recomputed every median in `r2/perf-ab5.txt` (`var/m9b/eval2/perf-ab5-recompute.txt`).
- 144 runs, all exit 0.
- Every ratio in `perf-ab5-summary.txt` reproduces.
- The worst median wall is 1.111 (fastapi_realworld). mattermost is wall 1.078, CPU 1.009, RSS 1.037.
- Six targets have a median CPU ratio over 1.1. See S1.

## 7. Tests and hygiene

**fmt and clippy.** `cargo fmt --all --check` exits 0, and `cargo clippy --workspace --all-targets -- -D warnings` exits 0 (`var/m9b/eval2/fmt-clippy.log`).

**Full suite.** `cargo test --workspace --no-fail-fast -- --test-threads=1` with `RB_TEST_DATABASE_URL=…55433` and `DOCKER_CONTEXT=desktop-linux` (`var/m9b/eval2/test.log`, HEAD `2a9c7d6`, clean tree):
- **1,133 passed, 3 failed, 0 ignored, 0 `skipping:`** over 26 result lines.
- 1,136 tests in all, which is P5's 1,135 plus the new `data_rest_services_…` test. rb-indexer 363 and rb-frameworks 205 match `r2/unit-*-r2.log`.
- The three failures:
  - `rb-syntax tests::deep_chains_and_nested_calls_extract_quickly` (`JavaScript: 8.20 s` against a 5 s bound). It is a listed wall-clock test.
  - e2e `a_checkout_is_its_repository_whatever_its_name`: `cypher query cancelled by its statement timeout of 2000 ms`.
  - e2e `check_gates`: empty `rb check --json` stdout.
- Re-run twice in isolation (`test-rerun.log`, `test-rerun2.log`), the same three failed the same way (7.08 s and 7.45 s; the same timeout).
- No Maven fetch timeout occurred.
- See S3 for the evidence that the cause is external.

**Hygiene.**
- **My probe repositories were removed by the suite, not by me.** The test harness's start-up clean-up deletes every `t_*` repository (`crates/rb-cli/tests/common/mod.rs:53-65`). The `tests/api.rs` binary runs first, so `t_eval2_m9b_e3` and `t_eval2_m9b_e2` were gone after the suite.
- I deleted `t_eval2_m9b_chk` myself (`DELETE FROM rb.repo …` then `rb gc --prefix t_eval2_m9b_`, which dropped 1 graph).
- No `rb_t_eval2*` graph and no orphan snapshot remain.
- `SELECT * FROM rb.repo ORDER BY 1` is **identical** before and after (`var/m9b/eval2/repo-{before,after}.txt`): `fx_man`, `fx_date`, `fx_full`, `alvorabackend`.
- No mutant touched a DB write path. The mutated files were `sql.rs`, `routes.rs` (both crates), `lib.rs`'s `data_rest_services` and `resolve.rs`.
- `git status` shows only the pre-existing `bench/__pycache__/` and `worktrees/`, and no `rbmut` worktree is left.
- `/tmp/rbmut-target-{0,1}` were reused, not deleted.

## 8. Records consistency

These agree with each other and with the evidence above:
- amendment 7, the Result, CHANGELOG (`:3-24`), IMPLEMENT (`:865-874`), todo (`:1187-1196`), the design §13 row (`:1001`, "evaluator pass 1 answered (amendment 7), pass 2 pending") and README (`:317`);
- the binary `36ed5ea4…` from `b651523`, `PIPELINE` `m9b-2`;
- batteries 39/31/29;
- unit 363/205/79 and e2e 52/0/0;
- the 5/2/5 split, 17 of 18, 6 crates, compare exit 0 with 25/3, and 24 warnings with a worst median wall of 1.111.

The r1 sha `3fc09844…` appears only where it describes r1: todo P6 `:1185`, the CHANGELOG 16:55 entry, Result `:376` and C7 `rb.r1`. The exceptions are S2 and N3.

## Findings

**S1 (should-fix): six of the 24 perf warnings are called noise against a rule they do not meet.**
- The perf rule is amendment 0, `contract-m9b.md:262`: noise is "CPU time within 1.1×, medians within the limit". Amendment 7 (`:370`) says "All 24 are noise under amendment 0's rule", and `perf-ab5-summary.txt` writes `noise` on every row.
- From `perf-ab5.txt`, median CPU r2/M9a is over 1.1 on 6 targets:

  | target | median CPU ratio |
  |---|---|
  | express_ts_boilerplate | 1.303 |
  | mse_callista | 1.300 |
  | mse_anilallewar | 1.228 |
  | go_gin_example | 1.135 |
  | nestjs_realworld | 1.125 |
  | mse_mdeket | 1.105 |

- The conclusion very probably holds:
  - The absolute CPU is 0.2–1.2 s, and the per-run ranges overlap (express M9a 0.27/0.33/0.47 against r2 0.31/0.43/0.47).
  - The median walls are 0.897–1.069 on those targets.
  - The 157 dumps are byte-identical.
  - The code change adds two cheap checks.
- But the record applies a looser rule than the one it cites, and the summary script's verdict column does not test CPU.
- Fix: say that these 6 exceed the CPU criterion and why they are still noise (absolute size, overlapping ranges). Or re-measure them with more runs, as the rule's "until the difference is classified" allows. Make `perf_ab5_summary.py` print the criterion it applies.

**S2 (should-fix, records): the Result's perf table shows r1's numbers under the committed r2 file's name.**
- `contract-m9b.md:478-484` is headed "`bench/scoreboard/m9b-perf.json` against `m9a-perf.json`". It lists mattermost m9b `noscip-fresh` 20.89 s / 2,075,607,040 B, `noscip-cached` 14.47 s, `scip` 18.43 s / 2,662,825,984 B.
- The committed `m9b-perf.json`, re-derived by my re-score, now has 111.04 s / 960,151,552 B, 70.43 s and 88.76 s / 1,529,020,416 B. `2a9c7d6` replaced 522 lines of it.
- The Result's preamble (`:376`) says "The numbers below come from the first run and hold unchanged for r2". That is true for every quality number, but not for perf.
- Fix: label the table as r1's (`b651523:bench/scoreboard/m9b-perf.json`), or add the r2 column with its load (5.15–8.74).

**S3 (should-fix, environment): pass 2 could not reproduce a green workspace suite.**
- Result: 1,133 passed / 3 failed / 0 skipped (§7). All three failures are clock or statement-timeout failures, in crates that M9b has not touched since the green P5 suite: `git diff 3b83d68 HEAD -- crates/rb-syntax crates/rb-cli crates/rb-store crates/rb-api crates/rb-server` is empty.
- The machine was 2–6× slower than at P5 for reasons outside this repository. Times:

  | measure | now | at P5 or r2 |
  |---|---|---|
  | rb-indexer lib tests | 129.2 s | 22.9 s |
  | rb-frameworks lib tests | 61.9 s | 10.8 s |
  | e2e | 2,421 s | 943 s |
  | `rb-bench score` (release) | 10.6 s | 1.5 s at r1 |
  | rb-syntax timing test, release build | 3.8 s | "fast" (it is linear) |
  | `python3 -c "sum(range(10**7))"` | 0.37–0.46 s | about 0.1 s on an M4 |

- Host memory was at 15 G used, 6.6 G compressor, 70–94 MB free and 2.9 G swap. GeForceNOW and WindowServer were at about 30–37 % CPU.
- `rb check --json` on a 138-node graph (`t_eval2_m9b_chk`) is cancelled at the 2,000 ms default ×2 and finishes in 2.28 s with `--timeout-ms 30000`. This is the same failure as `check_gates`.
- I recorded this rather than working around it. The r2 records (unit 363/205/79, e2e 52/0/0 on the committed code) are the latest green evidence, and my lib totals match them.
- Fix: re-run the 3 tests, or the suite, when the machine is idle, and log the result before the close is recorded.

**N1 (note): no test pins that a qualified configurer name caps.**
- My x1 (`name.rsplit('.').next()` → `Some(name)`) SURVIVED under `data_rest_endpoints_are_synthetic`. Every test row uses imported simple names.
- Today's behaviour is right: `sbeanfq`, which returns a fully qualified `org.springframework…RepositoryRestConfigurer`, is capped.
- Fix: one row with a qualified return type or supertype, plus x1 as a mutant.

**N2 (note, records): amendment 5's correction cites the wrong finding.**
- `contract-m9b.md:293` says '"7 crates" before pass 1's E5 was a miscount'. Pass 1 raised it under **E4** (`eval-m9b-pass1.md:82`).

**N3 (note, records): "17 of 17" survives uncorrected in three summary lines.**
- todo's Review (`tasks/todo.md:1199`, "mse_jferrater endpoints 0 → 17 of 17"), IMPLEMENT `:855` and CHANGELOG `:36` still say it.
- In each file a later line gives 17 of 18 (todo `:1203`, IMPLEMENT `:870`, CHANGELOG `:10`).
- The CHANGELOG and IMPLEMENT lines are r1's dated entries. The todo Review line is current, though, and reads as fact.

**N4 (note): probe naming and the test harness.**
- The brief's `t_eval2_m9b_*` names fall under the e2e start-up clean-up (`t_*`). Any probe repository still present when a suite starts is deleted by the suite. That is harmless here, and the user's repositories have no `t_` prefix.
- Future evaluators who keep fixtures across a suite run should use a different prefix, or finish their probes before the suite.
- The new lesson ("scoring timed with the debug `rb-bench` took 36 s, against the record's 1.5 s") attributes the gap to the build profile only. Today's release scoring also takes 8–11 s. S3's slowdown accounts for the rest, and 30 s still holds.
