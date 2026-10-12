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
- **D4: fixture plan is code (F4).** Three new fixtures (`java-guice`, `ts-nest`,
  `spring-xml`) + two extensions (`kotlin-beans` into `kotlin-gradle`, `dotnet` into
  `csharp`); exact file lists amended in phase 0; polyglot untouched.

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

(none yet)
