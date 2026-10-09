# M6b contract: request-path precision — gin parameter groups, Express construction chains, in-process and empty Go requests, axios configurations, constructor-assigned members, userinfo holes, discovery narrowing, client breadth (draft, planner, 2026-10-09)

**Status:** contract written 2026-10-09; forks F1–F7 all as recommended (orchestrator, 2026-10-09); not started.

Written before any code. M6b is done only when every assertion below is backed by a passing test or a recorded measurement. "Verified facts" are what the 2026-10-09 probe read in the code, goldens and records at commit `3562b95` (M6a closed; `PIPELINE` `m6a-3`, Java recipe `m5-egress-1`, TS recipe `m6a-ts-1`).

This contract follows the M5/M6a format and applies, from the start, the lessons M3b–M6a paid for:
- the soundness claims and the closing bar are stated before any review (lesson 2026-10-06);
- every rule that admits data is an **allow-list** of positively identified forms (lessons 2026-10-06);
- a rule that suppresses data counts what it suppressed (lesson 2026-10-01);
- growth is tested by counted operations, never wall clock (lesson 2026-10-06);
- positions are never hand-counted in tests (lesson 2026-09-30);
- a sorted scan with a segment-aware predicate breaks on the string boundary (lesson 2026-10-09);
- invariant 1 compares dumps against the previous milestone's close with every changed line classified (lesson 2026-09-30).

## Soundness claims and closing bar (read first)

M6b's outputs whose failure would be silent: an endpoint key that merges two registrations or splits one; an exact/host-trusted edge to the wrong service from a composed prefix, a folded constant, an unseen base or an unconfigured client; a dropped request that was really network traffic; a `synthetic` restored by narrowing that the narrowed rule cannot see through. **Everything not proven takes the safe direction:** an unresolvable composition stays `prefix_unknown` (labelled, keyed apart); an unfodable value stays a hole; a contested base with an external candidate is external; a client form not proven plain is capped; narrowing only fires on positively resolved evidence.

| Surface | Claim (exactly this, no more) | Mechanism | Residue (out of scope by construction) |
|---|---|---|---|
| gin parameter groups (S2) | A composed prefix is exactly the string the call site passes (a literal, or a value folded by the existing constant rules, never a bare name), attached to the callee's registrations; one composed identity per (router site, call site). Registrations no call site reaches stay `prefix_unknown`. | M3a resolution of the callee (unique declaration only); the argument folded as a route prefix is folded (literals, constants, no holes); composition guarded (visited set, depth cap, per-site memo); the endpoint key keeps F3's router identity and gains the composed path, path certainty `heuristic` at most (composed across functions, M4a S7 rule). | A group built through a non-literal (a variable the folder cannot read, a loop, a map) stays unknown. A prefix that varies per request (a parameter holding two literals at different sites) is one node per call site, never merged. |
| Express chains (S3) | A `Router()` chain's `use(prefix, child)` links are read only from the chain's own call links; an anonymous default export binds only the single `Router()`/`express.Router()` construction it returns. Same key/certainty treatment as S2. | Chain walk over `CallChain` facts (plain links only); `default_export` names a construction, never an import (one level, as S19). | A chain through a helper function, a conditional, or a re-export stays unknown. |
| in-process / empty Go requests (S4) | A request value passed to `ServeHTTP`, an `httptest` recorder/response, or a mux it was registered on is never stitched; an empty URL is never path-matched. Both counted per file. | Positive sink patterns (`ServeHTTP`, `httptest.NewRecorder`/`NewResponseRecorder`, `mux.Handle` registration of the same handler) matched on the call graph from the `NewRequest` site (bounded walk, visited set); `""` goes to its own `unmatched` bucket (`http_empty_url`), never `/`. | A hand-rolled recorder (own `ResponseWriter` impl) is not recognised. A request sent both in-process and over the network keeps its network edge. |
| axios folding (S5) | A folded `baseURL`/config value comes only from: a literal, a same-file constant object (spread allowed, `MAX_SPREADS` kept), or a cross-file constant resolved through M3a to a unique declaration; a contested base with any external-literal candidate is external; a config passed by name that does not fold to a URL is `dynamic`, never suffix-matched on its tail. | `object_base_candidates` extended within the same allow-list; cross-file through `Lookup::external` + unique M3a resolution (ambiguous → unknown); Spring configuring statements ordered by source position (later wins, deterministically); contested-external rule before matching. | A base computed at runtime (env read through a wrapper, string concatenation across files, DI-provided options) stays unknown. |
| constructor members (S6) | `this.x`/`self.x` folds only when assigned exactly once, in a constructor, to a foldable value, with no other assignment token to that member anywhere in its scope. | Single-assignment proof by a scope-wide token scan of the member name (the Gate-A shape: declaration + harmless uses only); the value folded by the existing rules. | Members written outside constructors, written twice, or written through aliases stay holes. Other languages' member idioms (Python `self.x`, Go structs) are not covered. |
| userinfo holes (S7) | `http://u:${hole}@host/path` matches within the literal `host`; certainty never rises above what the rest of the URL earns. | Authority split keeps the post-`@` literal as the host; the userinfo hole marks only the userinfo unknown. | A hole in the host itself keeps today's rules. Credentials in URLs are never stored (only the host/path enter the graph; already the case). |
| discovery narrowing (S8) | A discovery/load-balancer cap is lifted only for positively resolved evidence: a type name resolving to a known discovery/LB type through imports, or JSON keys co-occurring inside one object literal. Everything else stays capped. Each lifted site is counted. | Import-resolved type check (unique M3a resolution; unresolved → capped); JSON co-occurrence within a single literal's span (not file-wide). | An unresolvable import, a dynamic registry, or keys spread across literals stay capped (over-capping residue shrinks, never inverts: no lifted `synthetic` is wrong by construction of the allow-list). |
| new clients (S9) | `got`, `ky`, `superagent`, `undici` enter as fetch-class sites (same gates: import/require presence, global-hook token scan extended with their names); Angular `HttpClient` and the axios member/namespace/direct-call forms enter capped at `heuristic`, never `synthetic`. | Kind-specific call shapes + the repository-wide token scan for configuration surfaces (`got.extend`, `ky.extend`, `superagent.agent`, `HttpClient` providers/interceptors as taints). | Interceptor/configured instances of the new libraries are capped like axios's. Python `requests`/`httpx` stay M6d. |
| inherited claims | M4a's `CONSUMES` bar (no false `synthetic` from any form a repository-wide scan can see), M4b's DI/RTA/orphan rules, M5's API/retention/egress claims stand. | Invariant 1 (below) + the unchanged e2e suites. | As in contracts M4a–M5. |

**What M6b must not change.** No new node or edge kind. Endpoint keys change only through S2/S3 (a `prefix_unknown` key gaining its composed path; classified G/E). `CONSUMES` edges change only by: (a) S4 removals (in-process/empty, counted); (b) S5–S7 re-bucketing (never above the stated certainty; every upgraded `heuristic`→exact/`synthetic` listed with its source); (c) S8 lifted caps (each listed); (d) S9 new sites (new edges only, heuristic max except fetch-class with hook proofs). `rb_indexer::PIPELINE` becomes `m6b-1` at the first dump change and is bumped on each later one.

This is invariant 1, checked against `3562b95` dumps of every fixture (with and without SCIP) and of the M6b measurement repositories at their pins, every differing row classified by script (G/E/C/F/U/D/B classes + `W-cov`-style consequences).

**Closing bar.** Decided now:
1. **Evaluator pass 1 is a full review,** adversarial probing of the claims above included (literal-vs-folded prefixes, chain shapes, recorder variants, config aliasing across files, double writes, userinfo edge forms, discovery evasion spellings, new-library configuration surfaces). Each finding is fixed test-first as an amendment.
2. **Evaluator pass 2 is verification-only** (re-runs every finding's repro + controls, the suite, mutants, invariant 1, determinism).
3. **Blocks closing only:** (a) a pass-1 fix that does not hold; (b) a regression; (c) a false edge above its stated certainty through a controlled form, a merged/split endpoint identity, or a dropped real request; (d) non-determinism; (e) a failed test/guard/step.
4. **Not blocking:** a new form in a residue class (recorded for its owner); a precision miss contradicting no rule; §12.5 timing (measured, not asserted).
5. **Timings are measured, not asserted** (hardware, VM allocation, loads, run counts with each number).

## Scope

**In (M6b):** the 9 items above (design §13 M6b row; D12: contract-m5 6, 7, 11, 12, 10-discovery/JSON parts, 17-M4a-16/19/20; contract-m4a 2 gin groups, 14 part; contract-m4b 1–2 shortfalls).

**Out of M6b (each assigned):** Spring YAML/profiles/gateways/compose/k8s hosts → M6g; Gradle/catalogs, pnpm/Yarn≥2, `go.work`, tsconfig scoping, image toolchains → M6e; DI/RTA/orphan items (M5 13, M4b 4–8, 14, 16–17, rule (e)) → M6c; Go/Python SCIP + `requests`/`httpx` → M6d; contract packs → M6f; Python member idioms → with M6d if cheap, else recorded.

## Engineer declaration

**Canonical state:** no migration, no table change, no new dependency planned (needs `serde_json`-style readers: none — all facts exist or are small syntax additions). Graph: no new node/edge kind; `PIPELINE` `m6b-1`… at each dump change. New counters only when non-zero (D3-style): `http_in_process`, `http_empty_url`, per-rule lifted-site counts.
**Derived:** composed prefixes, folded member/config values, narrowed caps, new client sites.
**Invariants:**
1. **Dumps.** `--no-scip` and SCIP dumps differ from `3562b95` only in classified rows (G/E/C/F/U/D/B + consequences); nothing unclassified.
2. **Keys.** One endpoint node per (service, method, path-or-tail, router identity); composition never merges two registrations and never splits one (router site is part of the identity).
3. **Certainty monotonicity.** No edge rises above its claim's maximum (`heuristic` for composed/cross-file/member/form-capped; `synthetic` only through the proven carriers of Gate A or the S8 allow-list); every rise is listed per edge.
4. **Drops counted.** Every S4 exclusion lands in exactly one counted bucket; buckets sum to sites (the M4a sum test extended).
5. **No silent fold.** A value folded from another file or a member is folded only through unique resolution / single-assignment proof; otherwise a hole.
6. **Direction of narrowing.** S8 lifts caps, never adds edges; the lifted set is enumerated.
7. **Determinism.** Fresh == cached, two names, forced rebuilds: byte-identical on fixtures and measurement repos.
8. **Bounded work.** Every new walk has a visited set + caps; growth ≤ 5× ops at 4× input per new walk.
9. **Stated complexity by counted operations** (S19-style tests for: param-group composition, chain walk, in-process reachability, cross-file fold, member scan).

## Decisions (made here, with reasons)

- **D1: one composed identity per (router site, call site).** A parameter group is composed at each call site from that site's argument value; two sites passing different literals yield two endpoint nodes (never merged). Rationale: the path genuinely differs per call; merging would join distinct registrations (the F3 lesson: identity first).
- **D2: composed paths are `heuristic` at most.** Cross-function composition is cross-file composition (M4a S7 rule). No `synthetic` through parameters.
- **D3: new counters only when non-zero** (M6a D3 rule), so unrelated dumps stay byte-identical.
- **D4: in-process and empty-URL sites are `unmatched` with their own diagnostic kinds** (`http_in_process`, `http_empty_url`), not silent drops and not edges.
- **D5: contested-external.** A contested base with any external-literal candidate is external (M4a item 21's stated rule, adopted as decided).
- **D6: member folding is TS/JS + Java only** (the folded-URL languages); Python/Go member idioms recorded for M6d.
- **D7: S8 narrowing is import-resolved or single-literal only.** A discovery word in parsed code lifts the cap only when its type/import resolves uniquely to a known discovery/LB type; JSON keys must co-occur in one literal. Raw-text hits stay capped (the raw scan cannot resolve).
- **D8: new-library `synthetic` needs the hook scan.** `got`/`ky`/`superagent`/`undici` reach `synthetic` only with the extended repository-wide token scan proving no configuration surface (extend/create/agent forms as taints); otherwise `heuristic`. `HttpClient` and member forms cap at `heuristic` always in M6b.
- **D9: measurement is `--no-scip` on public repos** (request paths need no SCIP tier: Go is Tier B; routes fold from facts; TS heuristic callers suffice for prefix/bucket measurement) **plus the full container e2e in verify** (fixtures cover SCIP-mode paths). Pins: `golang-gin-realworld@626c372d2594`, `gothinkster/node-express-realworld@30b68e1e8814`, `microservices-patterns/ftgo-application@558dfc53b11d`, `mattermost/mattermost@b2ce71db9c18` (M5's pin; Go in-process + empty-URL re-check), `spring-petclinic-microservices@dc9ca15a7118` + `piggymetrics@6bb2cf9ddbca` (no-regression guards). Never anything under `/Users/rohit/Public/alvora`. Shallow fetch, one repo at a time, `df` before each, 6 GiB floor, remove after each with `VACUUM`.
- **D10: fixture plan is code** (lesson 2026-10-06): extend `polyglot-monorepo` (has gin billing, TS web, Java auth) for gin param groups, member folding, axios configs, userinfo; Go in-process/empty-URL cases into billing; new-library clients into web. Check against the language/indexer rules before the fixture phase.
- **D11: every M5/M4a-numbered gap keeps its number** in records (e.g. "M5-6", "M4a-16") so the evaluator can trace each to its source row.
- **D12: dispositions.** M6b takes M5 6, 7, 11, 12, 10-discovery/JSON, 17-M4a-16/19/20; M4a 2 and 14-part; M4b S19-shortfall (contract-m4b item 2). Everything else stays where design §13 put it.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **Facts.** Member-assignment facts (TS/JS `this.x`, Java field writes in constructors, single-assignment tokens), new-library call gates as import/qualified facts, `ServeHTTP`/recorder sink patterns as call facts. Framework-neutral, fingerprinted; no golden change from facts alone. | `rb-syntax`/`rb-frameworks` units; e2e unchanged |
| S2 | **gin parameter groups.** `ArticlesRegister(v1.Group("/articles"))` shapes: literal, folded constant, one prefix per call site; unresolvable callee/argument stays `prefix_unknown` (keyed apart). Units per shape + visited-set/depth tests; growth test. | unit; mutation; e2e goldens; gin-realworld measured |
| S3 | **Express chains + anonymous defaults.** `Router().use(p, child)` chains bind; `export default Router().use(…)` binds the construction; re-export/import chains stay unknown (counted). Units + growth test. | unit; mutation; e2e goldens; node-express measured |
| S4 | **In-process + empty URL.** `NewRequest`→`ServeHTTP`/recorder/mux sites excluded into `http_in_process`; `""` into `http_empty_url`; buckets sum to sites; no edge from either. Units per sink + growth test. | unit; mutation; e2e goldens; mattermost re-measured (8 `ambiguous` gone) |
| S5 | **axios/certainty folding.** Same-file spreads, unique cross-file constants, by-name configs (`dynamic` unless folding to URL), contested-external, ordered Spring statements. One biting row per rule. | unit; mutation; e2e goldens; ftgo re-measured |
| S6 | **Constructor members.** Single-assignment proof; `this.consumerService`-style URLs fold; double/external writes stay holes. Units + growth test. | unit; mutation; e2e goldens; ftgo 4 `dynamic` re-measured |
| S7 | **userinfo holes.** `http://u:${p}@users/x` matched within `users`; certainty capped by the rest. Units per edge form. | unit; mutation; e2e goldens |
| S8 | **Discovery narrowing.** Import-resolved types + single-literal JSON keys lift caps (each lifted site counted); everything else capped. Calibration: 0 unintended lifts on fixtures + repos. | unit; mutation; e2e goldens |
| S9 | **Client breadth.** `got`/`ky`/`superagent`/`undici` sites with hook-scan proofs; `HttpClient` + member forms capped at `heuristic`. Per-library gate units (positive + negative). | unit; mutation; e2e goldens |
| S10 | **Fixtures + guards.** Polyglot extensions per D10; must-exist/must-not-exist per new edge class; precision guards with a biting mutation per line; dump checks. | e2e with containers; goldens |
| S11 | **Public repos.** gin-realworld prefix_unknown re-measured; node-express 19 re-measured; ftgo dynamics re-measured; mattermost 8 ambiguous + empty-URL re-measured; microservices + piggymetrics no-regression (byte-identical modulo classified rows). | recorded measurement |
| S12 | **Determinism + invariant 1.** Fresh == cached, two names, forced rebuilds on fixtures; `--no-scip` + SCIP dumps vs `3562b95` classified by script. | e2e; recorded |
| S13 | **Growth by counted operations** (invariant 9): each new walk ≤ 5× at 4×; a quadratic mutant of each bites. | unit |
| S14 | **Verification loop + closing.** `var/m6b/scripts/verify.sh` (paths from M5/M6a): fmt, clippy, counted units, store/server with DB, e2e + api with containers, listener + stub checks. Goldens explained by classified diffs. | verification loop |

## Verified facts (read at `3562b95`, 2026-10-09 probe)

1. gin: `routes.rs:720-746` `gin_prefix` (same-function/package, `MAX_GROUP_DEPTH=32`), `749-778` chain fold, `780-829` `gin()` (`*RouterGroup|IRouter|IRoutes` param → `None`); `RouteBase::Unknown` (`routes.rs:59-63`), router identity `274-291`, join `rb-indexer/src/routes.rs:301-356`, key `endpoint.rs:600-602`.
2. Express: `routes.rs:554-657` (binding-init chains only, no chain-built/anonymous); Composer `rb-indexer/src/routes.rs:69-218` (`MAX_MOUNT_DEPTH=32`, `MAX_PREFIXES=64`); S19 default-import lookup `190-201` (one level).
3. Go clients: `clients.rs:1983-2050` (no sink inspection); empty `""` → `/` via `endpoint.rs:44-96`; matters: mattermost 4 sites → 8 false `ambiguous`.
4. axios: `create_base:1082-1091`, `config_base:1099-1122`, `object_base_candidates:1134-1167` (`MAX_SPREADS=4`, same-file); `binding_value:fold.rs:208-246` holes for cross-file objects; Gate A `stitch.rs:107-113`.
5. Members: `fold.rs:222-223,242-244` holes (except Spring `clients.rs:1516-1627` for bases only); ftgo 4 `dynamic`.
6. userinfo: `endpoint.rs:254,307-310,429-467` (hole poisons host); item 19 kept.
7. Discovery: `clients.rs:524-687` words/keys/raw-scan; stitch cap `stitch.rs:114-117,778-781`; costs in contract-m5 amendment 11 item 5.
8. Client kinds: `clients.rs:35-41,140-157` (7 kinds); M4a-13 additions present; item-14 forms absent.
9. Fixtures: 5 (polyglot has gin billing + TS web); goldens per probe §0; M4a pins in CHANGELOG:822 + contract-m4b:347,549-576.

**Not verified (checked in phase 0, before the step that needs it):** gin-realworld's call-site shapes (which arguments: literals vs vars); node-express's chain/re-export shapes; ftgo's member/config shapes live; mattermost's 8 sites' sinks; the `HttpClient` import spellings on a real repo (none in measured repos — synthetic probe instead); S8 calibration counts.

## Fixture plan

Extend `fixtures/polyglot-monorepo/` (checked against language/indexer rules before the fixture phase):
- billing (gin): a `Register(api.Group("/v1"))` parameter group + a literal group (S2 controls); an in-process test client + an empty-URL call (S4, must-not-exist).
- web (TS): a `got`/`ky` client pair (plain + intercepted), an axios spread from an imported const + a contested base with an external candidate (S5), a `this.svc`-in-constructor client (S6), a userinfo-hole URL (S7), an `HttpClient`-shaped call (S9 capped).
- Goldens: must-exist/must-not-exist per class; `status-http.txt` buckets (`http_in_process`, `http_empty_url`); S22-style path goldens where routes move.

## Public-repository measurement plan

| Step | Repository @ pin | Runs | Notes |
|---|---|---|---|
| Phase 0 | — | `3562b95` binary dumps (`--no-scip`) of nothing yet; e2e green baseline | small |
| Phase 10 | gin-realworld `626c372d2594`, node-express `30b68e1e8814`, ftgo `558dfc53b11d` | `--no-scip` with `3562b95` (baseline) and M6b binaries | small clones; counts + classified diffs |
| Phase 10 | mattermost `b2ce71db9c18`, petclinic-microservices `dc9ca15a7118`, piggymetrics `6bb2cf9ddbca` | `--no-scip` both binaries | mattermost: 8 ambiguous + empty-URL re-check; others: no-regression guards |
| Phase 11 | records + evaluator passes | — | — |

No SCIP runs for measurement (`--no-scip` suffices per D9); containers only in the verify e2e. Disk policy as M6a D11 (6 GiB floor, one repo at a time, remove + `VACUUM`, part-by-part figures).

## Determinism

No wall-clock/host-path/run-order in dumps (unchanged rules). New walks are pure functions of facts + index. Fresh == cached, two names, forced rebuilds on fixtures (e2e); `--no-scip` both binaries byte-identical modulo classified rows.

## Verification loop

`var/m6b/scripts/verify.sh <tag>` (copied from M6a's, M6b paths/logs): fmt, clippy `-D warnings`, counted units, `rb-store`/`rb-server` with DB, e2e + api with containers, listener + stub checks, mutation runs per M4a harness rules.

## Lessons applied

| Lesson | Where |
|---|---|
| Soundness bar before review; close on verification | this section |
| Allow-lists, never deny-lists | S2 (folded literals/constants only), S4 (positive sink patterns), S5 (unique resolution), S6 (single-assignment proof), S8 (resolved evidence only), S9 (named configuration surfaces) |
| A rule that suppresses data counts it | D3 (`http_in_process`, `http_empty_url`, lifted-site counts) |
| Growth by counted operations | S13, invariant 9 |
| Never hand-count positions | S-assertions |
| String-boundary scans | member/config scans |
| Invariant vs previous close, classified | S12 |
| Fixture plan is code | D10 |

## Forks for the orchestrator

- **F1: gin composition granularity.** Recommendation: **one identity per (router site, call site)** (D1), never merged across sites.
- **F2: new-library maximum.** Recommendation: **`got`/`ky`/`superagent`/`undici` fetch-class (synthetic-eligible with hook proofs); `HttpClient` + member forms capped at `heuristic`** (D8).
- **F3: `HttpClient` alternative.** Recommendation: **not deferred** — heuristic-capped sites in M6b (detection value now, DI proof later in M6j).
- **F4: in-process/empty-URL reporting.** Recommendation: **counted `unmatched` buckets** (D4), never silent.
- **F5: contested-external.** Recommendation: **adopted as decided** (D5; M4a item 21's rule).
- **F6: S8 narrowing depth.** Recommendation: **import-resolved + single-literal only** (D7); raw-text stays capped.
- **F7: measurement mode.** Recommendation: **`--no-scip` on public repos + container e2e in verify** (D9).

## Amendments

1. **(phase 0) Baselines, pins and shape checks.** Baseline binary `var/m6b/base/rb-3562b95` (HEAD's code = `3562b95`'s; sha256 `ede3e290…55aad5c`, independently reproduced by the pass-2 evaluator's own release build). Fixture dumps with it (`var/m6b/base/{noscip,scip}/`): `--no-scip` and `--scip` hashes equal M6a's closing ones on all 5 fixtures. Shape checks on shallow checkouts (removed after reading; never under `/Users/rohit/Public/alvora`): gin-realworld — `Register(v1.Group("/users"))` literal args into `*gin.RouterGroup` params, nested through `v1 := r.Group("/api")` (composition must fold the receiver chain too); node-express — `routes.ts` holds `const api = Router().use(tagsController)…` (prefix-less chain links over default imports) plus `export default Router().use('/api', api)` (anonymous default of a construction over a local const); ftgo — `this.consumerService = \`${options.baseUrl}/consumers\`` in the constructor, used as `${this.consumerService}/${consumerId}`. Full ftgo pin: `558dfc53b11d30a5f1d995c0c6d58d5106c28189` (contract D9's short form supplemented; HEAD == short form per `ls-remote`). `HttpClient`: no occurrence in any measured repo (item 14) → covered by a synthetic probe in the fixture phase, not a public repo. Images verified by inspect; DB 0 repos after the dumps; disk accounted per step.

## Amendments

2. **(phase 1) Member writes carry their constructor context.** TypeScript constructors are not definitions (established: their parameters sit in the class), so `enclosing` alone cannot answer "assigned in a constructor" for TS. Each `MemberWrite` therefore carries `ctor: bool`, decided at extraction from the nearest function-like ancestor (`method_definition` named `constructor`, `constructor_declaration`; a nested function is not a constructor write — the callback may run later). Compound assignments (`+=`, `||=`, `??=`) count as writes (read and write: the safe direction for counting). Bare-field writes are not members. New-library gates and sink patterns need no new facts (imports, calls with args, and `Binding.type_name` already carry them); the pack phases use them as-is.

3. **(phase 2) Composed keys are path-based; identity lives in evidence.** A composed endpoint's key is the normal `endpoint_key(method, path, service)` (no `?router` suffix), so identical compositions merge and literal registrations join them; the (router site, call site) identity is one `EXPOSES`/`HANDLED_BY` evidence row per site (D1). The `prefix_unknown` node stays iff no call site was found or some site did not resolve-or-fold (safe direction). One level only (a group passed onward stays unknown); `Arg::Other` folds through `chain_at(span)`, `Arg::Name` through its initialiser's call chain; the 64-prefix mount cap does not apply (no cross-level multiplication — removed during the phase when it broke per-site identity).

4. **(phase 3) Chain-built and anonymous routers without new name facts.** `RouterDecl.binding` is `Option<usize>` (one consumer: the unknown-fallback identity, `{path}#default` when `None`). `router_kind` reads the first link only, so `const api = Router().use(…)` binds; every `use` link on a bound router's chains is a mount (single-link behavior preserved). The anonymous default is matched by the new `default_export_expr` span fact against a Router-starting chain (exactly one default per module); unexported inline constructions bind nothing (no false endpoints from dead code). The default-import fallback resolves `None` to the `"default"`-named decl. Mutants M-c/M-d/M-e bite at unit and end-to-end level.

5. **(phase 4) In-process and empty-URL Go requests.** The pack computes `FileClients.in_process` (spans of `NewRequest` chains whose request value reaches a sink): seeds are non-member bindings whose statement holds the `NewRequest` chain and no sibling chain (`req, err := …`; a wrapper or a `range` puts a sibling in, and stays out — Go's arity rules make a two-name single-call statement the only seed shape in valid code); three positive sink patterns (`ServeHTTP` with the name, co-pass with an `httptest.NewRecorder`/`NewResponseRecorder` value bound or inline, a call of a mux-registered handler); the walk follows bare names through uniquely named plain same-file functions (depth cap 8, visited set); every use site resolves through scope-indexed resolution (the same filter and maximum as `binding_index_at`, cached per definition and name, so the walk stays linear — per-site full scans made it 15× at 4× input); a `Do` with the name unmarks (sent both ways: the network wins). Registration is file-global; members, cross-file flows, reassignments and multi-value passthroughs stay out (documented). The stitcher buckets these sites `unmatched` with detail `in_process` (sink evidence dominates), and empty URLs (no literal text, no holes — `""` normalises to `/`, so the check comes first) with detail `empty_url`, at site level for every client kind; both get their own diagnostic kinds (D4) and non-zero-only `http_coverage` splits (D3), with `unmatched` counting plain-unmatched only so the buckets still sum to the sites (invariant 4, sum test extended). `rb status` prints `in-process`/`empty-url` after `unmatched`, zero-omitted; the Service Card's `unstitched` counts the new kinds. Fixture rows per D10 (billing `main_test.go`: a recorder-served foreign-absolute request, a recorder co-pass with no `ServeHTTP`, an empty URL next to a plain call) with three must-not-exist guards; the previously-`synthetic`-shaped case is `heuristic` pre-fix (`NewRequest` carriers are never verified) and still bites. `PIPELINE m6b-3`. Mutants killed: visited gate (9,958 vs 124 ops), `Do` carve-out, `ServeHTTP`/recorder/mux rules, seed siblings, kind override, coverage split, empty check.
