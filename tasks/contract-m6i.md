# M6i contract: Kotlin and C# syntax (draft, 2026-10-12)

**Status:** draft written 2026-10-12 at the M6h close (`8fa5a78`); forks F1–F4 decided below;
phase 0 not started.

Written before any code. M6i is done only when every assertion below is backed by a passing test or a
recorded measurement. "Verified facts" are what the 2026-10-12 probes read in the tree at the M6h
close (`8fa5a78`).

This contract follows the M6e–M6h format and applies the lessons M3b–M6h paid for (soundness bar +
closing bar up front; allow-lists; suppressions counted; growth by counted ops; no hand-counted
positions; invariant 1 vs previous close with classified rows; fixture plan is code; config-surface
probing; no builds during container e2e; non-empty dumps).

## Soundness claims and closing bar (read first)

M6i's outputs whose failure would be silent: a Kotlin/C# definition with the wrong parent or span
(a query that does not mean what its name says); a reference attributed to the wrong scope (Tier B
misresolution through a new import shape); an exact edge to the wrong symbol (a Kotlin descriptor
rule that does not hold); a new grammar crashing on real code (a panic in a query predicate).
**Everything not proven takes the safe direction:** files that fail to parse yield partial
definitions plus `syntax_error` (never a panic, per M1b C8); unresolvable references stay
references, never edges; new languages never change old languages' facts (fingerprinted); SCIP
owns a Kotlin document only under the unchanged agreement checks.

| Surface | Claim (exactly this, no more) | Mechanism | Residue (out of scope by construction) |
|---|---|---|---|
| Grammars (S1) | Kotlin + C# parse with pinned grammar crates; definitions/references per file into `FileFacts` | `rb-syntax` language registry + queries (stock `TAGS_QUERY` plus measured supplements) | New dependency only with stated reason; version pins recorded |
| Tier B (S2) | Imports, bindings, module + call/type/implements resolution for both languages | Existing M3a machinery extended; per-language shapes as units | Novel resolution rules beyond the measured shapes |
| Kotlin exact (S3) | scip-java indexes Kotlin through Gradle; join reuses the unchanged checks | M6e Gradle jobs + M3b agreement checks (new descriptor calibrations) | Maven Kotlin builds (unmeasured); Gradle builds that need network beyond the allow-list |
| Fixtures + guards (S4) | `kotlin-gradle` + `csharp` fixtures with per-class guards, each biting | Fixture plan is code; e2e fns + goldens | Executable C# builds (no dotnet here); Kotlin Maven |
| Public repos (S5) | Sampled definitions/references/edges correct; 0 false exact edges in samples | Measurement plan below | New forms recorded for their owners |

**What M6i must not change.** Existing six languages' facts byte-identical (fingerprint + goldens).
`PIPELINE` becomes `m6i-1` at the first dump change (new fixtures change nothing old).

This is invariant 1, checked against the M6h close dumps of every fixture (with and without SCIP)
and of the M6i measurement repositories at their pins, every differing row classified by script
(dumps asserted non-empty first).

**Closing bar.** Decided now:
1. **Evaluator pass 1 is a full review,** adversarial probing included (query confusion, scope
   smuggling, descriptor games, grammar panics on adversarial inputs).
2. **Evaluator pass 2 is verification-only.**
3. **Blocks closing only:** (a) a pass-1 fix that does not hold; (b) a regression; (c) a false
   definition/reference/edge through a controlled form; (d) non-determinism; (e) a failed
   test/guard/step; (f) an AGE parity mismatch on any fixture.
4. **Not blocking:** a new form in a residue class (recorded); a precision miss contradicting no
   rule; §timing (measured, not asserted).
5. **Timings are measured, not asserted.**

## Scope

**In (M6i):** the 5 items above (design §13 M6i row; contract-m5 14 Kotlin part).

**Out of M6i (each assigned):** Guice/Dagger/NestJS/Spring-XML/.NET/Kotlin-beans → M6j; scip-dotnet
→ beyond M6 (1,253 MB image); Groovy → dropped; Kotlin Maven builds → residue.

## Engineer declaration

**Canonical state:** two new grammar crates + their queries in the extractor fingerprint; per-blob
`FileFacts` for the new languages; Kotlin SCIP jobs through the existing Gradle path.
**Derived:** Symbol nodes, Tier B edges, Kotlin exact edges.
**Invariants:**
1. **Dumps.** `--no-scip` and SCIP dumps differ from the M6h close only in classified rows; nothing
   unclassified; every compared dump asserted non-empty.
2. **Old facts frozen.** Every existing language's `FileFacts` byte-identical (extractor
   fingerprint + fixture goldens).
3. **No new unmatched-exact risk.** Kotlin documents pass the unchanged agreement checks; any new
   calibration is itself calibrated to 0 on the Java repos.
4. **Egress closure.** New fetches only through the existing allow-list (Gradle plugin/distribution
   hosts as needed, each probed first).
5. **Determinism.** Fresh == cached, two names, forced rebuilds on fixtures and measurement repos.
6. **Bounded work.** New queries walk with the existing caps; growth ≤ 5× ops at 4× input.
7. **Stated complexity by counted operations** for each new walk.

## Decisions (made here, with reasons)

- **D1: stock queries first, supplements measured (F1).** Each grammar's `TAGS_QUERY` (when exported)
  runs before any hand-written addition, and every addition comes from a measured gap (M1b Java
  precedent: enum/record/constructor supplement).
- **D2: Tier B reuses M3a (F2).** Imports/bindings/resolution are language-parameterized already
  (Go precedent); per-language shapes land as units, not new machinery.
- **D3: Kotlin exact reuses the unchanged checks (F3).** scip-java emits Kotlin descriptors; the
  M3b agreement suite decides per document. New calibrations (if any) are additive rejections.
- **D4: fixture plan is code (F4).** `fixtures/kotlin-gradle/` (Gradle Kotlin Spring or plain
  Kotlin with a service boundary + routes/clients?) and `fixtures/csharp/` (plain classes,
  interfaces, calls); polyglot untouched.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **Grammars.** Kotlin + C# definitions/references per language fixture file equal hand-written expectations. | unit per language; mutation |
| S2 | **Tier B.** Imports/bindings/resolution shapes per language; no cross-language leakage. | unit; mutation; e2e goldens |
| S3 | **Kotlin exact.** Gradle scip-java indexes the fixture; exact edges sampled correct; agreement rejections calibrated. | e2e with containers; recorded |
| S4 | **Fixtures + guards.** Per-class must-exist/must-not-exist, each biting. | e2e with containers; goldens |
| S5 | **Public repos.** Classified diffs + samples; 0 false exact edges. | recorded measurement |
| S6 | **Determinism + invariant 1.** Fresh == cached, two names, forced rebuilds; dumps vs M6h close classified. | e2e; recorded |
| S7 | **Growth by counted operations** (invariants 5–6). | unit |
| S8 | **Verification loop + closing.** `var/m6i/scripts/verify.sh`: fmt, clippy, counted units, store/server with DB, e2e + api with containers, listener + stub checks. | verification loop |

## Verified facts (to be read at the M6h close commit, in phase 0)

1. Grammar crate candidates + versions + `TAGS_QUERY` exports (cargo registry, before any code).
2. The extractor fingerprint list (where new queries register).
3. scip-java's Kotlin support shape (descriptor samples; Gradle-only?).
4. Fixture buildability (Gradle wrapper + deps through the proxy; dotnet absent).
5. Measurement pins (`git ls-remote`, never clone-first).

**Not verified (checked in preflight, before the step that needs it):** per-language definition/reference shapes in the wild; import/binding forms; Tier B resolution behaviour on the new shapes; agreement-check behaviour on Kotlin documents.

## Fixture plan

- `fixtures/kotlin-gradle/`: Kotlin sources (classes, functions, calls, imports) + Gradle build; goldens: symbols, Tier B edges, SCIP exact edges where the build works.
- `fixtures/csharp/`: C# sources (classes, interfaces, calls); goldens: symbols, Tier B edges (no SCIP).
- No changes to existing fixtures.

## Public-repository measurement plan

| Step | Repository @ pin | Runs | Notes |
|---|---|---|---|
| Phase 0 | — | M6h close binary dumps (`--no-scip`); e2e green baseline | small |
| Phase 5 | Kotlin Gradle repo @ pin (chosen preflight) | `--no-scip` + SCIP both binaries | classified diffs, samples |
| Phase 5 | C# repo @ pin (chosen preflight) | `--no-scip` both binaries | classified diffs, samples |

Disk policy as M6b D9 (6 GiB floor, one repo at a time, remove + `VACUUM`, part-by-part figures).

## Determinism

No wall-clock/host-path/run-order in dumps (unchanged rules). Fresh == cached, two names, forced
rebuilds on fixtures (e2e); `--no-scip` both binaries byte-identical modulo classified rows.

## Verification loop

`var/m6i/scripts/verify.sh <tag>` (copied from M6h's, M6i paths/logs): fmt, clippy `-D warnings`,
counted units, `rb-store`/`rb-server` with DB, e2e + api with containers, listener + stub checks,
mutation runs per M4a harness rules.

## Forks (decided here, with reasons)

- **F1: stock queries first.** Supplements only from measured gaps.
- **F2: Tier B reuse.** No new resolution machinery.
- **F3: unchanged agreement checks.** Kotlin earns exact edges through the same bar.
- **F4: fixture plan is code.** Two new fixtures; polyglot untouched.

## Amendments

(none yet)

## Amendments

1. **(phase 0) Baselines, pins and shape checks.** Baseline binary `var/m6i/base/rb-m6hclose`
(release, sha256 `ad57c0f0…`; HEAD code == the M6h close tree — the contract commit adds no code).
Grammar crates (cargo registry, before any code): `tree-sitter-kotlin =0.3.8` (ABI 14, NO
`tags.scm` — the `TAGS_QUERY` line is commented out, so Kotlin needs a hand-written query) and
`tree-sitter-c-sharp =0.23.5` (ABI 15, `tags.scm` exported); runtime ABI load proven in phase 1
by load+parse units. scip-java 0.13.1 indexes "Java/Kotlin codebases" (local image help text) —
S3 stands. No dotnet/msbuild/kotlinc on this machine (C# Tier-B-only justified). Measurement
pins verified by `git ls-remote` (full SHAs): Kotlin/kotlinx.coroutines `113ee1f3c0b0ca0b7a809e75dd8d2340290121c6`
(HEAD; Gradle), AutoMapper/AutoMapper `6e8697bc44f02fb54ef6a556a7d134e63485299e` (HEAD).
Sizes accepted per M5 precedent (parse-only `--no-scip` handled 1.7M lines; one repo at a time,
remove + `VACUUM`). Baseline verify `var/m6i/logs/p0-baseline-*` running. DB holds only the 3
pre-existing `fx_*` repos.

2. **(phase 1) Grammars with definitions and references.** `tree-sitter-kotlin-ng =1.1.0`
(ABI 14, `LANGUAGE` fn-pointer constant — no unsafe needed; the 0.3.x line binds tree-sitter
0.20, whose `Language` is a different Rust type under the workspace's `unsafe-code = forbid`)
and `tree-sitter-c-sharp =0.23.5` (ABI 15, loads under runtime 0.27 — proven by the green C1
tests). Kotlin tags hand-written (interface/class/method/function anchoring with
first-pattern-wins order; bare + member calls); C# stock tags plus an invocation supplement
(stock `@reference.send` is unmapped). `Language::{Kotlin, CSharp}` wired end to end
(`language_for_path` already mapped `kt`/`cs`); Tier-B-adjacent matches default to marked
no-ops; `EXTRACTOR_VERSION` + pin test extended. Tests-first (2 C1 tests green after
hand-count corrections); old languages byte-identical (102 syntax units). Mutants: pending
phase-1 close battery.

3. **(phase 2) Tier B: imports through implements.** Imports (Kotlin Java-shaped + alias/wildcard;
C# plain/alias/static) + Kotlin `package`; bindings (Kotlin params/properties/locals with
`val`-constness and member detection; C# params/fields/locals with `var`-construction typing);
call chains (Kotlin positional + navigation, C# field-based); module resolution (Kotlin reuses
Java packages; C# namespace→file with rsplit walk); same-package keys (Kotlin Java-like, C#
top-level module); constructor dispatch stays `None` (calls to classes are References, Go
precedent); supertypes (Kotlin delegation roles + interface-keyword detection; C# first-extends
convention); member-call qualifiers (Kotlin positional navigation, C# `expression` field).
Resolve units prove cross-file calls, typed receivers, statics and implements for both
languages. Shared-denylist lesson: `BUILTIN_TYPES` must not contain Java reference-type
spellings (`String` et al. broke a Java Gate-A verdict — caught by the carriers e2e; kept only
Int/Char/Unit + decimal/dynamic/nint/nuint/sbyte/uint/ulong/ushort). Tests-first throughout;
12/12 mutants killed. Framework packs (routes/clients/ORM) stay out of M6i.

3. **(phase 2) Tier B: imports through implements.** Imports (Kotlin Java-shaped + alias/wildcard;
C# plain/alias/static) + Kotlin `package`; bindings (Kotlin params/properties/locals with
`val`-constness and member detection; C# params/fields/locals with `var`-construction typing);
call chains (Kotlin positional + navigation, C# field-based); module resolution (Kotlin reuses
Java packages; C# namespace→file with rsplit walk); same-package keys (Kotlin Java-like, C#
top-level module); constructor dispatch stays `None` (calls to classes are References, Go
precedent); supertypes (Kotlin delegation roles + interface-keyword detection; C# first-extends
convention); member-call qualifiers (Kotlin positional navigation, C# `expression` field).
Resolve units prove cross-file calls, typed receivers, statics and implements for both
languages. Shared-denylist lesson: `BUILTIN_TYPES` must not contain Java reference-type
spellings (`String` et al. broke a Java Gate-A verdict — caught by the carriers e2e; kept only
Int/Char/Unit + decimal/dynamic/nint/nuint/sbyte/uint/ulong/ushort). Framework packs
(routes/clients/ORM) stay out of M6i. Tests-first throughout; 12/12 mutants killed.
Fixtures `kotlin-gradle` + `csharp` with reviewed dump goldens and 11/4 biting guards (7
mutant runs, incl. implementation-preferred dispatch both languages).

4. **(phase 3) Kotlin exact edges.** The fixture must build with the Kotlin version scip-java
itself compiles against (2.2.0, read from its build file): 1.9.24 dies in compiler
environment setup, 2.0.21/2.1.21 die in version-mixed FIR checking, all inside the project's
own `compileKotlin` (Gradle detection + fetch worked throughout). With 2.2.0 the join needs
zero new calibrations — 6/6 definitions matched, 0 documents rejected, 3 exact `CALLS` + 2
exact `IMPLEMENTS` with the same targets Tier B found (asserted pair-wise in e2e).
`scip_kotlin_gradle` e2e: status/callers/dump goldens (all reviewed), cached + fresh +
second-name identical. The version-bite is proven thrice over (each wrong version fails the
test); no join mutants apply (no join code changed).

5. **(phase 5) Public measurement: coroutines + automapper classified and sampled.** Head binary
`target/release/rb` (sha256 `9bfa7510…`, built 16:41, postdates the phase-3 commit `0b1dd40`;
current HEAD is contract-only over it, so the binary is the current tree). The planned MEASURE2
rerun (`var/m6i/scripts/p5-measure.sh`, `*2_` names) was superseded and never ran: both legs
already ran on the post-rebuild binary (coroutines finished 16:41, automapper 16:47), and the
head dumps prove phase-2 code (21k/20k resolved Tier B edges with candidate counts — a phase-1
binary resolves none). Dumps: `var/m6i/measure/{coroutines,automapper}_{base,head}.jsonl`,
`--no-scip`, both binaries, asserted non-empty.
Coroutines @ `113ee1f3`: base 3,018 rows → head 33,509. Added 30,496: 8,162 N (every one a
`.kt`/`.kts` symbol), 21,299 E (every row touches a Kotlin endpoint), 1,035 D (972 + 42
`unresolved_references` for `.kt`/`.kts` files, 16 `syntax_error`, 3 updated java diagnostics,
2 coverage updates). Removed 5 D only (the 3 superseded java diagnostics + 2 coverage rows) —
zero removed N/E. Automapper @ `6e8697bc`: base 1,208 → head 31,445. Added 30,239: 9,245 N
(all `.cs`), 20,490 E (all touch C#), 504 D (500 unresolved + 2 `syntax_error` + 2 coverage).
Removed 2 coverage rows. Zero old-file changes on automapper.
Java→Kotlin interop (classified, intended — Kotlin reuses Java packages, amendment 2):
`RxJava2PlaysScrabble(.Opt).java` gain 7 edges to Kotlin (IMPLEMENTS + REFERENCES to
`ShakespearePlaysScrabble`, CALLS to `IterableSpliterator#of`, `play()` IMPLEMENTS), all read
in source at the pin (`abstract class` in Kotlin, `extends` in Java — IMPLEMENTS is the
established extends/implements convention per scip.rs). Consequence: external 51→47, types
2→1 on those files. `RunBlockingJavaTest.java` keeps 3 total unresolved with one
re-categorized external→calls (3→2, 0→1): its `kotlinx.coroutines` import now resolves to an
in-repo Kotlin file so `is_external` no longer fires, while the member itself stays unresolved
— safe direction, no new edge, recorded precision residue (statically-imported top-level
Kotlin fun shape).
`syntax_error` rows (16 coroutines + 2 automapper) are partial definitions plus the diagnostic,
never a panic (M1b C8); whole-file ranges (e.g. `Semaphore.kt` [0,0,395,0]) are grammar gaps
recorded for their owner, not blockers (closing bar 4).
Samples read in source at the pins, all correct: the Scrabble interop edges above;
`NumbersBenchmark.kt:24` `flow` → `FlowPlaysScrabbleOpt.kt/flow()` (the file imports
`benchmarks.flow.scrabble.flow`, a shadowing top-level `fun <T> flow` at line 184 — an
import-qualified Tier B hit, not the stdlib builder); `Profiles.cs:84/77` `FooService` /
`MutableService : ISomeService` + `Modify()` IMPLEMENTS; `MappingExpression.cs:13`
`reverseMap.ForMember(...)` CALLS (3 candidates over the overloads); `ISourceToDestinationNameMapper`
interface + methods defined. 0 false edges in samples; no exact edges involved (`--no-scip`
legs — exactness is S3, proven by the fixture e2e). S5 holds.
