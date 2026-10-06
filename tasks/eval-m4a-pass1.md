# M4a evaluator pass 1 (`d2aee54`)

- Date: 2026-10-06.
- Commit evaluated: `d2aee54` (`M4a phase 11: records`). The tree was clean at the start; no tracked file was changed (every mutant was copied aside, edited by script, run, restored and checked with `filecmp`; `git status --short` was empty before this report was written).
- Runtime: Docker Desktop (`DOCKER_CONTEXT=desktop-linux`), database `127.0.0.1:55433`, images `rb-scip-java:0.13.1` (`cbd9963468b4`) and scip-typescript `028c00429913` present; nothing pulled. Disk 9.2 GiB free at the start, 8.4 GiB at the lowest; no `cargo clean`.
- Throwaway material under `var/eval-m4a-1/` (scratch repositories `a1`–`a5`, `gin-edit`, four public clones, an M3b binary built from `git archive 38d55e4` in its own target directory, dumps, scripts), removed at the end, together with every `ev4a1_*` repository and its AGE graphs. All my own index runs used `--no-scip`, so they created no `rb.scip_index` rows (the e2e suite refreshes its own fixture rows by design).

## Measured baseline

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| Unit tests | 419 passed, 0 failed (436 lines summed by awk minus the 17 vacuous e2e lines of the run without a database) |
| e2e with database and containers | 17 passed, 0 failed, 0 `skipping:` lines, 47.7 s |
| Growth-ratio tests (`rb-store` path, `rb-indexer` stitcher + composition, `rb-frameworks` packs), 6 isolated reruns | 18/18 ok |
| spring-petclinic-microservices @ `dc9ca15a7118` | 16 endpoints, 7 sites (5 heuristic, 1 external, 1 dynamic), 5 `CONSUMES`; reproduced |
| node-express-realworld-example-app @ `30b68e1e8814` | 20 endpoints (19 `prefix_unknown`), 1 site (self); reproduced |
| golang-gin-realworld-example-app @ `626c372d2594` | 22 endpoints (19 `prefix_unknown`), 51 sites (44 unmatched, 5 self, 2 dynamic), 0 `CONSUMES`; reproduced |
| **New:** sqshq/piggymetrics @ `6bb2cf9ddbca` (Spring Cloud, 4 Feign clients across services) | 11 endpoints, 4 sites (1 heuristic, 3 unmatched), **1 `CONSUMES`, false** (P2) |

## Criteria and invariants

| # | Verdict | Evidence |
|---|---|---|
| S1 | PASS | rb-syntax units present and passing; not attacked further. |
| S2 | PASS | `every_extractor_module_is_fingerprinted`; `calls.rs` in `fingerprint_input`. |
| S3 | **FAIL** | "An absolute URL splits into scheme, host, port and path" does not hold when the port has a hole: the host is discarded (P1). Table and property tests pass. |
| S4 | PASS (minor P7) | `@Component` + `@RequestMapping` gives no endpoint; `{id:\d+}` → `{}`; `@GetMapping({"/a","/b"})` → two endpoints (scratch `a1`). Profile files and config-server YAML are silently ignored (P7). |
| S5 | PASS | `app.get('env')`, `app.get('port')`, an object method `get`, a string-embedded and a commented route give nothing; app created inside a function, two mounts of one router (`/api/me`, `/v2/me`), `asyncHandler(fn)`, `route().get().post()` all correct (`a1`, `a5`). |
| S6 | PASS (minor P8) | gin groups, `c.Get` misuse ignored, Go 1.22 `GET /items/{id}` and `/items/` subtree correct; client `GET http://inventory/items/5` picks `GET /items/{}` over `ANY /items/{*}`. ServeMux host patterns lose their host (P8). |
| S7 | PASS | Certainties as specified on all dumps. |
| S8 | PASS | Unit `handler_names_resolve_like_calls`; no bare-name link seen on any dump. |
| S9 | PASS (minor P6) | `fetch`, axios, imported `axios.create` instance → `synthetic` with host `users`. A parameter named `axios` is still axios (P6). |
| S10 | PASS | RestTemplate via `@Value` property, `static final` constant, `final` local shadowing a field (folds the local), Feign `url = "${users.url}"` (`a1`). |
| S11 | PASS | `http.Get(fmt.Sprintf("http://users/users/%s", id))` → `synthetic` to `GET /users/{}`. |
| S12 | PASS (test gaps P5) | Local `const` shadowing a top-level constant folds the local; a reassigned `let` stays a hole. Two folding guards have no biting test (P5). |
| S13 | **FAIL** | D6 is not applied to hosts whose port has a hole (P1); the suffix row admits matches whose matched tail has no literal segment (P2); a query concatenated onto a path turns the last segment into `{}` (P3). One synthetic-row condition (`wildcard_not_last`) has no biting test (P5). |
| S14 | PASS | Buckets sum to sites on every dump (`a1`: 12+12+1+1+3 = 29); `rb status` `http:` line equals `http_coverage`. |
| S15 | PASS | Tiers 1–4 work; ambiguous input lists and exits 1; `--pick` out of range re-lists without saying so (cosmetic). |
| S16 | **FAIL** | A second endpoint whose inline handler lives in an already-walked file loses its downstream (P4). |
| S17 | PASS (test gaps P5) | Upstream continuation and the A ↔ B cycle are correct (`a3`); the upstream hop cap and depth flag have no biting test. |
| S18 | PASS | JSON keys `{endpoint, upstream, downstream, histogram, truncated}`, node keys as amended (`branch`, `stitch` added); two runs `cmp`-identical. |
| S19 | PASS | Cycle, depth and growth tests present; mutants removing the fold depth cap, the fold on-path guard, the endpoint visited set, the seen set and the node cap are killed. |
| S20 | PASS | Card sections at query time; e2e goldens; `a1`/`a3` cards consistent with the dumps. |
| S21 | PASS | `rb impact sendBoth` lists `POST /login` at distance 2 (`a3`); goldens pass. |
| S22 | PASS | `path-login.txt` re-derived by hand from `AuthController.java`, `MfaClient.java`, `Login.tsx`, `auth.ts`, `server.ts`: every hop, line, certainty and the histogram (exact 3, synthetic 1, heuristic 4) match. |
| S23 | PASS | Guards and dump invariants pass in e2e; my own invariant script finds no violation on 7 dumps. |
| S24 | PASS | `scip_polyglot_monorepo` ran with containers (fresh `--no-cache` run inside the test), no `skipping:`; `scip-path-login.txt` has the `exact` Java hop. |
| S25 | PASS | Four public repositories: plain, `--force`, `--force --no-cache`, and a second repository name give byte-identical dumps (4 × 4 `cmp`). |
| S26 | PASS | e2e. |
| S27 | **FAIL** | piggymetrics: the one `CONSUMES` is false (0/1 heuristic correct; combined heuristic precision 5/6 = 83%, target ≥ 90%). See precision below. |
| S28 | **FAIL** | Loop is green, but this pass has open blocker and major findings. |
| Inv 1 | PASS | Exactly one `EXPOSES` per endpoint on all dumps; router identity stable under inserted lines, an inserted function and a renamed unrelated function (22/22 keys identical, `gin-edit`); distinct for `users`/`posts` arrow functions, `tags()`, an object method and a callback (`a4`). Judgement on `GET /test`: see P9. |
| Inv 2 | PASS | Property test; not broken by any input tried. |
| Inv 3 | PASS | No `CONSUMES` above `synthetic` on any dump. |
| Inv 4 | PASS | Edge certainty is the minimum of the facts the code sees; the defects in P1–P3 are wrong edges, not upgraded certainties. |
| Inv 5 | PASS (letter), D6 loopback clause FAIL | No `CONSUMES` from a service to itself on any dump; but a loopback call with a port hole is linked to another service (P1), which D6 forbids. |
| Inv 6 | PASS (minor P6) | See S5, S9. |
| Inv 7 | **FAIL** | `https://status.example.com:${port}/orders/1` is path-matched to `orders` (P1). |
| Inv 8 | PASS | Mutants removing span scoping (down and up) are killed. |
| Inv 9 | PASS | See S25; `rb path` JSON identical across runs. |
| Inv 10 | PASS (test gaps P5) | See S19. |
| Inv 11 | PASS | Ratio tests, best of five, 18/18 reruns; the pathological input is a 20 s wall-clock bound as specified. |
| Inv 12 | PASS | Every `EXPOSES`/`HANDLED_BY`/`CONSUMES` row on 7 dumps has evidence. |
| Inv 13 | PASS | M3b binary (`38d55e4`) vs HEAD on all four public repositories: symbol nodes plus `CALLS`/`REFERENCES`/`IMPLEMENTS` rows 438/96/795/1,100, 0 rows differ; every row other than the M4a kinds and `http_*`/`route_*` diagnostics is identical too. |

## Findings

### P1 — blocker: a hole in the port discards the host, so loopback, external and named hosts are path-matched across services

- **Repro** (scratch `a2`: services `users`, `orders` (`GET /orders/:id`), `names`, `inventory`, web; `--no-scip`):
  - `svc/users/src/server.ts`: `await fetch(\`http://localhost:${port}/orders/1\`)` → `CONSUMES … -> HTTP GET /orders/{} @svc/orders [heuristic]`.
  - `apps/web/src/calls.ts`: `await fetch(\`https://status.example.com:${port}/orders/1\`)` → `CONSUMES … -> HTTP GET /orders/{} @svc/orders [heuristic]`.
  - `apps/web/src/calls.ts`: `await fetch(\`http://users:${port}/orders/1\`)` → `CONSUMES … -> HTTP GET /orders/{} @svc/orders [heuristic]` (the host names `users`).
  - Scratch `a1`: `fetch(\`http://localhost:${port}/health\`)` in `orders` → `users`' `GET /health`; `fetch(\`http://127.0.0.1:${process.env.PORT}/users/me\`)` in `orders` → `users`' `GET /users/me`; `fetch(\`http://users:${PORT}/users/me\`)` with `const PORT = 3000` (a number, which does not fold) loses its host too.
- **Observed:** host attribute absent, scope "any service", heuristic edge to whichever other service has the path.
- **Expected:** D6/invariant 7: loopback → bucket `self`; a literal host naming no service → `external`; a literal host naming one service restricts candidates to it. Only the port is unknown in each case.
- **Where:** `crates/rb-frameworks/src/endpoint.rs:245` (`client_url`: `if end == rest.len() && pieces.len() > 1` sets `host_dynamic` whenever the first literal ends before a `/`, i.e. also after `host:`). Unit `client_templates` only tests a hole in the host itself (`http://{?}:8080/x/y`); `hosts_restrict_candidates` likewise.
- **Regression test:** `client_url([lit("http://localhost:"), Hole, lit("/x")])` has host `localhost`, port unknown; stitcher units for loopback, external and named host each with a port hole, plus a fixture line (`fetch(\`http://localhost:${PORT}/internal/mfa/email\`)` in notification) under `must-not-exist`. `http://localhost:${port}` is the most common form of a self call in Node code.

### P2 — major: suffix matches whose matched tail is all parameters link external APIs to internal endpoints (public repository)

- **Repro:** piggymetrics @ `6bb2cf9ddbca`, `--no-scip`. `statistics-service/.../client/ExchangeRatesClient.java`: `@FeignClient(url = "${rates.url}", name = "rates-client")` with `@RequestMapping(method = GET, value = "/latest")`. `rates.url` is `https://api.exchangeratesapi.io` in the config server's YAML (not read, so the base is unresolved).
- **Observed:** `ExchangeRatesClient#getRates() -CONSUMES-> HTTP GET /{} @account-service [heuristic] {match: suffix}`. The client's `latest` matched `account-service`'s `GET /{name}`; `statistics-service`'s own `GET /{accountName}` also matched and was removed as own-service, so the one remaining wrong candidate became a single `heuristic` edge instead of nothing.
- **Expected:** no edge (`unmatched`). A suffix match whose matched endpoint tail contains no literal segment carries no evidence that the client calls that endpoint; at minimum it must not be a single-candidate `heuristic`.
- **Where:** `crates/rb-frameworks/src/endpoint.rs:399–421` (`match_path`: the literal-segment requirement is on the client side only) and `crates/rb-indexer/src/stitch.rs:500–509` (own candidates are discarded before specificity, so the own service's equal match cannot make the site ambiguous). S13's suffix row is followed to the letter; the rule itself is unsound.
- **Regression test:** stitcher unit: client `{?}/latest`, endpoints `GET /{}` in two other services and in the caller's own → `unmatched`; e2e or fixture guard with an unresolved-base Feign client to a one-segment path.

### P3 — major: a query string concatenated onto a path makes the last segment `{}` and links the wrong endpoint

- **Repro:** scratch `a2`: `orders` has `GET /api/items`, `names` has `GET /api/:name`; web `fetch("/api/items" + qs)` → `CONSUMES -> HTTP GET /api/{} @svc/names [heuristic]`. Scratch `a1` (no `/api/{}` anywhere, an SPA with `app.get("*")`): the same call and `` fetch(`/api/items${qs}`) `` → `HTTP GET /{*} @svc/spa [heuristic]`; the right endpoint `GET /api/items @svc/orders` is never a candidate.
- **Observed:** `items{?}` is one segment flagged `partial`, normalised to `{}` (client `{}` matches only endpoint parameters).
- **Expected:** a trailing hole glued to a literal segment is far more often a query/suffix (`?…`) than a parameter; at least the literal-prefix reading (`/api/items`) must be tried, and a partial last segment must not produce a single-candidate edge to an unrelated parameter route.
- **Where:** `crates/rb-frameworks/src/endpoint.rs:315–323` (`client_url`, the `_ => { flags.partial = true; Segment::Param }` arm).
- **Regression test:** stitcher unit with endpoints `/api/items` and `/api/{}` in two services and client `/api/items{?}`.
- Related, by design (D9) but worth an explicit decision: any cross-service catch-all (`app.get("*")` SPA fallback, Go `/`) absorbs every otherwise unmatched relative call as a single `heuristic` edge (`a1`: `fetch("/totally/unknown/path")` → `GET /{*} @svc/spa`).

### P4 — major: `rb path` drops the downstream of a second endpoint handled inline in an already-walked file

- **Repro:** scratch `a3`: `notify/src/server.ts` has two inline handlers, `POST /email` (inserts `email_log`) and `POST /sms` (inserts `sms_log`); `auth`'s `sendBoth()` fetches both (`http://notify/email`, `http://notify/sms`, POST).
  ```
  rb path "POST /login"
  …
      → CONSUMES POST /email  [notify]  [synthetic, branch heuristic]  (host notify)
        ⇒ svc/notify/src/server.ts (top-level code)  [notify]  [exact, branch heuristic]
          → QUERIES public.email_log (insert)  [exact, branch heuristic]
      → CONSUMES POST /sms  [notify]  [synthetic, branch heuristic]  (host notify)
        ⇒ svc/notify/src/server.ts (top-level code)  [notify]  [exact, branch heuristic]
  ```
  `QUERIES public.sms_log` is missing (it is shown by `rb path "POST /sms"`), `collapsed` is 0 and `truncated` is empty, so nothing tells the reader. JSON shows the same.
- **Where:** `crates/rb-store/src/path.rs:366` (`downstream`: `if self.seen.insert(c.node)` — the `File` was already seen with the first endpoint's `handler_spans`, so the second endpoint's spans are never walked).
- **Regression test:** path unit on a built graph: one `File` handling two endpoints with disjoint `handler_spans`, each reaching a different table, both reached through `CONSUMES` from one handler; assert both tables appear.

### P5 — minor: guards with no biting test (mutation results)

Each mutant was run against the crate's unit tests; the `rb-store` survivors also against the e2e `request_path_goldens` and `code_question_goldens`.

| Mutant | File | Result |
|---|---|---|
| upstream hop cap `hop < hops` → `<=` | `rb-store/src/path.rs` | survives unit and e2e |
| upstream `truncated: depth` never set | `rb-store/src/path.rs` | survives unit and e2e |
| downstream ordering without certainty | `rb-store/src/path.rs` | survives unit, killed by e2e golden |
| `!url.flags.wildcard_not_last` removed from the synthetic row | `rb-indexer/src/stitch.rs` | survives |
| `!m.suffix` removed from the synthetic row | `rb-indexer/src/stitch.rs` | survives; equivalent (a suffix match always has no host, so `Scope::Only` already fails) |
| memoise results computed while a guard was hit | `rb-frameworks/src/fold.rs` | survives (amendment 3 claims "only results computed without hitting any of the three are memoised") |
| fold step budget removed | `rb-frameworks/src/fold.rs` | survives |

Killed (for the record): loopback off, host naming own service off, `Scope::Only` filter off, each of prefix-unknown/partial/method/origin/wildcard/endpoint-cap in the synthetic row, candidate cap `>=`, method preference, amendment 9, own-service partition, proxy-as-synthetic, external→unmatched, dynamic check; node cap, downstream hop cap, downstream depth flag, endpoint visited set, both span scopings, collapse, crossing, seen set; fold depth cap, on-path guard, mutable folding, relabels.

### P6 — minor: an `axios` parameter shadowing the import is still treated as axios

`a1`: `export async function axiosShadowed(axios: { get(u: string): void }) { axios.get("/users/me"); }` → `CONSUMES -> HTTP GET /users/me @svc/users [heuristic]`. `crates/rb-frameworks/src/clients.rs:284` tests the receiver name against the import names without checking that no binding shadows it (`fetch` does this check). Invariant 6 says recognition goes through the import.

### P7 — minor: Spring configuration outside `application.properties` is silently ignored

- `a1`: `application-prod.properties` sets `users.url=http://legacy-users.example.com`; the `@Value("${users.url}")` call stays `synthetic` to `users`, with no diagnostic.
- piggymetrics: every service sets `server.servlet.context-path` in the config server's `config/src/main/resources/shared/<service>.yml`; all 11 endpoints are `exact` without their deployed prefix (`/accounts`, `/statistics`, `/notifications`, `/uaa`), no `spring_config_unread` (only `application*.yml` beside the service's own resources is checked, `crates/rb-indexer/src/lib.rs:234`), and the three Feign calls that use the real paths are `unmatched`.
- Lesson 2026-10-01 ("a rule that suppresses a fallback must count what it suppressed"): count profile files and `bootstrap.yml`/config-server YAML that mention `context-path` or a key a `@Value` reads.

### P8 — minor: Go `ServeMux` host patterns lose their host

`a2`: `mux.HandleFunc("example.com/hosted", h)` → `ANY /hosted`, `mux.HandleFunc("GET api.example.com/v2/things", h)` → `GET /v2/things`, with no flag or diagnostic (`crates/rb-frameworks/src/routes.rs:869`, by design per its unit test). A call `http://inventory/hosted` would be stitched although Go would not serve it.

### P9 — minor (judgement asked for): known-prefix routes of different servers in one service share a node

gin-realworld `GET /test` is one node with three `HANDLED_BY` (three `gin.New()` engines in three test functions). D1 allows it, and since all handlers are in one service it cannot cause a cross-service false stitch. It does mean a node does not denote one route (the lesson recorded for F3), e.g. an admin server and a public server in one service registering `GET /health` merge. Not a correctness defect under D1; record it as an accepted residue in the contract.

### P10 — minor: dead code

`rb_frameworks::fold::fold_placeholders` (`crates/rb-frameworks/src/fold.rs:105`) is `pub` and has no caller, test included.

### Suspected (not reproduced)

- A `WebClient`/`RestClient` whose `baseUrl` is set in a constructor (`this.client = builder.baseUrl(x).build()`) has no base, so `uri("/owners")` is matched as an exact relative path (`match: exact`) instead of as an unknown base (suffix).
- Services that expose endpoints are classified `role = library` when they have no start script (`a3`'s four Express services); D8 then follows `CALLS` into them as in-process code instead of "crosses into". Pre-existing role rule, but D8 now depends on it.

## Precision samples

Sampling rule: endpoint keys sorted bytewise, N = ⌊count/20⌋, indices ⌊N/2⌋ + i·N (all when count < 20). Each item read in the source (method, path as composed, handler, registration line).

| Repository | Endpoints | Verdict |
|---|---|---|
| spring-petclinic-microservices @ `dc9ca15a7118` | 16 (all) | **16/16** correct (class mappings `/owners`, `/vets`, `/api/gateway`, `/` checked; `/fallback` GET and POST from one `method = {…}`) |
| node-express-realworld @ `30b68e1e8814` | 20 (indices 0–19) | **20/20 correct as labelled** (19 `prefix_unknown` tails on their routers, inline handler spans at the registration; `GET /` in `main.ts` full path) |
| golang-gin-realworld @ `626c372d2594` | 20 (indices 0–19 of 22) | **20/20 correct as labelled** (17 router-keyed tails, `GET /api/ping` → `main()` inline via `r.Group("/api/ping")`, `GET /test` ×3 tests, the test's root `POST /login`) |
| piggymetrics @ `6bb2cf9ddbca` | 11 (all) | **11/11** method, controller path and handler; **0/11** deployed full path (context path in config-server YAML, unflagged; P7) |

`CONSUMES` (all, since every repository has ≤ 20; there is no `synthetic` and no `ambiguous` edge on any public repository):

| Edge | Verdict |
|---|---|
| petclinic `VisitsServiceClient#getVisitsForPets` → `GET /pets/visits` (visits) | correct (`hostname + "pets/visits?petId=…"`, mutable field) |
| petclinic `AIDataProvider#addOwnerToPetclinic` → `POST /owners` (customers) | correct |
| petclinic `AIDataProvider#addPetToOwner` → `POST /owners/{}/pets` (customers) | correct |
| petclinic `AIDataProvider#getAllOwners` → `GET /owners` (customers) | correct |
| petclinic `VectorStoreController#loadVetDataToVectorStoreOnStartup` → `GET /vets` (vets) | correct (`vetsHostname` is a non-final local) |
| piggymetrics `ExchangeRatesClient#getRates` → `GET /{}` (account-service) | **false** (external exchange-rate API; P2) |

Heuristic: 5/6 = 83% (target ≥ 90%: **missed**). Synthetic: none on public repositories; on scratch `a1` 12/12 `synthetic` edges were checked and are correct (constant, local `const` shadowing, `@Value` property, Feign `${…}` property, Go `Sprintf`, imported axios instance, upper-case host, trailing slash, query and fragment).

## Closing readiness

- **Blockers:** P1.
- **Major:** P2, P3, P4.
- **Minor / follow-ups:** P5–P10 and the suspected items.
- **Open-items list:** not complete. It describes every gap as "a missed or less precise result, never a false `synthetic`", which is literally true but hides that this pass found false `heuristic` stitches on a public repository (P2) and from common idioms (P1, P3), and a silently wrong `rb path` answer (P4). Missing from it: P1–P4, P7's profile/config-server gap (open item 3 covers only service-discovery names), P8, P9, and the untested upstream caps and fold guards (P5). Open item 5 is honest about the own-service exclusion; P2 shows that exclusion also turns an ambiguous own/other match into a single wrong `heuristic` edge.
- Baseline claims (fmt, clippy, 419 + 17, three public repositories, determinism, invariant 13) are reproduced.

VERDICT: FAIL
