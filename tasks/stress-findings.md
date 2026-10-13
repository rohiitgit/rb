# Stress & usability campaign: findings register

**Subject:** HEAD `eb496af`, release binary `var/stress/bin/rb` (sha256 `94e0da61…1007f1`).
**Mode:** report only, no code changed. Plan: `~/.claude/plans/make-a-plan-to-curious-taco.md`.
**Started:** 2026-10-13.

**Environment:**
- PG18 + AGE on 55433.
- The Docker Desktop daemon is wedged (CLI calls hang) after the disk-full incident, so SCIP probes wait for a restart.
- DB suites were run with a fail-fast `docker` shim on `PATH` (`var/stress/fakebin/docker`).

**Severity scale:**
- S1: crash or wrong `exact` answer.
- S2: wrong or misleading answer, red suite, hang, or data loss.
- S3: confusing UX, error or exit code.
- S4: polish.

Each finding has a repro at `var/stress/repro/F<n>.sh` and evidence under `var/stress/evidence/`. Every repro ran twice with identical output (timings within ±1 s), except F15 (2 GiB of test data): one repro run plus the original case run.

## Summary

**31 findings**, plus 10 UX proposals and 7 optimization opportunities.

| Severity | Count | IDs |
|---|---|---|
| **S1**: wrong answer labelled `exact` | **3** | F17 gRPC spec endpoints misattributed · F18 FastAPI prefixes dropped, routes merged · F19 Go protobuf getters as RPC calls |
| **S2**: wrong/misleading answer, red suite, hang | **10** | F1 `status` regression (2 DB suites red) · F2 index hangs on an unresponsive Docker · F9 cubic time on method chains · F16 Go `vendor/` as services · F20 Express constant-path routes dropped · F21 default-import axios calls vanish · F22 `dist/` indexed · F23 `.sql` always a migration · F24 `orphan_endpoints` flags almost everything · F28 .NET DI under ImplicitUsings |
| **S3**: confusing, gap without signal | **16** | F3–F7, F10–F15, F25–F27, F29, F30 |
| **S4**: polish | **2** | F8, F31 |

**By area:**

| Area | Findings |
|---|---|
| HTTP, gRPC and routes | 11 |
| CLI/API UX | 8 |
| Performance and memory | 5 |
| Input hygiene (vendor, dist, ignore) | 4 |
| DI | 2 |
| SQL | 1 |

**What held up well:**
- Determinism: byte-identical dumps across two names on all 12 real repos.
- Crash consistency: `kill -9` at 6 points.
- Concurrent publish and read: 400/400 consistent.
- HTTP parity with the CLI: 16/16.
- Load shedding and deadlines.
- All adversarial path, encoding and size cases (no crash, no panic).
- Query latency on a 58k-node graph, and linear growth.
- No performance regression against M5.

**Biggest themes:**
1. The `exact` label is only as good as service and prefix attribution (F17, F18).
2. Many recall losses on *supported* stacks are silent, with coverage lines that read as complete (F20, F21, F25, F28).
3. Nothing keeps vendored or build output out of the graph (F13, F16, F22).
4. One O(depth²) tree-walk pattern causes multi-minute indexing on ordinary-looking files (F9).

## Phase 0: baseline

| Suite (real DB on 55433) | Result |
|---|---|
| `rb-store --lib` | 72/72 |
| `rb-server` | 12/12 |
| `rb-cli --test e2e` | **46/47**, `json_answers_snapshots_and_error_codes` fails (F1) |
| `rb-cli --test api` | **0/1**, `api_bodies_equal_cli_json` fails (F1) |

The earlier "975 passed" run had no `RB_TEST_DATABASE_URL`, so the DB tests silently skipped. That masked F1, which came in with `bfa87ce` "Drop RB_REPO", after M6l closed.

## Findings

| ID | Sev | Area | Finding | Expected vs actual | Suggested fix |
|---|---|---|---|---|---|
| F1 | S2 ✅ fixed M7b | CLI | `rb status` without `--repo` errors instead of listing every repo. Two DB suites are red on HEAD. | README:78 says it "works from anywhere", and e2e:2280 plus the api test expect all repos. Actual: exit 2, "no git repository found" or "no indexed repository matches". | In `answer()` (`crates/rb-cli/src/main.rs:789-819`), skip cwd inference for `Status` (and `snapshots`?). Add a DB-gated CI job so `skipping:` can't hide red suites. |
| F2 | S2 ✅ fixed M7b | SCIP / robustness | `rb index` (SCIP on) hangs forever, with no output, when the docker daemon doesn't answer. Killing `rb` orphans the `docker version` child. | Expected: a bounded probe, then `unsupported` jobs (as when docker is missing). Actual: an indefinite hang (observed live, and with a sleeping shim). The e2e harness's `docker_available()` hangs the same way. | Put a timeout (about 10 s) and kill on the `docker version` probe in `rb-scip/src/runner.rs:177`. Same for `kill_container` and the `gc --egress` calls. |
| F3 | S3 | CLI | A checkout indexed with an explicit `--repo` isn't recognised from its own directory. The hint says to run `rb index .`, which would create a **second** repo under the directory name. | Expected: cwd maps to the repo whose recorded path or remote matches. Actual: inference by directory name only (`demo_spring` ≠ `st_demo`). | Record the checkout path or root commit per repo at index time and match on it, or at least say "indexed under another name? pass --repo". |
| F4 | S3 | CLI / API | The parameter name is glued onto the message: "Error: --repo no git repository found…", and in JSON `"message": "repo no indexed repository…"`. Positional diff FROM is reported as `--from is required`. | Expected: a readable sentence that names real flags. | The message composition in `rb-api/src/error.rs` `cli_text`: separate the param from the message (`--repo: …`) and add `from`/`to` to `POSITIONAL` (error.rs:139). |
| F5 | S3 | CLI (`check`) | `rb check` exits **1** on DB-unreachable, the same code as "violations found", so CI reads an outage as a rule failure. An unknown repo exits 2 here but 3 everywhere else. `--json` errors are plain text on stderr. | Help says "2 on errors". | Route `check` errors through the shared envelope and exit mapping (`main.rs:2395`). |
| F6 | S3 | CLI | DB unreachable: `Error: connecting… Caused by: 0: … 1: …` (anyhow Debug), exit 1 for every command, and **no JSON envelope** under `--json`. | Expected: `{"api":1,"error":{"kind":"unavailable",…}}` and a dedicated exit code. | `main.rs:444,489`: map connection errors to an envelope kind. |
| F7 | S3 | Targets | `callers`/`impact` reject `Db#load()`, `Db#load` and `load()` ("nothing matches", exit 3), while `rb find` resolves each to exactly **one** symbol. Only `Db#load().`, `Db.load`, `load` and `path:line` work. | Expected: a unique `find` hit resolves, or the error names the exact key to type. | Fall back to the `find` match in `rb-store/src/code.rs` target resolution when it is unique, otherwise list candidates as `ambiguous`. |
| F8 | S4 | CLI | The commands suggested in errors aren't shell-quoted. `try \`rb find GET /x --kind endpoint\`` fails with a clap error, and `rb find Db#load()` is a zsh parse error. | Expected: copy-pasteable suggestions. | Quote the argument in the hint text. |

| F9 | S2 ✅ fixed M7b | Perf / syntax | **Indexing time explodes on long method chains.** One JS file holding `b.add(1).add(1)…` (n links): n=1000 (7 KB) takes 16 s, n=2000 (14 KB) takes 124 s, n=4000 (28 KB) takes over 300 s (killed). Growth is about cubic. A single committed builder/DSL file can stall `rb index` for minutes, with no cap, diagnostic or progress output. Java: 2 s / 14 s / 110 s / >300 s at 1k/2k/4k/8k. JS at realistic lengths: 250 links 0.7 s, 500 links 2.5 s. | Expected: linear. **Root cause (sampled, `var/stress/evidence/sample_chain_js.txt`):** 99% of samples are in `rb_syntax::mentions::mentions` → `ts_node_parent` → `ts_node_child_with_descendant`. tree-sitter's `Node::parent()` re-descends from the root (O(depth)), and `mentions` calls it per identifier (`mentions.rs:43`), then `java_role`/`es_role` climb the ancestors with repeated `parent()` (`mentions.rs:127,207`). That is O(identifiers × depth²). The nested-call profile (F10) also shows `calls::chains`/`is_inner_link` (`calls.rs:187`) using the same pattern. | Carry the ancestor stack through the existing DFS (`stack: Vec<(Node, depth)>` plus a parent vector), or walk with a `TreeCursor`, so role decisions read cached ancestors. Expected: linear. |
| F10 | S3 ✅ fixed M7b | Perf / syntax | Nested calls `f(f(…f(1)…))` index in quadratic time in every language: n=10k takes Go 8 s, Python 9 s, Java 14 s, JS 17 s, and 2× depth gives about 4× time. Less realistic than F9, but there's no `MAX_NESTING`-style cap on expressions (that cap covers definitions only). | Expected: a cap and a diagnostic. | Same area as F9, plus `rb_syntax::extract`. |
| F11 | S3 ✅ fixed M7b | Perf / routes | One 183-line Express file with 20 routers mounted on 5 prefixes and cyclic `router.use` takes **44 s** and emits about 1,200 chained endpoints (`/p0/n0/n1/…/n18/x`). The caps (`MAX_MOUNT_DEPTH` 32, `MAX_PREFIXES` 64) bound correctness but not the work done. | Expected: cycle detection that stops enumeration, flags `route_prefix_unknown`, and stays sub-second. | `rb-indexer/src/routes.rs:25-151` mount expansion. |
| F12 | S3 ✅ fixed M7c | Recall / Express | `const app = require('express')();` (the inline factory call) isn't recognised as an Express app. Its routes produce **0 endpoints**, and the services are classified `library`. The two-line `const express = require('express'); const app = express();` works. Found while testing stitching. Not in any residue list. | Expected: an endpoint per `app.get`. | The Express app-binding recognition in `rb-frameworks` routes (the `Router()`/`express()` binding only follows a named import). |
| F13 | S3 | Config / design gap | No way to keep files out of **parsing**. `rb.toml` `exclude` only drops *manifests* from service detection (by design, `docs/repository-brain-design.md:165`), and nothing skips `.min.js`, `linguist-generated` or vendored trees. A committed `node_modules/` (3,000 files) gives 3,000 `http_unmatched` diagnostics and 3,001 `unresolved_references` of noise. `exclude = ["**"]` silently does nothing to parsing and only turns the service into `(unassigned)`. | Expected: something like `rb.toml` `ignore = [globs]` for parsing, and honouring `.gitattributes linguist-generated/vendored`. | `rb-git/src/boundaries.rs:71-125` and `rb-git/src/lib.rs:159`. |
| F14 | S3 | Cypher UX | `rb cypher "MATCH (n) RETURN n"`, the most natural first query, fails: `agtype_value_to_text: unsupported argument agtype 6` (edges 7, paths 8). Only property and scalar returns work. Same over HTTP. | Expected: vertices, edges and paths rendered as JSON (agtype → text cast), or a clear "return properties, e.g. n.key" error. | The cypher result decoding in `rb-store` (`agtype_value_to_text` call). |

| F15 | S3 ✅ fixed M7b | Memory / git | A committed **1 GiB binary** raises peak RSS to **2,063 MiB** (about 2× the blob) just to sniff its first 8 KB. A 300 MB SQL dump costs 317 MiB, though it's then skipped as `too_large`. A repo with a few multi-GB artifacts (models, datasets, releases) can OOM a 16 GB machine. | Expected: memory bounded by `max(8 KB, MAX_PARSE_BYTES)` per blob. | `rb-git/src/lib.rs:85` `find_blob` reads whole blobs: read the header (object size) first and stream or skip blobs over `RAW_SCAN_BYTES`. |
| F16 | S2 | Services / Go | **Go `vendor/` is indexed as part of the repo.** go-gin-example: 1,020 of 1,073 files are `vendor/`, and they become **14 extra projects** in `rb service`. One (`vendor/github.com/ugorji/go/codec`) is even labelled a deployable **service**. Each gets a scip-go job, and the graph is about 10× inflated (10,077 nodes, of which the app is a small fraction). `node_modules` is excluded from discovery but `vendor/` isn't. | Expected: `vendor/` (when `go.mod` + `vendor/modules.txt`) excluded from discovery and parsing by default, like `node_modules`. | `rb-git/src/boundaries.rs` project discovery, `rb-indexer/src/projects.rs:382`, `rb-scip/src/jobs.rs:136`. Related to F13. |

### Phase 3: correctness on real repos

Pins: `var/stress/repos.tsv`. Per-repo evidence: `var/stress/evidence/real/<tag>/`. All 12 repos indexed in ≤ 3.5 s, and two names gave **byte-identical dumps on all 12** (determinism holds).

The audits were done by three read-only agents. I re-verified the ★ items directly (code line plus `rb` output).

| ID | Sev | Area | Finding | Evidence / repro | Suggested fix |
|---|---|---|---|---|---|
| F17 ★ | **S1** ✅ fixed M7a | gRPC / services | **Proto copies become `exact` endpoints of whichever service holds the file.** microservices-demo copies `demo.proto` into several services, so 41 of 50 gRPC endpoints are attributed to services that don't implement them (adservice 14, currencyservice 13, paymentservice 14 wrong). The real Go/Python implementers (productcatalog, shipping, checkout, email, recommendation) get none. `rb check` then fails on these phantom endpoints. | `rb path "GRPC hipstershop.ProductCatalogService/GetProduct" --repo st_r_microservices_demo_a` matches adservice, currency and payment, never productcatalog. | `rb-indexer/src/routes.rs:644-678` `add_spec_endpoints`: require a server registration (`AddXServiceServer`, `addService`, `RegisterXServer`, `bindService`) before `EXPOSES`, or cap at `heuristic` and say "spec declared here, implementer unknown". |
| F18 ★ | **S1** ✅ fixed M7a | FastAPI routes | **`include_router` prefixes from another module are dropped, and routes merge under `exact`.** `api_router.include_router(authentication.router, prefix=…)` (an attribute argument) and `prefix=settings.api_prefix` are skipped. 19 real routes collapse into 12. `DELETE /{}` holds both article-delete and comment-delete (`/api/articles/{slug}/comments/{id}`), both `[exact]`, and `GET /` has 4 unrelated handlers. No `prefix_unknown` signal (Express has one). | `rb path "DELETE /{}" --repo st_r_fastapi_realworld_ex_a` gives 2 exact handlers; `rb find /api --kind endpoint` gives nothing. | `rb-frameworks/src/routes.rs:1386-1424`: resolve `module.router` through imports, and emit `prefix_unknown` (with heuristic certainty) whenever a prefix can't be folded. |
| F19 | **S1** ✅ fixed M7a | gRPC clients (Go) | Protobuf **getters are reported as RPC calls** (`resp.GetProductIds()`, `cart.GetItems()`: 10 of 11 `http_unmatched` sites), while real calls `pb.NewXClient(conn).Rpc(…)` are missed (1 of about 19 found). The false calls create `DEPENDS_ON` edges (shown "via http") from checkout/frontend to ad/currency/payment. | `rb dump --repo st_r_microservices_demo_a \| grep http_unmatched`. Source: `src/checkoutservice/main.go:321,329`, `src/frontend/rpc.go:105`. | `rb-frameworks/src/clients.rs` `generated_calls` `Init=None` branch (about 4150-4175): don't type the result variable as the client. Accept the `pb.NewXClient(c).Rpc` chain root. |
| F20 ★ | S2 ✅ fixed M7c | Express routes | **All 110 bookcars routes are dropped silently.** `routes.route(routeNames.create).post(…)` uses an imported constant object for the path. `routes.rs:754` `continue`s on any non-template path *before* `dynamic()` can emit `route_path_dynamic`, so there's no diagnostic either. The fold machinery that clients use (`fold.rs`) isn't applied to routes. | `rb find / --kind endpoint --repo st_r_bookcars_a` gives 0; `grep -c "routes.route(" backend/src/routes/*.ts` gives 110. | Fold `Arg::Name` / member paths through `fold.rs`. At minimum, emit `route_path_dynamic` instead of `continue`. |
| F21 ★ | S2 ✅ fixed M7c | HTTP clients | **Calls through a default-exported axios instance vanish from every count.** `export default axios.create({baseURL})` plus `import axiosInstance from './axiosInstance'` in 28 files gives about 195 call sites that aren't stitched, `unmatched`, or counted in `sites`. `stitch.rs:563` filters `name != "default"`, then hits `None => continue` (`stitch.rs:415`). The coverage line reads complete ("26 client calls"). | `grep http_coverage …/bookcars/dump_a.jsonl` shows `sites:26`; grep in source shows 195. | Resolve default imports to the module's default export in `imported_instance`. Count unresolved instances as `dynamic` rather than dropping them. |
| F22 | S2 | Services / TS | **Committed build output (`dist/`) is indexed as source.** nestjs-realworld: 70 dist files duplicate every symbol, so `callers ArticleService` becomes "matches 2 symbols", and dist accounts for 954 of the 1,102 unresolved references. That's why `code:` shows 5.4% (about 30% on `src/` alone). | `rb callers ArticleService --repo st_r_nestjs_realworld_exa_a` | Honour `tsconfig` `outDir` (and `exclude`) in the core indexer, as the SCIP job already does. Same family as F13/F16. |
| F23 | S2 | SQL roles | **Every `.sql` file is treated as a migration.** aiosql query files (`app/db/queries/sql/*.sql`, 24 `-- name:` DML queries) become `MIGRATES`, while the real Alembic migration (`op.create_table`) shows as a code `QUERIES alter` site. Callers of the queries aren't linked. | `rb table-deps public.users --repo st_r_fastapi_realworld_ex_a` | `rb-indexer/src/sql.rs:226-251`: classify by location or content (DDL vs DML, migration-dir conventions, `-- name:`), not by extension. |
| F24 | S2 | `rb check` | **`orphan_endpoints` flags almost everything on real repos**, so `rb check` exits 1 and fails CI by default: fastapi 12/12, go-gin 18/18, spring-microservices 13/14 (4 are actually called through the gateway, 4 by the webapp's JS, 5 are Thymeleaf pages), microservices-demo 44/50. It ignores coverage gaps (unsupported clients, `gateway_unread`, dynamic sites) and says nothing about them. The reverse also happens: any `CONSUMES` edge, even an ambiguous or false one (F19), clears an endpoint. | `var/stress/evidence/real/*/check.txt` | `rb-store/src/check.rs:29-125`: gate the rule on client coverage, exempt page routes, require non-ambiguous consumers, and print the coverage caveat. |
| F25 | S3 | Recall, no signal | Supported-stack misses with **no diagnostic and a status line that reads as complete**: routing-controllers routes and typedi DI (11 routes, 0 found); TypeORM repository usage never yields `QUERIES` (express-ts 14 sites, nestjs 51 sites; tables are mapped with 0 code sites); Spring `@HttpExchange` clients (planned in contract-m4a:12, no code); jQuery `$.ajax`/`$.getJSON`; the Python gRPC stub; the Java stub `carrier` uses the variable name (`blockingStub`), so `grpc_target_match` fails (`stitch.rs:691`). | Per-repo `status.txt` | Add an "unrecognised framework" signal: when a manifest dependency is a known framework without a pack, emit `framework_unsupported {name}` and show it in `status`. |
| F26 | S3 | DI (Nest) | `@InjectRepository(Entity) Repository<Entity>` (TypeORM-provided) gives `di_unresolved no_bean` (8 of 13 points) where it should be `external`. The Nest lane has no external-type branch (`rb-indexer/src/di.rs:713`, compared with the Spring lane at `di.rs:1700`). | `grep '"di_' …/nestjs_realworld_exa/dump_a.jsonl` | Add the external branch and treat `@InjectRepository` as a token. |
| F27 | S3 ✅ fixed M7c | gRPC clients (TS) | `new OAuth2Client()` (google-auth-library) plus `client.verifyIdToken()` is labelled a **gRPC** call. Any `new *Client` is treated as a stub with no gRPC/protobuf import check. | `grep '"grpc"' …/bookcars/dump_a.jsonl` | `rb-frameworks/src/clients.rs:4143-4195`: require a `@grpc/*` or generated-pb import. |

| F28 ★ | S2 | DI (.NET) | **.NET DI registrations are dropped in any project using implicit usings** (the SDK default since .NET 6). `dotnet_di` only reads files with an explicit `using Microsoft.Extensions.DependencyInjection` (`rb-frameworks/src/di.rs:971-976`), and nothing in `crates/` reads `<ImplicitUsings>`. eShopOnWeb: every registration in `Program.cs` and `Configure*Services.cs` is lost. Also: classes registered behind an interface (`AddScoped<IBasketService, BasketService>`) never become injection points, because the owner check compares against the *service* type. Open generics (`AddScoped(typeof(IRepository<>), typeof(EfRepository<>))`) are skipped. `event Action` fields are taken as constructor parameters. BCL/framework types (`ILogger`, `IOptions`, `HttpClient`) report `no_bean`, not `external`. | `grep di_ …/e_hop_n_eb/dump_a.jsonl`; `Web.csproj:5`, `ConfigureCoreServices.cs:15-20` | Treat `ImplicitUsings=enable` (csproj) as importing the DI namespace. Match owners against the implementation type argument. Read `typeof` pairs. Add an external-type branch. |
| F29 | S3 | Go / vendor | Beyond F16, vendored packages without their own `go.mod` are counted as **root-service files**: 750 files reported, 36 actually the project's own. The status line `sql: 1 tables; 3 code sites` is entirely `sqlite_master` from `vendor/…/gorm/dialect_sqlite3.go`. 1,059 ambiguous references are all in vendor, from OS/arch build-tag twins (`isatty_bsd.go` vs `isatty_android.go`). Go build constraints aren't modelled. | `…/go_gin_example/status.txt` | Same fix as F16. Optionally pick one build-tag variant (e.g. linux/amd64) per package. |
| F30 | S3 | Honesty | **Unsupported languages and frameworks give silent zeros.** Laravel/PHP: `rb status` shows "code 0.0%… 0 routes" with no hint that 84 `.php` files were skipped. `composer.json` isn't recognised, so the repo becomes a "library" detected via `package.json`. echo (Go), Flask, Kotlin MVC (`@RestController` ×5, `@*Mapping` ×19), ASP.NET controllers and annotation-based Kotlin DI likewise produce "0 routes / 0 injection points", indistinguishable from "there are none". SQLAlchemy queries never become code sites, and `sa.Table` / Alembic `op.create_table` aren't read. | `rb service . --repo st_r_laravel_realworld_ex_a` ("other 113") | One generic mechanism: a `language_unsupported {ext, files}` diagnostic plus a status line, and `framework_unsupported` from manifest deps (pairs with F25). |
| F31 | S4 | Routes / spec | go-gin's "18 routes" includes 3 that exist **only in a stale swagger spec** (`GET /auth`, where the code has `POST`). Status doesn't separate spec-only endpoints. Spans from JSON specs are always `[0,0,0,0]`. `callers <handler> --include-references` shows "(no callers found)" for handlers passed as values (`v1.GetTags`), without saying it's an endpoint handler. | `rb find / --kind endpoint --repo st_r_go_gin_example_a` | Status: "N routes (M from specs only)". Record JSON spec positions. |

| F32 | S3 | Stitching (Express) | Found during M7c. Express matches route paths case-insensitively by default, but stitching compares case-sensitively. bookcars calls `/api/Notifications/…` against the route `/api/notifications/…` (4 sites unmatched). | `rb dump` bookcars \| grep Notifications | Fold case for Express endpoints (unless `caseSensitive` routing is set). |

**Correct on real repos:**
- gin routes: 15/15, including group prefixes and the `r.POST` inside an `apiv1` block trap.
- go-clean-arch: `http_in_process` 5/5 correct.
- MySQL is honestly `unparsed_sql`.
- The gateway gap is honestly flagged as `gateway_unread`.

**Precision of what *was* emitted:**
- bookcars: 8/8 sampled `CALLS` correct.
- nestjs: 5/5 `INJECTS` correct.
- spring-microservices: the one stitched RestClient edge is correct.
- express-ts: 2/2 TypeORM `MAPS_TO` correct.

The recall gaps and the spec/prefix attribution are where it goes wrong.

## Observations pending classification

Each of these gets a repro before it's promoted to a finding.

| Area | Observation |
|---|---|
| CLI | `rb path` rejects `--json` (it uses `--format json`); every other command takes `--json`. |
| CLI | `--pick N` is silently ignored when there is one match: `callers Db --pick 3` returns 0. |
| CLI | `rb find <no match>` exits 0 with empty output and no "no matches" line. |
| CLI | `rb service NAME --limit 5` gives "--limit is not a parameter of this request"; `--limit` is only valid without NAME. |
| CLI | `RB_CYPHER_TIMEOUT_MS=abc` is reported as `--timeout-ms must be…`, not naming the env var. |
| CLI | `RB_TEST_CAPS=garbage` makes production commands fail (exit 1): a test hook is live in release builds. |
| CLI | `cypher --cols a,b` with a mismatch returns AGE's raw "return row and column definition list do not match". |
| CLI | A Cypher write (`DETACH DELETE`) is refused, which is correct, but with the misleading "cannot execute SELECT in a read-only transaction". |
| CLI | `rb diff <other repo's snapshot> <mine> --repo st_demo` diffs across repos. The header labels it, so this is possibly intended, but `--repo` is ignored. |
| Targets | `src/Db.java` (no line), `Db.java:4` (basename), `./src/Db.java:4` and padded whitespace don't resolve. Negative and zero lines are refused properly (contradicts the exploration hypothesis). |
| CLI | `rb find "" --kind endpoint` gives "q is required". There's no way to list every endpoint or table except a hack like `rb find /`. |
| Index | Empty repo (no commits): `Error: resolving revision "HEAD"` plus a gix "Caused by" chain ("rev-spec is malformed…"), exit 1. A plain "repository has no commits" would be clearer. |
| Index | Latin-1 source files are decoded lossily, so `function café()` becomes a `syntax_error` and the definition is lost. UTF-16 files are silently treated as binary (NULs), with no diagnostic. |
| Index | 20k files: 6.8 s and 246 MiB, linear and fine. But one `unresolved_references` diagnostic per file (20,004 diagnostics) floods `rb dump`. |
| Index | HTTP parity of the server against CLI `--json`: **16/16 identical**. Load shedding (`503 busy`), deadline (`503 deadline_exceeded`), 405/415/421 and the 1 MiB body cap all behave as documented. The 100 KB URL gives a bare 414 (known: m5 #23). `%zz` is accepted literally (200), not refused. |

## Phase 4: performance against M5 (no regression)

mattermost `b2ce71db9c18`, `--no-scip`. Log: `var/stress/evidence/bench_mm.log`.

| Run | M5 (contract-m5:488) | HEAD `eb496af` | Load |
|---|---|---|---|
| fresh (`--no-cache`) | 29.2 s, 1.51 GiB | **26.1 s / 25.3 s, 1.55 / 1.60 GiB** | 3.1–5.5 |
| cached | 19.6 s | **20.2 s, 1.78 GiB** | 5.9 |

The graph grew slightly since M5: 58,506 nodes (same) and 169,043 edges (+288). Time is within noise and memory up 3–6%. The cached run peaks **higher** than the fresh one (1.78 vs 1.55 GiB), so the facts-cache read path holds more in memory than parsing does (O4).

## Concurrency and crash consistency (all passed)

Evidence: `var/stress/evidence/concurrency/`.
- **Same repo indexed twice at once:** both exit 0. The row lock serialises the publish, so both publish two snapshots of the same commit. Harmless, but the whole extraction runs twice (an optimization note, O3).
- **Two different repos at once:** both exit 0, and the AGE DDL lock holds.
- **`kill -9` at 0.3/0.8/1.5/2.2/2.8/3.1 s into an index:** the live snapshot stays intact every time, with no orphan rows, graphs or `building` rows. `rb gc` finds nothing to remove, and the next index is healthy. The only residue is an empty `var/age-load/rb_<repo>_<id>` directory per killed run, which `gc` never sweeps (S4, cosmetic).
- **Server reads during 3 publishes:** 400/400 requests succeed, and the snapshot id only moves forward (31425 → 31428). No errors, no torn reads.
- **Blocked until Docker Desktop is restarted** (the daemon is wedged and its CLI hangs; see F2): DB restart mid-query/mid-publish, `gc --egress` during a SCIP fetch, and SCIP failure injection.

## Robustness cases that passed

Generator: `var/stress/scripts/mkrepo.py`. Results: `var/stress/evidence/cases_batch*.tsv`.

These all ran with no crash and the documented diagnostic, at ≤ 1.1 s and ≤ 41 MiB:
- binary-only repo
- submodule (`skipped_submodule`)
- symlink loop and absolute symlink (`skipped_symlink` ×3)
- non-UTF-8 path (`skipped_non_utf8_path`)
- unicode, space and newline names
- 200-deep directories
- 1 MiB boundary: exactly 1 MiB parsed, +1 byte gives `extraction_skipped too_large`
- 1.1 MB minified bundle (`too_large`)
- 100-deep nested classes (`definitions_too_deep`)
- malformed `rb.toml` (TOML error with line and column, run fails as designed)
- LFS pointer
- 2,000 identical modules (one blob, parsed once)
- 2/17/20 services sharing a path: `ambiguous` at 2, `too_ambiguous` at 17 and 20, as designed

## UX simplification proposals

These keep the certainty and evidence model intact. Each removes a step or a surprise seen in this campaign.

| # | Proposal | Motivation (finding) |
|---|---|---|
| U1 | **Remember the checkout.** Store the indexed path and root commit per repo. Infer `--repo` from cwd by matching the git toplevel or root commit, not the directory name. `rb status` with no repo lists all repos. | F1, F3. Today `--repo` is needed on almost every command. |
| U2 | **Forgiving targets.** If `find` yields exactly one symbol, `callers`/`impact` use it. Otherwise answer `ambiguous` with the candidates and copy-pasteable, quoted `--pick` commands. Accept `Class#method`, `Class#method()`, `method()`, `file.java:N` (basename), `./path:N` and trimmed input. | F7, F8. The canonical key syntax (`Db#load().`) is the main stumbling block. |
| U3 | **One error contract.** Every command, `check` and DB-down included, emits the JSON envelope under `--json`. Messages read `--flag: reason` and name only real flags and env vars. Add a dedicated `unavailable` kind and exit code. | F4, F5, F6, obs. (env var names, `--cols`) |
| U4 | **Consistent flags.** `path --json` as an alias of `--format json`. `service NAME --limit` should apply or be hidden. `find --kind X` without text lists everything of that kind. `--pick` beyond the candidate count is an error. `find` with no hits prints "no matches". | obs. |
| U5 | **Bounded, summarised big answers.** `callers`/`impact` on a hub print 4,600–11,000 lines. Add `--limit` with summary-first output (counts per service, per entry kind), and `--exclude-tests` (tests dominate hub callers). | Q-phase (`NewId`: 4,608 lines) |
| U6 | **Say what wasn't analysed.** `status` adds a line: "not analysed: php 84 files; frameworks without packs: echo, flask, routing-controllers". Plus an "excluded: vendor/ 1,020 files" line once F16 is fixed. | F25, F30 |
| U7 | **`rb doctor`.** Checks the DB/AGE version and migrations, Docker reachability *with a timeout*, the context, pinned images, disk floor, and the load/work dirs being absolute. | F2, plus the Phase 0 wedge |
| U8 | **Ignore file for parsing.** `rb.toml` `ignore = [globs]`, honouring `.gitattributes linguist-generated/linguist-vendored`, with defaults for `vendor/` (Go), `dist/` and `build/` (from tsconfig `outDir`). | F13, F16, F22 |
| U9 | **Cypher ergonomics.** Render vertices, edges and paths as JSON. Map `--cols` mismatches to a `--cols` error. Report a write attempt as "cypher is read-only". | F14, obs. |
| U10 | **CI-friendly `check`.** `--rule orphan_endpoints` gated by coverage, with a baseline file (`--baseline`) so existing orphans don't fail CI on day one. | F24 |

## Optimization opportunities

| # | Opportunity | Measured | Expected gain | Risk |
|---|---|---|---|---|
| O1 | Ancestor-aware walks in `rb-syntax` (`mentions`, `calls`): carry parent context instead of `Node::parent()` | Cubic: 1,000-link chain 16 s, 2,000-link 124 s | Linear; removes the only multi-minute path found | Low; local refactor, goldens pin behaviour |
| O2 | Header-first blob reads in `rb-git` (size from the object header; read ≤ 8 KB to sniff, skip > `RAW_SCAN_BYTES`) | 1 GiB blob gives 2,063 MiB RSS | Memory bounded per blob | Low |
| O3 | Single-flight `rb index` per repo (advisory lock at start, or "already indexing" exit) | Concurrent same-repo runs both do the full extraction | 2× wasted CPU avoided; no duplicate snapshots | Low |
| O4 | Cached-run memory: the facts-cache path peaks at 1.78 GiB vs 1.55 GiB fresh on mattermost | +15% RSS when cached | Stream cached facts instead of materialising | Medium |
| O5 | Route mount expansion with cycle detection and memoisation (`routes.rs`) | 183-line file: 44 s, about 1,200 endpoints | Sub-second | Low–medium (path goldens) |
| O6 | Skip vendored/build trees by default (F13/F16/F22) | go-gin: 95% of files are vendor; nestjs: dist duplicates | Graph and index time cut by up to 10× on such repos; fewer false ambiguities | Medium (changes graphs; needs a classified invariant run) |
| O7 | Diagnostic volume: one `unresolved_references` row per file (20k files give 20k rows) | 20,004 diagnostics on the 20k-file case | Aggregate per service/language | Low |

**Healthy, nothing to do:**
- `--no-scip` mattermost is 26 s against M5's 29 s.
- Query p95 is under 2.5 s on a 58k-node graph, and hub `callers --depth 25` takes under 1 s.
- Growth is linear (5k/10k/20k files: 2.0/2.7/6.8 s).
- Load shedding and deadlines are exact.

## Coverage by stack (Phase 3)

| Stack | Routes | Clients | DI | Data | Signal when missing |
|---|---|---|---|---|---|
| gin | ✅ 15/15 incl. groups | n/a | n/a | gorm ✗ (known) | — |
| echo | ✗ 0/4 | — | — | MySQL unparsed (known) | ✗ silent |
| FastAPI | ⚠️ prefixes lost, merged as exact (F18) | test calls honestly dynamic | — | aiosql inverted (F23) | ✗ |
| Flask | ✗ (known) | — | — | SQLAlchemy queries ✗ | ✗ silent |
| Express (`routes.route(CONST)`) | ✗ 0/110 (F20) | default-import axios ✗ (F21) | — | — | ✗ silent |
| routing-controllers / typedi | ✗ 0/11 | — | ✗ | TypeORM queries ✗ | ✗ silent |
| NestJS | ✗ (known) | — | ⚠️ 5/5 correct, 8 mislabelled (F26) | TypeORM queries ✗ | ✗ |
| Spring Boot | ✅ 14 | RestClient ✅; `@HttpExchange` ✗ | ✅ | JPA ✅ | gateway honest |
| gRPC (proto) | ✗ misattributed exact (F17) | Go getters as RPCs (F19) | — | — | ✗ |
| ASP.NET / .NET DI | ✗ (known) | — | ✗ under ImplicitUsings (F28) | EF ✗ (known) | ✗ |
| Kotlin Spring | ✗ (known) | — | annotation DI ✗ (by design) | — | ✗ silent |
| PHP | ✗ (unsupported) | — | — | — | ✗ silent (F30) |

## Environment and docs issues

1. **Green-without-DB.** Without `RB_TEST_DATABASE_URL`, about 130 DB tests "pass" by skipping. The `bfa87ce` regression (F1) shipped through it. Proposal: `cargo test` fails, or at least prints a loud summary, when DB tests skip, unless `RB_ALLOW_SKIP=1` is set.
2. **Disk-full aftermath.** The Docker Desktop daemon stayed wedged after the disk filled (the CLI hangs, while the container and port keep working). Combined with F2, every non-`--no-scip` index hangs. Document "restart Docker Desktop after a disk-full" in the troubleshooting section, and fix F2.
3. **README.** "`rb status` works from anywhere" (README:78) is false at HEAD (F1). `var/m6l/scripts/verify.sh`, referenced in README "Development", doesn't exist; the latest is `var/m6b/scripts/verify.sh`.
4. **Stale headings in `IMPLEMENT.md`.** M6a and M6c say "in progress" though closed, M6d has no heading, and `contract-m6b.md` says "not started".
5. **A foreign `rb serve` process** (pid 65064, debug build, started 11:44) was running throughout. Not touched.

## Campaign hygiene (verified at close)

- `git status`: only `tasks/stress-findings.md` is new. No change under `crates/`, `fixtures/` or `migrations/`.
- DB: all 68 `st_*` campaign repos and their 68 AGE graphs removed in batches of 15 with `rb gc`. Only the pre-existing `alvorabackend` and `fx_*` remain.
- No campaign `rb serve` listener left. The empty leftover load directory was removed.
- Disk never went below 31 GiB free. Bench numbers cite `var/stress/evidence/bench_mm.log`.
- Left in place: `var/stress/` (1.2 GB, gitignored) with scripts, evidence, 13 pinned clones (`repos/`) and synthetic cases (`work/`). `rm -rf var/stress/work var/stress/repos` frees about 1.1 GB. The repro scripts re-create what they need.

## Suggested fix batches (input for a future contract)

1. **Batch A, correctness of `exact` (S1): ✅ done in M7a** (`tasks/contract-m7a.md`, uncommitted): F17 (spec endpoints need an implementer), F18 (FastAPI prefixes plus `prefix_unknown`), F19 (Go gRPC getters). Each with a fixture and goldens. Run invariant 1 on the 12 Phase 3 repos.
2. **Batch B, regressions and hangs: ✅ done in M7b** (`tasks/contract-m7b.md`): F1 (`status` and the red suites), F2 (Docker probe timeout), F9 and F10 (O1), F11 (O5), F15 (O2). Add a CI job with the DB.
3. **Batch C, silent recall losses on supported stacks:** part 1 ✅ done in M7c (F20, F21, F12, F27); part 2 (F28, F26, F23) is M7d: F20, F21, F28, F12, F26, F27, F23.
4. **Batch D, input hygiene:** F13, F16, F22, F29 (U8 / O6). Graph-changing, so classify against the previous close.
5. **Batch E, honesty and UX:** F25, F30 (U6), F24 (U10), F3–F8, F14 (U1–U4, U9), U5, U7.
