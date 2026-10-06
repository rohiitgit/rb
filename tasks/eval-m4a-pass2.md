# M4a evaluator pass 2 (`7d12001`)

- Date: 2026-10-06.
- Commit evaluated: `7d12001` (`Fix M4a evaluator pass 1: hosts, suffix evidence, opaque tails, path scopes`), contract amendment 11, `PIPELINE` `m4a-http-5`. Tree clean at the start. No tracked file was changed: every mutant was applied by script to a copy-aside file, the crate's tests run, the file restored and compared byte for byte; `git status --short` was empty after every batch and before this report was written. The test binaries were rebuilt from the restored sources (`cargo test --no-run`) before any timing measurement that followed a mutant.
- Runtime: Docker Desktop (`DOCKER_CONTEXT=desktop-linux`), database `127.0.0.1:55433`; nothing pulled. Disk 8.8 GiB free at the start, 8.0 GiB at the lowest; no `cargo clean`.
- Throwaway material under `var/eval-m4a-2/`: scratch repositories `b1` (TS host/URL forms), `b2` (Java/Go forms, Spring config), `b3` (`rb path` scopes and cycles), `b4` (nested placeholder, config client key, `ServeMux` host), five public clones, a `d2aee54` and a `38d55e4` release binary built from `git archive` in their own target directory, dumps and the mutation harness. Removed at the end with every `ev4a2_*` repository and its AGE graphs. All my index runs used `--no-scip` (no `rb.scip_index` row created by me; the e2e suite refreshes its own fixture rows by design).

## Measured baseline

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| Unit tests | **437** passed (rb 14, rb-frameworks 65, rb-git 26, rb-indexer 158, rb-model 11, rb-scip 40, rb-sql 27, rb-store 25, rb-syntax 71), summed from per-binary `test result` lines of a `> log 2>&1` run |
| e2e with database and containers, run twice | **17/17** both runs, 0 `skipping:` lines, 51.5 s and 48.2 s |
| Stub scan | only `rb-sql/src/lib.rs:1111` (a test string); `fold_placeholders` gone; no unused `pub fn` in the changed modules |
| petclinic-microservices @ `dc9ca15a7118` | 16 endpoints, all `prefix_maybe_incomplete`; 7 sites (5 heuristic, 1 external, 1 dynamic); 5 `CONSUMES` byte-identical to `d2aee54`; 8 `config_unread` — **reproduced** |
| node-express-realworld @ `30b68e1e8814` | byte-identical dump to `d2aee54` — **reproduced** |
| golang-gin-realworld @ `626c372d2594` | byte-identical dump to `d2aee54` — **reproduced** |
| piggymetrics @ `6bb2cf9ddbca` | 11 endpoints flagged, 9 `config_unread`, 4 sites all `unmatched`, 0 `CONSUMES` (the false `ExchangeRatesClient#getRates()` edge gone, site `{?}/latest`) — **reproduced** |
| **New:** microservices-patterns/ftgo-application @ `558dfc53b11d` (Gradle; Spring controllers in 7 services, a WebClient API gateway, a Node GraphQL gateway) | 18 endpoints, 3 sites (1 ambiguous → 2 edges, 2 self), 0 `config_unread` |
| Determinism | all five public repositories: `--force --no-cache` == `--force` == second repository name, `cmp`-identical; `rb path` text and JSON identical across two runs on `b3` |
| Invariant 13 | `38d55e4` (M3b) binary vs HEAD, symbol nodes + `CALLS`/`REFERENCES`/`IMPLEMENTS` rows: 438 / 96 / 795 / 1,100 / 2,813, 0 rows differ on all five |
| Dumps | no host path (`/Users/`, `/private/`, `/tmp/`) in any dump |

## Criteria and invariants

| # | Verdict | Evidence |
|---|---|---|
| S1 | PASS | not attacked beyond the forms below. |
| S2 | PASS | fingerprint unit present and passing. |
| S3 | PASS (see Q2) | Port-hole forms keep the host (`[::1]:`, `127.0.0.1:`, `0.0.0.0:`, upper case `HTTP://LOCALHOST:`, Java `String.format`, Go concatenation). A hole glued to a literal host fragment discards the host and keeps an *exact* path (Q2); the record says the unresolved-base rules apply, they do not. |
| S4 | PASS (minor Q3) | `config_unread` emitted for profile, YAML, bootstrap and config-client services on both Spring repositories; config servers counted as `yaml` only. `classpath:/config/` files are ignored entirely (Q3). |
| S5–S8 | PASS | unchanged; ftgo 18/18 endpoints correct. |
| S9 | **FAIL** | axios verb calls ignore the request config (`axios.get(url, { baseURL })`, `axios.post(url, body, { baseURL })`): Q1. |
| S10 | PASS (minor Q1) | Suspected item 1 fixed (constructor-set bases are unknown → suffix). A `RestTemplate` whose `rootUri` is in a visible initializer is still read as base-unknown (Q1). |
| S11, S12 | PASS | Go `Sprintf`/concatenation folding; `results_cut_by_a_guard_are_not_memoised` and `the_step_budget_bounds_unclean_diamonds` kill their mutants. |
| S13 | **FAIL** | A false `synthetic` edge (Q1, `f18`), false `heuristic` edges from a literal external base ignored (Q1) and from host holes adjacent to literal host fragments (Q2). |
| S14 | PASS | `b1`: 2 + 12 + 0 + 0 + 1 + 5 + 5 + 1 = 26 sites; `b2` 14; buckets sum on every dump. |
| S15 | PASS | not re-attacked. |
| S16 | PASS (minor Q7) | P4 fixed: `rb path "POST /login"` on `b3` shows `QUERIES public.sms_log` under `POST /sms`. |
| S17 | PASS | upstream continues into each inline handler of one file; cycle through two inline handlers in two files that call each other (`b3`: `fa` ⇄ `fb`, `/a`, `/b`, plus `auth` ⇄ `loop` CONSUMES cycle) terminates in both directions with `--hops 8 --depth 25 --full`. |
| S18 | PASS | JSON `cmp`-identical across runs; order `(certainty desc, via, key)`, deduplicated per `(via, node)`: total. |
| S19 | PASS (minor Q4, Q5) | P5's five mutants now killed. Growth tests are load-sensitive (Q4). |
| S20–S23, S25, S26 | PASS | e2e twice, no golden changed. |
| S24 | PASS | `scip_polyglot_monorepo` ran with containers in both e2e runs, no `skipping:`. |
| S27 | PASS on the public repositories | Heuristic 5/5 (petclinic), ambiguous 2/2 real candidates (ftgo), 0 synthetic; endpoints 16/16, 20/20, 20/20, 11/11, 18/18 (see precision). The false edges of Q1/Q2 come from scratch layouts, not from these repositories. |
| S28 | **FAIL** | Loop green, but Q1 (blocker) and Q2 (major) are open. |
| Inv 1, 2 | PASS | unchanged. |
| Inv 3 | PASS | no `CONSUMES` above `synthetic`. |
| Inv 4 | **FAIL** | `b1` `f18` is `synthetic` while the call states another base (Q1). |
| Inv 5 | PASS | no `CONSUMES` to the caller's own service on any dump. |
| Inv 6 | PASS | P6 fixed (mutant killed). |
| Inv 7 | **FAIL** | A literal external base in the call (`baseURL: "https://api.stripe.com"`) is path-matched (Q1); `https://${tenant}.zendesk.com/…`, `https://api.stripe.com${v}/…`, `http://api.${d}/…` are path-matched across services (Q2). |
| Inv 8 | PASS (minor Q7) | span scoping of handler roots holds; a callee that registers inline handlers is walked whole. |
| Inv 9 | PASS | see baseline. |
| Inv 10 | PASS | `b3` cycles terminate. |
| Inv 11 | PASS (minor Q4) | tests exist and pass isolated; three of four fail intermittently under load. |
| Inv 12 | PASS | evidence on every new edge. |
| Inv 13 | PASS | five repositories, 0 rows differ. |

## Pass-1 findings

| # | Pass 1 | Status at `7d12001` | Evidence |
|---|---|---|---|
| P1 | blocker | **Fixed as stated; adjacent forms open (Q2).** | `b1` `f01`–`f03`, `f05` → `self` (`[::1]`, `127.0.0.1`, `0.0.0.0`, `LOCALHOST`); `f23` `https://api.github.com:${port()}/…` → `external` (was heuristic at `d2aee54`); `b2` Java `String.format("http://localhost:%d/…")` and Go `"http://localhost:" + port + …` → `self`. Mutant M01 killed (3 units). Mutant M02 (bracketed IPv6 followed directly by a hole) **survives** unit and e2e (Q5). |
| P2 | major | **Fixed.** | piggymetrics edge gone (site `unmatched`). M04 (tail literal not required) and M09 (own-service ranking) killed. `b1` `relOwnTie` (own `/profile/:id` ties orders') → `self`. |
| P3 | major | **Fixed.** | `b1` `` fetch(`/api/items${qs}`) `` → `GET /api/items @svc/orders` (was `GET /api/{} @svc/names`); `/orders/ord_${id}` still reaches `GET /orders/{}` (an endpoint `{}` matches the kept literal). M05, M06, M07, M10 killed. |
| P4 | major | **Fixed** (both directions). | `b3` (`/email` + `/sms` repro): `sms_log` present downstream; upstream `client fb()` reaches `⇐ GET /b` and, through `fa()`, `⇐ GET /a`. M20, M22 killed; M21 (upstream root key without scope) **survives** unit and e2e (Q5). |
| P5 | minor | **Fixed.** | All five pass-1 survivors killed (upstream hop cap, upstream depth flag, `wildcard_not_last`, memoising guard-cut results, step budget). |
| P6 | minor | **Fixed.** | M08 killed (`axios_calls_configs_and_instances`). |
| P7 | minor | **Partly fixed (Q3).** | Detection works on petclinic and piggymetrics; misses YAML flow mappings and `src/main/resources/config/`, and counts `spring.cloud.config.enabled=false` as a client. M11, M12, M16, M17, M18, M19 killed. |
| P8 | minor | **Fixed** (open item 13 wording inaccurate, Q8). | `b4`: `mux.HandleFunc("GET users/things", h)` → endpoint attribute `host: "users"`; M13 killed. |
| P9 | minor | Recorded (open item 12). | — |
| P10 | minor | **Fixed.** | no `fold_placeholders` in the workspace. |
| Suspected 1 | — | **Fixed for constructor bases**; visible `rootUri` still ignored (Q1). | `b2` `j3`/`j4` (constructor) → suffix; `j4b` field `WebClient.create("https://api.stripe.com")` → `external`; `j3b` field `new RestTemplateBuilder().rootUri("https://api.stripe.com").build()` → suffix heuristic to `orders`. |
| Suspected 2 | — | **Fixed.** | unit `a_library_role_service_that_exposes_endpoints_is_crossed_into`. |

### Mutation results (my harness: scripted edit, the crate's `--lib` tests, plus `rb-cli --test e2e` with database and containers for the survivors; every run printed its `test result` lines)

Killed: M01 port hole → host unknown; M03 userinfo ignored; M04 suffix tail literal; M05 opaque tail not dropped; M06 not flagged; M07 glued segment not `partial`; M08 axios shadowing; M09 own-service ranking `any` → `all`; M10 opaque tail not capped; M11 overridden site not capped; M12 maybe-incomplete endpoint not capped; M13 endpoint host ignored; M14 `[::1]` dropped from loopback; M15 `0.0.0.0` dropped; M16 config-server exclusion removed; M17 build-file client detection off; M18 `bootstrap*` ignored; M19 YAML pop `>=` → `>`; M20 path downstream scope key → `None`; M22 path upstream inner scope key → `None`; M24 `join` always adds an unknown base; P5a–P5e.

Survive unit **and** e2e: **M02** (`authority_before_hole`: bracketed IPv6 + hole → `InHost`), **M21** (`upstream`: root visited key without scope), **M23** (`site`: a host naming the caller's own service no longer `self`; pass 1 recorded this mutant as killed — the P2 ranking now hides it whenever the own service exposes endpoints, and no test has an own service that exposes none).

## Findings

### Q1 — blocker: a base stated in the call is ignored; via an axios instance the result is a false `synthetic` edge

- **Repro** (`b1`, `apps/web/src/calls.ts`, `import axios from "axios"`, `export const api = axios.create({ baseURL: "http://orders" })`; `svc/orders` exposes `GET /orders/:id` and `POST /charges`):
  - `f18`: `await api.get("/orders/1", { baseURL: "https://api.stripe.com" })` → `CONSUMES -> HTTP GET /orders/{} @svc/orders` **`[synthetic]`** `{host: orders, base: literal, match: exact}`; `rb path f18` prints `client f18() … [synthetic] (host orders)`.
  - `f09`: `await axios.get("/orders/1", { baseURL: "https://api.stripe.com" })` → `GET /orders/{} @svc/orders [heuristic]`.
  - `f19`: `await axios.post("/charges", body, { baseURL: "https://api.stripe.com" })` → `POST /charges @svc/orders [heuristic]`.
  - Control: `f11` `axios({ url: "/charges", method: "post", baseURL: "https://api.stripe.com" })` → `external` (the config form is read).
  - `b2` `j3b`: `private final RestTemplate rootRt = new RestTemplateBuilder().rootUri("https://api.stripe.com").build();` then `rootRt.postForObject("/charges", …)` → `POST /charges @svc/orders [heuristic] {match: suffix}`.
- **Observed vs expected:** axios merges the request config over the instance defaults, so all three calls go to `api.stripe.com`. Expected `external` (invariant 7: a literal host naming no service is never path-matched); at the very least never `synthetic` (invariant 4: the base the edge relies on is contradicted by a fact in the same call).
- **Pre-existing:** `d2aee54` gives the same three edges (`f18` synthetic). Pass 1 did not test it.
- **Where:** `crates/rb-frameworks/src/clients.rs:306` (`axios_call(chain, base, instance, None, None)` for verb calls: the 2nd argument of `get/delete/head/options` and the 3rd of `post/put/patch` are never read, so `baseURL` (and `url`/`method` there) are lost; `axios_call` reads `baseURL` only from the first-argument config). `clients.rs:410` (`RestTemplate` base is always `UrlValue::Unknown`, even when the receiver's initializer chain has `rootUri(lit)`; the `WebClient` pack reads `baseUrl`/`create` from that chain, `RestTemplate` does not).
- **Regression tests:** clients unit: `axios.get(u, { baseURL: lit })`, `axios.post(u, d, { baseURL: lit })`, `api.get(u, { baseURL: lit })` on an instance → base `lit`; stitcher unit: those three with an external literal → `external`; a guard line in a fixture (`must-not-exist`) for an instance call overriding `baseURL`. `RestTemplate`: read `rootUri(lit)` from the receiver binding's initializer chain like `fluent_base`.

### Q2 — major: a hole next to a literal host fragment drops the host and matches the path *exactly* across every service

- **Repro** (`b1`/`b2`; `svc/orders` exposes `GET /orders/:id`, `POST /charges`, `GET /api/v2/tickets`):
  - `` fetch(`https://${tenant}.zendesk.com/api/v2/tickets`) `` → `GET /api/v2/tickets @svc/orders [heuristic] {match: exact}`.
  - Java `rt.getForObject(String.format("https://%s.zendesk.com/api/v2/tickets", tenant), …)` → same edge. **New in `7d12001`:** at `d2aee54` `String.format` was not a template and the site was `dynamic`, so amendment 11's Java form opened this false edge.
  - Go `http.Get(fmt.Sprintf("https://%s.zendesk.com/api/v2/tickets", t))` → same edge.
  - `` fetch(`https://api.stripe.com${ver}/charges`, { method: "POST" }) `` and Go `http.Post("https://api.stripe.com"+ver+"/charges", …)` → `POST /charges @svc/orders [heuristic]`.
  - `` fetch(`http://api.${domain}/orders/1`) `` → `GET /orders/{} @svc/orders [heuristic]`.
- **Observed vs expected:** `client_url` sets `host_dynamic`, drops everything up to the next `/`, and leaves `base_unknown = false`, so the remaining path is matched exactly against every service (`Scope::Any`). Amendment 11 says that for such holes "the unresolved-base rules apply"; they do not (`match: exact`). D6 matches a host to a service only by equality with a service name, so a host whose literal part contains a dot-separated domain (`.zendesk.com`, `api.stripe.com…`, `api.`) can never name a service: by D6's own logic these are `external` (or at least a counted `unknown host` bucket), never path-matched. Not listed in the open items (item 10 covers relative URLs, suffix tails and D6 names, not host holes; amendment 5's "a host with a hole restricts nothing" is a rule, not a stated false-edge risk). Multi-tenant SaaS URLs (`https://${tenant}.<vendor>.com/…`) are a common idiom.
- **Where:** `crates/rb-frameworks/src/endpoint.rs:273–277` (`AuthorityEnd::InHost` → `host_dynamic`, `skip_authority`); `crates/rb-indexer/src/stitch.rs:506–526` (`host_dynamic` falls through to `Scope::Any`).
- **Regression tests:** `client_url` unit: a literal host fragment containing `.` next to a hole is external (or flagged); stitcher unit with the five shapes above and an endpoint of the same path in another service → no edge; a must-not-exist guard line.

### Q3 — minor: `config_unread` detection misses common Spring forms (P7 fix incomplete)

- **YAML flow mappings:** `b2` `java/billing`: `application.properties` `stripe.url=http://orders`; `application-prod.yml` `stripe: { url: "https://api.stripe.com" }`. `config_unread` is emitted (reason `profile`), but `yaml_keys` lists only `stripe`, so the Feign site folding `${stripe.url}` is not marked: `StripeClient#charge() -CONSUMES-> POST /charges @svc/orders` **`[synthetic]`** — exactly the case amendment 11 says stays below `synthetic`. `crates/rb-frameworks/src/fold.rs:415` (`yaml_key` stops at the first `: `).
- **`classpath:/config/`:** `b2` `src/main/resources/config/application-prod.properties` sets `users.url=https://legacy-users.example.com`; no `config_unread`, and `Clients#j8()` (`@Value("${users.url}") + "/users/me"`) is `synthetic`. Spring Boot reads `classpath:/config/application*.{properties,yml}` by default. `crates/rb-indexer/src/lib.rs:249–269` (`config_file` accepts only files directly in `src/main/resources`).
- **Disabled client counted:** `b4` `application.properties` `spring.cloud.config.enabled=false` → `config_unread {reasons: [config_client]}`; every site of the service is capped (`Pay#p2()` heuristic). Under-claims certainty; no false edge. `lib.rs:293–298`.
- Not reproduced, by reading: Gradle version catalogs (`implementation(libs.spring.cloud.starter.config)`) are not detected as a config client (`lib.rs:346` substring test on the build file only).
- **Regression tests:** `yaml_keys` on a flow mapping; `config_file("svc/src/main/resources/config/application-prod.yml")`; `is_config_client_key("spring.cloud.config.enabled")` with value `false`.

### Q4 — minor: the growth-ratio tests fail under intermittent load

Measured on the test binaries built from `7d12001` (each test run alone; "load" = the other crate's full unit suite re-run in a loop with 0.4 s pauses, the shape of a compile or a parallel test binary starting and stopping):

| Test | Isolated | Under intermittent load | Failing ratios |
|---|---|---|---|
| `rb-indexer` `stitch::tests::the_stitcher_grows_linearly` | 0/6 (also 0/10 under constant 12× `yes` load) | **1/30, 1/25** | 2.54×, 2.71× (31.8 → 80.9 ms) |
| `rb-indexer` `routes::tests::composition_grows_linearly` | 0/20 | **5/25** | 2.60–3.03× (≈11 → 31 ms) |
| `rb-frameworks` `routes::tests::packs_grow_linearly` | 0/10 | **7/20, 9/20** | 2.52–3.39× (≈30 → 100 ms) |
| `rb-store` `path::tests::rb_path_grows_linearly` | — | 0/25 | — |

Best-of-N wall time removes spikes but not a load step that covers one of the two measurements (a few tens of milliseconds each). The generator's two 2.78× failures are this, not noise. Suggested: count operations (candidate confirmations in the stitcher, binding lookups in the packs and composition, as `wide_diamonds_are_memoised` already counts resolutions) and keep at most one coarse wall-clock bound; or interleave small/large measurements (ABAB…) and compare minima.

### Q5 — minor: three rules of amendment 11 have no biting test

M02 (`[::1]` immediately followed by a hole: amendment 11 claims it keeps its host; switching it to host-unknown changes nothing any test sees), M21 (upstream root visited key without scope), M23 (host naming the caller's own service → `self`; with the P2 ranking this is only observable when the own service exposes no endpoint, e.g. a web app calling `http://web/…`, which would become a `Proxy` path-matched across all services). All three survive the unit tests and the e2e suite. Tests: `client_url` row `http://[::1]${p}/x` → host `[::1]`; a stitcher unit with an own service that exposes nothing; an upstream unit where a root `File` is reached again within the same scope.

### Q6 — minor (record): a correct edge lost to open item 11

`b1` `` fetch(`http://orders:3000${p}/orders/1`) `` was `GET /orders/{} @svc/orders [heuristic]` at `d2aee54` and is `unmatched` now (path `{}/orders/1` restricted to `orders`). Open item 11 states the reading but says "no occurrence measured" and not that it removes edges. Every other lost edge I found is correctly lost: `b1` `f01`–`f05`, `f23`, `relOwnTie`; `b2` Go loopback; ftgo's 3 test-only `http://localhost:${port}/orders…` edges (gateway calling itself); piggymetrics' false edge.

### Q7 — minor: a callee that registers inline handlers is walked whole downstream

`b3` `svc/loop`: `fa()` registers `ra.get("/a", async () => { fb(); … a_log })`, `fb()` registers `rb.get("/b", async () => { fa(); … b_log; fetch("http://auth/login", POST) })`. `rb path "GET /a"` shows under `fa()` → `fb()` → `QUERIES public.b_log` and `CONSUMES POST /login`, which run only when `/b` is requested: calling `fb()` registers a handler, it does not run its body. Invariant 8 is applied to handler roots only (`crates/rb-store/src/path.rs:449`, `(c.node, None)` for nodes reached by `CALLS`). Pre-existing; the layout (a handler calling a registrar) is unusual. Either scope a `CALLS`-reached node to its code outside its inline handler spans, or record it.

### Q8 — doc: open items and deferrals

- Open item 13 says a host-pattern endpoint is "reached only when its host is a service's name"; it is not reached then either unless that service is the endpoint's own (`b4`: endpoint `GET /things` with `host: users` on `go/hosted`; `http://users/things` → `Scope::Only(svc/users)` → `unmatched`). Missed edge, no false one; the wording is wrong.
- `node-fetch` and `request` are not in the deferral list (scope lists got, superagent, Angular `HttpClient`). ftgo's GraphQL gateway makes all its cross-service calls through `const fetch = require("node-fetch")` (`OrderServiceProxy.js:10`, `ConsumerServiceProxy.js:20,39`, `RestaurantServiceProxy.js:16`): the shadowing rule removes them and no bucket counts them (lesson 2026-10-01).
- Open items do not list Q1–Q3.

## Precision samples

Sampling rule: endpoint keys sorted bytewise, N = ⌊count/20⌋, indices ⌊N/2⌋ + i·N (all when fewer than 20).

| Repository | Endpoints | Verdict |
|---|---|---|
| petclinic-microservices | 16 (all) | **16/16**: rows differ from pass 1's verified `d2aee54` rows only by `prefix_maybe_incomplete: true`, which is correct (every service has `application.yml`, 7 of 8 are config clients by `pom.xml`). |
| node-express-realworld | 20 (all) | **20/20 correct as labelled** (dump byte-identical to the one pass 1 verified). |
| golang-gin-realworld | 20 (indices 0–19 of 22) | **20/20 correct as labelled** (byte-identical to the one pass 1 verified). |
| piggymetrics | 11 (all) | **11/11** method, controller path, handler; flag correct (bootstrap + config client); deployed context paths still absent (open item 3). |
| ftgo-application (new) | 18 (all) | **18/18**: each read in its controller (class `@RequestMapping` + method mapping with `method = RequestMethod.X`; e.g. `OrderController` `/orders` POST, `/{orderId}` GET, `/{orderId}/cancel`, `/{orderId}/revise` POST; `KitchenController` `POST /tickets/{}/accept`; `DeliveryServiceController` two class-less mappings); the GraphQL gateway's `GET`/`POST /graphql` have no `HANDLED_BY` (handler is a call result, `makeGraphQLHandler()`), as labelled. |

`CONSUMES` on the public repositories (all of them; no `synthetic`):

| Edge | Verdict |
|---|---|
| petclinic `VisitsServiceClient#getVisitsForPets` → `GET /pets/visits` (visits) | correct (`hostname + "pets/visits?petId={petId}"`, mutable field, suffix) |
| petclinic `AIDataProvider#getAllOwners` → `GET /owners`; `#addOwnerToPetclinic` → `POST /owners`; `#addPetToOwner` → `POST /owners/{}/pets` (customers) | correct (`getCustomerServiceUri() + "/owners…"`) |
| petclinic `VectorStoreController#loadVetDataToVectorStoreOnStartup` → `GET /vets` (vets) | correct (`vetsHostname` non-final local) |
| ftgo `OrderServiceProxy#findOrderById` → `GET /orders/{}` @order-service and @order-history-service `[ambiguous, candidates 2]` | both real candidates for `orderDestinations.getOrderServiceUrl() + "/orders/{orderId}"` (unknown base); the deployed target is order-service |

Heuristic 5/5, ambiguous 2/2 real, synthetic 0. The repositories exercise none of Q1/Q2's shapes; the scratch layouts do: of `b1`'s 14 edges, 1 `synthetic` and 5 `heuristic` are false (`f18`; `f09`, `f13`, `f14`, `f15`, `f19`) plus `f17` into the SPA catch-all (open item 9, honest); of `b2`'s 8 edges, 2 `synthetic` rest on a property that unread configuration overrides (Q3: `StripeClient#charge`, flow YAML, against amendment 11's own rule; `Clients#j8`, `config/` file, undetected) and 4 `heuristic` are false (`g1`, `g4`, `j2`, `j3b`), plus `j3`/`j4` (constructor-set external bases, suffix-matched; honest under item 10).

## Closing readiness

- **Blocker:** Q1 (false `synthetic` from an axios instance call whose request config overrides `baseURL`; the same root cause gives false `heuristic` edges for `axios.get/post(…, { baseURL })` and a visible `RestTemplate` `rootUri`).
- **Major:** Q2 (host holes next to literal host fragments are matched exactly across services; one shape newly opened by amendment 11's `String.format` support; the amendment's description of this case is wrong).
- **Minor / follow-ups:** Q3 (P7 detection gaps, one giving `synthetic` against the amendment's own rule), Q4 (load-sensitive growth tests), Q5 (three untested rules), Q6 (open item 11 removes a previously correct edge), Q7 (inline handlers of a called registrar walked), Q8 (open-item wording, node-fetch not deferred or counted).
- P1–P10 and both suspected items: fixed except P7 (partly; Q3). The pass-1 baseline, the four recorded repositories, determinism and invariant 13 reproduce exactly.
- **Open-items list:** honest about what it lists (items 9 and 10 reproduce as described), but incomplete: it omits Q1 and Q2, which produce false edges (one `synthetic`), and Q3.

VERDICT: FAIL
