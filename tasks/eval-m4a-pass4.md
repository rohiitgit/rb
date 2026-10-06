# M4a evaluator pass 4 (`b75b6fc`)

- Date: 2026-10-06.
- Commit evaluated: `b75b6fc` (`Fix M4a evaluator pass 3: verified bases, literal path starts`), contract amendment 13 (Gate A, Gate B), `PIPELINE` `m4a-http-7`. Tree clean at the start and at the end. No tracked file was changed: every mutant was applied by script to a copy-aside file, the `rb-syntax`, `rb-frameworks`, `rb-indexer` `--lib` tests run, the file restored and compared byte for byte; `git status --short` was empty after each batch.
- Runtime: Docker Desktop (`DOCKER_CONTEXT=desktop-linux`), database `127.0.0.1:55433`; nothing pulled. 16 GiB free at the start (no `cargo clean` needed).
- Throwaway material under `var/eval-m4a-4/` (removed at the end): release binaries of HEAD, `e06e590`, `d2aee54`, `38d55e4` (each `git archive` built in its own target directory; embedded `PIPELINE` strings checked: `m4a-http-7`, `-6`, `-4`, `m3b-scip-8`); scratch repositories `x` (pass-3 repros plus Gate A/B attacks, TS and Java), `x2` (more Java field-access forms), `x3` (config-client disable variants); seven public clones. All index runs `--no-scip`; every `ev4a4_*` repository and AGE graph was dropped at the end; no `rb.scip_index` row was created by this pass.

## Measured baseline

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| Unit tests | **458** passed (rb 14, rb-frameworks 75, rb-git 26, rb-indexer 165, rb-model 11, rb-scip 40, rb-sql 27, rb-store 27, rb-syntax 73) — reproduced |
| e2e with database and containers, `--nocapture`, twice | **17/17** both runs, 0 `skipping:` lines (`scip_polyglot_monorepo`, `scip_java_spring_jpa`, `scip_ts_express_prisma` ran), 50.8 s and 50.1 s |
| Goldens | `b75b6fc` changes only `polyglot-monorepo/dump.jsonl`'s `http_coverage` row (`bases: {0, 0}`) |
| Six recorded public repositories vs `e06e590` | every dump differs only by the `bases` key of `http_coverage` — **reproduced**: `{0,0}` on petclinic-microservices, node-express-realworld, gin-realworld, piggymetrics, piomin; `{verified: 2}` on ftgo (both its integration test's local `WebClient.create("http://localhost:"+port+…)`, used only for `get()`/`post()`; both `self`) |
| Determinism | petclinic, ftgo, store, scratch `x`: `--force --no-cache` == `--force` twice under a second repository name, `cmp`-identical |
| Invariant 13 vs `38d55e4` | symbol nodes + `CALLS`/`REFERENCES`/`IMPLEMENTS`: 0 rows differ on all seven public repositories (438 / 96 / 795 / 1,100 / 2,813 / 239 / 1,554 rows) |
| Host paths | no `/Users/`, `/private/`, `/tmp/` in any dump |
| Stub scan | only `rb-sql/src/lib.rs:1111` (a test string) |

## Criteria and invariants

| # | Verdict | Evidence |
|---|---|---|
| S1, S2 | PASS | uses facts recorded for the forms in `uses_of_configured_names`; `x` exercises them. |
| S3 | PASS | Gate B holds on every leading-unknown form I built (below). |
| S4 | PASS (T5) | profile files/documents no longer disable the client (`billing2`, `bk5`, `bk9`); a base document does (`bk8`). Non-profile activation documents are misread (T5). |
| S5–S8 | PASS | store: endpoints 20/20 sampled; fixtures' goldens pass. |
| S9 | **FAIL** (T1, T3) | axios instance bases verified although code before the declaration or an aliased export's importer changes them → false `synthetic`. |
| S10 | **FAIL** (T2, T4) | Spring instance bases verified although the field is changed through another reference, a subclass, another file or a generated getter; `uriBuilderFactory` overrides `baseUrl` but `baseUrl` is taken → false `synthetic`. |
| S11, S12 | PASS | |
| S13 | **FAIL** | false `synthetic` edges (T1–T4); `synthetic` above its stated certainty for a config client (T5). |
| S14 | PASS (T6 minor) | buckets sum on every dump (`x` 47, `x2` 5, store 20); inherited-field calls are silent non-sites (T6). |
| S15–S18 | PASS | R5 fixed (`rb path "POST /login"` upstream: only `GET /y/b`). |
| S19 | PASS | growth tests green; R7 mutants C, G killed. T/U not re-mutated (item 23 states the residue). |
| S20–S26 | PASS | e2e twice, goldens unchanged except the recorded row. |
| S27 | PASS on the public repositories | new repository: endpoints 20/20, `CONSUMES` 4/4 heuristic, 0 synthetic; fresh == cached. |
| S28 | **FAIL** | loop green, but T1–T4 (blockers) and T5 (major) are open. |
| Inv 1, 2, 3, 5, 6, 8, 9, 10, 11, 12, 13 | PASS | |
| Inv 4 | **FAIL** | `synthetic` on bases changed by code the gate does not attribute (T1–T3), on a base the builder ignores (T4), on a config client disabled only on a cloud platform (T5). |
| Inv 7 | **FAIL** | requests that go to `api.stripe.com` are path-matched `synthetic` to `svc/orders` (T1–T4). |

## Pass-3 findings

| # | Pass 3 | Status at `b75b6fc` | Evidence (scratch `x`, HEAD vs `e06e590`) |
|---|---|---|---|
| R1 | blocker | **Fixed for the six repro forms; the class is not closed (T1–T4).** | `t01` (`defaults.baseURL`), `t02` (interceptor), `t03` (`let` reassigned after), `t04` (spread in `create`), `t15` (exporting module mutates), `j01` (mutated `WebClient.Builder` field): `synthetic` → `heuristic`, `base: unverified`. Controls `t00`, `j00` stay `synthetic`. My extra in-file forms `a07` (`api?.defaults`/`api!.defaults`), `a08` (`Object.assign(api["defaults"], …)`), `a09` (`const { defaults } = api`), `k08` (`rt.getInterceptors().add`), `k09` (`rt::setUriTemplateHandler`) → unverified, correct. Mutants A1–A12 (escaped uses, writes, receiver links, create spread, inline spread, builder rule, importer taint, the `synthetic` cap, bean base, unbound escapes, TS write marking, `this.x`/member uses) all **killed**. |
| R2 | blocker | **Fixed.** | `l01`, `l02`, `j19` → `match: suffix`, `heuristic`; `l03` stays `synthetic`. Gate B attacks: `"" + env + "/reports/1"` (`b01`), `` `${""}${env}/…` `` (`b04`), `String(env) + …` (`b05`), `EMPTY + env + …` with `const EMPTY = ""` (`b09`), Java `"" + System.getenv("X") + …` (`k10`) → suffix `heuristic`; `"//api.stripe.com/…"` (`b02`) and `"HTTPS://api.stripe.com/…"` (`b03`) → `external`; `fetch(new URL("/orders/1", env))` (`b06`) and `"".concat(env, …)` (`b08`) → `dynamic`; `" " + env + …` (`b07`) → `heuristic`. Mutant B1 (join a leading hole) killed. |
| R3 | major | **Fixed for profiles; residue T5.** | `billing2` → `heuristic`, `config_unread` reasons `config_client`, `profile`; `bk5` (YAML `on-profile` document), `bk9` (dotted `on-profile` document) → `heuristic`; `bk8` (base `application.properties` sets `false`) → `synthetic`, correct. |
| R4 | major | **Fixed.** | `t28` `` `http://orders:${p}${q}/orders/1` `` → `match: suffix`, `heuristic`. Item 11 now states the one-hole residue. Mutant B2 killed. |
| R5 | minor | **Fixed.** | `fa`/`fb` layout: upstream of `POST /login` shows only `GET /y/b`; downstream of `GET /x/a` has no `POST /login`. |
| R6 | minor | **Accepted as restated** (invariant 11, item 23). Not re-mutated. | growth tests pass. |
| R7 | minor | **Fixed.** | Mutant C (`patch` reads its 2nd argument) killed by `axios_request_configs_state_the_base`; G (fresh = disagreement) killed by `spring_bases_are_verified_only_when_unchanged`. |
| R8 | minor/doc | **Fixed.** | `t05` (`axios.create(…).get`), `j05` (`new RestTemplate().getForObject`), `n03` (`cross-fetch`) → `synthetic` sites, correct; README:15 lists `node-fetch` and `cross-fetch`. |
| R9 | minor | **Fixed (documented).** | Item 21 states the `t10`/`t25`/`j07` false-`heuristic` outcome. |

## New findings

### T1 — blocker: Gate A does not see a use written before the instance's declaration (TS/JS) → false `synthetic`

- **Repro** (`x`, `apps/web/src`, `svc/orders` (Express) exposes `GET /orders/:id`):
  - `a01.ts`: `export function a01setup(url) { a01api.defaults.baseURL = url; }` **above** `const a01api = axios.create({ baseURL: "http://orders" });`, then `export function a01() { return a01api.get("/orders/1"); }` → `a01() -CONSUMES-> HTTP GET /orders/{} @svc/orders` **`[synthetic]`** `{base: literal, host: orders, match: exact}`.
  - `a02.ts`: `export function a02reset() { a02api = axios.create({ baseURL: "https://api.stripe.com" }); }` above `let a02api = axios.create({ baseURL: "http://orders" });` → **`synthetic`** (a `let` binding that *is* written).
  - `a06.ts`: inside one function, `const setup = () => { a06api.defaults.baseURL = "https://api.stripe.com"; }; const a06api = axios.create(…); setup(); return a06api.get("/orders/1");` → **`synthetic`**; every request of this function goes to Stripe.
  - `a10.ts`: `class Cfg { static init() { a10api.defaults.baseURL = "https://api.stripe.com"; } }` above the instance, `Cfg.init()` below → **`synthetic`**.
- **Observed vs expected:** expected `heuristic`, `base: unverified` (each is exactly the `t01`/`t03` mutation, only placed earlier in the file, which JS hoisting makes legal and common: setters and config classes are often declared before the instance). Observed `synthetic`.
- **Cause:** `Uses::new` maps a `NameUse` to a binding with `local_binding` → `FileScope::binding_index_at`, which only sees a binding from the end of its declaration (`rb-syntax/src/bindings.rs:164`, `rb-frameworks/src/routes.rs:214–228`). A use before it resolves to nothing and goes to `unbound_escaped` (`clients.rs:194`), which only taints *imported* names; the instance stays verified. Same for a write (`a02`).
- **Not in any open item** (item 22 lists globals, CommonJS, dynamic `import()`, unresolved re-exports, reflection, Spring outside the file). Pre-existing: `e06e590` gives the same edges (`synthetic`), so this is the R1 class, not a regression.
- **Regression tests:** `instance_bases_are_verified_only_when_unchanged` rows for a use and a write in a function/arrow/class declared before the instance (module and function scope); stitcher row → `heuristic`, `base: unverified`. The rule should resolve uses by scope (the binding whose scope contains the use, regardless of position) or treat any unresolved use of an instance's name inside its scope as escaping.

### T2 — blocker: Java Gate A sees only `name` and `this.name`; a field changed through any other reference, a subclass, another file or a generated accessor stays verified → false `synthetic`

- **Repro** (`x`/`x2`, `java/pay`, imports `RestTemplate`, `RestTemplateBuilder`, `WebClient`, `DefaultUriBuilderFactory`; every field below is `…= new RestTemplateBuilder().rootUri("http://orders").build()` and its method calls `getForObject("/orders/1", String.class)`):
  - same file, package-private `final` field, another class: `class Tweak1 { static void tweak(C1 c) { c.rt1.setUriTemplateHandler(new DefaultUriBuilderFactory("https://api.stripe.com")); } }` → `C1#k01()` **`[synthetic]`**.
  - same class, **private** field, another instance: `static void tweak(C4 other) { other.rt4.setUriTemplateHandler(…stripe…); }` → `C4#k04()` **`[synthetic]`**.
  - inner class: `class Inner { void t() { D1.this.rt1.setUriTemplateHandler(…stripe…); } }` → `D1#m01()` **`[synthetic]`**.
  - through a method returning `this`: `d.self().rt5.setUriTemplateHandler(…stripe…)` (other class, same file) → `D4#m05()` **`[synthetic]`**.
  - subclass in another file: `Base2` has `protected final RestTemplate rt2`; `Sub2 extends Base2 { Sub2() { rt2.setUriTemplateHandler(…stripe…); } }` → `Base2#k02()` **`[synthetic]`**.
  - Lombok `@Getter` on `C3` (private final `rt3`); `Use3.java`: `c.getRt3().setUriTemplateHandler(…stripe…)` → `C3#k03()` **`[synthetic]`**.
  - `public RestTemplate rt7` (non-final, never written in its file); `Use7.java`: `c.rt7 = new RestTemplateBuilder().rootUri("https://api.stripe.com").build();` → `C7#k07()` **`[synthetic]`**.
- **Observed vs expected:** `setUriTemplateHandler(new DefaultUriBuilderFactory(stripe))` replaces the root-URI handler, so `"/orders/1"` goes to Stripe; the reassignment does the same. Expected `heuristic`, `base: unverified`. Rule (1) of amendment 13 ("never written") and rule (2) ("every use") are checked only for uses spelled `x` or `this.x` in the declaring file: `uses.rs:110` (`this_member` only for `this`), `uses.rs:121–127` (a `field` of any `field_access` is never a use), `clients.rs:205` (chains whose root contains `.`, e.g. `c.rt1`, `D1.this.rt1`, are skipped). Java has no cross-file pass at all (the TS importer check has no Java analogue), and a non-`private` field, a `protected` field read by subclasses, or a field exposed by a getter (hand-written getters are caught as `return rt`; Lombok's are not) is reachable from anywhere.
- **Open items:** item 22 lists "reflection" and "Spring customisation outside the file (open item 15)"; item 15 is about injected instances and `@Configuration`/customizers. Four of the seven repros are in the declaring file; none is reflection or DI. Not covered. Pre-existing (`e06e590`: all `synthetic`).
- **Suspected, same cause:** `static final RestTemplate RT` mutated as `D2.RT.getInterceptors().add(…)` from another class (my `x2` used `setUriTemplateHandler`, which `SpringBases` does read as a second base → contested → suffix `heuristic`; `getInterceptors()` is not a base fact and the chain root `D2.RT` is skipped by `Uses`).
- **Regression tests:** `spring_bases_are_verified_only_when_unchanged` rows for `other.f.setX(…)`, `Outer.this.f.setX(…)`, a `@Getter`/`@Data` class, a non-`private` field; a stitcher row with a second file (`extends`, field write). The sound rule: verified only for `private` fields of a class with no Lombok accessor annotation, with every `*.f` field access in the file (any qualifier) counted as a use.

### T3 — blocker: an instance exported under another name is tainted under the exported name but looked up under its binding name → false `synthetic`

- **Repro** (`x`):
  - `a04.ts`: `const a04api = axios.create({ baseURL: "http://orders" }); export { a04api as ordersApi }; export function a04() { return a04api.get("/orders/1"); }`; `a04mut.ts`: `import { ordersApi } from "./a04"; ordersApi.defaults.baseURL = "https://api.stripe.com";` → `a04()` **`[synthetic]`**.
  - `a05.ts`: `export { a05api as default }`; `a05mut.ts`: `import client from "./a05"; client.interceptors.request.use((c) => { c.baseURL = "https://api.stripe.com"; return c; });` → `a05()` **`[synthetic]`**.
- **Observed vs expected:** the importer's mutation is the exact `t15` case of amendment 13 ("an exported axios instance is unverified when a file importing it uses it otherwise … that holds in its own module too"). Expected `heuristic`, `base: unverified`.
- **Cause:** `escaped_instances` inserts `(file, exported name)` (`stitch.rs:381`: `ordersApi`, `default`); the exporting module's calls check `unverify(path, top_instance)` with the **binding** name (`stitch.rs:312`: `a04api`). `export_specifier` is not a use (`uses.rs:140`), so the export itself does not unverify either. M3a resolved the import (the taint was recorded); this is not item 22's "re-exports the M3a import targets do not resolve". Pre-existing (`e06e590`: `synthetic`).
- **Regression test:** stitcher rows for `export { x as y }` and `export { x as default }` with a mutating importer → `base: unverified`. Either map exported names to bindings from `export_specifier`, or count an aliased `export_specifier` as a use.

### T4 — blocker: `uriBuilderFactory(…)` on a `WebClient`/`RestClient` builder makes `baseUrl(…)` ignored, but the chain's last base link is taken → false `synthetic`

- **Repro** (`x`, `C5.java`): `private final WebClient w5 = WebClient.builder().uriBuilderFactory(new DefaultUriBuilderFactory("https://api.stripe.com")).baseUrl("http://orders").build();` `w5.get().uri("/orders/1").retrieve()` → `C5#k05() -CONSUMES-> GET /orders/{} @svc/orders` **`[synthetic]`**; the same with `RestClient.builder()` (`k06`) → **`[synthetic]`**.
- **Observed vs expected:** Spring's `WebClient.Builder.baseUrl` / `RestClient.Builder.baseUrl` Javadoc: mutually exclusive with `uriBuilderFactory`; if both are set the `baseUrl` is ignored (`DefaultWebClientBuilder.initUriBuilderFactory` returns the configured factory first). The request goes to Stripe. Expected: the factory's base (here `external`), or at least unknown. `chain_base` (`clients.rs:771–789`) treats the links as "last base-setting link wins", which is wrong for these two builders (and for `RestTemplateBuilder`, where `rootUri` wraps the handler and wins whatever the order — the opposite error, a missed edge, minor).
- **Not in any open item**; item 21 talks about ordering *statements*, not links of one chain. Pre-existing (`e06e590`: `synthetic`). Narrow, but it is a construction-time read that Gate A then certifies.
- **Regression test:** `spring_client_bases_visible_on_the_instance` rows with both orders of `baseUrl`/`uriBuilderFactory` for `WebClient` and `RestClient` → the factory base; a stitcher row → `external`.

### T5 — major: a `.properties` `#---` document activated by something other than a profile is read as a base document and turns the config client off

- **Repro** (`x3`, `java/bk7`: `spring-cloud-starter-config` dependency; `application.properties`: `users.url=http://users`, `spring.config.import=optional:configserver:http://cfgsrv:8888`, `#---`, `spring.config.activate.on-cloud-platform=kubernetes`, `spring.cloud.config.enabled=false`) → `UsersCaller#bk7() -CONSUMES-> GET /users/me @java/users` **`[synthetic]`** `{base: config}`, and no `config_unread`. The YAML equivalent (`bk6`) drops the `config_client` reason (only `yaml` left); its edge stays `heuristic` only because YAML is unread.
- **Observed vs expected:** that document applies only on Kubernetes (the usual idiom: Spring Cloud Kubernetes replaces the config server there); everywhere else the client is on and the config server's `users.url` wins. Expected: like a profile document, it never turns the client off (`heuristic`, `config_client` kept). `fold::profile_document` (`fold.rs:459–463`) recognises only `spring.config.activate.on-profile` and `spring.profiles`; any other `spring.config.activate.*` key (`on-cloud-platform`) makes a non-base document too.
- **Severity:** major (same class as pass 3's R3: amendment 11's config-client cap removed by a disable rule that misreads where the switch applies).
- **Regression test:** `only_the_base_document_turns_the_config_client_off` rows with `on-cloud-platform` in YAML and in a `#---` properties document.

### T6 — minor: calls on an inherited field are not sites and are in no bucket

- **Repro** (`x2`, `D3.java`): `class D3Base { protected final RestTemplate rt3 = …rootUri("http://orders")…; }`, `public class D3 extends D3Base { public String m03() { return rt3.getForObject("/orders/1", String.class); } }` → no `CONSUMES`, no diagnostic; `http_coverage` counts 5 sites for 6 client calls. `local_binding` only finds fields of a class *containing* the call (`fold.rs:279–297`), and `rest_template` drops an unresolved receiver (`clients.rs:671–690`). Open item 14 lists class members in TS and inline Java forms, not inherited Java fields (a common base-client pattern). Expected: counted, or listed in item 14.

### T7 — doc: amendment 13, open item 22 and the 2026-10-06 lesson overstate what Gate A proves

- Amendment 13 says the base is verified only when "every use is the receiver of a recognised request call"; the lesson says "prove the binding cannot change it … checked from recorded use facts, in importers too". As built the proof covers uses after the declaration (T1), spelled `x`/`this.x` (T2), in the declaring file (T2, Java), and importers whose names match the binding (T3). Item 22's "not seen" list omits all four, so a reader takes the `synthetic` tier as sound where it is not.

## Gate A on real code

| Repository | Sites with an instance base | Verified | Unverified | Check |
|---|---|---|---|---|
| ftgo-application @ `558dfc53b11d` | 2 | 2 | 0 | both `WebClient client = WebClient.create(…)` locals in `ApiGatewayIntegrationTest` (lines 60, 96), used only as `client.get()`/`.post()`: correct. |
| **New:** JakubDevv/Store-Microservices-BackEnd @ `2c7b390d9ee2` (Maven; Store, Identity, Gateway; cross-service `WebClient` calls Store → Identity) | 19 | 19 | 0 | every one is a method-local `WebClient webClient = WebClient.builder().baseUrl("http://" + val + ":8080").build();` (Store 16, Identity 3) used only as the receiver of `get()/post()/delete()` (grep of every other `webClient`/`client` occurrence: none). All 19 correct. |
| Other five public repositories, fixtures | 0 | 0 | 0 | |

No public repository contains a T1–T4 shape (grep: no `uriBuilderFactory`, no Lombok-annotated client holder, no aliased axios export, no axios instance at all). The verified bases above are right; the gate's soundness failures are shown on scratch code only.

## Precision samples

Sampling rule: endpoint keys sorted bytewise, N = ⌊count/20⌋, indices ⌊N/2⌋ + i·N (all when fewer than 20).

| Repository | Endpoints | `CONSUMES` | Verdict |
|---|---|---|---|
| **New:** Store-Microservices-BackEnd @ `2c7b390d9ee2` | 92 (N = 4; indices 2, 6, …, 78) | 4 heuristic, 0 synthetic | **Endpoints 20/20**: each method, full path (class `@RequestMapping` + method mapping) and handler read in `Store` `AdminController` (`/admin`: `DELETE /category/{id}` `deleteCategory`, `DELETE /product/{id}/retire` `retireProductById`, `GET /category/products`, `GET /orders`, `GET /products/time`, `GET /user/{id}`, `POST /filter`), `CompanyController` (`/company`: `/orders`, `/products`), `ProductsController` (`/products`: `/categories`, `/product/{id}`, `/rating`, `/subCategories`), `UserController` (`/user`: `@GetMapping("/")` → `GET /user` `getUser`, `/stats`), `Identity` `AdminController` (`/admin-identity`: `GET /user/{id}` `getUserById`) and `AuthController` (`/auth`: `GET /company` `getCompany`, `GET /user` `getUser`, `GET /users` `getAmountOfUsers`, `POST /company` `createCompany`); `prefix_maybe_incomplete` is honest (profile YAML unread). **CONSUMES 4/4**: `WebClientService#findUser` → `GET /auth/user/{}`, `#getUser` → `GET /admin-identity/user/{}`, `#banUser` → `DELETE /admin-identity/user/{}`, `#banCompany` → `DELETE /admin-identity/company/{}`, all `@Identity`; host `http://{?}:8080` (`identity.server` only in profile YAML) → suffix `heuristic`, correct. The other 15 Store/Identity sites use `uri(uriBuilder -> …)` lambdas → `dynamic` (counted); the Gateway's `webClientBuilder.build()…uri("http://localhost:8080/auth/validate-token…")` → `self`. |
| petclinic-microservices | 16 | 5 heuristic | dump == `e06e590` but `bases`; edges re-listed: `getVisitsForPets` → `GET /pets/visits`, `AIDataProvider` ×3 → customers `GET /owners`, `POST /owners`, `POST /owners/{}/pets`, `VectorStoreController` → `GET /vets` (verified in passes 2–3). 5/5. |
| piomin/sample-spring-microservices-new | 16 | 4 heuristic | dump == `e06e590` but `bases`; the four Feign edges as in pass 3. 4/4. |
| ftgo-application | 18 | 2 ambiguous | unchanged; both real candidates. |
| node-express-realworld, gin-realworld, piggymetrics | 20, 22, 11 | 0 | unchanged since pass 2's 20/20, 20/20, 11/11. |

Public totals: heuristic 13/13 correct, ambiguous 2/2 real, synthetic 0. Scratch totals at HEAD: `x` 20 `synthetic`, of which **11 false** (`a01`, `a02`, `a04`, `a05`, `a06`, `a10`, `k01`, `k02`, `k03`, `k04`, `k07`) plus `k05`, `k06` (T4) = 13 false; `x2` 4 `synthetic`, 2 false (`m01`, `m05`); `x3` 1 false-certainty `synthetic` (`bk7`, T5).

## Coverage regressions

- vs `e06e590`: public repositories and fixtures — none (only the `bases` key). Scratch `x`: `synthetic` → `heuristic` for `t01`–`t04`, `t15`, `j01`, `a07`–`a09`, `k08`, `k09` (unverified, correct), `l01`, `l02`, `j19`, `k10`, `b01`, `b05`, `b09` (suffix, correct), `t28` (suffix, correct), `UsersCaller#me` (config client, correct); gained `n03` (`cross-fetch`, correct). No edge was lost.
- vs `d2aee54`: lost/lowered edges are piggymetrics' false `ExchangeRatesClient#getRates()` edge, ftgo's 3 test loopback edges (gateway calling itself), piomin's 4 edges `synthetic` → `heuristic` (config clients) — all correctly lost or lowered, as pass 3 classified. Store: same edges at all three binaries (4 heuristic).

## Open items 1–23

Items 1–21 and 23 reproduce as written (11 now accurate: `t28` suffix; 14 now lists polyfills and member/inline forms but not inherited Java fields, T6; 21 states R9). **Item 22 is incomplete**: it omits uses before the declaration (T1), Java access through any reference other than `x`/`this.x`, from subclasses, other files and generated accessors (T2), and aliased exports (T3); and the effect of those omissions is a false **`synthetic`**, not a false `heuristic`. T4 (`uriBuilderFactory` vs `baseUrl`) and T5 (non-profile activation documents) are in no item.

## Closing readiness

- **Blockers:** T1 (uses before the declaration), T2 (Java field access other than `x`/`this.x`, cross-file, subclasses, Lombok), T3 (aliased exports), T4 (`uriBuilderFactory` overrides `baseUrl`). All four are false `synthetic` edges in forms amendment 13 claims to cover ("every use", "importers") or that the construction read gets wrong; none is in an open item.
- **Major:** T5 (config client turned off by a non-profile activation document).
- **Minor / doc:** T6 (inherited fields silent), T7 (amendment 13, item 22 and the lesson overstate Gate A).
- R1 fixed for its repros, class open (T1–T4); R2, R4, R5, R7, R8, R9 fixed; R3 fixed for profiles (T5 residue); R6 accepted as restated. Baseline, the six recorded repositories, determinism, invariant 13, e2e twice and the new public repository reproduce.
- **M4a cannot close with the open items as written.** The public repositories are clean, but the `synthetic` tier is still not sound for instance bases; either Gate A's proof is completed (scope-based use resolution, all Java field accesses with `private`-only verification, alias-aware export taint, factory-over-base precedence) or item 22 must state that Gate A-verified bases can be false `synthetic` in these forms and the orchestrator must accept that explicitly.

VERDICT: FAIL
