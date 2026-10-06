# M4b evaluator pass 2 (verification only)

- Date: 2026-10-07
- Commit: `304fe4a` (fix of pass 1, contract amendment 21, `PIPELINE` `m4b-di-8`)
- Environment: `DOCKER_CONTEXT=desktop-linux`, DB `127.0.0.1:55433`; release binary of HEAD; release binaries of `531e14e` (pass 1's commit) and `28e47ba` (M4a's closing commit) built from `git archive` in throwaway target directories. JDK 21 (Homebrew), Spring 6.1.13 / Boot 3.3.4 jars from the local `.m2` cache, Go 1.26.2 (offline, `GOPROXY=off`).
- Scope (closing bar 2): pass-1 repros and controls, regression suite, mutations, public-repository measurements. Blocking only on closing bar 3 (a)–(e).

## 1. Pass-1 findings rebuilt end to end

Scratch repositories under `var/eval-m4b-2/scratch/` (`git init`, one commit, Spring Boot parent pom or `package.json` with `scripts.start`, plus a `Dockerfile`, so `role = service`), indexed `--no-scip` with HEAD and with `531e14e`. Every repro reproduces the pass-1 failure on `531e14e` and gives the expected result on HEAD.

| # | Repro (HEAD) | `531e14e` | HEAD | Runtime confirmation | Holds |
|---|---|---|---|---|---|
| Y1 | `rta1`: `Holder$LoadedImpl` in `META-INF/services`, `Class.forName("com.acme.Holder2$ReflectImpl")` | `rta: 5 of 5` proven; `LoadedImpl`/`ReflectImpl` pruned | both kept (`string` 2); `rb callers … --precision rta` lists `App#use()` via dispatch | JDK 21 run prints `loaded`, `reflect` | yes |
| Y2 (Java) | `rta1`: local class in a lambda extending `LambdaBase`; local class in an anonymous `Runnable` body extending `AnonBase` (extra form) | both proven, `LambdaBase.greet` "(no callers found)", 1 pruned | both kept (`mention`), `use()` listed under `--precision rta` | prints `lambda-local`, `anon-local` | yes |
| Y2 (TS) | `rta2`: `const Derived = class extends ExprBase {}`; named form `class NamedDerived extends NamedExprBase` | `3 of 3` proven | `0 of 3`; `ExprBase.greet` and `NamedExprBase.greet` keep `use()` under `rta` | — (no node on host, by rule) | yes |
| Y7 | `rta3`: `import * as commands from "./commands/index"` + `Object.values`; extra form `import * as more from "./more"` with `export * from "./star"` | `3 of 3` proven | `DeployCommand`, `StarCommand` kept (`mention`); control `NeverCommand` (no module object names it) still proven and pruned only under `--precision rta` | — | yes |
| Y14 | `rta4`: names in `tools/gen.py`, `tools/main.go`, `conf/plugins.txt`, `conf/deps.lock` | `4 of 5` proven (Py, Go, txt kept nothing) | `1 of 5`: only the control `Control` proven | — | yes |
| Y3 | `orph1`: `AuthInterceptor extends BaseInterceptor implements HandlerInterceptor`; extra forms: depth 2 (`DeepInterceptor extends MidInterceptor extends BaseInterceptor`), in-repo interface extending `ApplicationListener` | `rb impact Probe.probe` lists `preHandle`, `tokenPresent`; same for the two extra forms | 0 orphans for all three; control `Probe.probeHelper` still lists `Probe#onlyHelper()` | — | yes |
| Y4 | `di2`: `@Import(SmsNotifier.class)` of a `@Primary` class outside the scan root | `Alerts → EmailNotifier`, `unique`, no `capped_by` (bucket `plain`) | `Alerts → EmailNotifier` `heuristic`, `capped_by: [mention]`, bucket `capped` | Spring 6.1.13 context: `Alerts -> sms` | yes, as amended (see N1) |
| Y5 | `di2`: `Store<String>` with `StringStore` and `@Primary IntStore` | `→ IntStore`, `resolution: primary` | two `ambiguous` edges (`IntStore`, `StringStore`), `capped_by: [generic]`, no `primary` | `Catalog -> s` (StringStore) | yes |
| Y6 | `di2`: `@Named("fast")` point and bean, `@Primary SlowCodec` | `→ SlowCodec`, `primary`, `annotation_unread` | `→ FastCodec`, `resolution: qualifier`, `qualifier: fast` | `Encoder -> fast` | yes |
| Y13 | `di2`: two `@Primary` and a third | ambiguous over 3 (unchanged) | ambiguous over 3 | Spring: `NoUniqueBeanDefinitionException … more than one 'primary' bean found among candidates: [routeA, routeB, routeC]` | yes |
| Controls | `Timer(Clock)` one bean; `Printer(Ink)` single `@Primary` | `unique` / `primary` | unchanged (`plain`) | `Timer -> system`, `Printer -> black` | yes |
| Y8 | `pkg1/svc`: `dup` at 1.0 and 2.0 | two `exact` edges | one `heuristic` edge to `dup@?`, `version_from: unresolved`, `versions: [1.0, 2.0]`; control `twice` (4.0 twice) stays `exact` `@4.0` | — (no Maven on host) | yes |
| Y11 | `lib.version` defined 1.0 then 2.0 | `lib@1.0` `property` | `lib@?` `heuristic`, `versions: [1.0, 2.0]`; control `same@3.1` `property` unchanged | — | yes |
| Y9 | `pkg1/web`: `"foo": "npm:bar@^1.0.0"`, extra scoped form `"@scope/alias": "npm:@real/pkg@^2.0.0"` | `npm/foo@1.2.3`, `npm/@scope/alias@2.4.0` (`exact`) | `npm/bar@1.2.3` and `npm/@real/pkg@2.4.0`, `exact`, `alias` set; control `left-pad@1.3.0` unchanged | — | yes |
| Y10 | `pkg1/gosvc`: unversioned replace of gin by v1.9.1; `replace golang.org/x/text v0.13.0 => …` with `v0.14.0` required; `uuid` replaced by another module | `gin@v1.10.0`, `uuid@v1.6.0` | `gin@v1.9.1` `synthetic`, `replaced`; `x/text@v0.14.0` (replace not applicable); `uuid@?` `unresolved`, `replaced: github.com/gofrs/uuid v4.4.0+incompatible` | `go list -m` (offline): `gin v1.10.0 => gin v1.9.1`; `uuid v1.6.0 => gofrs/uuid v4.4.0+incompatible`; `x/text v0.14.0 replace=<nil>` | yes |
| Y12 | mutant R4 (`is_none_or` → `is_some_and` in the (i) seed) | survived in pass 1 | killed by `rta::tests::an_unconsidered_instantiated_subtype_keeps_its_supertype` | — | yes |
| Y15 | open items 16–22, weaker-false-results list, S9 (c) wording | — | present and consistent with the code read (see §4) | — | yes (N1, N2 wording notes) |

A Java `main` that builds an `AnnotationConfigApplicationContext` itself (my first DI repo `di1`) trips gate D (`surface`) on every point, as S6 intends; `di2` is the same repo without it, which is pass 1's shape.

## 2. Mutations (own, 21; harness `var/eval-m4b-2/mut/mut.py`: exact single replacement, fresh mtime, `cargo test -p <crates> --lib` after `cargo clean -p rb-syntax -p rb-frameworks -p rb-indexer`, restore by copy with fresh mtime, `cmp` bytes; `git status --short` empty after the batch)

| Id | Amendment-21 condition mutated | Result |
|---|---|---|
| X1 | string words: `$` segments off (`mentions.rs`) | killed (`rta::tests::binary_names_and_every_text_file_keep_a_type`) |
| X2 | raw-text words: `$` segments off (`clients.rs`) | killed (`clients::tests::raw_scan_collects_words_and_di_surface`) |
| X3 | supertype role regardless of the naming type being a definition | killed (`rta::tests::subclasses_that_are_no_definitions_keep_their_base`) |
| X4 | class expression's own name treated as a declaration | killed (`mentions::tests::supertype_of_a_type_that_is_no_definition_is_other`) |
| X5 | S9 (d) word scan of every non-binary file off | killed (`binary_names_and_every_text_file_keep_a_type`) |
| X6 | module-object rule off | killed (`rta::tests::module_objects_keep_their_types`) |
| X7 | computed `require`/`import()` (`*`) names nothing | killed (same) |
| X8 | orphan rule (e) walk not transitive | killed (`liveness::tests::external_supertypes_through_repository_bases_keep_members`) |
| X9 | depth cap counts as internal | killed (same) |
| X10 | gate M over registrants off | killed (`di::tests::registrations_outside_the_candidates_cap_by_mention`) |
| X11 | unread annotation no longer blocks `@Primary`/name | killed (`di::tests::primary_never_decides_what_the_facts_cannot`) |
| X12 | type arguments no longer block `@Primary`/name | killed (same) |
| X13 | JSR-330 `@Named` not a qualifier | killed (same) |
| X14 | collection of a generic element not `ambiguous` | killed (same) |
| X15 | Maven duplicate versions: `> 1` → `> 2` | killed (`packages::tests::duplicate_declarations_and_properties_are_unresolved`) |
| X16 | npm alias keyed by the alias name | killed (`packages::tests::npm_aliases_key_the_real_package`) |
| X17 | `replace` for another version applied to any version | killed (`packages::tests::go_replace_directives`) |
| X18 | `replace` by another module treated as a version change | killed (same) |
| X19 | D2: `primaries.len() >= 1` | killed (`primary_never_decides_what_the_facts_cannot`) |
| X20 | R4: (i) seeds only considered kept types | killed (`an_unconsidered_instantiated_subtype_keeps_its_supertype`) |
| X21 | `rb callers` default back to `rta` (`main.rs`), e2e with DB | killed (5 e2e tests: `code_question_goldens`, `spring_di_rta_and_orphan_goldens`, three `scip_*`) |

21/21 killed. The harness was stopped (batch finished) before the e2e runs; `main.rs` restored and `cmp`-identical.

## 3. Public repositories (release, `--no-scip`)

Pins checked out and confirmed: spring-petclinic `818c4136ea971c21674525f9053de0d9c7ad8cfe`, spring-petclinic-microservices `dc9ca15a71189bc4be6c02518d7180232c0555b7`, gothinkster/spring-boot-realworld-example-app `ee17e31aafe733d98c4853c8b9a74d7f2f6c924a`, jhipster-sample-app `6b000b5d23a36c45e01472471b84a44fa2464044`.

| Repository | dump lines | fresh (`--force --no-cache`) == cached (`--force`) | vs `531e14e` | `INJECTS` | RTA proven | orphan-eligible | `manifest` packages | wall s (no cache / cached) |
|---|---|---|---|---|---|---|---|---|
| spring-petclinic | 1,556 | yes | byte-identical | 6 | 0 / 8 | 0 | 28 | 1.20 / 0.37 |
| spring-petclinic-microservices | 1,740 | yes | byte-identical | 15 | 0 / 1 | 0 | 126 | 0.50 / 0.42 |
| realworld-spring | 2,336 | yes | byte-identical | 24 | 0 / 23 | 6 | 0 (Gradle) | 0.32 / 0.28 |
| jhipster-sample-app | 8,689 | yes | **8 diff lines** (4 rows): `SearchWithPagination`'s two `IMPLEMENTS` lose `impl_instantiated: false`; `rta_coverage` proven 1 → 0, mention 13 → 14; `orphan_coverage` mention 211 → 205, supertype 34 → 40 (eligible 35 both) | 19 | 0 / 16 | 35 | 101 | 0.78 / 0.61 |

Exactly the diff amendment 21 claims. No `/Users/` in any dump (public or fixture).

**Samples** (each read in the source):
- **`INJECTS`**: 64 edges over the four repos, sorted bytewise by (src, dst), N = 3, indices 1, 4, …, 58: **20/20 correct** (microservices: `VisitsServiceClient(WebClient.Builder)` → `ApiGatewayApplication#loadBalancedWebClientBuilder()`, the only such `@Bean`; three Spring Data repositories; `PetclinicTools` → `@Service AIDataProvider`; jhipster: `jwtDecoder(SecurityMetersService)` → `@Service`, `AuthenticateController(JwtEncoder)` → `@Bean jwtEncoder()`, four Spring Data repositories; realworld: eight Lombok `@AllArgsConstructor` fields → `@Service`/`@Repository` (`MyBatisArticleRepository`, the only `ArticleRepository` implementation); petclinic `OwnerController` → `OwnerRepository`). 0 `synthetic`.
- **Orphans**: `rb impact` on 20 callers of eligible symbols (10 per repo with eligible symbols, M3b rule); 9 orphans listed (jhipster: `SecurityUtils#extractPrincipal()`, `headerToString`, `decodeHeaderValue`, `HeaderMessage`, `formatAsBytes`, `Contexts`, `BankAccountFormDefaults`, `AuthorityFormDefaults`, `AlertType`; realworld: none). `git grep -w` of each: every occurrence is its declaration or inside the removed function/type or another orphan: **9/9 correct**.
- **Package versions**: 255 `manifest` edges (petclinic, microservices, jhipster), N = 12, indices 6, 18, …, 234: **20/20** re-derived from the pom/`package.json` (gatling `3.15.1` and webjars bootstrap `5.3.3`, okhttp `5.0.0-alpha.14` from same-pom properties; `liquibase.version` defined only in the parent → `@?`; versionless Boot starters → `@?`; jhipster has no `package-lock.json` → npm `@?` with `declared`).

## 4. Regression

| Check | Result |
|---|---|
| `cargo fmt --all --check` | exit 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | exit 0 |
| Unit tests (`cargo test --workspace --lib --bins` after `cargo clean -p` of the three touched crates, awk sum of 9 `test result:` lines) | **559 passed, 0 failed** (545 + 14 new) |
| e2e with DB and containers (`--nocapture`), run 1 / run 2 | **18/18, 18/18**, 0 `skipping`; SCIP fresh indexes ran (scip-java 9.7 s, polyglot 26.0 s) |
| Invariant 1 vs `28e47ba` (M4b additive attributes stripped) | spring-petclinic-microservices and jhipster-sample-app: **0 rows removed**; added rows only `Package` nodes, `manifest` `DEPENDS_ON`, `INJECTS`, `di_*` diagnostics and the three coverage rows |
| Determinism on fixtures (`git archive` copies; fresh `--no-cache`, cached, second repository name) | `cmp`-identical on java-spring-jpa (244 lines), polyglot-monorepo (308), ts-express-prisma (74); `KafkaAuditSink` still the one proven type |
| Host paths | none in 14 dumps or the goldens |
| Stub scan (`todo!`, `unimplemented!`, `TODO`, `FIXME` in `crates/`) | only `rb-sql/src/lib.rs:1111` (a SQL test input) |
| Docs vs code | amendment 21 matches the code read (each rule located and mutated above); D5 table and S10 header text match `rb … --help` and the printed header; README `--precision` paragraph matches; open items 1–22 present; soundness table rows updated (`opt-in since amendment 21`, binary names, module objects, transitive (e)). Notes N1, N2 below. |

## Notes (non-blocking; closing bar 3 does not apply)

- **N1 — doc wording, Y4.** The fix caps the `@Import` form (`capped_by: [mention]`, bucket `capped`), as amendment 21 states, but the edge still names `EmailNotifier`, the bean Spring does not inject (`SmsNotifier`). The contract's "Removed by amendment 21" sentence lists "`INJECTS` that ignored a class registered by `@Import` (now capped by `mention`)" among removed false results; it is a remaining weaker false `heuristic` (flagged), not a removed one. Closing records move it to the weaker-false-results list. No `synthetic` is involved (F2), so not (c).
- **N2 — stale design text.** `docs/repository-brain-design.md` §13's M4b row still says "`precision=rta` default for Q1/Q4", §10's `rb path` example header shows `precision=rta`, and §6.1's manifest note does not mention amendment 21's duplicate, alias and `replace` rules. Fixed in the closing records.

## M5 adversarial-review list (forms in residue classes or needing new facts; not hunted further)

1. **`@Import`-registered `@Primary` bean outside the scan roots (Y4 residue):** gate M caps the point but narrowing does not model registration by `@Import`, so the single `heuristic` edge names the non-primary scanned bean. Modelling `@Import`/`@Bean` registrants as candidates is a new fact use.
2. Open item 3: Spring Data fragment implementations as orphans.
3. Open item 20: computed class names (string concatenation, template holes, data, binary resources) for RTA and orphans.
4. Open item 21: orphan rule (e) through a wrong unique Tier B supertype resolution.
5. Open item 18: bean registration by forms the repository text does not name (library `@EnableXxx`, auto-configuration).
6. Maven's choice between duplicate declarations (Y8/Y11) is unconfirmed (no Maven on the host); the `@?` answer is safe either way.

## S / invariant table (as now worded)

| # | Verdict | Evidence |
|---|---|---|
| S1 | PASS | X1, X3, X4 killed; Y1/Y2 repros |
| S2 | PASS | `@Named` stereotype/qualifier (Y6, X13) |
| S3 | PASS | units; realworld Lombok samples 8/8 |
| S4 | PASS | Y5, Y6, Y13 repros match Spring or are `ambiguous`; X11–X14, X19 |
| S5 | PASS | 0 `synthetic` `INJECTS` anywhere |
| S6 | PASS | Y4 capped by `mention`; X10 |
| S7 | PASS | buckets sum to points on every dump read |
| S8 | PASS | Spring Data samples |
| S9 | PASS | Y1, Y2, Y7, Y14 repros; X1–X7, X20 |
| S10 | PASS | default `cha` everywhere; opt-in header; X21 |
| S11 | PASS | goldens |
| S12 | PASS | Y3 repro (depth 1, 2, interface); X8, X9 |
| S13 | PASS | 9/9 public orphans; control orphan still listed |
| S14 | PASS | goldens |
| S15 | PASS | Y8–Y11 repros, `go list` agreement; X15–X18; 20/20 versions |
| S16 | PASS | units, goldens |
| S17 | PASS | units, goldens |
| S18 | PASS | goldens |
| S19 | PASS as amended | open item 2 |
| S20 | PASS | units |
| S21 | PASS | units |
| S22 | PASS | recorded (pass 1) |
| S23 | PASS | units |
| S24 | PASS | e2e twice |
| S25 | PASS | e2e with containers, 0 `skipping` |
| S26 | PASS | fresh == cached on 4 public repos; fixtures fresh == cached == second name |
| S27 | PASS | measurement table and samples above |
| S28 | PASS | verification loop green; pass-1 findings fixed |
| Inv 1 | PASS | 0 rows removed vs `28e47ba` on two public repos; three repos byte-identical to `531e14e` |
| Inv 2 | PASS | all `INJECTS` `di`, Symbol → Symbol, ≤ `heuristic` |
| Inv 3 | PASS | no merge conflicts |
| Inv 4 | PASS | Y1–Y3, Y7, Y14 now keep; versions unresolved when unproven |
| Inv 5 | PASS | `@Named` only via `jakarta.inject`/`javax.inject` imports |
| Inv 6 | PASS | |
| Inv 7 | PASS | |
| Inv 8 | PASS | X9 (depth cap conservative) |
| Inv 9 | PASS (units) | |
| Inv 10 | PASS | e2e dump checks |
| Inv 11 | PASS | coverage sums |

## Cleanup

`var/eval-m4b-2/` (clones, scratch repos, throwaway binaries, dumps) removed; every `ev4b2_*` repository and its AGE graph dropped one at a time (before the e2e runs and at the end; `left: 0`). `rb.scip_index` has 32 rows before and after (no SCIP run of mine).

VERDICT: PASS
