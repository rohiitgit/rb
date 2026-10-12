# M6i evaluator pass 1 (full review, adversarial probing included)

Ran 2026-10-12 on the close tree (`d0babae`; code identical to the `p6-close2`-verified
`456f1a8` — the re-bless commit touches goldens + contract only). VERDICT: PASS with 1 fix
(H1, found and fixed during probing, in-tree before this pass; mutant killed, unit green in
the close loop's 888).

## Findings

- **H1 (fix: Kotlin wildcard imports resolved names nowhere).** `import com.acme.model.*`
  found its target files (modules level, unit-tested) but `imported()`'s wildcard arm was
  Java-only, so a star-imported class stayed unresolved (scratch end-to-end: 2 unresolved
  calls, 0 edges). Deeper than it looked: the arm fix alone did not bite — `levels()`'
  on-demand level is also Java-gated, and a lost-update race between two parallel edits to
  the same file silently dropped the arm fix (caught by re-reading; lesson recorded).
  Final fix is one line (arm reads `Java | Kotlin`): Kotlin star imports resolve at the
  import level, so imports precede the package — the non-Java order, matching Kotlin's
  import-shadows-package rule (probed live: same-package `p.Foo` vs star-imported `q.Foo`
  resolves to `q.Foo#who`, `println` the only unresolved). Test-first
  (`kotlin_wildcard_imports_resolve`: constructor stays `References`, Go precedent);
  revert-mutant fails it. Residue: untyped constructor-call values, no nested-type star
  fallback, no extension receivers (all pinned or probed, safe direction).
- **P2 (verified, no change): object expressions and companions.** `object : Runnable` members
  land as methods under the enclosing function (`Host.start.run` — no symbol collision: locals
  are not symbols, receivers are type-gated); companion members land on the class (`C.make`,
  correct — that is how Kotlin addresses them); enum/data classes land as classes (Java-record
  precedent); top-level-object members land top-level. Anonymous `object` needs no
  `anonymous_scopes` entry: nothing resolves to it wrongly.
- **P3 (verified, no change): grammar panics.** 500-deep nesting parses without panic or
  `syntax_error`; the 18 real-world `syntax_error` files in measurement all yield partial
  definitions plus the diagnostic (M1b C8), never a panic. Whole-file ranges are grammar
  gaps recorded for their owner.
- **P4 (verified, no change): C# invocation supplement.** `Foo`/`this.Bar`/`N.S` each emit
  exactly one reference with the right qualifier — stock tags contribute no duplicate (their
  `@reference.send` is unmapped, as claimed).
- **P5 (verified, no change): descriptors.** No join code changed in M6i; Kotlin exactness
  rides the unchanged agreement checks — `scip_kotlin_gradle` green in the close loop with
  0 rejected documents and pair-wise Tier-B agreement. Nothing to game.
- **P6 (verified, no change): cross-language leakage.** C# resolution is language-gated
  (`csharp()` filters `Language::CSharp`; no C# arm reads other languages); automapper shows
  zero old-file changes. Java↔Kotlin share packages by design and every such edge was read
  in source (7 Scrabble interop edges; `flow` import hit). `BUILTIN_TYPES` keeps no Java
  reference-type spellings (phase-2 lesson, carriers-e2e-caught).
- **P7 (verified, no change): query confusion.** First-pattern-wins order holds on real code
  (`LongWrapper` interface + `MutableLong` class in one file, both correctly kinded);
  `object_declaration` requires a name (anonymous/companion objects emit no phantom class).
- **P8 (verified, no change): growth.** No new unbounded walk (tree-ancestor O(depth),
  dot-count rsplit, single-pass visitors); S7 by design note per the M6b-S9 precedent.
- **Reviewed, no change:** `PIPELINE m6i-1` metadata-only (no golden pins it); invariant-1
  (12/13 identical + 1 classified); determinism + AGE parity (close-loop e2e green);
  `java-gradle-catalog` re-bless reviewed row by row (amendment 8).

## Mutants (this pass)

Wildcard-arm revert (Java-only): the new unit fails — killed, fix restored byte-identical
(`git diff` of the restore matches the committed fix). Phase batteries cited, not re-run:
12/12 S2, C1 loads, guard bites (7 + 3 runs), version-bite ×3.

## Verdict

PASS with the H1 fix in-tree. No closing-bar blocker: (a) the fix holds (unit green in the
close loop on the exact tree); (b) no regression (invariant-1 + 42/42 e2e after re-bless);
(c) no false definition/reference/edge through a controlled form (6 source-read samples +
  guards); (d) determinism holds; (e) every test/guard green; (f) AGE parity on all fixtures.
