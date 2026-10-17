VERDICT: PASS (0 blocking; 6 should-fix, 6 notes)

# M9c close: evaluator pass 1 (2026-10-17)

Scope: `tasks/contract-m9c.md` (D1–D8′, C1–C16 with C8 withdrawn, amendments 1–2, notes PI–P7b, Result), `tasks/plan-m9.md` M9c, the diff `e428d15..5f2eefc -- crates docker bench/mutants bench/probe` (commits `f5d80b0` … `5f2eefc`), `docker/scip-go/Dockerfile` and the images `rb-scip-go:0.2.7` / `rb-scip-go:0.2.7-xtools0.50.0`, the binary of record `var/m9c/bin/rb` (`f340b267d529…`), the run of record `var/bench/out/f340b267d529/` against the M9b record `36ed5ea46ddf/`, `bench/scoreboard/m9c{,-perf}.json`, `bench/accept-m9c.tsv`, `bench/samples/exact.tsv`, `var/m9c/logs/`, and the records (CHANGELOG, IMPLEMENT, todo, bench-findings, README, design doc, plan).

Method:
- Tools: `var/m9c/bin/rb` and `var/m9c/bin/rb.r1`, `var/m9b/bin/rb`, `target/release/rb-bench` (`82839c81…`, the record's), `python3 -I` scripts.
- Six probe repositories in `:55433`, `t_eval1_m9c_{gob2,tsb3,pyb5,god8,gin,ab_*}`, all deleted. Fixtures are in `var/m9c/eval1/fx/`.
- The scip-go images were run offline (`--network none`) on two fixtures. A minimal SCIP decoder is `var/m9c/eval1/scipdec.py`.
- I wrote 10 mutants of my own (`var/m9c/eval1/mut/eval-m9c.json`, runner `--jobs 1`).
- Scratch is in `var/m9c/eval1/`.

**What reproduces:**
- **Scoring.** `rb-bench score` of `f340b267d529` twice gives `cmp`-identical output, scoreboard and perf both. Against `bench/scoreboard/m9c.json` it differs in exactly one leaf, `rb.commit`, which I passed as `5f2eefc`. The perf file is `cmp`-identical to the committed one. Re-scoring `36ed5ea46ddf` with `--rb-commit b651523…` is `cmp`-identical to `var/m9c/m9b-rescored.json` (`score/`).
- **Gate.** The quoted `compare … --accept bench/accept-m9c.tsv --perf …` exits 0 with 41 `changed` and 6 `accepted` lines. Its output is line-identical to `logs/p7b/compare.txt`. Without `--accept` it exits 1 with 6 REGRESSION lines. I recounted 17 of 314 cells changed, the same 17 the record lists.
- **Invariant.** `invariant.py`, re-run into my scratch with only its two output paths changed (`inv/`), gives an `invariant.txt` that is `cmp`-identical to `logs/p7/invariant.txt`. `row-buckets.jsonl` holds the same rows; only the order differs, and the sorted sha1 is equal.
- **The 368 D3-doc `exact` scip `CALLS`.** I checked 15 random ones (seed 91) in the pinned checkout. **15 of 15 are TRUE** (`d3doc.txt`): `th.App.BulkImportWithPathAndOpts`, `mlog.Int`, `api.APISessionRequired`, `a.ListDirectory`, `setHandlerOpts`, `NewS3FileBackend`, `fileBackend.WriteFile`, `mlog.String` and others.
- **New `exact` `IMPLEMENTS`.** The 13 new scip `IMPLEMENTS` on mattermost (7 IMG, 6 D3-doc) were read in source and hold structurally: `driverConn` is satisfied by `*Conn`'s 8 methods, `rowScanner` by the embedded `*sqlx.Rows`, and `i18n.PluginAPI` (4 methods) by `apiRPCClient`.
- **D8′ synthetic edges.** 15 random `synthetic` scip `CALLS` (seed 4242) are 15 of 15 TRUE (`b1.txt`): `model.SafeDereference`, `WebSocketEvent#Add`, `request.Context#WithSession`, `model.NewAppError`, `Logger#Warn`, `PostList#Extend`, `RemoteClusterFrame#IsValid` and others.
- **Accepted pairs.** 20 random accepted out-of-scope pairs (`accepted.txt`) are **20 of 20 correct**: 12 mattermost `calls` (call segments `Setup(t).InitBasic(t)`, call results such as `newTestDatabaseStore` → `*Store` → `Store.SetFile`, package var `mainHelper.Parallel`, and TS through `mattermost-redux/*`), 3 mattermost file-scope, 3 bookcars and 2 fastapi inherited `__init__`.
- **Skew fp.** 6 random fp among the 185 Go fp are all absent from `server/public@v0.4.3` in the module cache and present in-repo.
- **The 18 fp the record calls "not skew"** are listed in `fp18.txt`. 15 are recursive `walk(p)` self-calls in `docs/site/scripts/*.mjs`, and 3 are `this.prop(…)` calls in e2e-tests TS. All 18 were already fp in M9b (N3).
- **Invariant spot check.** 17 more rows across B2 (`+` and `~`), B3j, B3d and hk_todo's B2 are correct edges, and each sits in its stated bucket (`inv30.txt`). With the 50 above, that is 67 rows read.
- **Sample.** The judge prompt (`judge-scip-1.jsonl` row 0) is blind and adversarial ("ASSUME EACH EDGE IS FALSE"; reads limited to the checkout). Each `verdicts-scip-N.tsv` matches its drawn headers 18 of 18. `bench/samples/exact.tsv` only gains lines (+55, −0: 54 verdicts and a comment), and `sample-missing-after.txt` reads 0.
- **Output identity (P7b).** `rb.r1` and `rb` indexing go_gin_example in SCIP mode give `cmp`-identical dumps, and both are `cmp`-identical to the run of record's `go_gin_example.scip.jsonl`.
- **D8′ end to end.** Fixture `go_d8` (`t_eval1_m9c_god8`, real SCIP mode, 4 scip-go jobs) behaves as specified:
  - Root module `example.com/srv` gets a local `replace example.com/pub => ./pub`.
  - Two modules declare `example.com/dup`.
  - The calls cover `model.NewId()`, `u.Name()`, the promoted `u.Hello()` from an embedded `Base`, the generic `model.MakeBox[int](1).Get()`, a struct literal, `strings.ToUpper` and the constant `model.Version`.
  - Results:
    - `NewId`, `User#Name`, `Base#Hello` (the right declaring type) and `Box#Get` are `CALLS`; `MakeBox` and `User#` are `REFERENCES`. All have certainty `synthetic` (3).
    - `d.Do()` gives no edge and counts `ambiguous` 1 against both `dup1` and `dup2`.
    - stdlib and namespace symbols are not handed back.
    - `rb status` reads `0 exact calls (scip)`.
  - On the run of record, mattermost's scip `CALLS` are 34,689 `exact` and 10,133 `synthetic`, and the status line reads `34689 exact calls (scip)`. No site has both a heuristic and a synthetic `CALLS` (0 of 21,781).
- **Image.**
  - The pins are as claimed: Ubuntu digest, Go tarball sha256 per architecture, the v0.2.7 tag with its commit checked, and `go.mod`/`go.sum` sha256 checked after `go get`. `go mod verify` runs, the build uses `-mod=readonly`, and `go version -m` must list x/tools v0.50.0.
  - **The self-test fails closed.** Its script, run offline in each image, exits 1 on `rb-scip-go:0.2.7` and 0 on `rb-scip-go:0.2.7-xtools0.50.0`. `set -eux` makes the RUN fail.
  - **The cache is keyed by image ID.** `Images::for_job` returns the image ID, `projects.rs:201` stores it as `project.image`, and `StoreCache::get` looks up `scip_index_get(input_hash, image)` (`rb-cli/src/main.rs:1657`). The ID is `sha256:47eab865…`.
  - No other code pins the old tag. `doctor.rs` and `e2e.rs` use the constant. The remaining `0.2.7` strings are the indexer version and comments.
  - **Spot comparison on my own fixtures, old image against new:**
    - `go_b2` (two own packages): 308 = 308 occurrences, identical sets.
    - A stdlib-heavy package (`sync.Mutex` embed, generics, `net/http`, `sort`, `errors`): old 66, new 80, **0 lost**, 14 gained, all stdlib members.
- **Perf.**
  - P7b's A/B medians, recomputed from `perf-ab7b.txt`, match the summary: wall 1.061, CPU 0.975, RSS 1.031, instructions 0.903.
  - My ABBA, 2 runs per side on mattermost, gives:
    - `noscip-fresh`: wall 24.38 / 21.61 s, CPU 1.019×, instructions 1.023×;
    - `scip`: wall 20.24 / 19.81 s, CPU 0.981×, instructions 0.903× (`ab.txt`).

    Both are noise under the record's rule.
- **Tests and lint.**
  - `cargo test -p rb-indexer --lib` 375, `-p rb-scip --lib` 82 and `-p rb-syntax` 124 pass (`unit.txt`).
  - `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` exit 0.
  - The P8 full-suite log sums to 1,154 passed, 0 failed, 0 ignored, 0 `skipping:` over 26 test binaries.
  - The P8 batteries read m9c 38, m9b 39, m9a 31 and m8b 29 KILLED, none cached; `crates-sha-{before,after}` are equal.

## Findings

**E1. Should-fix: two Go Tier B forms make wrong `heuristic` edges: a generic result named by a type parameter, and a `select` receive variable that falls back to a package var.**
- Repro: fixture `var/m9c/eval1/fx/go_b2` (`t_eval1_m9c_gob2`, `--no-scip`; dump `go_b2.jsonl`).
  - `app/more.go:19-20`: `w := genericParam(A{})`, then `w.Foo()`, where `func genericParam[V any](v V) V` and the package also declares `type V struct{}` with `func (V) Foo()`.
    - Edge: `genericRes() CALLS V#Foo()`, `heuristic`.
    - Go calls `A.Foo`. The result `V` is the type parameter, not the package type.
    - Same in `app/use.go:38-39`: `t := Id(B{})`, then `t.Foo()` gives `T#Foo()` (`func Id[T any](x T) T` beside `type T struct{}`).
  - `app/use.go:95-98`: `select { case cfg := <-ch: cfg.Bar() }` with `ch chan *B` and the package var `var cfg = &A{}`.
    - Edge: `selectCase() CALLS A#Bar()`. The right callee is `B#Bar`.
    - The receive statement declares no binding, so `first_segment` falls through to `package_var` (`resolve.rs:1827-1836`).
  - Controls that are right in the same fixture:
    - block shadowing (`x := NewA(); if … { x := NewB(); x.Foo() }; x.Bar()` gives `B#Foo` then `A#Bar`);
    - range, type-switch, parameter, closure-parameter, named-result and receiver shadows of the package var;
    - a use before a local `cfg :=`;
    - `(T, U, error)`, `(A, B)` and named results are all refused;
    - `GetList()[0].Foo()` and `Fn()()` give no edge;
    - an alias to another package's type, and two-level embedding.
- Cause:
  - `result_types` (`resolve.rs:1422-1431`) resolves the result text in the callable's file without the callable's type parameters.
  - `bindings.rs` has no arm for `receive_statement` inside `communication_case`.
- Impact: `heuristic` only. 0 on mattermost by a scan: 6 generic functions return a bare type parameter and none collides with a type of their directory; 128 `case x := <-` and none is named like a package var of its directory.
- Breaks: D4 ("A result is usable when it is one named type"; a type parameter is not a named type) and the package-var bullet's spirit (a name with a local declaration never resolves through the package table).
- Fix:
  - Refuse a result whose name is one of the callable's type parameters (read them from the declaration's `type_parameters`).
  - Push an untyped binding for the left side of `case x := <-ch` (and `x, ok :=`).
  - Add both rows to `go_call_results_and_segments_type_receivers` and `go_package_vars_of_sibling_files_type_names`.

**E2. Should-fix: Python's inherited `__init__` is breadth-first, not the MRO, and a star import never overrides an earlier named import. Both give wrong `heuristic` edges in ordinary code.**
- Repro: fixture `var/m9c/eval1/fx/py_b5` (`t_eval1_m9c_pyb5`; dump `py_b5.jsonl`).
  - `mro.py:22-38`: `class A: __init__`, `class P(A): pass`, `class Q: __init__`, `class R(P, Q): pass`, and `R()`.
    - Edge: `make() CALLS Q#__init__()`, `heuristic`.
    - Python's MRO is R, P, A, Q, so `A.__init__` runs. This is not a diamond: P and Q share no base.
    - The P4 note's caveat ("not C3 for deeper diamonds") understates it. Any left base that inherits `__init__` loses to a right base that declares one.
  - `star_after_named.py`: `from mod_n import f`, then `from mod_m import *` (`mod_m` defines `f`), then `f()`.
    - Edge: `use() CALLS mod_n.f()`.
    - Python binds `f` to `mod_m.f`, because the later statement wins. `star_after_def.py` (a `def g` before `from mod_m import *`) has the same issue.
- Impact: `heuristic`. PyCG's cases pass (`two_parents` has no inherited left base). No bench target was measured for it.
- Breaks: D5 ("Inherited `__init__`. `constructor` uses `members(class, "__init__")`"). `members` walks breadth-first; Python's method resolution is C3, which here equals left-first depth-first.
- Fix:
  - Walk the supertypes in C3 order (left-first DFS with the last occurrence of a shared base kept) for `__init__` and for `super()`.
  - Order named and star imports by source position, so the later binding wins (or refuse a name that both bring).
  - Add both rows to `python_raise_and_inherited_init_construct` and `python_super_star_reexport_and_module_vars_resolve`.

**E3. Should-fix: 4 of my 10 mutants on the new code survive with real behaviour changes, and 2 more survive as probably equivalent.**
- Repro: `python3 -I bench/mutants/run.py var/m9c/eval1/mut/eval-m9c.json e1 e2 e4 e5 e6 e7 e8 e9 e9b e10 --jobs 1` and `… e3` (logs `mut/eval-m9c{,-2}.log`).
  - The first attempt used broad filters (`resolve::tests`, `tests::`), which select wall-clock tests, so the runner refused them as INVALID. The filters were then narrowed to test names that `--list` shows exist.
  - `crates/` was byte-identical afterwards (sha256 of every `src/*.rs`).
- KILLED:
  - e3 (`parse_jsonc` drops `,` before `}` only);
  - e6 (namespace symbols handed back);
  - e9 (the P7b package memo keyed by a constant), killed by `scip::tests`; its variant e9b, under the D8′ test alone, survives;
  - e10 (package var used despite a same-named definition).
- SURVIVED, with a behaviour change no test pins:
  - **e1** (`resolve.rs:450`): `split_top` stops skipping string and rune literals. `a.B(")").C()` mis-splits. Filter `resolve::tests::go`.
  - **e2** (`resolve.rs:500`): `call_segment` accepts any segment that starts with a call. A qualifier `GetList()[0].Foo()` would then type `[0]` as `List` and call `List.Foo`, a wrong edge. The contract's "any other bracketed segment stops the walk" has no row for `f()[0]`.
  - **e4** (`resolve.rs:1178`): D2 stops resolving default imports in JavaScript files. D2 names "TS/JS", but no JS row exists.
  - **e8** (`lib.rs:1088`): failed scip-go jobs become D8′ candidates, so a failed job that shares the target's module path makes every join `ambiguous`. D8′ says "exactly one ok job"; C9′'s test has no failed job.
- SURVIVED, probably equivalent:
  - e5 (`__all__` checked only as a binding): `__all__ = […]` is both a definition and a binding (P4 reading).
  - e7 (the hand-back ignores the manager): every symbol in a scip-go index has manager `gomod`.
- Breaks: the usage guide's §5.4 rule ("a surviving mutant is a test bug until proven otherwise") for D2, D4 and D8′.
- Fix: rows for `a.B(")").C()`, `GetList()[0].Foo()`, a `.js` default import, and a D8′ job list with a failed duplicate. Add e1, e2, e4 and e8 (or equivalents) to `bench/mutants/m9c.json`.

**E4. Should-fix: two tsconfig forms still lose their `paths`. The P1 note says one of them is recorded, and it is not.**
- Repro: fixture `var/m9c/eval1/fx/ts_b3` (`t_eval1_m9c_tsb3`, `--no-scip`). Each project has `src/main.ts` calling `helper.info()` and `info()` through `@/util/helper`.
  - `crlf/`: CRLF line endings, `//` and multi-line `/* */` comments (one holding `"quoted"`), a `//` URL, an escaped quote with `,}` inside a string, and trailing commas in nested arrays and objects. **It resolves** (2 sites).
  - `bom/`: the same strict tsconfig as `nobom/` with a leading UTF-8 BOM. **It resolves nothing.** `nobom/` (control) resolves. `parse_jsonc` hands the BOM to `serde_json`, which rejects it; `tsc` accepts it. The bench has no BOM tsconfig.
  - `dot/`: `"baseUrl": "."` with `"@/*": ["src/*"]`, the canonical form. **It resolves nothing.** `dot2/` (no `baseUrl`, `./src/*`) and bookcars' `"./"` resolve.
- The P1 note (contract, "Seen, not fixed") finds the `join_dir` cause itself: `"baseUrl": "."` gives `mobile/./src/…`. It then says "It is recorded beside `baseUrl`-only bare specifiers." The Out-of-scope bullet (`contract-m9c.md:275`) lists only "`baseUrl`-only bare specifiers (mattermost `components/…`)". The Result's open items, `bench-findings.md` B3 and the M9d contract do not mention `"baseUrl": "."` either. The gap is pre-existing (`join_dir`), but it sits on D1's path and is unrecorded.
- Breaks: D1 ("as `tsc` reads a `tsconfig.json`") for the BOM. For `"baseUrl": "."` it breaks the P1 note's recording claim.
- Fix: strip a leading U+FEFF in `parse_jsonc`; normalise `./` and `.` in `join_dir`; add rows to `tsconfig_jsonc_resolves_aliases`. Record the `"."` case in B3's open items if it is not fixed.

**E5. Should-fix: D8′'s `synthetic` edges became gold for the gate's biggest cell, but no judged sample covers them, and the precision headline does not say so.**
- Evidence:
  - `rb-bench`'s gold keeps every `scip` `CALLS` whatever its certainty (`crates/rb-bench/src/scip_ref.rs:48`, `e.provenance == Provenance::Scip`).
  - The 10,133 `synthetic` `CALLS` (21,781 sites) therefore enter `scip:mattermost` gold. That is how 5,944 of the 6,141 base fp became tp and 871 out-of-scope pairs moved to tp (Result, "mattermost fp"; "Gold, 33,312 → 43,840").
  - The `exact` sample never draws them (amendment 2: "D8′ edges never enter the pool"). The certainty table's `synthetic` row is "83.0 % of 47, unchanged", which measures other edges.
  - So no judged sample measures the precision of 23,186 graph edges and of the reference they now help define. My 15-edge spot check (above) found 15 of 15 TRUE, which is reassuring but not a protocol sample.
  - CHANGELOG ("scip `calls` agreement P 78.4 % → 99.5 %"), the plan's M9c note and todo's Review quote the precision gain without saying that most of it is the reference gaining rb's own joined edges. The Result and bench-findings B1 do say it.
- Breaks: the M8b protocol's intent that every `scip` edge class serving as truth is sampled; the records rule against overclaiming.
- Fix:
  - Add a `synthetic`-`scip` stratum to `rb-bench sample` (or judge a one-off 30-edge D8′ sample under the M8b prompt), and report the cell with and without synthetic gold.
  - Qualify the headline in CHANGELOG, the plan note and todo: fp 6,141 → 203 is mostly gold expansion (5,944), and Tier B in-scope fp grew by 13.

**E6. Should-fix: the run of record's mattermost walls are about 3× what the same binary takes, and M9d's NFR uses them as its base.**
- Evidence:
  - The run of record's mattermost walls are `noscip-fresh` 69.55 s, `noscip-cached` 74.60 s and `scip` 71.81 s (`bench/scoreboard/m9c-perf.json`; `logs/p8/runs-perf.txt`).
  - The same binary, re-timed by me today at load 3.4–4.6 (`var/m9c/eval1/ab.txt`), takes `noscip-fresh` 24.14 and 24.62 s and `scip` 20.62 and 19.85 s. That agrees with P7's r1 run (24.51 / 16.85 / 25.22 s), whose dumps are byte-identical.
  - The Result admits "The walls were not re-measured on mattermost after run 2". Its host evidence is hk_polls only.
  - `tasks/contract-m9d.md:136` sets M9d's NFR to "`noscip-fresh` wall ≤ 1.25× … the M9c run of record". The A/B rule runs only after a warning. Against a 69.55 s base, a wall regression up to about 3.6× (≈ 87 s) raises no warning, so M9d's wall NFR cannot fire as written. P7b's diagnosis of host and DB wait (autovacuum after the probe deletes) is plausible, but it is not measured on mattermost.
- Breaks: C14's evidence is sound for M9c itself, since every verdict comes from an A/B. The problem is the perf record M9c hands on.
- Fix: re-time the three mattermost commands on a quiet host after a `VACUUM` and record them as the perf base (or name P7 r1's `m9c-perf.json` as the base); amend M9d's NFR to say which. At least record in M9d that `m9c-perf.json`'s walls are host-inflated.

## Notes

**N1. Image reproducibility.**
- The content pins are sound.
- The apt layer (`ca-certificates curl git`) is unpinned; this is pre-existing.
- The image ID changes per build (attestation, amendment 2 Risks), so every rebuild re-indexes every Go job.
- The Dockerfile and README call the self-test "offline", but `docker build` runs it with network. It needs none: a stdlib-only module.
- The self-test only proves stdlib export data loads. That covers the root cause (one decoder for every import), not a module-specific failure.
- P0b's 68 occurrences lost "at their range" on server remain unclassified. My two fixtures lost 0.

**N2. D8′ with a local `replace`.** In my `go_d8` fixture, srv type-checks against the in-repo `./pub` through `replace`, yet the joins are `synthetic`. That is conservative and matches D8′ ("always `synthetic`"). M9d's go.work question would want `exact` here; worth a row when it is decided.

**N3. The 18 "not skew" fp are likely correct edges, not wrong ones.** 15 are recursive `walk(p)` calls inside `walk` (`docs/site/scripts/gen-*.mjs`), and 3 are `this.prop(id)` calls in e2e-tests page objects. All were fp in M9b too. The accept line's "190 version-skew calls" slip is disclosed in the Result. The Result could also say these 18 are reference gaps.

**N4. Python edge cases labelled `ambiguous` or contrived.**
- `super().m()` in `class D(B, C)` gives `ambiguous` edges to both `B.m` and `C.m`; Python calls `B.m`.
- `raise self.error` with a method `error` gives `CALLS H#error()`.
- `raise e` on a caught exception adds one unresolved call reference per site (fixture `py_b5`; fastapi's counter moved 137 → 134 net).

**N5. README status line.** It says "M9c built 2026-10-17" but has no sentence on what M9c did (M9a and M9b each have one). P8 lists "README status line" as done.

**N6. Judge convention.**
- E137 `Group# IMPLEMENTS Auditable#` (pointer receiver) was judged TRUE against the prompt's own Go rule ("method set satisfies the interface"). M9a's sample did the same (`GroupWithUserIds`), so it is consistent.
- The prompt should state the convention (scip-go relates `T` when `*T`'s method set satisfies the interface) rather than leave judges to override it.

## Safety

- `SELECT * FROM rb.repo ORDER BY 1` is identical before and after (`var/m9c/eval1/repo-{before,after}.txt`, 4 rows: `fx_man`, `fx_date`, `fx_full`, `alvorabackend`).
- `t_eval1_m9c_*` repositories and `rb_t_eval1%` graphs left: 0. The `go_d8` module cache `var/scip/cache/t_eval1_m9c_god8` was removed. Its 4 `rb.scip_index` cache rows, keyed by input hash, remain.
- No e2e ran. No mutant touches a DB write path: all are indexer, scip-join or JSONC logic, and none is in rb-store.
- No docker image was built, deleted or retagged. Containers ran `--rm --network none` on copies in `var/m9c/eval1/img/`.
- `worktrees/` and `bench/__pycache__/` were not touched. No commit, no push. Disk was 9.5 GiB free at the end.
