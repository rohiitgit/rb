# M4a evaluator pass 7 (`95e1fbb`)

- Date: 2026-10-06.
- Commit evaluated: `95e1fbb` (`Fix M4a evaluator pass 6: allow-listed carrier roles`), contract amendment 16 and its closing bar, `PIPELINE` `m4a-http-10` (checked in the release binary). Baselines: pass 6 (`tasks/eval-m4a-pass6.md`, `d4010fa`) and `38d55e4` (M3b; release binary built from `git archive` into its own target directory) for invariant 13.
- Runtime: Docker Desktop (`DOCKER_CONTEXT=desktop-linux`), database `127.0.0.1:55433`; nothing pulled. Load average 3.7–5. 12–13 GiB free; one `cargo clean -p rb-syntax -p rb-frameworks -p rb-indexer` before the mutation run, no full clean.
- Throwaway material under `var/eval-m4a-7/` (removed at the end): a base scratch repository (`svc/orders` Express `GET /orders/:id`, `web` TS, `pay` Java, `gocli` Go) and 44 one-change copies of it; seven public clones; the `38d55e4` tree and binary; the mutation harness. Every index run `--no-scip`; all 50 `ev4a7_*` repositories and their 57 AGE graphs dropped; `rb.scip_index` 23 rows before and after. Tree clean at the start and the end; every mutant written and restored by script with a fresh mtime, restored bytes compared.

The closing bar (contract, amendment 16): "no false `synthetic` from any form a repository-wide name-token scan can see"; residue is forms that defeat such a scan by construction (reflection, computed names, side-effect-only imports, `eval`, instrumentation, runtime classpath magic, configuration outside the repository). This pass judges by that bar.

## Measured baseline

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 (re-run after touching `rb-syntax`, so the touched crates were checked again) |
| Unit tests (`--workspace --lib --bins`, awk-summed, e2e excluded) | **468** passed, 0 failed (rb 14, rb-frameworks 81, rb-git 26, rb-indexer 168, rb-model 11, rb-scip 40, rb-sql 27, rb-store 27, rb-syntax 74). Matches the changelog. |
| e2e with database and containers, `--nocapture` | run 1: **17/17** (66 s); run 2: **17/17** (63 s). 0 `skipping:` lines; `scip_polyglot_monorepo`, `scip_java_spring_jpa`, `scip_ts_express_prisma` and `request_path_goldens` ran. |
| Goldens | `git diff d4010fa 95e1fbb -- crates/rb-cli/tests/golden`: empty |
| Determinism | all seven public repositories: `--force --no-cache` == `--force` (cached), `cmp`-identical |
| Invariant 13 vs `38d55e4` | symbol nodes + `CALLS`/`REFERENCES`/`IMPLEMENTS`: 0 rows differ on petclinic (438), Store (1,554), ftgo (2,813) |
| Host paths | no `/Users/`, `/private/`, `/tmp/` in any dump (public and scratch) |
| Stub scan | only `rb-sql/src/lib.rs:1111` (a test string) |
| V6 precondition | after `cargo clean -p` of the three touched crates the baseline `cargo test -p rb-syntax -p rb-frameworks -p rb-indexer --lib` was green (323 = 74 + 81 + 168), so no stale binary |

## Pass-6 findings (end to end, release binary, `rb dump` + `rb path "GET /orders/{id}"`)

| # | Pass 6 | Status at `95e1fbb` | Evidence |
|---|---|---|---|
| V1 | blocker | **Fixed.** | `jj`: `j1` (`RestClient.create(rt)` with an interceptor), `j2` (`RestClient.builder(injected)`), `j5` (local `RestClient.create(injected)`), `j3` (`new RestTemplateBuilder(cust)`), `j4` (inline `new RestTemplate(new InterceptingClientHttpRequestFactory(…))`), `j10` (anonymous subclass in a local), `j10b` (inline anonymous subclass), plus mine: `j6` (`WebClient.builder(x)`), `j7` (injected `WebClient.Builder`), `j8` (`additionalInterceptors`), `j9` (`WebClient.create(unboundName)`), `j11` (`RestClient.create(this.restTemplate)`): 12/12 `heuristic`, `base: unverified`. |
| V2 | blocker | **Fixed.** | `jc` (`configuration = RewriteConfig.class`), `jf` (global `@Bean RequestInterceptor`), `k7` (mine: `class Rw implements feign.RequestInterceptor`, fully qualified, no import): Feign `heuristic`, `base: unverified`. |
| V3 | blocker | **Fixed.** | `pb` (`(globalThis as any).fetch = …`), `pb2` (`(window as any).fetch`), `pb3` (`(global as any).fetch = jest.fn()`), `pc` (`fetch = wrap(fetch)`), `pb5` (mine: the stub in `web/test/setup.ts`): the `fetch` site `heuristic` (coverage `heuristic: 1`), axios untouched. |
| V4 | blocker | **Fixed.** | `pi` (`axios.defaults.adapter`), `pf` (function-local `require("axios")` + interceptor): `t00`, `fl`, `inl`, `axios.get`, `axios(…)` all `heuristic` (5), `fetch` stays `synthetic`. |
| V5 | blocker (rare) | **Fixed.** | `g4` (`import . "net/http"; DefaultTransport = …`): both Go sites `heuristic`. |
| V6 | minor (process) | **Fixed.** | Clean build green before mutating; harness restores with a fresh mtime and compares bytes (mine did the same; results below). |

Controls (scratch `base`, no hook): `t00`, function local, inline instance, bare `fetch`, `axios.get`, `axios(…)`, local and inline `new RestTemplate()`, `WebClient.create(ORDERS)`, `RestClient.create("http://orders")`, Store-style builder local, Feign `OrdersFeign`, Go `http.Get` + `http.DefaultClient.Get`: **14/14 `synthetic`** (coverage row), `base: literal`, `match: exact`; `rb path` lists them `[synthetic]`. In every repro copy the controls of the other kinds stay `synthetic`.

### Mutation check of amendment 16 (my harness; `cargo test --no-fail-fast -p rb-syntax -p rb-frameworks -p rb-indexer --lib`; clean build, green baseline first)

17 of 18 killed; **1 survivor** (W5).

| Mutant | Condition | Killed by |
|---|---|---|
| A | `fetch`: `Other` role allowed | `global_hooks_of_module_clients`, `carriers_are_plain_only_by_allow_listed_roles`, `every_carrier_is_gated_whatever_the_url` |
| B | `fetch`: `node-fetch` declaration not allowed | `global_hooks_of_module_clients` |
| C | axios: any non-callee, non-receiver role allowed | `global_hooks…`, `carriers_are_plain_only…`, `every_carrier…` |
| D | axios: any receiver call allowed | `global_hooks…` |
| **E** | axios: a `require("axios/<subpath>")` binding not a module binding (`is_module` → `==`, `clients.rs:559`) | **survived** (W5) |
| F | stitcher: a hook does not cap verified instances | `carriers_are_plain_only…` |
| G | stitcher: Feign config key ignored | `carriers_are_plain_only…` |
| H | Go: dot import ignored | `global_hooks…` |
| I | Feign: `configuration =` ignored | `global_hooks…`, `carriers_are_plain_only…` |
| J | Feign: only qualified `feign` tokens count | `global_hooks…` (+ `field_chains_binding_chains_and_big_packages_stay_fast`) |
| K | Feign: wildcard import plain | `global_hooks…` (+ the same growth test) |
| L | fresh root: class body allowed | `fresh_constructions_are_allow_listed`, `carriers_are_plain_only…` |
| M | fresh root: `create(<anything>)` allowed | `fresh_constructions…`, `carriers_are_plain_only…` |
| N | `url_arg`: any bound name is a URL | `fresh_constructions…`, `carriers_are_plain_only…` |
| O | settings scan: `.yml`/`.yaml` not read | `feign_client_config_in_any_settings_file` |
| P | tokens: surface names not recorded as property/field names | 4 tests incl. `module_and_surface_name_tokens` |
| Q | tokens: a property token judged a callee | 4 tests incl. `module_and_surface_name_tokens` |
| R | fresh root: `new RestTemplateBuilder(args)` allowed | `chain_base_reads_a_fresh_root`, `fresh_constructions…`, `carriers_are_plain_only…` |

## Soundness probe within the closing bar (release binary; one copy of `base` per probe)

Held (hook seen, the hooked kind capped, everything else `synthetic`):

| Probe | Form | Outcome |
|---|---|---|
| k3mts, k3cts, k3jsx | `axios.defaults.adapter = …` in `.mts`, `.cts`, `.jsx` | 5 axios sites capped |
| k6a | `import * as ax from "axios"; ax.default.defaults.adapter = …` | capped |
| r7 | `import { default as ax2 } from "axios"; export const x = ax2;` | capped |
| r8 | `const axios = require("axios/dist/node/axios.cjs"); axios.defaults…` | capped (but no unit row bites this, W5) |
| k6g | Go `http.DefaultClient = &http.Client{…}` | Go sites capped |
| k6d | `declare function fetch(…)` in `globals.d.ts` | `fetch` capped (a cost, conservative) |
| k7 | Java `implements feign.RequestInterceptor`, no import | Feign capped |

Not held, scan-visible (findings W1–W4):

| Probe | Form | Outcome |
|---|---|---|
| k1 | Kotlin `@Configuration` `@Bean fun rewrite(): RequestInterceptor = RequestInterceptor { t -> t.target("https://api.stripe.com") }` in `pay/src/main/kotlin/p/Global.kt`, Feign interface in Java | Feign **`synthetic`** |
| k1g | the same bean in Groovy (`Global.groovy`) | Feign **`synthetic`** |
| k2 | `web/src/App.vue` `<script lang="ts">import axios from "axios"; axios.defaults.adapter = reroutingAdapter;</script>` | `t00`, `fl`, `inl`, bare axios **`synthetic`** |
| k2h | `web/public/index.html` `<script>window.fetch = (u, i) => realFetch("https://api.stripe.com/v1", i);</script>` | bare `fetch` **`synthetic`** |
| k4go | `gocli/zz_hook.go` starting `// Code generated by mockgen. DO NOT EDIT.` with `http.DefaultTransport = nil` | Go sites **`synthetic`** |
| k4ts | `web/src/hook.ts` starting `/** @generated */` with `axios.defaults.adapter = …` | axios sites **`synthetic`** |
| k4big | `web/src/vendor.js` > 1 MiB with `globalThis.fetch = …` on line 2 | `fetch` **`synthetic`** |
| r1 | `import axios = require("axios"); axios.defaults.adapter = …` (TS import-equals) | axios sites **`synthetic`** |
| r2 | `const ax = require("axios").default; ax.defaults.adapter = …` | axios sites **`synthetic`** (`rb path`: `f()`, `fl()`, `inl()`, `bare()` `[synthetic]`) |
| r3 | `const { default: ax } = require("axios"); ax.defaults.adapter = …` | axios sites **`synthetic`** |
| r5 | `import("axios").then((m) => { m.default.defaults.adapter = …; });` | axios sites **`synthetic`** |
| k5 | Feign `@FeignClient(name = "orders")` (no `url`) + `pay/src/main/resources/application.yml` `spring.cloud.discovery.client.simple.instances.orders[0].uri: https://api.stripe.com` | Feign **`synthetic`** |
| k5r | the same with `application.properties` `orders.ribbon.listOfServers=api.stripe.com:443` | Feign **`synthetic`** |

Not held, residue or unlisted residue-like forms (W6, doc):

| Probe | Form | Outcome | Classification |
|---|---|---|---|
| r4 | `re.ts`: `export { default as ax } from "axios";` `hook.ts`: `import { ax } from "./re"; ax.defaults.adapter = …` | `synthetic` | Listed residue ("an axios object reached through another module's export"), but a static re-export naming `"axios"` does not defeat a token scan by construction: the listing does not fit the bar's own definition. |
| r6 | `(globalThis as any)["fetch"] = wrapped;` | `synthetic` | String-literal subscript; listed under "computed property names" only by the `defineProperty(globalThis, "fetch", …)` analogy. |
| s1, s2 | `jest.spyOn(global, "fetch").mockImplementation(…)`; `vi.stubGlobal("fetch", vi.fn())` | `synthetic` | String-named property via an API, same family as the listed `Object.defineProperty`; not named, and the two most common TS test stubs of `fetch`. |
| k8 | `@Bean OAuth2FeignRequestInterceptor oauth2FeignRequestInterceptor()` imported from `org.springframework.cloud.security.oauth2.client.feign` | Feign `synthetic` | The type's supertype is in a dependency: classpath knowledge, residue-like, not listed (the listed form is an auto-configured bean). |

The `WebClientCustomizer` question: Spring Boot applies `WebClientCustomizer` beans in `WebClientAutoConfiguration` to the prototype `WebClient.Builder` bean it defines; the static `WebClient.builder()` returns a fresh `DefaultWebClientBuilder` that no customizer touches. Injected builders are never plain (`j7`), so nothing to find. Not probed further.

## Public repositories (release HEAD, `--no-scip`; `--force --no-cache`, then `--force` cached: identical on all seven)

| Repository @ commit | Endpoints | Sites | Buckets (syn / heu / amb / ext / dyn / self / unm) | Bases verified / unverified | Carriers (sites) | `CONSUMES` by certainty |
|---|---|---|---|---|---|---|
| spring-petclinic-microservices @ `dc9ca15a7118` | 16 | 7 | 0 / 5 / 0 / 1 / 1 / 0 / 0 | 0 / 6 | 1 bare + 6 capped | 5 heuristic |
| node-express-realworld @ `30b68e1e8814` | 20 | 1 | 0 / 0 / 0 / 0 / 0 / 1 / 0 | 0 / 1 | 1 bare, hooked | — |
| golang-gin-realworld @ `626c372d2594` | 22 | 51 | 0 / 0 / 0 / 0 / 2 / 5 / 44 | 0 / 51 | 51 capped (`NewRequest`) | — |
| piggymetrics @ `6bb2cf9ddbca` | 11 | 4 | 0 / 0 / 0 / 0 / 0 / 0 / 4 | 0 / 4 | 4 Feign, capped (`RequestInterceptor` beans) | — |
| ftgo-application @ `558dfc53b11d` | 18 | 7 | 0 / 0 / 1 / 0 / 4 / 2 / 0 | 2 / 5 | 2 verified + 5 capped (4 `node-fetch` by `{ fetch }`) | 2 ambiguous |
| piomin @ `905529a715c3` | 16 | 4 | 0 / 4 / 0 / 0 / 0 / 0 / 0 | 0 / 0 | 4 Feign, plain | 4 heuristic (config client) |
| Store-Microservices @ `2c7b390d9ee2` | 92 | 20 | 0 / 4 / 0 / 0 / 15 / 1 / 0 | 19 / 1 | 19 verified + 1 capped | 4 heuristic |

Identical to the changelog's re-measurement. Verified carriers re-read in source (21): ftgo `ApiGatewayIntegrationTest.java` 60 and 96 (`WebClient client = WebClient.create("http://localhost:" + port + …)`, then only `client.post()`/`client.get()`); Store `WebClientService` 16 locals `WebClient webClient = WebClient.builder().baseUrl("http://" + val + ":8080").build()` and Identity `WebClientService` 3 locals `client` (16, 31, 45), every other token the receiver of a verb. All plain. **No `synthetic` `CONSUMES` on any public repository**, so none to re-read. None of the seven contains a W1–W4 form (grep: no `.kt`/`.kts`/`.vue`; ftgo's 19 `.groovy` files and the 4 `.html` files of petclinic and piggymetrics have no Feign, `fetch` or axios hook; no generated-marker client file; no `require("axios").default`/import-equals/destructured `require`; no `listOfServers` or `discovery.client.simple` key).

## New findings

### W1 — blocker (by the bar): hooks in files the scan never reads give false `synthetic`

- **Repro:** `k1`, `k1g`, `k2`, `k2h` above (Kotlin/Groovy Feign `RequestInterceptor` bean; a Vue SFC `axios.defaults.adapter`; an HTML inline `window.fetch = …`). Each hooked kind stays `synthetic`, `base: literal`; `rb path` `[synthetic]`.
- **Cause:** `global_hooks` runs only over parsed files (`crates/rb-indexer/src/stitch.rs:302–306`, `inputs.files`); `rb_syntax::Language` has no Kotlin, Groovy, Vue, Svelte or HTML (`crates/rb-git/src/lib.rs:135–155` names `kotlin` but nothing parses it; `.groovy`, `.vue`, `.svelte`, `.html` are no language).
- **Contract vs code:** amendment 16 and item 22 say "every token naming its configuration surface, **anywhere in the repository**"; the bar says "a repository-wide name-token scan". These are plain tokens in repository files, none of the residue categories.
- **Frequency:** mixed Java/Kotlin Spring services with Kotlin `@Configuration` are common; `axios.defaults…`/`axios.interceptors…` in a `.vue` component and inline `<script>` patches occur. Not on the seven public repositories.
- **Fix direction:** a raw-text word scan of the surface names (and `axios` bindings) over every non-binary file that is not parsed, capping the kind on any hit; or cap a kind when the repository has files of a language that can host its hooks (Kotlin/Groovy → Feign; `.vue`/`.svelte`/`.astro`/`.html` → `fetch`, axios); or list these file types as residue and rewrite the bar's "repository-wide".

### W2 — blocker (by the bar), rare: files the parser skips are not scanned

- **Repro:** `k4go` (`// Code generated by mockgen. DO NOT EDIT.` + `http.DefaultTransport = nil`), `k4ts` (`/** @generated */` + `axios.defaults.adapter`), `k4big` (a 1.03 MiB `vendor.js` with `globalThis.fetch = …`): the hooked kind stays `synthetic`.
- **Cause:** `crates/rb-indexer/src/syntax.rs:57–60` skips generated (`crates/rb-git/src/lib.rs:159–162`: `@generated` or `Code generated`+`DO NOT EDIT` in the first 1 KiB) and over-`MAX_PARSE_BYTES` (1 MiB, `crates/rb-syntax/src/lib.rs:70`) files, so they have no facts and no tokens.
- **Frequency:** a hook in generated code or a committed bundle is rare; same class as pass 6's V5. Close by scanning skipped files' text for the surface names (as for W1), or by listing it.

### W3 — blocker: axios module bindings the scan does not recognise, in parsed TS/JS

- **Repro:** `r1` `import axios = require("axios")`, `r2` `const ax = require("axios").default`, `r3` `const { default: ax } = require("axios")`, `r5` `import("axios").then((m) => { m.default.defaults.adapter = … })`, each with an `axios.defaults.adapter` hook in `web/src/hook.ts`: all five axios sites of `t.ts` stay **`synthetic`**.
- **Cause:** `module_names` (`crates/rb-frameworks/src/clients.rs:530–566`) takes import facts (no fact for a TS import-equals) and bindings whose initialiser chain is exactly one `require(<axios>)` link with no root — a `.default` member after it, a destructuring pattern, or a dynamic `import()` is not a binding.
- **Contract vs code:** item 22 (2): "axios bindings, imports and `require`s at any scope, subpaths included". Item 22's residue lists `require("axios").default.defaults…` **with no binding**; `r2` has a binding. `r2` is the CommonJS form the axios README prescribes for TypeScript typings.
- **Fix direction:** an axios binding is any local name whose declaration's initialiser mentions a `require("axios…")`/`import("axios…")` (any member or destructuring after it) or a TS `import x = require("axios…")`; or cap axios whenever a file names the `"axios"` module outside the allow-listed import forms (that also closes `r4`).

### W4 — blocker by the bar's wording (needs a ruling against F1): service-discovery configuration retargets a name-only Feign client

- **Repro:** `k5` (`@FeignClient(name = "orders")`, `application.yml` `spring.cloud.discovery.client.simple.instances.orders[0].uri: https://api.stripe.com`), `k5r` (`orders.ribbon.listOfServers=api.stripe.com:443`): Feign **`synthetic`** to `svc/orders`.
- **Why false:** a Feign client with no `url` goes through the load balancer, whose instance list for `orders` these keys set; the request goes to `api.stripe.com`. The settings scan already reads every settings file, but only for Feign client keys (`crates/rb-indexer/src/lib.rs:306–320`, `crates/rb-frameworks/src/clients.rs:428–435`).
- **Tension:** F1 accepted that a host naming a service by convention (D6) may reach `synthetic` without reading compose/k8s; item 10 records that convention's risk only for `heuristic`. These keys are scan-visible settings in the repository, so by the bar's wording this is a blocker; the orchestrator may instead rule it under F1 and list it (items 10 and 22). Cheapest close: cap name-only Feign sites (or every D6 host match) when any settings file sets `spring.cloud.discovery.client.simple.instances.<host>`, `<host>.ribbon.listOfServers` or `spring.cloud.loadbalancer…` keys.

### W5 — minor (test): the `require` subpath condition has no biting row

- Mutant E (`clients.rs:559`, `is_module(&m, module)` → `m == module`) survives all 323 tests. The behaviour is right end to end (`r8` capped), but the changelog's `ps6` row tests an ES subpath import, not a `require` of a subpath. Add a `require("axios/dist/node/axios.cjs")` hook row.

### W6 — doc: item 22's residue list misfiles one form and omits four

- `r4` (a static re-export `export { default as ax } from "axios"`) is listed as residue but is visible to a token scan; the bar defines residue as forms that defeat the scan by construction. Move it to costs (cap) or fix it with W3.
- Not named: `jest.spyOn(global, "fetch")` and `vi.stubGlobal("fetch", …)` (`s1`, `s2`; the usual TS test stubs; item 22's cost line implies a test stub caps every `fetch`, true only for the assignment spelling), `globalThis["fetch"] = …` with a literal key (`r6`), and a Feign interceptor bean declared by its concrete dependency type from a non-Feign package (`k8`). Each is `synthetic`; each fits a residue family (string-named property, classpath knowledge) by analogy only. Name them, or record string-literal `"fetch"` keys/arguments as tokens.

## Docs vs code

- Amendment 16, the corrections in amendments 14 and 15, CHANGELOG, IMPLEMENT.md, todo and the two lessons match the code for V1–V6; the public re-measurement reproduces byte for byte; the test count (468) is right.
- Amendment 16 / item 22 claim "anywhere in the repository" and "axios bindings, imports and `require`s at any scope": not what the code reads (W1–W3). Item 22's residue list misfiles `r4` and omits `s1`/`s2`/`r6`/`k8` (W6).
- README is unchanged ("M4a … awaits the evaluator's review") and contradicts nothing.
- The changelog's "63/63 mutants killed" is consistent with my run of 18 (17 killed; the survivor targets a condition the changelog's list does not name).

## Criteria and invariants (as now worded, given open items 1–23 and the closing bar)

| # | Verdict | Evidence |
|---|---|---|
| S1, S2 | PASS | facts recorded (`surface_names` in property/field/type positions, `Callee`, `Init::New.body`); mutants P, Q killed. |
| S3–S8 | PASS | routes, endpoints, handlers unchanged; goldens unchanged; public endpoint counts as in pass 6. |
| S9 | **FAIL** (W1, W2, W3) | axios instances and bare calls `synthetic` under a hook in a `.vue` file, a generated file, or through `require("axios").default`/import-equals/destructured `require`/`import()`; bare `fetch` `synthetic` under an HTML or >1 MiB-file hook. V3/V4 forms fixed. |
| S10 | **FAIL** (W1, W4) | Feign `synthetic` with a Kotlin or Groovy `RequestInterceptor` bean, or a discovery-config retarget of a name-only client. V1/V2 forms fixed. |
| S11 | PASS (W2 rare) | Go hooks caught in every parsed form tried (dot import, assignment to `DefaultClient`); only a generated-file hook escapes. |
| S12 | PASS | folding unchanged. |
| S13 | **FAIL** (W1–W4) | false `synthetic` stitches from carriers judged plain on scan-visible forms. |
| S14 | PASS | buckets sum on every dump (scratch 14/26, public 7/1/51/4/7/4/20). |
| S15–S18 | PASS | `rb path` lists sites with certainty; `request_path_goldens` passed twice. |
| S19 | PASS | growth tests green. |
| S20–S26 | PASS | e2e 17/17 twice, no `skipping:`, SCIP tests ran; fresh == cached on seven; AGE parity tests green. |
| S27 | PASS | public table reproduces the changelog; 21 verified carriers re-read, all plain; 0 `synthetic`. |
| S28 | **FAIL** | verification loop green, blockers W1–W4 open. |
| Inv 1, 2, 3, 5, 6, 8, 9, 10, 11, 12, 13 | PASS | inv 13: 0 differing rows vs `38d55e4` on petclinic, Store, ftgo; inv 9: fresh == cached on seven. |
| Inv 4 | **FAIL** | `synthetic` on requests whose carrier is reconfigured by a scan-visible form (W1–W4). |
| Inv 7 | **FAIL** | requests retargeted to `api.stripe.com` are path-matched `synthetic` to `svc/orders` (`k1`, `k2`, `k2h`, `r1`–`r3`, `r5`, `k5`). |

## Follow-ups

- **Before closing M4a:** W1 and W2: scan the text of every file that has no token facts (unparsed languages, generated, oversized) for the surface names and axios module mentions, capping the kind on a hit, or rewrite the bar to "every parsed file" and list the file types as residue. W3: recognise axios bindings from `require("axios…")` with a member or destructuring, TS import-equals and `import()`; rows `r1`, `r2`, `r3`, `r5` (and `r4` if moved out of residue). W4: a ruling (cap name-only Feign/D6 host matches under discovery keys, or list under F1). W5: one `require` subpath row. W6: residue list edits.
- **M4b/M5:** measure item 22's costs (unchanged from pass 6: petclinic 6/7, gin 51/51, piggymetrics 4/4, ftgo 5/7 capped); a test-source-only hook rule; string-literal `"fetch"` keys and `spyOn`/`stubGlobal` arguments as tokens; `field_chains_binding_chains_and_big_packages_stay_fast` also fails under mutants J and K (cause not investigated; probably a tick-count coupling to `global_hooks`).
- **M6:** Kotlin and Groovy parsing would turn W1's Feign half into ordinary scan coverage; compose/k8s/discovery host names (item 10).

VERDICT: FAIL
