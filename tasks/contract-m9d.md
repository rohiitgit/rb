# M9d contract: SCIP fetch and index failures (B4), Go workspaces for in-repo modules, scip-go health, `rb check` JIT (2026-10-17)

**Status:** contract written 2026-10-17 from `tasks/plan-m9.md` §4 "M9d", the note under M9c, `tasks/contract-m9c.md` amendment 2 and the PI note (the `go.work` question, scip-go's silent degradation), and the open item in `tasks/todo.md` (M9b pass 2, S3: `unowned_table_writes`). Not yet built. M9d is built after M9c closes; commits go on branch `m9`.

**Evidence base.** File:line references are at HEAD `15371eb` (M9c P4), checked by `var/m9d/plan/scripts/check_citations.py` (`logs/citations.txt`: 42 of 42 hold; the control, shifted 7 lines, fails 42 of 42). M9c P5 is uncommitted in `crates/rb-indexer/src/{lib.rs,scip.rs}`, `crates/rb-store/src/{query.rs,db_tests.rs}` while this is written, so citations into `lib.rs` and `scip.rs` are read from HEAD with `git show`, and P0 re-reads both at M9c's close. The run of record is M9b's `var/bench/out/36ed5ea46ddf/` (M9c has not benched yet). Every probe log, script and raw index is under `var/m9d/plan/{logs,scripts,idx}/` (gitignored). Probes ran in Docker only: `--network none`, or behind an egress proxy built like rb's (`scripts/proxy_fetch.sh`: `rb-egress-proxy:1.11.3`, the `REGISTRIES` filter, own `m9d-probe-*` names, removed after each run). Every prediction that depends on M9c's outcome is marked **[M9c]**.

## Why

| Item | What is missing | Measured (M9b run of record, or probe) |
|---|---|---|
| B4 | 8 SCIP jobs fail and 5 Python roots get no job, so 5 targets have vacuous `scip` cells (gold 0) and 2 more lose part of their reference | 7 targets, 9 failed or absent jobs; table below |
| Go skew | mattermost `server` type-checks against `server/public v0.4.3` from the module cache, not the in-repo tree | 1,541 locals `invalid type`, 172 agreement fp, every D8′ join `synthetic` (M9c amendment 2) |
| Health | scip-go exits 0 when packages fail to load; `rb status` cannot see it | 119,977 invalid locals on the old image went unnoticed for 5 days (M9c amendment 2) |
| `rb check` | `unowned_table_writes` takes 1.8–2.5 s on a 44-node graph under load, at the 2,000 ms timeout | `check_gates`, `a_checkout_is_its_repository_whatever_its_name` fail under load (`var/m9b/logs/r3/s3-limit-probe.txt`, `s3-check-ab.txt`) |

## Root causes, verified (file:line at `15371eb`)

**B4: every failing or absent SCIP job in the run of record.** The 11 `*.scip.status` files list 8 failed jobs; `microblog.scip.out` lists none. Each failure replays from `rb.scip_index` (`failed (cached)`, wall 2–5 s). Rows were matched to jobs by their `elapsed_ms`, which equals the `.scip.out` time for each (`logs/failing-jobs.tsv`, `failed-rows.txt`, `failed-tails.txt`, read `READ ONLY`). All 8 were written on 2026-10-13 by the M8 bench and never re-run.

| # | Target / root | Indexer, phase, time | Root cause (verified) | Evidence | Verdict |
|---|---|---|---|---|---|
| 1 | nestjs_realworld `.` | scip-typescript, fetch, 1,526 s | `package-lock.json` (v1) resolves 194 of 1,100 entries from `https://npm.styque.de`, a host outside the allow-list (`egress.rs:36`). The fetch runs `npm ci` (`node_lock` prefers `package-lock.json`, `jobs.rs:280`) and fails after 25 min with `egress proxy refused: npm.styque.de:443`. All 194 entries (146 distinct) have the same integrity as `registry.npmjs.org` and the same tarball path | `logs/styque-integrity.txt`; probe `logs/nestjs-npm-ci-proxy.txt`: with `npm_config_replace_registry_host=always`, `npm ci` adds 1,100 packages in 150 s, connects only to `registry.npmjs.org`, 0 refused; index rc 0 in 2 s, 275 KB | **fix (D1)** |
| 2 | fastapi_realworld `.` | scip-python, fetch, 13 s | The scip-python image is Alpine (musl): no manylinux wheel applies, so Poetry builds every pin without a musllinux wheel from its sdist. `pyyaml 6.0` (a `dev` dependency, `poetry.lock:992`) fails under Cython 3 (`'build_ext' object has no attribute 'cython_sources'`). The stored tail shows only `Creating virtualenv …`: Poetry 1.6.1 prints its error on stdout, and the runner discards stdout (`runner.rs:288`) and keeps 40 lines of stderr (`runner.rs:327`) | `logs/scip-python-image-tags.txt` (Alpine, musl, 0 manylinux tags); `fastapi-poetry-offline.txt` (error on stdout); `fastapi-poetry-proxy.txt` (the Cython error, 9 s); `scip-python-noenv.txt`: `scip-python index` with no environment, offline, rc 0, 11 s, 808 KB | **fix (D3, D4)** |
| 3 | microblog `.` | none planned | Python roots are only `pyproject.toml`/`setup.py` directories (`jobs.rs:246`); microblog has only `requirements.txt` (54 pins). Without a pyproject, `scip-python index` crashes on an undefined project version (`TypeError … makePackage`) | `scip-python-noenv.txt`: crash, then rc 0 and 263 KB with `--project-name microblog --project-version .`; `microblog-pip-proxy.txt`: `pip install --no-deps --ignore-requires-python -r requirements.txt` rc 0 in 43 s | **fix (D2)** |
| 4 | microservices_demo `src/{emailservice,loadgenerator,recommendationservice,shoppingassistantservice}` | none planned | Same planner rule: each holds only `requirements.txt` (5, 1, 5, 1 `.py` files) | read of the tree; fetches not measured (`shoppingassistantservice` uses `--hash` pins; grpcio on musl) | **fix (D2)**, P0 measures the fetches |
| 5 | mattermost `tools/sharedchannel-test` | scip-go, fetch, 0.7 s | Its `go.mod` replaces server/public with `../../server/public` (`go.mod:51`), but Go jobs export only the root, root-relative (`projects.rs:255`), so `/server/public/go.mod` does not exist | `sharedchannel-probe.txt`: root plus `server/public` exported repository-shaped, cwd at the root: fetch rc 0, index rc 0 in 16 s, 194 KB, 219 server/public member references, all version `.`, 0 of 242 locals invalid | **fix (D6)** |
| 6 | spring_boot_microservices `.` | scip-java, fetch, 150 s | `<java.version>25</java.version>` (`pom.xml:15`) drives `maven.compiler.release`; the image's JDK is 21 (`docker/scip-java/Dockerfile:5`): `release version 25 not supported`. `repo.spring.io:443` was also refused (a milestone repository in `api-gateway/pom.xml`), effect unmeasured | `scip-java-jdk25.txt`: scip-java 0.13.1 run on `eclipse-temurin:25-jdk` (25.0.4.1), offline, over a two-class project with `release 25` and an unnamed pattern: `BUILD SUCCESS`, rc 0, `Greeter#greet()` defined and referenced. The full Spring Boot 4.1 build is not measured | **fix (D12), gated by P0** |
| 7 | microservices_demo `src/adservice` | scip-java (Gradle), fetch, 27 s | `:generateProto` cannot run `protoc-4.36.1-linux-aarch_64.exe`: error 13. The cached file is mode `0644`. Inside the container, on Docker Desktop's bind mount, `test -x` and `File.canExecute()` say yes for root while `execve` fails EACCES, so a plugin that trusts `canExecute()` never sets the bit | `protoc-exec.txt` (APFS clone of the cache: `-rw-r--r--`, `test -x: yes`, `canExecute=true`, `Permission denied`; after `chmod +x`: `libprotoc 36.1`) | **fix (D13), gated by P0** |
| 8 | kotlin_spring_realworld `.` | scip-java, index, 122 s | 43 `.kt` and 0 `.java` files. scip-java's Maven path indexes javac output only; Maven compiles the Kotlin, `BUILD SUCCESS`, and scip-java writes no index (`orchestrate.rs:539`, `(no index.scip was produced)`). The four 2026-10-12 rows (`b9a8966b…` etc.) show the Gradle Kotlin path failing too (`AbstractMethodError` in the FIR checkers) | `failed-tails.txt` | **record only** |
| 9 | mattermost `webapp` | scip-typescript, fetch, 3 s | `webapp/.npmrc:6` `engine-strict=true` with engines `node 24.11.1`, `npm 11.6.2`; the image has 24.6.0 / 11.5.1 (`EBADENGINE`). Behind it: 2 `git+ssh://git@github.com` dependencies (`marked`, `react-bootstrap`), and 4,432 TS files with 2,017 packages indexed emulated under 4 GiB (unmeasured) | `failed-tails.txt`; lockfile read | engine check **fix (D1)**; git dependencies and size **record only** (stays failed) |
| 10 | mattermost `e2e-tests/cypress` | scip-typescript, fetch, 23 s | `git+ssh://git@github.com/larkox/js-client-oauth2.git`: the proxy refused `codeload.github.com:443` and `github.com:443`, then `git ls-remote` over ssh failed on DNS | `failed-tails.txt` | **record only** |

Not jobs, recorded: microservices_demo `currencyservice` and `paymentservice` are JavaScript without a `tsconfig.json` (no job; scip-typescript's `--infer-tsconfig` is not used); `cartservice`, eshop_on_web (C#) and laravel_realworld (PHP) have no indexer. The other 6 failed rows in `rb.scip_index` are not bench jobs: the user's `alvorabackend` (`EBADENGINE`, engines `node 22.x`), the four Kotlin Gradle rows, and two deliberate e2e failure fixtures (`@acme/shared`, `com.acme:nope`).

None of the 8 is a fetch timeout, an OOM or the disk budget. Every fetch that ran used the bench budget (`--scip-timeout-secs 3600`); the default is 600 s (`orchestrate.rs:206`).

**Go skew: one `go.work` probe on mattermost `server`, `rb-scip-go:0.2.7-xtools0.50.0` (`sha256:47eab865…`).**
- **Run 1, `--network none` on a clone of the bench's module cache** (`logs/gowork-probe.txt`, `scripts/gowork_probe.sh`).
  - `go work init . ./public` writes `go 1.27.2` and `use ( . ./public )`, as `make setup-go-work` does (`server/Makefile:466-473`). mattermost gitignores `go.work`, and no bench repository commits one.
  - Workspace mode refuses the job's `GOFLAGS=-mod=mod` (`orchestrate.rs:860`): `go: -mod may only be set to readonly or vendor when in workspace mode`.
  - With `GOFLAGS` empty: 135 packages have load errors (0 without go.work). The workspace graph needs `github.com/rogpeppe/go-internal@v1.14.1`'s `go.mod` (via `bep/imagemeta v1.0.0`), which neither job's `go mod download` fetched (`logs/gowork-golist.txt`). Locals typed `invalid type` rise from 1,541 to 13,234. So the cache does not allow `--network none`: the fetch must run under the same `go.work`.
- **Run 2, as the job would run: the fetch under `go.work` behind the proxy, then the index offline** (`logs/gowork-probe2.txt`, `scripts/gowork_probe2.sh`). The fetch: rc 0, 195 s, `go.work.sum` written, 0 refused. The index: 0 package errors, scip-go rc 0, 167 s, `memory.peak` 3,681,382,400 B of 4 GiB, 0 `max`/`oom` events. Load was 4.77 at the start (M9c measured 94 s without go.work on a quieter host). The cache's growth was not measured.
- **The index** (`logs/gowork-measure.txt`, `scripts/gowork_measure.py`, against M9c P0b's fixed-image index):

  | | no go.work (M9c) | go.work, cache only | go.work, fetched |
  |---|---|---|---|
  | documents (under `public/`) | 1,830 (0) | 1,830 (0) | 1,830 (0) |
  | definitions | 29,410 | 29,410 | 29,410 |
  | occurrences | 1,911,159 | 1,734,283 | 1,918,253 |
  | locals typed `invalid type` | 1,541 / 202,391 | 13,234 / 202,391 | **0** / 202,391 |
  | server/public member references (version) | 274,382 (`v0.4.3`) | 275,274 (`.`) | 281,357 (`.`) |
  | of which byte-equal to a server/public definition | 0 (270,290 modulo version) | 271,248 | **277,264** |
  | not defined by server/public's index | 4,092 | 4,026 | 4,093 |

  server's index does not absorb server/public's packages (0 documents under `public/`), so the two jobs stay separate.
- **The 4,092 are not skew** (`logs/gowork-residue.txt`). They are 133 interface-method symbols (134 under go.work): `request.CTX#Session()` 1,300, `CTX#Logger()` 988, `mlog.LoggerIFace#Warn()` 547, `#Error()` 363, … server/public's own index defines no symbol for them (0 in its definitions and 0 in its `SymbolInformation`), and go.work leaves them. `request.CTX` (`shared/request/context.go:199`) and `mlog.LoggerIFace` (`shared/mlog/mlog.go:34`) are interfaces.
- **Agreement, emulated** (`logs/gowork-agreement.txt`, `scripts/gowork_agreement.py`: M9c P0b's `agreement_cells` unchanged, server's index swapped). It reproduces M9c's S0/S1/S2 first.

  | `scip:mattermost:noscip:calls` | gold | tp | fp | fn | P | R |
  |---|---|---|---|---|---|---|
  | S0, the M9b record | 33,312 | 20,640 | 6,141 | 12,672 | 0.771 | 0.620 |
  | M9c S2: fixed image + D8′ join | 41,878 | 26,591 | 190 | 15,287 | 0.993 | 0.635 |
  | fixed image + go.work server index | **42,126** | **26,763** | **18** | **15,363** | 0.999 | 0.635 |

  Under go.work every cross-job pair joins by an equal symbol string (8,814 pairs; 0 need the version dropped). The 172 skew fp become tp. Noscip edges are the M9b record's, so B2's effect (M9c P3) is not in these numbers **[M9c]**.
- **rb's view of these references** (HEAD). A symbol the job does not define is `references_undocumented` only if its package is one of the job's own (`scip.rs:376` at `8e91884`, `:336` at `15371eb`; comparing manager, name and version), else `references_external` (`scip.rs:621` at `8e91884`, `:553` at `15371eb`). So server/public symbols with version `.` stay external, and M9c's D8′ hands them back (uncommitted P5) **[M9c]**.
- **Package nodes.** The M9b `scip` dump holds 30 `scip` `DEPENDS_ON` into `go/github.com/mattermost/mattermost/server/public@v0.4.3`; the `noscip` dump holds 0. Under go.work the version is empty, so these would move to a new `…server/public@?` node unless D9 applies.

**Health.** M9c amendment 2 measured, across every bench scip-go job: 119,977 invalid locals in server on the old image, 1,541 on the fixed one (skew), 0 in every other job. This probe adds 13,234 for the cache-only workspace, 0 under the fetched go.work and 0 in sharedchannel-test. scip-go writes nothing about load errors and exits 0. `go list -e` sees the workspace's missing module but not the skew (0 errors without go.work, 1,541 invalid locals), so the index content is the signal, as M9c concluded. rb's decoder drops `SymbolInformation.signature_documentation` (`decode.rs:659`), so nothing reads it today.

**`rb check`: the slow query is LLVM JIT, not the LIMIT.** Probe `t_m9d_chk`: the `topo-packs` fixture committed to a scratch git repo, indexed `--no-scip` by `var/m9b/bin/rb` (sha256 `36ed5ea4…`), 44 nodes and 52 edges, deleted afterwards (`logs/explain-1.txt`, `explain-variants.txt`, `scripts/explain*.sql`; PostgreSQL 18.6, as `rb_reader`, the exact SQL `age::cypher_sql` builds, `age.rs:260`).

| Variant | Execution | JIT |
|---|---|---|
| A: no LIMIT | 29.1 ms | 137 functions generated (5.7 ms), never optimised or emitted |
| B: `LIMIT 1001`, as rb appends (`lib.rs:1627`) | 318.4 ms | inlining 50.2, optimisation 135.1, emission 130.4 ms (total 317.5) |
| G: B again | 267.0 ms | 266.6 ms |
| C: B with `SET LOCAL jit = off` | **1.6 ms** | none |
| D: LIMIT in an outer query | 367.7 ms | 348.2 ms |
| E: `orphan_endpoints`, no LIMIT, 1 row | 0.4 ms | none (plan cost 1,459) |

- The rule's plan costs 1,044,383. That is above `jit_above_cost` (100,000) and above `jit_inline_above_cost` and `jit_optimize_above_cost` (500,000; server defaults).
- The cost is high because none of the graph's 27 AGE label tables has ever been analysed: the planner assumes 1,200 rows per vertex label and 970 per edge label, and five `OPTIONAL MATCH` joins multiply that to 2.86M rows. rb analyses `rb.node`, `rb.edge` and the rest after publishing (`lib.rs:1170`), but not the AGE graph.
- A pays nothing only because its result is empty, so no compiled expression runs. The Limit node evaluates its count at start-up, which compiles the whole module.
- JIT time scales with CPU contention: 0.27–0.37 s at load 2.4–3.2 here, 1.8 s at load 5 in M9b's probe. `rb check` on this graph took 0.52 and 0.33 s at load 2.9, against 2.06–2.47 s at load 4.1–6.3 in M9b's A/B.
- An outer-query LIMIT does not help (D).

## Plan corrections

1. **Targets.** The plan names 4 targets. The run of record has 8 failed jobs on 6 targets, plus microblog's and microservices_demo's 5 unplanned Python roots, plus server's skew.
2. **Causes.** No failure is a timeout, an OOM or the disk budget. nestjs looked like a timeout (25 min), but it is a lockfile host outside the allow-list. kotlin_spring_realworld fails in the index phase, and scip-java 0.13.1 cannot index it.
3. **M9c amendment 2's "4,092 members it no longer defines (skew)" is wrong:** they are interface methods server/public's index defines no symbol for, and go.work leaves 4,093. The skew is the 1,541 invalid locals and the 172 fp. M9c's risk list and Out of scope inherit the error; M9d records the correction, and M9c's record is not edited.
4. **go.work alone makes things worse** with the existing cache (13,234 invalid locals): the fetch must run under the same go.work.
5. **M9c's open question is answered yes.** Under go.work, 277,264 of 281,357 server → server/public member references are byte-equal to server/public's definitions, so D8′'s join can be `exact` (D8).
6. **The `rb check` item is JIT, not the LIMIT** (above). The fix is one session setting.
7. **Scope.** The plan's file list is empty. M9d touches `rb-scip` (`jobs.rs`, `orchestrate.rs`, `decode.rs`), `rb-indexer` (`projects.rs`, `scip.rs`, `lib.rs`), `rb-store/src/lib.rs`, `rb-cli` (`main.rs`, `doctor.rs`) and `docker/scip-java/`.
8. **Mutant ids** start at **m360**: M9c owns m300–m340 and may add more.

## Design

```mermaid
flowchart LR
    PL[jobs: planner] -->|requirements.txt roots D2,<br/>go.mod requires / replaces D5 D6,<br/>Maven release D12| J[Job]
    J --> PF[projects::job_files<br/>replace targets exported D6]
    PF --> RJ[orchestrate::run_job]
    RJ -->|npm env D1| TS[scip-typescript]
    RJ -->|project flags D2, stdout D3,<br/>fallback D4| PY[scip-python]
    RJ -->|go.work both phases D5| GO[scip-go]
    RJ -->|JDK 25 image D12,<br/>exec bits D13| JV[scip-java]
    TS & PY & GO & JV --> DEC[decode: invalid locals D10] --> SJ[scip::join + D8′ join]
    SJ -->|byte-equal symbol: exact D8;<br/>in-repo module: no Package D9| G[(graph)]
    G --> CY[cypher(): jit off D11] --> CHK[rb check / rb cypher]
    G --> D[dumps] --> SB[score] --> CMP{compare vs M9c record}
```

**State, derived state, invariants.**
- **State.** No `FileFacts` change and no migration. A `Job` gains `workspace` (in-repo module roots for its generated `go.work`), `replace_targets` (in-repo directories outside the root that it exports) and `jdk` (21 or 25). The `rb.scip_index` row of a Python job whose fetch failed and whose index ran without an environment is `status = 'ok', phase = 'fetch'`, a combination the existing CHECK constraints allow and no current binary writes.
- **Derived.** Per scip-go index: `locals` and `locals_invalid_type` (D10). In the D8′ join: certainty from symbol equality (D8).
- **Invariants.**
  - No `noscip` row changes. M9d touches only the SCIP path, `cypher()` and `rb status` text.
  - A job whose inputs and recipe are unchanged keeps its cache row. Only nestjs-style npm jobs (all TS, by the recipe bump), Python jobs, and Go jobs with a workspace or replace target are re-run.
  - `exact` from D8 only when the reference's symbol string equals the defining job's definition string, version included.
  - The fetch allow-list does not change (`egress.rs:36`).
  - A degraded index is always visible: an environment-less Python index (D4) and any `invalid type` local (D10) print on the job's status line.

**Trade-offs, with this system's numbers.**
- **nestjs: rewrite hosts or use `yarn.lock`.** nestjs also holds a `yarn.lock` (785 entries, all `registry.yarnpkg.com`). Preferring it would change the planner's lockfile order for every repository with both. `replace-registry-host=always` rewrites only the host, and the lockfile's integrity still checks every tarball (194 of 194 match). **Chosen:** the npm setting. Adding `npm.styque.de` to the allow-list is rejected: a private mirror is not a registry rb trusts by default, and `RB_SCIP_REGISTRIES` already lets a host opt in.
- **fastapi: fix the build or index without the environment.** `poetry install --only main` would drop pyyaml, but `asyncpg 0.26` and `psycopg2-binary 2.9.3` (2022 pins) have no musllinux wheels and need a compiler the image lacks (unmeasured). A glibc scip-python image is not published for 0.6.6. scip-python indexes the project's own code without an environment (measured: rc 0, 808 KB), as `PythonLock::None` already does on purpose ("the safe direction", `orchestrate.rs` doc of `python_runs`). **Chosen:** the fallback (D4), shown on the status line. It costs third-party resolution, which no agreement cell scores.
- **go.work or a `replace` overlay.** A `replace => ./public` written into server's `go.mod` edits a repository file (the input hash, the export, `go.sum` semantics). go.work is how mattermost itself builds (`server/Makefile:466-473`) and touches no tracked file. **Chosen:** a generated `go.work`, written by rb (deterministic text, unit-testable) rather than `go work init` (whose `go` line follows the toolchain).
- **sharedchannel-test: repository-shaped export for every Go job or only some.** Uniform would mirror scip-typescript (M6a D7), but it re-runs all 13 Go jobs and moves their "documents outside the root" counts for no measured gain. **Chosen:** only jobs with an in-repo target outside the root export repository-shaped; the 11 others keep their layout, their recipe and their cache rows.
- **Exact or synthetic for byte-equal joins.** M9c chose `synthetic` because the type checker resolved `v0.4.3` while the target was the in-repo tree. Under go.work (or a local replace) the checker resolved the in-repo tree: the version is `.`, 0 locals are invalid, and the symbol strings are equal. **Chosen:** `exact` exactly when the strings are equal; `synthetic` otherwise, as M9c built it.
- **JIT: off, or analyse the AGE graph.** `ANALYZE` on the label tables at publish would fix this rule's estimate, but it changes plans for every Cypher query, adds publish time, and leaves JIT on for any ad-hoc query whose estimate passes 500,000 (a 0.3–2 s compile on a statement capped at 2 s). `ALTER DATABASE … SET jit` changes a server rb does not own. **Chosen:** `SET LOCAL jit = off` in `cypher()`'s transaction, next to `statement_timeout`.
- **JDK 25: replace the JDK or add an image.** Gradle 8.14.6 does not run on Java 25, so replacing JDK 21 breaks Gradle jobs and changes every Java index. Overriding `-Dmaven.compiler.release=21` fails on Java 22+ syntax and misstates the build. **Chosen:** a second image from the same Dockerfile (`ARG` base), used only by Maven jobs whose effective release is above 21.

**NFRs.**
- mattermost `noscip-fresh` wall ≤ 1.25× and peak RSS ≤ 1.15× the M9c run of record, with M9c's A/B re-measure rule (CPU within 1.1×, ABBA, `instructions retired`; lesson of 2026-10-16). Predicted unchanged: no noscip path changes.
  - (Noted by M9c amendment 3, E6.) The M9c run of record is r2, `var/bench/out/3b94ded449f8/` with `bench/scoreboard/m9c-perf.json` from it: mattermost walls 26.10 / 16.63 / 20.34 s (`noscip-fresh` / `noscip-cached` / `scip`), checked against a standalone timing of the same binary on a quiet host (22.72 / 16.43 / 20.18 s). r1's walls (69.55 / 74.60 / 71.81 s, `f340b267d529`) were host-inflated and are not the base.
  - (Noted by M9c amendment 4, N6.) M9c's run of record is now the pass-2 run, `var/bench/out/6cd5baaa8d1f/` (binary `6cd5baaa…f5a8`; all 157 dumps byte-identical to `3b94ded449f8`'s), and `bench/scoreboard/m9c-perf.json` is its: mattermost walls 22.96 / 17.74 / 20.18 s, peak RSS 1,954,463,744 / 1,725,612,032 / 2,493,186,048 B. mattermost's peak RSS varies by about 12 % between runs of one binary (M9c evaluator pass 2: 2,153,119,744 B standalone against 1,922,760,704 B in the pass-1 record), close to the 1.15× limit. So a peak-RSS warning on mattermost is settled by an A/B of binary against binary under the rule above, never by the single bench run.
  - (Noted by M9c amendment 3, N2.) With a local `replace` onto the defining root, scip-go type-checks against the in-repo tree, yet D8′ labels the join `synthetic` (it never reads `go.mod`'s replaces). Whether such joins, and `go.work` ones, become `exact` is part of M9d's `go.work` question; the M9c evaluator's `go_d8` fixture (`var/m9c/eval1/fx/go_d8`) is a ready row.
- Scoring ≤ 30 s; two scorings `cmp`-identical; warm full bench ≤ 20 min (M9b r2 warm: 14 min 3 s, 18:30:22–18:44:25).
- `rb check` on `topo-packs` ≤ 100 ms per rule at load ≥ 4 (C10's timing log, not a unit test).
- server's scip-go under go.work: `memory.peak` < 4 GiB with 0 `oom_kill`; wall reported A/B against the no-go.work run under the same load (measured 167 s vs M9c's 94 s at different loads).

### Budgets per target

Cold costs are one-off: a cached index serves every later run. Measured on this host (Apple M4, 10 CPUs) under the job budget `--cpus 2 --memory 4g`; "unmeasured" ones are P0's.

| Target / job | Expected | Fetch (cold) | Index (cold) | Caches added | Warm |
|---|---|---|---|---|---|
| nestjs_realworld `.` | **failed → ok** | 150 s (was 1,526 s, failed) | 2 s | npm 93 MB; `node_modules` 215 MB, transient | cache hit |
| fastapi_realworld `.` | **failed → ok (environment not installed)** | 9–13 s, fails | 11 s | pip and Poetry 35 MB | cache hit |
| microblog `.` | **none → ok** | 43 s | about 10 s | pip 28 MB | cache hit |
| microservices_demo 4 Python roots | **none → ok**, or ok without environment | unmeasured (P0) | unmeasured, ≤ 30 s each expected (1–5 files) | unmeasured | cache hit |
| mattermost `tools/sharedchannel-test` | **failed → ok** | about 1 s (cache warm) | 16 s | none | cache hit |
| mattermost `server` | ok → ok under go.work | 195 s | 167 s, peak 3.68 GB | unmeasured growth of the 1.1 GB module cache (P0) | cache hit |
| spring_boot_microservices `.` | **failed → ok if P0 passes**, else recorded | unmeasured (go-offline took ≤ 150 s before failing) | unmeasured | m2 384 MB exists; JDK 25 image about 1.3 GB in Docker's disk | cache hit |
| microservices_demo `src/adservice` | **failed → ok if P0 passes**, else recorded | unmeasured | unmeasured | Gradle 51 MB exists | cache hit |
| kotlin_spring_realworld, webapp, cypress | stay failed (cached rows replay; webapp's tail changes, D1) | n/a | n/a | n/a | replay |
| every other TS job (bookcars ×9, express, mattermost TS ×3) | ok → ok, re-run once by the TS recipe bump; indexes predicted byte-identical (all their lockfiles resolve only from `registry.npmjs.org`, engines satisfied) | as before | as before | none | cache hit |

Disk: about 0.5 GiB of host caches, plus about 1.3 GiB for the JDK 25 image in Docker's disk. Free space was 13 GiB at the end of planning; the 6 GiB floor holds. The 20-minute budget is for the warm run; P9 runs the bench cold once and then warm for the record (D14).

## Decisions

- **D1 (npm fetch).** Every scip-typescript fetch gets `npm_config_replace_registry_host=always` and `npm_config_engine_strict=false` beside `npm_config_update_notifier` (`orchestrate.rs:579`). Environment variables beat a project `.npmrc`. `RECIPE_TS` `m6a-ts-1` → `m9d-ts-1` (`orchestrate.rs:27`). Yarn and pnpm read neither and are unchanged. Predicted: nestjs ok; webapp still failed, with its tail moving from `EBADENGINE` to the git-dependency refusal; every other TS index byte-identical.
- **D2 (Python roots).** A directory with `requirements.txt` and neither `pyproject.toml` nor `setup.py` is a Python root (`jobs.rs:246`), with `PythonLock::Requirements`. Every Python root without `pyproject.toml` indexes with `--project-name <root's last segment, or "root" at the repository root> --project-version .`. A root with `pyproject.toml` keeps today's arguments.
- **D3 (Poetry's output).** The Poetry fetch runs as `sh -c 'exec poetry install --no-root --no-interaction 1>&2'`, so its error reaches the stored tail. pip writes errors to stderr already.
- **D4 (Python environment fallback).** When a scip-python job's fetch exits non-zero (not a timeout, not a Docker client exit code), the index phase runs anyway, offline, without the environment. If it writes an index, the job is `ok` with `phase = 'fetch'`, and the stored tail is the fetch's. The status line prints `scip-python ok (environment not installed: fetch failed)`. The row is cached like any ok row. `RECIPE_PY` `m6d-py-4` → `m9d-py-1`. Other indexers are unchanged.
- **D5 (Go workspace).** For a scip-go job whose `go.mod` requires (with no `replace`) the module path of another `go.mod` in the commit that lies under the job's root:
  - rb writes `<root>/go.work` into the export before the fetch: `go <highest go line of those modules>`, then `use` with `.` and each such directory, sorted;
  - both phases run with `GOFLAGS` empty (workspace mode refuses `-mod=mod`);
  - the fetch is `go mod download` under that file.

  The job's recipe is `RECIPE_GO_WORK = "m9d-go-1"`. A root that already has a `go.work` is left as it is (Out of scope). On the bench this is mattermost `server` (`use . ./public`) and nothing else.
- **D6 (Go replace targets).** A `replace` in a job's `go.mod` whose target is a relative path:
  - inside the repository and outside the root: its tracked files are exported (`projects::job_files`, `projects.rs:338`, as for scip-typescript link targets), the export is repository-shaped, and the container starts in the root;
  - outside the repository: the job is `unsupported` (`replace target outside the repository`).

  Such a job's recipe is `RECIPE_GO_WORK`; a job with neither D5 nor D6 keeps `RECIPE_GO` and its layout. On the bench this is `tools/sharedchannel-test`.
- **D7 (planner reads go.mod).** `jobs()`'s reader is asked for each Go root's `go.mod` too (today `package.json` and workspace `yarn.lock` only), so D5 and D6 are decided in the plan and covered by the input hash through the exported `go.mod`.
- **D8 (D8′ certainty, delta on M9c) [M9c].** In M9c's post-loop join, an edge is `exact` when the handed-back symbol string is byte-equal to the defining job's definition symbol, and `synthetic` otherwise. `scip_cross_project` gains `exact` beside `{synthetic, ambiguous, unanchored}`. `rb status`'s `exact calls (scip)` (M9c C10) counts them.
- **D9 (in-repo modules are not packages).** A scip-go reference whose package name is another ok scip-go job's module path and whose version is empty creates no `Package` node and no `scip` `DEPENDS_ON`; it is joined (D8). A reference at a real version (`v0.4.3`, with no go.work) keeps today's package. That matches the `noscip` dump, which holds 0 such edges.
- **D10 (scip-go health).** For an index whose tool is `scip-go`, the decoder counts local symbols, and those whose signature text contains `invalid type` (go/types' spelling). The job's coverage records `locals_invalid_type` and `locals`, written only when the first is non-zero, as `lombok_removed` is (`lib.rs:1677` at `8e91884`, `:1530` at `15371eb`). The status line appends `, N of M locals typed invalid`. No threshold: every healthy bench index measures 0.
- **D11 (`cypher()` JIT).** `SET LOCAL jit = off` joins `statement_timeout` and the role in `cypher()` (`rb-store/src/lib.rs:1632`), built by one helper that the test also calls. This covers `rb check`, `rb cypher` and the API's Cypher route; relational queries are unchanged.
- **D12 (JDK 25) [P0-gated].**
  - `docker/scip-java/Dockerfile` takes `ARG BASE` (default: today's Temurin 21 digest), so `rb-scip-java:0.13.1` rebuilds byte-for-byte as before.
  - `rb-scip-java:0.13.1-jdk25` is built with `BASE=eclipse-temurin:25-jdk@<digest pinned in P6>`.
  - A Maven job uses it when the highest effective release over its poms is above 21. The release is read from `maven.compiler.release`, `source` or `target`, or the compiler plugin's `<release>`, with `${…}` resolved through same-pom properties and the parent chain inside the job; `java.version` reaches it only through those properties.
  - A release above 25 makes the job `unsupported`, and so does an absent image.
  - Gradle jobs always use the JDK 21 image.
  - `rb doctor` lists the second image.
- **D13 (Gradle exec bits) [P0-gated].** Gradle runs in both phases get a read-only init script that, before each `GenerateProtoTask` (or once the task graph is ready, as P0 measures), sets the executable bit on the resolved `*.exe` tool artifacts under `GRADLE_USER_HOME`. Without trusting `canExecute()`. `RECIPE_GRADLE` `m6e-gradle-1` → `m9d-gradle-1`. Also, the stale `Fetch::Gradle` doc ("not verified yet: such jobs are `unsupported`", `jobs.rs:52`) is corrected.
- **D14 (gate).** The base is M9c's run of record and `bench/scoreboard/m9c.json`, re-scored with HEAD's rb-bench **[M9c]**. No key changes. Accept lines go in `bench/accept-m9d.tsv`, each naming its claim, bucket and diff. The run of record is warm: P9 runs once cold to fill the cache, and the warm run is the record.

## Claims

| # | Claim | Test |
|---|---|---|
| C1 | P0 record: disk; M9c's record dumps exist and are non-empty; the re-score is `cmp`-identical to `m9c.json`; the planner's probes re-run on M9c's close (citations, go.work agreement against M9c's measured cell, rb.scip_index rows); P0's gated probes (D12 full build, D13 hook, the 4 Python fetches, go.work A/B and cache growth) logged | `var/m9d/logs/p0/`, amendment 1 |
| C2 | D1: fetch arguments carry both npm settings; `RECIPE_TS` pinned at `m9d-ts-1`; yarn and pnpm runs unchanged | unit `npm_fetches_rewrite_registry_hosts_and_ignore_engines` (orchestrate) |
| C3 | D2: a requirements-only root is planned with `Requirements`; one beside a `pyproject.toml` is not doubled; index arguments carry the project flags only without a `pyproject.toml`; the root-level name is `root` | units `requirements_only_directories_are_python_roots` (jobs), `python_roots_without_pyproject_name_their_project` (orchestrate) |
| C4 | D3, D4: the Poetry fetch is wrapped `1>&2`; a failed fetch is followed by an offline index and an `ok`/`fetch` result with the fetch's tail; a fetch timeout is not; a non-Python job is not; the cached row round-trips through `RunStatus::from_cached`; the status line prints the suffix | units `poetry_errors_reach_the_tail`, `a_failed_python_fetch_indexes_without_the_environment` (orchestrate), `python_fallback_rows_round_trip` (projects), a status-line row (rb-cli `main.rs` tests) |
| C5 | D5, D7: `server`-shaped plan (`require` of a module under the root) → `workspace = ["public"]`, the generated `go.work` text, both phases with `GOFLAGS` empty, `RECIPE_GO_WORK`; a required in-repo module that is also replaced → no workspace; a module outside the root → no workspace; a plain job → unchanged arguments and `RECIPE_GO` | units `go_jobs_requiring_in_repo_modules_get_a_workspace` (jobs), `workspace_jobs_write_go_work_and_clear_goflags` (orchestrate) |
| C6 | D6: a `../../x` replace target inside the repository is exported repository-shaped with the container in the root; one leaving the repository is `unsupported`; a plain Go job's export stays root-relative | units `go_replace_targets_inside_the_repository_are_exported`, `go_replace_targets_outside_are_unsupported` (projects) |
| C7 | D8 [M9c]: equal symbol strings → `exact`; `v0.4.3` vs `.` → `synthetic`; the diagnostic counts each | unit `byte_equal_cross_project_symbols_join_exact` (lib), beside M9c's `external_references_join_another_projects_definitions` |
| C8 | D9: a `.`-version reference to another job's module adds no `Package`/`DEPENDS_ON`; a `v0.4.3` one does; a third-party module at `.` (replaced outside any job) still does | unit `in_repo_modules_are_not_packages` (scip) |
| C9 | D10: a scip-go index with 2 locals, 1 `invalid type` → `1 of 2`; non-locals not counted; a scip-typescript index with that text → not counted; written only when non-zero; the status line | units `scip_go_invalid_locals_are_counted` (decode), a coverage row (lib), a status-line row (main) |
| C10 | D11: on a `ScratchDb` graph shaped like `topo-packs`, `EXPLAIN (ANALYZE, FORMAT JSON)` of `unowned_table_writes` under `cypher()`'s session settings has no `JIT` key (deterministic: label tables are never analysed, so the cost stays above 100,000); `rb check` timing on `topo-packs` A/B (M9c binary vs M9d), 5 runs each at load ≥ 4 | rb-store test `cypher_runs_without_jit` (ScratchDb); `var/m9d/logs/p1/check-ab.txt` |
| C11 | D12 [P0-gated]: the release reader (literal, `${java.version}`, parent chain, plugin `<release>`, > 25 unsupported); image choice; Gradle stays on 21; doctor lists both | units `maven_release_above_21_takes_the_jdk25_image` (jobs), the doctor test |
| C12 | D13 [P0-gated]: both Gradle phases carry the init script read-only; `RECIPE_GRADLE` pinned | unit `gradle_runs_mark_tool_artifacts_executable` (orchestrate) |
| C13 | `PIPELINE` M9c's last → `m9d-1` before the verify build; binary of record from a clean `crates/` | Result: HEAD, sha256 |
| C14 | Invariant over every target and PyCG dump against M9c's record, by row identity, byte counts non-zero: every differing row in a bucket below; 0 unclassified; 0 `noscip` rows differ | `var/m9d/logs/invariant.txt` |
| C15 | `compare var/m9d/m9c-rescored.json bench/scoreboard/m9d.json --accept bench/accept-m9d.tsv --perf` exits 0; every `changed` and `accepted` line classified; no `exact` fp rise | Result, pasted output |
| C16 | NFRs | `compare --perf`, A/B logs, C10's log |
| C17 | Mutants m360–m391 KILLED, none STALE or INVALID; `m8b`, `m9a`, `m9b`, `m9c` batteries re-run KILLED | battery logs |

**Invariant buckets (C14).** A row may match one bucket; the first that applies wins.
- **NEWIDX.** Rows of a target's `scip` dump whose job went from none or failed to ok: nestjs, fastapi, microblog, sharedchannel-test, the 4 microservices_demo Python roots, and spring_boot and adservice if P0 passes. These are the job's `scip` nodes and edges, the Tier B rows that the join replaced in its covered files, `Package`/`DEPENDS_ON` from its references, and its `scip_status` row.
- **WORK.** mattermost rows that differ because server's index changed under go.work: `exact` `scip` edges whose evidence span gained or lost an own-index symbol; D8′ rows whose certainty went `synthetic` → `exact` (D8); new joined pairs (the 172 skew sites and the rest of the 248 new gold pairs); the 30 `scip` `DEPENDS_ON` into `server/public@v0.4.3` and that `Package` node removed (D9); server's coverage counters.
- **FALLBACK.** fastapi's `scip_status` row (status suffix, phase).
- **HEALTH.** `locals_invalid_type`/`locals` keys. Predicted empty: every index of record is 0 once server runs under go.work.
- **RECIPE.** TS and Python jobs re-run by the recipe bumps. Predicted empty; webapp's `scip_status` tail is not in the dump, and its status line is unchanged (`failed (fetch phase)`).

**Predicted movement.** The base is M9c's record **[M9c]**. Where M9c has not measured yet, the M9b record and M9c amendment 2's emulation are quoted.

| Cell (`bench:target:mode:class`) | Base | Predicted m9d | Claim |
|---|---|---|---|
| `scip:nestjs_realworld:noscip:calls`, `calls_file_scope` | gold 0 (vacuous) | gold > 0; at most 30 call sites (the index's references to own callables, `logs/new-index-shapes.txt`); tp, fp (heuristic only), fn measured; accept line NEWIDX | C2 |
| `scip:fastapi_realworld:noscip:calls`, `calls_file_scope` | gold 0 | gold > 0; at most 335 call sites; accept line NEWIDX + FALLBACK | C4 |
| `scip:microblog:noscip:calls`, `calls_file_scope` | gold 0 | gold > 0; at most 89 call sites; accept line NEWIDX | C3 |
| `scip:spring_boot_microservices:noscip:calls`, `calls_file_scope` | gold 0 | P0-gated: gold > 0, else unchanged | C11 |
| `scip:microservices_demo:noscip:calls`, `calls_file_scope` | M9b: gold 208, tp 208, fn 0 (Go only) | gold rises by the 4 Python roots' pairs (and adservice's if P0 passes); fn may rise (accept line NEWIDX); Go part unchanged | C3, C12 |
| `scip:mattermost:noscip:calls` | M9c's close [M9c]; emulated M9c S2: gold 41,878, tp 26,591, fp 190, fn 15,287 | relative to S2: gold +248, tp +172, fp 190 → 18, fn +76 (emulated with the M9b noscip edges; M9c's B2 edges change tp and fn, not the reference) [M9c]. Plus sharedchannel-test's own pairs (NEWIDX). Accept line for fn, bucket WORK | C5, C6, C7 |
| `scip:mattermost:noscip:calls_file_scope` | M9c's close [M9c] | measured and classified at P9 (not emulated) | C5 |
| `scip:{bookcars,express_ts_boilerplate,go_clean_arch,go_gin_example}:*` | M9c's close | identical | |
| `exact` sample, `scip` family | M9c's judged pool [M9c] | D8 turns M9c's `synthetic` cross-project `CALLS` into `exact` (about 8,800 mattermost pairs, emulated), and new targets add `exact` scip edges, so the draw moves: every drawn edge without a verdict is judged under the M8b protocol before the gate | C7, C14 |
| `rb status` mattermost `exact calls (scip)` | M9c's close | rises by the D8′ `CALLS` edges M9c records as `synthetic` [M9c] | C7 |
| `noscip` cells (handkey, microsecend, PyCG, every `noscip` dump) | M9c's close | identical | C14 |

**Gate expectation (C15).** compare flags fn and fp rising from 0 on each newly scored cell (nestjs, fastapi, microblog, and spring_boot if P0 passes) and fn rising on microservices_demo and mattermost. Each gets one accept line in `bench/accept-m9d.tsv` naming its bucket and the invariant rows behind it. fp on these cells comes from noscip `heuristic` and `ambiguous` edges, so no `exact` fp rise is possible. D8's new `exact` edges are SCIP references, not scored edges. Gold rises are not regressions by themselves (`compare.rs:211` flags only a fall).

## Phases

Rules for every phase: the failing test is written and seen failing first; one file, one edit per message; no `git checkout --`; `cargo build -p rb-cli` before any e2e; no `cargo` while a container suite runs; every bless is followed by a row-by-row `git diff` of the golden; before changing behaviour a test asserts, grep the tests for the old value (lesson of 2026-10-13); probes of a repository that must outlive a suite are named `bench_*`, and `t_*` probes finish before any suite starts (lesson of 2026-10-16).

**P0. Re-base on M9c, and the gated probes.**
- **Files:** `bench/probe/m9d_probe.py` (new, `python3 -I`, read-only: the planner's `gowork_measure`, `gowork_residue`, `gowork_agreement` and `check_citations`, ported unchanged and pointed at M9c's record).
- **Action:**
  1. `du -sh target`, `df -h .`.
  2. Check that every dump of M9c's record exists and is non-empty; re-score it with HEAD's rb-bench.
  3. Re-read `rb-indexer/src/{lib.rs,scip.rs}` at M9c's close: where D8′'s join and its certainty are, the `Package` code for D9, the coverage keys for D10.
  4. Re-run the go.work emulation against M9c's measured `scip:mattermost` cell.
  5. Docker probes, one at a time, logged, through `var/m9d/plan/scripts/proxy_fetch.sh` or its committed equivalent:
     - (a) go.work A/B on server: no go.work and go.work, alternating, same load, `du` of the module cache before and after the workspace fetch;
     - (b) spring_boot_microservices' full fetch and index on a `rb-scip-java:probe-m9d-jdk25` built from the D12 Dockerfile (a `repo.spring.io` refusal is fatal or not; wall; m2 growth);
     - (c) adservice with the D13 init script under each candidate hook (`whenReady`, `beforeTask`);
     - (d) the 4 microservices_demo Python fetches (pip, musl), and scip-python on each.
- **Verify:** the re-score is `cmp`-identical to `m9c.json`. The probe reproduces this contract's numbers on M9b's record (S0 33,312 / 20,640 / 6,141 / 12,672; S2 41,878 / 26,591 / 190 / 15,287; go.work 42,126 / 26,763 / 18 / 15,363; 4,092 / 4,093 interface methods) before it prints M9c's.
- **Done:** amendment 1 re-bases every [M9c] prediction, and settles D12 and D13: each is built, or moved to Out of scope with its measured reason (that phase is then dropped, and its mutants retired).

**P1. `cypher()` without JIT (D11).**
- **Files:** `crates/rb-store/src/lib.rs`, `crates/rb-store/src/db_tests.rs`.
- **Action:** C10's test first (it fails: the plan has a `JIT` key), then D11.
- **Verify:**
  - `cargo test -p rb-store` on a `ScratchDb`;
  - `check_gates` and `a_checkout_is_its_repository_whatever_its_name` alone, then in the full suite of P8, with the load at start and end logged (M9c PI saw both fail at load ~5);
  - the `rb check` A/B log.
- **Mutants:** m387.

**P2. npm fetch (D1).**
- **Files:** `crates/rb-scip/src/orchestrate.rs`.
- **Action:** C2's test first, then D1.
- **Verify:**
  - `cargo test -p rb-scip --lib orchestrate`;
  - a probe: index nestjs_realworld in SCIP mode as `t_m9d_nest` (fetch ok, `scip-typescript ok`), then delete the repo;
  - the TS e2e suites: goldens unchanged.
- **Mutants:** m360–m362.

**P3. Python (D2–D4).**
- **Files:** `crates/rb-scip/src/jobs.rs`, `crates/rb-scip/src/orchestrate.rs`, `crates/rb-indexer/src/projects.rs`, `crates/rb-indexer/src/lib.rs` (the status detail), `crates/rb-cli/src/main.rs`.
- **Action:** C3 and C4 tests first, then D2–D4.
- **Verify:**
  - `cargo test -p rb-scip`, `-p rb-indexer --lib projects` and `lib`, `-p rb-cli --bin rb`;
  - probes: fastapi as `t_m9d_fa` (`ok (environment not installed: fetch failed)`, the Cython error in `stderr_tail`) and microblog as `t_m9d_mb` (`scip-python ok unpinned`), both deleted after;
  - the Python e2e suites: goldens unchanged (both fixture roots have a `pyproject.toml`).
- **Mutants:** m363–m369.

**P4. Go workspace and replace targets (D5–D7).**
- **Files:** `crates/rb-scip/src/jobs.rs`, `crates/rb-scip/src/orchestrate.rs`, `crates/rb-indexer/src/projects.rs`.
- **Action:** C5 and C6 tests first, then D5–D7.
- **Verify:**
  - `cargo test -p rb-scip`, `-p rb-indexer --lib projects`;
  - the Go e2e suites: goldens unchanged (no fixture `go.mod` requires an in-repo module or has a replace);
  - a probe: mattermost in SCIP mode as `bench_m9d_mm`: server `ok`, `sharedchannel-test ok`, server's index has 0 invalid locals; then deleted with `bench/cleanup.sh`.
- **Mutants:** m370–m378.

**P5. Join certainty, in-repo packages, health (D8–D10).**
- **Files:** `crates/rb-indexer/src/lib.rs`, `crates/rb-indexer/src/scip.rs`, `crates/rb-scip/src/decode.rs`, `crates/rb-cli/src/main.rs`; scip-status goldens if any change.
- **Action:** C7, C8, C9 tests first, then D8–D10.
- **Verify:**
  - `cargo test -p rb-scip --lib decode`, `-p rb-indexer --lib scip` and `lib`, `-p rb-cli --bin rb`;
  - the SCIP e2e suites: goldens unchanged (fixtures have 0 invalid locals on the fixed image, M9c PI);
  - re-read the P4 probe's `scip_cross_project`: server → server/public `exact` > 0, `synthetic` 0.
- **Mutants:** m379–m386.

**P6. JDK 25 image (D12), if P0 passed.**
- **Files:** `docker/scip-java/Dockerfile`, `docker/scip-java/README.md`, `crates/rb-scip/src/jobs.rs`, `crates/rb-scip/src/orchestrate.rs`, `crates/rb-cli/src/doctor.rs`.
- **Action:** C11 tests first. Build `rb-scip-java:0.13.1-jdk25` with the digest pinned, log kept. Rebuild-check the default: `docker build` with no build-arg reproduces the old image's layers (`docker image inspect` of the tools: Maven, Gradle, scip-java checksums, unchanged).
- **Verify:**
  - `cargo test -p rb-scip`, the doctor test;
  - the Java e2e suites unchanged;
  - a probe: spring_boot_microservices as `t_m9d_sb`, `scip-java ok`, then deleted;
  - `rb-scip-java:0.13.1` untouched.
- **Mutants:** m388–m390.

**P7. Gradle exec bits (D13), if P0 passed.**
- **Files:** `crates/rb-scip/src/orchestrate.rs`, `crates/rb-scip/src/jobs.rs` (doc fix).
- **Action:** C12 test first, then D13 with P0's hook.
- **Verify:** `cargo test -p rb-scip`; the Gradle e2e suites unchanged; a probe: microservices_demo as `t_m9d_ms`, adservice `ok`, then deleted.
- **Mutants:** m391.

**P8. Close the code.**
- **Files:** `crates/rb-indexer/src/lib.rs` (`PIPELINE`), `bench/mutants/m9d.json`, `README.md`, `docs/usage-and-testing.md`, `docs/repository-brain-design.md`.
- **Action:** `PIPELINE` `m9d-1`. Design notes for D4 (environment fallback), D5/D6 (Go workspaces), D8 (`exact` joins), D11 (JIT). Usage guide for the new status texts and the second Java image.
- **Verify:**
  - `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && echo CLEAN`;
  - the full real-Docker run of usage guide §5.2, log kept: 0 failed, `grep -c 'skipping:'` = 0;
  - `SELECT name, checkout FROM rb.repo` identical before and after;
  - all five batteries KILLED, re-run after the last fmt.
- **Done:** committed on `m9`.

**P9. Bench.** Build `var/m9d/bin/rb` from the committed tree (refuse a dirty `crates/` or `migrations/`; print HEAD and sha256).
- Before the run: `docker image inspect` every image the jobs need (absent → stop).
- `nohup bench/run.sh var/m9d/bin/rb` once cold, then again warm; the warm run is the record (D14).
- Export the changed raw indexes `READ ONLY` for buckets NEWIDX and WORK.
- Invariant C14. `rb-bench sample`, with every drawn edge without a verdict judged (transcripts in `var/bench/labels/exact-m9d/`); the gate fails on any unjudged edge.
- Score twice, `cmp`. Write `bench/scoreboard/m9d.json` and `m9d-perf.json`. Gate C15, C16.

**P10. Records and evaluators.**
- `CHANGELOG.md`, `IMPLEMENT.md`, this Result, `tasks/todo.md` (the `unowned_table_writes` open item closed), `tasks/bench-findings.md` (B4 with measured numbers; the 4,092 correction), `tasks/lessons.md`, the README status line, the usage guide, the design doc's milestone row.
- Evaluator pass 1 (full, hostile), then pass 2 (verification only).

## Mutants (`bench/mutants/m9d.json`, from m360)

Each spec mutates one line that survives a reflow and compiles, and names its `test` filter. A kill counts only from a failing test that matches the filter, in a compiling tree. m387 runs against a `ScratchDb` only (lesson of 2026-10-13).

| Id | Crate | Mutation | `test` filter |
|---|---|---|---|
| m360 | rb-scip | `replace_registry_host` setting dropped | `npm_fetches_rewrite_registry_hosts_and_ignore_engines` |
| m361 | rb-scip | `engine_strict` setting dropped | `npm_fetches_rewrite_registry_hosts_and_ignore_engines` |
| m362 | rb-scip | `RECIPE_TS` left at `m6a-ts-1` | `npm_fetches_rewrite_registry_hosts_and_ignore_engines` |
| m363 | rb-scip | `requirements.txt` directories not roots | `requirements_only_directories_are_python_roots` |
| m364 | rb-scip | project flags passed with a `pyproject.toml` too | `python_roots_without_pyproject_name_their_project` |
| m365 | rb-scip | project flags never passed | `python_roots_without_pyproject_name_their_project` |
| m366 | rb-scip | Poetry run without `1>&2` | `poetry_errors_reach_the_tail` |
| m367 | rb-scip | a failed Python fetch returns `failed` (no fallback) | `a_failed_python_fetch_indexes_without_the_environment` |
| m368 | rb-scip | fallback also after a fetch timeout | `a_failed_python_fetch_indexes_without_the_environment` |
| m369 | rb-scip | fallback for a non-Python job | `a_failed_python_fetch_indexes_without_the_environment` |
| m370 | rb-scip | no `workspace` for a required in-repo module | `go_jobs_requiring_in_repo_modules_get_a_workspace` |
| m371 | rb-scip | a replaced in-repo module also put in `workspace` | `go_jobs_requiring_in_repo_modules_get_a_workspace` |
| m372 | rb-scip | a module outside the root put in `workspace` | `go_jobs_requiring_in_repo_modules_get_a_workspace` |
| m373 | rb-scip | `GOFLAGS=-mod=mod` kept in a workspace job | `workspace_jobs_write_go_work_and_clear_goflags` |
| m374 | rb-scip | `go.work` written only for the index phase | `workspace_jobs_write_go_work_and_clear_goflags` |
| m375 | rb-scip | a workspace job keeps `RECIPE_GO` | `workspace_jobs_write_go_work_and_clear_goflags` |
| m376 | rb-indexer | replace targets not exported | `go_replace_targets_inside_the_repository_are_exported` |
| m377 | rb-indexer | a plain Go job exported repository-shaped | `go_replace_targets_inside_the_repository_are_exported` |
| m378 | rb-indexer | a target outside the repository accepted | `go_replace_targets_outside_are_unsupported` |
| m379 | rb-indexer | equal symbols joined `synthetic` | `byte_equal_cross_project_symbols_join_exact` |
| m380 | rb-indexer | unequal versions joined `exact` | `byte_equal_cross_project_symbols_join_exact` |
| m381 | rb-indexer | `.`-version in-repo reference creates a `Package` | `in_repo_modules_are_not_packages` |
| m382 | rb-indexer | every `.`-version package dropped (third-party too) | `in_repo_modules_are_not_packages` |
| m383 | rb-scip | invalid locals not counted | `scip_go_invalid_locals_are_counted` |
| m384 | rb-scip | counted for any indexer | `scip_go_invalid_locals_are_counted` |
| m385 | rb-scip | non-local symbols counted | `scip_go_invalid_locals_are_counted` |
| m386 | rb-cli | status line omits the count | the D10 status-line row |
| m387 | rb-store | `jit = off` removed from the session | `cypher_runs_without_jit` (ScratchDb) |
| m388 | rb-scip | release above 21 keeps the JDK 21 image | `maven_release_above_21_takes_the_jdk25_image` |
| m389 | rb-scip | `${java.version}` not resolved | `maven_release_above_21_takes_the_jdk25_image` |
| m390 | rb-scip | a Gradle job takes the JDK 25 image | `maven_release_above_21_takes_the_jdk25_image` |
| m391 | rb-scip | init script missing from the fetch phase | `gradle_runs_mark_tool_artifacts_executable` |

No spec's filter may select a wall-clock test (`bench/mutants/timing_tests.txt`). The D12 and D13 images have no Rust mutant beyond these; they are verified by their probes and the e2e suites.

## Verification (closing bar)

- Tests first. Mutants m360–m391 (less those retired by amendment 1) KILLED; the m8b, m9a, m9b and m9c batteries re-run KILLED.
- fmt and clippy clean. Real-Docker suites: 0 failed, 0 skipped, full log kept. `rb.repo` unchanged. No `t_m9d_*` or `bench_m9d_*` repository left; no probe image or container left.
- `PIPELINE` `m9d-1`; the binary of record is built from the last code change (C13).
- Invariant C14: every differing row classified, 0 unclassified, 0 `noscip` rows.
- `compare` against the re-scored M9c record: exit 0, every accepted cell classified, no `exact` fp rise.
- Every changed cell reported measured beside predicted, each newly indexed target with its job status before and after. Two scorings `cmp`-identical.
- Perf per the NFR rule; `rb check`'s A/B; every re-measure logged and disclosed.
- Records complete. Evaluator pass 1, then pass 2 (verification only).

## Out of scope

- **Recorded, not fixed:**
  - kotlin_spring_realworld: scip-java 0.13.1's Maven path indexes javac output only, and its Gradle Kotlin path fails on Kotlin 2 (FIR `AbstractMethodError`). A Kotlin SemanticDB compiler plugin for Maven is not part of scip-java.
  - mattermost `webapp` and `e2e-tests/cypress`: `git+ssh` GitHub dependencies (3 in all) need `codeload.github.com` (and for the ssh fallback, `github.com` and ssh), which the allow-list refuses by design. A host can opt in with `RB_SCIP_REGISTRIES`, unmeasured. webapp's index (4,432 TS files, 2,017 packages, emulated amd64 under 4 GiB) is unmeasured against the budget.
  - An environment fallback for scip-typescript (it would index webapp's own code without `node_modules`): the size above is unmeasured.
  - JavaScript-only packages (microservices_demo `currencyservice`, `paymentservice`; scip-typescript's `--infer-tsconfig`). C# and PHP targets: no indexer.
  - A committed `go.work` at a job root (0 on the bench): it would meet `GOFLAGS=-mod=mod` today.
  - The 4,093 server → server/public interface-method references (`request.CTX#…`, `mlog.LoggerIFace#…`): server/public's scip-go index defines no symbol for interface methods, so D8′ counts them `unanchored`. Why scip-go 0.2.7 omits them is not measured.
  - Cross-project `IMPLEMENTS` (352 relationships; M9c Out of scope), unchanged by go.work.
  - Analysing the AGE label tables at publish (rejected for D11, see trade-offs).
- **M9c's own scope** is unchanged. M9d builds on its D8′, C10, the IMG bucket and its record.

## Amendments

Append each amendment after the previous one's last sentence (lesson of 2026-10-11).

**Planner's note (2026-10-17, before P0; HEAD `15371eb`, M9c P5 in progress).**
- **Probes run.**
  - 2 scip-go runs on mattermost `server` (the first `--network none`, the second with its fetch behind the proxy, because the first showed the cache missing a module the workspace needs).
  - 1 `go list` diagnostic.
  - Offline: scip-python on fastapi and microblog, the protoc exec check, Poetry with stdout kept.
  - Fetches behind the proxy: fastapi, microblog, nestjs (with its index), sharedchannel-test (with its index).
  - scip-java on Temurin 25 over a two-class project.
  - EXPLAIN on `t_m9d_chk`.
- **Safety.**
  - `SELECT * FROM rb.repo ORDER BY 1`: the user's 4 rows (`fx_man`, `fx_date`, `fx_full`, `alvorabackend`) are identical before and after (`var/m9d/plan/logs/repo-before.txt`, `repo-after-At.txt`). The transient `t_m9c_mm`/`t_m9c_mmb` rows at the start belonged to the M9c agent.
  - `t_m9d_chk` and its AGE graph were deleted.
  - The bench's caches were only APFS-cloned, and the clones were deleted.
  - `eclipse-temurin:25-jdk` (pulled for the probe) was removed, along with every `m9d-probe-*` container and network. No existing image was touched.
  - No `rb.scip_index` row was written: no rb index ran with SCIP on.
  - No commit, no `cargo` run, no edit outside this file and `var/m9d/plan/`.

**Amendment 1 (2026-10-18, P0; M9c closed at `8e91884`).** P0's record is `var/m9d/logs/p0/`. The probe is `bench/probe/m9d_probe.py` (`python3 -I`, read-only; output `logs/p0/probe-citations-record.txt`, `probe.txt`). The Docker probe scripts, the D12 and D13 drafts and the content checker are in `var/m9d/p0/{scripts,docker,gradle}/`. Every probe ran in Docker, one at a time, behind an egress proxy built as rb builds it (`scripts/lib.sh`, the planner's `proxy_fetch.sh` config), or under `--network none`. Each probe used APFS clones of the bench caches, and the clones were deleted afterwards.
- **Disk and record.**
  - At the start: `du -sh target` 5.8 GiB, `df -h .` 17 GiB free. `docker system df` fails on this host ("rw layer snapshot not found" for `d9bdd04301c9`, a container that is not rb's), so the Docker VM was read through a throwaway container's `df /`: 166 GiB available. The host's free space is the binding floor. It never fell below 13 GiB, and it was 18 GiB at the end (`disk-start.txt`).
  - M9c's record `var/bench/out/6cd5baaa8d1f/` holds 157 dumps (38 targets, 119 PyCG cases), 0 of them empty, and 11 `.scip.status` files.
  - Re-scored with HEAD's release `rb-bench`, it is `cmp`-identical to `bench/scoreboard/m9c.json` and `m9c-perf.json`; the control against `m9b.json` differs (`rescore.txt`). `cargo build --release -p rb-bench` recompiled the binary (sha256 `e9dea089…`, where M9c's Result names `82839c81…`). The scoreboards are identical, so only the binary's bytes differ.
- **Citations.** The planner's 42 checks hold 37 of 42 at `8e91884`. Five moved: rb-indexer `lib.rs` 145 → 157 (`PIPELINE` `m9c-3`), 1500 → 1647 (coverage), 1530 → 1677 (`lombok_removed`), and `scip.rs` 336 → 376 (`own_packages`), 553 → 621 (`references_external`). The body now cites both lines. The updated list adds the contract's 5 unchecked citations and M9c's sites below: 74 of 74 hold, and the +7 control fails 74 of 74.
- **M9c's close, re-read** (`8e91884`).
  - **D8′'s join is `join_across_projects`** (`lib.rs:1531`), called after the job loop (`lib.rs:1182`) on the references each scip-go job hands back (`scip.rs:623`, through `hands_back`, `scip.rs:827`, keyed by `unversioned`, `scip.rs:837`). Its certainty is fixed in one place: `.chain(cross_edges…map(|e| (e, Certainty::Synthetic)))` (`lib.rs:1234`). The counts are at `lib.rs:1579` (`synthetic`) and `:1587` (`unanchored`), and the diagnostic at `:1601`. D8 changes those three sites: the edge carries its certainty out of the join, `exact` when `r.symbol == *dst`.
  - **D9's `Package` code** is the per-document "Packages" loop (`scip.rs:651–679`): every occurrence of a symbol the index does not define, unless it is in `own_packages` (`scip.rs:664`). Those become `PackageUse`s, which `lib.rs:1185` turns into `Package` nodes and `scip` `DEPENDS_ON`. A handed-back reference also passes through this loop, so D9 goes there, with the job's `modules` set (already passed to `join_handing_back`, `scip.rs:217`).
  - **D10's coverage keys** are the always-written block (`lib.rs:1647`, which since M9c includes `references_unanchored`, `:1657`) and the non-zero block (`lib.rs:1677`), where `locals_invalid_type` and `locals` go.
- **B4 at M9c's close.** M9c's record has the same 34 jobs as M9b's, with the same 8 failed jobs (same status and phase) and the same absent Python roots. sharedchannel-test was re-run on the fixed image (row `f420b550…`, 2026-10-17 08:13, `failed`/`fetch`, 718 ms) and failed with the same `reading /server/public/go.mod` error (`sharedchannel-row.txt`, read `READ ONLY`).
- **go.work emulation re-run** (`probe.txt`).
  - **Inputs.** The 13 scip-go indexes M9c's record joined (`var/m9c/p7/idx`, exported `READ ONLY` from `rb.scip_index` under the fixed image, `record-go-rows.txt`) are each IDENTICAL, as multisets, to M9c P0b's `idx-new`. The emulation's inputs are therefore the record's.
  - **On M9b's record** it reproduces every planned number: S0 33,312 / 20,640 / 6,141 / 12,672; S2 41,878 / 26,591 / 190 / 15,287; go.work 42,126 / 26,763 / 18 / 15,363; 4,092 and 4,093 interface-method references (133 and 134 symbols); 1,541 and 0 invalid locals.
  - **The cache-only index is gone.** The planner's cache-only index (`mattermost-server-gowork.scip`) no longer exists, so its column (13,234) is not re-measured.

  | `scip:mattermost:noscip:calls` (gold, tp, fp, fn) | value | step |
  |---|---|---|
  | A: M9b's record, emulated fixed image + D8′ (planned S2) | 41,878 / 26,591 / 190 / 15,287 | |
  | X1: A's gold, M9c's noscip edges | 41,878 / 40,397 / 202 / 1,481 | A → X1: tp +13,806, fp +12: M9c's Tier B (B2, B3, B5) |
  | X2: M9c's gold, M9b's noscip edges | 43,771 / 27,762 / 191 / 16,009 | A → X2: gold +1,893: rb's join against the emulation, IMG, D3-doc, conversions now `REFERENCES` |
  | B: M9c's record, as scored (equals `m9c.json`'s cell) | 43,771 / 42,181 / 203 / 1,590 | |
  | **C: M9c's record + the go.work server index** | **44,024 / 42,366 / 18 / 1,658** | **B → C, go.work alone: gold +253, tp +185, fp −185, fn +68** |

  - **Why the go.work delta differs from the plan's.** The plan's go.work delta was gold +248, tp +172, fp −172, fn +76 on M9b's edges. On M9c's record, M9c's Tier B already finds more of the newly joined pairs, so 13 more of them are tp and fewer are fn. The 185 fp that go.work removes are exactly M9c's 185 version-skew calls. The 18 left are the `docs/site` and e2e-tests calls (M9c P8 `fp-split.txt`).
  - Under go.work the 10,097 D8′ pairs leave the gold as cross-job joins and re-enter as own-symbol pairs (byte-equal), plus 253 new ones.
- **D8, D9, D10 bases** (`probe.txt`, sections `d8`, `measure`).
  - **D8.** Each of M9c's 23,180 `synthetic` scip edges (`CALLS` 10,098, `REFERENCES` 13,082) has, under go.work, a reference at its evidence span whose symbol is byte-equal to its target: 23,180 of 23,180. M9c's `scip_cross_project` row is server → server/public: `synthetic` 44,269, `ambiguous` 0, `unanchored` 16,366.
  - **D9.** M9c's `scip` dump holds 30 `DEPENDS_ON` into `go/github.com/mattermost/mattermost/server/public@v0.4.3`, as M9b's did.
  - **D10.** Locals typed `invalid type`: 1,541 of 202,391 in server, 0 in each of the 12 other record indexes, and 0 of 202,391 under go.work.
- **Probe (a): go.work A/B on server** (`a-gowork.txt`, `a-content.txt`; `rb-scip-go:0.2.7-xtools0.50.0`, cpus 2, memory 4g).
  - **Module cache** (clone of `bench_mattermost_scip/gomod`): 1,195,156 KiB, 41,416 files, 274 module directories.
    - Fetch A, rb today (`GOFLAGS=-mod=mod go mod download`): 1 s, no growth.
    - Fetch B (`go work init . ./public`, `GOFLAGS` empty, `go mod download`): rc 0, 150 s, through `proxy.golang.org` and `storage.googleapis.com` only, 0 refused. The cache grew to **2,971,120 KiB (+1.69 GiB)**, 114,215 files, 580 module directories. In workspace mode `go mod download` fetches the whole workspace module graph.
  - **Index, ABBA, `--network none`.** Loads 2.58–6.70 over the four runs.

    | Run | Wall | CPU (cgroup) | `memory.peak` |
    |---|---|---|---|
    | A1 (no go.work) | 101 s | 182.3 s | 3,804,770,304 B |
    | B1 (go.work) | 146 s | 272.3 s | 3,863,781,376 B |
    | B2 (go.work) | 145 s | 271.0 s | 3,859,456,000 B |
    | A2 (no go.work) | 99 s | 179.5 s | 3,875,983,360 B |

    go.work costs 1.46× wall and 1.50× CPU. `memory.events` shows 0 `max`, 0 `oom`, 0 `oom_kill` in all four runs.
  - **Content.** The A index is IDENTICAL to the record's server index, and the B index to the planner's go.work index (0 invalid locals).
  - **(a2), added because of the growth** (`a2-golist.txt`, `a2-content.txt`). A narrower workspace fetch, `go list -e -deps -test ./... ./public/...` under the same `go.work`, took 11 s and grew the cache by **54,160 KiB** (1,029 files, 1 module directory).
    - The offline index that followed found 0 package errors (`go list -e`) and was **IDENTICAL** to B's (scip-go rc 0, 143 s).
    - Its `memory.peak` read 4,294,967,296 B, which equals the 4 GiB limit; this run did not read `memory.events`.
- **Probe (b): spring_boot_microservices on JDK 25** (`b-build.txt`, `b-base-tools.txt`, `b-build2.txt`, `b-spring.txt`, `b-spring2.txt`, `b-spring3.txt`, `b-control.txt`, `b-content.txt`, `b-scip-java-index.log`).
  - **Draft 1 fails.** The D12 draft as planned (`ARG BASE` on `docker/scip-java/Dockerfile`, `BASE=eclipse-temurin:25-jdk@sha256:2d5d5e9b…`, the index digest, Ubuntu 26.04, 25.0.4.1) does not build: that base has neither `curl` nor `wget` (exit 127 at the Maven download).
  - **Draft 2 builds** (`Dockerfile.jdk25`): it copies `/opt/maven`, `/opt/gradle` and `/usr/local/bin/scip-java` from `rb-scip-java:0.13.1` onto the JDK 25 base. It built in 7 s to 1,091,022,554 B, and `mvn`, `scip-java` and the Gradle launcher have the same sha256 as in 0.13.1. `rb-scip-java:0.13.1` was not rebuilt; its ID is still `f9612a94…`.
  - **Fetch**, rb's Maven recipe behind the proxy, m2 cloned from the bench (393,428 KiB): `go-offline` rc 0 in 49 s and 48 s, `clean verify -DskipTests` rc 0 in 17 s. **`repo.spring.io:443` was refused twice and is not fatal.** m2 grew by 67 MiB (to 460,384 and 461,856 KiB).
  - **Index without a targetroot: no index.** Under `--network none` the build passes (`BUILD SUCCESS`, 111 files compiled with `release 25`, 12 s), but scip-java writes no `index.scip`, so rb would record `(no index.scip was produced)`.
    - **Cause.** scip-java's Maven targetroot defaults to `target/scip-targetroot` under the source root. The root pom is an aggregator that is not its modules' parent (they inherit `spring-boot-starter-parent`), so the reactor builds it last, and its `clean` deletes `/sources/target`, the targetroot included, before scip-java collects it.
    - **Control.** A two-class project with `release 25` indexes (1,718 B) with maven-compiler-plugin at the default, 3.13.0, 3.14.0 and 3.15.0, so neither the JDK nor the plugin version is the cause.
  - **With `--targetroot /scip-targetroot`** (outside `/sources`), the same index phase is rc 0 in 13 s, writes 1,002,030 B with 111 documents for 111 tracked `.java` files, and peaks at 716 MB, with 0 OOM.
- **Probe (c): adservice, D13's hook** (`c-gradle.txt`, `c-gradle2.txt`, `c-gradle3.txt`, `c-content.txt`). Recipe: `rb-scip-java:0.13.1`, rb's Gradle recipe, the init script mounted read-only and passed as `--init-script` to both fetch runs and to the Gradle arguments of the index.
  - **Script.** It resolves the project's `protobufToolsLocator_*` configurations and sets 0755 with `Files.setPosixFilePermissions` on each `*.exe` under `GRADLE_USER_HOME`, without asking `canExecute()`.
  - **Controls without a script** fail warm (bench Gradle home cloned, both tools 0644) and cold (empty home) at `:generateProto` with `error: 13 (Permission denied)`.

  | Scenario | Fetch 1 | Fetch 2 | Index | Gradle home growth |
  |---|---|---|---|---|
  | `whenReady`, warm | 5 s | 9 s | rc 0, 10 s, 4,239,341 B | +6 MiB |
  | `whenReady`, cold | 23 s | 19 s | rc 0, 11 s, 4,239,341 B | 59,300 KiB |
  | `beforeTask`, warm | 5 s | 10 s | rc 0, 11 s, 4,239,341 B | +6 MiB |
  | `beforeTask`, cold | 21 s | 32 s | rc 0, 11 s, 4,239,341 B | 61,364 KiB |

  - **Hook behaviour.** In both phases, `whenReady` fires once per build and `beforeTask` once per `GenerateProtoTask` (twice, main and test). Both tools end 0755.
  - **Index content.** 12 documents: the 2 tracked `.java` files and 10 generated under `build/generated/`, with 0 invalid locals.
  - **Harness fault.** `c-gradle.txt`'s two `beforeTask` scenarios first failed in the harness: Docker Desktop refused to mount a probe directory deleted and recreated at once (`error while creating mount source path`), so no proxy ran. They were re-run with a fresh path per scenario (`c-gradle2.txt`). rb names each run's directories by instance (`orchestrate.rs:365`) and is not exposed to this.
- **Probe (d): the 4 microservices_demo Python roots** (`d-python.txt`, `d-python2.txt`, `d-content.txt`). Recipe: D2's recipe on the pinned scip-python image (linux/amd64, Alpine), a fresh pip cache per root; the index in a new container, `--project-name <segment> --project-version .`.

  | Root | `.py` | Pins | Fetch | Index (as rb) |
  |---|---|---|---|---|
  | emailservice | 5 | 35 | rc 0, 28 s, 14 MB | rc 0, 4 s, 141,910 B |
  | loadgenerator | 1 | 39 | **rc 1**, 22 s (psutil: no `gcc`), 20 MB | rc 0, 2 s, 8,842 B |
  | recommendationservice | 5 | 31 | rc 0, 14 s, 14 MB | rc 0, 4 s, 149,308 B |
  | shoppingassistantservice | 1 | 79 | **rc 1**, 26 s (numpy's meson build), 32 MB | rc 0, 3 s, 12,844 B |

  - **Planner's `--hash` note.** shoppingassistantservice has 0 `--hash` lines (a `uv pip compile` file with `# via` comments), so that note is withdrawn. The first run of 3 roots hit the same Docker Desktop mount fault and was re-run (`d-python2.txt`).
  - **New finding: rb's pip environment never reaches the index.** The pip fetch installs into the fetch container's own site-packages, and only `/root/.cache/pip` persists.
    - The rb-shaped indexes name only the project and `python-stdlib` (2 packages).
    - The control, which fetches and indexes in one container, names 14 (emailservice) and 12 (recommendationservice) packages (`grpcio`, `protobuf`, `opentelemetry-*`, …).
    - For the 2 roots whose fetch failed, rb-shaped and control indexes are identical.
    - So a Requirements job's index already equals D4's fallback index. This holds since M6d, for `Requirements` and `None` alike (M6d's record: "scip-python emits no third-party `py/` rows"). Poetry's virtualenv lives under the mounted `/root/.cache/pypoetry`; whether scip-python finds it is unmeasured.
- **Verdicts.**
  - **D12: built.** Two changes:
    - **The image** is `docker/scip-java/Dockerfile.jdk25` (draft 2: `ARG BASE` defaulting to the measured `eclipse-temurin:25-jdk@sha256:2d5d5e9b…`, tools copied from `rb-scip-java:0.13.1`). `docker/scip-java/Dockerfile` is not edited, so the JDK 21 image, its ID and every cached Java row are untouched. P6's "rebuild-check the default" becomes: `rb-scip-java:0.13.1`'s ID unchanged, and the three tool checksums equal across both images.
    - **D12b (new).** Every Maven index phase passes `--targetroot /scip-targetroot`, a container path outside `/sources`. `RECIPE_JAVA` `m5-egress-1` → `m9d-java-1`, so every Maven job re-runs once. Predicted: every other Maven index (e2e fixtures) byte-identical, because the targetroot is not in the index. The spring index gains 111 documents.
    - The mechanism does not depend on the JDK, so D12b applies to JDK 21 jobs too.
    - P6 keeps m388–m390 and adds **m393** (`--targetroot` dropped from the Maven index arguments; filter `maven_index_phase_writes_the_targetroot_outside_the_sources`, orchestrate) with that unit (C11).
  - **D13: built**, with **`whenReady`**: it fires once, before any task of the build, in both phases, warm and cold. `beforeTask` works too, but it runs once per proto task. m391 stays.
  - No phase is dropped and no mutant is retired.
- **D5 amended: the workspace fetch is `go list -deps -test` over `.` and each `use` directory (`./... ./public/...`), not `go mod download`.** Measured: the same index byte for byte, 11 s against 150 s, and +53 MiB of module cache against +1.69 GiB.
  - The probe ran `go list` with `-e`; rb runs it without, so a missing module fails the fetch, as `go mod download` does. P4's server probe checks that this exits 0.
  - C5's test asserts the argument list. **m374** keeps its meaning. **m394** (new): a workspace job's fetch is `go mod download`. Filter `workspace_jobs_write_go_work_and_clear_goflags`.
- **Re-based [M9c] predictions** (planned → on M9c's record).
  - **D14's base:** `var/m9d/m9c-rescored.json` (and `-perf.json`), `cmp`-identical to `m9c.json`.
  - **`scip:mattermost:noscip:calls`.**
    - Planned: relative to the emulated S2, gold +248, tp +172, fp 190 → 18, fn +76.
    - Now: gold **43,771 → 44,024** (+253), tp **42,181 → 42,366** (+185), fp **203 → 18**, fn **1,590 → 1,658** (+68), plus sharedchannel-test's own pairs (NEWIDX).
    - Gate: fn rises, so one accept line (WORK); fp falls and needs none.
  - **D8.**
    - Planned: about 8,800 pairs.
    - Now: all **23,180** of M9c's `synthetic` scip edges become `exact` (`CALLS` 10,098, `REFERENCES` 13,082). `scip_cross_project` server → server/public: `synthetic` 44,269 → 0, `exact` ≥ 44,269, `unanchored` measured at P9 (16,366 at M9c, of which the 4,093 interface-method references stay).
  - **`rb status` mattermost `exact calls (scip)`:** 34,647 → **≥ 44,745** (+10,098 from D8, plus `CALLS` go.work types anew).
  - **`exact` sample.**
    - Planned: draws move.
    - Now: the `scip` family's mattermost stratum grows by about 23,180 edges, so nearly all 60 strides move. Budget for re-judging up to 60 edges.
  - **D9:** the 30 `DEPENDS_ON` into `…server/public@v0.4.3` and their `Package` node leave (WORK).
  - **HEALTH bucket:** empty, as planned: 0 invalid locals in every index under go.work (measured on the probe index).
  - **Perf NFRs:** unchanged (base: the pass-2 run, N6).
- **Re-based budgets.**
  - **mattermost `server`:**
    - Fetch: 11 s with D5's `go list` (150 s with `go mod download`).
    - Index: 145 s against 100 s without go.work at the same loads.
    - `memory.peak`: 3.86 GB, with 0 OOM over 2 runs, and once at the limit (a2, `memory.events` not read).
    - Cache: +53 MiB.
  - **spring_boot_microservices:** fetch 48 s + 17 s, index 13 s, m2 +67 MiB, image 1.09 GB in Docker.
  - **adservice:** fetch about 14 s warm and 42–53 s cold, index 10–11 s, Gradle home +6 MiB.
  - **The 4 Python roots:** fetch 14–28 s (2 ok, 2 failed → D4 fallback), index 2–4 s, pip 14–32 MB each.
  - P0's measured cache additions sum to about 0.2 GiB (53 + 67 + 6 MiB, plus 80 MB of pip caches), within the planned 0.5 GiB. The JDK 25 image adds 1.09 GB in Docker's disk, below the planned 1.3 GiB. The 8 GiB floor holds.
- **New risks.**
  1. **The go.work fetch would have cost 1.69 GiB** of module cache per run of server (D5 amended above). Any later workspace job meets the same choice.
  2. **server under go.work peaks at 3.86–4.29 GB of a 4 GiB cgroup.** `memory.peak` includes page cache, and 0 OOM was read where events were read. The NFR line "memory.peak < 4 GiB with 0 `oom_kill`" becomes "0 `oom_kill` and 0 `oom`, with `memory.peak` reported", and P4's probe logs `memory.events`. An OOM there is a stop: the budget is a design question, not a P4 fix.
  3. **pip environments never reach scip-python's index phase** (probe (d)). D2's fetch only proves installability and warms the pip cache. The fix (install to a mounted `--target` with `PYTHONPATH`) changes every Python index and is Out of scope for M9d.
  4. **D12's release reader sees only in-repo properties.** spring_boot is caught through the root pom's `maven.compiler.release=${java.version}`, while its modules set only `java.version` and get the release from `spring-boot-starter-parent`, outside the job. A repository without such a root property stays on JDK 21 and fails as before. This is recorded, not chased.
  5. **D13's script knows the protobuf plugin's locator configurations only.** Other plugins' native tools are not covered.
  6. **This host's `docker system df` fails** on a foreign container's missing snapshot. Disk checks read the host `df` and a throwaway container's `df /`.
- **Added item: e2e SCIP caches leak** (`logs/p0/cache-leak.txt`).
  - **Root cause, verified.** Go's module cache is read-only (directories 0555, files 0444). `Cleanup::drop` (`crates/rb-cli/tests/common/mod.rs:197`) calls `std::fs::remove_dir_all` on `var/scip/cache/<repo>`, which fails with EACCES on such a tree, and discards the error (`let _ =`). Reproduced with `std::fs::remove_dir_all` on a Go-shaped tree: `Err(PermissionDenied)`, tree still present.
  - **Second path.** The start-up clean-up (`remove_leftover_test_repositories`, `mod.rs:53`) deletes the rows of a killed run's `t_*` repositories but never their caches.
  - **Paths that do not fail.** rb itself deletes no package cache: the only removals on SCIP paths are the per-run export and egress directories (`orchestrate.rs:461–462`, `:991`), which hold no module cache. `bench/cleanup.sh` deletes rows and graphs only; the `bench_*_scip` caches persist by design (4.2 GiB now). `e2e.rs:4428` unwraps its removal of an npm cache, which is writable.
  - Hence the 65 `t_*` directories (7.9 GiB).
  - **Fix, in P1 (new D15, claim C18), so that every later phase's e2e runs stop leaking:**
    - `rb_scip::remove_cache_tree(path)` in `crates/rb-scip/src/orchestrate.rs` gives each directory owner write permission, then removes the tree.
    - `Cleanup::drop` uses it and prints any error with the path.
    - The start-up clean-up also removes `var/scip/cache/<name>` for each leftover `t_*` repository.
    - Test first: unit `cache_trees_with_read_only_modules_are_removed` (rb-scip), a 0555 directory holding a 0444 file, plus a plain tree. It fails on plain `remove_dir_all`.
    - **m392**: the permission pass dropped. Filter `cache_trees_with_read_only_modules_are_removed`. P1's files become `crates/rb-store/src/lib.rs`, `db_tests.rs`, `crates/rb-scip/src/orchestrate.rs` and `crates/rb-cli/tests/common/mod.rs`.
    - Verify: after P1's e2e runs, `ls var/scip/cache | grep -c '^t_'` is 0.
- **Mutant ids.** m360–m394: m392 (D15), m393 (D12b), m394 (D5's fetch). None retired. The closing bar reads "m360–m394".
- **Safety.**
  - `SELECT * FROM rb.repo ORDER BY 1` is identical before and after (`repo-before.txt`, `repo-after.txt`, 4 rows), and no `t_m9d_*` or `bench_*` repository was created.
  - No rb index ran, so no `rb.scip_index` row was written.
  - Images: the probe image `rb-scip-java:probe-m9d-jdk25` and the base it pulled (`eclipse-temurin@sha256:2d5d5e9b…`, absent before) were removed. 0 `m9d` containers and 0 `m9d` networks remain. No other image was touched (`cleanup.txt`).
  - No change in `crates/` or `docker/*/Dockerfile`; the bench caches were only cloned. Python probes read downloaded sources only inside containers.

**P1 note (2026-10-18; D11, D15).** Logs in `var/m9d/logs/p1/`. Binaries: `var/m9d/bin/rb.pre-p1` (debug, HEAD `c2ca543`, sha256 `04f5cc48…`) and `rb.p1` (debug, the P1 tree, `16248677…`).
- **C10.** `cypher_runs_without_jit` failed first on the final assertion (`c10-red.txt`: 138 functions, JIT total 247 ms under `cypher()`'s settings), with its row and its control passing. It passed after D11.
- **`rb check` A/B** (`check-ab-ambient.txt`, `check-ab-loaded.txt`; `scripts/check_ab.py`, ABAB, 5 runs per binary, same output from both).

  | | Predicted | Measured |
  |---|---|---|
  | probe graph | 44 nodes, 52 edges (M9b binary) | 43 nodes, 51 edges (this binary) |
  | plan cost | 1,044,383 | 1,044,446 |
  | execution, jit off (variant C) | 1.6 ms | 1.2 ms (load 2.6), 0.9 ms (load 4.4) |
  | JIT, jit on | 0.27–0.37 s at load 2.4–3.2 | 0.40 s (load 2.6), 0.32 s (load 4.4) |
  | `rb check --rule unowned_table_writes`, median | ≤ 100 ms at load ≥ 4 (NFR) | 464 → **53 ms** at load 4.4–5.0 (4 busy loops); 349 → 40 ms at 2.5 |
  | `rb check`, all three rules, median | | 469 → 64 ms at 4.4–5.0; 357 → 52 ms at 2.5 |

  The pre-change walls are below M9b's 2.06–2.47 s at load 4.1–6.3; one ambient run of the old binary took 2,261 ms.
- **Timing tests alone** (`e2e-alone.txt`). `check_gates` and `a_checkout_is_its_repository_whatever_its_name` passed at load 2.8–2.9. `check_gates` passed again at load 4.0–4.3 (`e2e-startup-removes-leak.txt`).
- **D15 deviates from the amendment in one point: the start-up clean-up removes every `t_*` directory under `var/scip/cache`, not only those of leftover `t_*` repositories.** A failed `Cleanup` deletes the repository's rows first, so its cache is left with no row to find it by. The control shows it (`e2e-scip-go-cache-control.txt`): `scip_go_chi_sqlx` run with HEAD's harness left `t_scipgo_6009` (17 directories 0555, 86 files 0444) and no `t_` row. The next run's start-up, with P1's harness, removed it (`e2e-startup-removes-leak.txt`: "1 package caches", then 0). The `t_` prefix belongs to tests, as the start-up clean-up of rows already assumes.
- **D15 tests.** The unit `cache_trees_with_read_only_modules_are_removed` failed first with `PermissionDenied` (`d15-unit-red.txt`). The harness test `e2e_clean_ups_remove_read_only_test_caches` (in `e2e.rs`, a fifth P1 file; no database or Docker) failed first at `Cleanup`'s half (`d15-e2e-red.txt`). Its start-up half failed under a temporary mutation of the `t_` filter (`d15-e2e-startup-control.txt`, file restored byte-identical). With P1's harness, `scip_go_chi_sqlx` leaves 0 `t_` caches (`e2e-scip-go-cache.txt`).
- **rb's own removals, re-checked.** The Go module cache is mounted only from `<cache>/gomod` (`orchestrate.rs:838`). The per-run work and egress directories hold no package cache, and `age.rs:73` removes the AGE CSV directory. No rb code path removes a package cache.
- **Mutants** (`mutants-p1.txt`): m387 KILLED (ScratchDb; `cypher_session` is called only by `cypher()`'s read-only transaction), m392 KILLED.
- `rb.repo` identical before and after (`repo-before.txt`, `repo-after.txt`); `t_m9d_chk` was deleted at the end of each A/B run.

**P2 note (2026-10-18; D1).** Logs in `var/m9d/logs/p2/`. Binary `var/m9d/bin/rb.p2` (debug, the P2 tree, sha256 `0501d5cf…`).
- **D1 applies to npm fetches only** (`package-lock.json` and no lockfile), not to every scip-typescript fetch as D1's first sentence reads. `base.env` is shared by the yarn and pnpm fetches and by the index phase, and C2 requires yarn and pnpm runs unchanged. The two settings are appended to the npm fetch's environment. Yarn and pnpm runs and every index phase keep today's environment, and the test asserts it.
- **C2.** `npm_fetches_rewrite_registry_hosts_and_ignore_engines` failed first on `RECIPE_TS` (`m6a-ts-1`; `c2-red.txt`). After the bump, it failed on the missing setting. One test fix came before D1 was written: its yarn branch had compared the whole fetch environment, but `run_job` appends the egress proxy's variables to every fetch, so it now checks each setting and today's leading entries.
- **Old values pinned by tests**, grepped and updated:
  - `a_workspace_job_fetches_at_its_root_and_indexes_every_member`: `RECIPE_TS` `m6a-ts-1` → `m9d-ts-1`.
  - `default_budget_argv_is_cfa8ab9s`: the npm fetch line gains `-e npm_config_replace_registry_host=always -e npm_config_engine_strict=false` after `npm_config_update_notifier`, and nothing else moves (`argv-diff.txt`, with a control that must differ).
  - No golden holds a recipe.
- **Probe `t_m9d_nest`** (`nest-probe.txt`, `nest-row.txt`, `nest-cleanup.txt`; bench budget `--scip-timeout-secs 3600`, cold npm cache).

  | | Predicted | Measured |
  |---|---|---|
  | status | failed → ok | `scip-typescript ok` (row `e276e6ea…`, `ok`) |
  | fetch + index | 150 s + 2 s | 363.6 s together (load 2.3 at the start, 8.3 at the end) |
  | index | about 275 KB | 274,729 B |
  | `exact calls (scip)` | at most 30 call sites | 30 |
  | npm cache | 93 MB | 97 MB |

  - Refused hosts are not observable here: an `ok` row keeps no tail.
  - The repository, its graph and its cache (after `chmod -R u+w`) were deleted. The two egress networks the probe created were removed with `rb gc --prefix t_m9d_ --egress`.
- **TS e2e** (`e2e-ts.txt`): `scip_ts_express_prisma`, `scip_ts_workspaces`, `scip_ts_pnpm_yarn`, `scip_polyglot_monorepo` and `scip_contract_packs` passed, 5 of 5, in 1,614 s at load 5.3–8.8.
  - Goldens are unchanged: no bless, and no golden file is modified.
  - Since the bump, 10 `ok` rows and 1 `failed` row were written. The failed row is the deliberate `@acme/shared` 404, with its tail unchanged.
  - 0 `t_` caches after the run.
- **Rows invalidated by the recipe bump** (`ts-rows-invalidated.txt`, read `READ ONLY`): every scip-typescript row written before P2 is keyed on `m6a-ts-1`, 94 in all (89 `ok` with 113 MB of indexes, 5 `failed`). Every TypeScript job (npm, yarn and pnpm alike) re-runs once on its next index:
  - the bench's TS jobs (bookcars ×9, express, mattermost's TS jobs, nestjs);
  - the e2e fixtures;
  - the user's `alvorabackend` TS job, whose cached `EBADENGINE` failure will no longer replay (its outcome under D1 is unmeasured; the repository was not touched).

  The old rows stay in the table, unreachable.
- **Mutants** (`mutants-p2.txt`): m360, m361 and m362 KILLED. m387 and m392 were re-run in the same battery: KILLED.
- `rb.repo` identical to P1's baseline (`repo-after.txt`). No `rb-egress` container or `rb-scip` network is left.

**P3 note (2026-10-18; D2–D4).** Logs in `var/m9d/logs/p3/`, scripts in `var/m9d/p3/scripts/`. Binary `var/m9d/bin/rb.p3` (debug, the P3 tree, sha256 `d4c9ecc3…`).
- **C3, C4 red first.** Seven tests failed on their assertions before any code changed (`c3-c4-scip-red.txt`, `c4-indexer-red.txt`, `c4-cli-red.txt`), each for the reason it targets:
  - `requirements_only_directories_are_python_roots`: 2 jobs where 5 were expected.
  - `python_roots_without_pyproject_name_their_project`: index args `scip-python index` without the project flags.
  - `poetry_errors_reach_the_tail`: the bare `poetry install …` argv.
  - `a_failed_python_fetch_indexes_without_the_environment` and `python_fallback_rows_round_trip`: `("failed", Some(Fetch))` where `("ok", Some(Fetch))` was expected.
  - `scip_status_records_an_index_without_its_environment` (lib): no `phase` key.
  - `scip_status_line_prints_an_index_without_its_environment` (main): `ok (fetch phase)`.
- **The red tests compile against HEAD's API.** They read the fallback through `as_str()`, `phase()` and `RunStatus::from_cached("ok", Some("fetch"))`, not through the new variant, so the red runs are assertion failures, not build errors.
- **Design.**
  - **D4 is a `RunStatus` variant, `OkWithoutEnvironment`.** It is stored as `ok` with phase `fetch`, and `RunStatus::is_ok()` covers both `ok` variants.
  - **Every site that matched `RunStatus::Ok` now calls `is_ok()`.** These are the decode in `projects.rs`, and the scip-go module list and the coverage join in `lib.rs`. `grep -rn 'RunStatus::Ok\b' crates` finds, outside tests, only `RunStatus`'s own methods, `finished()` and D4's match on the index outcome.
  - **`from_cached`.** `ok`/none gives `Ok`, and `ok`/`fetch` gives the fallback. `ok`/`index`, which no binary writes, is a miss.
  - **D2's `pyproject.toml` test reads the export.** It checks `<export>/<root>/pyproject.toml` (the export is repository-shaped), so no `Job` field is added and the input hash already covers the file.
- **Deviations, each with its reason.**
  1. **`rb index` prints the elapsed time between the status and D4's suffix:** `scip: . scip-python ok 151.1s (environment not installed: fetch failed)`. That is where `(fetch phase)` goes today. `rb status` prints D4's text exactly.
  2. **The `scip_status` diagnostic records `stderr_tail` for the fallback** as well as `phase: fetch`. FALLBACK's "(status suffix, phase)" did not name the tail, but an `ok` row with a phase and no reason would not show why the environment is missing. A plain `ok` row still records no tail, because its tail is the index phase's progress text.
  3. **D4 does not say what happens when the index fails after a failed fetch.** In that case the job keeps the fetch's failure (`failed`/`fetch`, the fetch's tail), as before D4. An index timeout or a docker-client failure in that index phase is returned as such and is not cached. The test asserts the `failed` case.
  4. **D2's "with `PythonLock::Requirements`" is applied literally.** A requirements-only root stays `Requirements` even beside a `poetry.lock`, because Poetry needs a `pyproject.toml`. The unit has a row for it.
  5. **`pyproject.toml` test reflowed for its mutants.** rustfmt split the `pyproject.toml` test across lines, so it was rewritten as `let pyproject = …; if !pyproject.is_file()`, keeping m364 and m365 on one line.
  6. **`bench/probe/m9d_probe.py:84` is a P0 record and was not edited.** It cites `RECIPE_PY` at `m6d-py-4`, and it reads `git show HEAD:` as P0 ran it (P2 had already moved its lines).
- **Old values pinned by tests**, grepped (`m6d-py`, `RECIPE_PY`, `scip-python index`, `poetry install`, `(fetch phase)`) and updated:
  - `a_workspace_job_fetches_at_its_root_and_indexes_every_member`: `RECIPE_PY` `m6d-py-4` → `m9d-py-1`.
  - `python_jobs_fetch_lockfile_driven_environments_then_index_offline`: the index line now ends `scip-python index --project-name svc --project-version .`, because its `files()` export holds no `svc/pyproject.toml`. Its Poetry check (`contains("poetry install --no-root --no-interaction")`) still holds inside the `sh -c` text.
  - No golden holds a recipe or a Python argv, and no other test was changed.
- **Probes** (`fa-probe.txt`, `mb-probe.txt`; `probe.sh` as P2's: the bench checkout, `--force --scip-timeout-secs 3600`, cold caches; load 6–8).

  | | Predicted | Measured |
  |---|---|---|
  | fastapi `t_m9d_fa` status | `ok (environment not installed: fetch failed)` | `rb status`: `scip: . scip-python ok (environment not installed: fetch failed), definitions matched 338/338 …` |
  | fastapi row | `ok`, phase `fetch`, the Cython error in `stderr_tail` | `ok`/`fetch` (`80db828a…`), 2,458-character tail ending in `AttributeError: 'build_ext' object has no attribute 'cython_sources'` and Poetry's `ChefBuildError` note on pyyaml 6.0, which reaches the tail through D3 |
  | fastapi time | fetch 9–13 s (fails) + index 11 s | 151.1 s together (emulated amd64, load 6–7) |
  | fastapi index | 808 KB (planning's environment-less probe) | 807,852 B, the same byte count |
  | fastapi `exact calls (scip)` | at most 335 call sites | 314 |
  | fastapi caches | pip and Poetry 35 MB | 35 MB |
  | microblog `t_m9d_mb` status | `scip-python ok unpinned` | `scip: . scip-python ok unpinned, definitions matched 194/194 …`; row `ok`, no phase (pip fetch rc 0) |
  | microblog time | fetch 43 s + index about 10 s | 209.0 s together |
  | microblog index | 263 KB with `--project-name microblog` | 253,630 B with `--project-name root` (D2 names the repository root `root`; not decomposed further) |
  | microblog `exact calls (scip)` | at most 89 call sites | 74 |
  | microblog cache | pip 28 MB | 28 MB |

  Clean-up (`probe-cleanup.txt`): both repositories, their graphs (`rb_t_m9d_fa_43188`, `rb_t_m9d_mb_43189`) and their caches (`chmod -R u+w`, then `rm -rf`) were deleted. The two egress networks were removed with `rb gc --prefix t_m9d_ --egress`. After it: 0 rows, 0 graphs, 0 caches, 0 containers, 0 networks.
- **Rows invalidated by the recipe bump** (`py-rows-invalidated.txt`, read `READ ONLY`). 19 scip-python rows predate P3: 18 `ok` (459 kB of indexes) and 1 `failed`/`fetch` (the bench's fastapi row). All are keyed on `m6d-py-4`, so every Python job re-runs once. Since P3 began: 1 `ok`/`fetch` (the fastapi probe) and 3 `ok` (microblog, and the e2e fixture's 2 roots).
- **Python e2e** (`e2e-py.txt`; `scip_python_fastapi_sqlalchemy` is the only `fn scip_py…`): passed, 1 of 1, in 91 s.
  - Goldens are unchanged: no bless, no fixture file modified, and both fixture roots have a `pyproject.toml`.
  - Start-up removed 0 leftovers, and 0 `t_` caches were left after.
- **Unit suites.**
  - fmt and clippy (`--workspace --all-targets -D warnings`) clean (`clippy.txt`, `fmt-final.txt`).
  - `cargo test -p rb-scip`: 88 passed (`rb-scip-tests.txt`).
  - `-p rb-indexer --lib projects`: 16 passed.
  - `-p rb-cli --bin rb`: 47 passed.
  - `-p rb-indexer --lib`: 376 passed and 3 failed at load 8.2 (`rb-indexer-lib-tests.txt`). The 3 were the wall-clock tests `resolve::…stay_fast`, `stitch::…within_a_bound` and `di::many_beans_of_one_type_are_too_ambiguous`, all listed in `bench/mutants/timing_tests.txt` and in files P3 does not touch. Each passed alone at load 7.0–5.7 (`rb-indexer-timing-alone.txt`).
- **Mutants** (`mutants-p3.txt`): m363–m369 KILLED. The whole m9d battery (`mutants-m9d-battery.txt`; m387 on a `ScratchDb`) is 12 of 12 KILLED, and the tree was restored.
- **Process.**
  - Three times, two `Edit` calls to one file went out in one message: orchestrate.rs twice and lib.rs once, on different hunks.
  - Each pair was re-read or grepped and found landed before anything ran. The lesson of 2026-10-17 already covers this, so no new rule is added.
- **`rb.repo`** is identical before, after the probes, and at the end (`repo-before.txt`, `repo-after-probes.txt`, `repo-after.txt`; 4 rows).

**Amendment 2 (2026-10-18, before P4's code; new D5b, the workspace fallback).**
- **Why.** server under go.work peaks at 3.86–4.29 GB of its 4 GiB cgroup (amendment 1, risk 2), and amendment 1 calls an OOM there "a stop". But an OOM, or any other failure the workspace causes, must never cost the user the server index that M9c already produces without go.work.
- **D5b (workspace fallback).** A D5 job's workspace run (`RECIPE_GO_WORK`, through D5) can fail in either of two ways:
  - its fetch or index phase exits non-zero, for example with exit code 137 (an OOM kill);
  - it writes no index (`(no index.scip was produced)`).

  In either case `run_job` runs the job again exactly as a plain `RECIPE_GO` job runs: a fresh export with no `go.work`, `GOFLAGS=-mod=mod`, `go mod download`, the root-relative layout.
  - **Exceptions, as D4's:** a timeout, or a docker client failure (exit 125–127, or a client that does not start), in the workspace run is returned as such and is not re-run.
  - **The re-run writes an index:** the job is `ok`, and its status line appends `(workspace failed: indexed without go.work)`. The index is the re-run's. The stored tail is the workspace run's, which says why the workspace failed. The elapsed time covers both runs. The row is cached under the job's own key (`RECIPE_GO_WORK`), so it replays until the job's inputs or recipe change.
  - **The re-run fails as well:** its own outcome (`failed`, `timeout` or `environment`) is the job's result.
- **Representation.** `RunStatus` gains `OkWithoutWorkspace`, stored as `status = 'ok', phase = 'index'`.
  - **Why `index`.** D4's `ok`/`fetch` says that the fetch's product, the environment, is missing. D5b's `ok`/`index` says that the workspace's index is missing and the plain index stands in. The phase CHECK (`migrations/0005_scip_index_phase.sql`) allows only `fetch` and `index`, and no binary has written `ok`/`index` (P3 made it a miss). So no existing row changes meaning, and `from_cached` maps each pair to exactly one variant: `ok`/none to `Ok`, `ok`/`fetch` to D4, `ok`/`index` to D5b. Which phase of the workspace run failed is in the stored tail.
  - **Rejected: storing the workspace run's failed phase.** A failed workspace fetch would then be stored as `ok`/`fetch`, which `from_cached` cannot tell from D4 without the indexer.
  - **The suffix text lives with the variant.** `RunStatus::note()` gives D4's and D5b's suffix texts. `rb index` prints it after the status. `rb status` prints it for the variant `from_cached` gives the diagnostic's `status` and `phase`, so the stored pair, the variant and the line round-trip in one place.
  - **Visibility, as D4's.** `rb index` and `rb status` print the suffix. The `scip_status` diagnostic records `phase: index` and the workspace run's `stderr_tail`. This adds `crates/rb-indexer/src/lib.rs` and `crates/rb-cli/src/main.rs` to P4's files.
- **D6 jobs.** The fallback does not apply to a job with a replace target outside its root (D6), whether or not it also has a workspace. A plain run cannot resolve a relative replace: sharedchannel-test's plain fetch fails on `reading /server/public/go.mod` (B4 row 5), so the re-run could only fail again.
- **Claim C5b (new).**
  - Unit `a_failed_workspace_run_reindexes_without_go_work` (orchestrate) covers three things:
    - the rule: a failed workspace fetch, an index exiting 137 and a missing index are each re-run plain, the re-run seeing no `go.work`, `GOFLAGS=-mod=mod` and `go mod download`, giving `ok`/`index` with the workspace run's tail; a re-run that fails gives its own failure; a D6 job and a plain job are not re-run;
    - the exceptions: a workspace timeout or docker client failure is not re-run;
    - the status-suffix round trip: `from_cached("ok", Some("index"))` gives the variant, whose `note()` is the suffix.
  - The `scip_status` diagnostic (lib) and the status line (main) each get a row.
- **Risk 2 amended.** An OOM in P4's server probe is still reported plainly, and it still fails the claim that server indexes under go.work. With D5b it no longer costs the index, and the probe shows whether D5b's plain index stood in.
- **Mutants.** **m395**: no re-run after a workspace failure. **m396**: a re-run after a workspace timeout too. Both use the filter `a_failed_workspace_run_reindexes_without_go_work`. The closing bar reads "m360–m396".

**P4 note (2026-10-18; D5–D7, D5b).** Logs in `var/m9d/logs/p4/`, scripts in `var/m9d/p4/scripts/`. Binary `var/m9d/bin/rb.p4` (debug, the P4 tree, sha256 `3fe4c23b…`).
- **Red first.** Before any behaviour changed, the `Job` fields (`workspace`, `replace_targets`, empty everywhere), `RECIPE_GO_WORK` and a `note()` holding D4's text alone were added, so that the tests compiled. Seven tests then failed, each on its assertion:
  - `go_jobs_requiring_in_repo_modules_get_a_workspace`: no `work[…]` and no `replace[…]` in the plan (`c5-c5b-scip-red.txt`).
  - `workspace_jobs_write_go_work_and_clear_goflags`: `go.work` seen as `[None, None]`.
  - `a_failed_workspace_run_reindexes_without_go_work`: `("failed", Some(Fetch))` where `("ok", Some(Index))` was expected.
  - `go_replace_targets_inside_the_repository_are_exported`: `link_targets` 0 where 1 was expected (`c6-indexer-red.txt`).
  - `go_replace_targets_outside_are_unsupported`: `None` where the reason was expected.
  - `scip_status_records_an_index_without_its_workspace` (lib): `expect("D5b's cached pair")`, because `from_cached("ok", Some("index"))` was a miss.
  - `scip_status_line_prints_an_index_without_its_workspace` (main): D4's text, `ok (environment not installed: fetch failed)` (`d5b-cli-red.txt`).
- **Design.**
  - **Planner (D5, D7).** A Go root's `go.mod` is read through the planner's reader. Its workspace holds the directories under the root of the other `go.mod` files whose module path the root requires, `// indirect` included, with no `replace` of that path. A root that holds its own `go.work` gets no workspace.
  - **Replace targets (D6).** These are `replace` targets that are local directories by Go's rule (`.`, `..`, `./`, `../`, `/`) and do not resolve lexically to the root or below it. They are kept as written. `projects::job_files` resolves them with the link resolver: one inside the repository exports its tracked files, and one leaving it gives `replace target outside the repository`.
  - **Runs.** `go_runs` writes `<root>/go.work` into the export before either phase runs. Both phases then run with `GOFLAGS` empty and `GOWORK` set to that file, and the fetch is `go list -deps -test ./... ./<dir>/...`. A job with replace targets starts in its root of a repository-shaped export. A job with neither keeps today's argv, layout and `RECIPE_GO`; `go_job_runs_download_then_scip_go` is unchanged and passes.
  - **D5b.** `run_job` wraps `run_once`.
- **Deviations, each with its reason.**
  1. **`lib.rs` and `main.rs` are P4 files** (amendment 2). `rb status` and `rb index` print D4's and D5b's text through `RunStatus::note()`. `main.rs` lost its `WITHOUT_ENVIRONMENT` constant, and D4's lines are unchanged (its tests pass as they were).
  2. **`GOWORK` is set in both phases of a workspace job.** D5 does not name it. With it, each phase's workspace is part of its argv, and it gives m374 a one-line mutation: the export, and with it the `go.work`, is shared by both phases, so "written only for the index phase" is mutated as the fetch running with `GOWORK=off`, which has the same effect.
  3. **Vendored and ignored directories are no workspace modules.** D5 did not say this. `bench_go_plan.py`, a check of every bench repository's `go.mod` files that is independent of the Rust code, found that go_gin_example commits `vendor/` with six vendored `go.mod` files, all of which its root requires. The first planner would have given that root a six-module workspace, against D5's "nothing else" on the bench.
     - A row for `vendor`, `testdata`, `.x` and `_x` failed first (`c5-vendor-red.txt`), and then the rule was added.
     - Re-run, the check reaches only mattermost `server` (`public`) and `tools/sharedchannel-test` (`../../server/public`) (`bench-go-plan.txt`).
  4. **An absolute replace target is unsupported, like one leaving the repository.** D6 speaks only of relative paths. An absolute directory on the indexing host is never in the export, so such a fetch fails today.
  5. **Replace targets count as `link_targets`.** sharedchannel-test's line reads `scip-go ok, 1 link targets`, so its `scip_status` row gains that key (bucket NEWIDX). The key already means "directories outside the root whose files the job exported".
  6. **The `go` line is omitted when no module declares one.** A module path declared by two `go.mod` files under one root is not de-duplicated: Go would refuse that `go.work`, and D5b's re-run would stand in.
  7. **D5b's elapsed time covers both runs**, as D4's does.
- **Old values pinned by tests**, grepped (`RECIPE_GO`, `m6d-go-1`, `go mod download`, `GOFLAGS`, `environment not installed`, `Some("index")`) and updated:
  - `a_failed_python_fetch_indexes_without_the_environment`: `from_cached("ok", Some("index"))` was `None` (a miss). It is now asserted to differ from D4's variant (amendment 2).
  - No golden holds a Go recipe, a Go argv or `GOFLAGS`, and no other test changed.
- **Unit suites.**
  - fmt and clippy (`--workspace --all-targets -D warnings`) are clean (`fmt.txt`, `clippy.txt`).
  - `cargo test -p rb-scip`: 91 passed.
  - `-p rb-indexer --lib projects`: 18 passed. `-p rb-indexer --lib`: 382 passed, at load 2.9–3.2.
  - `-p rb-cli --bin rb`: 48 passed.
- **Go e2e** (`e2e-go.txt`): `scip_go_chi_sqlx` (the only `fn scip_go…`) and `scip_polyglot_monorepo` (its `services/billing` Go job) passed, 2 of 2, in 182.7 s at load 4.1–4.5.
  - Goldens are unchanged: no bless and no fixture modified. Neither fixture `go.mod` requires an in-repo module or has a replace.
  - 0 `t_` caches before and after.
- **Probe `bench_m9d_mm`** (`mm-probe.txt`, `mm-memwatch.txt`, `mm-server-phases.txt`, `mm-measure.txt`, `mm-same-index.txt`).
  - **Setup.** `probe.sh` runs as `bench/run.sh`'s scip mode does: `--force --scip-timeout-secs 3600`, at mattermost `b2ce71db`. The package caches started as an APFS clone of `bench_mattermost_scip`, the bench's warm caches. `memwatch.zsh` read each scip-go container's `memory.peak` and `memory.events` from inside it once a second, as P0's scripts read them.

  | | Predicted (amendment 1) | Measured |
  |---|---|---|
  | server status | `ok` under the workspace | `scip-go ok`, no D5b suffix; row `480e3ee4…` `ok`, no phase |
  | server fetch | `go list`, rc 0, 11 s | `go list -deps -test ./... ./public/...` with `GOWORK=/sources/go.work`, `GOFLAGS` empty; sampled from 21:34:27 to 21:34:41 (about 15 s), rc 0 (else D5b would have run), peak 450 MB |
  | server index | 145 s (P0, against 100 s without go.work) | sampled from 21:34:43 to 21:37:40, row at 21:37:43 (about 180 s) |
  | server wall, job (fetch + index) | | 196.9 s against M9c's server row 119.0 s (`ad9e1f9a…`, `m9c-server-row.txt`): 1.65× at load 5.2–7.8, not an A/B |
  | server `memory.peak` | 3.86 GB, 0 OOM | **3,921,051,648 B**, last sample 3 s before the row |
  | server `memory.events` | 0 `oom`, 0 `oom_kill` | `low 0 high 0 max 0 oom 0 oom_kill 0 oom_group_kill 0` in every sample |
  | server index content | 0 invalid locals | 177,066,985 B, 1,830 documents, **0 of 202,391** locals `invalid type` (control: M9c's record index, 1,541), 391,694 server/public references at version `.` (control: 384,650 at `v0.4.3`); **IDENTICAL** in occurrences and symbols to P0's go.work index (control: M9c's differs) |
  | module cache | +53 MiB | +54,160 KiB (1,195,156 → 1,249,316 KiB), P0 a2's figure |
  | sharedchannel-test | `ok`, 16 s, 194 KB, 0 of 242 invalid | `scip-go ok, 1 link targets` in 18.5 s, 194,524 B, 0 of 242 invalid, 295 server/public references at `.`, peak 1.06 GB |
  | other Go jobs | cache hits | 6 `ok (cached)` (unchanged recipe and inputs) |

  - **D5b did not fire.** server indexed under the workspace with 0 OOM events, so no plain re-run was needed or made.
  - **Harness fault.** The `docker events` stream (`mm-docker-events.txt`) is empty: its `image=` filter named the tag, while rb runs containers by image ID. The memory evidence is memwatch's. The exit status follows from the `ok` row.
  - **Observed, not P4's (D1, P2).** webapp's and cypress's fetch tails now read `git … ls-remote ssh://git@registry.npmjs.org/…`. `replace-registry-host=always` also rewrote the `git+ssh` GitHub host. They stayed failed, as D1 predicted, but the tail is not "the git-dependency refusal" as worded. This is recorded for P9's RECIPE bucket and P10.
  - **Clean-up** (`mm-cleanup.txt`).
    - `bench/cleanup.sh 'bench\_m9d\_mm'` left 0 rows and 0 graphs.
    - `rb gc --prefix bench_m9d_ --egress` removed `rb-scip-fetch` and `rb-scip-egress`, which this phase's runs created (0 before P4). 0 networks and 0 `rb-egress` or `rb-scip` containers are left.
    - The probe's package cache `var/scip/cache/bench_m9d_mm` (2.1 GB by `du`, an APFS clone plus the 53 MiB of growth) is kept, because P4's deletion rules do not cover it.
- **Mutants** (`mutants-p4.txt`, `mutants-p4-m372.txt`, `mutants-m9d-battery.txt`).
  - m370–m378, m394, m395 and m396 are KILLED.
  - **m372** was INVALID first: its mutation `*dir != root` did not compile. It was re-specified as `is_under(dir, "")`, which puts every module outside the root in, and is KILLED.
  - The whole m9d battery is 24 of 24 KILLED, with m387 on a `ScratchDb`. No scratch database is left.
- **Process.** One message carried three `Edit` calls to `orchestrate.rs` (the test `Fake`'s fields, its constructor and its `run`). All three were grepped and found landed before anything compiled. Multi-hunk changes otherwise went in as one scripted edit per file and message, each asserting that every hunk matched once.
- **`rb.repo`** is identical before and after (`repo-before.txt`, `repo-after.txt`; 4 rows).

**P5 note (2026-10-18; D8–D10).** Logs in `var/m9d/logs/p5/`, scripts in `var/m9d/p5/scripts/`. Binary `var/m9d/bin/rb.p5` (debug, the P5 tree, sha256 `b05d9881…`).
- **Red first.** The two count fields (`ScipIndex::{locals, locals_invalid_type}`, `Coverage::{locals_invalid_type, locals}`) were added at 0 first, so that every test compiled against today's behaviour. Five tests then failed, each on its assertion:
  - `scip_go_invalid_locals_are_counted` (decode): `(0, 0)` where `(1, 3)` was expected (`c9-decode-red.txt`).
  - `byte_equal_cross_project_symbols_join_exact` (lib): all three joins `Synthetic`, where the two byte-equal ones were expected `Exact` (`c7-c8-c9-indexer-red.txt`).
  - `in_repo_modules_are_not_packages` (scip): a `go/…server/public@` (empty version) `PackageUse` first in the list. The hand-back control before it passed.
  - `invalid_locals_are_written_only_when_non_zero` (lib): coverage `(0, 0)` where `(1, 2)` was expected.
  - `scip_status_line_prints_invalid_locals` (main): the line without the suffix (`c9-cli-red.txt`).
- **Design.**
  - **D8.** `join_across_projects` returns each edge with its certainty: `exact` when `r.symbol == *dst`, else `synthetic`. The call site chains these edges as they come. `scip_cross_project` gains `exact`. `exact calls (scip)` needed no change: its query counts `scip` `CALLS` of certainty `exact`.
  - **D9.** In the per-document packages loop, a package whose manager is `go`, whose version is empty and whose name is in the job's `modules` (the other ok scip-go jobs' module paths, the set D8′ already hands back to) is skipped. Its namespace occurrences are skipped too, so the import does not bring the node back.
  - **D10.** The decoder counts, for tool name `scip-go`, each `SymbolInformation` whose symbol starts with `local `, and those whose `signature_documentation.text` contains `invalid type`. It counts over the documents as written, before duplicates merge, as P0's `invalid_locals` does. The join copies both counts into the coverage. `scip_status` writes both keys only when `locals_invalid_type > 0`. `rb status` appends `, N of M locals typed invalid` at the end of the line.
- **Deviations, each with its reason.**
  1. **`bench/mutants/run.py` is changed, a file outside P5's list.** rb-cli has no library target, and the runner always ran `cargo test -p <crate> --lib <filter>`, so m386 could not run. A spec may now name `"bin": "rb"`, which runs `--bin rb` in place of `--lib`. Every other spec runs as before.
  2. **The probe adds a control run.** mattermost was indexed as `bench_m9d_mm` twice, one run after the other: first with `rb.p4` (no D8, D9 or D10), then with `rb.p5`. Both snapshots were measured, so every P5 number below has a same-commit, same-index base, and every zero has a control that is not zero.
  3. **`measure.sql` first counted `Package` nodes under the wrong kind** (`'Package'`; the stored kind is `package`), so `mm-probe.txt`'s package-node rows read 0 for both runs. They were re-measured read-only on both kept snapshots (`mm-packages.txt`), and the script was then corrected. The DEPENDS_ON rows in `mm-probe.txt` were right.
  4. **A throwaway test** decoded two real server indexes with the P5 decoder, as a control for D10's zeros at scale (`d10-real-index-control.txt`). It was added `#[ignore]` and removed at once. `decode.rs`'s diff is the same before and after.
  5. **m380's mutation** compares the two symbols with the version dropped (`scip::unversioned(…) == scip::unversioned(…)`), which is the shortcut "unequal versions joined `exact`" names, rather than a constant `true`.
  6. **The decode unit counts 1 of 3**: a third local, without signature documentation, sits in a second document, so the count is shown to run over every document. The status-line row prints `1 of 2`, as C9 words it.
  7. **C7's test also asserts D9 at the graph.** Only `go/…/server/public@v0.4.3` is a `Package` node. This tests the wiring from the join to `Package` nodes, which C8's unit (on `join_handing_back`) does not reach.
  8. **The records still say "synthetic only".** `docs/repository-brain-design.md:275` describes the join as `synthetic`, never `exact`, with `{to, synthetic, ambiguous, unanchored}`, and `docs/usage-and-testing.md:236` describes the diagnostic. Both are P8's files (its D8 design note), so they are not edited here.
  9. **`rb.p5`'s sha256 differs from a later rebuild of the same tree** (`d94dab90…`, `rb.p5-rebuild.txt`). Between the two builds, the only edits under `crates/` were the throwaway test (`#[cfg(test)]`, outside the binary) and its removal; the mutant runs restored every file by sha256. That this is debug-build relink nondeterminism is not verified. P9's binary of record is built from the commit.
- **Old values pinned by tests**, grepped (`scip_cross_project`, `"synthetic"`, `locals_invalid_type`, `locals typed invalid`, `go/…@` package keys; `old-values-grep.txt`) and updated:
  - `external_references_join_another_projects_definitions`: its three `scip_cross_project` rows gain `"exact": 0`. Every join there is at `v0.4.3`, so every certainty stays `synthetic`.
  - Two exhaustive `Coverage` literals in scip.rs tests gain the two new fields at 0, a change of shape, not of value.
  - No golden holds `scip_cross_project` or a D10 key. Both Go fixtures have a single scip-go job, so `modules` is empty and D9 cannot move a fixture package.
- **Unit suites** (`units-final.txt`, after the last edit): `-p rb-scip --lib decode` 18 passed; `-p rb-scip --lib` 92; `-p rb-indexer --lib scip` 73; `-p rb-indexer --lib` 385; `-p rb-cli --bin rb` 49. fmt and clippy (`--workspace --all-targets -D warnings`) are clean (`fmt-final.txt`, `clippy-final.txt`).
- **SCIP e2e** (`e2e-scip.txt`, `scripts/e2e_scip.sh`): every `fn scip_*` test, 15 of 15, passed in 654 s at load 4.7–5.8, single-threaded.
  - Goldens unchanged: no bless, and no file under `crates/rb-cli/tests` or `fixtures/` is modified.
  - 0 `t_` caches before and after.
- **Probe `bench_m9d_mm`** (`mm-probe.txt`, `mm-packages.txt`, `scripts/probe.sh`, `scripts/measure.sql`). mattermost `b2ce71db`, over P4's kept package cache, run as P4 ran it (`--force --scip-timeout-secs 3600`).
  - All 13 jobs were cache hits in both runs: 0 `rb.scip_index` rows were written and the module cache did not grow. Walls were 88.5 s (`rb.p4`) and 89.4 s (`rb.p5`).

  | | Predicted | Control (`rb.p4`, snapshot 43265) | Measured (`rb.p5`, snapshot 43266) |
  |---|---|---|---|
  | `scip_cross_project` server → server/public | `exact` > 0 (≥ 44,269), `synthetic` 0 | `synthetic` 45,478, `unanchored` 16,558 | **`exact` 45,478, `synthetic` 0**, `ambiguous` 0, `unanchored` 16,558 |
  | `scip_cross_project` sharedchannel-test → server/public | (NEWIDX) | `synthetic` 90, `unanchored` 54 | `exact` 90, `synthetic` 0, `unanchored` 54 |
  | `scip` edges `synthetic` (`CALLS` / `REFERENCES`) | amendment 1: 23,180 at M9c (10,098 / 13,082) become `exact` | 10,384 / 13,489 | **0 / 0** |
  | `exact calls (scip)` | M9c 34,647 → ≥ 44,745 | 34,709 | **45,093** (+10,384, the control's `synthetic` `CALLS`) |
  | `scip` `REFERENCES` `exact` | | 18,784 | 32,273 (+13,489) |
  | `scip` DEPENDS_ON into `…server/public@*` | the 30 into `@v0.4.3` leave (M9c) | 31 into `@?` (30 server services, as at M9c, and sharedchannel-test) | **0** |
  | `scip` DEPENDS_ON, all | | 249 | 218 (−31; else identical: 31 rows only in the control, 0 only in P5) |
  | `package` nodes | the `server/public` node leaves | 457, of which `go/…@?`: `server/public`, `vmihailenco/msgpack/v5` | 456: `server/public@?` gone; `msgpack/v5@?` (third-party, no version) stays, as D9 means |
  | graph | | 58,475 nodes, 199,587 edges | 58,474 nodes, 199,556 edges |
  | D10 status lines | no suffix (every index 0) | — | 0 lines with `locals typed invalid`; neither key in any of the 8 scip-go jobs' coverage |

  - **D10 control at scale** (`d10-real-index-control.txt`, the P5 decoder, release build): M9c's record server index (`ad9e1f9a…`) reads 1,541 of 202,391 locals typed invalid, and P4's go.work index (`480e3ee4…`, the row the probe served from the cache) reads 0 of 202,391. Both equal P0's and P4's Python counts.
  - **Clean-up** (`mm-cleanup.txt`): `bench/cleanup.sh 'bench\_m9d\_mm'` left 0 rows and 0 graphs. `rb gc --prefix bench_m9d_ --egress` found nothing: 0 networks before, after and at the end, and 0 `rb-egress`/`rb-scip` containers. The package cache `var/scip/cache/bench_m9d_mm` is kept for P9 or the close to decide.
- **Mutants** (`mutants-p5.txt`, `mutants-m9d-battery.txt`): m379–m386 KILLED. The whole m9d battery is 32 of 32 KILLED, with m387 on a `ScratchDb` (`RB_TEST_DATABASE_URL` set). No `t_store_db_*` database is left, and the tree was restored.
- **Process.** Each message carried one `Edit` call, or one scripted edit of one file that asserted every hunk matched once (scip.rs three times, decode.rs, lib.rs and the mutant specs once each).
- **`rb.repo`** is identical before and after (`repo-before.txt`, `repo-after.txt`; the user's 4 rows, the same as P4's baseline).

**P6 note (2026-10-18; D12, D12b).** Logs in `var/m9d/logs/p6/`, scripts in `var/m9d/p6/scripts/`. Binary `var/m9d/bin/rb.p6` (debug, the P6 tree, sha256 `d7a06221…`).
- **Red first.** Before any behaviour changed, `Job::java_release` (`None` everywhere), `SCIP_JAVA25_IMAGE` and `Images::java25` (looked up, used by nothing) were added, so that the tests compiled. Three tests then failed, each on its assertion:
  - `maven_release_above_21_takes_the_jdk25_image` (jobs): `["scip-java  maven"]` where `["scip-java  maven release[25]"]` was expected (`c11-jobs-red.txt`).
  - `maven_index_phase_writes_the_targetroot_outside_the_sources` (orchestrate): `--targetroot` `None` where `Some("/scip-targetroot")` was expected (`c11-targetroot-red.txt`). Its first two runs failed on setup (the work and cache directories did not exist), so the test creates them; only the third run is the red evidence.
  - `doctor::tests::rows_follow_their_facts`: `IMAGES` without `rb-scip-java:0.13.1-jdk25` (`c11-doctor-red.txt`).
- **Design.**
  - **Planner (D12).** For a Maven root, `jobs()` reads every `pom.xml` at or under the root through its reader. Each pom's release is the highest of `maven.compiler.release`, `source` and `target` and the `maven-compiler-plugin` configuration's `release`, `source` and `target` (its own and its executions'). These are read over the pom and its parent chain inside the job as Maven inherits them: a child's property over its ancestors', every ancestor's plugin configuration. A whole `${name}` value is resolved through those properties, bounded at 8 steps. `1.8` is release 8. The job's `java_release` is the highest over its poms. A Gradle root never gets one.
  - **Parent chain.** `<parent>`'s `relativePath` (default `../pom.xml`; a directory gets `pom.xml`; empty means outside), resolved from the pom's directory and accepted only when that pom's `artifactId` is the one `<parent>` names, as Maven checks before it looks in the repositories. Profile properties are not read.
  - **Image choice.** `jobs::jdk(job)` returns JDK 25 above 21, JDK 21 otherwise, and `Err(release)` above 25. `image_for` and `Images::for_job` follow it. Above 25 the job is `unsupported` (`the poms declare Java release N; the newest scip-java image runs JDK 25`), and so it is when the JDK 25 image is absent (its lookup reason).
  - **D12b.** `java_runs` passes `--targetroot /scip-targetroot` before `--` in the index phase, for both images. `RECIPE_JAVA` is `m9d-java-1`. The Gradle runs are unchanged.
  - **Doctor.** `rb doctor` checks 6 images, both scip-java ones among them.
- **Image** (`image-build.txt`, `scripts/build_jdk25.sh`).
  - `docker/scip-java/Dockerfile.jdk25` is P0's draft 2 with production comments: `ARG BASE=eclipse-temurin:25-jdk@sha256:2d5d5e9b…`, with Maven, Gradle and scip-java copied from `rb-scip-java:0.13.1`. `docker/scip-java/Dockerfile` is not edited.
  - `rb-scip-java:0.13.1-jdk25` built in 6 s: ID `sha256:9e1396344c970a2a814629e9654586a7cb042027f19add4d30a9a3aa563ff676`, 1,091,022,554 B, arm64 (P0's draft built to the same size). It runs Temurin 25.0.4.1, Maven 3.9.16 and Gradle 8.14.6.
  - **Base.** The pinned Temurin 25 base was pulled again, by digest, by BuildKit: P0 had removed it. No `eclipse-temurin` tag was created.
  - **Tools.** `mvn` (`f9381d0c…`), `scip-java` (`a694cae1…`) and `gradle-launcher-8.14.6.jar` (`7f8e1ade…`) have the same sha256 in both images, and so do digests over every file of `/opt/maven` (96 files) and `/opt/gradle` (316): `TOOLS IDENTICAL`. The control, each image's own `javac`, differs.
  - `rb-scip-java:0.13.1` is `sha256:f9612a94…` before and after. No other image was touched.
- **Deviations, each with its reason.**
  1. **Three files outside P6's list, each forced by a change in it.**
     - `crates/rb-scip/src/hash.rs`: `the_java_key_is_pinned_across_the_recipe_split` pins `RECIPE_JAVA` and the key of a fixed input. Both moved with D12b: `c1fd98fd…` → `4e2b943d…`, the new value as printed by the failing test (`hash-key-new.txt`).
     - `crates/rb-indexer/src/projects.rs`: two test `Job` literals gain `java_release: None` (shape only). The reader's comment now names the poms.
     - `crates/rb-cli/src/main.rs`: its test's two `Images` literals gain `java25` (shape only). Also, `tool_versions` records `scip_java25` (the JDK 25 image's ID, or `absent`), with test rows. M6d S1's rule is that a rebuilt image is never served as "up to date", and the JDK 25 image is one more image. An absent image already forces a rebuild through `unsupported` (amendment 44). Every SCIP-on snapshot's `tool_versions` gains the key, so each repository indexes once more on its next run, as P8's `PIPELINE` bump does anyway.
  2. **The compiler plugin's `source` and `target` are read too.** D12 names `maven.compiler.release`, `source` or `target`, and the plugin's `<release>`. The plugin's `<source>`/`<target>` set the same javac flags as those properties, so they are read on the same footing. Another plugin's `<release>` (a test row: `maven-enforcer-plugin`'s 99) is not.
  3. **The parent check uses `artifactId`.** D12 says "the parent chain inside the job". Without the check, a module whose parent is outside (a `spring-boot-starter-parent`) but which omits `<relativePath/>` would inherit an aggregator's properties at `../pom.xml`, which Maven does not do. The test has that row (`svc`).
  4. **Disk.** After the image build, free space was 9.8 GiB, under the 10 GiB floor, so `target/` was cleaned (`cargo clean`: 21.0 GiB; `cargo-clean.txt`) and rebuilt.
  5. **D12b's "byte-identical" prediction holds for content, not for bytes** (`java-rows-after-e2e.txt`, `java-rows-compare.txt`, `java-rows-control.txt`, `scripts/java_rows_compare.py`).
     - The e2e run wrote 6 `ok` rows on the JDK 21 image. 3 are byte-equal to an older row of the same document set. 2 are not, although documents, occurrences and symbols are equal, as multisets, to every older row of their set. The last is the Kotlin Gradle fixture's, whose older row it replaced.
     - **Control.** The two older 83,173 B rows (`b40600aa…`, 2026-10-11; `e2fdcebe…`, 2026-10-17) already differ from each other byte-wise. scip-java's output order was not stable from run to run before P6.
  6. **Probe harness faults.**
     - `probe_sb.sh`'s export mixed psql's `BEGIN`/`COMMIT` into the base64 stream, and the first re-export used `COPY`, which escapes `encode()`'s line breaks. The index was re-exported with a plain `SELECT` in a read-only session (`sb-content.txt`).
     - `cleanup.sh` first ran `rb gc` without `RB_DATABASE_URL`, so the default port was refused. The gc was re-run with it set, and the script was fixed (`sb-cleanup.txt`).
  7. **P6's "rebuild-check the default"** is amendment 1's check: `rb-scip-java:0.13.1`'s ID is unchanged, and the tools are equal across both images.
- **Old values pinned by tests**, grepped (`m5-egress-1`, `build-tool Maven`, `pinned images present`, `c1fd98fd`; `old-values-grep.txt`) and updated:
  - `a_workspace_job_fetches_at_its_root_and_indexes_every_member`: `RECIPE_JAVA` `m5-egress-1` → `m9d-java-1`.
  - `maven_threads_change_the_java_runs_and_their_recipe_only`: `m5-egress-1+T4` → `m9d-java-1+T4`.
  - `default_budget_argv_is_cfa8ab9s`: the Maven index line gains `--targetroot /scip-targetroot` before `--`, and nothing else moves.
  - `maven_jobs_fetch_online_then_index_offline_in_the_local_image`: its index argv gains the same two arguments.
  - `the_java_key_is_pinned_across_the_recipe_split` (hash.rs): the recipe and the key, as above.
  - `doctor::tests::rows_follow_their_facts`: a new row for `6 pinned images present`; no test pinned `5`.
  - No golden holds a recipe, a Maven argv or the doctor's image count. `bench/probe/m9d_probe.py:158` is a P0 record and was not edited.
- **Unit suites** (`units.txt`, `rb-indexer-lib.txt`, after the last code edit): `cargo test -p rb-scip` 94 passed; `-p rb-cli --bin rb` 49; `-p rb-indexer --lib projects` 18; `-p rb-indexer --lib` 385. fmt and clippy (`--workspace --all-targets -D warnings`) are clean (`fmt.txt`, `clippy.txt`).
- **Probe `t_m9d_sb`** (`sb-probe.txt`, `sb-content.txt`, `sb-cleanup.txt`; `scripts/probe_sb.sh`). spring_boot_microservices `5899a530`, `--scip-timeout-secs 3600`, with the package cache an APFS clone of `bench_spring_boot_microservices_scip`, as P0 probe (b) started from it.

  | | Predicted (amendment 1) | Measured |
  |---|---|---|
  | status | `scip-java ok` | `scip: . scip-java ok 36.2s`; row `7b3662b1…` `ok`, no phase, image `sha256:9e1396344c97…` (JDK 25) |
  | documents | 111 (for 111 tracked `.java` files) | **111**, 8,023 occurrences (P0: 111 and 8,023); by module order-service 51, catalog-service 19, notification-service 19, bookstore-webapp 19, api-gateway 3, as P0 |
  | index | 1,002,030 B | 1,002,030 B |
  | time | fetch 48 s + 17 s, index 13 s | 36.2 s for the job (fetch and index together), 38.5 s for `rb index`, at load 4.1–6.2 |
  | m2 | +67 MiB | 393,428 → 460,436 KiB (+65.4 MiB) |
  | `exact calls (scip)` | | 179 (`definitions matched 373/373`) |
  | `repo.spring.io` refusal | not fatal | not observable: an `ok` row keeps no proxy refusals |

  Clean-up: the repository, its graph (`rb_t_m9d_sb_43267`) and its cache (450 MB, `chmod -R u+w`, then `rm -rf`) were deleted. The two egress networks were removed with `rb gc --prefix t_m9d_ --egress`. After it: 0 rows, 0 graphs, 0 caches, 0 containers, 0 networks. The exported index is kept in `var/m9d/p6/idx/` (gitignored).
- **Java e2e** (`e2e-java.txt`, `scripts/e2e_java.sh`; run after the probe was deleted). Every `fn scip_*` test passed, 15 of 15, in 821.6 s at load 3.5–4.5, single-threaded. Those include `scip_java_spring_jpa`, `scip_java_lombok`, `scip_java_gradle_catalog`, `scip_kotlin_gradle`, `scip_polyglot_monorepo`, `scip_image_identity_is_part_of_the_tool_configuration` and `scip_unfinished_jobs_are_not_up_to_date`. Then `java_spring_jpa`, `kotlin_gradle_tier_b` and `doctor_reports_each_check` passed, 3 of 3.
  - Goldens are unchanged: no bless, and 0 changes under `crates/rb-cli/tests` and `fixtures` before and after.
  - 0 `t_` caches before and after.
  - The one `failed` row written is the deliberate `broken` fixture's (`com.acme:nope`), as in P5's run.
- **Mutants** (`mutants-p6.txt`): m388, m389, m390 and m393 KILLED, and the tree was restored.
- **Process.** Each message carried one `Edit` call, or one scripted edit of one file that asserted every hunk matched once. The scaffold's scripted edit of orchestrate.rs inserted `java_release: None` twice in one literal (one replace pattern was a suffix of the other). A one-line `Edit` removed the duplicate before anything compiled.
- **`rb.repo`** is identical before and after (`repo-before.txt`, `repo-after.txt`; the user's 4 rows).

## Result

Not yet built.
