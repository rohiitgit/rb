# M6j evaluator pass 1 (full review, adversarial probing included)

Ran 2026-10-12 on the close tree (post-`p6-close` green). VERDICT: PASS with 4 fixes
(H1–H4, all found during probing and in-tree before this pass), all mutants killed.

## Findings

- **H1 (fix: trailing `.in(Scope)` dropped whole `bind()` chains).** `bind(X).to(Y).in(S)`
  destructured to nothing (last link `in` ≠ `to`). Trailing `in` links strip before
  matching (scopes stay unread); unit + Q11. Lesson: optional trailing DSL calls need
  stripping, not whole-shape rejection.
- **H2 (fix: `bind()` chains lacked the file-marker gate).** Every other Guice/Dagger
  rule requires framework imports, but `bind_chains` read any Java file. Gated on
  `framework_file` + decoy unit + Q12. A bare `bind().to()` elsewhere is another API.
- **H3 (fix: nested `beans { }` misattribution).** The innermost predicate excluded chains
  contained in *any* other container — including the outer one being computed — so inner
  beans vanished whenever nested. Restated as strictly-between + a nested unit that
  failed first try (it also caught the inner `beans` call polluting constructed-type
  discovery — DSL keywords now excluded). Lesson: test the nesting the predicate claims.
- **H4 (fix: C# same-namespace gap analysis).** File-scoped namespaces emitted no `Module`
  def, so same-namespace resolution silently missed modern C#. Supplement + unit. While
  there, proved `Index.packages` (not `Modules.java_packages`) serves same-package lookups,
  and that C# namespaces share Java's key space (accepted: collision needs the same dotted
  name in both ecosystems; Tier B candidates would show it).
- **P5 (verified, no change): qualifier semantics.** `@Named(name=)` is invalid JSR-330
  (element is `value`), so qualifier_of ignoring `name` keys matches the language; the
  shared `literals()` path behaves identically for Spring. Token-vs-name restriction
  (lane) proven by the `DB` decoy + R7 bite.
- **P6 (verified, no change): assisted params.** No false unique edge in 24 guice INJECTS
  (3 unique all source-verified; rest ambiguous); `@Assisted` non-scalars resolving
  uniquely would be a false edge — no instance found. Recorded hardening backlog, not a
  blocker (no demonstrated falsity; everything caps at heuristic).
- **P7 (verified, no change): decorations are dump-neutral.** Binding/definition
  decorations feed packs only; nest measurement shows +168/−1 with the single removal a
  coverage row. No old-row churn from the whole decoration surface.
- **P8 (verified, no change): no panics.** New paths index-checked (`get`), matches
  exhaustive-or-`continue`, no `unwrap`/`expect`/`panic` added (grep-verified over the
  phase diff); byte spans convert by scan; empty elements flow to unresolved (fixture).
- **P9 (verified, no change): determinism.** BTreeMap/Set iteration, encounter-order vecs,
  span-order numbering; invariant rerun + close loop green.
- **Reviewed, no change:** qualified annotations (shared import-or-qualified path);
  multibindings (`@IntoSet` reads as plain binds — set semantics residue); Dagger
  `@Component` not-a-bean; `Lazy`/`Provider` wrappers; keyed .NET; factories/instances/
  `typeof`/top-level hosting; C# property injection; lifetimes/scopes; Guice/Dagger in
  Kotlin files and NestJS in plain JS (language gates); pure-jakarta consumers;
  `spring.factories`; parent-bean merge; nested `<bean>`; val-form beans (+callable
  refs); conditional DSL; dynamic NestJS modules; Spring-XML-in-test-dirs silence.

## Mutants (this pass)

Batteries re-run to green on the final tree: p1 12/12 (P1l refreshed for the TS gate,
valid-variant swap), p2 14/14, p3 6/6, p4 7/7,
p5 8/8 (W1/W3/W5 patterns refreshed post-fmt; W8 retargeted to the pack test that reads
values), p6 5/5. E2E bites re-verified (restores green; p6-close 47/47 on the final tree
covers all post-restore states). 52/52 unit mutants + all guard bites (G/F/N/X/K/D/V/R7).

## Verdict

PASS with H1–H4 in-tree. No closing-bar blocker: (a) every fix holds (units + e2e green
on the close tree); (b) no regression (invariant-1 12/13 + 2 scip identical, csharp
classified; jpetstore gate-lift intended); (c) no false bean/injection through a
controlled form (samples read in source across all 5 legs); (d) determinism holds;
(e) every test/guard/step green (913 units, 72 store, 12 server, 47 e2e, 1 api);
(f) AGE parity on all fixtures (e2e).
