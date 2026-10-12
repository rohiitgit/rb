# M6j contract: DI breadth (draft, 2026-10-12)

**Status:** draft written 2026-10-12 at the M6i close (`d89e2f2`); forks F1–F4 decided below,
F5 deferred to phase 4 with options; phase 0 not started.

Written before any code. M6j is done only when every assertion below is backed by a passing
test or a recorded measurement. "Verified facts" are what the 2026-10-12 probes read in the
tree at the M6i close (`d89e2f2`).

This contract follows the M6e–M6i format and applies the lessons M3b–M6i paid for (soundness
bar + closing bar up front; allow-lists; suppressions counted; growth by counted ops; no
hand-counted positions; invariant 1 vs previous close with classified rows; fixture plan is
code; config-surface probing; no builds during container e2e; non-empty dumps; one file one
edit per block).

## Soundness claims and closing bar (read first)

M6j's outputs whose failure would be silent: a bean attributed to the wrong definition (a
pack allow-list that does not mean what its name says); an injection edge to the wrong bean
(a qualifier rule that does not hold); an XML bean with the wrong class (an id/class split
that misreads); a taint gate that stops tainting (gate D going blind on a new form); a
decorator read from the wrong node (a Kotlin/C# attachment rule that does not hold).
**Everything not proven takes the safe direction:** unread beans stay taint (gate D never
loses a finding); unresolvable injections stay `unresolved`, never edged; new packs never
change old packs' edges (fingerprinted); XML that fails to parse taints the file (never a
panic, per M1b C8); .NET stays `heuristic` at most (M4b precedent, no scip-dotnet).

| Surface | Claim (exactly this, no more) | Mechanism | Residue (out of scope by construction) |
|---|---|---|---|
| Decorations (S1) | Kotlin annotations + C# attributes (+ their args) into `Decoration` facts | `rb-syntax` decorations extended; stock node shapes first (M6i D1) | Modifiers beyond what packs read; nested-attribute exotic args |
| Guice/Dagger (S2) | `@Inject` points, `@Singleton`/`@Provides`/`@Binds` beans, Guice `bind().to()` links | M4b DI machinery extended; per-framework allow-lists as units | AssistedInject, custom scopes beyond the list; annotation-processed code (Dagger generates it — Tier B reads source only) |
| NestJS (S3) | `@Module`/`@Injectable`/`@Controller`/`@Inject` wiring through TS decorators | Existing TS decoration machinery + packs | Dynamic modules (`forRoot` returns), request-scoped subtleties |
| Spring XML (S4) | `<bean id class>` + `context:component-scan` + aliases into bean facts; gate-D XML findings retire where read | `parse_xml`/`Element` reuse (M6f MyBatis precedent); reader units | AOP/TX/MVC namespaces (no graph effect); parent-bean merge subtleties; `spring.factories` stays taint |
| Kotlin beans (S5) | `beans { }` DSL registrations read as bean facts | New small reader over M6i Kotlin facts; recorded forms only | Conditional DSL (`if`/`env`), profiles beyond literals |
| .NET (S6) | `IServiceCollection` registrations + constructor injection over M6i C# facts, Tier-B-only | Packs over Tier B edges; `heuristic` max | Exact edges (no scip-dotnet, beyond M6); open generics beyond the list |
| Fixtures + guards (S7) | New/extended fixtures with per-class guards, each biting | Fixture plan is code; e2e fns + goldens | — |
| Public repos (S8) | Sampled beans/injections correct; 0 false `INJECTS` in samples | Measurement plan below | New forms recorded for their owners |

**What M6j must not change.** Existing facts byte-identical (fingerprint + goldens); no old
pack's edge moves (S6 gate-D findings only shrink where a form becomes read, each counted).
`PIPELINE` becomes `m6j-1` at the first dump change.

This is invariant 1, checked against the M6i close dumps of every fixture (with and without
SCIP) and of the M6j measurement repositories at their pins, every differing row classified
by script (dumps asserted non-empty first).

**Closing bar.** Decided now:
1. **Evaluator pass 1 is a full review,** adversarial probing included (allow-list
   confusion, qualifier games, XML misreads, decorator misattachment, taint gaps).
2. **Evaluator pass 2 is verification-only.**
3. **Blocks closing only:** (a) a pass-1 fix that does not hold; (b) a regression; (c) a
   false bean/injection through a controlled form; (d) non-determinism; (e) a failed
   test/guard/step; (f) an AGE parity mismatch on any fixture.
4. **Not blocking:** a new form in a residue class (recorded); a precision miss contradicting
   no rule; §timing (measured, not asserted).
5. **Timings are measured, not asserted.**

## Scope

**In (M6j):** the 8 items above (design §13 M6j row; contract-m6i scope-out; contract-m4b
items 4-part, 9-part, 12-part).

**Out of M6j (each assigned):** exact .NET edges → beyond M6 (scip-dotnet 1,253 MB);
AOP/TX/MVC XML namespaces → dropped (no graph effect); `spring.factories` reading →
residue (stays taint); Dagger generated code → Tier-B-source-only (residue); dynamic
NestJS modules → M6j residue; Groovy → dropped (M6i).

## Engineer declaration

**Canonical state:** `Decoration` facts for Kotlin/C#; per-file DI packs for the six forms;
XML bean facts beside `Content.config` (F5 decides the node shape); per-blob `FileFacts`
unchanged in shape.
**Derived:** bean nodes (existing symbols where the code defines them), `INJECTS` edges
(`heuristic` at most outside Spring-annotation parity, per M4b caps), gate-D findings.
**Invariants:**
1. **Dumps.** `--no-scip` and SCIP dumps differ from the M6i close only in classified rows;
   nothing unclassified; every compared dump asserted non-empty.
2. **Old packs frozen.** Every existing pack's edges byte-identical on fixtures (goldens);
   gate-D findings shrink only where a form becomes read (each counted).
3. **Taint never lost.** Every gate-D finding of the M6i close either persists or is replaced
   by a read fact with a counted retirement row; `surface_text` gains findings, never loses
   one, on the measurement repos.
4. **Egress closure.** New fetches only through the existing allow-list (each probed first).
5. **Determinism.** Fresh == cached, two names, forced rebuilds on fixtures and measurement
   repos.
6. **Bounded work.** New walks carry visited sets + caps; growth ≤ 5× ops at 4× input.
7. **Stated complexity by counted operations** for each new walk.

## Decisions (made here, with reasons)

- **D1: packs reuse M4b (F1).** Allow-lists, gates, buckets, `INJECTS` caps — new forms are
  rows in existing machinery, not new machinery (Go-precedent rule from M6i D2).
- **D2: stock node shapes first (F2).** Kotlin `annotation`, C# `attribute_list`/`attribute`
  (verified below) are read before any hand-written attachment rule, and every rule comes
  from a measured gap (M6i D1 precedent).
- **D3: XML reuses `parse_xml` (F3).** The `Element` model + MyBatis reader are the
  precedent; Spring `<beans>` is a second reader over the same model, not a new parser.
- **D4: fixture plan is code (F4).** Four new fixtures (`java-guice`, `java-dagger`,
  `ts-nest`, `spring-xml`) + two extensions (`kotlin-beans` into `kotlin-gradle`, `dotnet`
  into `csharp`); exact file lists amended per phase; polyglot untouched. (Amended in
  phase 2: Dagger gets its own fixture — honest names over one mixed fixture.)

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **Decorations.** Kotlin annotations + C# attributes (names + literal args) equal hand-written expectations per shape. | unit per shape; mutation |
| S2 | **Guice/Dagger.** Points, beans, `bind().to()` shapes; no cross-framework leakage. | unit; mutation; e2e goldens |
| S3 | **NestJS.** Module/injectable/controller wiring through TS decorators. | unit; mutation; e2e goldens |
| S4 | **Spring XML.** `<bean>`/component-scan/aliases read; gate-D XML findings retire counted. | unit; mutation; e2e goldens |
| S5 | **Kotlin beans.** `beans { }` registrations read as bean facts. | unit; mutation; e2e goldens |
| S6 | **.NET.** Registrations + ctor injection over Tier B; `heuristic` max. | unit; mutation; e2e goldens |
| S7 | **Fixtures + guards.** Per-class must-exist/must-not-exist, each biting. | e2e with containers; goldens |
| S8 | **Public repos.** Classified diffs + samples; 0 false `INJECTS`. | recorded measurement |
| S9 | **Determinism + invariant 1.** Fresh == cached, two names, forced rebuilds; dumps vs M6i close classified. | e2e; recorded |
| S10 | **Growth by counted operations** (invariants 6–7). | unit |
| S11 | **Verification loop + closing.** `var/m6j/scripts/verify.sh`: fmt, clippy, counted units, store/server with DB, e2e + api with containers, listener + stub checks. | verification loop |

## Verified facts (read at the M6i close commit `d89e2f2`, before any code)

1. DI machinery: `crates/rb-frameworks/src/di.rs` (2,181 lines) — Spring packs + S6 gate D
   (`surface_text`: beans-XML namespace, `spring.factories`, `beans.xml`, `RAW_WORDS`
   incl. Component/Service/Repository/Configuration/Bean; `annotation_or_call` is
   Java-shaped `@word`/`word(`).
2. Decoration gaps: `decorations.rs:55` and `:633` return empty for Kotlin/C# ("lands with
   Tier B" — never landed); Java/Python/TS arms cover the other three forms.
3. XML machinery: `packages.rs:1236 parse_xml` + `Element` model; `sql.rs` MyBatis reader
   precedent (root-tag dispatch, attrs/ends, dynamics counted).
4. TS decorator extraction covers NestJS shapes (same `decorator` field machinery M4a
   uses); Guice/Dagger ride Java annotations (same).
5. M6i C# facts available for .NET packs (types, usings, supertypes, Tier B calls);
   M6i Kotlin facts available for the beans DSL (top-level functions, calls).
6. Grammar shapes (cargo registry, before any code): `-ng` has `annotation`
   (+ `modifiers` on class/object/function decls); `c-sharp` has `attribute_list` →
   `attribute` (with `name` field) + `modifier`. Attachment rules (which siblings carry
   them) are NOT verified — phase 1 discovers them by parsing, M6i-D1-style.
7. Baseline HEAD is `d89e2f2` (this commit); baseline binary + pins land in phase 0.

**Not verified (checked in preflight, before the step that needs it):** decorator
attachment shapes in the wild; import/binding interplay for the new packs; XML bean-id
conventions in real repos; `beans { }` DSL surface breadth; .NET registration idioms
beyond `IServiceCollection`.

## Fixture plan

- `fixtures/java-guice/`: module `bind().to()`, `@Inject` ctors, `@Singleton`/`@Provides`.
- `fixtures/ts-nest/`: `@Module` + providers/controllers, `@Injectable`, `@Inject` token.
- `fixtures/spring-xml/`: `<bean>` + `context:component-scan` + alias (+ a Kotlin-beans
  file if the Gradle shape stays cheap, else it moves to `kotlin-gradle/`).
- `fixtures/kotlin-gradle/` + `fixtures/csharp/` extended (beans DSL; `IServiceCollection`
  + ctor injection). No changes to other fixtures.

## Public-repository measurement plan

| Step | Repository @ pin | Runs | Notes |
|---|---|---|---|
| Phase 0 | — | M6i close binary dumps (`--no-scip`); e2e green baseline | small |
| Phase 8 | Guice or Dagger repo @ pin (chosen preflight) | `--no-scip` both binaries | classified diffs, samples |
| Phase 8 | nest repo @ pin (chosen preflight) | `--no-scip` both binaries | classified diffs, samples |
| Phase 8 | Spring-XML-heavy + Kotlin-beans repos @ pins | `--no-scip` both binaries | classified diffs, samples |
| Phase 8 | .NET DI repo @ pin (chosen preflight) | `--no-scip` both binaries | classified diffs, samples |

Candidates (pins by `git ls-remote` in phase 0, never clone-first): google/guice,
google/dagger, nestjs/nest, a Spring-XML sample, a `beans { }` sample, a .NET repo with
`IServiceCollection` use (automapper's M6i pin has DI tests and may serve).

Disk policy as M6b D9 (6 GiB floor, one repo at a time, remove + `VACUUM`, part-by-part figures).

## Determinism

No wall-clock/host-path/run-order in dumps (unchanged rules). Fresh == cached, two names,
forced rebuilds on fixtures (e2e); `--no-scip` both binaries byte-identical modulo
classified rows.

## Verification loop

`var/m6j/scripts/verify.sh <tag>` (copied from M6i's, M6j paths/logs): fmt, clippy `-D
warnings`, counted units, `rb-store`/`rb-server` with DB, e2e + api with containers,
listener + stub checks, mutation runs per M4a harness rules.

## Forks (decided here, with reasons)

- **F1: packs reuse M4b.** No new edge kinds, no new caps above `heuristic` (Spring-annotation
  parity where the rule already earns it).
- **F2: stock node shapes first.** Hand-written attachment only from measured gaps.
- **F3: XML reuses `parse_xml`.** Second reader, not a second parser.
- **F4: fixture plan is code.** Three new fixtures + two extensions; polyglot untouched.
- **F5 (deferred to phase 4): XML bean identity.** Options: (a) Symbol nodes keyed
  `xml:<file>#<id>` with a class link (MyBatis-fact precedent); (b) edges only where a
  code symbol resolves, ids otherwise taint. Decided when the reader's shapes are measured.

## Amendments

1. **(phase 0) Baselines, pins and shape checks.** Baseline binary `var/m6j/base/rb-m6iclose`
(release, sha256 `23723d89…`; rebuilt from HEAD — 0.48s no-op, so HEAD code == the M6i close
tree: the contract commit adds no code). Grammar shapes (cargo registry, before any code):
`-ng` has `annotation` (+ `modifiers` on class/object/function decls), `c-sharp` has
`attribute_list` → `attribute` (with `name` field) + `modifier`; attachment rules NOT
verified (phase 1 discovers them by parsing, M6i-D1-style). Measurement pins verified by
`git ls-remote` (full SHAs): google/guice `b8cab7e4e947adb82e343586c267c5c367cf5cf1`
(HEAD; `bind().to()` leg), google/dagger `5d4ee6ba…` (HEAD; backup), nestjs/nest
`45146232…` (HEAD), mybatis/jpetstore-6 `344ef63d…` (HEAD; Spring-XML leg), spring-fu
`8f8775ec…` (HEAD; Kotlin-beans leg), AutoMapper/AutoMapper `6e8697bc…` (M6i pin reused
for the .NET leg — it carries DI tests). Sizes accepted per M5 precedent (parse-only
`--no-scip` handled 1.7M lines; one repo at a time, remove + `VACUUM`). Baseline verify
`var/m6j/logs/p0-baseline-*` green (fmt, clippy `-D warnings`, 888 units, store 72, server
12, e2e 42/42, api 1/1, no skips, no listener, stub scan clean). DB holds only the 3
pre-existing `fx_*` repos.

2. **(phase 1) Kotlin/C# decorations.** Annotations ride `modifiers`, attributes ride
`attribute_list` (both Java-shaped, proven by S-exps); names (full paths) + literal args
(keyed and positional) with a `value_argument`/`attribute_argument` arm each; string
literals decode per language with `interpolation` holes plus a `dollar_interpolation`
flag so bare `$name` never reads literal (C# verbatim stays a raw-text miss; use-site
targets keep the name; `@file:` annotations correctly attach to nothing). Bindings carry
decorations for Kotlin/C# (lib.rs gate); `class_parameter` binds (primary-ctor injection
points). C# properties stay out (residue: S6 reads fields + ctor params). Tests-first (5
units); 12/12 mutants killed — including a harness repair (uncompilable `Language::XXX`
mutants give no verdict per the M6i lesson; non-unique REV patterns leave mutants in the
tree — both bitten during the run and fixed with string-only mutants, unique both
directions, plus an XXX sweep).

3. **(phase 2) Guice/Dagger packs.** Import-gated packs over the M4b machinery (certainty
caps at heuristic by construction): Guice `@Inject` points (field/ctor/setter),
`@Singleton` classes → `singleton` beans, `@Provides` → `bean_method` reuse in module
hosts (`@Module` or `*Module` supertype), `@Named` (guice package) qualifiers,
`bind(X).to(Y)` → `binds` beans (typed X, impl in `created`, qualifier joined from the
nested `named()` chain by span; unresolvable qualifiers and foreign middles skip the
pair); Dagger `@Module`/`@Component` understood but not beans, `@Binds` → `binds` beans,
`@Provides` → `bean_method`, jakarta `@Inject`/`@Singleton` points; injectable classes
(`@Inject` ctor) become `injectable` beans only with a Guice/Dagger file marker, so
Spring files read byte-identically. Engine: `Binds` resolves its bound type like
`BeanMethod` (created-fallback included); new origins eligible like stereotypes. The
`bind()`-side class literals join as M5-S19(g) registrants — `ambiguous` beside the binds
bean, every candidate genuinely related. `Arg::Type` decodes Java class literals (no
downstream consumer matches `Other` explicitly — proven by the unchanged frameworks
suite). Residue: impl-preference among multiples (ambiguous, honest); pure-jakarta
Dagger consumers without a dagger import; `Lazy`/`Provider` wrappers; Dagger generated
code. Fixtures `java-guice` + `java-dagger` with reviewed dump goldens and 8/5 guards;
every guard proven biting (G1–G4 code mutants, F1–F4 fixture perturbations). 12/12 pack
mutants + T1/T2 arg mutants killed. D4 amended: Dagger gets its own fixture (honest
names over one mixed fixture).

4. **(phase 3) NestJS.** TS binding decorations flow (the `decorator` field already
covered parameters — a redundant arm was added and removed); `@Injectable()`/
`@Controller()` classes → `injectable` beans; same-file `providers`/`controllers`
entries name undecorated beans (custom providers stay unread); constructor parameters
read as member bindings scoped to the class (TS ctors are not definitions — overloading
is residue); `@Inject(token)` qualifiers with token-literal decoding; a compact
`resolve_nest` lane (type match + qualifier-only token match, heuristic/ambiguous max,
shared buckets). A `DB`-named decoy bean pins that tokens never match auto-names.
Fixture `ts-nest` with reviewed dump golden and 3/3 guards. Tests-first; 6/6 pack
mutants killed (providers[] masked the decorator gates until the unit dropped its
`@Module`), lane bites proven end-to-end (exact-forcer, names-widening).

5. **(phase 4) Spring XML.** F5 decided: no new Symbol nodes — `<bean>` targets its
resolved class (FQ or simple-name match, nested-aware; 0/2+ matches skip), ids and
aliases ride names and qualifiers; refs become points owned by the consumer class with
XML evidence (element resolved through same-service ids, qualifier always the id).
Reader (`xml_beans.rs`) over the generic `Element` tree: beans/aliases/scans, abstract
and class-less beans skipped, value-only properties ignored, spans computed. Engine:
`Inputs.xml` (all `.xml` texts; 7 construction sites), XML beans merged into the shared
bean set (Java points match them too), XML scans union into roots (unknown stays
unknown; repositories follow defaults), read files retire their `beans namespace`
finding, `Origin::XmlBean` eligible ignoring scans. Fixture `spring-xml` (plain: 2
points, zero gates) with reviewed dump golden and 4/3 guards incl. qualifier attrs and
an alias-traversed edge. Tests-first (reader units); 7/7 reader mutants + X1–X5 e2e
bites (incl. an eligible-arm false-no-bite that exposed the lane bypassing per-origin
checks — fixed by mirroring the full match). Residue: parent-bean merge, nested
`<bean>`, `scope` modeling, `spring.factories`, unresolvable-id points (unresolved,
counted).

6. **(phase 5) Kotlin beans DSL.** `CallLink.type_args` carries Kotlin call-site type
arguments (dump-safe: chains never dump); `value_argument` decodes (names and named
args; existing Kotlin outlines get strictly more precise). Pack reads `bean` calls
inside `beans` spans (structural gate, no import FQNs needed): type-arg, constructed
callee (innermost-wins), or string name; top-level `val` forms without enclosing defs
stay unread; `ref()` points untyped (qualifier only from literals). Beans anchor at
their enclosing function as `Binds` (the configure-method precedent); the shared lane
covers Kotlin with zero engine changes (empty elements land unresolved, verified).
Fixture `kotlin-beans` (plain, no Gradle — spring-fu predates the 2.2.0 toolchain) with
reviewed dump golden and 4/3 guards incl. qualifier attrs. Tests-first; 8/8 mutants
killed (W1/W3 patterns fixed, W8 retargeted to the pack test that actually reads
values); K1 fixture + K2 lane-cap bites. D4 amended: beans get a plain fixture.
Residue: val-form top-level beans, nested-bean suppliers, `created` inference, ctor
signature inference for `ref()`, conditional DSL.

7. **(phase 6) .NET DI.** C# generic call args decode (`generic_name` spine walk, never
into arguments or nested invocations); file-scoped namespaces spell `module` (block
namespaces unchanged; sibling classes do not nest — residue); `Add/TryAdd
Singleton/Transient/Scoped` in imported files become `Binds` beans anchored at their
enclosing method (keyed/lambda/instance/`typeof`/top-level forms stay unread);
constructor parameters (non-member typed bindings scoped to a class) become points;
the lane resolves only points whose owner class is itself registered (unregistered
consumers stay silent). Two real gaps found by the fixture: C# namespaces never
entered the package map (same-namespace resolution was import-only), and the lane
built `by_type` from raw definitions instead of resolved bean types (extracted shared
`bean_start`, inferred gate mirrored). Fixture extends `csharp/` (mailer story renamed
off a real collision that killed old edges mid-phase); dump gains only additions.
Tests-first (pack + query + outline units); 5/5 mutants killed; D6/D7/V1/V2 e2e bites.
Residue: keyed services, factories, instances, `typeof`, top-level minimal hosting,
property injection, lifetimes, scopes.

8. **(phase 8) Public measurement.** Base-vs-head `--no-scip` on all five pins (head
`e6f7ddf5`, post-phase-6; `var/m6j/measure/*_{base,head}.jsonl`, dumps asserted
non-empty; one repo at a time + `VACUUM`):
- guice @ `b8cab7e4`: +40/−1. 24 added INJECTS (Guice-internal wirings) + 16 D; the one
  removal is the superseded coverage row. di_coverage: 292 test points untouched, 7
  ambiguous + 3 capped + 8 not_a_bean + 7 unresolved main points.
- nest @ `45146232`: +168/−1. 106 added INJECTS (controllers/pipes/services) + 62 D;
  one coverage swap. 184 points: 106 capped (scan_roots_unknown), 61 unresolved, 17
  test. Unresolved sampled: framework types only (`Reflector`, `DiscoveryService`,
  `HttpAdapterHost`, …) — honest misses.
- jpetstore @ `344ef63d`: +6/−6. The same 5 INJECTS edges with `scan_roots_unknown`
  REMOVED from `capped_by` (XML component-scan made roots known; buckets identical) +
  coverage swap. The designed scan effect, proven end-to-end.
- springfu @ `8f8775ec`: +39/−3. 33 added `QUERIES` + 3 tables (Kotlin string literals
  from S1 enable SQL candidates as a side effect) + coverage; 2 recounted diagnostics
  (constant totals); zero removed facts. No beans-DSL INJECTS anywhere: every real
  `beans { }` site is a top-level `val` form (confirmed in source at the pin), which
  stays unread per the S5 residue; `bean(::routes)` callable refs recorded as a new
  residue shape.
- automapper @ `6e8697bc` (M6i pin reused): +693/−134. 132 file-scoped-namespace
  `Module` defs (+132 DEFINED_IN) enabling 208 REFERENCES + 133 IMPLEMENTS + 59 CALLS
  in previously unresolving files; 131 recounted + 2 coverage diagnostics; exactly ONE
  removed edge, reborn as a twin with honest ambiguity (newly visible hierarchy adds a
  second candidate at certainty 1 — `MapperConfigurationExpression : Profile,
  IGlobalConfigurationExpression`, read in source).
- Samples read in source, all correct: JpaPersistService inner-class injection,
  ManagedFilterPipeline delegation, PersistFilter pair, HelloController, Nest
  self-injection, jpetstore `@Autowired` fields, AutoMap `ReverseMap()` call.
  0 false edges in samples. Unresolved buckets sample clean (assisted factories,
  custom binding annotations, framework types — all recorded residue).

9. **(S10 growth).** New walks counted: `bean_start` extracted shared (Java engine and
managed lane resolve `Binds` identically); class resolution indexed once per resolve
(replacing per-bean file scans after review); ticks on file scans, candidate loops and
lanes; `managed_lane_grows_linearly` + `xml_beans_grow_linearly` ratio tests green
(≤5× at 4×). No new unbounded walk anywhere (single-pass visitors, map lookups,
visited-set machinery reused).

10. **(pass 1) Full review, VERDICT PASS.** `tasks/eval-m6j-pass1.md`: H1 (`.in()`
tolerance), H2 (bind markers), H3 (nested innermost), H4 (file-scoped modules) —
all found during probing and in-tree; P5–P9 verified no-change (qualifiers, assisted
risk with no instance, dump-neutral decorations, no panics, determinism). 52/52 unit
mutants + all guard bites re-verified on the final tree.
