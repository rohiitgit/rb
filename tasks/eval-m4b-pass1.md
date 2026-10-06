# M4b evaluator pass 1 (full review)

- Date: 2026-10-06
- Commit: `531e14e` (M4b phase 16), `PIPELINE` `m4b-di-7`
- Environment: `DOCKER_CONTEXT=desktop-linux`, DB `127.0.0.1:55433`, release binary of HEAD; baseline binary of `28e47ba` built from `git archive` (throwaway target dir).

## Measured baseline

| Check | Result |
|---|---|
| `cargo fmt --all --check` | clean (exit 0) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean (exit 0) |
| Unit tests (`cargo test --workspace --lib --bins`, summed from the log) | **545 passed, 0 failed** |
| e2e with DB and containers, run 1 / run 2 | **18 passed / 18 passed**, 0 `skipping:` in either log |
| Stub scan (`todo!`, `unimplemented!`, `TODO`, `FIXME` in `crates/`) | clean (one hit is a SQL test input string) |
| Host paths in dumps (`/Users/` in 14 public-repo dumps, goldens) | none |
| README examples | `rb status` `di:`/`rta:` lines and the `rb path "POST /api/v1/login"` block reproduce byte-for-byte on `git ls-files` copies of the fixtures |
| Determinism | fresh (`--force --no-cache`) == cached (`--force`) on 7 public repos; a second repo name is byte-identical on 3 (petclinic, node-express-realworld, jhipster); SCIP (scip-java) cached == fresh on spring-petclinic |
| Invariant 1 vs `28e47ba` (7 public repos, M4b's additive attributes stripped) | **0 rows removed** on all 7; added rows are only `Package` nodes, `manifest` `DEPENDS_ON`, `INJECTS`, `di_*` and the three coverage rows; no `CALLS`/`REFERENCES`/`IMPLEMENTS`/`CONSUMES`/`EXPOSES`/`HANDLED_BY` change |
| Public-repo counts (`--no-scip`) | `INJECTS`: petclinic 6, microservices 15, realworld-spring 24, piggymetrics 19, petclinic-rest 33 (24 ambiguous): identical to the recorded measurement; RTA proven 0 on all six pinned repos |

Public repositories re-run (pins checked out and confirmed): spring-petclinic `818c4136ea97`, spring-petclinic-microservices `dc9ca15a7118`, gothinkster/spring-boot-realworld-example-app `ee17e31aafe7`, piggymetrics `6bb2cf9ddbca`, spring-petclinic-rest `afc8fc1d3b8e`, node-express-realworld `30b68e1e8814`. **New repository:** jhipster/jhipster-sample-app @ `6b000b5d23a36c45e01472471b84a44fa2464044` (Spring Boot, constructor DI, Spring Data JPA, `@Qualifier`, `@Bean` factories, Angular front end): 103 points (capped 19, external 29, test 54, unresolved 1; gates `surface` 19, `mention` 4, `external_type` 4), 19 `INJECTS`, RTA 16 types / 1 proven (`SearchWithPagination`, an unused TS interface: correct), 35 / 1,647 orphan-eligible, 101 `manifest` packages (no lockfile: npm unresolved).

## Criteria

| # | Verdict | Evidence |
|---|---|---|
| S1 | PASS | mention/binding units pass; no golden churn claim not re-tested beyond e2e |
| S2 | PASS | units; mutants D4 (decapitalize), D5 (Lombok final/`@NonNull`) killed |
| S3 | PASS | units; Lombok rows bite (D5) |
| S4 | **FAIL** | Y5, Y6: generic type arguments and JSR-330 `@Named` are not narrowing inputs, so `@Primary` picks a bean Spring never injects |
| S5 | PASS (under F2) | no `synthetic` `INJECTS` anywhere (fixtures, 7 public repos, scratch repos) |
| S6 | **FAIL** | Y4: gate M checks only the surviving candidates; an `@Import(X.class)`-registered `@Primary` bean outside the scan roots leaves a `plain`, uncapped, wrong `unique` edge |
| S7 | PASS | buckets sum to points on every dump read |
| S8 | PASS | units, guards |
| S9 | **FAIL** | Y1, Y2, Y7: types instantiated at run time are proven never instantiated |
| S10 | PASS (mechanics) | pruning counted; but it prunes Y1/Y2/Y7's real callers |
| S11 | PASS | units, goldens |
| S12 | **FAIL** | Y3: rule (e) reads only the declaring class's own supertypes; an override of an external method through an in-repo base is eligible and listed as an orphan |
| S13 | PASS (mechanics) | mutants O5, O6 killed; output is only as good as S12 |
| S14 | PASS | jhipster `bank_account` read against source; matches; recorded misses (inherited CRUD) only |
| S15 | **FAIL** | Y8 (two `exact` versions for one Maven artifact), Y9 (npm alias keyed under the alias name, `exact`), Y10 (go.mod `replace` ignored) |
| S16 | PASS | units; not attacked further |
| S17 | PASS | card units; mutant E3 killed by `card::tests` (survives e2e) |
| S18 | PASS | goldens |
| S19 | PASS as amended | amendment 14's shortfall recorded (open item 2) |
| S20 | PASS | units |
| S21 | PASS | units |
| S22 | PASS | recorded; not re-mutated |
| S23 | PASS (units) | growth/cycle units pass; not independently re-measured |
| S24 | PASS | e2e twice |
| S25 | PASS | e2e twice with containers, no `skipping:` |
| S26 | PASS | determinism above; AGE parity checks inside e2e |
| S27 | PASS (measurement) | counts reproduce; samples below |
| S28 | PASS (loop) / FAIL (closing) | verification loop green; evaluator findings open |
| Inv 1 | PASS | 0 rows removed on 7 repos vs `28e47ba` |
| Inv 2 | PASS | all `INJECTS` `di`, Symbol→Symbol, ≤ heuristic |
| Inv 3 | PASS | no merge conflicts observed |
| Inv 4 | **FAIL** | "unproven means safe" broken by Y1, Y2, Y3, Y7 |
| Inv 5 | PASS | decoys hold |
| Inv 6 | PASS | |
| Inv 7 | PASS | |
| Inv 8 | PASS (units) | |
| Inv 9 | PASS (units) | |
| Inv 10 | PASS | e2e dump checks |
| Inv 11 | PASS | coverage sums |

## Findings

All scratch repositories were built under `var/eval-m4b-1/` (`git init`, one commit), indexed `--no-scip` with the HEAD release binary, then deleted. Repro sources are given in full so they can be recreated.

### Y1 — blocker — RTA proves `$`-named nested classes never instantiated (ServiceLoader file, `Class.forName` literal)

Repro (`rta1`, Spring Boot parent pom, so `role = service`):
```java
// Greeter.java
public interface Greeter { String greet(); }
// Holder.java
public class Holder { public static class LoadedImpl implements Greeter { public String greet() { return "loaded"; } } }
// Holder2.java
public class Holder2 { public static class ReflectImpl implements Greeter { public String greet() { return "reflect"; } } }
// App.java (main)
for (Greeter g : ServiceLoader.load(Greeter.class)) System.out.println(use(g));
Object o = Class.forName("com.acme.Holder2$ReflectImpl").getDeclaredConstructor().newInstance();
static String use(Greeter g) { return g.greet(); }
```
plus `src/main/resources/META-INF/services/com.acme.Greeter` containing `com.acme.Holder$LoadedImpl`.

Observed: `rta: 4 of 4 implementing types never instantiated`; `IMPLEMENTS Holder#LoadedImpl#greet() -> Greeter#greet() {"impl_instantiated": false}` and the same for `ReflectImpl`. `rb callers LoadedImpl.greet` (default `rta`): `(no callers found)`, `1 dispatch step pruned`; `--precision cha`: `App#use()` via dispatch. Compiled and run with the host JDK 21: prints `loaded` and `reflect`, so both types are instantiated and `use()` calls both `greet()`s.

Expected: kept (`string`). Both forms are in the claim's own mechanism: a string literal (mention scan) and an unparsed non-binary file (raw-text scan), with the class's literal binary name, not a computed one.

Cause: both word splitters treat `$` as a word character, so `Holder$LoadedImpl` is one word and `LoadedImpl` never occurs: `crates/rb-syntax/src/mentions.rs:68` (`words`, string fragments) and `crates/rb-frameworks/src/clients.rs:480` (`text_words`, raw scan; also gate M). The orphan scan (`liveness.rs:62`) splits on `$` and is unaffected. The same hole covers Spring XML `<bean class="a.Outer$Impl">`, `spring.factories`/`*.imports` entries and Jackson `@JsonTypeInfo`-free class-name strings for nested classes.

Regression test: `rta::tests` rows with `"com.x.Outer$T"` in a string and in a raw-scan file, each keeping `T` (`string`); a `mentions` row asserting `Outer$T` yields the words `Outer` and `T` as `String`.

### Y2 — blocker — RTA proves a base class never instantiated when its only subclass is not a definition (Java local class in a lambda, TS class expression)

Repro (Java, same `rta1` repo):
```java
public class LambdaBase implements Greeter { public String greet() { return "lambda-local"; } }
// in App.main:
Supplier<Greeter> s = () -> { class Local extends LambdaBase {} return new Local(); };
System.out.println(use(s.get()));
```
Repro (TS, `rta2`, `package.json` with `scripts.start`):
```ts
// base.ts
export class ExprBase implements Greeter { greet(): string { return "expr"; } }
// main.ts
const Derived = class extends ExprBase {};
export function use(g: Greeter): string { return g.greet(); }
console.log(use(new Derived()));
```
Observed: `LambdaBase` and `ExprBase` proven (`impl_instantiated: false` on type and `greet()` edges); `rb callers LambdaBase.greet` / `ExprBase.greet` under the default `rta`: `(no callers found)`, pruned 1; under `cha`: `use()` via dispatch. The Java program prints `lambda-local` when run.

Expected: kept (`subtype`). S9 (c) allows the `supertype` role "handled by (i)", but (i) only walks supertype heads of `Definition`s (`crates/rb-indexer/src/rta.rs:132–145`), and classes inside anonymous scopes (Java `lambda_expression`, `object_creation_expression`; JS/TS `class` expressions; `crates/rb-syntax/src/queries.rs:20–26`) are not definitions. So their `extends` mention is allowed (`mentions.rs` `superclass`/`class_heritage` → `Supertype`) while their instantiation never reaches the base. Same for a local class inside an anonymous class body, and for interfaces with default methods implemented that way.

Regression test: `rta::tests` rows: a Java local class in a lambda extending `T`, a Java local class in an anonymous class body, a TS `const D = class extends T {}`; each keeps `T` (`subtype` or `mention`). Simplest sound fix: a `supertype` mention whose class node is not a definition counts as `other`.

### Y3 — blocker — orphan eligibility misses overrides of external methods through an in-repo base class

Repro (`orph1`, Spring Boot parent pom):
```java
public abstract class BaseInterceptor implements org.springframework.web.servlet.HandlerInterceptor {}
public class AuthInterceptor extends BaseInterceptor {
  public boolean preHandle(HttpServletRequest request, HttpServletResponse response, Object handler) { return tokenPresent(request); }
  private boolean tokenPresent(HttpServletRequest request) { return request != null && request.getHeader("Authorization") != null; }
}
@Configuration public class WebConfig implements WebMvcConfigurer {
  @Override public void addInterceptors(InterceptorRegistry registry) { registry.addInterceptor(new AuthInterceptor()); }
}
public class Probe { public boolean probe(AuthInterceptor interceptor) { return interceptor.preHandle(null, null, null); } }
```
Observed: `AuthInterceptor#preHandle()` and `#tokenPresent()` are `orphan_eligible: true`; `rb impact Probe.probe` lists
```
orphans: may become dead code (2):
  1  AuthInterceptor#preHandle()  ...  via Probe#probe(). (calls)
  2  AuthInterceptor#tokenPresent()  ...  via AuthInterceptor#preHandle(). (calls)
```
Expected: both kept. Spring MVC calls `preHandle` on every request for the registered interceptor (`HandlerInterceptor` is external). Rule (e)'s stated purpose is "an override of an external method is called by the library"; the implementation (`crates/rb-indexer/src/liveness.rs:375–390`) checks only the supertypes written on the definition and its enclosing types, never the supertypes of those supertypes. The facts hold the whole chain (`AuthInterceptor extends BaseInterceptor`, `BaseInterceptor implements HandlerInterceptor`). The contract's literal wording ("its class declares no supertype outside the repository") is met, its reason is not; same for an in-repo interface extending an external one (`interface AppListener extends ApplicationListener<E>`), and for React/Angular bases in TS.

Regression test: `liveness::tests` keeping row: a method of a class whose in-repo superclass (depth 1 and 2) implements an external interface is `supertype`.

### Y4 — major — gate M ignores `@Import`-registered beans that are not candidates: wrong `plain`/`unique` `INJECTS`

Repro (`di1`):
```java
@SpringBootApplication @Import(SmsNotifier.class) public class App { ... }          // com.acme
public interface Notifier { String send(String to); }                               // com.acme
@Component public class EmailNotifier implements Notifier { ... }                   // com.acme
@Primary public class SmsNotifier implements com.acme.Notifier { ... }              // com.other (outside the scan root, no stereotype)
@Service public class Alerts { public Alerts(Notifier notifier) { ... } }
```
Observed: `INJECTS Alerts# -> EmailNotifier#` `{"origin":"stereotype","resolution":"unique"}`, no `capped_by`, bucket `plain`. Real Spring 6.1.13 (`AnnotationConfigApplicationContext(App.class)`, jars from the `.m2` cache) injects `SmsNotifier` (`Alerts -> sms ops`).

Expected: at least `capped_by: [mention]`, and not a confident single edge to the wrong bean. S6 names `@Import(X.class)` as gate M's target form, but gate M is evaluated over the injected type and the candidates that survived scan-root filtering only (`crates/rb-indexer/src/di.rs:571–577`, with `eligible` at `:500–520`); an assignable class excluded by the roots, or never a bean by stereotype, is not checked. Under the pre-F2 contract this was the `synthetic` row (SCIP mode gives `plain` too).

Regression test: `di::tests` row: assignable class outside the roots named by a `class_literal` → `capped_by` contains `mention`; plus the S6 guard "no capped point is plain".

### Y5 — major — generic type arguments ignored, then `@Primary` picks a bean of the wrong parameterisation

Repro (`di1`): `interface Store<T>`; `@Component class StringStore implements Store<String>`; `@Component @Primary class IntStore implements Store<Integer>`; `@Service class Catalog { Catalog(Store<String> store) }`.
Observed: `INJECTS Catalog# -> IntStore#` `{"resolution":"primary","primary":true,"capped_by":["generic"]}`. Real Spring: `StringStore` (`Catalog -> s`), since generics are part of candidate matching (`GenericTypeAwareAutowireCandidateResolver`), before `@Primary`.
Expected: no single edge; both candidates `ambiguous` (or the generic match modelled). D9 says generic matching is not modelled and caps; capping the certainty while asserting `resolution: primary` to a bean Spring never injects is a confident-looking wrong edge (the brief's "major" class). `crates/rb-indexer/src/di.rs:624–634`.
Regression test: the repro as a `di::tests` row: `resolution` is `ambiguous` with two edges, or the generic filter applied.

### Y6 — major — JSR-330 `@Named` qualifiers ignored, then `@Primary` picks the other bean

Repro (`di1`): `@Component @Named("fast") class FastCodec implements Codec`; `@Component @Primary class SlowCodec implements Codec`; `@Service class Encoder { Encoder(@Named("fast") Codec codec) }`.
Observed: `INJECTS Encoder# -> SlowCodec#` `{"resolution":"primary","capped_by":["annotation_unread"]}`. Real Spring: `FastCodec` (`Encoder -> fast`).
Expected: `@Named` read as a qualifier (point) and bean name (class), or no single edge while an unread point annotation is present. Custom `@Qualifier`-meta-annotated annotations have the same shape. `crates/rb-frameworks/src/di.rs:770–806` (point annotations only recorded), `crates/rb-indexer/src/di.rs:493–495, 624–634`.
Regression test: the repro as a row; a point with an unread annotation must not narrow by `@Primary`.

### Y7 — major — RTA proves a TS class exported from a module that is iterated through a namespace import

Repro (`rta3`, deployable npm service):
```ts
// commands/index.ts
export class DeployCommand implements Command { run(): string { return "deploy"; } }
// main.ts
import * as commands from "./commands/index";
export function runAll(): string[] { return Object.values(commands).map((C) => execute(new (C as any)())); }
function execute(c: Command): string { return c.run(); }
```
Observed: `rta: 1 of 1 implementing types never instantiated`; `rb callers DeployCommand.run`: `(no callers found)`, 1 pruned. Amendment 9's argument ("every importer mentions the name") fails for `import * as ns` (and `export * from`), which the facts record. Arguably a residue form (no name is computed, though); recorded as major because the namespace import of `T`'s module is a fact-visible form. Fix: a namespace import or `export *` of the module declaring `T` keeps `T`. `crates/rb-indexer/src/rta.rs:186–200`.

### Y8 — major — a Maven artifact declared twice gets two `exact` versions

Repro (`pkg1/svc/pom.xml`): two `<dependency>` entries for `org.example:dup` with `<version>1.0</version>` and `<version>2.0</version>`.
Observed: `DEPENDS_ON svc -> maven/org.example/dup@1.0 [exact] {version_from: literal}` and `-> maven/org.example/dup@2.0 [exact]`. Maven resolves one version for an artifact (it warns that `dependencies.dependency.(groupId:artifactId:type:classifier)` must be unique and keeps one declaration), so at least one `exact` edge is false whichever wins. Amendment 7's "one edge per (service, target)" dedupes by key, which includes the version. `crates/rb-indexer/src/packages.rs:478–524`.
Regression test: duplicate artifact → one edge (the declaration Maven keeps, or `@?`/`unresolved`).

### Y9 — major — npm aliases keyed under the alias with the aliased package's version, `exact`

Repro (`pkg1/web`): `"dependencies": { "foo": "npm:bar@^1.0.0" }`, lockfile `"node_modules/foo": { "name": "bar", "version": "1.2.3" }`.
Observed: `DEPENDS_ON web -> npm/foo@1.2.3 [exact] {declared: "npm:bar@^1.0.0", version_from: lockfile}`. The installed package is `bar@1.2.3`; `foo@1.2.3` is a different (possibly non-existent) package. `lock_version` ignores the entry's `name` (`crates/rb-indexer/src/packages.rs:197–223`).
Regression test: alias → key `npm/bar@1.2.3` (or the alias unresolved), never `foo@<bar's version>` at `exact`.

### Y10 — major — go.mod `replace` to another module version is ignored

Repro (`pkg1/gosvc/go.mod`): `require github.com/gin-gonic/gin v1.10.0` and `replace github.com/gin-gonic/gin => github.com/gin-gonic/gin v1.9.1`.
Observed: `DEPENDS_ON gosvc -> go/github.com/gin-gonic/gin@v1.10.0 [synthetic] {version_from: go_mod}`. `go list -m github.com/gin-gonic/gin` (offline): `github.com/gin-gonic/gin v1.10.0 => github.com/gin-gonic/gin v1.9.1`. The soundness table's claim is "the version … is the one the stated source pins"; the same go.mod pins v1.9.1 and no residue entry covers `replace`. Only local-directory replaces are read (`packages.rs:321–330`). `synthetic`, not `exact`, hence major rather than blocker.
Regression test: version replace → the replacement's key, or `@?` `unresolved`.

### Y11 — minor (suspected) — duplicate `<properties>` entries resolve to the first

`<lib.version>1.0</lib.version><lib.version>2.0</lib.version>` gives `lib@1.0 [synthetic] {version_from: property}`; Maven's model reads properties into a map (last wins, as far as I know; not verified with Maven, which is not on the host). `packages.rs:439–446` (`text_of` takes the first child).

### Y12 — minor — RTA mutant R4 survives: subtype propagation from non-considered types is untested

Mutant: `rta.rs:149` seed `.filter(|t| types.get(t).is_none_or(Option::is_some))` → `is_some_and(...)` (instantiated subtypes that are not themselves "considered" no longer propagate). All 196 rb-indexer unit tests pass. A subtype whose own `IMPLEMENTS` edge is missing (unresolved head) is the untested case. Add a row.

### Y13 — minor — DI mutant D2 survives: two `@Primary` candidates untested

Mutant: `di.rs:630` `primaries.len() == 1` → `>= 1`. Unit tests pass. With two `@Primary` beans and a third non-primary one, the mutant drops the non-primary from the ambiguous list and labels the edge `resolution: primary`. Add a row.

### Y14 — minor (suspected, code read) — RTA reads no mentions from parsed Python/Go files

`mentions` returns nothing for Python and Go (`mentions.rs:21–23`) and parsed files are excluded from the raw scan (`syntax.rs:92–95`), so a Java/TS class name in a `.py`/`.go` file is invisible to S9 (c) and (d); the contract's (d) says "no unparsed non-binary file", which is literal-compatible but leaves this gap undocumented. `.txt` files are also inert for (d) (`clients.rs:558–566`), so a plugin list in a `.txt` read at run time is not seen. Not reproduced end to end.

### Y15 — doc — open items and "weaker false results" are incomplete

None of Y1–Y10 is in open items 1–15 or in the "weaker false results still possible" list. In particular the list says the remaining false `heuristic` `INJECTS` forms are external-type name clashes, auto-configuration and the Lombok qualifier; Y4–Y6 are three more, all labelled `unique`/`primary`. Open item 3 records only the Spring Data fragment orphan; Y3 is a broader orphan form. S9 (c)'s phrase "supertype (handled by i)" presents a proof step that Y2 shows does not hold.

## Mutations (own, 23 run; harness: copy aside, replace exactly once, fresh mtime, `cargo test`, restore, `cmp` bytes; `git status --short` clean after each batch)

| Id | Mutant | Result |
|---|---|---|
| R1 | RTA (d) raw-text word check off | killed (`rta::tests::one_keeping_row_per_rule`) |
| R2 | RTA static-import (enum) check off | killed |
| R3 | RTA member annotations ignored | killed |
| R4 | RTA (i) seeds only considered kept types | **survived** (Y12) |
| R5 | RTA (g) `includeFilters` check off | killed |
| R6 | RTA (f) default-export rule off | killed |
| R7 | RTA (i) BFS does not enqueue | killed (`instantiated_subtypes_keep_their_supertypes`) |
| O1 | orphan accessor convention off | killed |
| O2 | orphan rule (e) off | killed |
| O3 | orphan evidence span ignores file | killed |
| O4 | orphan `crowded` check off | killed |
| O5 | impact: no seed from implementing an outside method | killed (`orphans_are_the_candidates_no_live_node_reaches`) |
| O6 | impact: liveness not propagated by dispatch | killed (2 tests) |
| D1 | DI conditional only on chosen (not narrowed-away) | killed |
| D2 | DI `@Primary` with ≥ 1 primaries | **survived** (Y13) |
| D3 | DI point annotations never cap | killed |
| D4 | `decapitalize` two-capitals rule off | killed |
| D5 | Lombok required-args takes every uninitialised field | killed |
| P1 | npm lockfile walk without parent directories | killed |
| P2 | go.mod `// indirect` not skipped | killed |
| E1 | `rb impact` default precision `rta` | killed (4 e2e tests) |
| E2 | `rb table-deps` default precision `rta` | killed (2 e2e tests) |
| E3 | card counts a manifest edge to its own service | survives e2e, killed by `card::tests::dependencies_tables_and_call_graph` |

## Precision samples (HEAD, `--no-scip`, M3b sampling rule, each read in the source)

- **`INJECTS`:** jhipster 19/19 correct (incl. `@Qualifier("taskExecutor")` → `AsyncConfiguration#getAsyncExecutor()` `@Bean(name = "taskExecutor")`; Spring Data repos; `@Bean` `PasswordEncoder`/`JwtEncoder`); realworld-spring 20/20 (Lombok `@AllArgsConstructor` fields → `@Service`/`@Repository` classes); petclinic-microservices 15/15. **54/54** on real repos; 0 `synthetic`. Constructed: 3/3 wrong (Y4–Y6), confirmed against a real Spring 6.1.13 context.
- **RTA:** 1 proven type on the seven public repos (jhipster `SearchWithPagination`, unused interface): 1/1 correct. Constructed: 6 types proven that are instantiated (Y1 ×2, Y2 ×2, Y7 ×1; 3 confirmed by running the Java program).
- **Orphans:** `rb impact` on 10 sampled callers of eligible symbols per repo: piggymetrics 6, node-express-realworld 6, jhipster 10 (new), realworld-spring 0, petclinic-rest 0. Every listed orphan read with `git grep -w`: **22/22** have no other use. Constructed: 2/2 false (Y3).
- **Packages:** 20 sampled `manifest` edges each on petclinic, petclinic-microservices, node-express-realworld (lockfile), and jhipster (9 maven sampled from the npm-less list; 1 property): **69/69** re-derived from pom/lockfile. Constructed: Y8, Y9, Y10 wrong.
- **Q3:** jhipster `bank_account`: data access, `GET /api/bank-accounts` and `GET /api/bank-accounts/{}` read against `BankAccountResource`; correct, write endpoints missed through inherited `save`/`delete` (open item 8).

## Open items 1–15

Complete for what they cover and honest about RTA's zero yield and the public-repo misses. Incomplete as a gap list: Y1–Y10 are missing (Y15). Item 6 ("RTA proves nothing on real repositories") understates the risk now that Y1/Y2/Y7 show the proof itself is unsound for scan-visible forms, not just unproductive.

## Closing readiness

Not ready. Per the contract's closing bar, pass 1 findings are fixed test-first as amendments. Y1, Y2 (pruned-but-instantiated from forms in the mention scan / raw scan / facts that an allow-listed role or rule let through) and Y3 (false orphan through a rule's gap) are of closing-bar class (c) and would block pass 2 if left. Y4–Y10 are confident-looking wrong `INJECTS` and wrong versions. Y12/Y13 are test gaps from surviving mutants.

## Cleanup

`var/eval-m4b-1/` removed; all `ev4b1_*` repositories and their AGE graphs dropped (one at a time). `rb.scip_index` row count is unchanged (32 before and after): the one SCIP run re-wrote the existing petclinic cache row for the same input hash, so no row of mine remains. No commits; `git status --short` shows only this file.

VERDICT: FAIL
