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

---

# M8b findings (independent keys: microSecEnD, blind hand keys, the `exact` sample)

**Scoreboard:** `bench/scoreboard/m8b.json`, the same M7 close binary (sha256 `a038653b…`). The 260 M8a cells are unchanged; 54 cells and the `exact` sample are new.

## B7. Routes registered in test code become `exact` endpoints (major, `exact` precision)

- **hk_todo** (blind key, adjudicated): rb exposes `GET /identity` and `POST /sign-up` as `exact` endpoints of the application. Both are registered only in test files (`pkg/handler/middleware_test.go:92`, `pkg/handler/auth_test.go:80`), and the app's router has neither.
- These are 2 of the 8 wrong `exact` edges among the 130 that the independent keys judge (the other 6 are the missing context paths, B14).
- **Fix direction:** keep routes registered in test files out of `EXPOSES`, or label them, as rb's own `is_test_path` already does for orphans and DI.
- **M9a outcome (fixed, contract-m9a D1):** the endpoint stage reads no file that `is_test_path` matches, and each test file with route facts gets one `routes_in_test_code` diagnostic. hk_todo endpoints `fp exact 2 → 0`; handkey endpoints precision 95.1 % → 100.0 %. The test route's handler binding left too (`handler_binding` `found_out_of_scope{test} 1 → 0`). The invariant has 22 C1 rows: hk_todo 8, mattermost 7 per mode.

## B8. Routes read from API-spec annotations override the code (major, `exact` precision)

The `exact` sample's `routes` family is 58 of 60. Both wrong edges are in go_gin_example, where a Swagger annotation contradicts the router:
- **`GET /auth`:** the code registers `r.POST("/auth", api.GetAuth)` (`routers/router.go:30`). GET comes only from `@Router /auth [get]`.
- **`POST /api/v1/tags/import`:** the code registers `/tags/import` on the root engine (`router.go:48`), outside the `/api/v1` group.

A spec route that the code also registers should take the code's method and path, and a spec-only route should not be `exact`.

**M9a outcome (fixed, contract-m9a D2):** the cause was generated OpenAPI files (`docs/swagger.*`), not `@Router` comments, which rb does not read. An OpenAPI operation's own registration is now `synthetic`, and a route the code registers stays `exact`. go_gin_example's 3 spec-only `EXPOSES` moved `exact` → `synthetic` in both modes (invariant: 6 C2 rows). After the redraw the `routes` family is 60 of 60.

## B9. Gin handlers given as method values are not bound (major, Tier B recall)

- **hk_todo:** `handler_binding` is 0 of 13, although 13 of 13 endpoints are found. 12 of the 13 routes use a method value (`auth.POST("/sign-up", h.signUp)`); the 13th (`GET /swagger/*any`) is a library handler (`handler.go:24`), bound at its registration line.
- rb's own diagnostics say so: `route_handler_unresolved` 14.
- With method-value handlers resolved, `rb path` and `callers` of a handler would work on this stack.

## B10. SQL with table names from `fmt.Sprintf` constants is not read (major, Tier B recall)

- **hk_todo:** `table_queries` is 0 of 26 and `table_ops` 0 of 26.
- Every statement builds its table name as `fmt.Sprintf("SELECT … FROM %s …", todoListsTable)`, with the names from constants in `pkg/repository/postgres.go:9-13`.
- The constants are literal strings in the same package, so they can be resolved statically.

## B11. A repository type "queries" its table through inherited CRUD, labelled `synthetic` (convention, documented)

- **The edges:** for every Spring Data repository interface, rb emits a `synthetic` `QUERIES` from the interface type itself to its entity's table with all four ops, standing for the inherited `save`/`findById`/`delete`. The blind format (`bench/keys/FORMAT.md`, written before any rb output was seen) counts only queries a function states. The adjudicator, blind to which side said what, judged all of these FALSE under it.
- **Size:** 8 `table_queries` false positives across spring_boot_microservices and hk_polls (16 of 24 `synthetic` edges right; `table_ops` rows restate the same 8 edges once per op and are excluded from the certainty table, amendment 2). These are all of the wrong `synthetic` edges: 16 of 16 outside this convention.
- **Not a key change:** the format will not be loosened after seeing rb's output (contract M8b T4). The choice for rb is to document the convention in `rb table-deps` output, or to attribute inherited CRUD to its callers.
- **M9a outcome (documented, unchanged, contract-m9a D5):** README, the usage guide and the design doc state that a repository type's own `QUERIES` row stands for the CRUD it inherits. No output changed, so the 8 `synthetic` fp stay: `synthetic` is 22 of 30 right in M9a (16 of 24 in M8b; the 6 added are piggymetrics' context-path endpoints, all right). Attributing inherited CRUD to callers stays open.

## B12. microSecEnD: endpoints and service flows (accuracy, independent key)

- **Coverage:** 12 of 17 applications pass D1's evidence check. In every one, each internal service maps to its own rb service.
- **Endpoints:** precision 88.7 %, recall 75.8 % (47 of 62 after amendment 2's 23 removals — 6 context-path forms and 17 key errors, with 2 further artifacts kept in gold as fn; `bench/keys/microsecend/*.md`). Of the 28 first-scoring misses, 19 were model artifacts (17 removed, 2 kept in gold as fn):
  - bare class prefixes listed as endpoints: `/api` ×3 (fernando), `/catalog`, `/movies`, `/ratingsdata` (koushik), `/recipients` (piggymetrics), `/comments` ×2 (anilallewar, rohitghatol), `/recommendation` (mdeket);
  - Feign-client paths credited to the wrong service: 6 on `card-statement-composite` (`CardService.java:18-24`, `StatementService.java:17-23`), `/newuser` on user-service (mdeket).
- **Real rb misses (15 fn):** 6 true prefixed piggymetrics paths rb never emits (B14: `/accounts`, `/accounts/current`, `/statistics/current`, `/notifications/recipients/current`, plus the listed `/accounts/{}` and `/statistics/{}`); 4 no-leading-slash joins — `value = "{taskId}"` / `"{userName}"` under class `/` (anilallewar `TaskController.java:48`, `UserController.java:51`; the rohitghatol fork too); 5 Spring Data REST endpoints (`@RepositoryRestResource`), mse_jferrater 0 of 5. On genuinely served endpoints rb is 7/7 (fernando), 4/4 (koushik), 0/6 (piggymetrics, all context-path), 12/12 (mdeket), 4/6 (anilallewar, rohitghatol), 0/5 (jferrater).
- **Service flows:** 4 of 14 in-scope flows found, 0 false. 271 of the 285 model flows are out of scope: through gateways, registries or config servers (228), external (30), databases (9), messaging (4).
- **A miss form read in the source** (mse_callista): RestTemplate URLs built at runtime from `LoadBalancerClient` service discovery, so rb sees a dynamic URL.
- **M9a outcome (part a, the literal-brace paths; fixed, contract-m9a C6):** a Java string literal is a path whatever its first character, so `value = "{taskId}"` under class `/` gives `GET /{}`. mse_anilallewar and mse_rohitghatol endpoints `tp exact 4 → 6`, `fn 2 → 0` each (invariant: 10 C6 rows each). With B14, microSecEnD endpoints are P 100.0 %, R 91.9 % (57 of 62; M8b P 88.7 %, R 75.8 %). The 5 fn left are the Spring Data REST endpoints (part b, M9b). Service flows on both apps are unchanged.

## B14. Context paths from Spring Cloud Config shared files are not applied (major, `exact` precision)

- **piggymetrics** serves account-service under `/accounts`, statistics-service under `/statistics` and notification-service under `/notifications`, set as `server.servlet.context-path` in the config server's shared files (`config/src/main/resources/shared/account-service.yml:21`, `statistics-service.yml:21`, `notification-service.yml:12`).
- rb exposes the controller paths without the prefix (`GET /{}`, `POST /`, `GET /current`, `PUT /current` on account; `GET /current`, `GET /{}` on statistics; `/recipients/current` on notification), all `exact`. The model lists the same unprefixed forms, so 6 tp were spurious; the override file removes them (now 6 fp) and adds the true prefixed paths (now fn). The dataset authors knew about context paths elsewhere (auth-service's `/uaa/users` is prefixed in the model), so this is an rb gap, not a dataset convention.
- **Fix direction:** read `server.servlet.context-path` (including config-server shared/default files) into the endpoint's effective path, as context paths from local config already are.
- **M9a outcome (fixed, contract-m9a D4):** a `native` config server's shared files set the context paths of its clients, matched by `spring.application.name`, and the prefixed path is `synthetic`. piggymetrics endpoints `tp synthetic 0 → 6`, `fp exact 6 → 0`, `fn 6 → 0` (invariant: 78 C5 rows). Service flows `tp heuristic 0 → 2`, `fn 2 → 0`: the Feign calls to `/statistics/{}` and `/accounts/{}` now match. A third Feign flow, into `POST /uaa/users`, is correct but lands out of scope because the model marks auth-service infrastructural (`found_out_of_scope{not_in_model} 0 → 1`, accepted in `bench/accept-m9a.tsv`).
- **M9a evaluator pass 1 (E3, fixed, `PIPELINE` `m9a-2`):** an empty remote `context-path` now overrides a lower local or remote one and leaves the route at the root, as Spring serves it. The bench has no such case: the r2 dumps are identical by row to the first M9a run, and no number above moves. Inputs D4 does not read are residue in the contract (E6): `spring.cloud.config.name`, `optional:`/`classpath*:` locations, and a value without a leading `/`.

## B13. SCIP `IMPLEMENTS` from a method (minor, `exact` precision)

- The `exact` sample's `scip` family is 57 of 60, and all 3 wrong edges are mattermost `IMPLEMENTS`.
- **Two start at a method** (`SqlChannelMemberHistoryStore#GetMembershipChanges()` → `store/ChannelMemberHistoryStore#`), where the relation belongs to its type or to the interface's method: the SCIP relationship joined onto the wrong symbol.
- **One is plainly wrong:** `ElasticsearchInterfaceTestSuite#` → `IndexerJobInterface#`, a suite without `MakeWorker`.
- **M9a outcome (fixed for method sources, contract-m9a D3):** under the Go bypass only a type source makes an edge. A method's link to an interface method is dropped and counted in `relationships_dropped`. 1,732 method → interface `IMPLEMENTS` left the bench (mattermost 1,704, go_clean_arch 25, microservices_demo 3), and per target the removed edges equal the `relationships_dropped` delta. scip-go's type relationship already gave the type-level edge for 340 of the 342 (holder, interface) pairs; the other 2 are partial implementations. After the redraw the `scip` family is 60 of 60. The third edge has a separate cause (B13b).

## B13b. scip-go relates test-suite types to interfaces they do not implement (minor, `exact` precision; deferred to M9b)

- **The edges:** on mattermost, 277 `scip` `IMPLEMENTS` start at 5 `*Suite#` types, 38 of them to `AppIface#`. 264 (36 to `AppIface#`) come from the 4 types declared in `_test.go` files: `MmctlUnitTestSuite#` 136, `MmctlE2ETestSuite#` 84, `OpensearchInterfaceTestSuite#` 27, `ElasticsearchInterfaceTestSuite#` 17. `CommonTestSuite#` (13, 2 to `AppIface#`) is declared in `server/enterprise/elasticsearch/common/test_suite.go`, which `is_test_path` does not match. The count is the same in the M8 and M9a dumps. The M9a contract quoted 264 from 5 types; the recount above separates the two.
- **Example:** `ElasticsearchInterfaceTestSuite# → IndexerJobInterface#`, judged FALSE in M8b: the suite (`elasticsearch_test.go:33-40`) has no `MakeWorker`.
- **Cause:** the source is already a type. The relationship comes from scip-go's own symbol information on the type, which the M6d S9 bypass accepts without a walk. M9a's guard on method sources does not touch it.
- **Next (M9b):** read the raw index before choosing a fix. None of these edges was drawn in M9a's sample.

## Data quality of the keys (recorded, not rb findings)

- **microSecEnD:**
  - 5 of 17 applications fail the evidence check after pinning (80–87 % resolved; real line drift).
  - Two traceability files have trailing commas.
  - 99 evidence items (79 in kept applications) name config-repository files under the application's own URL; they exist in no commit.
  - The model writes `account_service` where its trace keys `account-service`.
  - Some models list no `Endpoints` at all (mse_piomin), so their rb endpoints are `out_of_scope{no_endpoint_list}`.
- **The `exact` sample's one wrong `sql` edge is borderline:** a materialized view recorded as a table, with the name and ops right.
- **The judges' conventions,** stated in `bench/samples/exact.tsv` evidence:
  - `CREATE INDEX` counts as `alter`;
  - an empty `COPY … FROM stdin` counts as `insert`;
  - a pointer-receiver method set counts for `IMPLEMENTS`.
- **The hand-key adjudications changed nothing.** All 133 verdicts agreed with the labellers. The labeller and the adjudicator are one model family, so an error they share would pass both.
