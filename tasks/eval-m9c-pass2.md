VERDICT: PASS (0 blocking; 3 should-fix, 6 notes). E1–E6 are each fixed or answered as claimed, and the gate, scoring, row diff, sample redraw, precision split, timing and full suite all reproduce. Two problems are new: the E2 MRO fallback takes exponential time on hierarchies C3 cannot merge (S2), and a third of the E5 sample's evidence column is misaligned (S1)

# M9c close: evaluator pass 2 (2026-10-17, verification of amendment 3)

Scope:
- Pass 1's findings E1–E6 and notes N1–N6 (`tasks/eval-m9c-pass1.md`). They were answered in `e58f6ee` (code, tests, mutants, Dockerfile comments, docs; `PIPELINE` `m9c-2`) and `80cb328` (re-close records, samples, scoreboards).
- `tasks/contract-m9c.md`: amendment 3 (`:1025-1102`) and the Result (`:1104-`).
- The binary `var/m9c/bin/rb` (`3b94ded4…`), the run `var/bench/out/3b94ded449f8/` against `f340b267d529/`, and `bench/scoreboard/m9c{,-perf}.json`.
- `bench/accept-m9c.tsv`, `bench/samples/{exact,synthetic-d8}.tsv`, `bench/probe/m9c_synthetic_sample.py`, `var/m9c/logs/r2/`.
- The records: CHANGELOG, IMPLEMENT, todo, bench-findings, lessons, README, design §13, the plan note, and `tasks/contract-m9d.md`.

Method:
- Everything was run at HEAD `80cb328`, branch `m9`.
- The probes used the binary of record. For before/after comparisons on the same fixture, I also ran `rb.r2` (`f340b267…`, the pre-fix binary, `PIPELINE` `m9c-1`) and the M9b binary.
- Probe repositories were named `t_eval2_m9c_*` in `:55433`. All were deleted before the suite ran, and again at the end.
- I wrote three mutants of my own: `var/m9c/eval2/mut/eval2-m9c.json`.
- Two read-only sub-agents helped. One checked records consistency. The other spot-checked 19 sample verdicts in the pinned checkout. I re-checked every discrepancy they reported before using it here.
- Scratch is in `var/m9c/eval2/`. I edited no file except this report.

## 1. E1, Go: verified

**Code.**
- `rb-syntax` `decorations.rs:646` (`go_type_params`) records a generic function's or type's own parameters, and the type arguments a method's receiver names (pointer receivers included).
- `resolve.rs:1591` (`result_types`) refuses a result whose name is one of them.
- `bindings.rs:1533` pushes an untyped binding for `case v[, ok] := <-ch` and none for `case v = <-ch`.

**Tests and red logs.**
- `r2/e1-red-syntax.log`: `go_type_parameters_are_facts` and `go_ranges_multi_assignment_type_switches_and_closures` fail.
- `r2/red-indexer.log`:
  - `go_call_results_and_segments_type_receivers` shows `g1 → T#Foo`, `g2 → V#Foo` and `g3 → E#Foo`.
  - `go_package_vars_of_sibling_files_type_names` shows `sel → Cfg#Load` and `sel2 → Cfg#Load`.

**My fixture** (`var/m9c/eval2/fx/go_e1`, `--no-scip`). It is pass 1's `go_b2` plus `app/e2.go`.

| Form | `rb.r2` (pre-fix) | binary of record |
|---|---|---|
| pass 1 `genericRes` (`genericParam[V any]`, package `V`) | `V#Foo` | no edge |
| pass 1 `generic` (`Id[T any]`, package `T`) | `T#Foo` | no edge |
| pass 1 `selectCase` (`case cfg := <-ch`, package var `cfg`) | `A#Bar` | no edge |
| generic method `(h *Holder[E]) Take() E` with package `E` | `E#Foo` | no edge |
| receiver renames the param `(h Holder[U]) TakeU() U` with package `U` | `U#Foo` | no edge |
| control `(h *Holder[E]) Named() *B` | `B#Bar` | `B#Bar` |
| type param named like an alias of an imported type, `Conv[Ctx any]` with `type Ctx = web.Context` | `Context#Do` | no edge |
| type param named like a dot-imported type, `Cast[Thing any]` | `dot.Thing#Run` | no edge |
| control `Mk[K any](k K) T` | `T#Foo` | `T#Foo` |
| `case v, ok := <-ch: v.Bar()`, then `v.Foo()` after the select | `A#Bar` + `A#Foo` | `A#Foo` only (after the select) |
| nested select: inner `case w := <-ch2: w.Bar()`, inner `default: w.Foo()`, outer `v.Bar()`, other case `v.Foo()` | `A#Bar` ×2 + `A#Foo` ×2 | `A#Foo` ×2 only (the package vars) |
| `case v = <-ch: v.Bar()` (assignment) | `A#Bar` | `A#Bar` (right) |
| `for { select { case w, ok := <-ch: w.Foo() } }` | `A#Foo` | no edge |

- A full diff of every `CALLS`/`REFERENCES` edge, `rb.r2` against the binary of record, removes exactly those 10 wrong edges and changes nothing else (`fx/go_e1{,_r2}.jsonl`).
- Snapshot `tool_versions.pipeline` reads `m9c-2` for the binary of record, `m9c-1` for `rb.r2` and `m9b-2` for M9b.
- One residue, which M9b's binary has too, is N1.

**Mutants.** m342–m346 are all **KILLED** in my run (`var/m9c/eval2/mut/battery.log`). My x9 (a pointer receiver's type arguments not read) is **KILLED** by `go_type_parameters_are_facts`.

## 2. E2, Python: verified (see S2 for a new performance defect)

**My fixture** (`var/m9c/eval2/fx/py_e2`, pass 1's `py_b5` plus `c3.py`, `ordr/`, `rex/`, `rex2.py` and `userex.py`). Results for the binary of record, with `rb.r2` where it differed:

| Case | Python | binary of record | `rb.r2` |
|---|---|---|---|
| `R(P, Q)`, `P(A)`, `R()` | `A.__init__` | `A#__init__` | `Q#__init__` |
| diamond `W(Y, Zz)`, `Y(X)`, `Zz(X)`, `W()` | `Zz.__init__` | `Zz#__init__` | (same) |
| Wikipedia C3 case `Z(K1, K2, K3)`, `z.m()` with `m` on `A` and `D` | `D.m` (MRO Z K1 K2 K3 D A …) | `D#m` | `A#m` + `D#m` ambiguous |
| `super().h()` in `W` (Y lacks `h`) | `Zz.h` | `Zz#h` | `X#h` + `Zz#h` ambiguous |
| `super().m()` in `D(B, C)` | `B.m` | `B#m` | `B#m` + `C#m` ambiguous |
| inconsistent MRO `Bad(X, Y)` with `Y(X)` (TypeError in Python) | TypeError | `X#__init__` (fallback); no crash | |
| cycle `Cy1(Cy2)`, `Cy2(Cy1)` | NameError | `Cy2#__init__`; no hang | |
| 80-level chain | `Deep0.__init__` | `REFERENCES Deep79#` only (64-level limit, conservative) | |
| `def f` then `from m import f` | `m.f` | `m.f` | local `f` |
| `from m import f` then `def f` | local | local | local |
| `from m import *` then `def g` | local | local | local |
| `from n import f`, `use()`, then `from m import *` | `m.f` (module-final binding) | `m.f` | `n.f` |
| `from n import f` then `from hidden import *` (`__all__ = ["g"]`) | `n.f` | `n.f` | `n.f` |
| `from m import *` then `from n import *` | `n.k` | `n.k` | ambiguous |
| re-export `rex/__init__.py`: `from n import k`, then `from m import *` | `m.k` | `m.k` | `n.k` |
| pass 1 `star_after_named`, `star_after_def` | `mod_m.f`, `mod_m.g` | same | `mod_n.f`, local `g` |

**Re-anchored specs, read against the code.**
- **m302** (`resolve.rs:1186`): it still filters `n.name != "default"` out of the loop whose `else` branch (`:1189-1203`) is D2. Same intent; the longer anchor only makes the text unique against `owners_from`'s loop at `:1701`.
- **m324**: `python_mro(class).skip(0)` makes `super()` include the class itself, which is the old "resolves to the enclosing class".
- **m325**: `if import.wildcard { if true { return Vec::new()`, so every star import brings nothing (`:1263-1266`). It is narrower than the old spec, which disabled all Python imports, and matches its `what`.
- **m326** (`_` names) and **m327** (`__all__` filter, `:1269`) mutate the same predicates at their new site.
- **m328**: `depth < MAX && depth > 0` stops the first re-export hop, the same as the old `|| depth == 0`.
- **m323** still removes the `super()` arm of `first_segment`, and the new `SH` row (`super().helper.m()`) is what reaches it.

**Mutants.** m302, m323–m328 and m347–m351 are all **KILLED** (`mut/battery.log`).

## 3. E3: verified

**Pass 1's specs**, re-run unchanged with `--jobs 1` (`mut/battery.log`):
- e1, e2, e3, e4, e6, e7, e8, e9 and e10 are **KILLED**.
- e5 is **STALE** (0 matches).
- e9b **SURVIVED**.
- `crates/**/src/*.rs` sha256 is identical before and after (92 files, `mut/crates-sha-{before,after}.txt`).

**e5 (the definition check removed).**
- `defines_all` (`resolve.rs:1644`) now reads the binding only.
- I compared `rb.r2` (both checks) with the binary of record (binding only) on star imports of modules whose `__all__` is:
  - a list;
  - a tuple;
  - an annotated assignment, `__all__: list = ["g"]`;
  - `__all__ = []` then `+=`.
- In all four, neither binary brings the unlisted `f` (`ordr/o8-o10.py`). Both binaries behave the same, so no behaviour was lost on these forms. The one form where they could differ, an annotation with no value, defines no `__all__` in Python, and dropping the definition check is right for it.

**e9b.** It is the same mutation as e9 under a narrower filter. With a constant memo key, every external symbol gets the first one's package. That changes only the `references_undocumented`/`external` split, because the hand-back keys on the symbol (`hands_back(symbol, modules)`). e9 is killed by `scip::tests`. Acceptable.

**Adoption.**
- `bench/mutants/m9c.json` has 56 specs.
- m356 (= e1), m357 (= e2), m358 (= e4) and m359 (= e8) carry pass 1's exact `old`/`new` with precise filters, and all four are **KILLED**.
- The rows exist: `resolve.rs:3069`, `:3073` and `:3076` (`a.Named("(")…`, `a.Named(").")…`, `GetList()[0].Get()`), `web/main.js` at `:2794`, and the failed duplicate (`lib.rs:3920`).

## 4. E4: verified

**Code.**
- `rb-scip` `jobs.rs:897` strips a leading U+FEFF.
- `rta.rs:403` drops `.` and empty segments.

**Red logs.** `r2/red-indexer.log` (`None` for `bom/src/utils/helper`) and `r2/red-scip.log`.

**My fixture** (`var/m9c/eval2/fx/ts_e4`, `--no-scip`; each project calls `helper.info`/`info` through an alias). The binary of record resolves every one of these; `rb.r2` resolves only `crlf`, `dot2`, `nobom`, `p2` and `p3`:
- pass 1's `bom` and `dot`;
- `p1`: BOM, CRLF and comments, with `"baseUrl": "."` and `"@/*": ["./src/*"]`, plus trailing commas;
- `p2`: `"baseUrl": "./"` with `src/*`;
- `p3`: `"baseUrl": "./src"` with `./*`;
- `p4`: `"baseUrl": "."` with `"@lib/*": ["./lib/./x/*"]`;
- `p5`: BOM, no `baseUrl`;
- `p6`: `"baseUrl": "./."`.

`p7` (`"baseUrl": "src/.."`) resolves in neither binary. Amendment 3 says `..` is kept as before (N5).

**Mutants.** m352 and m353 are **KILLED**.

## 5. E5: verified, but with a records defect in the sample (S1)

**Redraw.**
- `python3 -I bench/probe/m9c_synthetic_sample.py var/bench/out/f340b267d529/mattermost.scip.jsonl mattermost b2ce71db… var/m9c/eval2/redraw` prints "pool 10133, drawn 60, batches of 20, seed 91017".
- `drawn.tsv` and `drawn-synth-{1,2,3}.txt` are `cmp`-identical to `var/bench/labels/synthetic-m9c/`.
- The 60 identities equal `synthetic-d8.tsv`'s rows in order.
- 59 TRUE, 1 FALSE. My Wilson 95 % interval is 91.1–99.7 %.

**Verdicts in source.** 11 of 60 rows were read in the checkout: seed 20261017, plus the FALSE row. **11 of 11 agree.** The FALSE row is `updateChannelPrivacy` → `model.ChannelType#`, which is `model.ChannelType(privacy)` at `channel.go:501`, a conversion to `type ChannelType string`. **But the evidence column of rows S41–S60 is shifted** (S1).

**The conversion fix** (`scip.rs:575-593`; test `go_conversions_to_types_are_references_not_calls` at `:3391`).
- My SCIP-mode fixture `var/m9c/eval2/fx/go_e5` ran real scip-go jobs (`.` and `pub`, joined through a local `replace`).
- `go vet` is clean.
- The binary of record against `rb.r2`:

| Form | binary of record | `rb.r2` |
|---|---|---|
| `pa.Kind("x")` (a type) and `pb.Kind("y")` (a function), same file; local `func Kind` and `type Local` | `REFERENCES pa.Kind#`, `CALLS pb.Kind()`, `CALLS Kind()`, `REFERENCES Local#` (`exact`) | all four `CALLS` |
| `func shadow() { type Kind int; _ = Kind(1) }` beside a package `func Kind` | `REFERENCES shadow().Kind#` | `CALLS` |
| D8′: `model.Kind("a")`, `model.Kind("c").Ok()`, `model.NewKind("b")` | `REFERENCES Kind#`, `CALLS Kind#Ok()`, `CALLS NewKind()` (`synthetic`) | `CALLS Kind#` too |
| generic instantiation `model.Map[int, string](…)`, `model.Map(…)` | `CALLS Map()` | same |
| `model.List[int](nil)` (generic type) | `REFERENCES List#` | same |
| `model.Handler(func…)`, `model.Handler(nil)(2)`, `pa.Fn(nil)` (func types) | `REFERENCES Handler#`, `REFERENCES Fn#` | `CALLS` |
| method expressions `model.T.Method(t)`, `(*model.T).PMethod(&t)` | `CALLS T#Method()`, `CALLS T#PMethod()` | same |

- A function and a type with the same name cannot misfire, because the rule keys on the occurrence's SCIP symbol (`Kind().` against `Kind#`).
- **Mutants.** m354 and m355 are **KILLED**. My x7 (the `Language::Go` guard dropped) **SURVIVED** (N3).

**The 77 rows.**
- `r1` has 42 `exact` and 35 `synthetic` scip-go `CALLS` to a `#` symbol, all in `mattermost.scip.jsonl`, and `r2` has 0, across every `*.scip.jsonl` of both runs.
- My row diff (§8) classifies them, and the scoreboard leaves (69 `calls` and 8 `calls_file_scope` pairs, all `fn` to types) match `r2/agreement-diff-r2.txt`.

**Precision split.**
- I rebuilt the run without the 23,180 `synthetic` scip rows (`var/m9c/eval2/nosynth/`, my own filter) and scored it. Then I summed the `scip` `calls` cells:

| | gold | tp | fp | fn | P | R |
|---|---|---|---|---|---|---|
| M9b base, all targets | 35,660 | 22,246 | 6,141 | 13,411 | 78.4 % | 62.4 % |
| r2 committed, all targets | 46,119 | 44,494 | 203 | 1,622 | 99.5 % | 96.5 % |
| r2 `exact` gold only, all targets | 36,022 | 34,949 | 9,748 | 1,070 | 78.2 % | 97.0 % |
| mattermost, with `synthetic` gold | 43,771 | 42,181 | 203 | 1,590 | 99.5 % | 96.4 % |
| mattermost, `exact` gold only | 33,674 | 32,636 | 9,748 | 1,038 | 77.0 % | 96.9 % |

- This is identical to amendment 3's table (`:1051-1057`) and to `r2/e5-with-without-synthetic.txt`.
- Note that the brief's "recall 96.5 % and 97.0 %" pairs the with-synthetic all-target R with the exact-only all-target R. The records state each correctly.
- I did not re-derive the "5,944 of 6,141" pair split; it is P8's `agreement-diff.txt`, unchanged.

**Counts at r2.** `exact` scip `CALLS` 34,647 and `synthetic` 10,098 / 13,082 (`CALLS` / `REFERENCES`), counted from the dump. `mattermost.scip.status` reads `34647 exact calls (scip)`, against r1's 34,689.

## 6. E6: verified

- I copied `var/m9c/r2/e6_standalone.sh`, changing only the repository names (`t_eval2_m9c_e6{n,s}`) and the clean-up.
- I ran it once per mode on the binary of record at 20:56:51–20:57:54. Load was 3.44 → 3.05 after a quiet check: 2,406 dead `rb.edge` tuples and 0 probe repositories (`var/m9c/eval2/e6-standalone.txt`).

| mattermost | record r2 (`m9c-perf.json`) | their standalone | mine | record / mine |
|---|---|---|---|---|
| `noscip-fresh` | 26.10 s | 22.72 s | 22.42 s | 1.16 |
| `noscip-cached` | 16.63 s | 16.43 s | 16.82 s | 0.99 |
| `scip` | 20.34 s | 20.18 s | 20.11 s | 1.01 |

- The `scip` run gave 58,475 nodes and 198,983 edges, the same as the r2 dump.
- The `m9c-perf.json` mattermost entries are 26.1 / 16.63 / 20.34 s and RSS 1,922,760,704 / 1,771,028,480 / 2,413,690,880 B.
- `tasks/contract-m9d.md:137` names r2 (`3b94ded449f8`) as the base and records r1's walls as host-inflated.
- **Perf A/B.** I recomputed the medians from `r2/perf-abr2.txt`: 48 runs, all exit 0, 8 per side per mode.

  | mode | wall | CPU | RSS | instructions |
  |---|---|---|---|---|
  | `noscip-fresh` | 1.125 | 1.019 | 1.049 | 1.020 |
  | `noscip-cached` | 0.968 | 1.056 | 1.052 | 1.078 |
  | `scip` | 1.059 | 0.978 | 1.012 | 0.901 |

  Each is noise under the printed strict rule (CPU ≤ 1.10, wall ≤ 1.25, RSS ≤ 1.15, no exemption). The rule was applied correctly. See N6 for RSS.

## 7. Notes N1–N6: handled as claimed

- **N1.** The Dockerfile comment states the unpinned apt layer and why. Neither the Dockerfile nor `docker/scip-go/README.md` calls the self-test "offline" any more (`git show e58f6ee -- docker/`; `grep -i offline` finds nothing). The image ID is still `sha256:47eab865…`.
- **N2.** `tasks/contract-m9d.md:138` names the local-`replace` case and pass 1's `go_d8` fixture.
- **N3.** The rewritten mattermost `calls` accept reason splits the 203 fp into 185 skew and 18 non-skew. The Result says so too, but `:1247` still says the opposite (S3).
- **N4.** `super().m()` in `D(B, C)` gives `B#m` only (§2). `raise self.error` → `H#error` is still made (`mro.py`, my fixture) and recorded as residue.
- **N5.** The README status line now says what M9c did, with the precision caveat.
- **N6.**
  - The prompt in `bench/probe/m9c_synthetic_sample.py` and `var/bench/labels/exact-m9c-r2/prompt-1.txt:10` states "T implements I when the method set of T, or of *T, satisfies I; scip-go relates the type T in both cases".
  - The usage guide's sample bullet says the same.

## 8. Binary, bench and gate: verified

**Binary.**
- `shasum -a 256 var/m9c/bin/rb` is `3b94ded449f8e56a…9db6`, and `rb.r2` is `f340b267…06c9`.
- `crates/rb-indexer/src/lib.rs:154` has `PIPELINE = "m9c-2"`, at `e58f6ee` and at HEAD. `git diff e58f6ee HEAD -- crates migrations Cargo.toml Cargo.lock` is empty.
- `r2/build.log` records HEAD `e58f6ee`, "crates/ and migrations/ clean", and the sha.
- Snapshots written by the binary record `pipeline: m9c-2` (§1).

**Scoring.**
- `rb-bench score … --rb-commit e58f6ee… var/bench/out/3b94ded449f8`, run twice (1.80 s, 1.67 s), is `cmp`-identical between runs, and `cmp`-identical to the committed `m9c.json` and `m9c-perf.json` (`var/m9c/eval2/score/`).
- Re-scoring `36ed5ea46ddf` with `--rb-commit b651523…` is `cmp`-identical to `var/m9c/r2/m9b-rescored{,-perf}.json`. That file differs from `var/m9c/m9b-rescored.json` only in `/keys/exact:verdicts`.

**Gate.**
- The quoted `target/release/rb-bench compare var/m9c/r2/m9b-rescored.json bench/scoreboard/m9c.json --accept bench/accept-m9c.tsv --perf var/m9c/r2/m9b-rescored-perf.json bench/scoreboard/m9c-perf.json` gives **exit 0**: 45 `changed`, 6 `accepted` and 3 `warning` lines (mattermost peak RSS).
- Its output is line-identical to `r2/compare.txt` without its 3 header lines.
- `shasum` of `bench/accept-m9c.tsv` is `3211ff58…`, the sha `compare.txt` records.

**The two rewritten accept reasons** (`git diff e58f6ee 80cb328 -- bench/accept-m9c.tsv`) hold:
- **mattermost `calls`:** fn 12,672 → 1,590 and gold 33,312 → 43,771.
  - The 69 pairs that left the gold are all r1 `fn` to types (`agreement-diff-r2.txt`: `AccessDecisionReason`, `AutocompleteArgType`, `MemberInvite` …).
  - The 9,748 fp against `exact` gold reproduces (§5).
  - The 185 + 18 split is pass 1's.
- **mattermost `calls_file_scope`:** gold 978 → 974 and fn 0.
  - The 8 r1 `fn` pairs are all file-scope references to types (`cloud.go` → `BillingScheme`, `BillingType`, `ProductSku`, `RecurringInterval`, `SubscriptionFamily`; `notify_admin.go` → `MattermostFeature`; `channel_guards.go` → `ClusterEvent`; `handlers.go` → `APIHandlerOption`).
  - 978 − 6 + 2 = 974 is consistent with r1's +4 (2 conversions, `root.go`, and the `NewId` fp turned tp).

**Row diff** (`var/m9c/eval2/rowdiff/rowdiff.py`, my own script, keyed by the row's identity with provenance).
- `cmp` of all 157 dumps (recursive, same file set): **155 identical**. `mattermost.scip.jsonl` and `pycg/mro__two_parents.jsonl` differ.
- **mattermost: 154 rows, 0 unclassified.**
  - 77 `CALLS` to a `#` symbol were removed: 42 `exact`, 35 `synthetic`.
  - 58 `REFERENCES` were added, each with the same source, target, certainty and site set as a removed `CALLS`.
  - 19 `REFERENCES` were extended, each new site set a strict superset that contains the removed `CALLS`'s sites.
- **two_parents: 2 rows.** The `ambiguous` `CALLS B#func()` was removed, and `A#func()` went from `ambiguous` to `heuristic`.
- **Total: 156.**

**The 53 redrawn `exact` edges.**
- `exact.tsv` gains 54 lines (a comment and 53 verdicts, `:388-441`) and loses 0.
- All 53 are TRUE, and their sha256 `a9607324…` is the scoreboard's `exact:verdicts`. Scoring shows `scip` 60 right, 0 missing.
- 8 drawn by seed 20261018 were read in source, and **8 of 8 agree**. Among them: `validateKanbanProps → NewAppError` (`view.go:199`); `inviteUsersToTeam → App#Srv` (`team.go:1755`, `c.App` being `*app.App` through `web/context.go:20`); `MakeWorker → JobServer#HandleJobPanic` (`worker.go:26`, in the `execute` closure).
- A mechanical check found that every one of the 53 rows' evidence names its own edge's ends, unlike S1.

## 9. Tests and lint

**Full suite.**
- Command: `CARGO_INCREMENTAL=0 RB_TEST_DATABASE_URL=…55433 DOCKER_CONTEXT=desktop-linux cargo test --workspace --no-fail-fast -- --test-threads=1`, after `cargo build -p rb-cli` (`var/m9c/eval2/test-full.log`). The tree was clean; it ran 20:58–21:14 at load 2.61 → 2.42.
- **Result: 1,156 passed, 0 failed, 0 ignored, 0 `skipping:`**, exit 0, over 26 result lines. This equals the claim.
- Per crate: rb-indexer 376, rb-syntax 125, rb-scip 82, rb-frameworks 205, e2e 52, api 1.
- No timing test failed, so nothing was re-run.

**fmt and clippy.** `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` both exit 0 (`fmt-clippy.log`).

**Mutants** (`--jobs 1`, 21:23–21:28, `mut/battery.log`):
- m302, m323–m328 and m342–m359: **25 of 25 KILLED**.
- Pass 1's specs: as in §3.
- Mine:
  - x9 KILLED.
  - x7 SURVIVED (N3).
  - x10 SURVIVED. x10 memoises a partial MRO; S2 needs such a change, and no test pins its result.
- The batteries for m9b, m9a and m8b were not re-run, because pass 1 changed none of their code paths' specs. Their r2 logs read 39/31/29.

## 10. Records consistency

These agree with each other and with what I reproduced:
- the binary, the `PIPELINE` and the run;
- 1,156 tests and unit counts 125/82/205/376;
- batteries 56/39/31/29;
- 59 of 60 (91.1–99.7 %), 77 = 42 + 35, 156 rows, 53 TRUE, 45/6, exit 0;
- the precision table, the walls 26.10/16.63/20.34 and 22.72/16.43/20.18, and the A/B ratios;
- 34,647, and 23,180 = 10,098 + 13,082;
- gold 43,771, fn 1,590, gold 974.

The qualified precision headline (78.2 % against `exact` gold, 5,944 of 6,141) is in CHANGELOG, IMPLEMENT, todo's Review, the plan note, bench-findings B1, README and the Result. The exceptions are in S3.

## Findings

**S1 (should-fix, records): `bench/samples/synthetic-d8.tsv` carries the wrong evidence for 20 of its 60 edges (S41–S60, file lines 45–64).**
- **Cause.** The third judge built a zsh array of 20 evidence strings and wrote `"${ev[$i]}"` with `i` from 0. zsh arrays are 1-based (`judge-synth-3.jsonl`, the `Bash` call that writes `verdicts-synth-3.tsv`). As a result:
  - S41 (`Schedulers#Start → mlog.Error`) has an empty evidence field;
  - S42–S60 each carry the previous edge's evidence. For example, S47 `makeTeamEditionJobServer` cites `base_workers.go:78` `logger.Warn` in `DoJob`;
  - S60's own evidence (`sync_send_remote.go:923`) is dropped.
- **Scope.** Rows S1–S40 and all 53 `exact.tsv` rows are aligned: a mechanical check found the evidence names one of the edge's ends in every one.
- **Verdicts are unaffected.**
  - Every batch-3 verdict is TRUE, and the transcript's array holds the right evidence in edge order.
  - The source spot-check (done by my read-only sub-agent and summarised in §5) re-read S43, S51 and S52 against their own edges: all TRUE.
  - The 59 / 1 count stands.
- **The audit claim does not hold.** Amendment 3 (`contract-m9c.md:1047`) says the citations were checked and that "No verdict rests on a wrong line". The citation checker tests that a `file:line` exists, not that it belongs to the edge, so it could not see this. For S41 and S60 the published sample has no evidence for its own edge.
- **Fix:**
  - Rebuild the evidence column of `verdicts-synth-3.tsv` and `synthetic-d8.tsv` from the transcript's array, shifted back by one.
  - Correct the audit sentence.
  - Add an "evidence names the edge" check to the sample tooling.

**S2 (should-fix, new defect in the E2 fix): the MRO fallback is not memoised, so a Python hierarchy C3 cannot merge makes indexing time exponential in its depth.**
- **Code.** `mro_at` (`resolve.rs:1421-1459`) memoises a result only when it is `whole`. A class whose bases include a cycle, an inconsistent order or a depth cut is never memoised. So every class above it recomputes the whole sub-lattice, and each level doubles the work.
- **Repro.** Fixtures `var/m9c/eval2/fx/py_acyc{16,20,24,28}` and `py_lat`: a two-wide diamond lattice `Lka(L(k-1)a, L(k-1)b)` on top of `class T0`, `class T1(T0)` (so `L1a(T0, T1)` is an inconsistent MRO), then `make(): return Lna()`. `--no-scip --no-cache`, wall time:

  | layers | binary of record | `rb.r2` | M9b |
  |---|---|---|---|
  | 16 | 0.38 s | 0.22 s | 0.21 s |
  | 20 | 0.84 s | 0.17 s | 0.18 s |
  | 24 | 10.47 s | 0.22 s | |
  | 28 | 165.02 s | 0.21 s | |
  | 40 | killed at 240 s | | |

  - With a cycle at the base instead (`T0(T1)`, `T1(T0)`), 22 layers take 5.9 s.
  - The **valid** 40-layer lattice (`py_valid40`, accepted by Python) takes 0.18 s and gives the right `T0#__init__`. So only hierarchies Python rejects are affected.
- **Why it matters.** rb resolves bases heuristically and indexes code that does not run, so one unmergeable diamond in a mixin-heavy codebase, or one wrong base resolution, can hang an index run on attacker- or accident-shaped input. The pre-fix binary is flat. No bench target moved (§8), and no test covers a cycle's or an inconsistent hierarchy's MRO result or time (x10 SURVIVED).
- **Fix:**
  - Memoise the fallback order per class too, or memoise a per-class "unmergeable" marker and take `depth_first` (already linear, with a `seen` set) from it.
  - Add a wide-lattice row with an inconsistent base to a timing test (or a call-count assertion), plus a row pinning the fallback MRO, so x10 is killed.

**S3 (should-fix, records): the Result still states r1 facts as current in places that contradict amendment 3.**
- `contract-m9c.md:1245`: the accept-lines table row for mattermost `calls_file_scope` reads "fn 6 → 8 … fn: 1 B1 join, 1 D3-doc pair". The accept file and `compare.txt` now accept `gold 978 → 974`, with fn 6 → 0 (`:1235` says so).
- `:1247` has three such statements:
  - "fn fell to 1,659" (r2: 1,590);
  - "The mattermost `calls` line's reason calls the 190 remaining base fp 'version-skew calls'" (the rewritten reason splits 185 and 18);
  - "The accept file is not edited, since `compare.txt` records its sha256". The accept file was edited in `80cb328`, which the same Result says at `:1235`.
- Minor, of the same kind:
  - `:1133`: "whose 157 dumps the run of record reproduces byte for byte" (r2 differs in 2 dumps).
  - `:1291`: the NFR argument about "the binary of record" is P7b's.
  - `IMPLEMENT.md:904-905`: "on the final code after P7b". The final code is `e58f6ee`, with 1,156 tests and 56/56.
  - `IMPLEMENT.md:898-902`: r1 numbers under r1's sha, with an r2 sentence appended.
  - `tasks/todo.md:1216`: "amendments 1–2". `:1245`: fn 1,659 and R 96.3 % without r2's 1,590 and 96.5 %.
  - `tasks/bench-findings.md:17`: 23,186 (10,133 / 13,053) and 34,689 / 44,822 unlabelled as r1. At r2 they are 23,180 (10,098 / 13,082) and 34,647.
- Fix: correct them in place, or label each as r1, as was done elsewhere.

**N1 (note): a parameter or local var typed by a type parameter still types a same-named package type.**
- In `go_e1/app/e2.go`, `func Use[T any](x T) { x.Foo() }` gives `CALLS T#Foo()`, and `func (h *Holder[E]) Each(…) { var e E; e.Foo() }` gives `CALLS E#Foo()`. Both are `heuristic` and both are wrong.
- The M9b binary makes both edges too, so this is pre-existing and outside pass 1's E1, which was about results.
- It has the same root, and `Definition::type_params` now exists for Go, so refusing a binding whose `type_name` is an enclosing type parameter is cheap.

**N2 (note): a star import of an `__all__` module does not take part in "later binding wins".**
- `from ordr.n import g` then `from ordr.hidden import *`, where hidden has `__all__ = ["g"]`, gives `CALLS n.g`. Python binds `hidden.g`.
- D5 skips every `__all__` target by design, so the star "brings nothing" and the earlier named import wins.
- `rb.r2` does the same. The fix would read the list, as D5's out-of-scope line already anticipates.

**N3 (note): no test pins the conversion rule to Go.**
- My x7 (`files[path].0 == Language::Go` → `true`) SURVIVED under `scip::tests`.
- It would turn any non-Go scip `CALLS` to a `#` symbol into `REFERENCES`. r2's dumps have 0 such rows in every language, so no bench row depends on it today.
- One test row would pin it, for example a scip-python or Java call whose occurrence is a class symbol.

**N4 (note, records): "r1" and "r2" mean different things in different places.**
- `var/m9c/bin/rb.r1` is P7's binary (`930271d4`), and `rb.r2` is P7b's (`f340b267`).
- The Result's "r1" means P7b's run (`:1108`), but its perf table and A/B rows use "r1" for P7 (`:1275`, `:1285-1287`).
- Nothing is wrong, but it is easy to misread.

**N5 (note): known limits seen in my fixtures, all conservative.**
- An 80-level Python chain gets no `__init__` edge (64-level cap).
- `"baseUrl": "src/.."` does not resolve (`..` kept).
- `import rex2; rex2.k()` through a star + named re-export gives no edge in either binary.

**N6 (note): mattermost peak RSS varies about 12 % on the same binary.**
- My standalone `noscip-fresh` peaked at 2,153,119,744 B, against the record's 1,922,760,704 B (1.12×).
- M9d's NFR allows 1.15× RSS against that record before an A/B. Expect warnings that the A/B will have to settle.

## Safety

- `SELECT * FROM rb.repo ORDER BY 1` is identical before, after the probes, before and after the suite, and at the end (`var/m9c/eval2/repo-{before,after-probes,before-suite,after-suite,after-probes2}.txt`; 4 rows: `fx_man`, `fx_date`, `fx_full`, `alvorabackend`).
- **Probe repositories.**
  - 24 `t_eval2_m9c_*` repositories were made and deleted by my clean-up (`var/m9c/eval2/cleanup.sh`, pass 1's SQL with my prefix). 0 are left.
  - 0 `rb_t_eval2%` graphs and 0 orphan snapshots remain. `rb gc --prefix t_eval2_m9c_` had nothing to remove.
  - One index run (`py_lat_a`, 40 layers) was killed by my 240 s alarm before it created a repository row.
  - The empty `var/scip/cache/t_eval2_m9c_goe5_new` was removed. Its `rb.scip_index` rows, keyed by input hash, remain.
- No e2e ran while a probe existed: all probes were deleted before the suite, and the S2 probes were made after it.
- **Mutants.** No mutant touched a DB write path: they cover rb-syntax facts, rb-indexer resolve/scip/rta/lib join logic and rb-scip JSONC. `crates/` sha256 is identical before and after (92 files).
- **Git.** No commit, no push, no checkout, reset or stash in the project. `git status` shows only the pre-existing `bench/__pycache__/` and `worktrees/`.
- **Disk.** `target/debug` was rebuilt (2.9 GiB, `CARGO_INCREMENTAL=0`). Free disk was never below 12 GiB. No `/tmp/rbmut-target-*` was created (`--jobs 1`).
