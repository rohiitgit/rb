VERDICT: PASS (0 blocking; 4 should-fix, 4 notes)

# M9a close: evaluator pass 1 (2026-10-16)

Scope: `tasks/contract-m9a.md` (D1–D6, C1–C12, amendments 1–2, Result), `tasks/plan-m9.md`, `tasks/bench-findings.md` B7/B8/B11/B12a/B13/B13b/B14, the diff `0453fda..HEAD -- crates bench/mutants`, the evidence in `var/m9a/`, `bench/scoreboard/m9a{,-perf}.json`, `bench/accept-m9a.tsv`, `bench/samples/exact.tsv`, `var/bench/labels/exact-m9a/`, and the run outputs `var/bench/out/05a600c9ff2d/` against `a038653b8478/`. Method: tools at HEAD `55fbed0` (`target/debug/{rb,rb-bench}` rebuilt), `python3 -I` over the dumps, two fixture repositories indexed into `:55433` as `t_eval_m9a_b14` and `t_eval_m9a_rt` (deleted afterwards), and 13 mutants of my own. Scratch: `var/m9a/eval/`.

**What reproduces:**
- `invariant.py` re-run is byte-identical to `var/m9a/logs/invariant.txt`: 157 dumps, C1 22, C2 6, C3 1,740, C5 78, C6 20, 0 unclassified.
- Re-scoring both sides with HEAD's `rb-bench` is `cmp`-identical to `bench/scoreboard/m9a.json`, `m9a-perf.json`, `var/m9a/m8b-rescored.json` and its perf file. The re-scored base differs from `m8b.json` only in `keys["exact:verdicts"]`. With M8b's verdict file (`git show 0453fda:bench/samples/exact.tsv`) the restored base re-scores byte-identical to `m8b.json` and `m8b-perf.json`.
- `compare` re-run is identical to `var/m9a/logs/compare.txt`, exit 0. 8 of 314 cells changed. Against the committed `m8b.json` it exits 2, as D6 predicts.
- `rb-bench sample` over the M9a dumps, minus M8b's verdicts, leaves exactly 80 edges (routes 25, scip 55). That set is exactly the 80 appended verdict lines (81 lines with the comment; no existing line was edited).
- Both mutant batteries re-run fresh (explicit ids, no cache): m230–m247 18/18 KILLED, m8b 29/29 KILLED.
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean. The diff has no TODO, `todo!`, `dbg!` or `println!`.
- No new `exact` is wrong: see "Checked and held".

## Findings

**E1. Should-fix: 11 of my 12 mutants on the new rules survive their named test, and 3 survive the whole `rb-indexer` lib.**
- Repro: `python3 -I bench/mutants/run.py var/m9a/eval/mutants-eval.json e01 e02 e03 e04 e05 e06 e07 e08 e09 e10 e11 e12 --jobs 4` (log `var/m9a/eval/mutants-eval.log`).
  - Only e01 is KILLED (the gin call-site scan reads test files).
  - These survive `config_server_shared_files_set_client_context_paths` or `routes_in_test_files_are_not_endpoints`:
    - e02: `.properties` no longer beats YAML in one location.
    - e03: the empty-context-path rule is removed.
    - e04: the server is not excluded from the clients.
    - e05: an earlier location beats a later one.
    - e06: locations are not trimmed.
    - e07: a trailing `/` is kept (`classpath:/shared/`).
    - e08: remote YAML reads profile-gated documents.
    - e09: remote `.properties` reads profile-gated documents.
    - e10: `searchLocations` (camelCase) is not read.
    - e11: `routes_in_test_code` always reports `{"routes": 1}`.
    - e12: a config server at the repository root reads the wrong `resources` path.
  - Without the filter (`var/m9a/eval/mutants-eval-wide.json`), e10w, e11w and e12w still survive the whole lib. For e02w and e05w see E2: they are SURVIVED on a clean re-run.
- Expected: every D4 rule and every amendment 1 choice has a test that fails when it is removed ("a surviving mutant is a test bug until proven otherwise", usage §5.4). Actual: amendment 1's three choices (`.properties` over YAML, later location wins, empty counts as none) and D4's "unconditional documents only" and "other than the server" have no test.
- Current behaviour is right for e02, e05, e06, e07 and e08: my fixture (`var/m9a/eval/fx/b14`) gives `/beta-props`, `/gamma-common`, `/alpha` (profile document excluded), and reads `classpath:/shared/ ,  classpath:common`. So the gap is regression protection, not wrong output today. e03 is the exception: see E3.
- Breaks: C5 ("each row asserts…") covers only the cases it lists. Amendment 1 adds rules with no row and no mutant.
- Fix: add the cases to `config_server_shared_files_set_client_context_paths`, and add e02–e12 (or equivalents) to `bench/mutants/m9a.json`.

**E2. Should-fix: under `--jobs`, the runner reports KILLED for mutants that do not change any test result.**
- Repro:
  - The first wide run (`var/m9a/eval/mutants-eval-wide.log`) reported e02w–e09w KILLED: the first two specs of each worktree group.
  - Re-run `python3 -I bench/mutants/run.py var/m9a/eval/mutants-eval-wide.json e02w e05w --jobs 2` gives both SURVIVED.
  - `cargo test -p rb-indexer --lib` with e02 applied passes, 357/357.
  - Running the unmutated `rb_indexer` test binary 4× at once fails `resolve::tests::field_chains_binding_chains_and_big_packages_stay_fast` (`resolve.rs:2125`) in all 4 runs.
- Expected (P0, usage §5.4): "KILLED needs a failing test" means a test the mutation fails. Actual: when parallel worktrees load the machine, a wall-clock test panics, `panicked at` matches, and an unfiltered spec on `rb-indexer` is KILLED spuriously. Usage §5.4 recommends `--jobs 4`.
- Impact on this close: none found. Every m230–m247 spec has a `test` filter that excludes the timing test, `rb-bench` has no timing test (`grep Instant::now crates/rb-bench/src` is empty), and both batteries re-run clean.
- Fix: exclude timing tests from mutant runs (`--skip stay_fast`, or mark them `#[ignore]` and run them separately), or count KILLED only when a failing test name is outside a known-flaky list. Record it in lessons.

**E3. Should-fix: a remote empty context path does not override a lower one, against amendment 1's "as for local files" and against Spring.**
- Repro: fixture `var/m9a/eval/fx/b14`, service `svc-e`.
  - Local `application.properties` has `server.servlet.context-path=/local-eps`.
  - The shared `epsilon.yml` has `context-path: ""`.
  - Indexed as `t_eval_m9a_b14`.
- Expected: Spring Cloud Config's remote sources override local ones, so the effective value is `""` (root) and the route is `GET /x`. The local rule takes the highest-precedence value first and then treats empty as none (`lib.rs:1177-1183`). Actual: `EXPOSES svc-e -> HTTP GET /local-eps/x [synthetic]`, with no `config_server_context_path` diagnostic. The loop at `lib.rs:906-909` skips empty values, so a lower non-empty value survives. The same happens when remote `<name>.yml` sets `""` over a remote `application.yml` that sets a path.
- It is `synthetic` and rare (0 on the bench), so it is not an `exact` error. But amendment 1 states an equivalence the code does not have, and mutant e03 survives exactly here.
- Breaks: amendment 1 ("An empty `server.servlet.context-path` counts as none, as for local files"), and D4's precedence.
- Fix: take the highest-precedence value (empty included), then treat empty as none. Add the case to the test.

**E4. Should-fix: "18 of their citations were spot-checked against the source" cannot be traced.**
- Repro: `grep -ril spot var/m9a var/bench/labels/exact-m9a` finds nothing. Only `tasks/contract-m9a.md` and `tasks/todo.md` mention it.
- Expected: per M8b pass 1 finding 5 (and its lesson), the audit command and its output are recorded. Actual: there is no list of the 18 edges, no commands and no output.
- I audited 7 citations myself, and all match the pinned source: `hclog_adapter.go:29`, `api4/post.go:307`, `app/authorization.go:106`, `sqlx_wrapper_test.go:118`, `mmctl/commands/token.go:89`, `frontend/handlers.go:462`, `ldap_sync_removal_equals.spec.ts:54`. Also the go_gin `router.go` lines and `webhook_serve.js:21-55` in the routes transcript.
- These amendment 2 figures also have no log: "free space fell to 2.4 GiB" and "`cargo clean` freed 48.0 GiB".
- Breaks: Result ("18 of their citations were spot-checked"); the records rule "every number traceable".
- Fix: keep the spot-check list and commands under `var/bench/labels/exact-m9a/`, or reword the claim. Note the disk figures as unlogged.

**E5. Note: D1's test rule drops production endpoints that `is_test_path` matches by name.**
- Repro: fixture `var/m9a/eval/fx/rt` (`t_eval_m9a_rt`). Each of these gets only a `routes_in_test_code` diagnostic and no endpoint:
  - `jsvc/src/main/java/com/ex/test/DiagController.java` (a production package named `test` under `src/main`);
  - `jsvc/src/main/java/com/ex/AbTest.java` (a production controller);
  - `web/src/routes/tests/index.ts` (a router that production mounts at `/lab`, so `GET /lab/results` is lost);
  - `web/src/helpers.spec.ts` (a router that production mounts at `/spec`).
- The other side is also open: `gosvc/testdata/fixture.go` (Go never compiles `testdata/`) is still an `exact` endpoint `GET /from-testdata`, under its own service `gosvc/testdata`.
- This is D1 as written, it is counted, and the bench has none of it outside mattermost's `e2e-tests` (64 `exact` endpoints, kept on purpose, plan correction 1). It loses recall, not precision.
- Suggest: for routes, let `src/main/` override directory names, add `testdata/`, and record it as residue.

**E6. Note: unhandled B14 inputs, all outside D4's letter.**
- A client's `spring.cloud.config.name`, which Spring uses before `spring.application.name`, is not read.
- `context-path: delta` (no leading `/`) gives `/delta/x`; Spring refuses this value at startup.
- `optional:classpath:` and `classpath*:` locations are not read.
- The order "every `application.*` < any `<name>.*`, whatever the location" is the contract's; Spring Boot orders by location first. It differs only with several locations that each hold both files.

**E7. Note: the "possible" `service_flows` predictions were loose, and the Result says so honestly.**
- The anilallewar and rohitghatol flows did not move, and piggymetrics gained 2 tp plus 1 accepted out-of-scope flow.
- The accept line is correct against the source:
  - `AuthServiceClient.java:12`: `POST /uaa/users`.
  - `auth-service.yml:12`: `context-path: /uaa`.
  - `UserController.java:16,28`: `/users`, POST.
  - The dump: `CONSUMES … AuthServiceClient#createUser() → HTTP POST /uaa/users @auth-service`, `http_stitch`, heuristic.

**E8. Note: `perf_ab.sh`/`perf_ab2.sh` print `print:33: bad option: -`.**
- The timing rows are unaffected. My recomputed means match amendment 2: anilallewar 0.47/0.44, piggymetrics 0.55/0.59, fernandoabcampos 0.22/0.20, shabbirdwd53 0.36/0.33.
- The bench run's uniform +0.3 s on small targets does not reproduce A/B, which supports amendment 2's "not persisted".

## Checked and held

Pass 2 can verify these without redoing them.

- **B12a (C6).** Fixture `TaskController` under class `/tasks`:
  - `value = {"{a}", "b"}` gives `GET /tasks/{}` and `GET /tasks/b`.
  - `path = "{x}"` gives `PUT /tasks/{}`.
  - `value = "{id}/items", produces=…` gives `DELETE /tasks/{}/items`.
  - `produces=…, value = "{pid}"` gives `PATCH /tasks/{}`.
  - A constant `Paths.X = "{y}"` stays `route_path_dynamic`, as before.
  - The unit test pins `@PutMapping({})` → class path, and a 2-element array → 2 paths.
- **B8 (C2).**
  - Spec-only operations are `synthetic`: `GET /users` while code serves `POST /users`, `PUT /health` while code serves GET, `POST /v1/users`.
  - Joined operations stay `exact`: `POST /users`, `GET /users/{}`, and `GET /v1/users` through an interface mapping (`UserApi.java:3` + `openapi.json`).
  - The contract-packs golden moves only `EXPOSES . -> HTTP POST /users` (must-exist and dump).
  - A heuristic code route joined by a spec now becomes `synthetic` (the higher rank), never `exact`.
- **B7 (C1).**
  - Test files (`main_test.go`, `src/test/java`, `__tests__`, `.spec.ts`) give no endpoint and one diagnostic each, with the right count (`__tests__/fake.ts` `{"routes": 3}`).
  - `e2e/` stays, by design.
  - On the bench, outside `e2e-tests`, no `exact` `EXPOSES` has all its evidence in `testdata`/`fixtures`/`mocks`/`testing` directories.
  - The `param_compositions` filter is covered (my e01 KILLED).
- **B13 (C3, C4).**
  - No `scip` `IMPLEMENTS` from a method to a type is left in any M9a dump. Base: go_clean_arch 25, microservices_demo 3, mattermost 1,704; M9a: 0. Type-to-type and method-to-method counts are unchanged per target.
  - Every dropped (holder, interface) pair has its type edge in M9a, except the 2 partial pairs the contract names (`baseSetting#→Setting#`, `blockBase#→Block#`). go_clean_arch 5/5 and microservices_demo 3/3.
  - B13b recount: 277 from 5 `*Suite#` types, 38 to `AppIface#` (136/84/27/17 + `CommonTestSuite#` 13), identical in both dumps.
- **B14 (C5).** Fixture `cfg` has `native` in its base document and `search-locations: classpath:/shared/ ,  classpath:common` in a `spring.profiles: native` document. It applies:
  - `/alpha` (multi-document; the `docker` document is ignored);
  - `/beta-props` (`.properties` over `.yml`; client through `spring.config.import=configserver:`);
  - `/gamma-common` (later location);
  - `/eta` (`.yaml` file; name from `application.yml`);
  - the same name on two services (`svc-a`, `svc-z`): both `/alpha`.
  - All of these are `synthetic`, each with one `config_server_context_path` diagnostic, and the server's own route stays `exact /x`.
  - piggymetrics: the probe dump has 11 `EXPOSES`, all `synthetic`, under `/accounts|/statistics|/notifications|/uaa`.
- **No false `exact` was created.**
  - The invariant's C6 bucket is the only bucket that adds `exact` rows: 20 rows, each checked by the script against a source line holding `"{`.
  - C2 rows go `exact → synthetic` only. C5 rows go `exact → synthetic` with the same evidence. C1 and C3 only remove rows.
  - Certainty: `exact` 100.0 % of 126, `synthetic` 73.3 % of 30. Re-rendered output equals `certainty-m9a.txt` and `certainty-m8b.txt`.
- **Judging (D6).**
  - The judge prompt says "ASSUME EACH EDGE IS FALSE" and restricts reads to `var/bench/repos/<target>` and the drawn list.
  - Tool calls in the 4 transcripts (8, 20, 11, 12) touch nothing else under the project.
  - The edges shown are rb's claims, which is inherent in judging an edge. The judges read the source lines themselves.
  - 80/80 TRUE; 17 of the 25 routes edges are mattermost cypress webhook routes (`webhook_serve.js`, a standalone server).
- **Base restoration (amendment 2).**
  - The regenerated mattermost runs report the M8 counts: `noscip` 58,506 / 169,043 / 8,350 and `scip` 58,445 / 161,561 / 5,788, matching `a038653b8478/mattermost.*.out`.
  - The `scip.status` body is identical.
  - The re-score with M8b verdicts is byte-identical to `m8b.json`. Only the two `.jsonl` files carry the 04:01 timestamp.
- **Records.**
  - These match their logs: invariant buckets; 8/314 cells; aggregates (handkey P 95.1→100.0, microSecEnD P 88.7→100.0 and R 75.8→91.9, service_flows R 28.6→42.9); certainty and sample tables; mattermost perf 24.73→21.49 s, 1,801,732,096→1,869,234,176 B, scip 2,379,431,936 B; scoring 1.44 s; real-Docker 1,129 passed / 0 failed / 0 `skipping:` (`test.log`, which holds the new tests); fmt/clippy CLEAN; perf A/B means; B13b counts; the warm run 04:03:02–04:05:47.
  - `crates/` and `bench/mutants` are unchanged between `d299183`, `7a82313` (the binary of record) and HEAD.
  - The runner reports a type-error spec as INVALID (`var/m9a/eval/mutants-invalid.json`).
  - README, the usage guide and the design doc describe what the code does (B7, B8, B11, B13, B14).
- **Hygiene.**
  - `t_eval_m9a_b14` and `t_eval_m9a_rt` were deleted with their AGE graphs. `SELECT name FROM rb.repo` shows `alvorabackend, fx_date, fx_full, fx_man`.
  - My scratch worktree is removed. `git status` shows only the pre-existing `bench/__pycache__/`.
