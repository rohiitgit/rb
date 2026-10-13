# M6k evaluator pass 1 (full review, adversarial probing included)

Ran 2026-10-12 on the close tree. VERDICT: PASS with 6 fixes (H1–H6, all found
during probing/measurement and in-tree before this pass), all mutants killed.

## Findings

- **H1 (fix: dotted C# namespaces never spelled modules).** The module query took
  only bare `identifier` names, so dotted file-scoped namespaces (`namespace
  A.B.C;`) emitted no Module def and their files silently lost same-namespace
  resolution. Both namespace forms now take bare or dotted names (`queries.rs`)
  + unit. Found by classifying the automapper leg, not by a test.
- **H2 (fix: cross-namespace merge via the global fallback).** With H1 unfixed,
  every dotted-namespace file shared the `("","")` key, producing
  cross-namespace matches (e.g. `IgnoreAttribute` in `A.C.Annotations` →
  `IMemberConfigurationProvider` in `A.C`, no using — source-read, false).
  Bisected to the hunk with a two-file miniature; the fallback now only serves
  genuinely namespace-less files. Lesson: a fallback that fires on missing data
  must not fire on data missing through a bug.
- **H3 (fix: C# enclosing namespaces out of scope).** Rerun analysis showed 62
  lost true edges: parent-namespace visibility base covered by accident through
  the outer-namespace `using` fallback. `same_package` now walks enclosing
  namespaces nearest-first with hiding, plus global last (all genuine C#
  visibility). Miniature-proven both directions and pinned in the fixture
  (`SubHelper.cs` resolves, `OtherDecoy.cs` silent, both guards bite).
- **H4 (fix: anchored diagnostics sited the wrong file).** Unresolved anchored
  points reported the owner's path with record-file spans (found via springfu's
  15 new diagnostics: spans landing mid-identifier in handler files). The shared
  helper now sites the record file (declared points: identical). Unit pins it.
  The XML lane's owner-path shape stays as precedent, out of scope.
- **H5 (fix: static `[Inject]` properties pointed).** C# bindings carried no
  modifiers, so the pack could not skip statics (never injected; Java-field
  parity). Modifiers ride property bindings now; gate + unit (marked, served,
  plain, static).
- **H6 (fix: vacuous mutant tests).** The assisted unit used a scalar parameter
  (excluded anyway) and the ambiguous-anchor unit's qualifier matched nothing
  either way — both mutants survived. Repaired to discriminate; both kill now.
  Lesson: a mutant that survives is a test bug until proven otherwise.
- **P7 (verified, no change): import-gating.** Same-named `Assisted` from
  another package does not exclude (unit shows the annotation seen-but-kept).
- **P8 (verified, no change): no panics.** New paths index-checked or
  `continue`-guarded; the one `.expect` added mid-milestone replaced with
  `if let` (grep-verified over the phase diff); empty elements flow to
  unresolved (fixture + springfu leg).
- **P9 (verified, no change): determinism.** BTreeMap/Set iteration,
  encounter-order vecs, span-order numbering, sorted candidates; e2e
  double-index + forced-rebuild green on all touched fixtures.
- **Reviewed, no change:** keyed-scalar fallback points (qualifier-only, never
  type-looked-up); C# generic property points (unresolved, safe direction);
  `bean(::ref)` callable refs (skipped, residue); duplicate unanchored records
  (ambiguous, honest); `toProvider` registrant absence (the provider is not the
  service — correctly unique where `to` is ambiguous); dynamic keys (skip
  registration like unresolvable Guice qualifiers; dynamic-key points resolve
  unqualified — residue per contract); XML-lane foreign-span diagnostics
  (precedent, backlog).

## Mutants (this pass)

Manual battery, 7/7 kills on the final tree (assisted gate, `toProvider` name,
property call-site gate, C# `argument` unwrap, global-key revert, anchor
unique-or-skip, dotted-module query). E2E bites re-verified (all restores
byte-identical and green; close loop covers all post-restore states).

## Measurement (this pass)

- guice: +20/−19, all ambiguous-candidate growth from genuine `toProvider`
  beans (+`bean_type_inferred` where honest); the single removed unique edge
  reappears as ambiguous (source-read: plain `@Inject`, non-assisted). 0 false.
- springfu: +16/−1, all `D`: 15 newly-visible val-form `ref()` misses
  (`no_bean`, bare/untyped — the exact residue class), diagnostics sited in the
  record files. 0 false.
- automapper: +5740/−353 — 333 Module nodes + 334 DEFINED_IN (dotted-ns
  detection); IMPLEMENTS/REFERENCES 3501 all namespace- or using-legitimate
  (3377 same/parent-ns, 107 using-covered, 17 global visibility; programmatic, 0
  unexplained); CALLS 1227 via improved receiver typing (ambiguous = genuine
  overload/dispatch sets); removed = coverage churn + 1 evidence correction;
  zero lost edges, zero new exact Tier-B edges.
- Samples read in source: PersistFilter, EntityManagerFactoryProvider,
  OwnerConfig/Handler/Jdbc impls, IgnoreAttribute (+ the false positive that
  caught H2), InternalApi, MapperConfiguration, TestApp/Program,
  FlatteningMapper/Conventions/TypeMap call sites. 0 false in 20+ samples.

## Verdict

PASS with H1–H6 in-tree. No closing-bar blocker: (a) every fix holds (units +
mutants + e2e green on the close tree, bites re-verified); (b) no regression
(invariant-1 15/16 + 2 SCIP identical, 3 extended fixtures classified; the 62
transiently-lost parent-ns edges analyzed, restored by H3, and re-proven in the
rerun); (c) no false bean/injection through a controlled form (fixture guards +
20+ measurement samples, all genuine); (d) determinism holds (double-index +
forced-rebuild green everywhere); (e) every test/guard/step green (330 indexer, 196 frameworks, 119 syntax units;
store 72, server 12, 47 e2e incl. the extended guards, api 1 — close loop); (f) AGE parity on all fixtures (e2e).
