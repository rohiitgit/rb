# M6d contract: Go and Python SCIP — images, fetch phases, keys, lockfiles, clients (draft, planner, 2026-10-11)

**Status:** contract written 2026-10-11; forks F1–F6 all as recommended (user, 2026-10-11, by "continue"); phase 0 done 2026-10-11 (amendment 1).

Written before any code. M6d is done only when every assertion below is backed by a passing test or a recorded measurement. "Verified facts" are what the 2026-10-11 probe read in the code, goldens and records at commit `027a54f` (M6c closed; `PIPELINE` `m6c-5`).

This contract follows the M5/M6a/M6b/M6c format and applies, from the start, the lessons M3b–M6c paid for:
- the soundness claims and the closing bar are stated before any review (lesson 2026-10-06);
- every rule that admits data is an **allow-list** of positively identified forms (lessons 2026-10-06);
- a rule that suppresses data counts what it suppressed (lesson 2026-10-01);
- growth is tested by counted operations, never wall clock (lesson 2026-10-06);
- positions are never hand-counted in tests (lesson 2026-09-30);
- invariant 1 compares dumps against the previous milestone's close with every changed line classified (lesson 2026-09-30);
- fixture plan is code (lesson 2026-10-06);
- evaluator pass 1 probes configuration surfaces and allow-list edges (lesson 2026-10-10);
- shape checks cover root manifests, not just code dirs (lesson 2026-10-11).

## Soundness claims and closing bar (read first)

M6d's outputs whose failure would be silent: a package edge pinned to the wrong version (a misread lockfile, a `go.mod` replace/retract mishandled); a SCIP symbol joined to the wrong Tier B node (a Go/Python key mismatch, worse for Go's implicitly-satisfied interfaces); a `requests`/`httpx` call stitched to the wrong endpoint (a session/base-URL misfold); an egress allow-list that reaches beyond registries (a fetch that exfiltrates or a cache poisoned through an unlisted host). **Everything not proven takes the safe direction:** an unreadable lockfile pins nothing (version `?`, as today); an unresolvable Go/Python import stays external; a client call whose base URL is not proven stays `unmatched`; a fetch outside the allow-list fails closed; a new SCIP image that is not byte-pinned is never used.

| Surface | Claim (exactly this, no more) | Mechanism | Residue (out of scope by construction) |
|---|---|---|---|
| scip-go image (S1) | A locally built image on a pinned Go toolchain and pinned base indexes the fixture; its digest is recorded and re-verified on use. | `docker/scip-go/` recipe (Dockerfile + pinned toolchain digest), built locally, never pulled; image identity in tool versions. | Upstream `sourcegraph/scip-go` staleness stays as documented; cross-compilation (cgo) unmodelled. |
| scip-python image (S2) | The pinned image is recorded by digest only after its uncompressed size is measured against the disk budget. | Digest pin in `jobs.rs` after a measured pull; size recorded in the amendment. | `conda`/custom interpreter layouts unmodelled. |
| Go fetch (S3) | `go mod download` (and only it) runs behind the egress proxy with exactly the Go module proxy allow-listed. | Recipe `m6d-go-1` (fetch) + index phases, mirroring the Java/TS recipes; proxy env + settings mounted as M5 S21. | `GOPRIVATE`/Athens proxies, vendored modules stay as read (vendor/ is still indexed as source). |
| Python fetch (S4) | `pip`/`poetry`/`uv` environment resolution runs behind the proxy with exactly PyPI allow-listed. | Recipe `m6d-py-1`; requirements/lockfile-driven installs only, `--no-deps` where the tool supports it. | `conda`, system-site-packages, editable installs outside the repo unmodelled. |
| Go package keys (S5) | One `go/<module>@<version>` key shared by SCIP and manifest package edges. | Normalised module path + version in both producers; mismatch keeps both rows (never merges across keys). | `go.work`/`exclude`/nested modules stay M6e. |
| Lockfile versions (S6) | `poetry.lock`/`uv.lock` pin exact versions; anything else stays `?`. | Readers for the two formats (package + version + source); presence-gated, never inferred. | `Pipfile.lock`, `requirements.txt` (unpinned by construction), poetry groups/extras unmodelled. |
| requests/httpx (S7) | `requests`/`httpx` call sites become fetch-class client sites (M6b-S9 parity): verbs, sessions, base-URL folding; reads capped exactly as the existing fetch class. | Pack-side site walk with the same certainty folding; per-lib verb/config allow-lists. | `urllib3`/`aiohttp`/Tornado clients stay out (residue). |
| Python member idioms (S8) | `Session()`-style receivers and `self.session` members resolve like their TS/Go counterparts; anything dynamic stays dynamic. | Facts + fold parity with the M6b member idioms; allow-listed shapes only. | Monkey-patched attributes, `__getattr__` stays dynamic. |
| Fixtures (S9) | `go-chi-sqlx` (route groups, `database/sql`, implicitly-satisfied interfaces via SCIP) and `python-fastapi-sqlalchemy` (decorators, `__tablename__`, f-string SQL, import resolution) carry must-exist/must-not-exist guards per new edge class, each biting. | §12.1 shapes; D10 fixture plan is code. | chi middleware chains beyond route groups stay M6g. |
| Egress review (S10) | The fetch-phase egress review re-runs over the final allow-list: every forbidden path closed, every registry reachable, refusals named. | M5 S21 probe suite extended to the Go/Python registries. | Findings fix or record as residue. |
| Measurements (S11) | mattermost's Go re-measured with scip-go plus a Python repo at pin (chosen in preflight): classified diffs, sampled edges read in source. | `--no-scip` baselines + SCIP runs; container e2e in verify. | Gradle/pnpm/`go.work` breadth stays M6e. |

**What M6d must not change.** No new node or edge kind (package rows keyed by an existing `Package` shape; client sites reuse the fetch class). `INJECTS` rules untouched (no DI languages added). `PIPELINE` becomes `m6d-1` at the first dump change and is bumped on each later one.

This is invariant 1, checked against `027a54f` dumps of every fixture (with and without SCIP) and of the M6d measurement repositories at their pins, every differing row classified by script.

**Closing bar.** Decided now:
1. **Evaluator pass 1 is a full review,** adversarial probing of the claims above included (image digest spoofing, registry smuggling via redirects/CNAMEs, lockfile confusion, `replace`/`retract` directives, version-suffixed Go imports, session/base-URL folding lies, `verify=False`/proxy-env leaks in fixtures).
2. **Evaluator pass 2 is verification-only** (re-runs every finding's repro + controls, the suite, mutants, invariant 1, determinism).
3. **Blocks closing only:** (a) a pass-1 fix that does not hold; (b) a regression; (c) a false edge above its stated certainty, a pruned-but-instantiated type, a false orphan, or a Q3/Q4 path through a misresolved call, through a controlled form; (d) non-determinism; (e) a failed test/guard/step; (f) an egress probe failure (fetch reaches, or can be made to reach, outside the allow-list).
4. **Not blocking:** a new form in a residue class (recorded for its owner); a precision miss contradicting no rule; §12.5 timing (measured, not asserted).
5. **Timings are measured, not asserted** (hardware, VM allocation, loads, run counts with each number).

## Scope

**In (M6d):** the 11 items above (design §13 M6d row; D13: contract-m4b 13; contract-m5 12-part, 22; contract-m4a 14-part; §12.1 fixtures).

**Out of M6d (each assigned):** scip-python/-go follow-ups (new indexers) → later M6d work only if asked, else M6h+; Gradle/pnpm/Yarn/`go.work`/Maven classifiers/tsconfig-scoping → M6e; contract packs → M6f; topology/config packs → M6g; Kotlin/C# → M6i; Guice/Dagger/NestJS/Spring-XML/.NET → M6j; `urllib3`/`aiohttp`, `conda`, `GOPRIVATE`/Athens, `Pipfile.lock`/`requirements` pinning → residue (recorded per item).

## Engineer declaration

**Canonical state:** new readers: `go.mod` (already collected?) — verify in preflight — plus `poetry.lock`/`uv.lock` texts (plain-text, no new dependency); new recipes `m6d-go-1`/`m6d-py-1`; new images (local build + digest pin); new allow-list hosts (Go proxy + PyPI). Graph: no new node/edge kind; `PIPELINE` `m6d-1`… at each dump change. New counters only when non-zero (D3-style).
**Derived:** package versions, SCIP joins, client sites, member folds.
**Invariants:**
1. **Dumps.** `--no-scip` and SCIP dumps differ from `027a54f` only in classified rows; nothing unclassified.
2. **Kinds.** No new node or edge kind.
3. **Certainty monotonicity.** No client edge above `heuristic`; no `exact` version without a lockfile literal.
4. **Egress closure.** The probe suite passes on the final allow-list; a fetch outside it fails with a named refusal.
5. **Determinism.** Fresh == cached, two names, forced rebuilds: byte-identical on fixtures and measurement repos.
6. **Bounded work.** Every new walk has a visited set + caps; growth ≤ 5× ops at 4× input per new walk.
7. **Stated complexity by counted operations** for each new walk.

## Decisions (made here, with reasons)

- **D1: images are built/pinned locally, never pulled implicitly.** `sourcegraph/scip-go` is stale (design §13); the Go image builds locally on a pinned toolchain + pinned base. scip-python pins by digest only after measuring uncompressed size vs the disk budget. No image is ever built implicitly by a test.
- **D2: allow-list grows by exactly three hosts** (`proxy.golang.org`, `pypi.org`, `files.pythonhosted.org`) unless preflight measures otherwise; everything else stays refused. Each addition rides the existing `RB_SCIP_REGISTRIES`-style plumbing and invalidates the fetch cache by key (existing mechanism).
- **D3: one Go package key.** SCIP and manifest producers share `go/<module>@<version>`; a mismatch keeps both rows (never merges across keys) — the S4 exactness doctrine applied to keys.
- **D4: lockfiles pin, never infer.** `poetry.lock`/`uv.lock` give exact versions only when the format parses positively; `requirements.txt`/unlocked deps stay `?` (today's behaviour).
- **D5: requests/httpx enter as fetch-class** (M6b S9 parity: verbs, sessions, base-URL folding, capped reads) — not as a new client kind.
- **D6: fixture plan is code** (lesson 2026-10-06): `go-chi-sqlx` (route groups via chi, `database/sql` access, implicitly-satisfied interfaces proven through SCIP) and `python-fastapi-sqlalchemy` (router decorators, `__tablename__` models, f-string SQL normalisation, intra-repo import resolution).
- **D7: measurement is SCIP-on-Go plus a Python repo** (mattermost Go re-measured per M5-9; Python repo chosen at preflight from small, pinned, dependency-light candidates); `--no-scip` baselines everywhere; containers only in the verify e2e. Disk policy as M6b D9 (6 GiB floor, one repo at a time, remove + `VACUUM`).
- **D8: every M5/M4a-numbered gap keeps its number** in records so the evaluator can trace each to its source row.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **scip-go image.** Locally built on pinned toolchain + base; digest recorded and re-verified; fixture indexes green. | unit; recorded build |
| S2 | **scip-python pin.** Digest pinned after size measured vs budget; fixture indexes green. | unit; recorded measurement |
| S3 | **Go fetch.** `go mod download` behind the proxy; unlisted hosts refused by name. | unit; probe suite |
| S4 | **Python fetch.** Lockfile-driven env resolution behind the proxy; unlisted hosts refused. | unit; probe suite |
| S5 | **Go package keys.** SCIP and manifest edges share `go/<module>@<version>`; mismatches keep both rows. | unit; mutation; e2e goldens |
| S6 | **Lockfile versions.** `poetry.lock`/`uv.lock` exact; everything else `?`. | unit; mutation; e2e goldens |
| S7 | **requests/httpx.** Fetch-class sites with allow-listed verbs/sessions; reads capped as the class. | unit; mutation; e2e goldens |
| S8 | **Python member idioms.** Session/receiver folds for allow-listed shapes; dynamic stays dynamic. | unit; mutation; e2e goldens |
| S9 | **Fixtures + guards.** D6 fixtures; must-exist/must-not-exist per new edge class, each biting. | e2e with containers; goldens |
| S10 | **Egress review.** Probe suite green on the final allow-list. | recorded review |
| S11 | **Public repos.** Classified diffs + read-in-source samples. | recorded measurement |
| S12 | **Determinism + invariant 1.** Fresh == cached, two names, forced rebuilds; dumps vs `027a54f` classified. | e2e; recorded |
| S13 | **Growth by counted operations** (invariant 6). | unit |
| S14 | **Verification loop + closing.** `var/m6d/scripts/verify.sh`: fmt, clippy, counted units, store/server with DB, e2e + api with containers, listener + stub checks. | verification loop |

## Verified facts (read at `027a54f`, 2026-10-11 probe)

1. Images/jobs: `crates/rb-scip/src/jobs.rs` knows only `ScipJava`/`ScipTypescript` (`SCIP_JAVA_IMAGE = "rb-scip-java:0.13.1"`, scip-typescript pinned by digest); `docker/` holds only `egress-proxy` + `scip-java`.
2. Egress: `REGISTRIES` = npm/yarn/maven only (`crates/rb-scip/src/egress.rs:28`); no Go proxy, no PyPI.
3. Grammars: workspace pins `tree-sitter-go 0.25`, `tree-sitter-python 0.25` (Tier B extraction exists).
4. No Python `requests`/`httpx` client packs, no chi/FastAPI/Flask route packs (Flask-SQLAlchemy model naming only, `rb-frameworks/src/lib.rs:348`).
5. No Go/Python fixtures; `polyglot-monorepo` exercises Tier B Go only.
6. Pins: contract D13 full SHAs (realworld/jhipster/petclinic-mono+micro/piggymetrics/eladmin); Go/Python measurement pins chosen in preflight.

**Not verified (checked in preflight, before the step that needs it):** `go.mod` collection state; poetry/uv shape on candidates; image base/toolchain digests; uncompressed scip-python size; mattermost-Go + Python-candidate pins; egress behaviour of the three new hosts; §12.1 fixture dependency closures.

## Fixture plan

- `fixtures/go-chi-sqlx/` (checked against language/indexer rules before the fixture phase): chi route groups incl. nested/middleware shapes, `database/sql` query sites (normalised SQL), two types implicitly satisfying one interface (SCIP-proven dispatch), a `go.mod` with pinned deps. Goldens: routes/calls goldens, must-exist/must-not-exist per class, `status` lines.
- `fixtures/python-fastapi-sqlalchemy/`: APIRouter decorators incl. prefixes, `__tablename__` models with f-string SQL (normalised), intra-repo imports (relative + absolute), `requests` + `httpx` client sites (session + plain). Goldens likewise.

## Public-repository measurement plan

| Step | Repository @ pin | Runs | Notes |
|---|---|---|---|
| Phase 0 | — | `027a54f` binary dumps (`--no-scip`) of nothing yet; e2e green baseline | small |
| Phase 11 | mattermost (Go paths) @ pin | `--no-scip` both binaries + scip-go both binaries | M5-9 part; classified diffs |
| Phase 11 | Python candidate @ pin | `--no-scip` both binaries + scip-python both binaries | chosen in preflight; classified diffs |
| Phase 12 | records + evaluator passes | — | — |

Disk policy as M6b D9 (6 GiB floor, one repo at a time, remove + `VACUUM`, part-by-part figures).

## Determinism

No wall-clock/host-path/run-order in dumps (unchanged rules). Image identities are content digests. Fresh == cached, two names, forced rebuilds on fixtures (e2e); `--no-scip` both binaries byte-identical modulo classified rows.

## Verification loop

`var/m6d/scripts/verify.sh <tag>` (copied from M6c's, M6d paths/logs): fmt, clippy `-D warnings`, counted units, `rb-store`/`rb-server` with DB, e2e + api with containers, listener + stub checks, mutation runs per M4a harness rules.

## Lessons applied

| Lesson | Where |
|---|---|
| Soundness bar before review; close on verification | this section |
| Allow-lists, never deny-lists | S3/S4 (hosts), S5 (key match), S7 (verbs), S8 (shapes) |
| A rule that suppresses data counts it | fetch refusals named; narrowed keeps counted |
| Growth by counted operations | S13, invariant 6 |
| Never hand-count positions | S-assertions |
| Invariant vs previous close, classified | S12 |
| Fixture plan is code | D6 |
| Probe config surfaces + allow-list edges | S-assertions (evaluator pass 1 battery) |
| Shape checks cover root manifests | S6 lockfile enumeration at repo root first |

## Forks for the orchestrator

- **F1: scip-go image strategy.** Recommendation: **build locally** on a pinned Go toolchain + pinned base (design §13; upstream is stale).
- **F2: scip-python pin.** Recommendation: **measure uncompressed size first, then pin by digest** (disk budget is the binding constraint).
- **F3: egress additions.** Recommendation: **exactly `proxy.golang.org`, `pypi.org`, `files.pythonhosted.org`** — nothing else.
- **F4: Go key unification.** Recommendation: **one shared `go/<module>@<version>` key** for SCIP + manifest edges (S4 exactness doctrine).
- **F5: requests/httpx depth.** Recommendation: **fetch-class parity** (M6b S9: verbs, sessions, base-URL folding, capped reads).
- **F6: Python measurement repo.** Recommendation: **choose in preflight** (small, pinned, dependency-light; TBD).

## Amendments

1. **(phase 0) Baselines, pins and shape checks.** Baseline binary `var/m6d/base/rb-027a54f` (release, sha256 `bcb0e281…`; HEAD code == `027a54f` — the contract commit adds no code). Full-SHA pins verified by API/refs: mattermost Go `47eebd5530c0485391efa77915965b4b7317f16c` (fresh master HEAD; M6b's prefix no longer resolves to a ref); Python repo chosen (F6): `fastapi/full-stack-fastapi-template @ 7257e606cb94eada1dca40173e2b7f99dd549bc6` (7 MB, 252 files, `uv.lock` + `pyproject.toml`, 16 test files; the old `tiangolo/` path 301s to the `fastapi/` org). Shape checks: `go.mod` collected + parsed (direct reqs + replaces; poetry/uv/requirements absent everywhere); no scip-go/scip-python jobs, images or `docker/` dirs; egress `REGISTRIES` npm/yarn/maven only; tree-sitter go/python grammars pinned; no Python `requests`/`httpx` client packs, no chi/FastAPI/Flask route packs (Flask-SQLAlchemy model naming only); no Go/Python fixtures. scip-python amd64 manifest: 14 layers, 188.8 MB compressed (matches the design figure), 3× bound 566 MB vs 18G free — fits; digest `sha256:1847528c…` recorded for the S2 pin. Fixture dep versions read (pinned at creation): chi v5.3.2, fastapi 0.143.0, sqlalchemy 2.1.4, requests 2.34.2, httpx 0.28.1, uv 0.13.0. Go toolchain stable go1.27.2 recorded for the S1 image pin. Egress baseline: `rb-scip` 61/61 green; e2e baseline 8/8 green (4 no-scip fixture suites + 3 scip suites + structure). Images verified by inspect; DB 0 repos; disk 18G free. Forks F1–F6 confirmed (user, 2026-10-11, by "continue").

## Amendments

2. **(phase 1) scip-go image.** `docker/scip-go/` (Dockerfile + README): Ubuntu 24.04 pinned (`sha256:08571ca1…`), Go 1.27.2 tarballs with published sha256 per arch, scip-go v0.2.7 built from source at pinned commit `2e9ff3c2…` (tag fetched, commit verified; module cache dropped after build), `GOTOOLCHAIN=local`, `ca-certificates`/`curl`/`git` explicit. Built locally as `rb-scip-go:0.2.7` (ID `sha256:d70c5ea7…`, 644 MB; `go version go1.27.2`, `scip-go 0.2.7` verified in-image). Wiring: `SCIP_GO_IMAGE`, `Indexer::ScipGo`, `Fetch::Go` (unpinned iff no `go.sum`), per-`go.mod`-dir jobs, `RECIPE_GO m6d-go-1`, `go mod download` fetch + `scip-go` offline index, export paths root-relative (revisit with evidence in S5). Tests first (planning test failed: no jobs); mixed-repo expectation updated as predicted. Mutants killed: planning-dropped, unpinned-inverted, fetch-command-dropped, index-command-dropped. Growth: one more linear manifest scan, no new walk class. Smoke: stdlib-only module indexes to `index.scip` offline in the image. Images verified by inspect; DB 0 repos; disk 18G free.

## Amendments

3. **(phase 2) scip-python pin.** Pulled `sourcegraph/scip-python` v0.6.6 amd64 (`sha256:1847528c…`, == `latest` today): 14 layers, 188.8 MB compressed, 731.6 MB actual vs 18G free — fits; pinned by digest (`SCIP_PYTHON_IMAGE`, digest-reference identity like scip-typescript). Probed CLI: `scip-python index` (default `index.scip`), needs project metadata (bare files crash it fatally — job planning stays pyproject/setup.py-gated); emulated amd64 is slow (~25 s for 6 lines — S9 e2e budget note). Wiring: `Indexer::ScipPython`, per-manifest-dir jobs (`Fetch::Python`, unpinned until S4/S6), `RECIPE_PY m6d-py-1`, empty fetch vec (S4 fills it), offline `scip-python index`, emulated platform, repository-shaped exports (S5 evidence review). Tests first (planning failed: no jobs); mixed expectations untouched (no existing fixture has Python roots). Mutants killed: planning-dropped, unpinned-inverted, index-args-dropped, fetch-appears. Growth: linear manifest scans, no new walk class. Smoke: proper minimal project indexes to `index.scip` exit 0. DB 0 repos.

4. **(phase 3) Go fetch.** `proxy.golang.org` joins `REGISTRIES` (sorted first); Go fetch pins `GOSUMDB=off` (sumdb outside the allow-list; `go.sum` still verifies when present; image `GOFLAGS=-mod=mod` kept). Tests first (argv env failed; e2e probes failed on missing lines): allow-list + filter + pinned-key updates as predicted (cache key includes the list by design); e2e egress extended (`gomod 200`, `gosum` refused, refusals array). Real proof: chi-bearing module fetches through the proxy and indexes `scip-go ok unpinned` end-to-end via `rb index`. Mutants killed: registry-dropped, gosumdb-dropped. Growth: one env line, no new walk. No dump change, no `PIPELINE` bump. DB 0 repos.

## Amendments

5. **(phase 4) Python fetch.** `pypi.org` + `files.pythonhosted.org` join `REGISTRIES` (F3 complete: exactly the three hosts). `Fetch::Python` carries `PythonLock` (`Poetry`/`Requirements`/`None`, strongest pin wins; `uv.lock` takes the project-only path — no uv installer in the image, S6 reads its versions): `poetry install --no-root --no-interaction` (reproducible, `unpinned=false`), `pip install --no-deps -r requirements.txt`, `pip install --no-deps -e .` (project only; third-party stays unresolved — the safe direction). pip/pypoetry caches mounted per variant; `PIP_DISABLE_PIP_VERSION_CHECK=1` in every phase (M5 S22 precedent). `RECIPE_PY m6d-py-1` → `m6d-py-2` (fetch empty → non-empty invalidates the S2 cache). Tests first (new units did not compile before the change); e2e egress extended (`pypi 200`, `files 404`, `testpypi` refused, refusals array). Real proof: scratch `pyproject` + `requirements.txt` (`six==1.16.0`) fetches through the proxy and indexes `scip-python ok unpinned` via `rb index`, with an exact scip `main → helper` CALLS in the dump; cached re-index up to date, forced refetch byte-identical. Mutants killed (8): both registry drops, poetry→pip swap, `--no-deps` drop, version-check-env drop, recipe revert, unpinned inversion, requirements-gate drop. Growth: no new walk (planning reuses the manifest scan; fetch is one command). No fixture has Python roots, so no Python golden moves and no `PIPELINE` bump. Repair (pre-existing debt, found by this phase's verification): (a) `tool_versions_record_image_identities_when_scip_is_on` constructed `Images` without the phase-1 `go` field, breaking `--all-targets` builds since phase 1; (b) `scip_polyglot_monorepo` was red since phase 1 — the `services/billing` Go job (`scip-go ok unpinned`, gin fetched via the phase-3 proxy path, 14/14 definitions matched, 0 rejections) never had its goldens re-blessed. Four goldens re-blessed, every hunk classified W (wanted new-indexer coverage): `scip-status.txt` (+1 billing `scip-go ok unpinned` line; `code:` 25.0%→14.3% with exact calls 2→6 is the Tier B→SCIP bucket migration for billing — its 4 calls leave the heuristic count and join the exact count; Java/TypeScript lines byte-identical, all inline dump asserts hold); `dump.jsonl` (+1 `scip_status ... services/billing ... scip-go ... skipped unpinned:true` row, the only added row); `status.json` (diagnostics 34→35 + the same skipped row); `diff-polyglot.json` (`scip_status` 5→6 both sides). Carried: billing's `1 documents outside the root` (S5 export-shape evidence review). DB 0 repos; disk 15G free.

## Amendments

6. **(phase 5) Go package keys.** Measured first: a gin-bearing module indexed with `rb-scip-go:0.2.7` shows scip-go reporting module-resolved symbols under manager `gomod` (dependency with resolved version, stdlib as pseudo-module `github.com/golang/go/src@go1.23`, own module version `.`), while the manifest keys `go/...` — the live dump held both `go/gin@v1.10.0` (manifest) and `gomod/gin@v1.10.0` (scip) rows. Fix: `rb_scip::package()` normalises `gomod`→`go` (F4: one shared `go/<module>@<version>` key); names/versions pass through byte-identical; anything but `gomod` passes through untouched. `?`/version mismatch keys apart, so each row survives (D3). `/vN` suffixes and case escapes pass through unmodified with no special code — end-to-end proof deferred to the S9 chi fixture (recorded boundary). Export evidence review (amendments 2–3 follow-up): Go stays root-relative (14/14 definitions matched, 0 rejected; the 1 `documents_outside_root` is a definition-less document with a `..`-escaping path, skipped by construction) and Python stays repository-shaped (S4 scratch exact CALLS) — no code change either way. Tests first (both new units failed on the manager); e2e pin added to `scip_polyglot_monorepo` (one shared gin node, both provenances, stdlib normalized; no golden moved). Mutants killed (5): rewrite-dropped, rewrite-to-all, `?`-rule-dropped, version-in-key-dropped (the last needed its `?`-vs-pinned shape added first — survivor indicted the test). Growth: no new walk (one comparison in `package()`). No `PIPELINE` bump (keys merge rows; no new edge kind). Not re-run: `scip_java_spring_jpa`/`scip_ts_express_prisma` (phase 4; maven/npm pass through byte-identical per unit, and this phase's polyglot run re-ran real java+ts joins green). DB 0 repos; disk 17G free.

## Amendments

7. **(phase 6) Lockfile versions.** Readers (all new, in `rb-indexer/src/packages.rs`, `toml` crate already in the lockfile): `[project].dependencies` as PEP 508 strings (extras/markers/URLs never versions; parenthesised specs tolerated) plus `[tool.poetry.dependencies]` constraints/inline tables (`python` skipped; groups/extras/optional out); `poetry.lock`/`uv.lock` `[[package]]` name+version+source, pinning only registry-or-absent (uv) / absent-or-legacy (poetry) sources — git/directory/editable/url never pins; PEP 503 canonicalisation on both sides of the lookup. Manager `py` (no prior convention; recorded choice). Governance: same-dir poetry.lock wins ties, then uv.lock, then unread shapes (`requirements.txt`/`Pipfile.lock`/`Pipfile`, noticed never read), then the nearest ancestor uv.lock whose pyproject `[tool.uv.workspace] members` matches (exact or one-level `*`); an ancestor poetry.lock never governs (no workspaces). Certainty Exact with `version_from` poetry_lock/uv_lock/unresolved and npm-mirroring diagnostics (`lockfile_unreadable`/`missing_entry`/`unread`). Recorded boundaries: setup.py-only roots (arbitrary code, no reader), intra-workspace Service targets, per-package upstream source beyond the pin decision, `[dependency-groups]`, `**` member globs. Grounded on the measured `full-stack-fastapi-template` layout (root uv.lock + `[tool.uv.workspace] members` + backend `[project]` deps with extras). Tests first (missing functions failed); 10 mutants killed (3 malformed anchors discarded as harness errors, never as survivors; 1 survivor grew its test). Growth: linear scans only, no caps needed (as the existing readers). No fixture has pyproject/poetry/uv files, so no golden moves and no `PIPELINE` bump; e2e-golden coverage lands with the S9 fixture — S6 proven by units, mutants, a scratch end-to-end (poetry/uv/plain roots: exact pins, `?` rows, evidence spans and all three diagnostics verified in the dump) and a green `polyglot_monorepo_structure_goldens`. DB 0 repos; disk 14G free.

## Amendments

8. **(phase 7) requests/httpx.** Pack-side sites with M6b-S9 fetch-class parity, all as `ClientKind::Fetch` (no new kind): module verbs + `request`/`stream` dispatchers behind import gates (alias, `requests.api`/`.sessions` submodule paths via suffix-strip, from-imported verbs/ctors), session receivers bound directly to `Session`/`Client`/`AsyncClient` (httpx `base_url` folds; requests takes none), anonymous inline instances; one shared verb list. Caps (never above heuristic): every carrier site (fine-grained carrier verification is S8), unknown/splat kwargs against per-lib plain lists (`requests` hooks stay plain — response-only; httpx transports/mounts/event-hooks/extensions cap). Out: `send`/`build_request`/`mount`/non-verbs, `urllib3`/`aiohttp`, factories/member receivers/rebinding (S8). Extractor: Python `keyword_argument` reads as single-entry objects (values one level deep) and `**` keeps `...` (fact fingerprint auto-invalidates the cache); `poetry.lock`/`uv.lock` join `inert` (prose records nothing). Tests first (pack units failed on empty Python arm; extractor unit failed on `?`); 10 mutants killed (2 no-ops + 3 stale anchors discarded as harness errors; 1 survivor grew its test). Growth: indexed name lookups (`locals_by_name` + counted ticks), no new walk class. Real proof: mixed scratch (Python clients + Express) stitches 4 sites into 2 `CONSUMES` edges (`client:fetch`, folded literal base, synthetic+heuristic buckets, 0 unmatched). No fixture has requests/httpx imports, so no golden moves and no `PIPELINE` bump; e2e-golden coverage lands with the S9 fixture. Verify: fmt/clippy clean; 750 unit + 25 cli-bin; structure + code-question goldens green. DB 0 repos; disk 15G free.

## Amendments

9. **(phase 8) Python member idioms.** Member receivers resolve like their TS/Go counterparts, all capped: `self.`/`cls.` single-level paths through the enclosing class, via member bindings (construction first, bare-annotation fallback, construction-wins merge, anything conflicting dynamic); session-typed locals/params without construction (FastAPI `Depends` shape, import-gated, incl. bare from-imported names); enclosing-class walk per site with a precomputed (scope, name) map (one bindings pass). Out: deeper paths, factories, `None`/re-bound members, cross-class members, bare names seeing only members, subscripted annotations (`Optional`/unions — the extractor keeps no Python type arguments). Tests first (2 failed, 1 boundary passed vacuously); 9 mutants killed (1 survivor grew its test). Growth: indexed lookups + one linear pass, no new walk class. Real proof: mixed scratch stitches member-base (`self.client` + folded literal base → heuristic), member-session, and typed-param literal sites, with a param-URL site correctly edgeless (dynamic) and a method-mismatched literal correctly unmatched. No golden moves, no `PIPELINE` bump. Verify: fmt/clippy clean; 753 unit + 25 cli-bin; structure + code-question goldens green. DB 0 repos; disk 15G free.

## Amendments

10. **(phase 9a) chi + FastAPI route packs.** chi: top-level verbs, nested `Route` prefixes (inline closures via innermost-Function containment, memoised per param, depth-capped), prefix-less `Group`, `With` chains (incl. `With`-bound sub-routers), `Method`/`Handle` dispatchers; `Mount`, named-register params and non-`With` pre-links stay out. FastAPI: decorator verbs, `APIRouter` prefixes, `include_router` composition (recursive, depth-capped, cycle-safe), `api_route` method spreads (missing→ANY like gin, non-literal→dynamic); websockets, mounts and undecorated defs stay out. New `Framework::Chi`/`FastAPI`. Tests first (3 failed); 11 mutants killed (3 malformed anchors discarded; 2 survivors grew their tests — middleware-closure params, `/vN` import handle). Out (recorded): chi `Mount`, named-register cross-function folding (gin endpoint-stage analog), `**` member globs n/a, FastAPI `mount`/undecorated/class receivers. Verify: fmt/clippy clean; 757 unit + 25 cli-bin. DB 0 repos.

## Amendments

11. **(phase 9b) fixtures + guards.** `go-chi-sqlx` (chi nested Route/Group/With/verbs/Method, database/sql `?`-placeholder sites, Card+Cash implicitly satisfying Payer, sumdb-verified `go.sum` → pinned fetch) and `python-fastapi-sqlalchemy` (two services: shop app with APIRouter prefixes/includes/decorators, `__tablename__` models, f-string + literal SQL, absolute+relative imports; client with session/plain/module/member call sites; requirements-driven fetches, root `uv.lock` pins, non-member client stays `?`). Join: origin-attested interface IMPLEMENTS lands on the interface type (method+type edges; spec method has no node) — measured scip-go one-sided method rels + kinds (Interface/Method/Struct/Field); twin/heritage/class targets still take the rejecting walk. Dispatch CALLS through the interface stay unmatched (RTA future, residue — no guard fossilizes it). Guards per class, each biting (incl. `gomod/` regression pins and the client-`?` governance pin); e2e fns mirror the lombok shape (+ S5 node-merge inline asserts, pinned-fetch asserts). Goldens: `--no-scip` dumps, scip-status, callers — all classified (new files). Fixture pins: sqlalchemy 2.0.54 (2.1.4 needs Python >3.10, the image has 3.10 — measured at fetch failure), chi v5.3.2/go1.23. Tests first (join units failed); mutants: packs 11 + join 4 (kind/heritage/bypass-never + widened-kind), 0 survivors (1 redundant check removed with same-map proof; 3 malformed anchors discarded). Session slip: a `git checkout -- <file>` to undo a mutant reverted the whole S9b implementation — rewritten from context; rule extended (mutant scripts restore, never checkout). Verify: fmt/clippy clean; 759 unit + 25 cli-bin; full 34 e2e + 1 api green (incl. re-run container suites for the shared join). DB 0 repos; disk 17G free.

## Amendments

12. **(phase 10) Egress review.** Re-ran the M5 S21 probe suite over the final allow-list (`files.pythonhosted.org`, `proxy.golang.org`, `pypi.org`, `registry.npmjs.org`, `registry.yarnpkg.com`, `repo.maven.apache.org`, `repo1.maven.org`): `allowed`/`gomod`/`pypi` 200, `files` 404-through-proxy, `gosum`/`testpypi`/unlisted-host/IP/direct/gateway refused (`000`, non-zero exit), plain-HTTP 403, no external DNS, no UDP, internal fetch network confirmed; refusals named (`1.1.1.1:443`, `example.com:443`, `http://repo.maven.apache.org/maven2/`, `sum.golang.org:443`, `test.pypi.org:443`). Verdict: no finding, no code change. Posture per tool, all behind the same per-run proxy (env + Maven settings mount automatic for every fetch): Maven (settings.xml only, ignores env), npm (`--ignore-scripts`, update notices off), Go (`GOSUMDB=off`, `GOFLAGS=-mod=mod` from the image, `GOTOOLCHAIN=local` so a newer-toolchain repo fails closed rather than dialing out), pip (`PIP_DISABLE_PIP_VERSION_CHECK=1`), poetry (`--no-root` skips the root build). Build code (Maven plugins, `go build` during indexing is offline, pip setup.py/poetry backends) runs only inside filtered containers — the same trust class as the M5 Maven fetch; `pip install -e .` executes setup.py with the filter as its only network, matching `--ignore-scripts`' intent (pip offers no equivalent flag; recorded). Residue: conda/anaconda hosts refused by design; `test.pypi.org` stays refused (typo-adjacent confusion host). DB 0 repos.

## Amendments

13. **(phase 11) measurements.** mattermost @ `47eebd…` + full-stack-fastapi-template @ `7257e60…`; full record `var/m6d/bench/phase11.md` (ignored worktree file, as M6b/M6c). --no-scip base-vs-head: identical graphs both repos; +14 mattermost diagnostics (Go job rows + S7 `requests` sites, read in source) and +60 Python-repo rows (23 FastAPI endpoints + 23 handlers + 14 uv-lock pins incl. workspace-member governance, read in source) — all W, nothing removed. Two measurement-driven fetch fixes (both test-first, mutants killed —
storage-drop, GOPROXY-drop, pip-flag-drop): (a) `storage.googleapis.com`
joins the allow-list (Go zips redirect there; fail-closed before) + pinned
`GOPROXY` (S3/S10 verdict: the registry-smuggling battery item, resolved by
narrowing to the proxy's own bucket host); (b) pip
`--ignore-requires-python` + `RECIPE_PY m6d-py-3` (image 3.10 vs repo floor
3.14; the stale cached failure proved argv isn't in the key — recipe bumps
are the invalidation mechanism). SCIP: mattermost 6/9 Go jobs ok (`server` 143 s; `sharedchannel-test` cross-root replace fails with a clear message — M6e breadth); Python backend ok (9.5 s emulated); both fresh == cached byte-identical. Classified diffs: +34k exact Go CALLS, +2.4k Go IMPLEMENTS (implicit satisfaction at scale), +447/+137/+6 Python exact CALLS/REFS/IMPLEMENTS; Azure keys byte-identical (no escapes needed); scip-python emits no third-party `py/` rows (manifest covers versions). Samples read in source (Go interfaces, Azure, Pydantic, cross-tree test call). Disk 17→12→14 GiB (caches persist by design); DB 0 repos.

## Amendments

14. **(evaluator pass 1) fixes.** Two blocking findings fixed test-first, plus claim-breaking hardening; the rest recorded. F1 (E1-1): `tool_versions` now records `scip_go` (image ID) + `scip_python` (digest) — a rebuilt image rebuilds instead of serving "up to date"; unit extended (absent/present) + e2e `scip_image_identity` asserts both. F2 (E2-1): absolute-path `replace` targets never join services (only `./`/`../` join; version stays `?`) — was a false Synthetic service edge. F3 (E3-1): chi `Handle(pattern, handler)` reads `args[..]` (was gin's 3-slot shape; dropped the path and mis-diagnosed the handler). F4 (E3-2): double-included routers expand into duplicated rows (was last-wins silent drop). F5 (E3-3): dispatcher `url=`/`method=` keywords read (positional-Object slot never counted as URL; `method`/`url` excluded from cap keys as native slots). F6 (E2-2): declined with reason — a `gomod/`-prefixed wire name keys apart (D3 fail-closed on the unmeasured form); pinned by unit + mutant. F7 (E1-5): `GOTOOLCHAIN=local` + `GOFLAGS=-mod=mod` moved into the fetch env (argv-pinned by units). Fixture rows added for the fixed shapes (chi `Handle`, double-include with the `/v1`-only-serves-under-`/v2` correction caught by the bless run; session-merge comment). Recorded, no code: E1-2 (amendment 11's "narrowing" corrected — CONNECT-layer filtering cannot scope buckets; residual accepted, same kind as all registry hosts; per-job egress deferred), E1-3 (recipe-bump discipline + per-command argv pins are the enforcement; proved working 3×), E1-4 (poetry installs the full locked tree incl. build backends — same trust class as Maven plugins), E2-3/E2-4 (governance shadowing precedence: safe `?` + diagnostics, intended), E2-5 (malformed directives drop silently like malformed manifests — invalid input, not suppressible data), E3-4 (self-rebinding matches TS `this` parity), E3-5 (`if TYPE_CHECKING:` imports gate — needs If-tracking surface, deferred), E3-6(i) (merge comment added). Process: e2e `rb()` shells `target/debug/rb`, which `cargo test` does NOT refresh — always `cargo build -p rb-cli` before e2e runs (two bless runs debugged to this); double-include demotes carrier-relative matches to suffix/ambiguous by design (unknown base may be prefix-relative — conservatism, not a bug). Mutants killed across fixes: 8 (F2/F3/F4/F5/F7) + prior phases intact. DB 0 repos.

## Amendments

15. **(phase 12) Growth, pip hardening, invariant 1, evaluator passes, close.** S13 audit (each new walk measured at 100 vs 400 inputs, counted ops): chi's per-parameter chain scan 15.85× and FastAPI's per-route include walk 18× — both superlinear by construction. Fixed test-first (both ratio tests failed first): chi resolves closure parameters through a per-file function sweep (`FuncCache`: one linear pass plus one sweep over nesting syntax spans; batched queries, tie order = the old scan's rule; bundled as `ChiState` for clippy's argument limit), FastAPI composes over an adjacency index with per-router memoised levels plus a cycle gate keeping the legacy walk for cyclic files (diamond paths still fan out per path by design, amendment 14 F4 — work bounded by the output). `requests`/`httpx` and member packs measured linear as built (indexed lookups; no change). Same-name shadowing bindings scan linearly in the shared `binding_index_at` (pre-existing, all packs alike — 15.6× on the same-name chi shape after the sweep); the ratio tests use distinct names (the `packs_grow_linearly` precedent) and the shared cost is recorded, not asserted. Tests: 4 ratio tests + 1 cycle pin (the FastAPI chain test failed vacuously before its counting tick — instrumentation first, then the bite); 6 mutants killed (sweep-scan, memo-dropped, cycle-always-memo, cycle-always-legacy, replace-direction-flipped, pip-timeout-dropped). Files: `rb-frameworks/src/routes.rs`, `rb-frameworks/src/clients.rs` (tests only), `rb-scip/src/orchestrate.rs` (`PIP_TIMEOUT`).
PIP_TIMEOUT=60 (`RECIPE_PY m6d-py-3` → `m6d-py-4`): one fixture fetch read 19 s ok / 87 s ok / 33 s and 54 s pip read-timeout failures across four attempts (slow `files.pythonhosted.org` reads through the emulated stack under pip's 15 s default) — env, pinnable like F7's Go env, argv pin unit + mutant; the recipe bump is the invalidation mechanism, so stale rows cannot serve.
Contention incident (recovered): two `cargo test` runs during the p12 e2e's container suites starved the emulated fetch (curl exit 28 on a 200 probe; pip read timeouts) and poisoned one scip row to `failed`, which replayed as `failed (cached)`. Diagnosed from `rb.scip_index.stderr_tail`; cleared by an uncontended `--no-cache --force` run over the same layout (the input hash covers job-relative paths — a nested copy hashes differently); both tests re-ran green alone. The 32 unaffected passes stand (contention fails, never forges). Lessons recorded (no builds during container phases; poison-row recovery; vacuous-comparison guard).
S12 invariant 1 (`--no-scip` dumps, baseline `var/m6d/base/rb-027a54f` vs the closing tree, second HEAD name for determinism): 4/7 fixtures byte-identical; polyglot +1 billing `scip-go skipped` job row (amendment 5); go-chi +5 endpoints/+10 edges/+1 job row with orphan `entry` 1→6 / `mention` 11→6 (the HANDLED_BY-keeps-entry rule, by design); python-fastapi +5 endpoints/+5 EXPOSES/+5 HANDLED_BY/+3 CONSUMES/+6 py pins + http/lockfile diagnostics +2 job rows; 0 removed rows anywhere; two HEAD names byte-identical on all seven. The first sweep compared empty dumps (invalid repo names) — vacuous SAME discarded, guard recorded.
Evaluator pass 1 (3 parallel evaluators, amendment 14): 2 blocking + 9 non-blocking findings, blockers fixed test-first plus hardening, rest recorded. Pass 2 (verification-only, `tasks/eval-m6d-pass2.md`): every finding's repro + controls end to end, suite (fmt, clippy, 765 unit + 25 cli-bin, store 66/66, server 12/12, e2e 34/34, api 1/1 over 150 sets, no `skipping:`, no listener, stub scan clean), mutants 22/22 re-killed (p1 4, p2 4, p3 2, p12 6, eval1 6; p2/p3 anchors repaired after later phases reflowed them; phases 4–11 mutants were ad hoc, their killers green in the suite), invariant 1 and determinism as above. VERDICT PASS.
Records: design §13 (M6d measured, closed), README (four images, Go/Python fetch posture and packs), IMPLEMENT.md, CHANGELOG, lessons (8 new rules), this todo. Full verification loop (`var/m6d/logs/close-*`) green; the M3a wall-clock test flaked twice at ~3.1 s under load 7–10 (external: fseventsd, two VMs, Chrome; M3b-cited) and passes alone (1.52 s, 2.67 s on the closing tree). DB holds only 3 pre-existing evaluator repos (`fx_*`, not this phase's); disk 9→11 GiB across the loop (accounted: `/tmp` copies removed, e2e start-up clean-up). **M6d closed 2026-10-11.** Next: M6e (contract to be written when it starts).
