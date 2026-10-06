# M4a evaluator pass 3 (`e06e590`)

- Date: 2026-10-06.
- Commit evaluated: `e06e590` (`Fix M4a evaluator pass 2: call-site bases, partial hosts, counted growth`), contract amendment 12, `PIPELINE` `m4a-http-6`. Tree clean at the start. No tracked file was changed: every mutant was applied by script to a copy-aside file, the crates' `--lib` tests run, the file restored and compared byte for byte; `git status --short` was empty after every batch.
- Runtime: Docker Desktop (`DOCKER_CONTEXT=desktop-linux`), database `127.0.0.1:55433`; nothing pulled. `cargo clean` at the start (7.6 GiB free → 25 GiB), lowest 20 GiB.
- Throwaway material under `var/eval-m4a-3/` (removed at the end): scratch repositories `c1` (axios/fetch/node-fetch forms, partial hosts), `c2` (Spring bases, profile-disabled config client), `c3` (pass-2 Q3 repros, Java/Go partial hosts), `c4` (the Q7 `fa`/`fb` layout), `c5` (leading holes on a client with a base); copies of the three fixtures; six public clones; release binaries of `7d12001`, `d2aee54`, `38d55e4` (each `git archive` built in its **own** clean target directory — a shared target directory silently reused `7d12001`'s artifacts for `d2aee54`, caught by `cmp` and the embedded `PIPELINE` string, and rebuilt) and HEAD. All my index runs used `--no-scip`; every `ev4a3_*` repository and AGE graph was dropped at the end.

## Measured baseline

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| Unit tests | **450** passed (rb 14, rb-frameworks 71, rb-git 26, rb-indexer 162, rb-model 11, rb-scip 40, rb-sql 27, rb-store 27, rb-syntax 72) — reproduced |
| e2e with database and containers, `--nocapture`, run twice | **17/17** both runs, 0 `skipping:` lines (`scip_polyglot_monorepo`, `scip_java_spring_jpa`, `scip_ts_express_prisma` ran), 58.9 s and 65.9 s |
| Stub scan | only `rb-sql/src/lib.rs:1111` (a test string) |
| petclinic-microservices @ `dc9ca15a7118`, node-express-realworld @ `30b68e1e8814`, gin-realworld @ `626c372d2594`, piggymetrics @ `6bb2cf9ddbca` | dumps `cmp`-identical to `7d12001` — **reproduced** |
| ftgo-application @ `558dfc53b11d` | differs from `7d12001` only by `http_coverage` sites 3 → 7 (dynamic 0 → 4) and four `http_url_dynamic` rows (`ConsumerServiceProxy.js` 19/38, `OrderServiceProxy.js` 9, `RestaurantServiceProxy.js` 15, 0-based) — **reproduced** |
| Fixtures (`polyglot-monorepo`, `java-spring-jpa`, `ts-express-prisma`), `--no-scip` | HEAD == `7d12001` == `d2aee54`, `cmp`-identical |
| Determinism | all six public repositories: `--force --no-cache` == `--force` == second repository name, `cmp`-identical; `rb path` text and JSON identical across runs (`c4`) |
| Invariant 13 | `38d55e4` vs HEAD, symbol nodes / `CALLS` / `REFERENCES` / `IMPLEMENTS`: 0 rows differ on all six public repositories (2,956 / 1,725 / 710 / 90 rows) |
| Dumps | no `/Users/`, `/private/`, `/tmp/` in any dump |
| `work-count` leak | `cargo tree -p rb-cli -e features`: no `work-count`; it appears only through rb-indexer's dev-dependency. The symbol `…4work5COUNT…` is in the rb-indexer test binary (10 symbols), absent from the release `rb` and the e2e debug `rb` (0) |

## Criteria and invariants

| # | Verdict | Evidence |
|---|---|---|
| S1, S2 | PASS | spreads (`...`), `Arg::New`, `CallChain.assigned` exercised by `c1`/`c2`. |
| S3 | **FAIL** (R2, R4) | A leading hole is not "the base" when the client has a base: it is joined as a path segment and matched exactly (`synthetic`). Several holes after a port hole are all taken as port. |
| S4 | PASS (R3) | flow YAML, `config/`, `enabled=false` repros correct; the disable rule ignores profiles (R3). |
| S5–S8 | PASS | endpoints 16/16 on the new repository; fixtures unchanged. |
| S9 | **FAIL** (R1) | an axios instance's base is taken from its `create(...)` literal alone: later overrides (`defaults.baseURL`, interceptors, reassignment, a spread after `baseURL` in the create config) are ignored → false `synthetic`. |
| S10 | **FAIL** (R1, R2) | a `WebClient.Builder` mutated by a statement keeps its initialiser's base → false `synthetic`; a leading unknown URL part under a `baseUrl`/`rootUri` → `synthetic`. |
| S11, S12 | PASS | Go `Sprintf`/`+` partial hosts external or suffix as amended. |
| S13 | **FAIL** | false `synthetic` edges (R1, R2), `synthetic` above the stated open-item certainty (R4), `synthetic` for a config client (R3). |
| S14 | PASS | buckets sum on every dump (`c1` 38, `c2` 14, `c3` 12, `c5` 3). Silent non-sites: R8. |
| S15, S18 | PASS | |
| S16 | PASS | Q7 downstream fixed (`c4`). |
| S17 | PASS (minor R5) | upstream still walks a registrar's callers from a site inside its inline handler. |
| S19 | PASS (minor R6, R7) | counted loops bite (my V2/W2 quadratics killed); uncounted quadratics escape (T/U); mutants C and G survive. |
| S20–S26 | PASS | e2e twice, goldens unchanged, `scip_polyglot_monorepo` ran with containers. |
| S27 | PASS on the public repositories | endpoints 16/16 (new), heuristic 9/9, ambiguous 2/2, 0 synthetic; fresh == cached. |
| S28 | **FAIL** | loop green, but R1 and R2 (blockers), R3 and R4 (major) are open. |
| Inv 1, 2, 3, 5, 6, 9, 10, 12, 13 | PASS | |
| Inv 4 | **FAIL** | `synthetic` on bases the same file overrides (R1), on an unknown leading URL part (R2), on unknown holes after a port (R4), on a profile-only config disable (R3). |
| Inv 7 | **FAIL** | `https://api.stripe.com` set as the base by `defaults.baseURL`, an interceptor, a reassignment, a spread, a builder statement, or an env URL in a leading hole is path-matched to `svc/orders` (R1, R2). |
| Inv 8 | PASS (minor R5) | |
| Inv 11 | PASS (minor R6) | counted; the counter measures only instrumented helpers. |

## Pass-2 findings

| # | Pass 2 | Status at `e06e590` | Evidence (scratch repos rebuilt; HEAD vs `7d12001`) |
|---|---|---|---|
| Q1 | blocker | **Fixed as stated; the same root cause stays open in forms the amendment did not cover (R1, R2).** | `c1` `t13` (pass-2 `f18`: `api.get(u, { baseURL: stripe })`) `synthetic` → external; `t11` (`f09`), `t12` (`f19`), `t14` (imported instance), `t22` (`request` config), `t24` (`axios(url, cfg)`) → external; `t06` (config without `baseURL`) stays `synthetic` (correct). `c3` `j3b` (`rootUri(stripe)` field, relative `postForObject`) heuristic → external; `c2` same-file `@Bean` → external, other-file `@Bean` suffix heuristic (item 15, honest), `mutate().baseUrl(stripe)` → external, constructor `rt16 = injected.rootUri(orders).build()` → `synthetic` (correct). My mutants A, B, E, F, H, I, R, S killed; **C (`patch` reads its 2nd argument as config) and G (a fresh client counts as disagreement) survive** (R7). |
| Q2 | major | **Fixed.** | `{?}.zendesk.com`, `api.stripe.com{?}`, `api.{?}` → external in TS (`c1` `t18`), Java `String.format` and Go `Sprintf`/`+` (`c3`); `http://${h}/x`, `http://${h}:8080/x`, `http://${u}@orders/x` → suffix heuristic (items 18, 19, honest). No dotted-fragment case produced an edge; the dot rule only removes edges. In-cluster names (`{?}.default.svc.cluster.local`, `orders.{?}.svc`) and the fully literal `orders.internal` are external (missed, counted; item 18 honest; the literal FQDN case is items 3/10's D6 convention). Mutants J, K killed. |
| Q3 | minor | **Fixed for the three repros; the new disable rule over-reaches (R3).** | `c3` `k1` (flow-mapped profile YAML) Feign `synthetic` → heuristic; `k2` (`config/application-prod.properties`) → heuristic; `k3` (`enabled=false` in `application.properties`, no client) → `synthetic`, no `config_unread` (correct). Mutants M, N, O killed. |
| Q4 | minor | **Fixed (load-proof), with a narrower guarantee (R6).** | Semantics-preserving quadratics of counted loops fail: V2 (exact lookups examine every endpoint) kills `the_stitcher_grows_linearly`; W2 (binding lookup examines every local) kills `packs_grow_linearly`. The counter is not trivially linear. |
| Q5 | minor | **Fixed.** | rows/tests present and passing; not re-mutated beyond the above. |
| Q6 | minor | **Fixed; the residue is misstated (R4).** | `c1` `t20` and `c3` `g4` (`http://orders:3000${p}/orders/1`) → `GET /orders/{} @svc/orders [heuristic] {match: suffix, host: orders}`. Mutant L killed. |
| Q7 | minor | **Fixed downstream; upstream mirror open (R5).** | `c4`: `rb path "GET /x/a"` no longer shows `b_log`/`POST /login`; `GET /y/b` shows both; nested inline `/nested` scoped. Mutant P killed. |
| Q8 | doc | **Fixed.** | `c1` `svc/gw`: `const fetch = require("node-fetch")` → `synthetic`; `import nf from "node-fetch"` → `synthetic`; shadowed `fetch` parameter and shadowed `nf` local → no site. Mutant Q killed. Open item 13 reworded correctly. README's client list still omits `node-fetch` (R8). |

## New findings

### R1 — blocker: an axios instance's (or Spring builder's) base is read from its creation only; a later override in the same file gives a false `synthetic`

- **Repro** (`c1`, `apps/web/src/calls.ts`, `import axios from "axios"`; `svc/orders` (Express) exposes `GET /orders/:id`; every instance below is created `axios.create({ baseURL: "http://orders" })`):
  - `t01`: `api1.defaults.baseURL = "https://api.stripe.com";` then `api1.get("/orders/1")` → `CONSUMES -> HTTP GET /orders/{} @svc/orders` **`[synthetic]`** `{host: orders, base: literal, match: exact}`.
  - `t02`: `api2.interceptors.request.use((c) => { c.baseURL = "https://api.stripe.com"; return c; })` → same edge **`[synthetic]`**.
  - `t03`: `let api3 = …; export function t03setup() { api3 = axios.create({ baseURL: "https://api.stripe.com" }); }` → **`[synthetic]`**.
  - `t04`: `const stripeOpts = { baseURL: "https://api.stripe.com" }; const api4 = axios.create({ baseURL: "http://orders", ...stripeOpts });` → **`[synthetic]`** — the exact spread rule amendment 12 applies to request configs ("an object whose last `baseURL`-or-spread entry is a spread … leaves the base unknown") is not applied to the create config (`clients.rs:288`, `entry()` ignores spreads).
  - `t15`: `apps/web/src/client.ts` exports `shared2 = axios.create({ baseURL: "http://orders" })` and sets `shared2.defaults.baseURL = "https://api.stripe.com"`; `calls.ts` `shared2.get("/orders/1")` → **`[synthetic]`**.
  - Spring form (`c2`, `Clients.java`): `private final WebClient.Builder b1 = WebClient.builder().baseUrl("http://orders");`, constructor `b1.baseUrl("https://api.stripe.com");` (the builder is mutable), `b1.build().get().uri("/orders/1")` → `Clients#j01() -CONSUMES-> GET /orders/{} @svc/orders` **`[synthetic]`**.
- **Observed vs expected:** all six requests go to `api.stripe.com`. Expected: external, or at least base unknown (suffix, `heuristic`); never `synthetic` (invariants 4, 7). Amendment 12 reads "everything in the file that configures the receiver's binding" for Spring (assignments, `setUriTemplateHandler`) but not a builder's own mutating calls, and nothing of the kind for axios instances (no `defaults.baseURL` member assignment, no interceptor, no reassignment of a mutable binding — S12 says mutable bindings never fold, yet a `let` instance's base is trusted).
- **Pre-existing:** `7d12001` gives the same six edges. Not in any open item (grep: no `defaults`, `interceptor`, reassignment).
- **Where:** `crates/rb-frameworks/src/clients.rs:279–296` (instances: only `create`'s first-argument literal, spreads ignored, no `const` check, no later writes), `stitch.rs:299–307` (imported instance base taken as is), `clients.rs:658–668` (`SpringBases::new` collects only `assigned` chains and `setUriTemplateHandler`, not `baseUrl`/`uriBuilderFactory`/`defaultUriVariables` statements on a `*.Builder` binding).
- **Regression tests:** clients unit: `axios.create({ baseURL: lit, ...x })` → base unknown; an instance bound by `let`, or whose name is the target of a `defaults.baseURL` assignment or an `interceptors.request.use` call in the file (or in the exporting module, for imports) → base unknown; a `WebClient.Builder`/`RestClient.Builder` binding with a base-setting statement → each statement is a `BaseFact`. Stitcher unit with these and an external override → never `synthetic`; a must-not-exist guard line.

### R2 — blocker: an unknown leading URL part under a known base is read as a path segment and matched exactly (`synthetic`)

- **Repro** (`c5`, `svc/orders` exposes `GET /:tenant/reports/:id`; `const orders = axios.create({ baseURL: "http://orders" })`):
  - `l01`: ``orders.get(`${process.env.STRIPE_URL}/reports/1`)`` → `HTTP GET /{}/reports/{} @svc/orders` **`[synthetic]`** `{host: orders, base: literal, match: exact}`.
  - `l02`: `orders.get(base + "/reports/1")` (a parameter) → same, **`[synthetic]`**.
  - Java (`c2`): `private final WebClient wc10 = WebClient.create("http://orders");` `wc10.get().uri(System.getenv("X") + "/reports/1")` → `Clients#j19() -CONSUMES-> GET /{}/reports/{}` **`[synthetic]`**.
  - Control: `l03` ``orders.get(`/${id}/reports/1`)`` → `synthetic`, correct (the URL starts with `/`).
- **Observed vs expected:** axios ignores `baseURL` when the request URL is absolute, Spring's `DefaultUriBuilderFactory` likewise, and `RootUriTemplateHandler` only prepends its root to a template starting with `/`. A URL whose leading part is unknown may be absolute, so S3's own rule ("a leading hole is the base") applies: suffix matching, `heuristic` at most. `stitch::join` instead prepends the instance base, makes the hole a `{}` segment, and the origin of the join is the base's `literal` (`(o, Unresolved) => o`), so every `synthetic` condition holds.
- **Pre-existing:** `d2aee54` gives the same edges. Not in any open item. The shape needs an endpoint whose first segment is a parameter (tenant/locale/version prefixes) in the base's service; it is narrower than R1, but the edge's certainty is above its evidence.
- **Where:** `crates/rb-indexer/src/stitch.rs:368–393` (`join`: only `base.is_none()` treats a leading hole as the base).
- **Regression tests:** `join(Some(base), url starting with a hole)` → base unknown (suffix), never `synthetic`; stitcher rows for the three repros; a guard line.

### R3 — major: `spring.cloud.config.enabled=false` in a profile file or profile document turns the config client off for every profile (regression from Q3's fix)

- **Repro:** `c2` `java/billing2`: `pom.xml` depends on `spring-cloud-starter-config`; `application.properties` `users.url=http://users` and `spring.config.import=optional:configserver:http://cfgsrv:8888`; `application-local.properties` `spring.cloud.config.enabled=false`; the in-repo config server `java/cfgsrv` serves `shared/billing2.yml` with `users.url: https://legacy-users.example.com`. `UsersCaller#me()` (`rt.getForObject(usersUrl + "/users/me", …)`) → `GET /users/me @svc/users` **`[synthetic]`** at HEAD, `heuristic` at `7d12001`; `config_unread` reasons drop `config_client` (only `profile` left). Same in `c3` `k4` (`bootstrap-local.yml` with `on-profile: local`) and `k5` (`application.yml`, second document `on-profile: test`): heuristic → **`synthetic`**.
- **Observed vs expected:** disabling the config client in the `local`/`test` profile is the standard idiom; in the default and production profiles the client is on and the config server's `users.url` (external, here) wins. Amendment 11's rule — every property of a config client stays below `synthetic` — is defeated by a profile-scoped switch. Expected: only `application.properties`/`bootstrap.properties` (no profile) and profile-less YAML documents count as disabling; or never drop the client for a key.
- **Where:** `crates/rb-indexer/src/lib.rs:348–356` (`disabled` collects every file kind, profiles and bootstrap profiles included); `fold.rs:398–400` (`---` documents are flattened into one map, so a profile document's value overwrites).
- **Severity:** major, not blocker: the edge's target is wrong only through configuration the indexer does not read (open item 3), but the cap that made that honest is removed by a rule introduced in this commit.
- **Regression tests:** `config_directories_flow_mappings_and_disabled_clients` rows: a client by dependency with `application-local.properties` / `bootstrap-local.yml` / a profile document setting `enabled=false` stays a client.

### R4 — major: open item 11 understates its own false-edge risk: several holes after a port hole give `synthetic`, not `heuristic`

- **Repro:** `c1` `t28` ``fetch(`http://orders:${p}${q}/orders/1`)`` → `GET /orders/{} @svc/orders` **`[synthetic]`** `{host: orders, match: exact}` (HEAD and `7d12001`).
- **Observed vs expected:** open item 11 says such URLs are matched exactly within the host and "a false `heuristic` edge is possible when `${prefix}` is a real path prefix". The edge is `synthetic`: `AuthorityEnd::Port` → `skip_authority` drops every following hole, no flag is set, and the folded origin stays `literal`. When `${q}` is a path prefix (`/v2`), the real path does not exist and the edge is a false `synthetic`.
- **Where:** `crates/rb-frameworks/src/endpoint.rs:268–272` with `skip_authority` (`:464`); `tasks/contract-m4a.md` open item 11.
- **Regression test:** `client_url` row `http://users:${p}${q}/x` → `base_unknown` (or a flag that blocks `synthetic`); stitcher row → never `synthetic`.

### R5 — minor: Q7 is fixed downstream only; upstream still enters an endpoint through a registrar's caller

- **Repro:** `c4` (`svc/loop`: `fa()` registers `ra.get("/a", async () => { fb(); … })`, `fb()` registers `rb.get("/b", async () => { fa(); …; fetch("http://auth/login", POST) })`, mounted `/x`, `/y`). `rb path "POST /login" --repo …` upstream: `client fb()` `⇐ entered at GET /y/b` (correct) **and** `← fa()` `⇐ entered at GET /x/a [heuristic]`. Calling `fb()` from `/x/a` registers `/b`; it does not run the fetch. Downstream from `GET /x/a` correctly omits `POST /login`, so the two directions now disagree.
- **Where:** `crates/rb-store/src/path.rs` upstream: a site inside an inline handler continues to that endpoint *and* to the enclosing node's inbound `CALLS` (`outside: Vec::new()` at `:568`, `:663`).
- **Regression test:** the downstream Q7 layout asserted upstream: `POST /login` has only `GET /y/b` as an entry.

### R6 — minor: the growth tests bound only the instrumented helpers

- Mutants T (clients pack: an uncounted scan of every binding per call, O(calls × bindings)) and U (stitcher: an uncounted scan of every endpoint per site) **survive** `packs_grow_linearly` and `the_stitcher_grows_linearly`. Semantics-preserving quadratics inside the counted loops (V2, W2) are killed, so the counters are real; but "the stitcher/packs grow linearly" now means "their indexed lookups do", and quadratic work added anywhere else is invisible (the wall-clock tests saw it, flakily). Not stated in invariant 11's restatement. Suggest keeping one coarse wall-clock bound on the large input (as the pathological test does) or ticking per site/call examined in the outer loops.

### R7 — minor: two amendment-12 rules have no biting test

- **C** (`post/put/patch(url, data, config)` → `patch` reads its 2nd argument as the config) survives rb-frameworks and rb-indexer `--lib`.
- **G** (`agreed`: a fresh client counts as disagreement, so `new RestTemplate()` initialiser + constructor `rootUri(x)` assignment becomes unknown) survives both.

### R8 — minor/doc: client calls silently outside every bucket; README lags

- `axios.create({ baseURL: … }).get("/orders/1")` (`c1` `t05`), `new RestTemplate().getForObject(…)` (no binding; `c2` `j05`), and `import fetch from "cross-fetch"` (the import shadows `fetch`; `c1` `n03`) are not sites and appear in no `http_coverage` bucket (shadowed `fetch` parameters and locals, `c1` `n02`/`n05`, are correctly not sites). Open item 14 lists `request`, `got`, `superagent`, `ky`, `undici` but not fetch polyfills (`cross-fetch`, `isomorphic-fetch`, `isomorphic-unfetch`), inline `new RestTemplate()` or chained `create(…).verb(…)` (lesson 2026-10-01: a suppressed source is counted).
- `README.md:15` lists the clients without `node-fetch`.

### R9 — minor: "unknown base" rules give false `heuristic` suffix edges not listed as such

- `c1` `t10` (`api10.get("/orders/1", { ...stripeOpts })`), `t25` (`api25.get("/orders/1", cfg)`), `c2` `j07` (`rootUri("http://orders")` initialiser plus `rt7.setUriTemplateHandler(new DefaultUriBuilderFactory("https://api.stripe.com"))` in the constructor: two bases → unknown, although the later statement deterministically wins) → `GET /orders/{} @svc/orders [heuristic] {match: suffix}`; all go to `api.stripe.com`. Amendment 12 chose "unknown → suffix" deliberately and open item 10 says suffix matches trust the tail, but no open item says that an unreadable or contested *stated* base (which is often an external one) is still path-matched. State it, or treat a contested base whose candidates include an external literal as external.

## Precision samples

Sampling rule: endpoint keys sorted bytewise, N = ⌊count/20⌋, indices ⌊N/2⌋ + i·N (all when fewer than 20).

| Repository | Endpoints | `CONSUMES` | Verdict |
|---|---|---|---|
| **New:** piomin/sample-spring-microservices-new @ `905529a715c3` (Maven; employee/department/organization services, OpenFeign by service name, config server, gateway) | 16 (all) | 4 heuristic | **Endpoints 16/16**: each read in `EmployeeController`, `DepartmentController`, `OrganizationController` (no class mapping; `@PostMapping("/")`, `@GetMapping("/{id}")`, `/department/{departmentId}`, `/organization/{organizationId}[/with-employees]`, `/{id}/with-departments[-and-employees]`, `/{id}/with-employees`); handlers match; `prefix_maybe_incomplete` correct (config clients; the config server's `config/<service>.yml` set no context path). **CONSUMES 4/4**: `department EmployeeClient#findByDepartment` → `GET /department/{} @employee-service`; `organization DepartmentClient#findByOrganization` → `GET /organization/{} @department-service`, `#findByOrganizationWithEmployees` → `GET /organization/{}/with-employees @department-service`; `organization EmployeeClient#findByOrganization` → `GET /organization/{} @employee-service` (`@FeignClient(name = …)` equals the directory). `heuristic` (config client cap) is right; `d2aee54` had them `synthetic`. |
| petclinic-microservices | 16 (all) | 5 heuristic | dump identical to `7d12001` (pass 2: 16/16). Re-read all 5 edges: `VisitsServiceClient#getVisitsForPets` (`hostname + "pets/visits?…"`) → `GET /pets/visits`; `AIDataProvider` `getAllOwners`/`addOwnerToPetclinic`/`addPetToOwner` (`getCustomerServiceUri() + "/owners…"`) → customers; `VectorStoreController` (`"http://vets-service/"` in a non-final local + `"vets"`) → `GET /vets`. **5/5.** |
| ftgo-application | 18 (all) | 2 ambiguous | dump identical to `7d12001` except the four `dynamic` rows; `OrderServiceProxy#findOrderById` (`getOrderServiceUrl() + "/orders/{orderId}"`) → order-service and order-history-service, both real candidates. **2/2.** The four `node-fetch` sites are `dynamic` as item 16 says. |
| node-express-realworld, gin-realworld, piggymetrics | 20, 20 (of 22), 11 | 0 | dumps identical to `7d12001`, verified by pass 2 (20/20, 20/20, 11/11). |

Public totals: heuristic 9/9, ambiguous 2/2 real, synthetic 0. None of the public repositories contains R1–R4's shapes; the scratch repositories do: `c1` 12 `synthetic` of which 5 false (`t01`–`t04`, `t15`) plus `t28` (R4); `c2` 7 `synthetic` of which 2 false (`j01`, `j19`) and 1 above its evidence (`me`, R3); `c5` 2 of 3 `synthetic` false.

## Coverage regressions

- vs `7d12001`: public repositories and fixtures — none (identical but ftgo's +4 `dynamic` sites). Scratch: `c1` `t07`/`t08`/`t09`/`t10`/`t25` `synthetic` → heuristic (the call states a base the tool cannot read: by design; `t07`, `baseURL: undefined`, is a precision loss — axios keeps the instance base for `undefined`), `t11`–`t14`, `t18` (×3), `j03`, `j3b`, `g2` false edges removed (correct), `t19` heuristic edges for `{?}.default.svc.cluster.local` and `orders.{?}.svc` lost (missed in-cluster edges, item 18, honest). Gained `synthetic`: `j16` (constructor `rootUri`, correct), `j18` (path hole, correct), `billing2`/`k4`/`k5` (R3, wrong certainty), `node-fetch` sites (correct).
- vs `d2aee54`: lost edges are piggymetrics' false `ExchangeRatesClient#getRates()` edge, ftgo's 3 test-only loopback edges (`ApiGatewayIntegrationTest`, gateway calling itself), and piomin's 4 edges `synthetic` → `heuristic` (config clients). All correctly lost or lowered.

## Open items 1–20

Items 3, 9, 10, 12–19 reproduce as written. Item 11 is inaccurate (R4: `synthetic`, not `heuristic`). Item 14 omits fetch polyfills and inline/chained client forms (R8). Missing: R1 (instance/builder overrides), R2 (leading hole under a base), R3 (profile-scoped disable), R5 (upstream registrar), R9 (contested/unreadable stated bases are path-matched).

## Closing readiness

- **Blockers:** R1 (false `synthetic` from instance bases overridden in the same file: `defaults.baseURL`, interceptor, reassignment, spread in `create`, imported instance, mutated `WebClient.Builder`), R2 (false `synthetic` from an unknown leading URL part under a stated base; S3 violated).
- **Major:** R3 (Q3's disable rule ignores profiles; regression to `synthetic` for config clients), R4 (open item 11 misstates a `synthetic` false-edge risk).
- **Minor / follow-ups:** R5 (upstream registrar), R6 (growth tests bound instrumented helpers only), R7 (mutants C, G survive), R8 (silent non-sites, README), R9 (unlisted suffix risk of contested bases).
- Q1–Q8: fixed as amendment 12 states (Q1 for the forms it lists; Q3 with R3's over-reach; Q7 downstream only). Baseline, the five recorded repositories, determinism and invariant 13 reproduce exactly.

VERDICT: FAIL
