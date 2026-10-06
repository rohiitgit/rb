# M4a evaluator pass 5 (`2a20746`)

- Date: 2026-10-06.
- Commit evaluated: `2a20746` (`Fix M4a evaluator pass 4: conservative base verification`), contract amendment 14, `PIPELINE` `m4a-http-8`. Baselines: `b75b6fc` (pass 4, `m4a-http-7`) and `38d55e4` (M3b, `m3b-scip-8`) for invariant 13. Tree clean at the start and at the end; no tracked file changed (every mutant applied by script, tests run, file restored and `cmp`-checked; `git status --short` empty after each batch).
- Runtime: Docker Desktop (`DOCKER_CONTEXT=desktop-linux`), database `127.0.0.1:55433`; nothing pulled. 14 GiB free at the start, 12 GiB before e2e (no `cargo clean`).
- Throwaway material under `var/eval-m4a-5/` (removed at the end): release binaries of HEAD, `b75b6fc`, `38d55e4` (HEAD from the work tree, the other two from `git archive` into their own target directory; embedded `PIPELINE` checked: `m4a-http-8`, `-7`, `m3b-scip-8`); scratch repository `x` (every pass-4 repro, the controls, 34 new probes, TS/JS and Java, config-client modules `bk6`, `bk7`, `bk8`, `bk10`, `bk11`); seven public clones. All index runs `--no-scip`; every `ev4a5_*` repository and AGE graph dropped at the end; no `rb.scip_index` row created by this pass.

## Measured baseline

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| Unit tests (`--workspace --lib --bins`, awk-summed, e2e excluded) | **459** passed, 0 failed (rb 14, rb-frameworks 76, rb-git 26, rb-indexer 165, rb-model 11, rb-scip 40, rb-sql 27, rb-store 27, rb-syntax 73) — matches the changelog |
| e2e with database and containers, `--nocapture`, twice | **17/17** both runs, 0 `skipping:` lines (`scip_polyglot_monorepo`, `scip_java_spring_jpa`, `scip_ts_express_prisma`, `request_path_goldens` ran), 52.8 s and 48.3 s |
| Goldens | `git diff b75b6fc HEAD -- crates/rb-cli/tests/golden`: empty |
| Seven public repositories vs `b75b6fc` | all seven dumps **byte-identical** (`cmp`) |
| Determinism | all seven public repositories and scratch `x`: `--force --no-cache` == `--force` (cached), `cmp`-identical |
| Invariant 13 vs `38d55e4` | symbol nodes + `CALLS`/`REFERENCES`/`IMPLEMENTS`: 0 rows differ on all seven (438 / 96 / 795 / 1,100 / 2,813 / 239 / 1,554 rows) |
| Host paths | no `/Users/`, `/private/`, `/tmp/` in any dump (public and scratch) |
| Stub scan | only `rb-sql/src/lib.rs:1111` (a test string) |

## Pass-4 findings (end to end, release binary, scratch `x`; HEAD vs `b75b6fc`)

| # | Pass 4 | Status at `2a20746` | Evidence |
|---|---|---|---|
| T1 | blocker | **Fixed.** | `a01` (setter above the instance), `a02` (module `let` rebuilt above), `a06` (arrow inside the function, above), `a10` (class static method above): `synthetic` → `heuristic`, `base: unverified`, `match: exact`, host `orders`. |
| T2 | blocker | **Fixed.** | `C1#k01` (`c.rt1`, `C1.this.rt1`), `C1#k04` (`other.rt4`, `c.self().rt4`), `Base2#k02` (subclass `Sub2`), `C3#k03` (Lombok `@Getter`, `Use3`), `C7#k07` (public field written from `Use7.java`): all `heuristic`, `base: unverified`. Every Java field is now unverified by construction. |
| T3 | blocker | **Fixed.** | `a04` (`export { a04api as ordersApi }` + mutating importer), `a05` (`export { a05api as default }` + interceptor in importer): `heuristic`, `base: unverified`. |
| T4 | blocker | **Fixed.** | `C5#k05` (`WebClient` field, factory before `baseUrl`), `#k06` (`RestClient` field), `#k05l` (local, factory first), `#k06l` (local `RestClient`, `baseUrl` first, factory after): no `CONSUMES`; four `http_external` rows, host `api.stripe.com`, URL `https://api.stripe.com/orders/1`. |
| T5 | major | **Fixed.** | `bk7` (`#---` + `on-cloud-platform`): `heuristic`, `config_unread` reasons `[config_client]`; `bk6` (YAML): reasons `[config_client, yaml]`; my `bk11` (legacy `spring.profiles=k8s` `#---` document): `heuristic`, `[config_client]`. Controls: `bk8` (base document `enabled=false`) and my `bk10` (base document with `spring.profiles.active=dev` and `enabled=false`) stay `synthetic`, `base: config`, no `config_unread`. |
| T6 | minor | **Fixed (documented).** | Open item 14 now lists calls on a field inherited from a superclass. |
| T7 | doc | **Fixed for T1–T4; the new text overclaims again (U1).** | Amendment 13 carries a correction; item 22 and the lesson describe the token scan. Item 22's "*Proves:* … so nothing reconfigures the instance" is not what makes a request go to the verified base: a request sent through `x.mutate()…` is certified without its links being read (U1). |

Controls (all stay `synthetic`, `base: literal`, `match: exact`): `t00` (unexported module `const`), `fl` (function local), `inl` (inline `axios.create(…).get`), `Store#s01` (Store-style local `WebClient.builder().baseUrl(…).build()`), `bk8`. `http_coverage` on `x`: 57 sites = 22 synthetic + 31 heuristic + 4 external (sums), `bases {verified 15, unverified 30}` (was `{35, 10}` at `b75b6fc`).

### Mutation check of the new rule (scripted edit, `rb-syntax`/`rb-frameworks`/`rb-indexer` `--lib`, restore, `cmp`)

| Mutant | Condition | Result |
|---|---|---|
| M1 | an exported `const`/`let` declarator is a `declaration` (`tokens.rs` export check removed) | killed (`instance_bases_are_verified_only_when_unchanged`) |
| M2 | a Java `field_declaration` declarator is a `declaration` | killed (`rest_template_web_client_and_rest_client`, `spring_bases…`, `spring_client_bases_visible_on_the_instance`) |
| M3 | `other` tokens ignored in the scan | killed (`instance_bases…`, `spring_bases…`) |
| M4 | two declarations allowed (`>= 1`) | killed (`instance_bases…`) |
| M5 | a non-`const` top-level binding scanned over the file (module `let` verifiable) | killed (`instance_bases…`) |
| M6 | every construction link plain | killed (`spring_client_bases…`, `uri_builder_factories_override_the_base_and_unverify_it`) |
| M7 | `uriBuilderFactory` not read as a base (no precedence) | killed (`uri_builder_factories…`) |
| M8 | only `on-profile` makes a document conditional | killed (`only_the_base_document_turns_the_config_client_off`) |
| M9 | `fresh_root` returns `true` for every chain | **survived** — masked: the same condition removed only from `plain_construction` (M9b) is killed by `spring_bases…` (`l4`, a chain from a builder parameter, becomes verified); with M9 `l4` stays unverified through another path. Not a defect of the shipped code; the guard is pinned at its use site only. |
| M10 | optional chaining on the member counts as a receiver | killed (`name_tokens_of_configured_names`) |
| M11 | the scan starts at the binding's declaration (T1 regression) | killed (`instance_bases…`) |

## Gate A soundness probe (scratch `x`, release binary, `rb dump` + `rb path "GET /orders/{id}"`)

Held (correct outcome):

| Probe | Form | Outcome |
|---|---|---|
| p01 | function local returned (`return api`) | `heuristic`, unverified (a `return` token is `other`) |
| p02 | closure capturing the local passed to another function (`register(() => api)`) | unverified |
| p03 | `with (cfg) { api.defaults.baseURL = … }` (CommonJS `.js`) | unverified |
| p04 | `var api = axios.create(…)` | unverified (a `var` is never a declaration) |
| p05 | module `const` used only for requests + a nested function declaring the same name and mutating it | unverified (name-based over-taint, as designed) |
| p06 | module `const api` + class private field `#api` built for Stripe | module call `synthetic` — correct, `#api` is a different binding |
| p07 | `globalThis["p07api"].defaults…` | `synthetic` — correct: a module `const` is not a global-object property |
| p08 | `import { p08api }` mutated at module level, function local of the same name used | local `synthetic` — correct: the import is outside the local's scope and is another binding |
| p10, p14, p15, p16 | `typeof api`, `` `${api}` ``, JSX `client={api}`, a parameter of the same name | unverified (cost by design) |
| p11 | request config passed by name (`api.get(url, opts)`) | base unknown → suffix `heuristic` |
| p13 | `eval("p13api")` | `synthetic` — the documented blind spot (item 22) |
| q01 | Java local mutated inside a lambda (`rt.getInterceptors().add`) | unverified |
| q02 | anonymous class calling `rt.setUriTemplateHandler(stripe)` | contested base → suffix `heuristic` |
| q06, q07 | Java local returned / passed to a helper that sets the handler | unverified |
| q08 | Java local shadowing a mutated field of the same name | `synthetic` — correct (distinct binding, field outside the method range) |
| q10, q15 | local built with a non-plain link (`filter`) | unverified |
| q11 | `w.mutate().build().get()` (copy unchanged) | `synthetic` — correct |

Not held: U1 (blocker), U2 (major), U3 (minor, suspected), below.

## Public repositories (release HEAD, `--no-scip`; `--force --no-cache`, then `--force` cached: identical; identical to `b75b6fc`)

| Repository @ commit | Endpoints | Sites | Buckets (syn / heu / amb / ext / dyn / self / unm) | Bases verified / unverified | `CONSUMES` by certainty | Check |
|---|---|---|---|---|---|---|
| spring-petclinic-microservices @ `dc9ca15a7118` | 16 | 7 | 0 / 5 / 0 / 1 / 1 / 0 / 0 | 0 / 0 | 5 heuristic | edges as in pass 4 (`getVisitsForPets` → `GET /pets/visits`; `AIDataProvider` ×3 → customers; `VectorStoreController` → `GET /vets`) |
| node-express-realworld @ `30b68e1e8814` | 20 | 1 | 0 / 0 / 0 / 0 / 0 / 1 / 0 | 0 / 0 | — | unchanged |
| golang-gin-realworld @ `626c372d2594` | 22 | 51 | 0 / 0 / 0 / 0 / 2 / 5 / 44 | 0 / 0 | — | unchanged |
| piggymetrics @ `6bb2cf9ddbca` | 11 | 4 | 0 / 0 / 0 / 0 / 0 / 0 / 4 | 0 / 0 | — | unchanged |
| ftgo-application @ `558dfc53b11d` | 18 | 7 | 0 / 0 / 1 / 0 / 4 / 2 / 0 | **2** / 0 | 2 ambiguous | both real candidates (`order-service`, `order-history-service`); the 2 verified bases re-read: `ApiGatewayIntegrationTest` lines 60 and 96, `WebClient client = WebClient.create("http://localhost:" + port + …)`, the only other method tokens are `client.post()` (62) and `client.get()` (98) → `self`. Correct. |
| piomin/sample-spring-microservices-new @ `905529a715c3` | 16 | 4 | 0 / 4 / 0 / 0 / 0 / 0 / 0 | 0 / 0 | 4 heuristic | the four Feign edges (config clients) |
| JakubDevv/Store-Microservices-BackEnd @ `2c7b390d9ee2` | 92 | 20 | 0 / 4 / 0 / 0 / 15 / 1 / 0 | **19** / 0 | 4 heuristic | 19 verified bases re-read: Store `WebClientService` 16 locals `webClient` (lines 23–202) and Identity `WebClientService` 3 locals `client` (16, 31, 45); every other token of the name in its method is the receiver of `.get()` (13), `.delete()` (2), `.post()` (1), `.put()` (1, Identity), `.get()` (2, Identity). Correct. `CONSUMES` 4/4: `findUser` → `GET /auth/user/{}`, `getUser` → `GET /admin-identity/user/{}`, `banUser` → `DELETE /admin-identity/user/{}`, `banCompany` → `DELETE /admin-identity/company/{}`, all `@Identity`, suffix `heuristic` (host `{?}:8080`). |

No `synthetic` `CONSUMES` on any public repository, so there was no synthetic edge to re-read.

**Store sample** (92 endpoints, keys sorted bytewise, N = 4, indices 2, 6, …, 78): **20/20** — method, full path (class `@RequestMapping` + method mapping) and handler read in source: Store `AdminController` (`/admin`): `DELETE /category/{id}` `deleteCategory`, `DELETE /product/{id}/retire` `retireProductById`, `GET /category/products` `getAmountOfProductsByCategory`, `GET /orders` `getOrders`, `GET /products/time` `getProductsInTime`, `GET /user/{id}` `getUser`, `POST /filter` `createFilter`; Identity `AdminController` (`/admin-identity`): `GET /user/{id}` `getUserById`; Identity `AuthController` (`/auth`): `GET /company` `getCompany`, `GET /user` `getUser`, `GET /users` `getAmountOfUsers`, `POST /company` `createCompany`; `CompanyController` (`/company`): `GET /orders`, `GET /products`; `ProductsController` (`/products`): `GET /categories`, `GET /product/{id}` `getProductById`, `GET /rating` `getProductsSortedByRating`, `GET /subCategories`; `UserController` (`/user`): `@GetMapping("/")` → `GET /user` `getUser`, `GET /stats` `getStatsFromOrders`. `prefix_maybe_incomplete` honest (profile YAML unread).

## New findings

### U1 — blocker: a request sent through `x.mutate()…` is certified with `x`'s verified base without reading the copy's links → false `synthetic`

- **Repro** (`x`, `java/pay/src/main/java/p/Q.java`; `svc/orders` exposes `GET /orders/:id`):
  - `q03`: `WebClient w = WebClient.create("http://orders"); w.mutate().apply(b -> b.baseUrl("https://api.stripe.com")).build().get().uri("/orders/1").retrieve();` → `Q#q03() -CONSUMES-> HTTP GET /orders/{} @svc/orders` **`[synthetic]`** `{base: literal, host: orders, match: exact}`; `rb path "GET /orders/{id}"` lists it `[synthetic]`.
  - `q04`: `w.mutate().filter(ExchangeFilterFunction.ofRequestProcessor(rq -> Mono.just(ClientRequest.from(rq).url(URI.create("https://api.stripe.com/v1")).build()))).build().get().uri("/orders/1")…` → **`[synthetic]`**.
  - `q05`: `RestClient r = RestClient.create("http://orders"); r.mutate().requestInterceptor(…).build().get().uri("/orders/1")…` → **`[synthetic]`** (an interceptor is a non-plain link by amendment 14).
- **Observed vs expected:** `apply` sets the copy's base to Stripe and the filter rewrites the URL; the request never reaches `orders`. Expected `heuristic`, `base: unverified` (or `external` for `q03`). Amendment 14 rule 3 says any link outside `PLAIN_LINKS` (filters, interceptors, `apply`, customizers) "may change URI building, so the base is unverified", and item 22 claims a verified base proves "nothing reconfigures the instance". The instance `w` is indeed unchanged, but the request runs on the copy, whose links are never read.
- **Cause:** `mutate` is an allowed receiver call for `WebClient`/`RestClient` (`crates/rb-frameworks/src/clients.rs:898`), so `w` passes the scan; in `web_client` a request chain whose own links state no base falls back to `bases.of(receiver, kind)` (`clients.rs:1103`), and `plain_construction` is applied to the request chain only when the chain states a base (`clients.rs:1093–1097`). The links between `mutate()` and the verb are ignored.
- **Pre-existing:** `b75b6fc` gives the same three `synthetic` edges. Not in any open item.
- **Regression test:** `spring_bases_are_verified_only_when_unchanged` rows for `w.mutate().apply(…)`, `.filter(…)`, `.requestInterceptor(…)` before the verb → `unverified`; stitcher row → `heuristic` `base: unverified`. Fix: judge the request chain's links before the verb with the plain-link test (`mutate` and `build` added), or drop `mutate` from the allowed calls.

### U2 — major: an instance with no base read is never gated, so an interceptor or filter on it does not cap an absolute or configured URL → false `synthetic`; the contract is inconsistent on this

- **Repro** (`x`):
  - `p17.ts`: `const p17api = axios.create({ timeout: 5 }); p17api.interceptors.request.use((c) => { c.url = "https://api.stripe.com/v1/charges"; return c; }); export function p17() { return p17api.get("http://orders/orders/1"); }` → **`[synthetic]`**. `p18.ts` is identical except `axios.create({ baseURL: "http://orders" })` → `heuristic`, `base: unverified`. A `baseURL` that the absolute URL ignores flips the certainty.
  - `Q2.java` with `REWRITE = ExchangeFilterFunction.ofRequestProcessor(rq -> Mono.just(ClientRequest.from(rq).url(URI.create("https://api.stripe.com/v1")).build()))`: inline `WebClient.builder().filter(REWRITE).build().get().uri("http://orders/orders/1")` (`q09b`), a local built that way (`q12`), a **field** built that way (`q13`), and a field `new RestTemplateBuilder().additionalInterceptors(…).build()` with `getForObject("http://orders/orders/1", …)` (`q14`) → all **`[synthetic]`**. The same local with `.baseUrl("http://orders")` added (`q15`) → `heuristic`, unverified.
- **Observed vs expected:** amendment 13's *Effect* (still in force, `tasks/contract-m4a.md:322`): "Gate A is applied to every call on an unverified instance, also when the call states its own base or an absolute URL (an interceptor runs after both)"; amendment 14: Java fields are "never verified", inline instances "stay verified unless … a link is not plain". `q13`/`q14` are fields and `q09b` an inline instance with a non-plain link, yet `synthetic`. The code only gates bases read from a construction: `verified` is `None` whenever no base is read (`clients.rs:437`, `clients.rs:447` `base.is_some().then_some(…)`, `clients.rs:1098–1102` `BaseFact::Fresh`). Taken literally, the Effect line would also cap the `bk8` control (a field `new RestTemplate()` with a config URL), which amendment 14 says stays `synthetic` — the two amendments disagree, and item 22's "cannot see" list does not mention base-less instances.
- **Severity:** major, not blocker: not a Gate A-verified base, and a URL-rewriting interceptor or filter on a base-less client is uncommon (load-balancer filters rewrite to an instance of the same service, which is harmless). Still a false `synthetic` (invariant 4) the contract's own text says is handled. **Pre-existing** (`b75b6fc`: same edges).
- **Regression test / decision:** either cap absolute- and config-URL calls on any instance that fails the scan or carries a non-plain link (and rule on `bk8`), or state the residue in item 22 with the forms above.

### U3 — minor (suspected false `synthetic`): axios `create` configs are checked only for a spread after `baseURL`, not for an `adapter`

- **Repro:** `p09.ts`: function-local `axios.create({ baseURL: "http://orders", adapter: myAdapter })`, `get("/orders/1")` → `synthetic`, base verified. An `adapter` replaces axios's transport (the analogue of a Spring connector or request factory, which amendment 14 lists as non-plain); one that changes the destination makes this edge false. Not shown wrong on real code; custom adapters that reroute are rare. `clients.rs:391–392` (`create_base` reads only `baseURL` and spreads).

### Notes (no finding)

- `const g = api.get; g(url)` and `api["get"](url)` are not sites (p12): no edge, no bucket. Consistent with item 14's undetected forms.
- M9 survival (above): the `fresh_root` guard is pinned only where `plain_construction` uses it.

## Criteria and invariants (as now worded, given open items 1–23)

| # | Verdict | Evidence |
|---|---|---|
| S1, S2 | PASS | `name_tokens` recorded for TS/TSX/JS/Java (`name_tokens_of_configured_names`, M1, M2, M10 killed). |
| S3 | PASS | Gate B forms unchanged (dumps identical to `b75b6fc`). |
| S4 | PASS | T5 fixed (`bk6`, `bk7`, `bk11` keep `config_client`; `bk8`, `bk10` turn it off). |
| S5–S8 | PASS | Store 20/20; goldens unchanged. |
| S9 | **FAIL** (U2) | axios instance interceptor + absolute URL is `synthetic` (`p17`); U3 suspected. T1/T3 fixed. |
| S10 | **FAIL** (U1, U2) | `mutate()` copies certified with the receiver's base (`q03`–`q05`); fields/inline instances with filters or interceptors and absolute URLs `synthetic` (`q09b`, `q12`–`q14`). T2/T4 fixed. |
| S11, S12 | PASS | |
| S13 | **FAIL** (U1, U2) | false `synthetic` edges. |
| S14 | PASS | buckets sum on every dump (`x` 57; public 7/1/51/4/7/4/20). |
| S15–S18 | PASS | `rb path` lists upstream sites with certainty; e2e path goldens pass. |
| S19 | PASS | growth tests green (item 23 residue). |
| S20–S26 | PASS | e2e 17/17 twice, no `skipping:`, SCIP-mode path golden ran; determinism `cmp`-identical on eight repositories; goldens unchanged. |
| S27 | PASS | 0 synthetic; 13 heuristic correct; 2 ambiguous real; 21 verified bases re-read, all correct; Store 20/20. |
| S28 | **FAIL** | the loop is green, but U1 (blocker) and U2 (major) are open. |
| Inv 1, 2, 3, 5, 6, 8, 9, 10, 11, 12, 13 | PASS | inv 13: 0 differing rows on all seven vs `38d55e4`; inv 9: fresh == cached, HEAD == `b75b6fc` dumps. |
| Inv 4 | **FAIL** | `synthetic` on requests rerouted by a copy's links (U1) or an instance's interceptor/filter (U2). |
| Inv 7 | **FAIL** | requests that go to `api.stripe.com` are path-matched `synthetic` to `svc/orders` (`q03`, `q04`, `p17`, `q09b`, `q12`–`q14`). |

## Docs vs code

- Amendment 14, item 22, the lesson, CHANGELOG and README match the code for T1–T7 (the scan, the plain links, factory precedence, conditional documents; 459 tests and the mutant list as stated; the public re-measurement reproduces byte for byte).
- Item 22's *Proves* sentence is false for `mutate()` copies (U1); its *Cannot see* list omits base-less instances (U2). Amendment 13's *Effect* line ("every call on an unverified instance, also … an absolute URL") is not what is built, and it contradicts amendment 14's `bk8` control.
- Open items 1–21 and 23 reproduce as written; 14 now lists inherited fields (T6).

## Follow-ups

- **Before closing M4a:** U1 — read the request chain's links after `mutate()` with the plain-link test (or stop allowing `mutate`); unit + stitcher rows. U2 — decide whether base-less instances that fail the scan or carry non-plain links cap absolute/config URLs (and what `bk8` then is), or record the residue in item 22 with these forms; reconcile amendment 13's *Effect* line with amendment 14.
- **M4b/M5:** U3 (`adapter` and any other transport key in an axios config treated like a non-plain link); measure item 22's cost on the M5 benchmark repositories (field-held Spring clients now always `heuristic`); a test pinning `fresh_root` in `chain_base` (M9); item 14's undetected forms (`g = api.get`, `api["get"]`, inherited Java fields).
- **M6:** none new.

VERDICT: FAIL
