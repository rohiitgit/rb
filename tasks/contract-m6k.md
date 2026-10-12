# M6k contract: DI recall (draft, 2026-10-12)

**Status:** draft written 2026-10-12 at the M6j close (`f42720c`); forks decided below;
phase 0 not started.

Written before any code. M6k is done only when every assertion below is backed by a passing
test or a recorded measurement. "Verified facts" are what the 2026-10-12 probes read in the
tree at the M6j close (`f42720c`).

This contract follows the M6e–M6j format and applies the lessons M3b–M6j paid for (soundness
bar + closing bar up front; allow-lists; suppressions counted; growth by counted ops; no
hand-counted positions; invariant 1 vs previous close with classified rows; fixture plan is
code; config-surface probing; no builds during container e2e; non-empty dumps; one file one
edit per block).

## Why (read first)

M6j's measurement proved two features near-dormant in the wild: every real `beans { }`
site is a top-level `val` form (spring-fu leg: zero beans INJECTS), and minimal hosting
is the default .NET style (top-level statements unanchorable). Both stay silent by
construction today. M6k wires them through one shared mechanism (anchor at the bound
type), plus the cheapest precision gaps with demonstrated shapes: assisted params (the
one false-edge risk with no instance yet), C# property injection, keyed services, and
`toProvider`.

## Soundness claims and closing bar (read first)

New outputs whose failure would be silent: a bean anchored at the wrong type (a
val-form/type-arg misread); an assisted param still edging (the exclusion missing a
spelling); a property point on an unmarked property (the attribute gate too wide); a key
matching the wrong bean (qualifier confusion). **Everything not proven takes the safe
direction:** unresolvable anchors skip (never guess); assisted exclusion is by
import-gated annotation; properties need `[Inject]`/`[FromServices]`; keys ride the
existing qualifier machinery (literal-only); certainty never above heuristic.

| Surface | Claim (exactly this, no more) | Mechanism | Residue (out of scope by construction) |
|---|---|---|---|
| Anchor at type (S1) | Enclosure-less `bean()`/`AddX()` bind their bound type's definition | Shared anchor rule; unique-or-skip | Ambiguous anchors; `typeof`/instance/factory args (unchanged) |
| Val-form beans (S2) | Top-level `beans { }` registrations read as beans | S1 + existing `Binds` origin | `bean(::ref)` callable refs (unchanged); conditional DSL (unchanged) |
| Minimal hosting (S3) | Top-level `AddX<T>()` reads as beans | S1 + existing `Binds` origin | Everything else top-level (unchanged) |
| Assisted (S4) | `@Assisted` params never become points | Import-gated exclusion in `point()` | Assisted in other frameworks (none exist) |
| C# properties (S5) | `[Inject]`/`[FromServices]` properties are bindings + points | `property_declaration` arm; attribute gate | Unmarked properties (not injected, correctly silent) |
| Keyed .NET (S6) | `Add/TryAddKeyed*` + `[FromKeyedServices]` match by key literal | Keys as qualifiers through existing matching | Non-literal keys (dynamic, counted) |
| Guice providers (S7) | `toProvider(P.class)` records the impl | `to`-equivalent middle | `toInstance` (untypable, stays unread) |

**What M6k must not change.** Existing facts byte-identical modulo classified rows
(C# property bindings WILL shift C# dumps — re-bless with census; everything else
additive). `PIPELINE` becomes `m6k-1` at the first dump change.

This is invariant 1, checked against the M6j close dumps of every fixture (with and without
SCIP) and of the re-measured repositories at their pins, every differing row classified by
script (dumps asserted non-empty first).

**Closing bar.** Decided now:
1. **Evaluator pass 1 is a full review,** adversarial probing included.
2. **Evaluator pass 2 is verification-only.**
3. **Blocks closing only:** (a) a pass-1 fix that does not hold; (b) a regression; (c) a
   false bean/injection through a controlled form; (d) non-determinism; (e) a failed
   test/guard/step; (f) an AGE parity mismatch on any fixture.
4. **Not blocking:** a new form in a residue class (recorded); a precision miss contradicting
   no rule; §timing (measured, not asserted).
5. **Timings are measured, not asserted.**

## Scope

**In (M6k):** the 7 items above.

**Out of M6k (each assigned):** lifetimes/scopes modeling → residue (no demonstrated payoff);
conditional Kotlin DSL → residue; dynamic NestJS modules → residue; `typeof`/instance
factories → residue; new SCIP languages → beyond M6; exactness above heuristic → never.

## Engineer declaration

**Canonical state:** `Binds` beans anchored at type defs (new anchor mode beside
declaration anchors); assisted-free points; C# property bindings; keyed qualifiers.
**Derived:** `INJECTS` edges through the unchanged lanes.
**Invariants:**
1. **Dumps.** `--no-scip` and SCIP dumps differ from the M6j close only in classified rows.
2. **Old packs frozen** modulo the claimed additions.
3. **Taint never lost** (gate D untouched by this milestone).
4. **Egress closure** (no new fetches; re-measurement reuses M6j pins).
5. **Determinism** (fresh == cached, two names, forced rebuilds).
6. **Bounded work** (no new unbounded walk; anchor lookups reuse the class index).
7. **Stated complexity by counted operations.**

## Decisions (made here, with reasons)

- **D1: anchor at type (F1).** A binding declaration without an enclosing definition
  anchors at its bound type's unique definition (XML-bean precedent from M6j F5); zero or
  several matches skip. One mechanism serves val-form beans and top-level `AddX`.
- **D2: assisted excludes (F2).** Import-gated `Assisted` on a binding kills its point
  (the `@Value` precedent in `point()`).
- **D3: properties opt-in (F3).** C# properties become member bindings (uniform with
  fields), but points need `[Inject]`/`[FromServices]`; unmarked properties stay silent.
- **D4: keys are qualifiers (F4).** Key literals ride names+qualifiers through existing
  matching; `toProvider(P.class)` rides `created` like `to`.
- **D5: fixture plan is code (F5).** Extend `kotlin-beans` (val-form + anchor cases),
  `csharp` (minimal-hosting file? NO — top-level statements need no fixture service
  change; extend with keyed + property shapes), `java-guice` (`toProvider` case);
  exact file lists amended per phase.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **Anchor at type.** Unique anchors bind, ambiguous/missing skip. | unit; mutation |
| S2 | **Val-form beans.** Top-level registrations read; points wire. | unit; mutation; e2e goldens |
| S3 | **Minimal hosting.** Top-level `AddX` reads; points wire. | unit; mutation; e2e goldens |
| S4 | **Assisted.** No point from assisted bindings. | unit; mutation |
| S5 | **C# properties.** Marked properties bind + point; unmarked silent. | unit; mutation; e2e goldens |
| S6 | **Keyed + providers.** Keys match; `toProvider` records impl. | unit; mutation; e2e goldens |
| S7 | **Fixtures + guards.** Extended guards, each biting. | e2e with containers; goldens |
| S8 | **Re-measurement.** automapper + springfu + guice legs classified; 0 false. | recorded measurement |
| S9 | **Determinism + invariant 1.** Fresh == cached etc.; dumps classified. | e2e; recorded |
| S10 | **Growth.** No new unbounded walk (anchor lookups indexed). | design note + existing ratios green |
| S11 | **Verification loop + closing.** `var/m6k/scripts/verify.sh` standard. | verification loop |

## Verified facts (to be read at the M6j close commit, in phase 0)

1. Baseline HEAD `f42720c`; baseline binary + release sha recorded.
2. Re-measurement reuses M6j pins (guice/nest/jpetstore/springfu/automapper).
3. `point()` excludes `@Value` (the Assisted precedent shape lives beside it).
4. C# `property_declaration` carries `type` + `attribute_list` (M6j S6 probes).
5. `CallLink.type_args` + `bean_start` + class index all exist (shared machinery).

**Not verified (checked in preflight, before the step that needs it):** top-level statement
chain shapes for `AddX` (enclosing None confirmed?); `TryAddKeyed*` arg shapes;
`[FromKeyedServices]` attr shapes; assisted-import FQNs in the wild.

## Fixture plan

- `fixtures/kotlin-beans/`: val-form block + anchor-ambiguity case.
- `fixtures/csharp/`: top-level-style `AddX` in a methodless context? NO — top-level
  statements need no service change; add keyed registrations + `[Inject]` property to
  existing/new files.
- `fixtures/java-guice/`: `toProvider` case.
- No changes to other fixtures.

## Public-repository measurement plan

| Step | Repository @ pin | Runs | Notes |
|---|---|---|---|
| Phase 0 | — | M6j close binary dumps; e2e green baseline | small |
| Phase 4 | automapper/springfu/guice @ M6j pins | `--no-scip` both binaries | classified diffs (C# properties, val-form, assisted) |

One repo at a time, remove + `VACUUM`, part-by-part figures (M6b D9).

## Determinism

Unchanged rules. Fresh == cached, two names, forced rebuilds (e2e).

## Verification loop

`var/m6k/scripts/verify.sh <tag>` (copied from M6j's, M6k paths/logs): standard.

## Forks (decided here, with reasons)

- **F1: anchor at type.** Declaration-less bindings anchor at the bound type (XML precedent).
- **F2: assisted excludes.** Import-gated, `@Value`-style.
- **F3: properties opt-in.** Bindings uniform, points gated.
- **F4: keys are qualifiers.** Existing matching, literal-only.
- **F5: fixture plan is code.** Extensions named above.

## Amendments

(none yet)

## Amendments

1. **(phase 0) Baselines.** Baseline binary `var/m6k/base/rb-m6jclose` (release, sha256
`eb2242be…`, no-op rebuild proves HEAD code == the M6j close tree: the contract commit
adds no code). Re-measurement reuses M6j pins. Baseline verify
`var/m6k/logs/p0-baseline-*` green (fmt, clippy `-D warnings`, 914 units, store 72,
server 12, e2e 47/47, api 1/1, no skips, no listener, stub scan clean). DB holds only
the 3 pre-existing `fx_*` repos.

2. **(phase 1) Anchor at type (S1).** `FileDi::unanchored` (Default, non-breaking) +
shared `resolve_lane_point` + two-phase anchor/link in `resolve_nest` (unique-or-skip,
XML-bean precedent). Commit `d5b3db8`. Note: the phase arrived with a broken
`xml_beans_grow_linearly` header (insertion ate the doc + `fn` lines); repaired and
proven by the suite. Lesson re-learned: never parallelize two edits to the same file
(the C# `Aux` rename was lost the same way later and re-applied sequentially).

3. **(phase 2) Packs (S4–S6).** Import-gated `@Assisted` exclusion in `point()`;
`toProvider` beside `to` in `bind_chains`; C# `property_declaration` bindings +
`[Inject]`/`[FromServices]`/`[FromKeyedServices]` property points; keyed
registrations (key literal rides qualifiers, non-literal skips) +
`[FromKeyedServices]` point qualifiers; C# `argument`-wrapper unwrap in `summary()`
(identifier args now read as names, `this.Bar(x)`-shaped — the `csharp_calls_chain`
expectation updated, strictly more precise); global-namespace package key
(`("","")`) so namespace-less files resolve each other (minimal hosting's
Program.cs leg; namespaced files keep their keys). Indexer proofs for val-form,
top-level .NET, property+keyed wiring. Commit `7f00c81`.

4. **(phase 3) Fixtures + guards (S7).** `ValBeans.kt` (self-contained val form;
deliberately fresh type/method names — reusing `load`/`stock` tripped the
pre-existing orphan mention rule on existing nodes, so the fixture keeps the dump
diff purely additive); `Keyed.cs` (keyed pair + registered consumer with marked
and unmarked properties) + `Mini.cs` (top-level minimal hosting, global
namespace); Guice `Cache`/`CacheProvider`/`CacheUser`/`AssistedShop` + `toProvider`
line. Goldens re-blessed and reviewed row-by-row (all added rows classified;
removed rows are coverage updates, ShopModule line-shifts, blobs). Every new
guard bites by perturbation and the tree is restored byte-identical and green
after each. Commit `3ac29c3`.

5. **(phase 4) Units, mutants, PIPELINE.** Ambiguous/missing-anchor-skip unit.
Manual mutant battery, 7/7 kills (two test repairs: assisted param must be
non-scalar to discriminate the gate; the ambiguous-anchor ref must resolve when
bound, else the mutant is vacuous). `PIPELINE m6k-1` (`lib.rs`). fmt + clippy
`-D warnings` clean. Commit `ba6163c`.

6. **(phase 5) Invariant + measurement (S8–S9).** Scripts
`var/m6k/scripts/p9-invariant.sh` (all old fixtures + the three extended ones —
both legs use the working-tree fixtures, so the diff isolates code-driven
change; SCIP legs per precedent) and `p8-measure.sh` (guice/springfu/automapper
@ M6j pins; nest/jpetstore excluded — M6k touches neither TS nor XML).
Release head binary sha `99d7156c…`. Logs `var/m6k/logs/p9-invariant.log`,
`p8-measure.log`. (Results recorded here on completion.)

7. **(pass 1) Dotted namespaces + diagnostic siting.** File-scoped namespaces with
dotted names never matched the module query (it took only bare `identifier`s), so
their files silently lost same-namespace resolution; the M6k global-namespace
fallback then merged them all under `("","")`, producing cross-namespace matches
in the automapper leg (e.g. `IgnoreAttribute` → `IMemberConfigurationProvider`,
different namespaces, no using — a false positive, caught by source-reading the
new pair). Fix: both namespace forms accept bare or dotted names (`queries.rs`) +
unit; the `("","")` fallback stays, now correctly scoped to genuinely
namespace-less files (miniature-proven: same dotted-namespace resolves,
cross-namespace `Decoy` stays silent). Static `[Inject]` properties excluded;
`@Assisted` import-gating proven; anchored `di_unresolved` diagnostics site the
record file (the XML lane's owner-path-with-foreign-span shape stays as
precedent, out of scope). The first automapper leg is VOID (measured the
intermediate); invariant + measurement rerun on the final tree (amendment 8).

8. **(pass 1) Enclosing-namespace scope.** Rerun analysis showed 62 lost true
edges: parent-namespace visibility base covered by accident through the
outer-namespace `using` fallback (e.g. `IGlobalConfiguration` in
`AutoMapper.Internal` → `IConfigurationProvider` in `AutoMapper`, no using).
Fix: C# `same_package` walks enclosing namespaces nearest-first with hiding,
plus global last (all genuine C# visibility, no new guess). Miniature-proven
both directions and pinned in the fixture (`SubHelper.cs` resolves,
`OtherDecoy.cs` silent, both guards bite). Release `d07cf7e1` superseded;
invariant + measurement rerun on the final tree (amendment 9).

9. **(phase 5b) Rerun results (S8–S9, final tree `41e7d632`).**
Invariant-1 (`p9-invariant3.log`): 15/16 fixtures byte-identical + 2 SCIP
identical; only the 3 extended fixtures differ, exactly as their e2e goldens
pin (guice +INJECTS/−CacheUser-unresolved/−2 assisted edges; kbeans +val-form
edge; csharp +3 INJECTS/+SubHelper edges/−newly-resolved diagnostics +
supertype-keep lift on `SubHelper`, mechanism working as designed).
Measurement (`p8-measure3.log`):
- guice +20/−19: ambiguous-candidate growth from genuine `toProvider` beans
  (+`bean_type_inferred` where honest); the one removed unique edge reappears
  as ambiguous (source-read: plain `@Inject`). 0 false.
- springfu +16/−1, all `D`: 15 newly-visible val-form `ref()` misses
  (`no_bean`, bare/untyped residue), now sited in the record files. 0 false.
- automapper +5740/−353: 333 Module nodes + 334 DEFINED_IN (dotted-ns
  detection); IMPLEMENTS/REFERENCES 3501, every one namespace- or
  using-legitimate (3377 same/parent-ns, 107 using-covered, 17 global
  visibility; programmatic, 0 unexplained); CALLS 1227 (receiver types via
  parent-ns; ambiguous ones are genuine overload/dispatch sets); removed rows
  are coverage churn + 1 evidence-corrected DEFINED_IN; zero lost edges, zero
  new exact Tier-B edges. 20+ source-read samples, 0 false.
The two intermediate legs (polluted automapper, mis-sited springfu
diagnostics) are VOID and superseded by these numbers.

10. **(close) Evaluator pass 2 + records.** Pass 2 (`tasks/eval-m6k-pass2.md`):
VERDICT PASS — H1–H6 hold in the close loop (926 units), fixtures green,
invariant + measurement stand, tree == loop tree. Records: CHANGELOG
(`M6k CLOSED`), IMPLEMENT.md §M6k, design §13 M6k row, lessons (3 new),
todo (M6k closed). `PIPELINE` stays `m6k-1`.
