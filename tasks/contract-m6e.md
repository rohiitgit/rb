# M6e contract: build and lockfile breadth — Gradle, pnpm/Yarn, go.work, classifiers, tsconfig scoping, encodings (draft, planner, 2026-10-11)

**Status:** contract written 2026-10-11; forks F1–F6 decided below with reasons (no orchestrator round-trip: the user directed all of M6e–M6j to run end to end); phase 0 not started.

Written before any code. M6e is done only when every assertion below is backed by a passing test or a recorded measurement. "Verified facts" are what the 2026-10-11 probes read in the code at commit `de4f5c1` (M6d closed; `PIPELINE` `m6c-5`, `RECIPE_PY` `m6d-py-4`).

This contract follows the M5/M6a/M6b/M6c/M6d format and applies, from the start, the lessons M3b–M6d paid for:
- the soundness claims and the closing bar are stated before any review (lesson 2026-10-06);
- every rule that admits data is an **allow-list** of positively identified forms (lessons 2026-10-06);
- a rule that suppresses data counts what it suppressed (lesson 2026-10-01);
- growth is tested by counted operations, never wall clock (lesson 2026-10-06);
- positions are never hand-counted in tests (lesson 2026-09-30);
- invariant 1 compares dumps against the previous milestone's close with every changed line classified (lesson 2026-09-30);
- fixture plan is code (lesson 2026-10-06);
- evaluator pass 1 probes configuration surfaces and allow-list edges (lesson 2026-10-10);
- shape checks cover root manifests, not just code dirs (lesson 2026-10-11);
- no heavy builds during e2e container phases; poisoned cache rows are overwritten, never deleted (lessons 2026-10-11);
- vacuous comparisons prove nothing: dumps are asserted non-empty before diffing (lesson 2026-10-11).

## Soundness claims and closing bar (read first)

M6e's outputs whose failure would be silent: a package edge pinned to the wrong version (a misread catalog/lockfile, a `go.work` replace/exclude mishandled, a classifier collision merged); a Gradle build that exfiltrates through an unlisted host (a plugin repository outside the allow-list, the wrapper downloading its own distribution); a TS document wrongly rejected or wrongly kept (an N3 probe over-matching common code); an encoding misread that shifts positions (a wrong precedence). **Everything not proven takes the safe direction:** an unreadable manifest pins nothing (version `?`, as today); an unresolvable import stays external; a fetch outside the allow-list fails closed; a duplicate across different classifiers never merges; an ambiguous encoding keeps today's reading plus its diagnostics; a new image that is not byte-recorded is never used.

| Surface | Claim (exactly this, no more) | Mechanism | Residue (out of scope by construction) |
|---|---|---|---|
| Gradle image (S1) | Gradle runs from the local scip-java image on a pinned distribution, never the repository's wrapper; the build is recorded and re-verified on use. | `docker/scip-java/` gains a pinned Gradle (dist sha256); image rebuilt locally, ID recorded. | sbt/Bazel in the image stay out; the wrapper stays unexported and unused. |
| Gradle fetch+index (S2) | `gradle` dependency resolution runs behind the egress proxy with exactly the Gradle hosts allow-listed; scip-java indexes offline with `--build-tool Gradle`. | Recipe `m6e-gradle-1`; proxy env as M5 S21; plugin-portal + distribution hosts in the list. | Custom plugin repositories (auth, `maven.google.com`, jcenter mirrors) stay refused unless the host operator adds them via `--scip-registries`. |
| Gradle versions (S3) | `libs.versions.toml` + `build.gradle(.kts)` literals pin exact versions; anything else stays `?`. | Catalog reader (TOML, `toml` crate) + line readers for both DSLs; presence-gated, never inferred. | Kotlin `buildSrc` constants, Groovy `ext` maps, applied-from scripts unmodelled. |
| pnpm/yarn versions (S5) | `pnpm-lock.yaml` + `yarn.lock` (v1, Berry) pin exact versions; anything else stays `?`. | Hand-rolled line readers (no new dependency), allow-listed shapes only. | `package-lock.json` v1 stays unread (as today); `npm-shrinkwrap` unchanged. |
| pnpm/yarn grouping (S6) | pnpm workspaces group like npm ones; Yarn `file:`/`link:` packages resolve in-repository; Berry groups only if preflight clears its fetch determinism. | `pnpm-workspace.yaml` packages globs through the existing allow-list engine; link targets mirror npm's. | Yarn `portal:`/`git:`/`patch:` protocols stay out. |
| go.work/exclude (S8) | `go.work` `use` dirs join services and its replaces apply; an excluded required version resolves to `?` with a diagnostic. | `go.work` reader (use/replace/go lines); exclude rule (fail-safe `?`). | Vendor-mode workspaces unmodelled (vendor/ still indexed as source). |
| Classifiers (S9) | The package key carries the classifier; same GA with different classifiers never merges. | `group/artifact[/classifier]` key in pom producers; duplicate rule per full key. | `<type>` beyond the key (test-jar etc.) recorded, not keyed. |
| tsconfig scoping (S10) | `include`/`exclude`/`files` (+ bounded `extends`) are read, recorded, and used only to explain `files_without_document`; no edge changes. | Line-JSON reader with depth cap + cycle guard; diagnostic detail. | `exclude` globs beyond the allow-list engine stay unread (counted). |
| Encodings (S11) | The effective javac encoding resolves from all five configuration sources in documented precedence; anything unreadable keeps today's reading. | Precedence reader (pom, `.mvn/maven.config`, `compilerArgs`, module properties, `-D` flags); pure function + units. | Non-JDK charsets beyond the decoder's two readings stay Tier B by the existing guards. |
| TS restructurings (S12) | scip-typescript documents holding a line-start `-->` or `>>>>>>>` are rejected at the input (whole-file Tier B). | Decoder reason after 0-incidence calibration on public TS repos. | Mid-line `-->` (`x --> 0`) never rejects; other restructurings stay uncovered and listed. |

**What M6e must not change.** `INJECTS` rules untouched. No new node or edge kind (package rows keep the existing `Package` shape; diagnostics counted non-zero-only). `PIPELINE` becomes `m6e-1` at the first dump change and is bumped on each later one (no bump is also a result, as in M6d).

This is invariant 1, checked against `de4f5c1` dumps of every fixture (with and without SCIP) and of the M6e measurement repositories at their pins, every differing row classified by script (dumps asserted non-empty first).

**Closing bar.** Decided now:
1. **Evaluator pass 1 is a full review,** adversarial probing of the claims above included (Gradle build-script exfiltration shapes, wrapper smuggling, catalog confusion, lockfile confusion, workspace shadowing, replace/exclude games, classifier collisions, glob escapes, encoding lies, N3 probes, registry smuggling via redirects).
2. **Evaluator pass 2 is verification-only** (re-runs every finding's repro + controls, the suite, mutants, invariant 1, determinism).
3. **Blocks closing only:** (a) a pass-1 fix that does not hold; (b) a regression; (c) a false edge above its stated certainty, a misresolved version, or a Q3/Q4 path through a misresolved call, through a controlled form; (d) non-determinism; (e) a failed test/guard/step; (f) an egress probe failure (fetch reaches, or can be made to reach, outside the allow-list).
4. **Not blocking:** a new form in a residue class (recorded for its owner); a precision miss contradicting no rule; §12.5 timing (measured, not asserted).
5. **Timings are measured, not asserted** (hardware, VM allocation, loads, run counts with each number).

## Scope

**In (M6e):** the 12 items above (design §13 M6e row; contract-m4b 12 and 22-part; contract-m4a 17; contract-m5 18-part; contract-m3b amendment 81-part; contract-m6d residue).

**Out of M6e (each assigned):** contract packs → M6f; topology/config packs → M6g; `rb check` → M6h; Kotlin/C# → M6i (Gradle-Kotlin exact edges unlock with this milestone's image); Guice/Dagger/NestJS/Spring-XML/.NET → M6j; `urllib3`/`aiohttp`, `conda`, `GOPRIVATE`/Athens, `Pipfile.lock`/`requirements` pinning → residue; Yarn `portal:`/`git:`/`patch:`, buildSrc/ext/applied-scripts, vendor-mode workspaces, non-JDK charsets → residue (recorded per item).

## Engineer declaration

**Canonical state:** Gradle in the scip-java image (pinned dist + sha256, never the wrapper); new recipe `m6e-gradle-1` (fetch) + index phases; new allow-list hosts (Gradle plugin portal + distribution, measured); new readers: `go.work` text, `libs.versions.toml` (TOML), `build.gradle(.kts)` lines (both DSLs), `pnpm-lock.yaml`/`yarn.lock` lines (hand-rolled, no new dependency), `pnpm-workspace.yaml` globs, `tsconfig.json` include/exclude/files (+ bounded extends), effective-encoding inputs. Graph: no new node/edge kind; `PIPELINE` `m6e-1`… at each dump change. New counters only when non-zero (D3-style).
**Derived:** package versions, SCIP joins, job lists, file membership explanations, effective encodings.
**Invariants:**
1. **Dumps.** `--no-scip` and SCIP dumps differ from `de4f5c1` only in classified rows; nothing unclassified; every compared dump asserted non-empty.
2. **Kinds.** No new node or edge kind.
3. **Certainty monotonicity.** No client edge above `heuristic`; no `exact` version without a lockfile/catalog/pom literal.
4. **Egress closure.** The probe suite passes on the final allow-list; a fetch outside it fails with a named refusal.
5. **Determinism.** Fresh == cached, two names, forced rebuilds: byte-identical on fixtures and measurement repos.
6. **Bounded work.** Every new walk has a visited set + caps; growth ≤ 5× ops at 4× input per new walk.
7. **Stated complexity by counted operations** for each new walk.

## Decisions (made here, with reasons)

- **D1: one image holds Gradle (F1).** Gradle (~120 MB) joins the slim scip-java image rather than a second image: the orchestration (`Indexer::ScipJava`, `Fetch::Gradle` selecting the recipe) stays single-pathed, and the image is built locally so the cost is local disk only. Revisit only if the image breaks the disk budget (measured in preflight).
- **D2: the wrapper is never used, never exported (F1).** The repository's `gradlew` stays unexported (M3b amendment 25) and the fetch invokes the image's `gradle`; a build that requires its wrapper's exact version fails with a clear message rather than downloading one.
- **D3: Gradle egress starts at the plugin portal + the distribution host (F3).** `plugins.gradle.org`, `services.gradle.org`; every further host only if a measured build dials it (each addition probed, each refusal named).
- **D4: lockfile YAML is hand-rolled (F4).** `pnpm-lock.yaml` and Berry `yarn.lock` are read by line-oriented readers over allow-listed shapes — no new dependency, no general YAML parser whose surface the evaluator could smuggle through. Anything outside the shapes is `lockfile_unread` + `?` (today's safe direction).
- **D5: Gradle version precedence mirrors Maven (F2).** A build-file literal and a catalog entry for the same GA resolve like Maven duplicates (first kept, cleared to `@?` with `versions[]`); a `libs.` reference resolves through the catalog; `buildSrc`/`ext`/applied scripts are residue with examples.
- **D6: go.work replaces win, excludes fail safe (F5).** In workspace mode the `go.work` replace applies over the module's (Go semantics); a required version that is excluded resolves to `?` with an `excluded` diagnostic (MVS would pick an unknowable next version — never guess).
- **D7: classifiers ride the key (F2).** `group/artifact[/classifier]`; the duplicate rule runs per full key. `<type>` is recorded, not keyed.
- **D8: tsconfig scoping explains, never changes (F5).** `include`/`exclude`/`files` (+ `extends` bounded by depth 8 with a cycle guard) feed a membership table used only for `files_without_document` partitioning and a project detail — the indexer owns inclusion, so no edge may move because of this reader.
- **D9: encodings resolve in documented precedence (F2).** pom literal < module properties < `compilerArgs -encoding` < `.mvn/maven.config` `-D` < explicit `-D` flags (order verified against Maven docs in preflight); unreadable anywhere keeps today's reading with its diagnostics.
- **D10: N3 rejects at the input (F5).** Line-start `-->` and `>>>>>>>` reject a scip-typescript document whole-file (Tier B fallback) — but only after a 0-incidence calibration on the public TS repositories; mid-line `-->` never rejects.
- **D11: fixture plan is code** (lesson 2026-10-06): `java-gradle-catalog` (settings + Kotlin-DSL build + version catalog + exact CALLS) and `ts-pnpm-yarn` (pnpm workspace + classic yarn service with pinned versions).
- **D12: measurement reuses public pins where they exist** (contract D13 full SHAs); new pins (a small Gradle repo, a pnpm/yarn repo if needed for samples) chosen in preflight by `git ls-remote` + API, never by cloning first.
- **D13: every M5/M4a/M4b-numbered gap keeps its number** in records so the evaluator can trace each to its source row.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **Gradle image.** Pinned Gradle in the scip-java image, versions verified, size recorded; wrapper never used. | unit; recorded build |
| S2 | **Gradle fetch+index.** Dependency resolution behind the proxy; offline `scip-java` index with `--build-tool Gradle`; unlisted hosts refused by name. | unit; probe suite |
| S3 | **Gradle versions.** Catalog + both-DSL literals pin exact versions; everything else `?`. | unit; mutation; e2e goldens |
| S4 | **Gradle fixture + guards.** D11 fixture; must-exist/must-not-exist per new edge class, each biting. | e2e with containers; goldens |
| S5 | **pnpm/yarn versions.** `pnpm-lock.yaml` + `yarn.lock` (v1, Berry) exact; everything else `?`. | unit; mutation; e2e goldens |
| S6 | **pnpm workspaces + yarn links.** Grouped installs; `file:`/`link:` in-repository resolution; Berry groups only if preflight clears determinism. | unit; mutation; e2e goldens |
| S7 | **TS lockfile fixture.** D11 fixture with pinned versions per format. | e2e with containers; goldens |
| S8 | **go.work + exclude.** Workspace joins/replaces; excluded required versions `?` + diagnostic. | unit; mutation; e2e goldens |
| S9 | **Classifiers.** Full-key duplicates only; cross-classifier versions coexist. | unit; mutation; e2e goldens |
| S10 | **tsconfig scoping.** Membership read/recorded; `files_without_document` partitioned; no edge moves. | unit; mutation; e2e goldens |
| S11 | **Effective encodings.** Five-source precedence; unreadable keeps today's reading. | unit; mutation; scratch measurement |
| S12 | **N3 rejection.** Line-start `-->`/`>>>>>>>` reject whole-file after 0-calibration. | unit; mutation; calibration record |
| S13 | **Public repos.** Classified diffs + read-in-source samples; 0 false `exact` versions. | recorded measurement |
| S14 | **Determinism + invariant 1.** Fresh == cached, two names, forced rebuilds; dumps vs `de4f5c1` classified. | e2e; recorded |
| S15 | **Growth by counted operations** (invariant 6). | unit |
| S16 | **Verification loop + closing.** `var/m6e/scripts/verify.sh`: fmt, clippy, counted units, store/server with DB, e2e + api with containers, listener + stub checks. | verification loop |

## Verified facts (read at `de4f5c1`, 2026-10-11 probes)

1. Detection: `rb-git/src/boundaries.rs:129` per-dir `build.gradle(.kts)` only (no `settings.gradle`); `rb-scip/src/jobs.rs:151-182` outermost Gradle roots → `Fetch::Gradle` (same-dir `pom.xml` wins).
2. Orchestration: `orchestrate.rs:319-322` Gradle jobs return `Unsupported("scip-java Gradle builds are not verified yet")`; `java_runs()` Maven-only; `BUILD_WRAPPERS` includes `gradlew(.bat)` (never exported).
3. Image: `docker/scip-java/Dockerfile:1-36` Temurin 21 + Maven 3.9.16 + scip-java 0.13.1, no Gradle (~930 MB unpacked per README).
4. Egress: `REGISTRIES` 8 hosts (npm/yarn/maven/Go/Python); no Gradle hosts — even a wired job would fail closed.
5. Versions: `packages.rs:1` only `package-lock.json` v2/v3 + `go.mod` + `pom.xml` give versions; `yarn.lock`/`pnpm-lock.yaml` noticed-not-read (`lockfile_unread`); `is_input` omits them; `inert()` excludes all four from the raw-text scan.
6. Workspaces: `jobs.rs:124` grouping for npm + Yarn-v1 only; `pnpm-lock.yaml` → `workspace_lockfile_unsupported`; Berry detected (`is_yarn_berry`) → `yarn_berry` ungrouped; fetch maps Yarn→`yarn install`, Pnpm→`pnpm install` (all `--ignore-scripts`).
7. Go: `packages.rs:289` `GoMod{requires,replaces}` only; `exclude`/`go.work` zero hits (M6e residue, M4b-22).
8. Maven: `packages.rs:912` pom reads no `<classifier>`; key `group/artifact`; duplicates → `@?` with `versions[]`.
9. TS: `rta.rs:205` aliases read only `paths`/`exports`/`main`; `scip.rs:107` `files_without_document` counts without reading `include`/`exclude`; N3 layouts uncovered (amendment 79).
10. Fixtures: zero Gradle files; Java fixtures all Maven; no pnpm/yarn-lockfile fixtures.

**Not verified (checked in preflight, before the step that needs it):** scip-java's Gradle invocation flags; the Gradle stable pin + dist sha256 + image size delta; what a Gradle build dials (host set); pnpm/yarn image tooling (versions, Berry flags, Berry determinism); fixture dep versions; measurement pins; N3 incidence on public TS repos; Maven encoding precedence docs; `.mvn/maven.config` shapes on measured repos.

## Fixture plan

- `fixtures/java-gradle-catalog/` (checked against language/indexer rules before the fixture phase): `settings.gradle.kts` + `build.gradle.kts` + `gradle/libs.versions.toml`, one external dependency with a pinned version, a small call graph with an exact scip-java edge through the Gradle index, a catalog reference and a literal reference. Goldens: routes/calls/version goldens, must-exist/must-not-exist per class, `status` lines.
- `fixtures/ts-pnpm-yarn/`: a pnpm workspace root (2 members, shared `pnpm-lock.yaml`) plus a classic-Yarn service (`yarn.lock` v1), each with a pinned external version. Goldens likewise (versions per format). Berry: units only, unless preflight clears grouping determinism (then a Berry member joins the fixture by amendment).

## Public-repository measurement plan

| Step | Repository @ pin | Runs | Notes |
|---|---|---|---|
| Phase 0 | — | `de4f5c1` binary dumps (`--no-scip`) of nothing yet; e2e green baseline | small |
| Phase 13 | Gradle repo @ pin (chosen preflight) | `--no-scip` both binaries + Gradle SCIP both binaries | classified diffs |
| Phase 13 | pnpm/yarn + go.work + classifier + tsconfig carriers @ pins | `--no-scip` both binaries | version samples, 0 false `exact` |
| Phase 13 | existing public TS repos | N3 calibration scan (0-incidence) | no containers |
| Phase 14 | records + evaluator passes | — | — |

Disk policy as M6b D9 (6 GiB floor, one repo at a time, remove + `VACUUM`, part-by-part figures).

## Determinism

No wall-clock/host-path/run-order in dumps (unchanged rules). Image identities are content digests (the rebuilt Java image's ID recorded; a rebuild invalidates by identity, F1 of M6d). Fresh == cached, two names, forced rebuilds on fixtures (e2e); `--no-scip` both binaries byte-identical modulo classified rows.

## Verification loop

`var/m6e/scripts/verify.sh <tag>` (copied from M6d's, M6e paths/logs): fmt, clippy `-D warnings`, counted units, `rb-store`/`rb-server` with DB, e2e + api with containers, listener + stub checks, mutation runs per M4a harness rules.

## Lessons applied

| Lesson | Where |
|---|---|
| Soundness bar before review; close on verification | this section |
| Allow-lists, never deny-lists | S2/S3/S5 (hosts, catalog/literial shapes, lockfile shapes), S12 (line-start forms) |
| A rule that suppresses data counts it | fetch refusals named; narrowed keeps counted |
| Growth by counted operations | S15, invariant 6 |
| Never hand-count positions | S-assertions |
| Invariant vs previous close, classified | S14 |
| Fixture plan is code | D11 |
| Probe config surfaces + allow-list edges | S-assertions (evaluator pass 1 battery) |
| Shape checks cover root manifests | S3/S5 lockfile enumeration at repo root first |
| No builds during container phases; poisoned rows overwritten | S-verification, S14 |
| Vacuous comparisons prove nothing | S14 |

## Forks (decided here, with reasons)

- **F1: Gradle joins the scip-java image.** Single image (orchestration stays single-pathed; ~120 MB over ~930 MB local). The wrapper is never used or exported.
- **F2: Gradle version pinned in preflight.** Latest stable the installed scip-java supports; dist sha256 recorded; never the wrapper's version.
- **F3: egress starts at the plugin portal + the distribution host.** Additions only for measured build traffic, each probed.
- **F4: YAML is hand-rolled.** Line readers over allow-listed `pnpm-lock.yaml`/Berry shapes; no new dependency; everything else `lockfile_unread`.
- **F5: tsconfig scoping and Berry grouping are bounded.** Scoping explains only (no edge moves); Berry groups only if its fetch proves deterministic in preflight.
- **F6: measurement repos chosen in preflight.** Small, pinned, dependency-light; full SHAs; clones removed after reading.

## Amendments

1. **(phase 0) Baselines, pins and shape checks.** Baseline binary `var/m6e/base/rb-de4f5c1` (release, sha256 recorded; HEAD code == `de4f5c1` — the contract commit adds no code). Full-SHA pins verified by API/refs + `git ls-remote`: jcommander `c6f3d8d7c9f239a02ea7e7a98edc1c56a8dfd23a` (master; Gradle Kotlin DSL, Apache-2.0), ofetch `1dbc37fd1ceab832fc7c90cad81b1091c95ba563` (main; `pnpm-lock.yaml`, no workspace file, MIT). yarn-classic and go.work public samples: none in reach after 8 attempts (dayjs/axios/lodash/npm; prettier/babel Berry; mattermost/gin-realworld/grpc-go/grpc-gateway/hugo/dagger no `go.work`) — units + fixtures carry those shapes, recorded gap. Shape checks: Gradle detection per-dir `build.gradle(.kts)` + outermost SCIP roots (same-dir pom wins); `Fetch::Gradle` always `unsupported` today; image Maven-only (~931 MB); egress 8 hosts, no Gradle hosts; versions only npm-lock/go.mod/pom; grouping npm+Yarn-v1 only (Berry detected → ungrouped); no `exclude`/`go.work`/classifier/tsconfig-include readers; fixtures N3-clean (`-->`/`>>>>>>>` 0 hits). scip-java Gradle interface (primary docs + `--help` on the current image): `--build-tool=<gradle>`, default command `clean compileTestJava …`, detection files `settings.gradle`/`gradlew`/`build.gradle(.kts)`; Java 11/17/21 (image: Temurin 21). Gradle pin 8.14.6 (dist sha256 `7988ed07…`; 8.x line scip-java 0.13.1 was built against, 8.14.6 current Oct 2026 — 9.x breaks APIs, recorded reason). Image tooling (pinned TS image): yarn 1.22.22 classic (Berry grouping stays out — versions read only), pnpm 10.17.1, node v24.6.0. Fixture dep versions read: commons-lang3 3.20.0 (Maven Central), semver 7.8.5, is-odd 3.0.1 (npm registry). Encoding precedence from the compiler-plugin docs (`<encoding>` User Property `encoding` default `${project.build.sourceEncoding}`; `compilerArgs` raw append): pom literal < module properties < `.mvn/maven.config` `-D` < `-Dencoding` hypothesized, with the `compilerArgs`-position and explicit-vs-`-D` points flagged for scratch measurement in S11. Images verified by inspect; DB holds only the 3 pre-existing `fx_*` repos; disk 11G free. Forks F1–F6 decided in the contract with reasons. Dumps (`--no-scip`, baseline binary, all non-empty): polyglot 73,846 B, java-spring-jpa 89,280 B, ts-express-prisma 9,971 B, java-lombok 19,818 B, ts-workspaces 23,524 B, go-chi 14,474 B, python-fastapi 25,566 B; 7 `t_p0_*` repos await the next e2e start-up clean-up.

2. **(phase 1) Gradle image.** Temurin 21 base unchanged; Gradle 8.14.6 `bin.zip` from `services.gradle.org` with published sha256 `7988ed07…`, extracted with the JDK's `jar` (`unzip` is not in the image) plus `chmod 0755` (`jar xf` drops exec bits — found by the first build's `Permission denied`, fixed before any use); `GRADLE_HOME=/opt/gradle`, `PATH` first. Built locally as `rb-scip-java:0.13.1` (same tag: the scip-java version is unchanged; ID `cbd99634…` → `f9612a94…`, 1,225,522,433 bytes ≈ 1.23 GB vs 931 MB before; tool_versions identities invalidate caches by ID). Verified in-image, `--network none`: `gradle --version` 8.14.6 on JVM 21.0.12.1, `mvn --version` 3.9.16 unchanged. No code change (jobs already plan `Fetch::Gradle`; recipes land in phase 2) and no mutants (image-only; S1 proven by the recorded build). DB 0 new repos; disk 11→9.6 GiB (image delta + build layers, accounted).

3. **(phase 2) Gradle fetch + index.** `RECIPE_GRADLE m6e-gradle-1`; `recipe()` now takes the job (Gradle jobs carry the Gradle recipe — a Maven-fetched index can never serve them; threads stay Maven-only). `gradle_runs()`: fetch `gradle --console=plain --quiet --no-daemon --no-build-cache dependencies` + `clean compileJava compileTestJava` behind the run's proxy, index `scip-java index --build-tool Gradle -- --offline …` (casing verified at smoke); per-repo `gradle` cache at `/root/.gradle` with `GRADLE_USER_HOME` explicit; `--quiet` on fetches only (stderr-tail determinism, Maven `-q` precedent). `Fetch::Gradle` `unsupported` arm deleted; dispatch by (indexer, fetch). Gradle reads proxy config from JVM system properties, not the environment (documented): per-run `GRADLE_OPTS=-Dhttps.proxyHost=…` extended onto Gradle fetches beside `proxy_env` (`--no-daemon` keeps one JVM, so it reaches resolution); the first smoke failed on name resolution, proving the need. `plugins.gradle.org` joins the allow-list (probe: 200 through the proxy); `services.gradle.org` stays refused (no measured traffic, no documented runtime need — wrapper-only host; fail-closed with a named refusal, operator-extensible; the probe names it in `refusals`). Smoke: scratch Groovy build fetches commons-lang3 through the proxy and indexes `scip-java ok` (105.7 s; exact `main→shout→exclaim` CALLS with manager `maven`); cached up-to-date, forced refetch and second name byte-identical. Tests first (argv pins failed on `Unsupported`; egress/key pins on the new hosts; the index-task typo caught by the pin); 6 mutants killed (registry, recipe, fetch, tool-swap, dispatch, proxy-opts). Verify: fmt/clippy clean; 766 unit + 25 cli-bin (one net new: the Gradle test split in two); store 66, server 12; e2e 34/34 (32 in-loop + probes/python re-green: probes needed the new refusal in expectations, python needed an uncontended refetch of the known pip read-timeout flake); api 1/1 (150 sets); no `skipping:`, no listener, stub clean. DB 0 new repos (t_* auto-cleaned); disk 11→10 GiB. Files: `rb-scip/src/{orchestrate.rs,egress.rs,hash.rs}`, `rb-indexer/src/projects.rs`, `rb-cli/tests/e2e.rs` (probes).
