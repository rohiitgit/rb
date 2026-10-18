VERDICT: FAIL (1 blocking; 5 should-fix, 6 notes)

# M9d close: evaluator pass 1 (2026-10-18)

## Scope

- **Contract:** `tasks/contract-m9d.md`: D1–D15 (with D5b and D12b), C1–C18 and C5b, amendments 1–3, notes P0–P9c, the Result and Out of scope.
- **Plan:** `tasks/plan-m9.md`, the M9d section.
- **Code:** `git diff 8e91884..e7d8488 -- crates docker bench/mutants bench/probe migrations`, commits `c2ca543` to `e7d8488`. `crates/` is identical between `559d41d` and `e7d8488`.
- **Docker:** `docker/scip-java/Dockerfile.jdk25` and three images:
  - `rb-scip-java:0.13.1-jdk25` (`sha256:9e139634…`);
  - `rb-scip-java:0.13.1` (`sha256:f9612a94…`, unchanged);
  - `rb-scip-go:0.2.7-xtools0.50.0` (`sha256:47eab865…`).
- **Binaries:** the binary of record `var/m9d/bin/rb` (`5d994f76699b…`), `rb.p9a` and `rb.p9b`, and M9c's `var/m9c/bin/rb` (`6cd5baaa8d1f…`).
- **Runs:** the run of record `var/bench/out/5d994f76699b/`, against M9c's record `var/bench/out/6cd5baaa8d1f/`.
- **Bench files:**
  - `bench/scoreboard/m9d{,-perf}.json`;
  - `bench/accept-m9d.tsv`;
  - the `# M9d` block of `bench/samples/exact.tsv`;
  - `bench/mutants/m9d.json`.
- **Logs and records:** `var/m9d/logs/`; CHANGELOG, IMPLEMENT, todo, bench-findings, lessons, README, the usage guide and the design doc.

## Method

- **Tools:**
  - `var/m9d/bin/rb` and `var/m9c/bin/rb`;
  - `var/m9d/bin/rb-bench` (`82839c81…`);
  - `python3 -I` scripts, all in `var/m9d/eval1/scripts/`;
  - a SCIP decoder written for this pass, `var/m9d/eval1/scripts/scipdec2.py`.
- **Scratch:** everything is in `var/m9d/eval1/`, with logs in `var/m9d/eval1/{logs,inv,perf,mut,score}/`.
- **Probe repositories** in `:55433`, all named `t_eval1_m9d_*`: 9 fixture repositories (`var/m9d/eval1/fx/`), 3 runs of one URL-dependency fixture, the check probe `t_eval1_m9d_chk`, and the perf A/B repositories `t_eval1_m9d_ab_*`. All were deleted with their graphs and caches (`chmod -R u+w` first), then `rb gc --prefix t_eval1_m9d_ --egress` ran.
- **Indexing:** every fixture was indexed in real SCIP mode, once cold and once with `--force` to replay the cache, then read through `rb status` and `rb dump`.
- **Mutants:** I wrote 13 of my own (`var/m9d/eval1/mut/eval-m9d.json`, `--jobs 1`). `crates/` hashed the same before and after (`crates-sha-{before,after}.txt`).
- **Builds:**
  - one e2e build: `cargo test -p rb-cli --test e2e`, two tests;
  - one release rebuild of `rb` and `rb-bench` at HEAD, for C13.
- **Records:** a read-only sub-agent traced every number in the Result to its cited log. I confirmed each discrepancy I report below myself.

## What reproduces

- **Scoring:**
  - `rb-bench score` of `5d994f76699b` twice (`--rb-commit 559d41d…`) gives `cmp`-identical output, and both outputs are `cmp`-identical to the committed `m9d.json` and `m9d-perf.json`.
  - Re-scoring M9c's record is `cmp`-identical to `var/m9d/m9c-rescored{,-perf}.json`. That file differs from `bench/scoreboard/m9c.json` in exactly one leaf, `keys["exact:verdicts"]`.
- **Gate:**
  - The quoted `compare … --accept bench/accept-m9d.tsv --perf …` exits 0 with 46 `changed`, 12 `accepted` and 0 `REGRESSION` lines. Its output is line-identical to `logs/p9/compare.txt` except for the header.
  - Without `--accept` it exits 1 with 12 REGRESSION lines (`eval1/score/compare{,-noacc}.txt`).
  - I recounted 12 of 314 cells changed, the same 12 the record lists. Two of them changed only `covered`.
- **C13:** `cargo build --release -p rb-cli -p rb-bench` at HEAD reproduces `rb` `5d994f76699be1cc…` and `rb-bench` `82839c81…` bit for bit (`eval1/logs/rebuild.txt`).
- **Invariant C14:**
  - `var/m9d/p9/invariant.py` re-run on the run of record gives the same output as `logs/invariant.txt` except for the order of two listed rows: buckets NEWIDX 8,717, WORK 23,855, FALLBACK 4, RECIPE 6, NOSCIP-D2 5, 0 unclassified (`eval1/inv/invariant-record.txt`).
  - Because NEWIDX accepts any row in a new job's documents, I checked whether it hides losses. I projected every changed edge onto (kind, provenance, end names, evidence) (`eval1/inv/rekey.txt`). Every non-call row in NEWIDX is a pure re-key: orm, di, http_stitch and HANDLED_BY rows net 0. The net changes are Tier B `heuristic` CALLS, REFERENCES and IMPLEMENTS replaced by `scip` ones in covered files, plus `scip` DEPENDS_ON added.
  - mattermost's lost `heuristic` edges all lie under `tools/sharedchannel-test` (132 CALLS, 61 REFERENCES).
  - **NOSCIP-D2:** the 5 rows are exactly the `skipped` rows of `.` in microblog and the four `src/*` Python roots in microservices_demo.
  - **RECIPE:** these rows differ only in `stderr_tail`: kotlin's `[INFO] Exit code: OK` prefix, and the webapp and cypress tails moving to `ssh://git@registry.npmjs.org/…`.
  - **FALLBACK:** these rows are `ok`/`fetch`, with fastapi's pyyaml `ChefBuildError`, loadgenerator's psutil wheel failure and shoppingassistantservice's metadata failure in their tails. All three were read in the dumps and match the checkouts' pins (for example `psutil==7.1.0`).
- **D8, the 23,180 `synthetic` → `exact` edges.** I decoded the raw indexes in `var/m9d/p9/idx/` and M9c's server index (`var/m9c/p7/idx/ad9e1f9a….scip`) myself (`eval1/inv/d8check.txt`).
  - At every evidence span, the new server index holds the target symbol byte-equal: 23,180 of 23,180.
  - The old index holds no byte-equal symbol at those spans, but holds the `v0.4.3` variant: 23,180 of 23,180.
  - server/public's index defines every target: 23,180 of 23,180 (21,417 definitions).
  - **D10 counts from my decoder:** 0 of 202,391 locals typed `invalid type` in the new server index, and 1,541 of 202,391 in M9c's.
  - **15 random edges (seed 9124) are 15 of 15 TRUE in the checkout** (`eval1/inv/d8-sample.txt`). They include `model.NewAppError(…).Wrap(err)`, `model.NewId()`, `mlog.LoggerIFace`, `model.GetMillis()`, `mlog.Info`, `request.CTX`, `model.IsValidId` and `model.StringArray{…}`.
- **D9.** The removed rows are exactly the `…server/public@v0.4.3` `Package` node and its 30 `scip` DEPENDS_ON: 27 under `server/channels`, 2 under `server/cmd` and 1 under `server/scripts`. The 2 added DEPENDS_ON are sharedchannel-test's (`golang/go/src@go1.26.7`, `logr/v2@v2.0.22`). The `noscip` dump holds 0 edges into `server/public@*`.
- **New cells and their accept lines** (`eval1/inv/accepted20.txt`, `fp-new.txt`).
  - 20 random accepted pairs (seed 2026), read in source, are correct gold pairs or correctly described fp. Examples:
    - fastapi `queries.get_comments_for_article_by_slug` resolves to a `.pyi` stub;
    - `UsersRepository(conn)` and `RegistrationForm()` are joined to the class;
    - spring's `orderRepository.save` resolves to the repository interface;
    - adservice's `AdService.this.stop()`;
    - mattermost's `bot.IsSystemOwned()`, absent from `v0.4.3`.
  - I read all 74 new fp lines. spring's 17 are as the accept line says: 13 inherited Spring Data `save`/`findAll`/`existsBy…` calls resolved to the interface, and 4 `OrderEventService.save` overload candidates.
  - The explanations' errors are in S4.
- **D2, D4, D5, D5b, D6, D12 and D13, end to end** (fixtures in `var/m9d/eval1/fx/`, logs in `eval1/logs/<fx>.*`).
  - **`py_mix` (D2/D4):**
    - It holds a root `pyproject.toml`, a nested `docs/requirements.txt`, a `tools/requirements.txt` with no `.py`, a `svc` with `setup.py` and `requirements.txt`, a `legacy` with an uninstallable pin, and a `both` with `pyproject.toml` and `requirements.txt`.
    - It plans 6 jobs. `legacy` is `ok … (environment not installed: fetch failed)` with pip's `Could not find a version` in the tail. `.` is the same, with pip's editable-build error in the tail.
    - Nested roots do not overlap: the root's index holds `app/` only.
    - On the `--force` re-run, the cached rows replay with the suffix, and `rb status` prints it.
  - **`go_wsfail` (D5b fires):**
    - The root requires `github.com/google/uuid v1.6.0`, and an in-repo copy declares that path and holds a broken package.
    - The job is `scip-go ok unpinned 7.1s (workspace failed: indexed without go.work)`. Its row is `ok`/`index`, with the workspace run's `go list` error (`no required module provides package github.com/google/uuid/nope`) as the tail.
    - `rb index --force` replays it as `ok (cached) … (workspace failed: …)`, and `rb status` prints the suffix.
  - **`go_ws2/wsonly` (D5 → D8 → D9):** it gets `go.work`. The job is `ok`, and `scip_cross_project` to `wsonly/inner` reads `exact 1, synthetic 0`. No `Package` node is created, and the manifest's service → service DEPENDS_ON stays.
  - **`go_d6` (D6):**
    - `svc/sub`, with `replace => ../../lib`, is `ok` and joins `exact 1`.
    - `far` (`../../outside/lib`) and `abs` (`/opt/lib`) are `unsupported: replace target outside the repository`.
  - **`go_vend`:** the vendored `go.mod` is neither a job nor a workspace module.
  - **`mvn` (D12):** each pom is listed with the image `scip_status` records for it.

    | pom | Image |
    |---|---|
    | release 21 | `f9612a94…` (JDK 21) |
    | release 25 | `9e139634…` (JDK 25) |
    | release 26 | `unsupported: … the poms declare Java release 26; the newest scip-java image runs JDK 25` |
    | parent `java.version=25`, child `maven.compiler.release=${java.version}` | JDK 25 |
    | parent `java.version=17` with `release=${java.version}`, child overriding `java.version=25` | JDK 25, compiled at release 25 |

  - **`gradle_multi` (D13, a multi-project build):** adservice is in a subproject `ads`, and the cloned Gradle cache has both `protoc*` artifacts at 0644. The job is `scip-java ok`, with 21 of 21 definitions, and both artifacts end `-rwxr-xr-x`.
- **D11:**
  - **`rb check` A/B** (`eval1/logs/check-ab.txt`, P9's script with my repository name, 5 runs each, 4 busy loops, load 4.67–4.93): `unowned_table_writes` median 436.9 → **22.9 ms**, and all three rules 454.8 → 34.0 ms. Outputs are identical.
  - **EXPLAIN:** under M9c's session the plan carries `JIT` (`Total` 307.5 ms, execution 328.1 ms). Under M9d's it does not (execution 0.9 ms).
  - The binary of record holds the string `SET LOCAL jit = off`, and M9c's does not. `cypher_runs_without_jit` explains under the very `cypher_session()` helper `cypher()` calls (`db_tests.rs`).
- **D15:**
  - **Control:** a Go-shaped tree (directories 0555, `go.mod` 0444) planted as `var/scip/cache/t_eval1_m9d_leak` defeats a plain `shutil.rmtree` (EACCES).
  - `e2e_clean_ups_remove_read_only_test_caches` passes.
  - `topo_packs` (a DB test) runs the start-up clean-up, which printed `… 0 building rows and 1 package caches`. 0 `t_*` caches were left (`eval1/logs/d15-e2e.txt`).
- **Perf (ABBA, 5 runs per side, scip mode, M9c's binary vs `rb`, load 1.36–1.70; `eval1/perf/ab{,-summary}.txt`):**

  | Mode | wall | CPU | RSS | instructions |
  |---|---|---|---|---|
  | microservices_demo `scip` | 1.120 | 1.211 | 1.514 | 1.252 |
  | spring_boot `scip` | 1.225 | 1.455 | 1.188 | 2.439 |

  - These match the record's round (1.129 / 1.211 / 1.515 / 1.251 and 1.161 / 1.455 / 1.189 / 2.435).
  - Absolute costs are small: spring CPU 0.11 → 0.16 s.
  - Every run exited 0. The cached-job lines were 5 vs 9 (microservices_demo) and 1 vs 1 (spring).
- **Sample:**
  - **Blind and adversarial:** `prompt-{1,2,3}.txt` say "ASSUME EACH EDGE IS FALSE", limit reads to the drawn list and the pinned checkouts, and allow one written file.
  - **Row-for-row match:** each `verdicts-scip-N.tsv` matches its drawn list row for row: 55 of 55 with the identity verbatim, and each row's evidence cites its own edge's file:line.
  - **Transcripts:** they hold 0 tool arguments outside `var/bench/repos/` and the label directory.
  - **`exact.tsv`:** it gains 56 lines and loses 0 (`8e91884..e7d8488`). The 55 rows are exactly the verdict files' rows.
- **Mutant specs (spot check):** m368, m374, m380, m389 and m391 each mutate the line their name claims:
  - m368: `phase() == Some(Fetch)` admits a `Timeout{Fetch}`;
  - m374: `GOWORK=off` on the fetch only;
  - m380: the versions dropped before comparing;
  - m389: no `${…}` resolution;
  - m391: the `init` chain removed from the `gradle` closure, which only the fetches use.
- **Docker:**
  - `rb-scip-java:0.13.1` is still `f9612a94…`.
  - In the JDK 25 image, `scip-java` (`a694cae1…`) and `mvn` (`f9381d0c…`) have the same sha256 as in 0.13.1, and `java -version` reads 25.0.4.1.
- **Plan coverage.** The plan's M9d names 4 targets.
  - spring_boot, fastapi (as an environment-less index) and nestjs are delivered.
  - kotlin_spring_realworld is recorded with its measured reason. That is not silently dropped.

## Findings

**B1. Blocking: D1's `npm_config_replace_registry_host=always` breaks the `RB_SCIP_REGISTRIES` opt-in for every npm lockfile URL that is not a `registry.npmjs.org` package. The record discloses only the `git+ssh` case, and the docs still say opting a host in works.**
- **Repro.** The fixture is `var/m9d/eval1/fx/npm_url`: one dependency `"xlsx": "https://cdn.sheetjs.com/xlsx-0.20.3/xlsx-0.20.3.tgz"`, with a `package-lock.json` resolved inside the scip-typescript image (`resolved: https://cdn.sheetjs.com/…`). The script is `eval1/scripts/npmurl.sh`, and its log is `eval1/logs/npmurl.log`.

  | Binary | `RB_SCIP_REGISTRIES` | Result |
  |---|---|---|
  | M9c (`6cd5baaa`) | `cdn.sheetjs.com` | `scip-typescript ok 3.4s` |
  | M9c, `--no-cache` | unset | `failed (fetch phase)`, `403 Filtered - GET https://cdn.sheetjs.com/…`, `egress proxy refused` (`npmurl_c0.out`) |
  | **M9d (`5d994f76`)** | `cdn.sheetjs.com` | **`scip-typescript failed 1.9s (fetch phase)`: `404 Not Found - GET https://registry.npmjs.org/xlsx-0.20.3/xlsx-0.20.3.tgz`** |

  So a fetch that M9c completed once the host was opted in, M9d cannot complete at all. npm rewrote the CDN host to the default registry.
- **The bench shows the same for git hosts.** M9c's cypress tail ends `egress proxy refused: codeload.github.com:443, github.com:443`: npm tried HTTPS on GitHub first, which an opt-in would have allowed. M9d's tail has no refusal and goes straight to `ssh://git@registry.npmjs.org/…`. This answers the Out of scope's "whether npm still tries `codeload.github.com` first … is not measured": it no longer does.
- **Records that are now wrong:**
  - `docs/repository-brain-design.md:257` justifies rejecting the allow-list change with "`RB_SCIP_REGISTRIES` lets a host opt in".
  - `docs/usage-and-testing.md:241-243` says the git fetch "fails, as git hosts always did behind the allow-list". With an opt-in it did not fail before.
  - Out of scope (P8) and `tasks/todo.md` describe the effect for `git+ssh` dependencies only. URL dependencies, and packages of a private registry a user opted in, are rewritten the same way. The private-registry case follows by the same mechanism and was not measured here.
- **Breaks:** a working, documented feature (contract M5 S21's opt-in) regresses for every TypeScript job with a non-npmjs `resolved` URL. The contract's own trade-off for D1 rests on that feature.
- **What would settle it:** any one of these, with a unit row and this fixture re-run as its probe, plus the docs corrected:
  - narrow the rewrite to the lockfile's foreign registry hosts that are neither allowed nor opted in (the Out of scope's own suggestion);
  - drop `always` when `RB_SCIP_REGISTRIES` names a host the lockfile uses;
  - or record this as a known regression and have the user accept it explicitly.

**S1. Should-fix: D5's workspace fails, and D5b cannot recover, whenever the job's `go.mod` also requires a module through a local `replace` and the in-repo workspace module's required version is not published. The README states the feature without that limit.**
- **Repro** (fixture `var/m9d/eval1/fx/go_ws2`, `eval1/logs/go_ws2.*`):

  | Job | `go.mod` | Result |
  |---|---|---|
  | `wsonly` | requires `example.com/wsonly/inner v0.0.0` (in-repo, unpublished) | `ok`, `exact` join |
  | `wsrep` | the same, plus `require example.com/wsrep/rep` with `replace … => ./rep` | `failed (fetch phase)` |
  | `wsout` | the same, plus `replace example.com/shared2 => ../shared2` (D5 with D6) | `failed (fetch phase)`: `main.go:4:2: example.com/wsout/inner@v0.0.0: reading https://proxy.golang.org/…: 404` |

  `go_ws/svc` fails the same way.
- **Offline control** (docker, `--network none`, rb's `go.work` and env; scratch since removed):
  - `go list -deps -test ./... ./inner/...` exits 0 for `wsonly`. It also exits 0 for `wsonly` plus a replace of a module that is not required, and for `wsonly` plus a replace of the workspace module itself.
  - It exits 1 for `wsrep` and `wsout`, asking the proxy for `inner@v0.0.0`.
  - It exits 0 for `wsrep` once `go.work` adds `replace example.com/wsrep/inner v0.0.0 => ./inner`. A replace without the version is refused (`workspace module … is replaced at all versions`).
- **Why the bench does not show it:** mattermost's `server/public v0.4.3` is published, so the proxy answers.
- **D5b cannot recover:** the plain re-run needs the unpublished version too. For `wsrep` the stored tail is the re-run's 404, which loses the workspace attempt (N2).
- **Settle:**
  - write a versioned `replace <path> <required version> => ./<dir>` for each workspace module in rb's `go.work`, then measure mattermost's index again (`exact` count, dumps), and add a `wsrep`-shaped row to `workspace_jobs_write_go_work_and_clear_goflags`;
  - or narrow README's Go row and the usage guide to "when the required version is published or no required module is replaced", and add the case to Out of scope.

**S2. Should-fix: C16's two scip-mode cells pass only under amendment 3, which was written after their OVER verdict. Its condition (iii) exempts the very region holding the remaining avoidable per-symbol work. The records report C16 as "met".**
- **Order:**
  1. `logs/p9/perf-verdict.txt` (05:37) reads "two confirmed OVER verdicts … Under the rule they block".
  2. Its profile already located the extra work in the join (`join_handing_back +109 samples (microservices_demo), +62 (spring_boot)`).
  3. Amendment 3 was committed at 06:00 (`73e2e62`) with the exemption "no single rb function outside decode or join takes more than 10 %".
- **The boundary is not drawn consistently:**
  - `remove_lombok_generated` is called inside `join_handing_back` (`scip.rs:238ff`), yet it was counted as outside the join. That is why P9b and P9c had to fix it (10.2 % at P9b).
  - `rb_scip::symbol::package` (17.6 %), `NameForms::of` (13.4 %), `last_descriptor_name` (8.1 %) and `may_name` (8.9 %) are called from the same function but were counted as inside it (`profile-shares4.txt`).
  - Together these are about 48 % of spring's `index_commit` samples. They are, in the contract's own words, "each parse every distinct symbol separately": work a memo would remove, which is what (iii) was written to catch.
  - The verdict lists also omit `rb_indexer::assemble` (55.6 %).
- **My A/B reproduces the overage:** CPU 1.211× and 1.455×, instructions 1.252× and 2.439× (What reproduces).
- **Records that overstate:**
  - CHANGELOG says "their profiles show no avoidable per-occurrence work after P9b and P9c".
  - The amendment itself records no user approval; only the P9c note says "approved".
- **Settle:**
  - reword C16 in the Result, CHANGELOG and IMPLEMENT as "over the limits; waived under amendment 3, written after the result", and get the user's explicit sign-off;
  - or do the shared per-symbol parse (Out of scope) and re-run the A/B.

**S3. Should-fix: 5 of my 13 mutants on M9d's code survive, each a real behaviour change no test pins.**
- **Repro:** `python3 -I bench/mutants/run.py var/m9d/eval1/mut/eval-m9d.json --jobs 1` (`eval1/mut/eval-m9d.log`). `crates/` hashed the same before and after.
- **KILLED (8):**
  - e01: `testdata` is a workspace module;
  - e02: `go.work` takes the lowest `go` line;
  - e04: an absolute replace target is not local;
  - e05: the pom chain merges in the wrong order;
  - e06: D5b stores the re-run's tail;
  - e10: the project name is the first segment of the root;
  - e11: `GOWORK` names the repository root's `go.work` for a D5+D6 job;
  - e13: the D10 keys are written whenever there are locals (control).
- **SURVIVED:**
  - **e12** (`orchestrate.rs:954`, `if (path.startsWith(home))` becomes `if (!path.startsWith(home))`): the init script then marks no tool under the Gradle home, so D13 does nothing. The unit test checks only that substrings are present (`orchestrate.rs:2897-2904`), and no e2e fixture uses the protobuf plugin. D13's behaviour is verified only by the P0 and P7 probes and the bench.
  - **e09** (`jobs.rs:334`, the `pyproject.toml` check dropped from `requirements_only`): a Poetry project that also commits a `requirements.txt` (exported pins are common) switches from `poetry install` to pip, and drops D2's "keeps today's arguments". No row has `pyproject.toml`, `poetry.lock` and `requirements.txt` together.
  - **e08** (`decode.rs:332`, `contains("invalid type")` becomes `contains("invalid")`): a local named or typed `invalid…` would count as a load failure. There is no negative row.
  - **e03** (`jobs.rs:778`, pre-releases ignored in `go_version_key`): the doc comment's `1.21` < `1.21rc1` < `1.21.0` order is untested.
  - **e07** (`orchestrate.rs:441`, D5b's elapsed taken from the re-run only): amendment 2 says "The elapsed time covers both runs", and no test says so.
- **Settle:** a row for each, and the specs (or equivalents) added to `bench/mutants/m9d.json`.

**S4. Should-fix: the gate's accept explanations and the Result describe some pairs wrongly.**
- **fastapi:** `bench/accept-m9d.tsv` says "All 47 fp are rb's Tier B CALLS to an inherited `__init__` (M9c B5: BaseRepository.__init__, TypedTable.__init__)".
  - By callee (`eval1/inv/fp-new.txt`), 24 are inherited: `base.py:5` 16 and `tables.py:15` 8.
  - 23 are the class's **own** `__init__`, read in source:
    - `ArticlesRepository.__init__` (`articles.py:29`) 13;
    - `ProfilesRepository.__init__` (`profiles.py:15`) 7;
    - `CommentsRepository.__init__` 1;
    - `Parameter.__init__` (`tables.py:8`) 1;
    - `FakePoolAcquireContent.__init__` 1.
  - The common cause is the constructor call joined to `__init__` against scip-python's class. That cause holds; "inherited" does not.
- **The Result's line on the new fp** ("an inherited `__init__` … fastapi 47, microblog 2, microservices_demo 1 + 2 file-scope") is wrong for all 5 non-fastapi fp:
  - microblog's `forms.py:17` and `:42` are each form's own `__init__`;
  - microservices_demo's 3 are the generated stubs' own `__init__` (`demo_pb2_grpc.py:162`, `:229`, `:629`).
- **mattermost:**
  - The accept line still says the 19th fp is "not explained in P9", while the Result and B15 explain it.
  - "64 are new fn whose callees v0.4.3 lacks" holds for 61 of 64. `Channel.IsGroupOrDirect`, `Channel.IsBoard` (`createChannel`) and `PropertyOptions.IsValid` exist in `v0.4.3`. Their receivers' types (`model.ChannelCreateRequest`, …) do not, so the old index had no symbol at those spans.
- **Settle:** correct the explanations and the Result. The gate's verdicts do not change.

**S5. Should-fix: numbers in the Result do not trace to the logs they cite.** The full list is in the sub-agent trace. I confirmed each of these.
- **"warm full bench 2 min 57 s"** (C16, Perf): `p9/bench-record.start` and `.end` read `16:08:44` and `16:11:56`, which is 3 min 12 s. 2:57 is the output files' mtimes, which the Names table says and C16 and Perf do not.
- **"mattermost's scip-go indexes 1.16 µs"** (Perf, P9 note, `perf-verdict-run3.txt`): the denominator of 2,321,257 occurrences includes 127,010 scip-typescript occurrences (`occ-counts.txt:13,18`). For scip-go alone it is about 1.23 µs.
- **Wrong log cited:**
  - C5's "0 of 202,391 locals invalid" cites `p4/mm-probe.txt`, which does not hold it. It is in `p4/mm-measure.txt:1`.
  - C8's "30 DEPENDS_ON" cites `p5/mm-packages.txt`, which says 31 (30 plus sharedchannel-test).
  - C9's `invalid_locals_are_written_only_when_non_zero` red run is not in the cited `c9-*` logs. It is in `p5/c7-c8-c9-indexer-red.txt:7`.
- **Close-run claims that no close log records:** "0 `t_*` rows and 0 `t_` caches after it", "0 scratch databases, 0 `rb-` containers, 0 networks, no `/tmp/rbmut*`". `close/full_suite.nohup` and `close/batteries.nohup` are 0 bytes.
- **Other claims that do not match their logs:**
  - "no autovacuum worker and no vacuum in progress": `quiet-before-record.txt` has dead-tuple counts, not that query.
  - "P10's two record commits": there are three, `e7d8488` included.
  - P9's `cargo clean` "under 10 GiB": `cargo-clean.txt` reads 10 Gi.
  - `dump-identity-run2.txt` "compares the first run with the P9b run": its mtime is 15:36:52, before the P9b run started.
  - `p9b/fmt.txt` and `p9c/fmt.txt` are empty (0 bytes, no exit code).
- **Settle:** fix each citation or number in the Result and the P9 note.

## Notes

**N1. D2 plans a job for a requirements-only directory that holds no Python file, and it changes `setup.py`-only roots.** In `py_mix`, `tools/` (`requirements.txt` and a README) is a scip-python job: a container run that is `ok` with 0 of 0 definitions. `svc/` (`setup.py`, no `pyproject.toml`) now indexes with `--project-name svc --project-version .`, which changes its symbol strings. That follows D2's wording, but no bench or e2e target has a `setup.py`-only root, so the change is unmeasured.

**N2. On a double failure, D5b stores only the re-run's tail.** For `go_ws2/wsrep` the row's tail is the plain run's `go: …inner@v0.0.0: … 404`. The workspace attempt is invisible, while a successful D5b keeps the workspace's tail. This conforms to amendment 2 ("its own outcome"), but it makes S1 harder to diagnose.

**N3. The judge prompt pre-empts the one doubt D8 raises.** The added line "The version in a key does not matter … whichever version go.mod requires" tells judges to accept a version-skewed resolution as TRUE, which is the `exact`/`synthetic` question itself. No harm follows here: my decode shows all 23,180 D8 edges byte-equal under `go.work`. Keep the line out of prompts that judge cross-module `exact` edges.

**N4. The e2e start-up clean-up now deletes every `t_*` directory under `var/scip/cache`** (P1's deviation), so it can delete a running `t_*` probe's cache: the prefix that evaluator and agent probes are told to use. It removed my planted tree as intended. The contract's rule "`t_*` probes finish before any suite starts" is the only guard.

**N5. `Dockerfile.jdk25` copies its tools `FROM rb-scip-java:0.13.1` by tag.** Its provenance is whatever that local tag points to when it is built. The files are byte-equal today, and the cache key is the image ID.

**N6. M9c's `synthetic` join also binds a third-party module's references to an in-repo copy.** In `go_wsfail`, the plain index references `github.com/google/uuid v1.6.0` from the module cache, and D8′ joins 2 of them `synthetic` to the in-repo `uuid` job, whose code the type checker never saw. This predates M9d and is labelled `synthetic`. It is recorded for the next design of the join.

## Safety

- **`SELECT * FROM rb.repo ORDER BY 1`** was identical before, mid-run and after (`var/m9d/eval1/repo-{before,mid,after-d15,after}.txt`): the user's 4 rows `fx_man`, `fx_date`, `fx_full` and `alvorabackend`. They were never touched.
- **Leftovers:** 0 `t_*` repositories, 0 `rb_t_*` graphs, 0 `t_*` caches, 0 `rb-` containers and 0 `rb-scip` networks are left. `rb gc --prefix t_eval1_m9d_ --egress` removed the two networks my runs created.
- **Cache rows:** the `rb.scip_index` rows my probes wrote are content-keyed cache rows and are left, as every phase's were. One is the M9c-recipe row of `npm_url` rewritten by the `--no-cache` control.
- **Mutants:** no mutant touched a DB write path. 12 are in rb-scip, and e13 is in rb-indexer's `scip_status` (pure).
- **Database tests:** the only DB test run was `topo_packs`, whose start-up clean-up removed nothing but my planted cache (`0 leftover test repositories`).
- **Docker:**
  - No image was built, deleted or retagged. The three pinned images keep their IDs.
  - Downloaded sources ran only in containers: `npm install --package-lock-only` and every fetch.
- **Disk:** free space stayed at 16–20 GiB, so no `cargo clean` was needed. `target/release` was rebuilt for C13.
- **Edits:**
  - The tracked tree is unchanged except for this report. I never used `git checkout --`, `git reset` or `git stash`.
  - I did not touch `worktrees/`.
  - `bench/probe/__pycache__/` (untracked) was present at my first `git status` and is now gone. None of my commands deletes it: the scripts I ran set `dont_write_bytecode` or are cargo, docker and psql. I cannot account for its removal.
- **Commit:** I committed only this report and pushed nothing.
