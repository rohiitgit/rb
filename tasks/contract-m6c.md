# M6c contract: DI, RTA and orphan precision — registrants, gate M on main sources, RTA yield, module roles and objects, rule (e), autowired non-beans, inherited Spring Data methods, Lombok qualifiers, generics and custom qualifiers (draft, planner, 2026-10-10)

**Status:** contract written 2026-10-10; forks F1–F7 all as recommended (user, 2026-10-10, by "continue"); phases 0–1 in progress (see `tasks/todo.md`).

Written before any code. M6c is done only when every assertion below is backed by a passing test or a recorded measurement. "Verified facts" are what the 2026-10-10 probe read in the code, goldens and records at commit `10a9ad5` (M6b closed; `PIPELINE` `m6b-8`).

This contract follows the M5/M6a/M6b format and applies, from the start, the lessons M3b–M6b paid for:
- the soundness claims and the closing bar are stated before any review (lesson 2026-10-06);
- every rule that admits data is an **allow-list** of positively identified forms (lessons 2026-10-06);
- a rule that suppresses data counts what it suppressed (lesson 2026-10-01);
- growth is tested by counted operations, never wall clock (lesson 2026-10-06);
- positions are never hand-counted in tests (lesson 2026-09-30);
- a sorted scan with a segment-aware predicate breaks on the string boundary (lesson 2026-10-09);
- invariant 1 compares dumps against the previous milestone's close with every changed line classified (lesson 2026-09-30);
- fixture plan is code (lesson 2026-10-06);
- evaluator pass 1 probes configuration surfaces and allow-list edges (lesson 2026-10-10).

## Soundness claims and closing bar (read first)

M6c's outputs whose failure would be silent: an `INJECTS` edge to the wrong bean (a missed registrant, a test-capped point restored wrongly, a generic/custom-qualifier tie broken wrongly); a pruned-but-instantiated type (an over-eager RTA yield rule); a false orphan (a narrowed rule (e) or module-object resolution that drops a real use); a Q3 path through a misresolved inherited method. **Everything not proven takes the safe direction:** an unresolvable registrant caps, never picks; an unread qualifier or config keeps `ambiguous`; an unproven type stays instantiated; an unresolvable module object keeps every type; an unresolvable import head stays external; an inherited call with no allow-listed name stays unresolved.

| Surface | Claim (exactly this, no more) | Mechanism | Residue (out of scope by construction) |
|---|---|---|---|
| @Bean registrants (S1) | An `@Import`-named config's `@Bean` methods join a point's candidates only when each is main-source, non-collection point, with at least one scanned candidate beside them; the point goes `ambiguous`, never single. | Class-literal import evidence (existing `literal_registrants` shape) extended to the named class's `@Bean` methods by assignable return type; `undecided` blocks `@Primary`/name exactly as S19 (g). | A `@Bean` method in an unnamed config, a collection point's registrants, a point with no scanned candidate (stays `unresolved`: no new edge). |
| gate M main-only (S2) | A test-path `ClassLiteral` mention never caps a main-source point; every other disallowed role, and every main-source mention, caps exactly as today. | `rb_model::is_test_path` on the mentioning file; role check unchanged. | Other test-path roles still cap (narrowing is class-literal-only); a main-source literal caps as before. |
| RTA yield (S3) | A Maven module with `spring-boot-maven-plugin` (repackage) or a `@SpringBootApplication` class is deployable (`role=service`); yield (considered/proven per rule histogram) is measured, never asserted. | Pom XML read (main `pom.xml` files only) + class fact; role change flows through the existing deployable consumers (RTA, orphans, DI classpath, cards) with every consequence classified. | Test-classpath proving stays as today (needs its own soundness argument: residue); Gradle modules without poms stay as read. |
| module objects (S4) | A `tsconfig.json` `paths` alias or `package.json` `exports`/`main` subpath resolves a module object exactly; anything unresolvable keeps every type, as today. | Manifest readers over the repository's `tsconfig.json`/`package.json` texts (nearest-scope-first for paths); shared by RTA's `module_may_name` and Tier B resolution; each narrowed keep counted. | Computed `require`/`import()` keeps all; bundler aliases unmodelled; non-TS languages unchanged. |
| rule (e) (S5) | A supertype head counts external only when its resolution actually reached an on-demand/wildcard import; same-package and single-type-import resolutions count in-repo. | Resolution-level tracking through `type_candidates` (extended where the level is not exposed); unique-and-bound rules otherwise unchanged. | An ambiguous head still follows every candidate (M4b open 21 stands). |
| non-bean consumers (S6) | Injection points of `@Bean`-returned objects and `ConstraintValidator` implementations resolve to candidates as new `heuristic` edges, each listed; nothing else changes bucket. | Consumer = the `@Bean` method symbol / the validator class; candidates by the unchanged assignability + qualifier rules; F2's heuristic cap. | Other container-built objects (filters, listeners) stay `not_a_bean`. |
| inherited Spring Data (S7) | An allow-listed Spring Data method name called on a repository-typed receiver resolves to the repository interface node with an `inherited` attr; any other unresolved member stays unresolved. | Receiver type via DI's Spring Data beans; name allow-list (`save*`, `find*`, `get*`, `delete*`, `count*`, `exists*`, batch variants); no new node kind. | Custom base-interface default methods outside the allow-list stay unresolved. |
| Lombok qualifiers (S8) | A field `@Qualifier` narrows only when the nearest `lombok.config` chain positively lists its annotation under `copyableAnnotations`; otherwise `ambiguous` as today. | Nearest-enclosing `lombok.config` chain (bubbling, `=`/`+=`/`!` forms); unparseable or absent config stays unread. Each narrowed point counted. | Other `lombok.config` keys unmodelled; Kotlin/Groovy out of scope. |
| generics/qualifiers (S9) | A point picks a single bean only on exact simple-name type-arg agreement, or a custom qualifier resolved through a unique import to a `@Qualifier`-meta-annotated declaration; otherwise `ambiguous` as today. | `Binding.type_args` vs bean supertype type-args (wildcards never agree); meta-annotation declaration search (same-file, else unique M3a import resolution). Each pick counted. | Bounded wildcards, nested-parameterisation agreement, non-unique meta-annotations stay ambiguous. |

**What M6c must not change.** No new node or edge kind (new edge *attributes* only: a registrant `origin`, `inherited`, narrowed `resolution` values). `INJECTS` edges change only by: (a) S1 registrant joins (ambiguous only); (b) S2 lifted caps (plain restorations, each counted); (c) S6/S7 new edges (heuristic max, each listed); (d) S8/S9 narrowings (single picks, each listed). `rb_indexer::PIPELINE` becomes `m6c-1` at the first dump change and is bumped on each later one.

This is invariant 1, checked against `10a9ad5` dumps of every fixture (with and without SCIP) and of the M6c measurement repositories at their pins, every differing row classified by script (G/E/C/F/U/D/B classes + `W-cov`-style consequences).

**Closing bar.** Decided now:
1. **Evaluator pass 1 is a full review,** adversarial probing of the claims above included (registrant evasion spellings, test-path smuggling, plugin/app-class spoofing, alias cycles, on-demand shadowing, validator/filter consumers, non-listed inherited names, config bubbling tricks, wildcard/bound generics, meta-annotation chains). Each finding is fixed test-first as an amendment.
2. **Evaluator pass 2 is verification-only** (re-runs every finding's repro + controls, the suite, mutants, invariant 1, determinism).
3. **Blocks closing only:** (a) a pass-1 fix that does not hold; (b) a regression; (c) a false `INJECTS` above `heuristic`, a pruned-but-instantiated type, a false orphan, or a Q3 path through a misresolved method, through a controlled form; (d) non-determinism; (e) a failed test/guard/step.
4. **Not blocking:** a new form in a residue class (recorded for its owner); a precision miss contradicting no rule; §12.5 timing (measured, not asserted).
5. **Timings are measured, not asserted** (hardware, VM allocation, loads, run counts with each number).

## Scope

**In (M6c):** the 9 items above (design §13 M6c row; D12: contract-m5 9-part, 13, 17-M4a-16/19/20-discovery/JSON-parts-done-in-M6b; contract-m4b 4, 5, 6, 7, 8, 14, 16, 17, 19; contract-m5 10-rule-(e)-part).

**Out of M6c (each assigned):** Go/Python SCIP + `requests`/`httpx` + Python member idioms → M6d; Gradle/pnpm/Yarn/`go.work`/tsconfig-scoping → M6e; contract packs → M6f; topology/config packs (incl. Spring YAML/profiles/gateways) → M6g; Kotlin/C# → M6i; Guice/Dagger/NestJS/Spring-XML/.NET → M6j; Groovy dropped (unchanged).

## Engineer declaration

**Canonical state:** no migration, no table change planned (new readers: `lombok.config`, `tsconfig.json` paths, `package.json` exports — all plain-text, no new dependency). Graph: no new node/edge kind; `PIPELINE` `m6c-1`… at each dump change. New counters only when non-zero (D3-style): lifted-cap counts, narrowed-pick counts, per-rule RTA/orphan deltas ride in existing coverages.
**Derived:** registrant candidates, narrowed caps, deployable roles, resolved module objects, in-repo heads, non-bean edges, inherited targets, qualifier narrowings.
**Invariants:**
1. **Dumps.** `--no-scip` and SCIP dumps differ from `10a9ad5` only in classified rows (G/E/C/F/U/D/B + consequences); nothing unclassified.
2. **Kinds.** No new node or edge kind; new attributes only, each named here.
3. **Certainty monotonicity.** `INJECTS` never above `heuristic` (M4b F2); a single pick only through S1 (ambiguous — never single), S8, S9 (each listed per edge); every rise listed.
4. **Lifts counted.** Every S2/S5 cap lifted lands in exactly one counted bucket; buckets sum to points (the M4a sum tests extended).
5. **No silent resolution.** Unique imports, exact agreement, positively read configs — otherwise a hole/cap/external.
6. **Direction of narrowing.** S2/S4/S5 lift caps or keep types alive, never add edges; the lifted/kept sets are enumerated. Only S1/S6/S7 add edges (each listed).
7. **Determinism.** Fresh == cached, two names, forced rebuilds: byte-identical on fixtures and measurement repos.
8. **Bounded work.** Every new walk has a visited set + caps; growth ≤ 5× ops at 4× input per new walk.
9. **Stated complexity by counted operations** (S19-style tests for: registrant join, gate-M scan, deployable read, alias/export resolution, level-tracked walk, validator/bean-method scan, inherited-name match, config-chain read, agreement/meta-annotation resolution).

## Decisions (made here, with reasons)

- **D1: registrants join, never lead.** An `@Bean`-method registrant joins a point that already has a scanned candidate (ambiguous, S19 (g) shape). A point with no scanned candidate stays `unresolved` — an edge to a registrant alone would be a new edge to an unproven carrier (the S19 (g) lesson: a tie-breaker never runs past an unmodelled filter).
- **D2: gate M narrows by path, not by role breadth.** Only `ClassLiteral`-role mentions in test paths (`rb_model::is_test_path`: `test`/`tests`/`__tests__` dirs, `*Test.java` etc.) stop capping. All other disallowed roles still cap everywhere: the item names literals, and widening further needs its own residue case.
- **D3: deployable is plugin-or-app-class, and its blast radius is classified, not feared.** `spring-boot-maven-plugin` in the module pom or a `@SpringBootApplication` class in the module ⇒ `role=service`. Roles feed RTA, orphans, DI classpath and cards; every consequence row is classified W-consequence, never silently absorbed.
- **D4: aliases resolve exactly or not at all.** A `paths`/`exports` mapping that resolves uniquely wins; multiple/failed mappings keep every type (today's behaviour). Nearest `tsconfig.json` wins for nested scopes.
- **D5: levels, not presence.** An on-demand import counts against a head only when that head's resolution reached it (same-package and single-type-import resolutions short-circuit first). Presence alone (M5 S19 (e)) was the over-cap.
- **D6: new consumers are heuristic-only.** S6's `@Bean`-method and validator consumers enter at `heuristic` with the unchanged candidate rules (F2); no Gate-A-style proof is attempted in M6c.
- **D7: inherited targets are interface nodes, never synthetic members.** No external symbols exist (M3b amendment 11); the allow-list names the Spring Data surface; the `inherited` attr marks the approximation for Q3.
- **D8: configs are read nearest-first, and anything unreadable stays capped.** `lombok.config` bubbles directory-up; a broken chain, an unknown key, or a missing annotation entry keeps `ambiguous`. The reader supports `=`/`+=`/`!` only.
- **D9: agreement is exact simple names; meta-annotations resolve uniquely.** Wildcards, bounds and nested parameterisations never agree (ambiguous); a meta-annotation found twice (or via wildcard) stays unread.
- **D10: fixture plan is code** (lesson 2026-10-06): extend `java-spring-jpa` (has auth + broken services; needs a test source added for S2) for registrants, non-bean consumers, inherited calls, Lombok qualifiers + `lombok.config`, generic + custom-qualifier points. Check against the language/indexer rules before the fixture phase.
- **D11: every M5/M4a-numbered gap keeps its number** in records so the evaluator can trace each to its source row.
- **D12: dispositions.** M6c takes M5 9-part, 13, 17-M4a-16/19 and the rule-(e) part of M5 10; M4b 4, 5, 6, 7, 8, 14, 16, 17, 19. Everything else stays where design §13 put it.
- **D13: measurement is `--no-scip` on public repos** (DI/RTA/orphans need no SCIP tier for counting; SCIP-mode paths are covered by the container e2e) **plus the full container e2e in verify.** Pins (full SHAs verified by API in phase 0): `gothinkster/spring-boot-realworld-example-app@ee17e31aafe733d98c4853c8b9a74d7f2f6c924a` (DI-heavy: points, gates, Lombok), `jhipster/jhipster-sample-app@6b000b5d23a36c45e01472471b84a44fa2464044` (qualifiers, `@Bean` factories, Angular), `spring-projects/spring-petclinic@818c4136ea971c21674525f9053de0d9c7ad8cfe` (roles, RTA), `spring-petclinic/spring-petclinic-microservices@dc9ca15a71189bc4be6c02518d7180232c0555b7` (role flip), `sqshq/piggymetrics@6bb2cf9ddbca980b664d3edbb6ff775d75369278` (no-regression), `elunez/eladmin@55fbf705956949697dbd68bf9003776609d3d029` (Lombok angles, Gradle `--no-scip`). Shallow fetch, one repo at a time, `df` before each, 6 GiB floor, remove after each with `VACUUM`. Never anything under `/Users/rohit/Public/alvora`.
- **D14: no new node or edge kind.** New attributes only (`origin` registrant-beanmethod, `inherited`, narrowed `resolution` values), each named in its assertion.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **Registrants.** `@Import`-named configs' `@Bean` methods join points with scanned candidates (ambiguous, `origin` set); collections/no-candidate points unchanged. One biting row per shape. | unit; mutation; e2e goldens; realworld measured |
| S2 | **Gate M main-only.** Test-path class literals no longer cap; lifted points counted; everything else caps as today. Calibration: 0 unintended lifts on fixtures + repos. | unit; mutation; e2e goldens |
| S3 | **RTA yield.** Deployable via plugin-or-appclass; yield (considered/proven per first-failing rule) measured on DI-heavy repos; no new unsound prune. | unit; mutation; recorded measurement |
| S4 | **Module objects.** `paths`/`exports` exact resolution for RTA + Tier B; unresolvable keeps all (each narrowed keep counted). | unit; mutation; e2e goldens |
| S5 | **Rule (e).** Level-tracked walk; same-package/single-import heads count in-repo (newly eligible counted); ambiguous heads unchanged. | unit; mutation; e2e goldens |
| S6 | **Non-bean consumers.** `@Bean`-method + validator points resolve as new `heuristic` edges, each listed; nothing else changes bucket. | unit; mutation; e2e goldens; realworld measured |
| S7 | **Inherited Spring Data.** Allow-listed names on repo receivers → interface node + `inherited`; others unresolved. Per-name gate units. | unit; mutation; e2e goldens |
| S8 | **Lombok qualifiers.** Nearest-chain `copyableAnnotations` reading; listed annotations narrow (counted); all else ambiguous. | unit; mutation; e2e goldens; eladmin measured |
| S9 | **Generics/custom.** Exact type-arg agreement picks (counted); unique-import meta-annotations narrow (counted); wildcards/non-unique stay ambiguous. | unit; mutation; e2e goldens; jhipster measured |
| S10 | **Fixtures + guards.** D10 extensions; must-exist/must-not-exist per new edge class; precision guards with a biting mutation per line; dump checks. | e2e with containers; goldens |
| S11 | **Public repos.** DI buckets/gates, RTA yield + histograms, orphan shares, INJECTS samples on the D13 set; role-flip consequences classified; no-regression guards. | recorded measurement |
| S12 | **Determinism + invariant 1.** Fresh == cached, two names, forced rebuilds on fixtures; `--no-scip` + SCIP dumps vs `10a9ad5` classified by script. | e2e; recorded |
| S13 | **Growth by counted operations** (invariant 9): each new walk ≤ 5× at 4×; a quadratic mutant of each bites. | unit |
| S14 | **Verification loop + closing.** `var/m6c/scripts/verify.sh` (paths from M6b): fmt, clippy, counted units, store/server with DB, e2e + api with containers, listener + stub checks. Goldens explained by classified diffs. | verification loop |

## Verified facts (read at `10a9ad5`, 2026-10-10 probe)

1. DI: `crates/rb-indexer/src/di.rs` — `resolve()` 194–417, `resolve_point()` 534–879, `MAX_CANDIDATES = 16` (30), `assignable()` BFS depth 64 (160–179), `literal_registrants()` 504–527, `registrants()` 454–498, gate M 220–235 + 673–683, `MENTION_ALLOWED` 182–191, `@Primary`/name 706–768 with `undecided` 725, buckets S7 (52–62), `di_coverage` in `add_injects()` 884–923.
2. RTA: `crates/rb-indexer/src/rta.rs` — `RULES` 21–32, `prove()` 79–201, `ALLOWED` 61–65, `module_may_name()` 207–242, `apply()` 358–399, invariants + linearity tests 924–997.
3. Orphans: `crates/rb-indexer/src/liveness.rs` — `RULES` 28–37, `own_rule()` 360–422, `external_supertype()` 431–454, `mentions_accounted()` 546–564, invariants + linearity 1141–1182.
4. Shared: `rb_model::is_test_path` (`crates/rb-model/src/lib.rs:193`); `PIPELINE m6b-8` (`crates/rb-indexer/src/lib.rs:104`, per-phase doc convention 60–103); frameworks DI facts `crates/rb-frameworks/src/di.rs` (origins 57–72, `repository()` 681–724, `feign_client()` 727–776, Lombok 240–265/1011–1085, generics 824–865, qualifiers 390–400).
5. Fixtures: `java-spring-jpa` has auth + broken services and no test dir (S2 needs one added); `polyglot-monorepo` unchanged by M6c scope (no DI shapes added there unless the fixture phase says so).
6. Pins: realworld `ee17e31aafe733d98c4853c8b9a74d7f2f6c924a`, jhipster `6b000b5d23a36c45e01472471b84a44fa2464044`, petclinic `818c4136ea971c21674525f9053de0d9c7ad8cfe` (`spring-projects`, not `spring-petclinic`), microservices `dc9ca15a71189bc4be6c02518d7180232c0555b7`, piggymetrics `6bb2cf9ddbca980b664d3edbb6ff775d75369278`, eladmin `55fbf705956949697dbd68bf9003776609d3d029` (full SHAs verified by API in phase 0; orgs in D13).

**Not verified (checked in phase 0, before the step that needs it):** full-SHA pins via API; which realworld/jhipster points hit each gate; microservices modules' pom plugin/app-class shapes; eladmin's Lombok qualifier shapes; tsconfig `paths`/exports shapes on jhipster; `lombok.config` absence/presence on fixtures and repos; S3/S11 calibration counts.

## Fixture plan

Extend `fixtures/java-spring-jpa/` (checked against language/indexer rules before the fixture phase):
- auth: an `@Import`ed config with a `@Bean` method beside a scanned bean (S1 ambiguous pair); a `@Bean`-returned type with an `@Autowired` member + a `ConstraintValidator` with an injected member (S6 capped edges); an inherited repository call (`save`/`findById` on the existing `UserRepository`, S7); a Lombok field `@Qualifier` + a `lombok.config` (S8); a generic point (`Notifier<Sms>`-shaped) + a custom-qualifier point with its meta-annotation (S9).
- A test source with a class-literal `@Import` (S2 control: caps nothing).
- Goldens: must-exist/must-not-exist per class; `status-di.txt` buckets; S22-style Q3 goldens where call targets move.

## Public-repository measurement plan

| Step | Repository @ pin | Runs | Notes |
|---|---|---|---|
| Phase 0 | — | `10a9ad5` binary dumps (`--no-scip`) of nothing yet; e2e green baseline | small |
| Phase 11 | realworld `ee17e31aafe7`, jhipster `6b000b5d23a3`, petclinic `818c4136ea97` | `--no-scip` with `10a9ad5` (baseline) and M6c binaries | DI-heavy; buckets/gates/yield/shares + INJECTS samples |
| Phase 11 | microservices `dc9ca15a7118`, piggymetrics `6bb2cf9ddbca`, eladmin `55fbf705` | `--no-scip` both binaries | role-flip consequences; no-regression; Lombok angles |
| Phase 12 | records + evaluator passes | — | — |

No SCIP runs for measurement (`--no-scip` suffices per D13); containers only in the verify e2e. Disk policy as M6b D9 (6 GiB floor, one repo at a time, remove + `VACUUM`, part-by-part figures).

## Determinism

No wall-clock/host-path/run-order in dumps (unchanged rules). New walks are pure functions of facts + index. Fresh == cached, two names, forced rebuilds on fixtures (e2e); `--no-scip` both binaries byte-identical modulo classified rows.

## Verification loop

`var/m6c/scripts/verify.sh <tag>` (copied from M6b's, M6c paths/logs): fmt, clippy `-D warnings`, counted units, `rb-store`/`rb-server` with DB, e2e + api with containers, listener + stub checks, mutation runs per M4a harness rules.

## Lessons applied

| Lesson | Where |
|---|---|
| Soundness bar before review; close on verification | this section |
| Allow-lists, never deny-lists | S1 (import-named configs), S2 (class-literal-only), S3 (plugin-or-appclass), S4 (exact-or-keep-all), S5 (level-reached), S6 (two consumer classes), S7 (name allow-list), S8 (listed annotations), S9 (exact agreement, unique resolution) |
| A rule that suppresses data counts it | D3 consequences, lifted/narrowed counts (D3-style non-zero-only) |
| Growth by counted operations | S13, invariant 9 |
| Never hand-count positions | S-assertions |
| String-boundary scans | mention/config scans |
| Invariant vs previous close, classified | S12 |
| Fixture plan is code | D10 |
| Probe config surfaces + allow-list edges | S-assertions (evaluator pass 1 battery) |

## Forks for the orchestrator

- **F1: registrant shape.** Recommendation: **join-as-ambiguous** (D1), never a lone edge to a `@Bean` method.
- **F2: gate-M breadth.** Recommendation: **class-literal-only in test paths** (D2); other roles still cap.
- **F3: deployable signal.** Recommendation: **plugin-or-appclass** (D3); either suffices.
- **F4: module-object depth.** Recommendation: **RTA + Tier B together** (D4) via one shared reader; split by amendment if Tier B proves too wide.
- **F5: inherited target.** Recommendation: **repository interface node + `inherited` attr** (D7); no synthetic members.
- **F6: lombok.config scope.** Recommendation: **nearest-chain bubbling** (D8); root-only is unsound against nearer removals.
- **F7: measurement mode.** Recommendation: **`--no-scip` on public repos + container e2e in verify** (D13).

## Amendments

1. **(phase 0) Baselines, pins and shape checks.** Baseline binary `var/m6c/base/rb-10a9ad5` (HEAD's code = `10a9ad5`'s — the contract commit adds no code; sha256 `264fec11…5add1b`, release build). Fixture dumps with it (`var/m6c/base/{noscip,scip}/`): `--no-scip` polyglot-monorepo, java-lombok and ts-workspaces dumps byte-identical to their goldens; ts-express-prisma and java-spring-jpa have no dump goldens (guards only) — hashes recorded. SCIP hashes equal M6a's recorded values where known (java-lombok `696599b9`, 84 lines; ts-workspaces `210f33fe`, 116 lines); the e2e compares SCIP goldens on `--no-scip` runs for lombok/workspaces, so hand-run SCIP dumps (whose symbol-join plumbing only the e2e drives faithfully) are recorded, not compared — a stated deviation. Full-SHA pins verified by API (petclinic org is `spring-projects`, not `spring-petclinic`; D13/V§6 corrected). Shape checks on shallow checkouts (removed after reading; never under `/Users/rohit/Public/alvora`): realworld is Gradle (no pom — S3's app-class signal carries it there), has `ConstraintValidator`s and no `@Import`/`@Qualifier` in main; jhipster has `@Import`/`@Bean`/`@Qualifier` shapes but no tsconfig `paths`/`exports` in its webapp (S4 paths-shapes ride on fixture probes); microservices root pom has the plugin plus 8 `@SpringBootApplication` classes; eladmin has no Lombok-qualifier shapes and no `lombok.config` exists on any measured repo or fixture (S8 repo legs are vacuous-by-absence guards); fixtures have no test dirs (S2 adds one) and no tsconfig `paths`; on-demand imports on fixtures only in annotation packages (`jakarta.persistence.*`, `springframework.web.bind.annotation.*` — S5 per-head reachability decides). Images verified by inspect; DB 0 repos after the dumps (`delete_repo` + `VACUUM`); disk 20G → 19G free.

2. **(phase 1) `@Bean`-method registrants.** `di::bean_method_registrants` maps each type to the main-source `@Bean` methods assignable to it whose declaring class a main-source `@Import` names (value + array items, `X.class` shapes; test sources register nothing); resolve joins them beside scanned beans with `origin: registrant` (shared `registered` vec, so `undecided`/counts/merge behave as S19 (g)). Y4's residue comment named exactly this gap — its test now expects the ambiguous join (predicted update). No dump changes on fixtures (no `@Import`-named configs there): e2e byte-identical, no `PIPELINE` bump. Mutants killed: registrant-drop, import-name, class-suffix, test-filter (×2 loops), bean-check, scanned-dedup (shared line; needed a candidates-count assertion since the merge hides the duplicate), array-items.

3. **(phase 2) Gate M main-only.** The `mentioned` set splits: test-path `ClassLiteral` mentions no longer land in it (D2); a `mentioned_broad` set keeps today's rule for the count. `gate_m_broad` mirrors `gate_m` arm-for-arm (element, candidates, registrants — structurally overlapping by construction); resolve counts a point in `mention_lifted` iff broad would cap it and its label is plain, reported non-zero-only in `di_coverage`. S19g's and S1's test-source rows now expect the plain row (predicted updates — M4b item 5 closed). No dump changes on fixtures (no test dirs there): e2e byte-identical, no `PIPELINE` bump. Mutants killed: exemption-drop, broad-empty, count-drop, coverage-drop, broad-closure-false, broad-registrants-drop; broad-candidates documented as jointly covered (arm overlap provably unseparable — same names flow through `simple()` and registrants).

4. **(phase 3) Deployable roles.** `app_services` (main-source `@SpringBootApplication` classes by module — Java-only, test-excluded, classes only) feeds one `deployable_services` predicate for service roles, the DI classpath and RTA alike (single source: never `role=service` without deployable or vice versa). The pom plugin half of D3 already existed (`pom_xml`); parent-inherited builds need no new read (app-class covers every measured case). Tests first (role flip + library/test/TS/method controls, linear growth). Mutants killed: union-drop, test-exclusion, annotation, Java gate, class gate, quadratic rescan. No fixture flips (their app modules already read service): e2e byte-identical, no `PIPELINE` bump yet.

5. **(phase 4) Module objects.** `rta::AliasMaps` reads `tsconfig.json` `paths` (+ `baseUrl`) and `package.json` `exports`/`main` (nearest scope wins; multi-star/computed/`extends`/non-string targets resolve nothing) and is shared by Tier B (`Modules::with_aliases` + `probe`, so bare specifiers resolve exactly or stay external as before) and RTA (`module_may_name` narrows only to the resolved file, else falls through to the approximate rule: keep-all). Precedence is exact-beats-pattern with longest-pattern winning (TypeScript/Node rules); a pattern with several targets is ambiguous and resolves nothing (D4). `tsconfig.json` joined `packages::is_input` so its texts reach the readers; `resolve` ticks once per tried pattern (counted). `PIPELINE m6c-1` (first dump change). Tests: Tier B bare-specifier unit, RTA exact/nowhere/unknown-package/multi-target end-to-end rows, precedence unit (exact-vs-longer-pattern, longest-wins, exports exact), `tsconfig-is-input` pin, linear growth (single-pattern maps; N-patterns x N-specs is O(N^2) by construction, so patterns stay config-bounded — first growth draft measured 1275 -> 20100 and was rewritten). Fixture `legacy/` gains `main.ts` (aliased + unknown-package imports), `models/user.ts`, `ext.ts` (`express` import, uninstalled) with 1 must-exist + 1 must-not-exist guard; SCIP resolves the alias too (guard key quotes the basename only: `src/models/`user.ts``). Mutants killed: rta-branch-dead, tier-b-dead, first-target-wins, first-match-wins, exports-exact-dropped (survived first: the test's exact-vs-`./*` case also resolves through longest-match, so the test gained the discriminating `./a`-vs-`./a*` shape), tsconfig-gate-dropped; build-dropped survives lib tests by construction (both unit harnesses build the maps directly) and is killed by the e2e golden instead (dump reverts, golden mismatches in ~2s). Goldens classified: +21 dump rows (3 new symbols + DEFINED_IN/IN_SERVICE, 1 heuristic CALLS, orphan library 22 -> 25, +2 unresolved-ref rows for the uninstalled imports), must-exist +1, must-not-exist +1, scip-status (3 -> 4 exact calls, 2 -> 5 defs, references 1 -> 2). Verify `var/m6c/logs/p4-*`: fmt, clippy `-D warnings`, 739 unit, 66 store, 12 server, 32/32 e2e (no `skipping:`), 1/1 api; final tree re-checked (fmt, workspace clippy, indexer 256/256) with `scip_ts_workspaces` green on it. DB 0 repos, 0 leftover graphs (one killed-run graph via `rb gc`).
