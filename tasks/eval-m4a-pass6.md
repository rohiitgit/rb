# M4a evaluator pass 6 (`d4010fa`)

- Date: 2026-10-06.
- Commit evaluated: `d4010fa` (`Fix M4a evaluator pass 5: every carrier gated`), contract amendment 15, `PIPELINE` `m4a-http-9` (checked in the release binary). Baselines: pass 5 (`tasks/eval-m4a-pass5.md`, `2a20746`) and `38d55e4` (M3b, `m3b-scip-8`, release binary built from `git archive` into its own target directory) for invariant 13.
- Runtime: Docker Desktop (`DOCKER_CONTEXT=desktop-linux`), database `127.0.0.1:55433`; nothing pulled. Machine loaded by other applications (load average 9–12 throughout). 12–14 GiB free; one `cargo clean -p rb-syntax -p rb-frameworks -p rb-indexer` (see V6), no full clean.
- Throwaway material under `var/eval-m4a-6/` (removed at the end): release binaries of HEAD and `38d55e4`; scratch repositories `x` (pass-5 repros), `y` (controls), `h` (`y` + the three hooks of the generator's unit), 31 probe repositories (`pa`–`pi`, `g1`–`g9`, `j`, `jf`, `js`); seven public clones. Every index run `--no-scip`; every `ev4a6_*` repository (46) and its AGE graphs dropped at the end; `rb.scip_index` row count unchanged (23 before and after). Tree clean at the start and the end; every mutant applied by script and restored, content compared.

## Measured baseline

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| Unit tests (`--workspace --lib --bins`, awk-summed, e2e excluded) | **464** passed, 0 failed (rb 14, rb-frameworks 79, rb-git 26, rb-indexer 166, rb-model 11, rb-scip 40, rb-sql 27, rb-store 27, rb-syntax 74) — matches the changelog |
| e2e with database and containers, `--nocapture` | run 1: **17/17** (254 s); run 2: 16/17 — `polyglot_monorepo` failed on `rb cypher … ERROR: canceling statement due to statement timeout` (the 2 s `statement_timeout`, `rb-store/src/lib.rs:640`, under load 12); run 3: **17/17** (422 s). 0 `skipping:` lines in all three; `scip_polyglot_monorepo`, `scip_java_spring_jpa`, `scip_ts_express_prisma`, `request_path_goldens` ran. Environmental, not caused by this change. |
| Goldens | `git diff 2a20746 HEAD -- crates/rb-cli/tests/golden`: empty |
| Determinism | all seven public repositories: `--force --no-cache` == `--force` (cached), `cmp`-identical |
| Invariant 13 vs `38d55e4` | symbol nodes + `CALLS`/`REFERENCES`/`IMPLEMENTS`: 0 rows differ on petclinic (438), ftgo (2,813), Store (1,554) |
| Host paths | no `/Users/`, `/private/`, `/tmp/` in any dump (public and scratch) |
| Stub scan | only `rb-sql/src/lib.rs:1111` (a test string) |

## Pass-5 findings (end to end, release binary)

| # | Pass 5 | Status at `d4010fa` | Evidence (`rb dump` + `rb path "GET /orders/{id}"`) |
|---|---|---|---|
| U1 | blocker | **Fixed.** | `q03` (`mutate().apply(baseUrl stripe)`), `q04` (`mutate().filter(rewrite)`), `q05` (`RestClient` `mutate().requestInterceptor`): `heuristic`, `base: unverified`, host `orders`, `match: exact`. `q11` (`w.mutate().build().get()`, an unchanged copy, `synthetic` in pass 5) is now `heuristic` too — a stated cost (item 22, "a `mutate()` copy"). |
| U2 | major | **Fixed for the repro forms.** | `p17`, `p18` (axios instance + interceptor, absolute URL), `q09b` (inline `WebClient.builder().filter(…)`), `q12` (local), `q13` (`WebClient` field), `q14` (`RestTemplateBuilder` field with interceptors), `bk8` (`RestTemplate` field + config URL, ruled `heuristic` by amendment 15): all `heuristic`, `base: unverified`. Amendment 13's *Effect* line now carries a correction; `bk8` ruled. Not fixed for carriers the gate still judges plain: V1–V5. |
| U3 | minor | **Fixed.** | `p09` (`axios.create({ baseURL, adapter })`): `heuristic`, `base: unverified`; `PLAIN_AXIOS_KEYS` allow-list (`clients.rs:514`). |
| M9 | masked mutant | **Fixed.** | `fresh_root` → always `true` killed by `chain_base_reads_a_fresh_root` plus four other rows (M9 below). |

Controls (scratch `y`, no hook): `t00` (unexported module `const`), `fl` (function local), `inl` (inline `axios.create(…).get`), `Store#s01` (Store-style local `WebClient.builder().baseUrl(…).build()`), `Store#rt0` (local `new RestTemplate()`), Feign `OrdersClient`, bare `fetch`, `axios.get`, `axios(…)`, Go `http.Get` + `http.DefaultClient.Get`: **10/10 `synthetic`**, `base: literal`, `match: exact`; `rb path` lists them `[synthetic]`. Hooked copy `h` (`globalThis.fetch = …`, `axios.interceptors…`, `http.DefaultTransport = …` in separate files): bare `fetch`, `axios.get`, `axios(…)` and Go `bare()` → `heuristic`, `base: unverified`; the instance, Store, `RestTemplate` and Feign controls stay `synthetic`.

### Mutation check of amendment 15 (scripted edit, `cargo test --no-fail-fast -p rb-syntax -p rb-frameworks -p rb-indexer --lib`, restore, compare)

Baseline green first (after V6's clean). All 14 killed.

| Mutant | Condition | Killed by |
|---|---|---|
| M1 | `mutate` back in the fluent request calls | `carriers_are_gated_whatever_the_url` |
| M2 | request-chain links before the verb not judged | `carriers_are_gated…` |
| M3 | `new RestTemplate(args)` a plain carrier | `carriers_are_gated…` |
| M4 | every axios config key plain | 4 rb-frameworks rows (`carriers…`, `axios_request_configs…`, `inline_instances…`, `instance_bases…`) |
| M5 | `fetch` global hook ignored | `global_hooks_of_module_clients`, `every_carrier_is_gated_whatever_the_url` |
| M6 | Go `DefaultTransport` not a hook | `global_hooks…`, `every_carrier…` |
| M7 | `verdict` ignores `plain` | 5+ rows |
| M8 | `fetch` not a module call | `carriers…`, `every_carrier…` |
| M9 | `fresh_root` always `true` | `chain_base_reads_a_fresh_root` and four others |
| M10 | configuring chains of a binding not judged | `carriers…`, `uri_builder_factories…`, `spring_bases…`, `spring_client_bases…` |
| M11 | stitcher: hooks never cap module calls | `every_carrier_is_gated_whatever_the_url` |
| M12 | stitcher: an unverified imported instance plain | `bases_stated_at_the_call…`, `a_base_reaches_synthetic_only_when…` |
| M13 | an axios `Other` token never a hook | `global_hooks…` |
| M14 | a non-literal axios config plain | `axios_request_configs…`, `carriers…` |

## Soundness probe (release binary; one probe repository per global hook)

Held:

| Probe | Form | Outcome |
|---|---|---|
| pd, pd2–pd8 | axios alias `import ax from "axios"; ax.interceptors…`; `(axios as any).defaults…`; top-level `const axios = require("axios")`; namespace `A.default.defaults…`; `{ default as ax }`; hooks in `.mjs`, `.cjs`, `.tsx` | bare `axios.get`/`axios(…)` capped |
| pb4–pb8 | `global.fetch = jest.fn()` in `src`, in `web/jest.setup.js`, in `web/test/setup.ts`; `globalThis.fetch ??= …`; `window.fetch! = …` | bare `fetch` capped |
| g1–g3, g5–g9 | `http.DefaultClient.Transport = …` in `init()`; `http.DefaultTransport.(*http.Transport).Proxy = …`; aliased import `nh.DefaultClient = …`; `var c = http.DefaultClient`; `t := http.DefaultTransport.(*http.Transport)`; `&http.DefaultClient`; hook in another package; hook in a `_test.go` | Go bare calls capped |
| j9 | local `new RestTemplate(new InterceptingClientHttpRequestFactory(…))` | `heuristic`, unverified |
| Feign2 | Feign `url = "${orders.url}"` overridden in `application-prod.properties` | `heuristic`, `base: config` (config rules govern, as stated) |
| js | module `const fetch = …` / `let fetch = …` wrapper; function-local `const fetch = wrap()` | no site (shadowed); `window.fetch(…)` → `synthetic`, correct |
| pa, pe | `Object.defineProperty(globalThis, "fetch", …)`; `require("axios").default.defaults…` (no binding) | `synthetic` — **listed** in item 22 (*cannot see*) |

Not held: V1–V5 below (each a false `synthetic` none of item 22's residue covers).

## Go extension (generator's own initiative): judgement

Sound in direction and consistent with "every carrier gated": bare `http.Get/Post/…` and `http.DefaultClient.*` are plain only while no Go file names `http.DefaultTransport` or uses `http.DefaultClient` other than as a request receiver; `http.Client` bindings and `NewRequest` cap. Eight of nine hook forms are caught (table above); only a dot import of `net/http` escapes (V5). Cost: gin-realworld 51/51 sites capped (all `NewRequest` in tests; buckets unchanged because none was stitched), and a local `&http.Client{Timeout: …}` with no `Transport` is capped by design — over-conservative, recorded in item 22's M5 follow-up. Accepted, with V5.

## Public repositories (release HEAD, `--no-scip`; `--force --no-cache`, then `--force` cached: identical on all seven)

| Repository @ commit | Endpoints | Sites | Buckets (syn / heu / amb / ext / dyn / self / unm) | Bases verified / unverified | Carriers (sites) | `CONSUMES` by certainty |
|---|---|---|---|---|---|---|
| spring-petclinic-microservices @ `dc9ca15a7118` | 16 | 7 | 0 / 5 / 0 / 1 / 1 / 0 / 0 | 0 / 6 | 1 bare + 6 capped (fields/injected) | 5 heuristic (all `base: unverified`, suffix) |
| node-express-realworld @ `30b68e1e8814` | 20 | 1 | 0 / 0 / 0 / 0 / 0 / 1 / 0 | 0 / 1 | 1 bare, hooked (`e2e/src/support/test-setup.ts` `axios.defaults.baseURL`) | — |
| golang-gin-realworld @ `626c372d2594` | 22 | 51 | 0 / 0 / 0 / 0 / 2 / 5 / 44 | 0 / 51 | 51 capped (`NewRequest`) | — |
| piggymetrics @ `6bb2cf9ddbca` | 11 | 4 | 0 / 0 / 0 / 0 / 0 / 0 / 4 | 0 / 0 | 4 Feign | — |
| ftgo-application @ `558dfc53b11d` | 18 | 7 | 0 / 0 / 1 / 0 / 4 / 2 / 0 | 2 / 1 | 4 bare + 2 verified + 1 capped | 2 ambiguous (`OrderServiceProxy#findOrderById` → order-service, order-history-service; `base: unverified`) |
| piomin @ `905529a715c3` | 16 | 4 | 0 / 4 / 0 / 0 / 0 / 0 / 0 | 0 / 0 | 4 Feign | 4 heuristic (Feign, config client) |
| Store-Microservices @ `2c7b390d9ee2` | 92 | 20 | 0 / 4 / 0 / 0 / 15 / 1 / 0 | 19 / 1 | 19 verified + 1 capped | 4 heuristic (`findUser`, `getUser`, `banUser`, `banCompany` → `@Identity`, suffix) |

Matches the changelog's re-measurement exactly. Verified carriers re-read in source (21): ftgo `ApiGatewayIntegrationTest` lines 60 and 96, `WebClient client = WebClient.create("http://localhost:" + port + …)`, only other tokens `client.post()`/`client.get()` → plain; Store `WebClientService` 16 locals `WebClient webClient = WebClient.builder().baseUrl("http://" + val + ":8080").build()` (lines 23–202) and Identity `WebClientService` 3 locals `client` (16, 31, 45, same builder), every other token the receiver of a verb → plain. Correct. **No `synthetic` `CONSUMES` on any public repository**, so none to re-read. Note: piggymetrics declares two global Feign `RequestInterceptor` beans (`account-service` and `notification-service` `ResourceServerConfig`, OAuth2 headers) — the V2 shape occurs in real code, harmlessly here (its Feign sites are unmatched).

## New findings

### V1 — blocker: a Spring client built from an existing client, a customizer or a constructor argument is judged a fresh plain carrier → false `synthetic`

- **Repro** (`j`, `pay/src/main/java/p/J.java`; `svc/orders` exposes `GET /orders/:id`; every row `rb path` `[synthetic]`, `base: literal`, `match: exact`):
  - `j1`: `RestTemplate rt = new RestTemplate(); rt.getInterceptors().add(rewrite); RestClient.create(rt).get().uri("http://orders/orders/1").retrieve();`
  - `j2`: `void j2(RestTemplate injected) { RestClient.builder(injected).build().get().uri("http://orders/orders/1").retrieve(); }`
  - `j5`: `RestClient c = RestClient.create(injected); c.get().uri("http://orders/orders/1").retrieve();` (a local, passes the scan)
  - `j3`: `RestTemplate rt = new RestTemplateBuilder(cust).build(); rt.getForObject("http://orders/orders/1", String.class);`
  - `j4`: `new RestTemplate(new InterceptingClientHttpRequestFactory(new SimpleClientHttpRequestFactory(), List.of(rewrite))).getForObject("http://orders/orders/1", String.class);` (the same construction held in a local, `j9`, is correctly `heuristic`)
  - `j10`: `RestTemplate rt = new org.springframework.web.client.RestTemplate() { }; rt.getForObject(…)` (an anonymous subclass)
- **Observed vs expected:** `RestClient.create(RestTemplate)` / `RestClient.builder(RestTemplate)` copy the template's request factory, interceptors and `UriTemplateHandler`; `RestTemplateBuilder(RestTemplateCustomizer…)` customizers may set either; a request factory may rewrite the URI; a subclass may override anything. Amendment 15 says "a parameter or injected instance", "`new RestTemplate(factory)` or a subclass" cap the site, and category 4 admits only "`new RestTemplate().getForObject(…)`" (no argument). Expected `heuristic`, `base: unverified` for all six.
- **Cause:** `fresh_root` (`crates/rb-frameworks/src/clients.rs:1021–1035`) accepts any `new RestTemplate…(…)`/`new RestTemplateBuilder(…)` root (the arguments are cut off at `(`) and any static `WebClient`/`RestClient` `create`/`builder` whatever its argument; `PLAIN_LINKS` (`clients.rs:970–991`) admits `create`/`builder` with any argument; the inline `RestTemplate` path (`clients.rs:845–849`) uses the same `fresh_root`; `SpringBases::carrier`'s `Init::New` argument check (`clients.rs:1174–1177`) covers only the local-`new` form, not chains, and ignores an anonymous class body.
- **Frequency:** `RestClient.create(restTemplate)` / `RestClient.builder(restTemplate)` is Spring's documented migration path from `RestTemplate` to `RestClient` (Boot 3.2+); none on the seven public repositories (none uses `RestClient` that way).
- **Pre-existing** (the `fresh_root`/`PLAIN_LINKS` logic predates amendment 15), not in any open item.

### V2 — blocker: a Feign interface is a plain carrier even when a `RequestInterceptor` (its `configuration` or a global bean) can retarget it → false `synthetic`

- **Repro:** `j`: `@FeignClient(name = "orders", url = "http://orders", configuration = RewriteConfig.class)` with `RewriteConfig` `@Bean RequestInterceptor rewrite() { return t -> t.target("https://api.stripe.com"); }` → `OrdersFeign#one() -CONSUMES-> HTTP GET /orders/{} @svc/orders` **`[synthetic]`**. `jf`: the same interface without `configuration`, plus a `@Configuration` class with that `RequestInterceptor` bean (Spring Cloud OpenFeign applies context `RequestInterceptor` beans to every client) → **`[synthetic]`**.
- **Why false:** Feign runs the interceptors before `HardCodedTarget.apply`, which sets the target only when the template's URL does not already start with `http`; an interceptor that sets an absolute target sends the request there. Amendment 15 caps every other client with an interceptor in sight; item 22's *cannot see* names "library hooks that apply to injected builders only (Spring Boot customizers …)" but not Feign interceptors, and category 3 says "declarative; its URL comes from the annotation or properties".
- **Frequency:** global Feign `RequestInterceptor` beans are common (piggymetrics has two, adding OAuth2 headers); retargeting ones are rare. Either cap Feign sites in a service that declares a `RequestInterceptor` bean or a `configuration =` class (cost: piggymetrics' 4 Feign sites, already unmatched), or list the residue in item 22 with this form.

### V3 — blocker: the `fetch` hook scan misses TypeScript cast forms and a bare assignment → bare `fetch` stays `synthetic`

- **Repro** (each a separate copy of `y` with one extra file `web/src/hook.ts`; bare `fetch("http://orders/orders/1")` in `web/src/t.ts` stays **`[synthetic]`**):
  - `pb`: `(globalThis as any).fetch = (u: string) => realFetch("https://api.stripe.com/v1");`
  - `pb2`: `(window as any).fetch = …`
  - `pb3`: `(global as any).fetch = jest.fn();`
  - `pc`: `fetch = ((orig) => (u: any, i?: any) => orig("https://api.stripe.com/v1", i))(fetch);` (assignment to the global binding; valid in a module)
  The uncast `global.fetch = jest.fn()` (`pb4`) is capped.
- **Contract text vs code:** item 22 (*proves*, case 2) says no file may name "`globalThis.fetch`, `window.fetch`, `global.fetch`, `self.fetch` other than called"; `(globalThis as any).fetch` names exactly that. The cast puts the global object's token under an `as_expression` (or `parenthesized_expression`), so `rb-syntax` gives it `TokenRole::Other` (`crates/rb-syntax/src/tokens.rs:176–181`), and `global_hooks` looks only for `TokenRole::Member { name: "fetch" }` on a global object (`crates/rb-frameworks/src/clients.rs:286–290`); a bare `fetch = …` has no global-object token at all. Neither form is in item 22's *cannot see* list (which names dynamic property strings, `defineProperty`, `Object.assign`).
- **Frequency:** `(global as any).fetch = jest.fn()` is the usual way to stub `fetch` in TypeScript tests (the uncast form often fails type-checking), so in exactly the repositories amendment 15 means to cap ("a test's `global.fetch = jest.fn()` caps every `fetch` in the repository"), the TS spelling slips through.

### V4 — blocker: a repository's own `axios.defaults…` hook caps bare axios calls but not axios instances, which inherit the defaults

- **Repro** (`pi`: `y` + `web/src/hook.ts` `import axios from "axios"; axios.defaults.adapter = reroutingAdapter;`): bare `axios.get`/`axios(…)` → `heuristic`, unverified (correct), but `t00` (`const t00 = axios.create({ baseURL: "http://orders" })`), `fl` (function local) and `inl` (inline `axios.create(…).get`) stay **`[synthetic]`**.
- **Why false:** `axios.create(config)` merges the module's `defaults` object (the one `axios.defaults` mutates) with `config`, so an instance created after the hook runs inherits `adapter`, `transformRequest`, `httpAgent`, `proxy`, interceptors aside. The scan sees the hook (it caps bare calls) but applies it only to `module_call` sites (`crates/rb-indexer/src/stitch.rs:313–316`); category 1's proof for an instance ignores module hooks, and item 22 lists only a *dependency* changing the axios defaults.
- **Also missed (same scan, `pf`):** `export function setup() { const axios = require("axios"); axios.interceptors.request.use(…); }` — `module_names` reads only top-level `require` bindings (`clients.rs:479–494`), so a function-local `require` hook caps nothing.
- **Fix direction:** an `axios` hook caps every axios site of the repository (instances included), and module names include function-scoped `require("axios")` bindings; or state both in item 22.

### V5 — blocker by the false-`synthetic` rule, rare: a dot import of `net/http` hides a Go hook

- **Repro** (`g4`): `import . "net/http"; func init() { DefaultTransport = rewriting{} }` → Go `bare()` stays **`[synthetic]`**. `global_hooks` returns early for alias `.` (`clients.rs:327–328`). Not in item 22. Practically nonexistent in real code; listing it in item 22 is a sufficient close.

### V6 — minor (process): the generator's mutation harness left a stale test binary in the shared target directory

- Before any edit of mine, `cargo test --no-fail-fast -p rb-syntax -p rb-frameworks -p rb-indexer --lib` on the clean HEAD tree **failed 3 tests**: `every_extractor_module_is_fingerprinted` ("calls.rs is not in the extractor fingerprint"), `carriers_are_gated_whatever_the_url` (a local `new RestTemplate()` unverified) and `port_holes_keep_the_host_in_every_source_form`. Cargo reported the `rb_syntax-ff4ac7d384e6420b` test binary fresh: it was built at 15:10:53 from a **mutated** `calls.rs` (no `Init::New`), and `calls.rs` was then restored with an mtime (15:10:52) older than the build, so cargo never rebuilt it. `cargo clean -p` of the three packages → all green (79/166/74). The `--workspace` combination (a different binary) was unaffected, so the stated 464 holds, but any mutant the harness measured on this package combination after a same-second restore may have been "killed" by stale code — my first mutation run showed exactly that (stitcher-only mutants "killed" by an rb-frameworks test). My 14 mutants were re-run after the clean; the generator's 40-mutant claim is not independently reproducible as recorded. Fix: restore with a fresh mtime (`cp`, not `cp -p`/`copy2`), or `touch` after restoring.

### Notes (no finding)

- e2e run 2's `statement_timeout` (2 s on `rb cypher`) under load 12 is environmental; it reproduces nothing in this change but makes the e2e suite load-sensitive.
- `j11` (`.attribute(…)` after the verb) stays `synthetic`: links after the verb do not route; correct. `j12` (`RestClient.create().get().uri(absolute)`) `synthetic`: correct.
- Feign2 (`${orders.url}` overridden by a profile file): `heuristic`, `base: config` — the config rules govern, as item 22 says.

## Criteria and invariants (as now worded, given open items 1–23)

Open items other than 22 were not re-reproduced in this pass beyond what the public re-measurement shows (byte-identical to the changelog's numbers).

| # | Verdict | Evidence |
|---|---|---|
| S1, S2 | PASS | facts (`Member`, `Init::New`, import and global tokens) recorded; M13 and V6's rows exercise them. |
| S3–S8 | PASS | routes, endpoints and handlers unchanged; goldens unchanged; public endpoint counts as in pass 5. |
| S9 | **FAIL** (V3, V4) | bare `fetch` with a cast or bare-assignment global hook, and axios instances under an `axios.defaults` hook, are `synthetic`. U1–U3 forms fixed. |
| S10 | **FAIL** (V1, V2) | `RestClient.create/builder(rt)`, `new RestTemplateBuilder(cust)`, inline `new RestTemplate(factory)`, anonymous subclasses, and Feign with a retargeting interceptor are `synthetic`. |
| S11 | PASS (V5 rare) | Go hook scan catches 8 of 9 forms; dot import escapes. |
| S12 | PASS | folding unchanged. |
| S13 | **FAIL** (V1–V5) | false `synthetic` stitches from carriers judged plain. |
| S14 | PASS | buckets sum on every dump (`x` 12, `j` 12, public 7/1/51/4/7/4/20). |
| S15–S18 | PASS | `rb path` lists sites with certainty; `request_path_goldens` passed. |
| S19 | PASS | growth tests green. |
| S20–S26 | PASS | e2e 17/17 on runs 1 and 3 (run 2: one environmental timeout), no `skipping:`, SCIP-mode path golden ran; determinism `cmp`-identical on seven; AGE parity tests green. |
| S27 | PASS | 0 synthetic; 13 heuristic and 2 ambiguous as in pass 5; 21 verified carriers re-read, all plain; carrier categories match the changelog. |
| S28 | **FAIL** | loop green, but blockers V1–V5 open. |
| Inv 1, 2, 3, 5, 6, 8, 9, 10, 11, 12, 13 | PASS | inv 13: 0 differing rows vs `38d55e4` on petclinic, ftgo, Store; inv 9: fresh == cached on seven. |
| Inv 4 | **FAIL** | `synthetic` on requests whose carrier is reconfigured (V1–V5). |
| Inv 7 | **FAIL** | requests retargeted to `api.stripe.com` are path-matched `synthetic` to `svc/orders` (`j1`, `j3`, `j4`, Feign `OrdersFeign`, `pb`, `pi`'s instances). |

## Docs vs code

- Amendment 15, the corrections in amendments 13 and 14, CHANGELOG, IMPLEMENT.md, todo and the lesson match the code for U1–U3 and M9; the public re-measurement reproduces byte for byte; the test count (464) is right.
- Amendment 15's carrier text ("a parameter or injected instance", "`new RestTemplate(factory)` or a subclass" cap) is not what `fresh_root` builds (V1). Item 22's *proves* case 2 lists `globalThis.fetch` as seen, but not its TS cast spelling (V3); case 3 (Feign) has no interceptor caveat (V2); case 1 ignores module defaults (V4); *cannot see* omits V2–V5's forms.
- README is unchanged and high-level; nothing in it contradicts the code.
- The changelog's "40 mutants, all killed" is not reproducible as recorded (V6).

## Follow-ups

- **Before closing M4a:** V1 — `fresh_root`/`PLAIN_LINKS` admit `create`/`builder` only with no argument or a URL-typed one (a string or a name that is not a client/builder binding), `new RestTemplate()`/`new RestTemplateBuilder()` only with no arguments and no class body, for chains and inline calls alike; rows `j1`–`j5`, `j10`. V2 — cap Feign sites of a service with a `RequestInterceptor` bean or a `configuration =` class, or list it in item 22. V3 — treat a global object under a cast/parenthesis/non-null wrapper followed by `.fetch` as a member, and a bare `fetch` assignment target as a hook; rows `pb`, `pb2`, `pb3`, `pc`. V4 — an axios hook caps every axios site (instances too); function-scoped `require("axios")` counts; rows `pi`, `pf`. V5 — Go dot import as a hook (or item 22). V6 — fix the harness restore and re-run the mutation list.
- **M4b/M5:** measure item 22's cost (petclinic 6/7 and gin 51/51 capped); a sounder rule for `http.Client` bindings with no `Transport`, test-only hooks, and `mutate()` copies with plain links (`q11`); the e2e suite's sensitivity to the 2 s `statement_timeout` under load.
- **M6:** none new.

VERDICT: FAIL
