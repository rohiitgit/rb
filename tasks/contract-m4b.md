# M4b contract: dependency injection, dispatch precision, service dependencies, Q3 roll-up and orphans (draft, planner, 2026-10-06)

Written before any code. M4b is done only when every assertion below is backed by a passing test or a recorded measurement. Nothing here is built yet. "Verified facts" are only what was read in the code, the goldens or the design at commit `415add0` (M4a closed, `PIPELINE` `m4a-http-11`).

This contract follows the M4a format. It also applies the lesson M4a and M3b paid for over 20 evaluator passes (lessons 2026-10-06, "state the soundness bar before adversarial review starts", "a deny-list … is never complete", "gate what carries the effect"). Every claim about a new output is stated narrowly, up front, together with:
- the allow-listed conditions under which it holds;
- the input forms declared residue;
- what a re-check must find to block closing.

## Soundness claims and closing bar (read first)

M4b adds five kinds of output whose failure is silent:
- an injection edge to the wrong bean;
- a dispatch target pruned although the program can reach it;
- a symbol reported as dead code although something uses it;
- a package version that is not the installed one;
- a service dependency that does not exist.

For each, the table states the claim, the maximum certainty, the conditions, and what is residue. **Everything not proven takes the safe direction:** an injection is capped below `synthetic`, a type counts as instantiated, a symbol counts as alive, a version is unresolved. A gate can only lower certainty or keep something. No gate ever adds an edge.

| Output | Claim (exactly this, no more) | Max | Allow-listed conditions (all must hold for the max) | Residue (out of scope by construction) |
|---|---|---|---|---|
| `INJECTS` (provenance `di`) | No false `synthetic` `INJECTS` from any form visible to the facts (S1), the repository-wide mention scan (S1) and the raw-text scan of unparsed files (M4a amendment 17's file selection). | `synthetic` (never `exact`, D1) | S5's synthetic row:<br>• one candidate after narrowing by `@Qualifier` literal or a single `@Primary`;<br>• injection point form, consumer and candidates allow-listed (S3, S4);<br>• no conditional class or `@Bean` method among the consumer and all candidates (`@Profile`, `@Conditional*`);<br>• main sources only;<br>• literal scan roots;<br>• the DI surface is plain (S6 gate D);<br>• no candidate or injected type is mentioned in a non-allow-listed role (S6 gate M);<br>• the injected type and the assignability path are resolved at `synthetic` or better (D2: in `--no-scip` mode every `INJECTS` is `heuristic` at most). | Beans registered by dependencies' auto-configuration or by `spring.factories` outside the repository; bean definitions built by reflection or from computed strings; bytecode instrumentation; configuration outside the repository (active profiles, `spring.main.*` set at deploy time); annotation processors that generate bean classes not committed (MapStruct, Lombok-generated constructors beyond S3's rules); bean post-processors in dependencies that replace a bean with an object of another class. |
| `impl_instantiated: false` on `IMPLEMENTS` (RTA, D4) | A dispatch step is pruned under `precision=rta` only when its implementing type, and every type that names it as a supertype, is proven never instantiated by S9's rules. Every pruned step is counted in the answer (S10), so pruning is never silent. | n/a (an attribute; no edge or certainty changes) | S9 (a)–(i):<br>• Java or TS/JS class;<br>• in a deployable service;<br>• not in a test source;<br>• every mention of its name in an allow-listed role (declaration, import, supertype head);<br>• no string or raw-text mention;<br>• no annotation outside the inert list on it or its members;<br>• not default-exported;<br>• not a Spring Data fragment implementation;<br>• no repository-wide unnamed-instantiation surface. | Instantiation by data (Java deserialisation of class names in a stream, Jackson `@JsonTypeInfo(use = CLASS)` payloads); reflection with computed names; classes instantiated by dependencies through classpath scanning of a package the repository never names; bytecode generation. `precision=cha` (the default for Q2 and Q3) never prunes. |
| `orphan` (Q2, `rb impact`) | A symbol listed as an orphan has no use visible to the whole-word mention scan of every non-binary file, the inert-annotation list, the naming-convention list and the entry-point rules (S12), and all its requiring inbound edges come from the removed set or other orphans (S13). | n/a (a query-time list labelled "may become dead code") | S12's eligibility:<br>• every whole-word mention of its name in the repository's text is its own declaration or lies in the evidence span of an edge into it;<br>• no annotation or decorator outside the inert list;<br>• not an entry point;<br>• not a convention-invoked name;<br>• not a Go method;<br>• its class has no supertype outside the repository;<br>• in a deployable service;<br>• not in a test source. | Names built at run time (`getattr(o, "get_" + x)`, `Method.invoke` with a computed name, `obj[name]()`); framework callbacks found by a convention not on S12's list; invocation from outside the repository (another repository calling a deployable service's code in process). |
| `DEPENDS_ON` Service → Package (provenance `manifest`) | The version on a `manifest` edge is the one the stated source pins: a lockfile entry, or a literal in the manifest. | `exact` (lockfile, pom literal); `synthetic` (go.mod `require`, same-pom property); a version that cannot be read is keyed `@?` and states `version_from: unresolved` | S15. | A lockfile that disagrees with what was actually installed (an install ignoring the lock); Maven version mediation for transitive dependencies (only direct dependencies are read); registry substitution (a mirror serving other bytes). |
| `DEPENDS_ON` Service → Service, `via=package` (provenance `manifest`) | The dependent service's manifest declares a dependency on the target service's package, and that name resolves uniquely to it. | `synthetic` | S16. | Registry packages that shadow a workspace package of the same name when the range excludes the workspace version (capped at `heuristic`, not residue). |
| Service dependencies `via=calls` / `via=http` (query time, D6) | Each listed dependency is backed by at least one stored edge (`CALLS`/`REFERENCES` across services, or `CONSUMES`). Its certainty is the best of those edges, and nothing is inferred beyond them. | that of the underlying edge | S17. | As the underlying edges (M3b, M4a). |

**What M4b must not weaken.** M4a's closing bar for `CONSUMES` stands: no false `synthetic` from any form a repository-wide name-token scan can see. The three M4a items M4b touches can only lower or remove edges, never raise them:
- S19, Express default exports: endpoint keys and paths, at the existing path-certainty rules;
- S20, the catch-all rule: fewer edges;
- S21, DI-resolved client bases: always `base_from_bean`, never `synthetic`.

M3b's exact tiers are unchanged: invariant 1.

**Closing bar.** The milestone closes on these terms, decided now (lesson 2026-10-06):
1. **Evaluator pass 1 is a full review.** Each finding is fixed test-first as an amendment.
2. **Evaluator pass 2 is verification-only.** It re-runs:
   - every pass-1 finding's repro and its controls;
   - the regression suite and the mutations;
   - the public-repository measurements (fresh == cached).
3. **A pass-2 result blocks closing only if it is one of:**
   - (a) a pass-1 fix that does not hold;
   - (b) a regression;
   - (c) a false `synthetic` `INJECTS`, a pruned-but-instantiated type, or a false orphan, from a form the claim's own mechanism can see. That means a form in the facts, the mention scan, or the raw-text scan, which an allow-listed role, list or rule let through;
   - (d) a non-deterministic dump;
   - (e) a failed test, guard or verification step.
4. **Not blocking.** A new attack form in a residue class above, or a form needing a new fact kind, goes to M5's adversarial review. M5 already holds the "adversarial carrier review" (M4a open item 22); M4b extends it to DI, RTA and orphans with the forms found. It is recorded as an open item with its example and does not block closing.
5. **Precision misses are not soundness failures.** A lower-than-possible certainty or a missed edge is measured and recorded (S27). It is not a blocker unless it contradicts a stated rule.

## Scope

**In (M4b):**
- **Spring DI synthesis (design §6.4).** It covers:
  - `Bean` and `InjectionPoint` facts: stereotypes, `@Bean` methods, Spring Data repositories, Feign clients;
  - constructor, field, setter and `@Bean`-parameter injection, including Lombok-generated constructors;
  - `@Qualifier`, `@Primary` and parameter-name fallback;
  - `@Profile` and `@Conditional*` as conditional;
  - collection and provider wrappers;
  - `INJECTS` edges, DI coverage, and the `rb status` `di:` line.
- **Spring Data in DI.** A repository interface is a bean whose type is the interface (the proxy), and its fragment implementations are instantiated. There is no synthetic `Symbol` and no edge to the framework base (D3). The `QUERIES` edges M2b built are unchanged.
- **RTA (design §6.2):**
  - proof-of-absence instantiation analysis at index time, `impl_instantiated: false` on `IMPLEMENTS`;
  - `--precision rta|cha` on `rb callers`, `rb path`, `rb impact` and `rb table-deps`, defaulting to `rta` for Q1/Q4 and `cha` for Q2/Q3 (D5);
  - pruned steps counted in every answer.
- **Entry points (design §9.2).** Kinds:
  - `endpoint` and `top-level` (query time);
  - `main`, `test`, `scheduled`, `listener` and `lifecycle` (index time, Symbol attribute `entry`).

  `rb callers` marks them. `rb impact` lists impacted tests (design §9.3's Tests roll-up).
- **Orphan detection (design §9.3).** It is carried from M3b's scope, defined as a reachability computation with a stated eligibility (S12, S13).
- **Q3's roll-up (design §9.4 steps 2–5).** It covers callers up to entry points, the per-service roll-up (ops, direct/indirect, endpoints reaching the table), and services reaching the table only through another service's API, up to `--hops`.
- **Package dependencies with versions (design §5.5).** Read from:
  - `package.json` plus `package-lock.json` (v2/v3);
  - `go.mod`;
  - `pom.xml`: literal and same-pom property versions.

  They give `Service -DEPENDS_ON-> Package` with provenance `manifest`.
- **Service-to-service dependencies (design §9.5):**
  - `via=package` stored, from manifests (S16);
  - `via=calls` and `via=http` derived at query time (D6, fork F1);
  - shared tables reported as coupling.
- **The rest of the full Service Card (design §9.5).** Its sections:
  - Packages with versions;
  - Database tables;
  - Dependencies;
  - Depended on by;
  - Shares tables with;
  - Call graph summary;
  - entry points by kind.
- **M4a open items assigned to M4b, as decided below:**
  - 1: Express default-import routers;
  - 5: guard mutations that could not bite;
  - 7: endpoints listed twice in `rb impact`, and prefix-unknown endpoints shown without their router;
  - 9: catch-all absorption;
  - 15: Spring bases from other files via DI by type, capped;
  - 24: X1's HTML unit row, the first code change.

**Out of M4b, with the reason:**
- **M4a open item 2, gin groups passed as parameters → M6.** This is interprocedural route composition:
  - it needs Go call-site resolution feeding route facts;
  - it needs a new recursion (function → call sites → argument chains), with its own cycle, depth and growth tests;
  - no M4b exit criterion needs it.

  The affected endpoints are flagged `prefix_unknown` and keyed apart (M4a F3), so nothing is false. M6 does it together with the other Go breadth work (chi).
- **Guice, Dagger, NestJS, Angular and .NET DI → M6.** The design orders Spring first (§6.4). Each is a pack plus fixtures.
- **Spring XML bean definitions, the Kotlin beans DSL, `spring.factories` in the repository → not read.** Their presence taints the DI surface (S6 gate D), so no `synthetic` edge survives next to them. Reading them is M6.
- **Active-profile evaluation (`spring.profiles.active`, `application-<p>.*`, YAML) → M6, fork F6.** It goes with M4a open item 3. Conditional beans are never `synthetic` and stay ambiguous across profiles.
- **Receiver-aware DI narrowing of dispatch → not planned.** This would narrow a call on an injected field to the injected bean's overrider. `CALLS` edges carry no receiver binding, and design §6.4 says DI facts never rewrite `CALLS`. RTA is the exit criterion.
- **RTA for Python and Go → never pruned.**
  - Python instantiates by plain calls, `getattr` and metaclasses.
  - Go satisfies interfaces structurally, with no `IMPLEMENTS` facts in Tier B and no scip-go yet.
- **`pnpm-lock.yaml`, `yarn.lock`, Gradle (lockfiles and version catalogs), `poetry.lock`/`uv.lock`, `Cargo.lock` → M6.** M4b has no YAML parser, the same reason as M4a, and no fixture uses these. A service whose lockfile is one of these gets a `lockfile_unread` diagnostic, and its versions come from the manifest literal or are unresolved.
- **Transitive packages → not nodes.** Only direct dependencies get `Package` edges; no question asks for transitive ones.
- **External `Symbol` stubs, an edge to Spring Data's `SimpleJpaRepository` → not built.** This is M3b amendment 11's reason: there are no external symbols (D3).
- **`rb diff` of service dependencies, the REST API → M5.**
- **Mermaid/DOT output → M6,** as in M4a.

## Engineer declaration

**Canonical state.** This is cached extractor output in `FileFacts`, per blob. There is no new table and no migration.
- **`Binding.parameter`** (new): the 0-based position of a parameter in its function, method or constructor, for Java, TS/JS, Go and Python; `None` for fields and locals. Today a parameter can only be told apart by its span (verified fact 9).
- **`Binding.type_args`** (new): the type arguments of the declared type as simple names, one level deep (`List<Notifier>` gives `["Notifier"]`), for Java and TypeScript. Today they are stripped (verified fact 9).
- **`Definition.return_type`** (new): a Java method's declared return type as written, with its type arguments, and a TypeScript method's or function's annotated return type. No return-type fact exists today (verified fact 8).
- **`Definition.modifiers`** (new), a sorted list:
  - Java: `public`, `protected`, `private`, `static`, `abstract`, `final`, `default`;
  - TS/JS: `export`, `export_default`, `abstract`, `static`.
- **`FileFacts.default_export`** (new, TS/JS): the local name a module's `export default <identifier>` names, or the class/function `export default class X`/`function f` declares.
- **`FileFacts.mentions`** (new, Java and TS/JS):
  - Shape: for every identifier, type identifier and property-name token, its text mapped to the set of roles it occurs in, aggregated per file (no spans).
  - Roles:
    - `declaration`: the name of a class, interface, enum, record, constructor, method or function being declared;
    - `import`: an import or export specifier, or a package clause;
    - `supertype`: the head of an `extends`/`implements` item;
    - `type`: any other type position, type arguments included;
    - `new`: the type of an object creation or anonymous class;
    - `class_literal` (`X.class`);
    - `method_ref` (`X::m`, `X::new`);
    - `member`: the qualifier of a member access or static call, `X.y`;
    - `annotation`: the name of an annotation or decorator;
    - `logger`: a class literal as the only argument of `getLogger(…)`;
    - `string`: a whole word inside a string literal;
    - `other`.
  - Fingerprinted. M4a's `name_tokens` are unchanged.
- The extractor fingerprint covers every new source file (M4a S2's unit).

**Derived** in `rb-frameworks` as pure functions of facts, and in the indexer per snapshot:
- **Spring facts per file** (`rb-frameworks`, new module `di`):
  - beans: stereotype classes, `@Bean` methods, Spring Data repositories, Feign interfaces;
  - for each bean: its name, qualifiers, `@Primary`, conditionals, and annotations outside the inert list;
  - injection points;
  - scan roots;
  - DI-surface findings (S6 gate D).
- **DI resolution** (indexer, new module):
  - classpath per service, from `via=package`;
  - candidates per injection point, narrowing, gates;
  - `INJECTS` edges;
  - `di_*` diagnostics and one `di_coverage` per snapshot.
- **RTA** (indexer, new module): per Java and TS/JS type, a proof of non-instantiation; the `impl_instantiated: false` attribute on `IMPLEMENTS` edges out of a proven type and its methods; one `rta_coverage` per snapshot.
- **Entry kinds and orphan eligibility** (indexer): `Symbol` attributes `entry` (sorted kinds) and `orphan_eligible: true`, from facts plus a whole-word mention scan of every non-binary file. The scan is not cached, because it depends on the whole repository.
- **Manifest dependencies** (indexer, new module):
  - `Package` nodes with the M3b key form, `@?` for unresolved;
  - `Service -DEPENDS_ON-> Package` and `Service -DEPENDS_ON-> Service` (`via=package`), provenance `manifest`, with evidence at the dependency entry;
  - `lockfile_*` diagnostics.
- **Query-time only, never stored:**
  - precision filtering and pruned counts;
  - `endpoint`/`top-level` entry marks;
  - orphans and the tests roll-up;
  - Q3's roll-up;
  - `via=calls`/`via=http` service dependencies and shared tables;
  - every new card section;
  - the `di:` and `rta:` status lines, read from the coverage diagnostics.

**Invariants:**
1. **The M3b and M4a code edges are unchanged.** M4b adds no `CALLS`, `REFERENCES` or `IMPLEMENTS` edge, removes none, and changes no certainty, provenance or symbol key. The only change to an existing edge kind is the attribute `impl_instantiated: false` on `IMPLEMENTS` edges. It is present only when false; absent means "not proven uninstantiated". `CONSUMES` changes only by S19–S21, each of which can only lower or remove. This is checked against an `m4a-http-11` dump of every fixture, with `CONSUMES` rows diffed and classified.
2. **`INJECTS` is never `exact`.** Every `INJECTS` edge:
   - has provenance `di` and certainty `synthetic`, `heuristic` or `ambiguous` (D1);
   - goes from a `Symbol` (the class owning a constructor, field or setter injection point, or the `@Bean` method owning a parameter) to a `Symbol` (a stereotype class, a `@Bean` method, or a Spring Data or Feign interface; D2).

   Several candidates give one `ambiguous` edge to each, with `candidates = n`. A collection injection gives an edge to each candidate, with `collection: true`, and is not ambiguous. Zero candidates, or more than 16, give a counted bucket and no edge.
3. **Certainty is never guessed upward.** An `INJECTS` edge's certainty is the minimum of:
   - the injected type's resolution;
   - the assignability path (the `IMPLEMENTS` edges or supertype resolutions it crosses);
   - S5's DI rule.

   Several sites for one `(src, INJECTS, dst, di)` edge merge into one edge with the best site's certainty and attributes, plus `sites = n`. This is M3a's rule; `GraphBuilder::add_edge` rejects differing certainty or attributes (verified fact 2).
4. **Unproven means safe.** RTA prunes only by S9's proof. Orphan eligibility needs S12's proof. A version is exact only from a lockfile or literal. Every gate that fails lowers certainty, keeps a type instantiated, or keeps a symbol alive, and none adds an edge. Every pruned dispatch step is counted in the answer that pruned it (`pruned_by_rta`), so `rta` is never a silent under-report.
5. **Syntax and imports, not text.** A Spring annotation is recognised only through an import of its package, or its qualified name (`rb_frameworks::routes::is_annotation`, verified fact 11). A same-named annotation from another package, a commented-out annotation, or a name in a string, is not Spring (lesson 2026-09-29). Raw-text scans only ever taint, keep alive or keep instantiated.
6. **Main sources only for DI.** A class under a test source path is not a bean candidate or consumer for main-source injection points. Test paths are Maven `src/test/`, `test/`/`tests/`/`__tests__/` directories, `*Test.java`/`*Tests.java`/`*IT.java`, `*.test.*`/`*.spec.*`, `*_test.go`, and `test_*.py`/`*_test.py`. The injection points of test classes are counted (bucket `test`) and not resolved.
7. **Determinism.** The same commit and tools give byte-identical dumps: fresh vs warm facts cache, two repos, a forced rebuild, SCIP fresh vs cached. Every output list has a total order: distance, then certainty rank, then natural key, compared bytewise (§3.3).
8. **Termination by construction.** Each walk has a visited set and a stated cap:
   - the supertype walk for assignability and for RTA's subtype propagation (depth 64, as M3a);
   - classpath closure;
   - the orphan liveness BFS;
   - Q3's caller BFS (depth cap 25, node cap 200,000) and its API hops (`--hops`, default 4, maximum 8);
   - the Express default-export lookup (one level; no chains).

   Every recursion edge has a cycle test (`A extends B`, `B extends A`; a `via=package` cycle; mutual recursion in orphans) and a depth test (lessons 2026-09-30).
9. **Stated complexity is tested by counted operations.** The test is at most 5× the operations for 4× the input (M4a amendment 12). It covers DI resolution, RTA, the mention scan, orphans, Q3's roll-up and the card's dependency aggregation. Each counted loop scans through `work::Counted` or ticks (M4a amendment 13). One pathological input keeps a wall-clock bound.
10. **Evidence.** Each new edge has at least one evidence row at a real `File`:
    - `INJECTS`: the injection point (the parameter, field or setter declaration);
    - `manifest` `DEPENDS_ON`: the dependency entry in the manifest or lockfile.
11. **Coverage sums.** Every injection point falls in exactly one DI bucket, and every Java/TS type with an outbound `IMPLEMENTS` falls in exactly one RTA bucket. A unit asserts each sum.

## Decisions (made here, with reasons)

- **D1: `INJECTS` tops out at `synthetic`.** Which bean Spring injects depends on:
  - the active profiles;
  - auto-configuration from the classpath;
  - properties;
  - post-processors.

  Static analysis sees none of these completely. `exact` stays reserved for what a compiler confirms (M4a D2's reasoning).
- **D2: edge ends, and certainty bound by resolution.**
  - **Source:** the symbol that owns the injection point. That is the class for constructor, field and setter injection, and the `@Bean` method for its parameters.
  - **Target:** the bean's definition:
    - the stereotype class's type symbol;
    - the `@Bean` method;
    - the repository or Feign interface's type symbol.

    Pointing at the `@Bean` method, not its declared return type, makes Q2 say "removing this factory breaks these consumers".
  - **Certainty:** at most the certainty of resolving the injected type and the assignability path. So in `--no-scip` mode every `INJECTS` is `heuristic` or `ambiguous` (Tier B resolution is `heuristic`, M3a), and `synthetic` needs SCIP (fork F2).
- **D3: Spring Data's "synthetic implementation" is a bean, not a node.**
  - A Spring Data repository interface becomes a bean of the interface's own type (`origin: spring_data`). Its derived methods already have `QUERIES` edges to the entity's table (M2b, verified fact 5).
  - Its fragment implementations count as instantiated, because Spring Data finds them by the `Impl` naming convention. A fragment implementation is a class implementing, by name, an interface that a repository interface extends.
  - There is no synthetic `Symbol` and no edge to `SimpleJpaRepository`, since external symbols are not nodes (M3b amendment 11). A synthetic node would also be a symbol that no code defines.
  - The orchestrator's brief asked for "synthetic implementation edges to the framework base"; that is fork F5.
  - Store-specific repositories count as Spring Data beans. That means any supertype imported from `org.springframework.data.*` whose simple name ends in `Repository` (`MongoRepository`, `ReactiveCrudRepository`). M2b's `QUERIES` stay JPA-only, since Mongo collections are not tables.
- **D4: RTA is a proof of absence, stored where the design says.**
  - A type is pruned only when S9 proves it is never instantiated. That proof is an allow-list of the roles its name may take. It is not the design's list of instantiation signals, which would be a deny-list and would prune types instantiated by XML, `META-INF/services`, convention or reflection (lesson 2026-10-06).
  - The flag is the design's `IMPLEMENTS` attribute `impl_instantiated`. It is written only as `false`, which keeps dump churn to the proven types.
  - It is computed before `IMPLEMENTS` edges are added, so `add_edge`'s rejection of differing attributes cannot fire. Equivalently, the attribute can be set by one explicit builder call that the generator adds with a unit; the generator's choice, recorded.
- **D5: precision defaults.**

  | Command | Default | Reason |
  |---|---|---|
  | `rb callers` (Q1), `rb path` (Q4) | `rta` | design §6.2, §14 |
  | `rb impact` (Q2) | `cha` | under-reporting is the dangerous failure, design §9.3 |
  | `rb table-deps` (Q3) | `cha` | a dependency and ownership question, where under-reporting is dangerous as in Q2; the design is silent |

  Every command takes `--precision rta|cha`.
- **D6: service-to-service dependencies `via=calls` and `via=http` are derived at query time; `via=package` is stored (fork F1).**
  - **Why calls/http are not stored:**
    - they are pure functions of stored edges (`CALLS`/`REFERENCES` whose ends lie in different services, and `CONSUMES` into another service's endpoint);
    - storing them would add a second source of truth;
    - it would need a new provenance, since the edge key `(src, kind, dst, provenance)` (verified fact 3) cannot hold `via=calls` and `via=http` between the same two services under one provenance;
    - it would duplicate every crossing edge's evidence.

    `rb cypher` users get them with a two-hop pattern; the design's own Cypher example works at the symbol level (§8.4).
  - **Why `via=package` is stored:** it comes from manifest text, which is not in the graph. It is stored with provenance `manifest` and evidence at the dependency entry.
  - Design §8.5's `DEPENDS_ON` row is annotated.
- **D7: package versions.**
  - **What is read:** only direct dependencies:
    - `package.json` `dependencies`/`devDependencies`/`peerDependencies`/`optionalDependencies`, with the version read from `package-lock.json` (v2/v3 `packages`) by Node's resolution walk from the service directory up to the lockfile's root;
    - `go.mod` `require` lines without `// indirect`;
    - `pom.xml` `<dependency>` with a literal `<version>`, or a `${prop}` defined in the same pom's `<properties>`.
  - **Keys:** M3b's key form `<manager>/<name>@<version>`, with `@?` when unresolved (verified fact 6). M3b's existing `Package` nodes are unchanged. A `manifest` edge and a `scip` edge to the same key are two edges, which the edge key allows.
  - **Certainty:**

    | Version source | Certainty |
    |---|---|
    | Lockfile entry or pom literal | `exact` |
    | Same-pom property (a profile or `-D` can override it) | `synthetic` |
    | `go.mod` (minimal version selection can pick a higher version when the file is untidy) | `synthetic` |
    | Unresolved: parent- or BOM-managed Maven versions, `package.json` without a lockfile | `exact` edge to the `@?` key (the dependency itself is declared) |

  - **Attributes:**
    - `declared`: the range or text as written;
    - `scope`: `runtime`, `dev`, `peer`, `optional`, `test` or `provided`;
    - `version_from`: `lockfile`, `literal`, `property`, `go_mod` or `unresolved`.
  - Go keys use manager `go` (`go/github.com/gin-gonic/gin@v1.10.0`). When scip-go arrives (M6), its manager name must be aligned with this, recorded for M6.
- **D8: DI scope is the main sources of one service's classpath.**
  - **Classpath:** classpath(S) is S plus the services S reaches by stored `via=package` edges, transitively, with a visited set.
  - **Candidates:** beans in classpath(S). A bean assignable to the injected type in a library service *outside* classpath(S) caps the point at `heuristic` (gate `classpath_unknown`), since an unread dependency may put it on the classpath. Deployable services are never on another's classpath.
  - **Scan roots:** the packages of S's main-source `@SpringBootApplication`/`@ComponentScan` classes and their literal `scanBasePackages`/`basePackages`/`value`/`basePackageClasses` attributes. A stereotype class outside every root of S is not a candidate. If S has no such class, or more than one, every stereotype class in classpath(S) is a candidate and the point is capped (gate `scan_roots_unknown`).
- **D9: Spring resolution rules, as modelled.** Each rule below is recalled; the generator confirms it from the reference docs in phase 0 before coding S3/S4, as M4a did for D12.
  - **Constructor injection:**
    - one declared constructor is autowired implicitly (Spring 4.3+);
    - with several constructors, the one marked `@Autowired`;
    - otherwise none (bucket `unresolved`, detail `constructor_ambiguous`).
  - **Field and setter injection:** marked with `@Autowired`, `jakarta.inject.Inject`/`javax.inject.Inject`, or `@Resource`. `@Resource` resolves by name first: the `name` attribute, or else the field or property name. Its edges are capped at `heuristic` (gate `by_name`).
  - **Lombok:** `@RequiredArgsConstructor` (final non-static fields without an initialiser, plus `@NonNull` fields) or `@AllArgsConstructor` (all non-static fields), on a class with no declared constructor, generates one constructor whose parameters are those fields in declaration order. `onConstructor_` is accepted.
  - **Narrowing order:**
    1. the injection point's `@Qualifier("x")` against bean names and class-level or `@Bean`-level `@Qualifier` literals;
    2. exactly one `@Primary` among the remaining candidates;
    3. the parameter or field name against bean names (Spring's fallback, which needs `-parameters` for parameters). This step caps the edge at `heuristic` (gate `by_name`).
  - **Bean names:**
    - a stereotype's literal value;
    - else the class's simple name with its first letter lower-cased (both of the first two letters upper-case keeps the name: `URLParser`; Java's `Introspector.decapitalize`, confirmed in phase 0);
    - for a `@Bean` method, its `name`/`value` literals, else the method name.
  - **Wrappers:**
    - `List`/`Set`/`Collection<T>`, `T[]`, `Map<String, T>`: every candidate, `collection: true`;
    - `Optional<T>`, `ObjectProvider<T>`, `jakarta.inject.Provider<T>`: as `T`, and zero candidates is bucket `optional_absent` (not an error).

    Generic matching on type arguments other than these wrappers is not modelled. A point whose type has other type arguments caps at `heuristic` (gate `generic`).
- **D10: entry points are a positive allow-list (S11).** Marking only shapes the display. It is not what keeps code alive in orphan detection: S12's mention, annotation and convention rules do that. A missed entry kind therefore cannot produce a false orphan through the entry rule alone. The `entry` attribute is stored on `Symbol` nodes, because the facts behind it (annotations, modifiers, file names) are not in the graph.
- **D11: orphans are a reachability computation, not an iterated fixpoint (S13).**
  - **Live set:** the nodes reachable, along requiring edges and dispatch, from live seeds:
    - every node outside removed ∪ candidates;
    - every ineligible or entry candidate.
  - **Orphans:** the candidates that are not live.
  - This is the greatest fixpoint of design §9.3's definition, so a dead cycle (`a ↔ b`, called only from the removed target) is orphaned, which the least fixpoint would miss. It terminates by construction: one BFS with a visited set (lesson 2026-09-30).
  - Orphans are computed only in deployable services. A library's symbols may be called from outside the repository.
- **D12: Q3's API hop.** A service that reaches the table only through another service's endpoint is listed under "Through APIs" with the endpoint chain. The walk goes from the endpoints that reach the table, through inbound `CONSUMES`, to the client's callers and their own endpoints, up to `--hops` (default 4, maximum 8), with a visited set on endpoints. It reuses `rb path`'s upstream continuation, including inline-handler span scoping (M4a invariant 8).
- **D13: dispositions of the M4a open items.** Item 2 is deferred, as decided in Scope.

  | M4a item | Disposition | Reason |
  |---|---|---|
  | 1: Express default imports | In: S19 | Small: one fact (`default_export`) and one lookup level. It is the most common Express layout: node-express-realworld has 19 of 20 endpoints `prefix_unknown`. Q3 and the card list endpoints, so unknown prefixes are visible noise there. |
  | 5: unbitten guard mutations | In: S22 | Fixture additions in the fixtures M4b edits anyway. If a check is redundant, record it as equivalent and delete it, rather than contorting a fixture (lesson 2026-10-06, pass 12). |
  | 7: endpoints twice in `rb impact` | In: S13 | M4b reworks `rb impact`'s output (orphans, tests) anyway. Listing an endpoint once, and showing prefix-unknown endpoints with their router everywhere, is part of that. |
  | 9: catch-all absorption | In: S20 | A known false-`heuristic` source with a one-rule fix that can only remove edges. |
  | 15: Spring bases via DI | In: S21 | DI by type is M4b's mechanism. Its result is capped at `heuristic` (`base_from_bean`), so M4a's carrier bar is untouched. |
  | 24: X1's HTML row | In: S1 | One test row, and the first code change. |

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **Neutral facts.**<br>• Phase 1 opens with M4a open item 24: a `raw_text_hooks_and_discovery_keys` row `("w/public/index.html", "<script>document.write(\"<!--\"); window.fetch = f; // --></script>", vec![Fetch])`, shown to kill the evaluator's mutant B (HTML stripped like XML).<br>• Then `rb-syntax` records `Binding.parameter`, `Binding.type_args`, `Definition.return_type`, `Definition.modifiers`, `FileFacts.default_export` and `FileFacts.mentions`, as the engineer declaration defines them.<br>• Tests per language. Positions in tests come from `src.find(…)`.<br>• `mentions` are tested per role: one row per role, plus a class literal inside `getLogger(…)` and inside `@Import(…)`, a qualified `a.b.X`, a string `"com.x.Foo"`, and the same name in two roles in one file.<br>• The full e2e suite passes with **no golden change**, since these facts do not reach the graph yet. | rb-syntax and rb-frameworks unit; e2e unchanged |
| S2 | **Bean facts** (`rb-frameworks::di`, pure):<br>• **Stereotypes**, import-gated: `Component`, `Service`, `Repository`, `Controller`, `RestController`, `Configuration`, `ControllerAdvice`, `RestControllerAdvice`. Meta-annotations declared in the repository are not followed: such a class is no bean, and if it implements an injected type the point is capped (gate `annotation_unread`).<br>• **`@Bean` methods** in a stereotype or `@Configuration` class. The type is `return_type`; a return type that is not a repository type (`Object`, an external interface) makes every repository class created by `new` in its body an extra candidate (gate `bean_type_inferred`).<br>• **Spring Data repositories**: M2b's resolver plus D3's store bases.<br>• **Feign interfaces** (`@FeignClient`), beans only when `@EnableFeignClients` is on a class of the same service.<br>• Bean names (D9), `@Qualifier` literals, `@Primary`.<br>• **Conditionals**: `@Profile`; `@Conditional` and `@ConditionalOn*` from `org.springframework.context.annotation` and `org.springframework.boot.autoconfigure.condition`.<br>• Scan roots (D8), and `@EnableJpaRepositories`/`@EnableFeignClients` roots.<br>• A scan or enable annotation with an attribute outside {`scanBasePackages`, `basePackages`, `value`, `basePackageClasses`, `exclude`, `excludeName`, `proxyBeanMethods`} sets `scan_unread`.<br>Decoys, each a negative unit: a same-named `@Service` imported from another package; a commented `// @Component`; a stereotype on an interface (no bean); `@Bean` in a class with no stereotype or `@Configuration`. | rb-frameworks unit |
| S3 | **Injection points** (pure, D9):<br>• **Forms:** constructor (implicit single, or `@Autowired`), Lombok-generated constructors, `@Autowired`/`@Inject`/`@Resource` fields and setters, `@Bean` method parameters.<br>• **Excluded:** parameters or fields annotated `@Value`; types `String`, primitives and boxed primitives.<br>• **Each point records:** owner, form, declared type and type arguments, wrapper, qualifier, name, annotations outside the inert list, span.<br>• A class with several constructors and none marked gives no constructor points (`constructor_ambiguous`).<br>• Lombok rules have one row each: final field, `@NonNull` field, static field excluded, initialised final excluded, a declared constructor present (no generation).<br>• An `@Autowired` field in a class that is not a bean is a point in bucket `not_a_bean`. | rb-frameworks unit |
| S4 | **Candidates and narrowing** (indexer):<br>• **Injected type:** resolved through SCIP occurrences in SCIP-owned files, or M3a's type resolution otherwise. An injected type declared outside the repository gets bucket `external`, unless a repository `@Bean` method's return type names it; then that method is the candidate, capped at `heuristic` (gate `external_type`), since auto-configuration may provide another.<br>• **Assignability:** the bean type equals the injected type or reaches it through resolved supertypes (type-level `IMPLEMENTS` edges, both tiers), transitively. The walk has a visited set and depth cap 64; it is tested at depth 1, at depth 2 through a class and through an interface, in reverse at each depth, and with two same-named types where only the wrong one leads to the target (lessons 2026-09-30 and 2026-10-01).<br>• **Classpath and scan roots** follow D8. Narrowing and wrappers follow D9.<br>• One unit per rule, plus two candidates where `@Qualifier` and `@Primary` disagree (the qualifier wins) and a qualifier naming no bean (bucket `unresolved`, detail `qualifier`). | rb-indexer unit |
| S5 | **`INJECTS` edges and certainty.** Certainty is the minimum of the resolution certainty (D2) and this DI rule:<br>• **`synthetic`:**<br>  – one candidate (or, for a collection, each candidate), reached without name fallback;<br>  – the consumer is a main-source bean under the service's literal scan roots;<br>  – no conditional among the consumer, the candidates and the candidates narrowing removed;<br>  – no gate below failed: `by_name`, `generic`, `external_type`, `bean_type_inferred`, `annotation_unread`, `scan_roots_unknown`, `scan_unread`, `classpath_unknown`, `surface` (S6 D), `mention` (S6 M).<br>• **`heuristic`:** one candidate otherwise.<br>• **`ambiguous`:** 2–16 candidates after narrowing; one edge each, `candidates = n`.<br>• **More than 16:** `di_too_ambiguous`, no edge.<br>• **Attributes:** `{origin, via, qualifier?, primary?, by_name?, collection?, conditional?, candidates?, capped_by?, sites}`.<br>  – `origin` is `stereotype`, `bean_method`, `spring_data` or `feign`;<br>  – `via` is `constructor`, `field`, `setter` or `bean_parameter`;<br>  – `capped_by` is the sorted list of failed gates, which says why an edge is not `synthetic`, as `base: unverified` did in M4a.<br>• Merge per edge follows invariant 3.<br>• A table-driven unit has one row per condition: the `synthetic` row, then each gate alone lowering it, each with its own biting row (lesson 2026-10-06, "two guards that each catch the same repro mask each other's mutants"). | rb-indexer unit (table-driven) |
| S6 | **The two soundness gates** (repository-wide, allow-lists):<br>• **Gate D, the DI surface.** It fails if the repository has any of the following:<br>  – a Java import, static import or qualified use of a name from `org.springframework.beans.factory[.annotation\|.config\|.support]`, `org.springframework.context[.annotation\|.support]` or `org.springframework.boot[.autoconfigure]` outside the plain set {stereotypes, `Autowired`, `Qualifier`, `Value`, `Bean`, `Primary`, `Lazy`, `Scope`, `DependsOn`, `Order`, `Import`, `Profile`, `Conditional*`, `ComponentScan`, `SpringBootApplication`, `SpringApplication`, `EnableAutoConfiguration`, `ApplicationContext`, `ApplicationEvent*`, `EventListener`, `CommandLineRunner`, `ApplicationRunner`, `InitializingBean`, `DisposableBean`, `ConfigurationProperties`, `EnableConfigurationProperties`};<br>  – a wildcard import of those packages;<br>  – an XML file whose text names the `springframework.org/schema/beans` namespace;<br>  – a `META-INF/spring.factories` or `META-INF/spring/*.imports` file;<br>  – a `beans.xml`;<br>  – in an unparsed non-inert file (M4a amendment 17's selection), a whole word from {`BeanPostProcessor`, `BeanFactoryPostProcessor`, `BeanDefinitionRegistryPostProcessor`, `ImportBeanDefinitionRegistrar`, `ImportSelector`, `FactoryBean`, `registerBean`, `BeanDefinition`, `Component`, `Service`, `Repository`, `Configuration`, `Bean`} as an annotation (`@Word`) or call. This raw-text list is a word list, like M4a's, and is stated as such.<br>• **Gate M, mentions.** For the injected type and each candidate's class (or a `@Bean` method's return type), every mention in every parsed Java file is in a role from {`declaration`, `import`, `supertype`, `type`, `new`, `member`, `annotation`, `logger`}. Any `class_literal`, `method_ref`, `string`, or `other` role fails the gate (`@Import(X.class)`, `SpringApplication.run(App.class, X.class)`, `registerBean(X.class, X::new)`, `"com.x.X"` in a `Class.forName`), and so does a whole-word occurrence in an unparsed non-inert file. A failure caps only the points whose injected type or candidates it names.<br>• **Tests:** one biting row per surface name class and per role; controls stay `synthetic` (a `getLogger(X.class)`, `new X()` in a test, an `@Import` of an unrelated class); a guard asserts that no capped point is `synthetic`. Mutations: each gate removed alone fails its rows. | rb-frameworks + rb-indexer unit; mutations |
| S7 | **DI coverage.**<br>• **Buckets:** every injection point falls in exactly one of `synthetic`, `heuristic`, `ambiguous`, `too_ambiguous`, `external`, `unresolved` (repository type, no candidate; detail `no_bean`, `qualifier` or `constructor_ambiguous`), `optional_absent`, `not_a_bean`, `test`.<br>• **Diagnostics:** `di_unresolved`, `di_too_ambiguous` and `di_not_a_bean`, each per site with span and detail. One `di_coverage` per snapshot holds every bucket, zeros included, plus `gates: {<gate>: n}`.<br>• **`rb status` line:** `di: <p> injection points: <a> synthetic, <b> heuristic, <c> ambiguous, …` (zero buckets omitted, order fixed).<br>• A unit asserts that the buckets sum to the points. When a golden with counts is first blessed, one member of each non-zero bucket is identified (lesson 2026-09-30). | rb-indexer unit; e2e golden `status-di.txt` |
| S8 | **Spring Data and Feign beans (D3).**<br>• A Spring Data repository interface under the service's repository roots (the `@EnableJpaRepositories` roots, else the scan roots) is a bean of its own type.<br>• Fragment implementations count as instantiated for RTA (S9 h).<br>• A Feign interface under the `@EnableFeignClients` roots is a bean of its own type.<br>• An `INJECTS` into each carries `origin`.<br>• Decoys: a repository interface outside every root is no candidate (bucket `unresolved`); a `@FeignClient` interface in a service with no `@EnableFeignClients` is no candidate. | rb-indexer unit; e2e guards |
| S9 | **RTA proof** (index time, D4). A type T (a class, or an interface for its default methods) is **proven uninstantiated** only when all of these hold:<br>(a) T is declared in a Java or TS/JS file;<br>(b) T's file is in a service with `role = service`, not under a test path (invariant 6);<br>(c) every mention of T's simple name in every parsed Java/TS/JS file is in role `declaration` (of T or its constructors), `import`, or `supertype` (handled by i);<br>(d) T's simple name occurs in no string literal and in no unparsed non-binary file, prose and npm lock files excepted (M4a's inert classes; XML comments are **not** stripped, so `<!--` inside CDATA cannot hide a name: M4a open item 22 c);<br>(e) T, and every member of T, carries no annotation or decorator outside the inert list (S12's list);<br>(f) T is not default-exported (TS/JS), and is not an enum with constants;<br>(g) the repository has no unnamed-instantiation surface: a `beans.xml`; an `includeFilters` attribute on a scan annotation; an import from `org.reflections` or `io.github.classgraph`; or `ClassPathScanningCandidateComponentProvider`;<br>(h) T does not implement, by name, an interface that a Spring Data repository extends (fragment convention, D3);<br>(i) every type that names T as a supertype (by simple name, any file) is itself proven uninstantiated. This is a propagation over the name-supertype graph with a visited set: "instantiated" flows from subtype to supertype, so a cycle (`A extends B`, `B extends A`) terminates. Types in Python, Go and other languages are never proven.<br>**Output:**<br>• every `IMPLEMENTS` edge whose source is T, or a method T contains, gets `impl_instantiated: false`;<br>• one `rta_coverage` per snapshot: types considered, proven, and the first failing rule per kept type (`mention`, `string`, `annotation`, `test`, `library`, `default_export`, `surface`, `fragment`, `subtype`, `language`); a unit asserts the sum;<br>• `rb status` prints `rta: <n> of <m> implementing types never instantiated (<k> dispatch edges prunable)`.<br>**Tests:** one keeping row per rule (`new T()`, `T::new`, `T.class`, a type position, `"com.x.T"`, the name in a `.kt` file, `@Component`, `@Test` on a method, a test path, a library service, `export default class T`, a CDI `beans.xml`, a fragment, an instantiated subtype at depth 1 and 2); proving rows (the fixture's `KafkaAuditSink`); a cycle; depth 1/2 and reverse (lesson 2026-09-30). | rb-frameworks + rb-indexer unit; mutations |
| S10 | **Precision at query time (D5).**<br>• `rb callers`, `rb path`, `rb impact` and `rb table-deps` take `--precision rta\|cha`. A dispatch step taken along an `IMPLEMENTS` edge with `impl_instantiated: false` is skipped under `rta` and counted.<br>• The step sources are:<br>  – `rb callers`: callers of the methods the target overrides (`code.rs:114–166`);<br>  – `rb path`: implementations downstream, and overridden methods upstream (`path.rs:332–351, 440–449, 630–636`);<br>  – `rb impact` and `rb table-deps`: their dispatch expansion.<br>• Text output gains a header `precision=<p>`, and when the count k > 0 a line `rta: <k> dispatch steps pruned (implementations never instantiated; --precision cha shows them)`. JSON gains `precision` and `pruned_by_rta`.<br>• Unit on built graphs: the same graph under both modes, a pruned step counted, a kept subtype-instantiated step. | rb-store unit; e2e goldens |
| S11 | **Entry points (D10).**<br>• **Stored kinds:**<br>  – `main`: a Java `static` method `main` with one `String[]`/`String...` parameter; a Go `func main()` in package `main`;<br>  – `test`: Java methods annotated `Test`, `ParameterizedTest`, `RepeatedTest`, `TestFactory` or `TestTemplate` (JUnit 4/5 packages); Go `Test*`, `Benchmark*`, `Fuzz*` and `Example*` functions in `_test.go`; Python `test_*` functions in test files; TS/JS test files' top-level code (marked on the `File` at query time);<br>  – `scheduled`: `@Scheduled`;<br>  – `listener`: `@EventListener`, `@TransactionalEventListener`, `@KafkaListener`, `@RabbitListener`, `@JmsListener`;<br>  – `lifecycle`: `@PostConstruct`, `@PreDestroy`, `@Bean` methods, and `run` in a class implementing `CommandLineRunner` or `ApplicationRunner`.<br>• All are import-gated (invariant 5) and stored as the `Symbol` attribute `entry` (sorted).<br>• **Query-time kinds:** `endpoint` (a `HANDLED_BY` target, with the endpoint's name and, if prefix-unknown, its router) and `top-level` (a `File` source).<br>• `rb callers` prints `(entry: <kind>[, <kind>…])` after an entry point's line, wherever it appears in the tree. Expansion continues past it (nothing hidden). JSON gains `entry`.<br>• Unit per kind with a decoy (a non-static `main`, a `@Test` from another package); e2e goldens. | rb-indexer + rb-store unit; e2e goldens |
| S12 | **Orphan eligibility** (index time, D11). A `Symbol` gets `orphan_eligible: true` only when all hold:<br>(a) its file is in a `role = service` service and not under a test path;<br>(b) it is no entry point (S11);<br>(c) it and its enclosing types carry no annotation or decorator outside the **inert list**. The list holds `Override`, `Deprecated`, `SuppressWarnings`, `FunctionalInterface`, `SafeVarargs`, `Transactional`, `Validated`, `Valid`, `NonNull`/`Nullable` (any package), and Lombok's `Getter`, `Setter`, `Data`, `ToString`, `EqualsAndHashCode`, `NoArgsConstructor`, `AllArgsConstructor`, `RequiredArgsConstructor`, `Builder`, `Slf4j`, `Log4j2`, `CommonsLog`, `Log`. It is one constant with a unit; an addition needs a stated reason that the annotation makes no framework call the symbol. The stereotypes are deliberately **not** inert: a bean's methods may be called by the container;<br>(d) its name is not convention-invoked:<br>  – Java and TS methods named `toString`, `equals`, `hashCode`, `compareTo`, `compare`, `close`, `iterator`, `run`, `call`, `get`, `accept`, `apply`, `test`, `finalize`, `readObject`, `writeObject`, `readResolve`, `writeReplace`, `valueOf`, `values`, or `get`/`is`/`set` followed by an upper-case letter (bean accessors read by Jackson, JPA and templates through property names);<br>  – Python `__*__`;<br>  – every Go method, since interfaces are satisfied structurally and called implicitly (`fmt` calls `String()`);<br>(e) its class declares no supertype outside the repository (an override of an external method is called by the library);<br>(f) every whole-word occurrence of its simple name in the text of every non-binary file of the repository is its own declaration's name span or lies inside the evidence span of an edge into it. The scan covers comments and strings, every language, and no inert exemptions, so it can only over-protect; it is a linear word scan, uncached, and its cost is measured.<br>**Coverage:** `orphan_coverage` counts symbols and eligible symbols, with the first failing rule. The status line is not printed (one count in `rb status --verbose` is not needed).<br>**Tests:** one keeping row per rule: a mention in a comment, in a Thymeleaf template, in a string `"process"`, in a same-named method elsewhere; `@Scheduled`; `getEmail`; a Go method; `class C implements Runnable`; a library service; a test path. Eligible rows: a private helper called once. | rb-indexer unit |
| S13 | **`rb impact` orphans, tests and item 7.**<br>• **Removed set:** the target plus its `CONTAINS` descendants (existing).<br>• **Candidates:** symbols in the target's service forward-reachable from it over outbound `CALLS` ∪ `REFERENCES` ∪ dispatch (implementations of reached methods; with `--precision rta` skipping flagged edges).<br>• **Liveness:**<br>  – every node outside removed ∪ candidates is live;<br>  – the seeds are every candidate that is not `orphan_eligible` or is an entry point, plus every candidate with an inbound requiring edge (`CALLS`, `REFERENCES`, `INJECTS`, `HANDLED_BY`, `MAPS_TO`) from a live node;<br>  – liveness flows forward along those edges inside the candidates, and from a live method (outside nodes included) to its implementations, by dispatch;<br>  – an implementation of a removed method gets nothing from it.<br>• **Orphans** are the candidates that are not live (D11), listed as `orphans: may become dead code (n):` with distance and the edge that last kept each alive inside the dead region. A target whose service is a library prints `orphans: not computed (library service)`.<br>• **`tests (n):`** impacted nodes whose `entry` contains `test`, and `File`s of test paths.<br>• **Item 7:**<br>  – `breaks` no longer lists `Endpoint` nodes: an endpoint whose handler is removed appears once, under `endpoints:` at distance 1, marked `(handler removed)`;<br>  – a prefix-unknown endpoint is shown as `<name>  (prefix unknown, router <file>#<binding>)` in `rb impact`, the card, the `rb path` header and `rb table-deps`.<br>• `rb impact` gains `--json`.<br>• **Units:** a dead mutual recursion is orphaned; a cycle with one outside caller is not; an implementation of a live interface method is live; an eligible symbol with an entry kind is live; the 5,000-deep chain; growth (invariant 9). | rb-store unit; e2e goldens |
| S14 | **Q3 roll-up (`rb table-deps`, D12).**<br>• **Data-access nodes:** the sources of `QUERIES` into the table (symbols and `File`s).<br>• **Callers up to entry points:** a BFS over inbound `CALLS` ∪ `REFERENCES` plus dispatch (default `cha`), depth cap 25, node cap 200,000, `truncated` named.<br>• **Per service** (the reached nodes' files), in this order:<br>  – `ops` (the union of the reached `QUERIES` ops);<br>  – `direct` (it contains a data-access node) or `indirect via <service>` (it reaches the table only through another service's code);<br>  – the endpoints `HANDLED_BY` a reached node. An inline handler counts only when the evidence that reached it lies in its spans (M4a invariant 8);<br>  – the other entry points reached, by kind;<br>  – the best certainty, as the minimum along each path and the best per service.<br>• **"Through APIs":** services reaching those endpoints only through `CONSUMES`, with the chain `via <endpoint> of <service>` and the certainty, up to `--hops`.<br>• Migrations and Mapped by are unchanged.<br>• The line `└─ Callers up to entry points arrive with call resolution (M3)` is gone from the three goldens that have it.<br>• JSON gains `services` and `through_apis`.<br>• Units: an inline handler in a file with two endpoints; a two-hop API chain; a service reached both directly and through an API (listed once, under services); a `CONSUMES` cycle A ↔ B. | rb-store unit; e2e goldens |
| S15 | **Manifest packages (D7).**<br>• Readers:<br>  – `package.json` direct dependencies with `package-lock.json` v2/v3 (`packages["<dir>/node_modules/<name>"]`, walking up from the service directory to the lockfile's directory, as Node resolves);<br>  – `go.mod` `require` lines, single and block, skipping `// indirect`;<br>  – `pom.xml` `<dependencies>` (not `<dependencyManagement>`), with a literal version or a same-pom property.<br>• Edges and attributes as D7, with evidence at the manifest line of the dependency.<br>• **Diagnostics:** `lockfile_unreadable` (bad JSON, unknown `lockfileVersion`); `lockfile_missing_entry` (a declared dependency absent from the lock); `lockfile_unread` (a `yarn.lock` or `pnpm-lock.yaml` present: versions unresolved).<br>• A dependency on another service's package (S16) gives no `Package` edge.<br>• **Units:** hoisted and nested lock entries; a scoped package; a v1 lock (unreadable); go `require` block with `// indirect`; a pom with literal, property, parent-managed and `dependencyManagement` entries; an unknown property. | rb-indexer unit; e2e (ts-express-prisma lockfile, polyglot go.mod and poms) |
| S16 | **Service → Service `via=package`.** A manifest dependency names another service of the repository: an npm `name`, a Maven `groupId:artifactId`, or a Go module path matched by `require`, or by `replace` to a local directory. It gives `Service -DEPENDS_ON{via: package}-> Service` (provenance `manifest`).<br>• **`synthetic`** when:<br>  – npm: the range is `*`, empty, `workspace:…`, or `file:`/`link:` to that directory;<br>  – Maven: groupId, artifactId and literal version equal the module's;<br>  – Go: a `replace` to its directory.<br>• **`heuristic`** otherwise (a range that may resolve to the registry).<br>• A name matching several services gives `ambiguous` edges, one per service.<br>• The closure over these edges is classpath(S) (D8); it has a cycle test. | rb-indexer unit; e2e (`apps/web → packages/shared`) |
| S17 | **Service dependencies at query time (D6).** For a service S:<br>• `via=calls`: services owning the targets of `CALLS`/`REFERENCES` from S's symbols and `File`s into another service, with the call and reference sites counted and the best certainty;<br>• `via=http`: services exposing endpoints S `CONSUMES` (M4a's `consumes`), with the endpoints;<br>• `via=package`: stored edges;<br>• **depended on by:** the inverse of all three;<br>• **shares tables with:** other services whose symbols or files `QUERIES` or `MAPS_TO` a table S also accesses, with the tables. This is coupling, kept apart from dependencies (design §9.5).<br>All lists are sorted by service key, bytewise. Unit on built graphs: two vias between one pair; a self-call excluded; a library dependency. | rb-store unit |
| S18 | **Full Service Card** (text and `--json`). After the M4a sections, the card prints these sections, each `none` when empty, the last closing the tree:<br>• `Packages (n)`: `<name> <version>  [<version_from>]`, and `(scip)` for M3b's edges;<br>• `Database tables (n)`: table, op union, best certainty, `(schema assumed)`;<br>• `Dependencies`: services by via, then packages count;<br>• `Depended on by`;<br>• `Shares tables with`;<br>• `Call graph`: symbols; edges by kind with both ends in the service (`CALLS`, `REFERENCES`, `IMPLEMENTS`, `INJECTS`); certainty shares; entry points by kind.<br>JSON gains `tables`, `dependencies`, `depended_on_by`, `shares_tables`, `call_graph`, `entry_points`. `packages` becomes objects `{key, name, version, version_from, provenance}`.<br>`ServiceCard`'s doc line "arrive with M4b" (`query.rs:34–35`) is removed. | e2e goldens: every card (polyglot `card-auth`, `card-gateway`, `card-notification`, `card-root.json`; the SCIP cards) |
| S19 | **M4a item 1, Express default exports.** A mount `app.use(prefix, r)` whose `r` is a default import of a file is resolved through `default_export`. The exported name must be a top-level Express router binding in that file. This is one level: a default export that is itself an import gives `prefix_unknown`, counted. The endpoint's key loses ` ?router=` and gains the composed prefix; its path certainty is `heuristic` (composed across files, M4a S7). Unit: named and default mounts, a default export of a non-router, a default re-export. | rb-frameworks + rb-indexer unit; e2e (ts-express-prisma fixture plan) |
| S20 | **M4a item 9, catch-all absorption.** An endpoint whose literal prefix before `{*}` is empty (`/{*}`) is a candidate only for a client whose host restricts candidates to its service (D6 of M4a: named host, not a proxy). Otherwise such a site is `unmatched`, with detail `catch_all_only` when that endpoint was the only path match. This can only remove edges. Units: evaluator pass 1's `a1` (`fetch("/totally/unknown/path")` → no edge); the same with host `spa` → `heuristic`; `/assets/{*}` unaffected. | rb-indexer unit |
| S21 | **M4a item 15, DI-resolved client bases.** M4a reads an injected `RestTemplate`/`WebClient`/`RestClient` field's base only from a same-file `@Bean` (verified fact 13). Now it takes it from the `@Bean` method its injection point resolves to (S4–S5, any file), when that resolution has exactly one candidate (any certainty) and the method's base folds. Otherwise the base is unknown, as before. The site keeps `base_from_bean` and is never `synthetic`. The carrier is a field, capped by M4a amendment 14 anyway. Units: a `@Configuration` class in another file; two qualified `@Bean`s resolved by the point's qualifier; an ambiguous resolution (unknown base). | rb-indexer unit |
| S22 | **M4a item 5, guards that bite.** Fixture additions (fixture plan) make the own-service exclusion the only rule between a site and a false `CONSUMES` (a relative call to an endpoint only its own service exposes). Its mutation must fail a guard. For the dynamic-URL check, the generator first tests whether it is equivalent to M4a amendment 11's P2 rule (a suffix match needs a literal in the matched tail). If equivalent, the redundant check is deleted, or kept and recorded as equivalent, with the reasoning; no fixture is contorted. Invariant 1's "one `EXPOSES` per endpoint" stays checked on every dump; no mutation can violate it by construction (the key contains the service). | e2e mutations, recorded |
| S23 | **Termination and growth** (invariants 8, 9).<br>• **Cycle tests:** supertype cycle (assignability and RTA), `via=package` cycle (classpath), orphan mutual recursion, Q3 `CONSUMES` A ↔ B.<br>• **Depth tests:** a 5,000-deep supertype chain is cut at 64 and is conservative there (instantiated, not assignable); 5,000-deep call chains for orphans and Q3.<br>• **Counted growth** (≤ 5× for 4× the input):<br>  – DI resolution (beans and points with distinct types);<br>  – RTA (types and mentions);<br>  – the mention scan (words);<br>  – orphans (chain and fan);<br>  – Q3 (chain and fan);<br>  – card aggregation (edges).<br>• **Pathological:** 2,000 beans implementing one interface against 2,000 single-valued points, all `too_ambiguous`, within a wall-clock bound an order of magnitude above a measured run. | unit |
| S24 | **Goldens and precision guards** (`--no-scip`).<br>• **New goldens:**<br>  – `status-di.txt`;<br>  – java-spring-jpa: `callers-kafka-record.txt` (default `rta`: the dispatch caller is pruned and counted) and `callers-kafka-record-cha.txt` (`--precision cha`: `DefaultAuthService#authenticate()` via dispatch), `callers-authenticate.txt` re-blessed with entry marks, `impact-legacy-orphans.txt`, a card;<br>  – Q3: `table-deps-users.txt` (java-spring-jpa), `table-deps-public-users.txt`, `table-deps-email-log.txt` (polyglot), `table-deps-invoices.json`;<br>  – the polyglot cards.<br>• **Guards:** `must-exist.txt` and `must-not-exist.txt` gain the fixture plan's `INJECTS`, `DEPENDS_ON` and `impl_instantiated` lines; the guard format allows an attribute subset `{"impl_instantiated": false}` (M4a amendment 8).<br>• **Invariant checks over every fixture dump:**<br>  – no `INJECTS` above `synthetic`; every `INJECTS` has provenance `di` and joins two `Symbol`s;<br>  – every new edge has evidence;<br>  – `impl_instantiated` appears only as `false` and only on `IMPLEMENTS`;<br>  – the `CALLS`/`REFERENCES`/`IMPLEMENTS` rows, attributes aside, equal those of an `m4a-http-11` dump (invariant 1);<br>  – producer coverage: `INJECTS` and both `DEPENDS_ON` forms each have a producer and a fixture asserting them (design §8.5).<br>• Each guard line and check is bitten by a recorded mutation (M4a amendment 8's harness rules: fresh mtime, restored bytes checked, `test result:` printed per mutant; lessons 2026-10-06). | e2e |
| S25 | **SCIP mode.**<br>• java-spring-jpa with scip-java: `AuthController → DefaultAuthService` and the `@Primary` hasher are `synthetic`, the profile issuers `ambiguous` with `conditional`. This is the `synthetic` row end to end (D2). Golden `scip-injects.txt`.<br>• Every M3b and M4a SCIP golden still holds, re-blessed only where the fixture plan changes inputs, each diff classified.<br>• A run that prints `skipping:` counts as not run (lesson 2026-09-30).<br>• Before the run: `df`, `docker context show`, `docker image inspect` for both images (lesson 2026-09-30). The new fixture classes need only `spring-context`/`spring-boot-autoconfigure`, which the existing starters bring (verify in phase 0 that the `.m2` cache holds them; the fetch phase has network otherwise). | e2e with Docker Desktop and both images |
| S26 | **Determinism and AGE parity.**<br>• Two repos, a forced rebuild, warm = cold, and SCIP cached = fresh give byte-identical dumps.<br>• `rb_indexer::PIPELINE` becomes `m4b-di-1`, and is bumped on every output change. The extractor fingerprint changes with S1.<br>• The AGE projection has `INJECTS` edges and Service → Service `DEPENDS_ON`, and their 1-hop neighbour sets equal the relational ones on both Java fixtures. | e2e |
| S27 | **Public repositories.** Pinned commits are recorded at measurement. Never anything under `/Users/rohit/Public/alvora`. Each candidate is verified by reading the clone before it is chosen; a substitution is recorded with its reason. Candidates:<br>(a) spring-projects/spring-petclinic @ `818c4136ea97` (M3b's pin): constructor injection, Spring Data repositories, `@SpringBootApplication`; SCIP and `--no-scip`;<br>(b) spring-petclinic/spring-petclinic-rest @ `afc8fc1d3b8e` (M3b's pin): repository interfaces with jdbc/jpa/spring-data-jpa implementations under `@Profile` (conditional, ambiguous) and MapStruct `@Mapper` interfaces whose implementations are generated (bucket `unresolved`, residue); SCIP and `--no-scip`;<br>(c) sqshq/piggymetrics @ `6bb2cf9ddbca` (M4a's pin): Feign beans, Mongo repositories (D3);<br>(d) spring-petclinic/spring-petclinic-microservices @ `dc9ca15a7118` (M4a's pin): Lombok constructors (D9);<br>(e) typestack/routing-controllers @ `bf6fbfdc7fa5` (M3b's pin): TS RTA and orphans;<br>(f) gothinkster/node-express-realworld-example-app @ `30b68e1e8814` and golang-gin-realworld @ `626c372d2594` (M4a's pins): lockfile/go.mod versions, S19 (the 19 `prefix_unknown` endpoints should become known), orphans in Go.<br>**Recorded per repository:**<br>• DI buckets and gates (one member per non-zero bucket identified);<br>• RTA counts (types considered, proven, edges prunable; first-failing-rule histogram);<br>• orphan eligibility share;<br>• packages per manager and `version_from`;<br>• service dependencies by via;<br>• wall time and facts-cache size against the M4a closing binary (`28e47ba`) on the same commit;<br>• fresh == cached.<br>**Precision:**<br>• `INJECTS`: 20 per repository by the M3b sampling rule (sorted by key, N = ⌊count/20⌋, indices ⌊N/2⌋ + i·N; all if fewer), each read in the source. Targets: `synthetic` 100%, `heuristic` ≥ 90%, every `ambiguous` edge a real candidate, 0 false `synthetic`.<br>• **RTA:** every proven type (all if ≤ 20, else 20 sampled) is checked by reading for any instantiation; the target is 0 instantiated. `rb callers` on 10 sampled overridden methods under both precisions is diffed and classified.<br>• **Orphans:** `rb impact` on 10 sampled methods per repository (sampling rule over eligible symbols with callers); every listed orphan is read for any use; the target is 0 false orphans.<br>• **Packages:** 20 sampled `manifest` edges checked against the lockfile or manifest; 20/20.<br>• **Q3:** `rb table-deps` on 2 tables per Java repository is checked by reading.<br>A measured cost (for example, the share of types kept by `mention`) is recorded, not optimised, unless it contradicts a rule (closing bar 5). | recorded measurement in this contract |
| S28 | **Verification loop and closing.**<br>• `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, unit tests (counted from a log holding both streams, lesson 2026-10-06), and the e2e suite with the database and with containers (no `skipping:`).<br>• Stub scan clean. No new dependency; a new one needs a stated reason.<br>• Every changed pre-existing golden explained by a classified line diff in the changelog (lesson 2026-09-30).<br>• Evaluator passes per the closing bar above: pass 1 full, pass 2 verification-only. | verification loop |

## Fixture plan

New fixture files must be tracked (`git add`) before the e2e runs (`fixture_repo` copies `git ls-files` only; M4a fixture plan). Package-private top-level classes share files, so the plan stays within five files per phase. Spring scans package-private classes. Every decoy below is a `must-not-exist` line. The generator writes the exact keys after the first index, as M4a did.

**`java-spring-jpa`** (package `com.acme.auth`; no `@SpringBootApplication` today, so every point would be capped by `scan_roots_unknown`):

| File | Change | Purpose |
|---|---|---|
| `AuthApplication.java` (new) | `@SpringBootApplication public class AuthApplication { public static void main(String[] a) { SpringApplication.run(AuthApplication.class, a); } }` | Scan root `com.acme.auth`; entry kind `main`. The `class_literal` of `AuthApplication` in `run(…)` names no injected type, so gate M is not touched. |
| `Hashing.java` (new) | `public interface PasswordHasher { String hash(String s); }`; `@Component @Primary class BcryptHasher implements PasswordHasher`; `@Component class PlainHasher implements PasswordHasher` | `@Primary` narrowing: `INJECTS DefaultAuthService → BcryptHasher#`. `PlainHasher` also gets a qualified point (below). |
| `Tokens.java` (new) | `public interface TokenIssuer { String issue(String user); }`; `@Component @Profile("prod") class JwtIssuer`; `@Component @Profile("dev") class DevIssuer` | Conditional candidates: two `ambiguous` edges with `conditional: true`, never `synthetic` (SCIP mode either). |
| `AuditConfig.java` (new) | `@Configuration public class AuditConfig { @Bean AuditSink auditSink() { return new LogAuditSink(); } }`; `interface AuditSink { void record(String e); }`; `class LogAuditSink implements AuditSink`; `class KafkaAuditSink implements AuditSink` (never referenced) | `@Bean` factory: `INJECTS DefaultAuthService → AuditConfig#auditSink()` with `origin: bean_method`. RTA: `KafkaAuditSink` is proven uninstantiated (`impl_instantiated: false`); `LogAuditSink` is kept (`new`). The `@Bean` method is entry kind `lifecycle`. |
| `Notifiers.java` (new) | `public interface Notifier { void send(String to); }`; `@Component class EmailNotifier`; `@Component class SmsNotifier` | `List<Notifier>` in `AuthController` gives two edges with `collection: true`. A point `Notifier smsNotifier` in `DefaultAuthService` resolves by name fallback: `heuristic`, `capped_by: [by_name]`. |
| `UserRepositoryCustomImpl.java` (new); `UserRepository.java` (edit) | `interface UserRepositoryCustom { void touch(long id); }` added to `UserRepository.java`, and `UserRepository` extends it; `class UserRepositoryCustomImpl implements UserRepositoryCustom` (never referenced) | Spring Data fragment: kept instantiated by S9 h. A guard checks that its `IMPLEMENTS` has no `impl_instantiated`. `INJECTS DefaultAuthService → UserRepository#` (`origin: spring_data`). |
| `support/Service.java`, `support/ReportNotifier.java` (new, package `com.acme.auth.support`) | `public @interface Service {}`; `@com.acme.auth.support.Service public class ReportNotifier implements Notifier` | A same-named non-Spring stereotype: no candidate. The `List<Notifier>` stays at two edges. |
| `AuthController.java` (edit) | Adds `@Autowired void setIssuer(TokenIssuer i)`, `@Autowired @Qualifier("plainHasher") private PasswordHasher legacyHasher;` and `List<Notifier> notifiers` as a second constructor parameter | Setter injection; the qualifier picks `PlainHasher` over the `@Primary`. |
| `DefaultAuthService.java` (edit) | Constructor adds `PasswordHasher hasher, AuditSink audit, Notifier smsNotifier`; `authenticate` calls `audit.record(email)` | `@Primary`, `@Bean` and name-fallback points; `AuditSink#record` gets a dispatch for RTA (`callers-kafka-record*.txt`: `KafkaAuditSink#record`'s dispatch caller pruned under `rta`, shown under `cha`; `LogAuditSink#record` keeps it under both). |
| `LegacyAuth.java` (edit) | Adds `@Autowired private AuthService delegate;` and a line `// @Component` above the class | An `@Autowired` field in a non-bean: bucket `not_a_bean`, no `INJECTS`. The commented stereotype gives nothing. `LegacyAuth#authenticate` is uncalled: an orphan case for `impact-legacy-orphans.txt`, run on a helper it alone calls (added in the same edit: a `private String legacyHash(String)` called only from `authenticate`). |

**java-spring-jpa guard lines:**
- **must-exist:**
  - `INJECTS` `AuthController# → DefaultAuthService#` (`heuristic` without SCIP);
  - `INJECTS` `DefaultAuthService# → BcryptHasher#` with `primary`;
  - `INJECTS` `AuthController# → PlainHasher#` with `qualifier`;
  - two `INJECTS` to `JwtIssuer#`/`DevIssuer#`, `ambiguous`, `conditional`;
  - two `collection` edges to the notifiers;
  - `INJECTS DefaultAuthService# → AuditConfig#auditSink().`;
  - `INJECTS DefaultAuthService# → UserRepository#`;
  - `IMPLEMENTS KafkaAuditSink#record() → AuditSink#record()` `{"impl_instantiated": false}`.
- **must-not-exist:**
  - `INJECTS * -> …BcryptHasher#` from `AuthController` (the qualifier wins);
  - `INJECTS * -> …ReportNotifier#`;
  - `INJECTS …LegacyAuth# -> *`;
  - `INJECTS DefaultAuthService# -> …PlainHasher#` (`@Primary` wins);
  - `IMPLEMENTS …LogAuditSink#record() -> *` `{"impl_instantiated": false}`;
  - `IMPLEMENTS …UserRepositoryCustomImpl#touch() -> *` `{"impl_instantiated": false}`.

**`polyglot-monorepo`:**

| File | Change | Purpose |
|---|---|---|
| `services/auth/.../AuthApplication.java` (new) | `@SpringBootApplication @EnableFeignClients public class AuthApplication { main(…) }` | Scan and Feign roots. |
| `services/auth/.../UserDao.java` (edit) | `@Repository` on the class | A bean. `JdbcTemplate` is an external point: bucket `external`. |
| `services/auth/.../AuthController.java` (edit) | Constructor adds `UserDao users`; `login` calls `users.touch(1L)` | `INJECTS AuthController# → MfaClient#` (`origin: feign`) and `→ UserDao#`. Q3: `POST /api/v1/login` reaches `public.users` (update) and `audit.users` (insert); web reaches them through that API. |
| `services/billing/internal/invoice.go` (edit) | Adds `func Owner(db *sql.DB, id int) { db.QueryRow("SELECT email FROM users WHERE id = $1", id) }`, called from nothing | Shared table: billing's `github.com/acme/billing` (library) shares `public.users` with auth (coupling line, Q3 second service). Orphan-ineligible: library service. |
| `services/notification/src/selfcheck.ts` (edit) | Adds `export async function smsCheck() { await fetch("/internal/mfa/sms", { method: "POST" }); }` | M4a item 5: a relative call whose only path match is notification's own `/sms`. With the own-service exclusion off, it gives a self `CONSUMES`, so the guard bites. |

**polyglot guard lines:**
- **must-exist:**
  - `INJECTS` to `MfaClient#` and `UserDao#`;
  - `DEPENDS_ON apps/web -> packages/shared` `{"via": "package"}` `[synthetic]` (range `*`);
  - `DEPENDS_ON services/billing -> go/github.com/gin-gonic/gin@v1.10.0` `[synthetic]`;
  - `DEPENDS_ON apps/web -> npm/axios@?` (no lockfile, `version_from: unresolved`).
- **must-not-exist:**
  - `CONSUMES …smsCheck… -> *`;
  - `INJECTS * -> *JdbcTemplate*`.

**`ts-express-prisma`:**

| File | Change | Purpose |
|---|---|---|
| `src/routes/health.ts` (new) | `import { Router } from "express"; const health = Router(); health.get("/health", (_req, res) => res.send("ok")); export default health;` | M4a item 1. |
| `src/app.ts` (edit) | `import health from "./routes/health"; app.use("/ops", health);` | Mount through a default import: `GET /ops/health`, known prefix, `heuristic`. |

**ts-express-prisma guard lines:**
- **must-exist:**
  - `EXPOSES . -> HTTP GET /ops/health @.`;
  - `DEPENDS_ON . -> npm/express@4.22.3` `[exact]` (`package-lock.json` L253, verified fact 16), and likewise `npm/pg@8.23.0` and `npm/@prisma/client@5.22.0`.
- **must-not-exist:**
  - any endpoint key containing ` ?router=src/routes/health.ts`;
  - `GET /health` without the prefix.

**Golden churn the generator must expect and classify:**
- `dump.jsonl`;
- `services.txt` (file counts);
- every card;
- the `status-*.txt` files;
- `callers-*`, `impact-*` (orphans, tests, item 7) and `table-deps-*`;
- `path-login*` (the downstream gains `QUERIES public.users` and `audit.users`);
- the three `scip-status.txt` and every SCIP golden whose fixture changed.

The changelog explains each diff.

## Verified facts (read at `415add0`)

1. `EdgeKind::Injects` (`INJECTS`) and `Provenance::Di` (`di`) and `Provenance::Manifest` exist; nothing produces `INJECTS` or uses `di` (`rb-model/src/lib.rs:91, 118, 124`; repository grep). `EdgeKind::DependsOn` exists (`:99`). `Certainty` ranks `Synthetic = 3` below `Dispatch = 4` and `Exact = 5` (`:129–165`).
2. `GraphBuilder::add_node` rejects a different value for an existing key (`rb-model/src/graph.rs:169–182`). `add_edge` keys `(src, kind, dst, provenance)`, merges evidence and rejects differing certainty or attributes (`:186–212`). `finish` rejects dangling ends and evidence whose path is not a `File` (`:216–255`). Hence invariant 3 and D4's ordering.
3. The edge primary key is `(snapshot_id, src, kind, dst, provenance)`; certainty is `CHECK (certainty BETWEEN 1 AND 5)` (`migrations/0001_init.sql:53, 55`). Hence D6.
4. `rb_indexer::PIPELINE` is `m4a-http-11` (`rb-indexer/src/lib.rs:65`). Stage order: services, files, SCIP joins and packages, `Package`/`DEPENDS_ON`, symbols and SQL, SCIP edges, Tier B edges, endpoints, stitching, ORM, migrations (`:493–776`).
5. **Spring Data today:**
   - `REPOSITORY_BASES` lists `Repository`, `CrudRepository`, `ListCrudRepository`, `PagingAndSortingRepository`, `ListPagingAndSortingRepository` and `JpaRepository`, with no store-specific bases (`rb-frameworks/src/lib.rs:189–196`).
   - Generic bases are resolved by `repository_bases`/`BaseResolver` (memoised, cycle-cut, `MAX_BASE_DEPTH = 512`; `:686–789`).
   - `accesses` gives inherited CRUD and derived-method accesses (`:802–905`).
   - The indexer emits only `QUERIES` (provenance `orm`; `synthetic` for convention) and creates no `IMPLEMENTS`, `CALLS` or synthetic node (`rb-indexer/src/orm.rs:362–456`).
6. **Packages (M3b):** `Package` nodes keyed `<manager>/<name>@<version>`, `?` for an unknown version (`rb-scip/src/symbol.rs:78–97`, `:91`). `Service -DEPENDS_ON-> Package` with provenance `scip`, certainty `exact`, one evidence and no attributes (`rb-indexer/src/lib.rs:585–670`). No lockfile is parsed. `rb-scip/src/jobs.rs:152–157` only tests a lockfile's presence to pick the fetch command.
7. `Service` attributes are `{role, detected_by, manifests}` (`rb-indexer/src/lib.rs:890–901`). `Symbol` nodes carry only `{kind}` (`rb-indexer/src/syntax.rs:155–160`).
8. `Definition` has `kind, name, name_span, span, parent, receiver, decorations, supertypes, class_constants, type_params`, with no return type and no modifiers (`rb-syntax/src/lib.rs:146–168`). A Java constructor is a `Method` named like its class (`queries.rs:41–46`). Annotation types (`@interface`) are not definitions (design §4.4).
9. `Binding` has `name, type_name, scope, member, span, visible, init, constant, decorations` (`rb-syntax/src/lib.rs:480–505`). Type arguments are stripped (`bindings.rs:268–293`), so `List<Notifier>` keeps `List` only. A parameter is told from a local only by its span; there is no position. Java fields and parameters carry decorations (`lib.rs:619–623`). Hence S1.
10. `Decoration` has a name as written, keyed or positional arguments with literal flags, array `items` and a span (`rb-syntax/src/lib.rs:172–195`). `Supertype` has a name as written, type arguments, `implements` and a name span (`:199–210`).
11. `rb_frameworks::routes::is_annotation(facts, d, package, simple)` accepts a qualified name, or a simple name with an exact or wildcard import (`rb-frameworks/src/routes.rs:369–381`). No code mentions `Autowired`, `Qualifier`, `Primary`, `Profile` or `Component` (repository grep).
12. Tier B `implements()` emits type → resolved supertype and method → same-named method in the nearest supertype (`rb-indexer/src/resolve.rs:1104–1145`). SCIP `IMPLEMENTS` are accepted only in the declared direction (`scip.rs:533–605`).
13. `@Bean` methods are read today only for client bases (`rb-frameworks/src/clients.rs:1405–1440, 1491–1528`); a site with `base_from_bean` is never `synthetic` (`rb-indexer/src/stitch.rs:106, 708`).
14. **Query code:**
    - `rb_store::code::REQUIRING` already lists `Injects` (`rb-store/src/code.rs:16–24`).
    - CHA lives in `overridden` (`:114–131`), `callers_of` (`:135–166`), `callers` (`:325–383`) and `impact` (`:412–538`), and in `path.rs:332–351`, `:440–449` and `:606–640`. There is no precision flag, no RTA and no entry notion.
    - `impact` lists endpoints both under `breaks` (`HANDLED_BY` is requiring) and in `endpoint_rollup` (`:544–596`); this is M4a open item 7.
    - The CLI's `impact` has no `--json` (`rb-cli/src/main.rs:153–162`).
15. **Service Card:** `ServiceCard` has `packages` (`DEPENDS_ON` targets, `query.rs:544`), `apis`, `consumes`, `consumed_by` and `unstitched`, and its doc says tables, service dependencies and the call graph "arrive with M4b" (`rb-store/src/query.rs:34–62`). `render_card` is `rb-cli/src/main.rs:922–1007`. `table_deps` has no caller walk (`query.rs:261–339`). Three goldens end in `└─ Callers up to entry points arrive with call resolution (M3)`: `java-spring-jpa/table-deps-users.txt`, `polyglot-monorepo/table-deps-public-users.txt` and `polyglot-monorepo/table-deps-email-log.txt`.
16. **Fixtures:**
    - No fixture has `@Bean`, `@Configuration`, `@Component`, `@Qualifier`, `@Primary`, `@Profile`, `@Autowired`, `@Repository` or `@SpringBootApplication`, and no interface has two implementations.
    - `java-spring-jpa`'s DI graph is `AuthController(AuthService)`, `DefaultAuthService(UserRepository)` (`@Service`), and `UserRepository extends JpaRepository<User, Long>`.
    - polyglot `AuthController(MfaClient)` with `@FeignClient`; `UserDao` has no stereotype.
    - The only lockfile is `ts-express-prisma/package-lock.json` (v3: express 4.22.3 at L253, `@prisma/client` 5.22.0 at L16, pg 8.23.0 at L587).
    - `apps/web/package.json` depends on `@acme/shared` with `*`.
    - `services/billing/go.mod` requires gin `v1.10.0`.
    - `java-spring-jpa/pom.xml` and polyglot `services/auth/pom.xml` have parent-managed versions only.
17. **e2e harness:**
    - `check_fixture` (`rb-cli/tests/e2e.rs:173`): determinism, store round trip, guards, HTTP invariants, AGE parity.
    - `parse_guards` (`:404`) and `http_invariant_failures` (`:491`).
    - `precision_guards_assert_every_http_edge_kind` (`:571`).
18. `services.txt` gives `.` (`acme`, library), `apps/web` (service), `packages/shared` (library), `services/auth` (service), `services/billing` (library), `services/billing/cmd/billing` (service), `services/broken` (library) and `services/notification` (service). D8's classpath and S12/S13's deployable rule rest on these roles.

**Not verified (checked in phase 0, before the step that needs it):**
- the Spring rules recalled in D9;
- Spring Data's fragment naming (`<Fragment>Impl`, postfix `Impl`);
- that `@EnableFeignClients` without attributes scans its own package;
- Spring Boot's JPA repository base package (the auto-configuration package);
- Lombok's generated-constructor field rules;
- `Introspector.decapitalize`;
- the npm v2/v3 lockfile `packages` layout for workspaces;
- the public candidates' current layout;
- whether the `.m2` cache holds what the new fixture classes need for scip-java.

## Forks for the orchestrator

The contract proceeds with each recommendation unless told otherwise.

- **F1: are service-to-service `DEPENDS_ON` `via=calls|http` stored or derived?** Recommendation: **derived at query time; `via=package` stored** (D6).
  - The alternative stores them as edges with a new provenance (`derived`), one per (pair, via).
  - That adds a second source of truth and a provenance value, and either duplicates every crossing edge's evidence or breaks the "every edge has evidence" check.
- **F2: may `INJECTS` reach `synthetic` at all?** Recommendation: **yes, under S5/S6's allow-listed gates and only through SCIP-resolved types** (D2), so `--no-scip` dumps hold `heuristic` at most.
  - The alternative caps every `INJECTS` at `heuristic`. That removes the adversarial surface entirely and shortens review, at the cost of a certainty label that never distinguishes a plain single-bean injection from a guess.
  - If the orchestrator expects a long adversarial review, the cap is the cheaper choice. S6 then remains as a `capped_by` explanation only.
- **F3: gin groups passed as parameters (M4a open item 2): M4b or M6?** Recommendation: **M6** (Scope).
- **F4: orphan eligibility by a whole-word mention scan of every file (S12 f)?** Recommendation: **yes.**
  - It is the one check that sees unresolved calls, reflective strings, templates and comments without a new fact per form. It can only over-protect.
  - The alternative (inbound edges only) lists as orphans every symbol whose callers Tier B left unresolved. M3a measured resolved shares of 15–63%, so that would be a false-orphan generator.
  - Cost: fewer orphans for common names (`get`, `run`), measured in S27.
- **F5: Spring Data's synthetic implementation: a bean of the interface type (D3) or a synthetic `Symbol` per repository with `IMPLEMENTS` to the interface?** Recommendation: **D3.** A synthetic node is code nobody wrote, and `IMPLEMENTS` from it would make every repository method a dispatch target in `rb callers`. That duplicates what the interface method already answers, adds no question's answer, and churns every dump with Spring Data.
- **F6: evaluate active profiles from `application.properties` (`spring.profiles.active`)?** Recommendation: **no; M6 with YAML and profile files** (M4a open item 3). Profiles are usually chosen at deploy time. Treating every conditional as `ambiguous` is the honest reading.

## Lessons applied

| Lesson | Where |
|---|---|
| State the soundness bar before review; close on verification | "Soundness claims and closing bar"; S28 |
| A deny-list of forms is never complete; allow-list roles | S6 gates D and M, S9 (c)–(e), S12 (c), (f) |
| Gate what carries the effect | S5's gates cover consumer, candidates and eliminated candidates; S9 (i) subtypes |
| A guarantee over "the repository" names the files it reads | S6/S9 use parsed facts plus M4a's raw-text selection; S12 scans every non-binary file |
| Measure a conservative fallback on the fixtures before trusting its cost | S27 records the share kept per rule; phase 0 runs the gates' matchers over the fixtures |
| A rule that suppresses a fallback counts what it suppressed | `rta_coverage`, `di_coverage` buckets and gates, `orphan_coverage`, `pruned_by_rta` |
| Termination claims need cycle tests, indirect edges included | invariant 8; S23 |
| Complexity claims: counted operations | invariant 9; S23 |
| A rule that walks a relation is tested past depth one | S4, S9 |
| Simple-name resolution is never exact | D2 (Tier B → `heuristic`); S9's name-based rules only keep types instantiated |
| Bare short names need import facts | invariant 5; S2, S11 |
| A relationship flag does not carry direction | S9 (i) propagates instantiation subtype → supertype only, tested both ways |
| Positions in tests come from the source | S1 |
| Never attribute a metric change without diffing | S24, S28 |
| A gap list says which weaker false results remain | the residue column above; S27 records false `heuristic` `INJECTS` |
| `skipping:` is not a pass; a mutation run that printed nothing has not run | S24, S25 |
| Name a counter by a positive property; identify one member per bucket | S7, S9, S27 |

## Checked in phase 0 (2026-10-06)

Environment: `df` 10 GiB free; `docker context show` = `desktop-linux`; `rb-scip-java:0.13.1` (`sha256:cbd99634…`, 0.93 GB) and `sourcegraph/scip-typescript@sha256:028c0042…` (1.63 GB) present. The `.m2` cache (`var/scip/cache/m2`) holds `spring-context-6.1.13.jar` and `spring-boot-autoconfigure-3.3.4.jar`, the versions the fixtures' Boot 3.3.4 parent selects, so the new fixture classes need no fetch.

**Primary sources read for the "not verified" list:**

| Item | Finding | Source |
|---|---|---|
| Implicit single constructor | Confirmed: "If a class only declares a single constructor to begin with, it will always be used, even if not annotated." Several constructors and none `@Autowired`: the primary/default constructor is used, so modelling it as `constructor_ambiguous` (no points) never adds an edge. | docs.spring.io, Framework reference, *Using `@Autowired`* |
| Collections, arrays, `Map<String, T>` | Confirmed (`T[]`, `Set`/`List`/`Collection<T>`, `Map` only with `String` keys). | same page; `DefaultListableBeanFactory.resolveMultipleBeans` |
| `Optional`, `ObjectProvider`/`ObjectFactory`, `jakarta.inject.Provider` | Confirmed as wrappers resolved like `T` (`DefaultListableBeanFactory.resolveDependency`). `ObjectFactory<T>` is added to D9's wrapper list (same branch as `ObjectProvider`). | spring-framework `main`, `DefaultListableBeanFactory.java` |
| Narrowing order | `determineAutowireCandidate`: primary, then the dependency name against bean names (and aliases), then `@Priority`, then a unique default candidate. `@Qualifier` filters the type-matched candidates before this. D9's order (qualifier, primary, name) holds. Name fallback for parameters needs `-parameters` since 6.1: confirmed. `@Priority` (`jakarta.annotation`) is not modelled: such points stay `ambiguous` with every candidate listed, never a wrong single edge. `@Fallback` (6.2) is in `org.springframework.context.annotation` and outside gate D's plain set. | *Fine-tuning Annotation-based Autowiring with Qualifiers*; `DefaultListableBeanFactory.determineAutowireCandidate` |
| `@Resource` | Confirmed: `name` attribute, else the field name or the setter's property name; only without an explicit name it falls back to a type match. | *Injection with `@Resource`* |
| `Introspector.decapitalize` | Confirmed: "FooBah" → "fooBah", "X" → "x", "URL" stays "URL" (first two upper-case: unchanged). | Java SE 21 API, `java.beans.Introspector` |
| Spring Data fragments | Confirmed: implementation = fragment interface name + postfix `Impl` (configurable by `repositoryImplementationPostfix`, which S2's attribute rule turns into `scan_unread`), found below the repository's package. D3's name-based rule ("implements, by name, an interface a repository extends") is broader than the convention and only keeps more types instantiated. | Spring Data Commons reference, *Custom Repository Implementations* |
| `@EnableFeignClients` default | Confirmed from `FeignClientsRegistrar.getBasePackages`: `value`, `basePackages`, `basePackageClasses`; when all are empty, the package of the annotated class. A `clients` attribute replaces scanning; it is outside S2's attribute set, so it sets `scan_unread`. | spring-cloud-openfeign `main`, `FeignClientsRegistrar.java` |
| Boot's JPA repository base | Confirmed: "the auto-configuration packages are searched for repositories", i.e. the package of the `@SpringBootApplication`/`@EnableAutoConfiguration` class; `@EnableJpaRepositories` overrides. | Spring Boot reference, *SQL Databases*, Spring Data JPA Repositories |
| Lombok constructors | Field rules confirmed (`@RequiredArgsConstructor`: non-initialised `final` fields and non-initialised `@NonNull` fields; `@AllArgsConstructor`: every field; static fields skipped; declaration order). **Deviation:** "the existence of an explicit constructor does not stop these annotations from generating their own constructor" (amendment 1). | projectlombok.org, *@NoArgsConstructor, @RequiredArgsConstructor, @AllArgsConstructor* |
| npm lockfile v2/v3 | Confirmed: `packages` keys are paths relative to the lockfile's root (`""`, `node_modules/x`, `node_modules/a/node_modules/b`, workspace folders); a workspace link entry is `{ "resolved": "<folder>", "link": true }` with no other field; v1 (npm 5/6) has no `packages`. | docs.npmjs.com, *package-lock.json* (v10) |

**Public candidates** (cloned under `var/m4b/`, pinned commits checked out and read):

| Candidate | Pin (full) | Layout read |
|---|---|---|
| spring-petclinic | `818c4136ea971c21674525f9053de0d9c7ad8cfe` | 2 `@SpringBootApplication` (one in tests), 18 `@Autowired`, 4 `@Bean`, 1 `@Profile`; 3 Spring Data repositories. As planned. |
| spring-petclinic-rest | `afc8fc1d3b8ec8de69414482b3bd70a1da78cb95` | 29 `@Profile`, 7 `@Mapper`, 28 `@Autowired`. As planned. |
| piggymetrics | `6bb2cf9ddbca980b664d3edbb6ff775d75369278` | 3 `@EnableFeignClients`, 4 `@FeignClient`, 4 `CrudRepository` repositories on MongoDB, 1 `@Qualifier`. As planned. |
| spring-petclinic-microservices | `dc9ca15a71189bc4be6c02518d7180232c0555b7` | 8 `@SpringBootApplication`, 1 `@Primary`, 15 `@Bean`. **No Lombok** at this pin (0 `lombok.` imports): D9's Lombok rule cannot be measured here; it stays covered by units. A substitution is decided in phase 15 and recorded there. |
| routing-controllers | `bf6fbfdc7fa5d4cd7d72eca8e5846b888ea58572` | TS library with `sample/` and `test/`. As planned. |
| node-express-realworld-example-app | `30b68e1e881462b2f4164ea09ab4c4f5699c7b0b` | `package-lock.json` at the root. As planned. |
| golang-gin-realworld-example-app | `626c372d259472148d93303f74aa9b9a1cdcef24` | `go.mod` at the root. As planned. |

**Invariant-1 baseline.** The binary of `1f81c8f` (HEAD before M4b; code identical to `28e47ba`, M4a's closing binary) was built and kept as `var/m4b/bin/rb-1f81c8f`; the three fixtures were dumped `--no-scip` from their `git ls-files` copies (`var/m4b/base/*.jsonl`; polyglot's is byte-identical to its golden `dump.jsonl`).

**Gate matchers run as a throwaway script** (`var/m4b/scripts/gates.py`, S6 gate D's import rule with exact package match, XML/factories files, the raw-text word list on unparsed files; S9 (g)'s surface), every hit read:
- fixtures: 0 hits for gate D and S9 (g).
- spring-petclinic: gate D: `org.springframework.context.annotation.ImportRuntimeHints` in `PetClinicApplication.java` (main; native-image hints, no bean definition, but outside the plain set: every point there is capped by `surface`, a recorded cost); `FilterType` and `ApplicationListener` in test sources only (amendment 3). S9 (g): `includeFilters` in `src/test/.../PetControllerTests.java` (`@WebMvcTest(includeFilters = …)`): (g) is repository-wide, so petclinic proves no type (recorded cost; a test context does instantiate main types).
- spring-petclinic-rest, piggymetrics, spring-petclinic-microservices: 0 gate D hits on exact packages (`org.springframework.boot.autoconfigure.condition.*`, `.boot.test.*`, `.boot.context.*` are other packages and not gated); 0 S9 (g) hits.
- Raw-text words on unparsed files: 0 hits in all seven repositories and the fixtures. Unparsed non-inert text files: 3, 19 and 4 in the fixtures, 34–95 in the Java repositories.

## Amendments

1. **Lombok constructors with a declared constructor (phase 0, D9/S3).** Lombok generates its constructor even when the class declares one. So a class with `@RequiredArgsConstructor`/`@AllArgsConstructor` and a declared constructor has two or more constructors: Spring then uses the one marked `@Autowired` (a declared one, or the generated one through `onConstructor_ = @__(@Autowired)`/`onConstructor = @__({@Autowired})`), else none (`constructor_ambiguous`). S3's row "a declared constructor present (no generation)" becomes "a declared constructor present: two constructors, none marked, no points".
2. **`INJECTS` coverage buckets under F2.** With `INJECTS` capped at `heuristic`, S7's `synthetic` and `heuristic` buckets become `plain` (one candidate, or a collection's candidates, reached without name fallback, with no conditional and no failed gate: S5's former `synthetic` row) and `capped` (one candidate otherwise). Every edge carries `resolution` (`unique`, `qualifier`, `primary` or `ambiguous`) and `capped_by` (the sorted failed gates, present only when non-empty). The status line reads `di: <p> injection points: <a> plain, <b> capped, <c> ambiguous, …`. Certainty: `heuristic` for one candidate or a collection, `ambiguous` for several; nothing is `synthetic`.
3. **Gate D reads main sources only.** Test sources are never on the main runtime classpath (Maven packages `src/main` only), and invariant 6 already restricts DI to main sources, so a post-processor or registrar in a test configuration cannot change a main-source injection. Measured: spring-petclinic's test-only `FilterType` and `ApplicationListener` imports would otherwise cap every point. S9 (g) stays repository-wide, since a test context does instantiate main types.

## Orchestrator decisions on the forks (2026-10-06)

- **F2: `INJECTS` is capped at `heuristic`; it never reaches `synthetic`.** The narrowing outcome is kept as data instead: every `INJECTS` edge carries `resolution` = `unique` | `qualifier` | `primary` | `ambiguous` (plus `capped_by` as specified), so a single-bean injection stays distinguishable from a guess without a soundness claim on runtime wiring. Gates D and M still decide `resolution` and coverage buckets, but no gate raises certainty. Wherever this contract states the `synthetic` row for `INJECTS` (S5, S25, the soundness section), read it as "`heuristic` with `resolution = unique|qualifier|primary`". Reason: M4a needed eight evaluator passes to defend `synthetic` on HTTP carriers against constructed inputs; the same surface (profiles, auto-configuration, post-processors) exists for DI, and the label adds little the `resolution` attribute does not.
- **F1, F3, F4, F5, F6: as recommended** (service dependencies `via=calls`/`via=http` at query time and `via=package` stored; gin parameter groups to M6; whole-word mention scan for orphan eligibility; Spring Data repositories as beans of their interface type; active profiles not evaluated, M6).
