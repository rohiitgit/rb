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
