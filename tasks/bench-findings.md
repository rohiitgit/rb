# Accuracy benchmark findings (M8a onwards)

Each finding comes from a scoreboard (`bench/scoreboard/<ms>.json`), and each number was produced by a tool (`rb-bench render`, or the edge-set scripts named in the finding), never typed by hand (lesson L3). The benchmark measures and does not fix (contract M8a D1); fixes go to later milestones, like the stress batches.

**First baseline:** the M7 close binary (`e08efcb`, `var/m7j/bin/rb`, sha256 `a038653b…`), scored in M8a on 2026-10-13.
- **Scoreboard:** `bench/scoreboard/m8a.json`.
- **Raw dumps:** `var/bench/out/a038653b8478/` (gitignored). The edge-set script used for B1–B3 is quoted in each finding.

## B1. SCIP mode drops calls across Go modules (major, SCIP recall)

- **What:** On mattermost, `--no-scip` has **10,599** `CALLS` from `server/` into `server/public/`, a separate Go module with its own `go.mod` and scip-go job. The SCIP-mode dump has **0** `scip` edges across that boundary and keeps only about **2,871** heuristic ones, from files that SCIP did not cover. So SCIP mode loses about 7,700 real calls, and the reference itself has none.
- **Effect on M8a's numbers:** **6,108 of the 6,147** `scip/calls` disagreements on mattermost (99.4 %) are such calls, so mattermost's agreement precision reflects a gap in the reference, not Tier B errors. Of the 17,157 misses, **0** cross a module boundary. These counts were recomputed after amendment 4 with aliases mapped; before it, 6,108 of 6,189.
- **Evidence:** each sampled extra edge is a real call, e.g. `server/channels/api4/group.go:1082` → `model.NewAppError` (`server/public/model/utils.go:370`).
- **Likely cause:** the SCIP join matches a reference's symbol against definitions from the same index. A cross-module reference names the other module's package (`github.com/mattermost/mattermost/server/public …`), and that symbol is defined only in the other job's index.
- **Next:** fix the join (a cross-index symbol table per repository) before any agreement threshold is set on mattermost. Re-score: the gate will show `gold` rising on `scip:mattermost`, which needs re-scoring the base (exit 2 is not involved, because SCIP references aren't keys).

## B2. Tier B misses Go calls through receivers and constructors (major, Tier B recall)

- **mattermost:** `scip/calls` recall **48.5 %** (16,155 of 33,312). **5,819** of the 17,157 misses start in `_test.go` files. The sampled forms are:
  - method calls through chained receivers (`th.App.UpdatePropertyField`, `s.GetReplica()`);
  - calls on a variable whose type is a constructor's return type;
  - interface methods with one implementer (`Auditable`).
- **go_clean_arch:** recall **75.5 %** (37 of 49). All 12 misses are test functions calling a service or repository built by a constructor (`article/service_test.go:84` → `GetByID`).
- **go_gin_example:** recall **94.4 %** (152 of 161).

## B3. Tier B misses TS calls through namespace imports behind path aliases (major, Tier B recall)

- **bookcars:** `scip/calls` recall **62.5 %** (1,155 of 1,849); **666** of the 694 misses are in `.tsx` files.
- **Sampled form:** `import * as helper from '@/utils/helper'` (a tsconfig `paths` alias) and then `helper.info(…)` (`admin/src/pages/ChangePassword.tsx:15,67`). The same goes for services imported the same way (`UserService.getUser`).
- **express_ts_boilerplate:** recall **69.2 %** (54 of 78), with 0 extra edges. Before amendment 4 it showed 2 extra edges, an artifact: SCIP merges a static and an instance `User#hashPassword`, and Tier B keeps them apart.
- **Evaluator pass 2** found that 610 of bookcars' 694 misses are namespace imports behind an `@/` alias.

## B4. SCIP coverage: 6 of 11 SCIP targets measure nothing (infrastructure)

| Target | SCIP outcome (from `rb status`) | Agreement cell |
|---|---|---|
| spring_boot_microservices | scip-java failed (fetch phase) | vacuous |
| fastapi_realworld | scip-python failed (fetch phase) | vacuous |
| nestjs_realworld | scip-typescript failed (fetch phase), after 1,530 s cold | vacuous |
| kotlin_spring_realworld | scip-java failed (index phase) | vacuous |
| microblog | no SCIP job (no recognised Python build manifest) | vacuous |
| microservices_demo | adservice scip-java failed (fetch); 4 scip-go jobs ok | scored, 21 of 49 files covered |
| mattermost | `webapp`, `e2e-tests/cypress` scip-typescript and `tools/sharedchannel-test` scip-go failed (fetch) | scored, 2,376 of 6,121 files covered |

So Java, Kotlin and Python have **no** SCIP agreement measurement in M8a, and PyCG is Python's only key. The fetch failures repeat M5's and the stress campaign's: workspace installs, unpinned lockfiles, egress.

## B5. PyCG: Tier B recall by Python feature (accuracy, independent key)

**Totals:** `calls` precision **92.3 %**, recall **25.0 %** (12 of 48); `calls_file_scope` precision **98.9 %**, recall **50.5 %** (94 of 186).

**Wrong edges (2 in total):**
- `decorators/return_different_func`: rb keeps `func2 → func` through the decorated function's body, while PyCG follows the decorator's rebinding.
- `mro/two_parents`: an `ambiguous` edge to `B.func` beside the correct `A.func`. That is a site hit, so `ambiguous` worked as labelled.

**Recall by category (tp / in scope):**
- 0 %: assignments 0/15, exceptions 0/3, dynamic 0/1
- 20–30 %: builtins 1/5, kwargs 2/10, functions 1/4, dicts 5/19, direct_calls 3/10
- 36–43 %: decorators 8/22, lists 5/13, generators 7/17, args 6/14
- 56–69 %: mro 9/16, imports 8/14, returns 8/12, classes 36/52
- 100 %: external, lambdas (in scope after exclusions)

**The 44 cases with recall under 50 %** (S10; counted per case, both classes summed: 57 if counted per cell), by category, with the form that misses:
- **assignments** (4), calls through a variable bound to a function or tuple: `chained`, `recursive_tuple`, `starred`, `tuple`
- **dicts** (8), calls through dict values: `add_key`, `assign`, `call`, `ext_key`, `nested`, `param_key`, `type_coercion`, `update`
- **lists** (4), calls through list elements: `ext_index`, `nested`, `simple`, `slice`
- **functions** (3), **classes** (3), **kwargs** (3), **direct_calls** (3), calls through an assigned or returned function value (`a = f; a()`, `f()()`), through parameters, or through keyword arguments:
  - functions: `assigned_call`, `assigned_call_lit_param`, `imported_call`
  - classes: `assigned_call`, `parameter_call`, `tuple_assignment`
  - kwargs: `assigned_call`, `call`, `chained_call`
  - direct_calls: `assigned_call`, `return_call`, `with_parameters`
- **decorators** (3), the decorator's returned function: `nested_decorators`, `param_call`, `return_different_func`
- **exceptions** (3), raise and constructor calls: `raise`, `raise_assigned`, `raise_attr`
- **generators** (3), calls inside iteration protocols: `iter_return`, `iterable`, `iterable_assigned`
- **imports** (3), star imports and package `__init__` re-exports: `import_all`, `init_import`, `submodule_import_all`
- **args** (1) `nested_call`; **builtins** (1) `map`; **dynamic** (1) `eval`; **mro** (1) `super_call`

Most of these forms need data flow (values bound to names, containers, returns), which Tier B does not claim. Star imports, `super()` and constructor calls in `raise` are syntactic and are candidates for a Tier B batch.

## B6. Time and memory (continuity with M5)

On the same Apple M4, mattermost `--no-scip`:

| Run | Fresh | Cached | Peak RSS fresh / cached |
|---|---|---|---|
| Run 1 (cold SCIP pass) | 22.2 s | 14.6 s | 1.77 / 1.62 GiB |
| Run 3 | 20.9 s | 15.9 s | 1.87 / 1.60 GiB |
| Run 4, perf of record (`bench/scoreboard/m8a-perf.json`) | 24.7 s | 18.1 s | 1.68 / 1.55 GiB |
| M5 | 29.2 s | 19.6 s | 1.51 GiB |
| Stress campaign | 26.1 s | 20.2 s | 1.55–1.78 GiB |

- **Wall time** is down since M5.
- **Fresh RSS** runs 1.11–1.24× M5's across these three runs on a shared host. Run 3 is above the gate's 1.15× warning line, and run 4 is below it.
- The run-to-run spread (±0.1 GiB, ±2 s) is the noise floor that the perf warnings have to live with.
- **SCIP timings:**
  - The cold pass over all 11 SCIP targets took 3,199 s (53 min): 0.9–1,530 s per target, with nestjs alone taking 25.5 min to fail its fetch.
  - A warm rerun takes seconds per target (mattermost 20 s).
