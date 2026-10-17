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
- **rb's view of these references** (HEAD). A symbol the job does not define is `references_undocumented` only if its package is one of the job's own (`scip.rs:336`, comparing manager, name and version), else `references_external` (`scip.rs:553`). So server/public symbols with version `.` stay external, and M9c's D8′ hands them back (uncommitted P5) **[M9c]**.
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
- **D10 (scip-go health).** For an index whose tool is `scip-go`, the decoder counts local symbols, and those whose signature text contains `invalid type` (go/types' spelling). The job's coverage records `locals_invalid_type` and `locals`, written only when the first is non-zero, as `lombok_removed` is (`lib.rs:1530`). The status line appends `, N of M locals typed invalid`. No threshold: every healthy bench index measures 0.
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

## Result

Not yet built.
