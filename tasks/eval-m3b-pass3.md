# M3b evaluator pass 3

- Date: 2026-10-01 (wall clock; started 2026-09-30 22:05)
- Commit evaluated: `30fb95d` (`Make the database host port configurable`, on top of the pass-2 fixes `74e0b20` and pass-1 fixes `18970c7`)
- Runtime: Docker Desktop (`desktop-linux`), `rb-scip-java:0.13.1` = `sha256:cbd9963468b4…`, scip-typescript `sha256:028c0042…` (amd64, emulated); database `127.0.0.1:55433`
- Disk: 22 GiB free at the start, 13 GiB at the end (`target/` 3.8 → 4.3 GiB; the rest is Docker Desktop's VM image growing under the ~25 container runs, which nothing in the project directory can reclaim).
- Nothing tracked was modified. Temporary edits for mutation checks were reverted with `git checkout --`; the temporary decode test `crates/rb-scip/tests/eval_pass3_tmp.rs` and `var/eval-pass3/` were removed; evaluator repositories (`ev3_*`), their AGE graphs and their `rb.scip_index` rows were deleted, leaving the 7 fixture rows.

## Baseline (measured, not trusted)

| Check | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` (no database) | 276 passed, 0 failed (awk over the `test result` lines). **13 of the 276 are the e2e tests passing vacuously** (`skipping: set RB_TEST_DATABASE_URL`), so there are 263 unit tests, not 276. |
| `RB_TEST_DATABASE_URL=…55433/rb cargo test -p rb-cli --test e2e` | 13 passed in 14.6 s on the warm index cache; no `skipping:` line. A cold-cache run of the three `scip_*` tests (Java rows deleted first) also passed: `scip_polyglot_monorepo` first index 10.8 s, `scip_java_spring_jpa` first index 8.3 s, cached 0.19 s. |
| Public repos (release build, isolated `--scip-work-dir var/eval-pass3/scip` with a clone of the warm caches) | spring-petclinic @ `818c4136ea97`: **`scip-java failed`** (F1). Same tree with `mvnw`/`mvnw.cmd` removed (scratch commit `a2463c9`): ok in 16.6 s, 240/240 definitions (1 by name), 256 exact `CALLS`, 36 packages, 415 nodes / 925 edges. class-validator @ `2e1a5c27dbd6`: ok in 8.7 s, 417/424, 394 exact `CALLS`, 5 packages, 639 nodes / 1441 edges, 42 files without a document. Cached re-index (decode + join + assemble + publish): 0.21 s petclinic, 0.19 s class-validator. |

## Criteria

| # | Verdict | Evidence |
|---|---|---|
| S1 | PASS (proof mismatch, F13) | Decoder verified end-to-end on both indexers with non-ASCII text on the call line: scip-java (`Document.text`) and scip-typescript (blob) both matched `bmp`/`astral` calls in the scratch repos J1 and T1. No `.scip` sample is committed (`git ls-files | grep .scip$` is empty); the proof column's "committed sample index" does not exist. The pipeline's blob-based conversion has no test that fails when it is removed (F9). |
| S2 | PASS (F12) | `jobs::tests`; a directory named `--help` holding a `tsconfig.json` is passed to `scip-typescript index` as a positional argument and kills the whole root's job. |
| S3 | FAIL (F3) | The fetch phase runs with `--network bridge` (`crates/rb-scip/src/runner.rs:80`), not "registry-only"; the code comment at `runner.rs:11` defers allow-listing to M5 while the contract row and invariant 3 claim it. Timeouts, missing Docker, missing image and build failures degrade correctly (below). The Java e2e proof does not run a container on a warm cache (F10). |
| S4 | PASS | `(cached)` on the second run of every repo; cold vs warm 16.6 s → 0.21 s (petclinic). Budget is not part of the key (F7). |
| S5 | FAIL (F2, F4) | Real scip-java index over 4 legal Java files gives `Shape#area(). IMPLEMENTS Circle#area().` (interface → class, `scip`/`exact`); real scip-typescript index with `interface Mid extends Base` loses `Svc#m() IMPLEMENTS Base#m()` and the other three relationships through the interface chain. |
| S6 | PASS (F15) | Key, whole descriptor, `/`-tails, dotted form and `path:line` all resolve on the Java fixture; the fully qualified dotted form `com.acme.auth.AuthService.authenticate` resolves to nothing. |
| S7 | PASS | Dump sweep on both public repos: 0 heuristic code edges with evidence in a documented file; 0 `scip` edges without certainty 5; `test/`/`sample/` files of class-validator keep 34 heuristic `CALLS`, 45 `REFERENCES`, 5 `IMPLEMENTS`. |
| S8 | PASS | petclinic 36 `Package` nodes / 36 `DEPENDS_ON`, class-validator 5 (`@types/node@20.12.10`, `@types/validator@13.15.3`, `libphonenumber-js@1.11.1`, `libphonenumber-js/max@HEAD`, `typescript@5.6.2`), as recorded. |
| S9 | PASS (F17) | `scip:` lines print status, unpinned, time, coverage; `code:` line reads `100.0% … (0 resolved, 0 ambiguous, 0 unresolved)` for an all-SCIP project. |
| S10 | PASS | Goldens pass on a cold cache with real containers (above); `services/broken` is `failed` with Maven's resolution error, `apps/web` failed, `services/auth` exact. |
| S11 | FAIL (F1) | spring-petclinic at the recorded commit no longer indexes on the environment of record: scip-java executes `/sources/mvnw` and dies with `Permission denied`. The numbers reproduce only after deleting the wrapper from the tree. Sampling on that derived tree and on class-validator: 20/20 and 20/20 (below). |
| S12 | FAIL as worded (F8) | Warm re-index dumps byte-identically (`c2 identical`), but two fresh runs of one commit differ in `"elapsed_ms"` (3255 / 5271 / 5280), and a warm re-index after a concurrent run differs from that run's own dump because the cache row now carries the other run's time. |
| S13 | PASS (F11, F18) | fmt, clippy, stub scan clean (`grep -rn "TODO\|FIXME\|HACK\|XXX\|unimplemented!\|todo!" crates/` hits only an SQL comment in a test string). Host npm never invoked (no `npm`/`npx` in `crates/`). |
| Inv. 1 | PASS | No mixing found on 45 + 134 documented files (S7 sweep); a Python script under a Maven root and 42 tsconfig-excluded files keep Tier B. Ownership by deepest root is untested (F14). |
| Inv. 2 | FAIL | F2: a false reverse `IMPLEMENTS` edge with provenance `scip`, certainty `exact`. |
| Inv. 3 | FAIL | F3: fetch phase on the full bridge network; it also runs the project's own build (`mvn clean verify`, npm lifecycle scripts) there. No credentials or host toolchains are mounted (`docker_args` passes only the work directory, caches and `NODE_OPTIONS`/`MAVEN_OPTS`). |
| Inv. 4 | PASS | petclinic's failed job, `services/broken`, `apps/web`, a 1 s timeout, a missing image and a hidden Docker all leave `rb index` exit 0 with the diagnostic recorded. |
| Inv. 5 | PASS with reservation (F8) | Same cache row → identical dumps; a cached index gives the same graph as a fresh one (only `elapsed_ms` differs). |
| Inv. 6 | PASS | Every `scip` edge's evidence span equals a Tree-sitter reference span; J1's interior-tab call is counted `unmatched`, not guessed. |

## Findings

### F1 — major — scip-java runs the repository's `mvnw` on Docker Desktop and fails; spring-petclinic (S11) does not index
- Repro: `rb index <spring-petclinic @ 818c4136ea97> --repo x` → `scip: . scip-java failed 14.4s`; the diagnostic's `stderr_tail`: `java.io.IOException: Cannot run program "/sources/mvnw" (in directory "/sources"): Exec failed, error: 13 (Permission denied)`.
- Cause: `docker run --rm -v <dir>:/sources rb-scip-java:0.13.1 sh -c 'test -x /sources/mvnw && echo executable; /sources/mvnw'` on a 0644 file prints `executable` and then `Permission denied` (virtiofs answers `access(X_OK)` yes for root). scip-java therefore prefers `./mvnw`, which the contract (amendment 8) and `crates/rb-scip/src/orchestrate.rs:278-280` say cannot happen ("exported files are never executable, so scip-java cannot prefer a repository's ./mvnw"). That claim held on colima only.
- Observed vs expected: every Maven repository carrying a wrapper (Spring Initializr default, both S11 Java candidates) is `failed` → Tier B; expected ok. The S11 measurement table is not reproducible on the runtime the project moved to (CHANGELOG 22:00 entry re-verified only the fixtures, which have no wrapper).
- Regression test: e2e on `fixtures/java-spring-jpa` plus a non-executable `mvnw` file (or a `jobs`/`orchestrate` unit test that the export or the index command neutralises a wrapper, e.g. by not exporting `mvnw`/`mvnw.cmd` or by passing `--build-tool Maven` with an explicit `mvn`), asserting `scip-java ok`.

### F2 — major — a same-named type anywhere in the project turns scip-java's reverse override link into an exact `IMPLEMENTS` edge (E1 regression, invariant 2)
- Repro (scratch Maven project, 4 files, no dependencies, indexed with the real image): `a/Common.java: interface Common {}`; `a/Shape.java: interface Shape extends Common { double area(); }`; `a/Circle.java: class Circle implements Shape { public double area() … }`; `b/Common.java: class Common extends a.Circle {}`.
- Observed: dump contains `IMPLEMENTS a/Shape#area(). -> a/Circle#area(). scip 5`; `rb impact "Circle#area()."` prints `breaks (1): Shape#area()  exact`. Expected: no edge from the interface to the class.
- Cause: the real index carries `a/Shape#area(). -> a/Circle#area(). impl=true` (scip-java lists both sides, as the contract records); `reaches_supertype` (`crates/rb-indexer/src/scip.rs:379-409`, name lookup at :392) resolves the declared supertype `Common` of `a.Shape` to every type named `Common`, including `b.Common`, whose `extends a.Circle` reaches the target. Amendment 21 documents "the walk follows both" as unresolved; since scip-java always carries the reverse link, the only condition for a wrong `exact` edge is a shared simple name, which nested `Builder`/`Listener`/`Base` types make common.
- Regression test: the J1 files above in `scip::tests::hierarchy`, asserting no `Shape#area() → Circle#area()` edge; resolve declared names through the file's imports/package (rb-syntax already records both) instead of by simple name.

### F3 — major — invariant 3 / S3: the fetch phase is not registry-only, and it executes the repository's build
- Evidence: `crates/rb-scip/src/runner.rs:78-82` maps `Network::Registries` to `--network bridge` (unrestricted egress, host services reachable); `runner.rs:11` comments "registry allow-listing is M5". `orchestrate.rs:316-319` runs `mvn … go-offline` and `mvn clean verify -DskipTests` (all project plugins) online; `orchestrate.rs:222-249` runs `actual-npm ci|install` without `--ignore-scripts`, so the project's and its dependencies' lifecycle scripts run online. Both phases mount the shared caches read-write (`/root/.m2`, `/root/.npm`), which every later job of every repository reads.
- Observed vs expected: the contract row S3 says "fetch phase with registry-only network" and invariant 3 "reaches package registries only"; design §5.2 says the same. The code implements neither, and the deferral is recorded only in a code comment, not in the contract's amendments.
- Regression test: a unit test on `docker_args` asserting the fetch phase gets a restricted network (a user-defined network with an egress allow-list, or a proxy) and `--ignore-scripts` for npm; until then, amend the contract and mark invariant 3 as not met.

### F4 — major — `IMPLEMENTS` through a TypeScript interface chain is lost (E1 not fixed for scip-typescript)
- Repro (scratch npm package T1, real scip-typescript): `interface Base { m(): void }`, `interface Mid extends Base { extra(): void }`, `class Svc implements Mid { m() {} extra() {} }`, `class Sub extends Svc { m() {} }`, `function use(b: Base) { b.m(); }`.
- Observed: the decoded index carries 11 `is_implementation` relationships including `Svc#m(). -> Base#m().`, `Sub#m(). -> Base#m().`, `Svc# -> Base#`, `Sub# -> Base#`; the dump has 7 `IMPLEMENTS` edges and none of those four. `rb callers "Svc#m()."` and `"Sub#m()."` print `(no callers found)` although `use()` calls `b.m()`; `rb impact "Base#m()."` lists no implementer.
- Cause: `rb_syntax::decorations::supertypes` (`crates/rb-syntax/src/decorations.rs:151-176`) reads only `class_heritage`; a TypeScript `interface … extends …` (`extends_type_clause`) yields no supertypes, so `reaches_supertype` stops at `Mid`. All hierarchy tests in `scip.rs` are Java.
- Regression test: the T1 files in a `hierarchy`-style test for TypeScript, asserting `Svc#m(). Implements Base#m().`; plus an `rb-syntax` test that an interface's `extends` list is a supertype.

### F5 — major — a snapshot indexed while the scip-java image was absent is "up to date" once the image exists (D3's twin)
- Repro: with a `docker` shim on `PATH` whose `image inspect` fails (everything else passed through): `rb index fixtures/java-spring-jpa-repo --repo x` → `scip: . scip-java unsupported: … not available locally`. Then with the real `docker`: `rb index … --repo x` → `x: up to date — live snapshot 102 already covers 38a1502257f0 (use --force to rebuild)`.
- Cause: `tool_versions` (`crates/rb-cli/src/main.rs:643-650`) records only `scip: on|off|unavailable`; the images' availability/identity is not part of the snapshot identity, although the graph is heuristic without them. The same holds for a rebuilt image with a new ID (the cache key changes, the snapshot identity does not).
- Regression test: e2e like `scip_availability_is_part_of_the_tool_configuration`, indexing with the image hidden and then present, asserting the second run is not "up to date".

### F6 — minor — an environmental container failure is cached as the project's `failed` status
- Repro: with a shim that makes `docker run` exit 125 (`shim: docker run refused`), one `rb index --no-cache` of the TypeScript fixture wrote `rb.scip_index` = `failed`, `stderr_tail: shim: docker run refused`; every later plain run of that commit reports `failed (cached)` until `--no-cache`. Likewise `--scip-timeout-secs 1` writes `timeout` (F7).
- Cause: `projects.rs:181-213` caches every non-`unsupported` outcome; `runner.rs:197-202` cannot tell a client/daemon failure (exit 125/126/127, image pull failure) from a build failure.
- Regression test: `orchestrate`/`projects` unit with a runner returning exit 125 or `Err`, asserting the outcome is not written to the cache (or is `unsupported`/retryable).

### F7 — minor — the budget is not part of the cache key, so a `timeout` from a small budget is served forever
- Repro: `rb index fx-ts --repo t --no-cache --force --scip-timeout-secs 1` → `timeout 1.2s` (container killed, no leftovers: `docker ps -a --filter name=rb-scip` empty, work dir empty); then `rb index fx-ts --repo t --force` (default 600/900 s) → `timeout (cached) 1.2s`. The `timeout` diagnostic carries no `stderr_tail` and no reason (which phase, which budget).
- Location: `projects.rs:181` (`cache.get(&input_hash, &project.image)`), `main.rs:438-442`.
- Regression test: unit: a cached `timeout` is not served to a run with a larger budget.

### F8 — minor — wall-clock `elapsed_ms` is in the canonical dump, so fresh runs of one commit are not byte-identical
- Repro: three `--no-cache --force` runs of the TS fixture commit: dumps differ only in `"elapsed_ms":3255|5271|5280`; a warm re-index of run c1 after concurrent run c2 differs from c1's dump (`cmp … differ: line 54`); a third repository indexed from the warm cache differs from the first.
- Location: `crates/rb-indexer/src/lib.rs:429-435` (`scip_status` detail). Contradicts design §3.3 rule 4 ("wall-clock time never enters the graph"), `lib.rs:3-4` ("pure function of the commit's tree"), S12 ("two runs are byte-identical") and §13's "cached and fresh indexes dump identically"; `mask_elapsed_ms` in the e2e and `<elapsed>` in goldens exist only to hide it.
- Regression test: e2e asserting two fresh dumps are byte-identical (move the run time out of the diagnostic into `rb index` output / the cache row only).

### F9 — minor — the pipeline's UTF-16 → byte column conversion from blobs has no failing test
- Mutation: `source_of` in `crates/rb-indexer/src/projects.rs:219-222` changed to always return `None` (columns stay in UTF-16 code units for scip-typescript, which emits no `Document.text`): 276 unit tests and all 13 e2e tests pass. The fixtures are ASCII-only, and `Document::unconverted`/`encoding_assumed` (`decode.rs:37-42`) are read by nothing outside the decoder's own tests and the debugging aid, so wrong columns would be silent.
- Regression test: a fixture file with a non-ASCII string before a call on the same line (T1's `bmp()`/`astral()` matched in the real run, so the code is right today), asserted in the TypeScript e2e; expose `unconverted` in `scip_status.coverage`.

### F10 — minor — the Java e2e tests run no container on a warm cache; a broken runner passes them
- Repro: with the `docker run`-refusing shim on `PATH`, `cargo test -p rb-cli --test e2e scip_`: `scip_java_spring_jpa` and `scip_polyglot_monorepo` pass (all rows served from `rb.scip_index`), only `scip_ts_express_prisma` fails at its `--no-cache` step. `e2e.rs:700-707` accepts a first run served from the cache.
- Regression test: a `--no-cache` step (as the TypeScript test has) in both Java tests.

### F11 — minor — test count and a test that asserts nothing
- `cargo test --workspace` counts the 13 e2e tests as passed without a database; CHANGELOG (21:42) reports "276 unit tests". `scip::sample::print_real_index_join` (`crates/rb-indexer/src/scip.rs:1199-1274`) is a debugging aid that returns early and is counted as a passing test. Per `tasks/lessons.md` (2026-09-30): "Report a test that printed `skipping:` as not run."
- Regression test: none; report 263 unit tests, move the debugging aid behind an `#[ignore]` or an example.

### F12 — minor — repository-controlled directory names are passed as flags to scip-typescript
- Repro: package root with `--help/tsconfig.json` next to `tsconfig.json` → `jobs` yields projects `[".", "--help"]`, `orchestrate.rs:262` appends them to `scip-typescript index --output index.scip --no-progress-bar`, the run prints usage and exits 0 with no index: `scip: . scip-typescript failed unpinned 2.4s`, `stderr_tail: (no index.scip was produced)`. The root's real project loses SCIP.
- Regression test: `jobs`/`orchestrate` unit asserting a `--`-separated or `./`-prefixed project path.

### F13 — doc — contract proofs and records that do not match the code
- S1: "unit, on a committed sample index" — no `.scip` file is committed; the decode tests build indexes in code.
- Amendment 8 / `orchestrate.rs:278-280`: "scip-java cannot prefer `./mvnw`" — false on Docker Desktop (F1).
- Measurement note: "SCIP [attributes the site] to the implementation `Validator#validateSync()` (line 61), which is where the call is" — line 61 is the first overload signature; the implementation starts at line 71 and the call is at line 84. The three declarations share one key and the node's location is the first `DEFINED_IN` evidence, so `rb callers` prints `Validator.ts:61`.
- Contract line 77 and `main.rs:69` (`--scip-work-dir` help: "under the home directory on colima") still describe colima; the runtime is Docker Desktop (IMPLEMENT.md "Restored on Docker Desktop").
- S11 table is stale for petclinic on the current runtime (F1).

### F14 — minor — amendment 1's ownership rule (deepest build root) has no test
- Mutation: `lib.rs:299` `owner_of(path) == Some(project.root.as_str())` replaced by `projects::in_root(path, &project.root)` (a file belongs to every enclosing root): 276 unit and 13 e2e tests pass. Only `owner_root` itself is unit-tested (`projects.rs:541`), not its use in `assemble`.
- Regression test: `assemble` with an outer ok TypeScript root whose index has a document for a file of a nested failed package, asserting the file stays heuristic.

### F15 — minor — the fully qualified dotted form does not resolve a scip-java key
- Repro (Java fixture, SCIP on): `rb callers com.acme.auth.AuthService.authenticate` → `nothing matches`; `acme.auth.AuthService.authenticate`, `auth.AuthService.authenticate`, `AuthService.authenticate` all work. `code.rs:270-284` splits the key on `#.()/` but not on the space before the descriptor (`… 1.4.0 com/acme/…` gives the part `0 com`).
- Also cosmetic: `Describer::describe` (`code.rs:189-193`) shortens keys only at a backtick, so every scip-java symbol is displayed as its full 90-character key in `rb callers`/`rb impact` (and in the Java goldens), while heuristic and scip-typescript symbols show their descriptor.
- Regression test: `target_resolution_tiers` with `com.acme.auth.AuthService.authenticate`.

### F16 — minor — the e2e harness copies ignored files into the fixture repository
- Repro: `mkdir -p fixtures/java-spring-jpa/target/classes/db/migration && cp …/V1__init.sql there` (ignored by `.gitignore:5`, `git status` clean) → `orm_table_deps_goldens` fails with an extra `V1__init.sql  create  target/classes/db/migration/V1__init.sql:1` line. `copy_dir` (`e2e.rs:102-113`) copies everything and `commit_all` runs `git add -A` in a repository without the ignore file.
- Regression test: copy `git ls-files` output only (or honour the workspace `.gitignore`); assert the fixture commit's file count equals `git ls-files | wc -l` of the fixture.

### F17 — minor — `rb status` reports `100.0% of references to own code resolved (0 resolved, 0 ambiguous, 0 unresolved; 0 external)` for an all-SCIP repository
- Observed on petclinic and J1. The share is 0/0 rendered as 100%; the real gaps (39 `unanchored`, `unmatched`) sit on the `scip:` line. S9 asks for "the exact share of `CALLS`", which is printed as a count (`256 exact calls (scip)`), not a share.
- Regression test: `resolution_coverage` with zero heuristic sites renders `-` or the SCIP share.

### F18 — minor — code quality
- Quadratic per-document loop: `scip.rs:308` scans `replaced` linearly for every `SymbolInfo`. Timed with a temporary test (release): one document with N definitions and N symbol infos joins in 41 ms (5k), 153 ms (10k), 400 ms (20k), 1.9 s (40k) — ~4× per doubling. Real documents are small (largest seen: 137 symbol infos, `ValidationExecutor.ts`), so this is not felt today; it is the same class as pass-1's D5.
- Dead decoded data: `Occurrence::is_import`, `ROLE_IMPORT/WRITE/READ`, `SymbolInfo::display_name`, `Relationship::is_type_definition/is_definition`, `ScipIndex::external_symbols`, `Document::encoding_assumed/unconverted` have no reader outside `decode.rs` tests and the debugging aid.
- `PIPELINE` (`lib.rs:32`, "bump whenever indexing output changes for the same input") has been `m1b-syntax-1` since `30cecdd`; `tool_versions` is `{"rb":"0.1.0","pipeline":"m1b-syntax-1","schema":"0004_scip_index","scip":"on"}` for both the pass-1 and pass-2 binaries, so a snapshot built by `18970c7` is "up to date" for `74e0b20` although its output (transitive `IMPLEMENTS`, `references_undocumented`) differs.
- Stale text: README (status "M0, M1a, M1b and M2a", no `rb callers`/`rb impact`, no Docker/`rb-scip-java` build step, no `--no-scip`/`--scip-work-dir`/`--scip-timeout-secs`); design §5.2 table still names `ghcr.io/scip-code/scip-java` while the code runs the local image; §5.3 "a definition occurrence becomes a `Symbol` node" (amendment 4 says not built); `render_card` (`main.rs:709`) prints "APIs, tables and call graph: not indexed yet (M2–M4)" under a card that lists packages from the call graph's index.

### Checked and not reproduced
- E2 (concurrency): two `rb index --no-cache --force` of one commit at once both succeeded (`rb-scip-360c245ccfdf873f-97133-…-fetch` and `…-97136-…-fetch` ran side by side in distinct `var/…/scip/<hash>-<pid>-<nanos>` directories); graphs identical to the single run apart from `elapsed_ms` (F8); no container or directory left afterwards, nor after the 1 s timeout. No AGE backend failure occurred in that run.
- E3: petclinic `undocumented 0 / external 1220`, class-validator `0 / 366`, T1 `undocumented 1` for a call into the tsconfig-excluded file (`1 files without a document`). Suspected only (not reproduced): an undefined symbol the `scip` parser rejects gets package `None` and is classified by whether some *defined* symbol also has `None` (`scip.rs:139-140, 251`).
- E4: SCIP key, whole descriptor, `/`-tails, `Class.method`, `path:line` resolve; several matches list candidates (`"authenticate" matches 3 symbols; pass --pick N`).
- Pass-1 fixes: ownership by document (Python script and excluded TS files keep Tier B; documented ones have no heuristic edge), `tool_versions.scip` off/unavailable/on (hidden Docker → `skipped: docker is not available`, then a real run is not "up to date"), timeout kill by name (no container left), `--no-cache` retry after a cached failure (restored the row after F6), `unpinned` printed on both `rb index` and `rb status`.
- Column conversion: BMP and astral characters before a call on the same line match on both indexers; a Java line with an *interior* tab before the call is `unmatched` (known scip-java limitation, counted, no edge either way).
- Java hierarchy features with the real indexer: enum and record implementing an interface, generic supertype `Repo<java.util.List<String>>`, nested `Outer.Listener`/`Outer.Impl`, anonymous class body calls attributed to the enclosing method — all correct.
- Exactness sweep: no `scip` edge with certainty other than 5, no heuristic code edge in a documented file, on 3 dumps (petclinic, class-validator, J1).
- Export refuses `..`, absolute and empty path components (unit); symlinks and submodules are skipped by the manifest and never exported; work directories are named from the hash, pid and nanos, not from repository strings.

## Precision samples

Rule: all `CALLS` edges with provenance `scip` from `rb dump`, sorted bytewise by `(source key, target key)`; with `N = floor(count / 20)`, take indices `floor(N/2) + i·N` for `i = 0…19`; each edge opened at its first evidence span in the source. Verdict = the token at the span is the call and the target is the declaration Java/TypeScript resolution picks (inherited accessors checked against the class hierarchy, overloads against parameter lists, receivers against field types).

**spring-petclinic @ 818c4136ea97 minus `mvnw`/`mvnw.cmd` (256 edges, N = 12, offset 6): 20/20 correct.**
1. `PostgresIntegrationTests#PropertiesLogger#onApplicationEvent()` → `PropertiesLogger#printProperties()` — :106 ✓
2. `Owner#addPet()` → `BaseEntity#getId()` — Owner.java:105 `existingPet.getId()`, Pet → NamedEntity → BaseEntity ✓
3. `Owner#toString()` → `BaseEntity#getId()` — :158 `this.getId()` ✓
4. `OwnerController#processUpdateOwnerForm()` → `BaseEntity#setId()` — :158 ✓
5. `OwnerControllerTests#processFindFormByLastName()` → `OwnerControllerTests#george()` — :153 ✓
6. `OwnerControllerTests#processUpdateOwnerFormWithIdMismatch()` → `Owner#setAddress()` — :267 ✓
7. `OwnerTests#addPetAddsPersistedPet()` → `Owner#addPet()` — :32 ✓
8. `PetController#processCreationForm()` → `BaseEntity#isNew()` — :111 `pet.isNew()` ✓
9. `PetController#updatePetDetails()` → `BaseEntity#getId()` — :187 ✓
10. `PetControllerTests#setup()` → `OwnerRepository#findById()` — :86, field `OwnerRepository owners`, declared OwnerRepository.java:60 ✓
11. `PetTypeFormatterTests#shouldThrowParseException()` → `PetTypeRepository#findPetTypes()` — :72 ✓
12. `PetValidatorTests#ValidateHasErrors#validateWithInvalidPetName()` → `Pet#setBirthDate()` — :95 ✓
13. `PetValidatorTests#supportsPetClass()` → `PetValidator#supports()` — :67 ✓
14. `VisitControllerTests#init()` → `OwnerRepository#findById()` — :65 ✓
15. `ClinicServiceTests#shouldAllowSamePetNameForDifferentOwners()` → `Pet#setType()` — :291 ✓
16. `ClinicServiceTests#shouldFindAllPetTypes()` → `EntityUtils#getById()` — :151 static ✓
17. `ClinicServiceTests#shouldFindVets()` → `VetRepository#findAll()` — :208 no-arg overload (VetRepository.java:46; `findAll(Pageable)` is `(+1)`) ✓
18. `ClinicServiceTests#shouldInsertPetIntoDatabaseAndGenerateId()` → `NamedEntity#setName()` — :167 ✓
19. `ClinicServiceTests#shouldUpdatePetName()` → `NamedEntity#getName()` — :193 ✓
20. `VetController#showResourcesVetList()` → `VetRepository#findAll()` — :70 ✓

**class-validator @ 2e1a5c27dbd6 (394 edges, N = 19, offset 9): 20/20 correct.**
1. `index.ts/validateOrReject()` → `Validator#validateOrReject()` — index.ts:85 ✓ (both symbols cover three overload declarations each)
2. `ArrayNotContains()` → `ValidateBy.ts/buildMessage()` — :27 ✓
3. `IsIn()` → `ValidateBy()` — :17 ✓
4. `IsNotIn.ts/validate0:` → `isNotIn()` — :22 (the `validate:` arrow property is the source) ✓
5. `MaxDate.ts/validate0:` → `maxDate()` — :22 ✓
6. `IsInstance()` → `ValidateBy()` — :22 ✓
7. `IsBIC()` → `ValidateBy()` — :20 ✓
8. `IsByteLength()` → `buildMessage()` — :26 ✓
9. `IsEAN()` → `buildMessage()` — :25 ✓
10. `IsHSL.ts/validate0:` → `isHSL()` — :26 ✓
11. `IsISBN()` → `ValidateBy()` — :24 ✓
12. `IsISRC()` → `buildMessage()` — :25 ✓
13. `IsLowercase.ts/validate0:` → `isLowercase()` — :24 ✓
14. `IsMongoId.ts/validate0:` → `isMongoId()` — :24 ✓
15. `IsPostalCode()` → `ValidateBy()` — :24 ✓
16. `IsTimeZone()` → `buildMessage()` — :35 ✓
17. `Matches.ts/validate0:` → `matches()` — :40 ✓
18. `IsArray()` → `ValidateBy()` — :17 ✓
19. `IsObject()` → `ValidateBy()` — :19 ✓
20. `ValidationExecutor#customValidations()` → `ValidatorConstraintInterface#validate()` — :279 `customConstraintMetadata.instance.validate(...)`, `instance: ValidatorConstraintInterface` ✓

Opposite errors: 0 heuristic code edges with evidence in a documented file on either repo; the 42 undocumented class-validator files keep 34 `CALLS`, 45 `REFERENCES` and 5 `IMPLEMENTS` heuristic edges (64 of them into SCIP-keyed symbols, keys consistent). The 7 heuristic-keyed symbols inside documented class-validator files are nested functions/class expressions SCIP keys as locals; their 6 outgoing edges are `scip`/`exact` with a heuristic source key, which amendment 4/S6 allow.

## Verdict

VERDICT: FAIL — F1–F5 are major (S3, S5, S11, S12 and invariants 2 and 3 fail as written).
