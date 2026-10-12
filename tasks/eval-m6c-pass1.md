# M6c evaluator pass 1 — full adversarial review (contract closing bar, item 1)

Date: 2026-10-11. Scope: every S1–S9 soundness claim + green confirmations.
Method: trace-analysis probes (read-only) with minimal repro snippets, then
executed verification of anything suspicious. Each finding is fixed test-first
as an amendment; each green confirmation cites the mechanism + evidence.

Conventions: `di.rs` = `crates/rb-indexer/src/di.rs` unless noted as
`fw/` (`crates/rb-frameworks/src/di.rs`).

## A. Registrants (S1) — join-as-ambiguous, never lone edges
- [ ] A1. `@Import` value form (single, non-array): joins.
- [ ] A2. `@Import` of an UNRESOLVED class (not in repo): no registrants, point unchanged.
- [ ] A3. `@Import` naming a config with NO `@Bean` methods: nothing joins.
- [ ] A4. `@Bean` method with UNRESOLVED return type in a named config: joins via created-types only when resolvable, else nothing.
- [ ] A5. Collection point + named config: no join (stays collection).
- [ ] A6. Test-source `@Import`: registers nothing.
- [ ] A7. `@Bean` method in an UNNAMED config: never a lone edge (stays unresolved without scanned candidates).

## B. Gate M main-only (S2)
- [ ] B1. Test-path `Field`/`Method` (non-literal) mentions still cap.
- [ ] B2. Main-source `ClassLiteral` mentions still cap.
- [ ] B3. Test-path `ClassLiteral` of a BEAN name (not registrant): frees the point, counted iff plain.
- [ ] B4. `__tests__`/`tests` dirs + `*Test.java` all count as test paths (probe `is_test_path`).
- [ ] B5. Unparsed-file words (`words_unread`) still cap regardless of path.

## C. Deployable roles (S3)
- [ ] C1. Plugin WITHOUT app class (plain library module with spring-boot plugin): service.
- [ ] C2. App class WITHOUT plugin (microservices shape): service (measured).
- [ ] C3. NEITHER plugin NOR app class: stays library.
- [ ] C4. `@SpringBootApplication` in TEST sources: does not flip.
- [ ] C5. Non-class `@SpringBootApplication` (e.g., on an interface?): no flip.
- [ ] C6. Role flip consequences flow to RTA/orphans/DI-classpath consistently (no half-flip).

## D. Module objects (S4)
- [ ] D1. Alias cycles (`a -> b -> a` in paths): terminates, resolves nothing (keep-all).
- [ ] D2. Multi-target patterns: keep-all (unit-pinned; re-verify).
- [ ] D3. `extends` chains in tsconfig: ignored (residue).
- [ ] D4. Nearest-scope shadowing (nested tsconfig overrides root): nearest wins.
- [ ] D5. Non-TS languages unaffected (Java/Go/Python imports identical).
- [ ] D6. `package.json` without `name`: ignored.
- [ ] D7. Exports conditions-object target selection (`default` vs first-string).

## E. Rule (e) levels (S5)
- [ ] E1. On-demand shadowing: same-package hit + outside wildcard → in-repo (unit-pinned; re-verify).
- [ ] E2. Outside single import + same-package hit → in-repo (predicted flip; re-verify).
- [ ] E3. Ambiguous heads still follow every candidate (M4b open 21 stands).
- [ ] E4. Dotted supertypes keep the old presence rule (boundary documented).
- [ ] E5. Depth-cap chains still count external (conservative).
- [ ] E6. `imported_from_outside` still used ONLY for dotted heads (no other caller drifted).

## F. Non-bean consumers (S6)
- [ ] F1. Validator WITHOUT injected members: no edges, no crash.
- [ ] F2. Validator in TEST sources: stays test (not heuristic edges).
- [ ] F3. `@Bean`-returned class ALSO a stereotype bean: normal path wins (no double counting).
- [ ] F4. Two `@Bean` methods, one conditional: conditional flag flows.
- [ ] F5. Filter/listener (non-validator, non-Bean-returned) with `@Autowired`: stays `not_a_bean`.
- [ ] F6. `ConstraintValidator` matched only on SIMPLE name + `implements` (a class EXTENDING something named ConstraintValidator does not match; an unrelated interface named e.g. `MyConstraintValidator` does not match — suffix check!).
- [ ] F7. Buckets still sum to points with multi-method fanout (invariant 4).

## G. Inherited Spring Data (S7)
- [ ] G1. Non-listed names (`customAudit`, `flush`, `removeByX`): unresolved.
- [ ] G2. Allow-listed name on NON-repository receiver: unresolved.
- [ ] G3. Declared member (default method on the interface): Tier B only, no double.
- [ ] G4. Call in TEST sources on a repo: edged (calls are calls) — confirm intended.
- [ ] G5. `inherited: true` present on every S7 edge; certainty heuristic; provenance heuristic.
- [ ] G6. Unresolvable qualifier (untyped variable receiver): skipped, no crash.

## H. Lombok qualifiers (S8)
- [ ] H1. `lombok.config` with CRLF / trailing spaces / comments: parses.
- [ ] H2. `copyableAnnotations` with duplicate entries: fine.
- [ ] H3. Nearest file GARBAGE (unparseable): whole chain unread (not fallback to root!).
- [ ] H4. `@Named` (not `@Qualifier`) listed: rides too (is_qualifier covers both).
- [ ] H5. Qualifier on a DECLARED (non-Lombok) ctor param with config present: unaffected (declared path).
- [ ] H6. Two Lombok points, one listed qualifier one not: only listed narrows.
- [ ] H7. `lombok.config` in a nested dir applying to nested files only (scope check).

## I. Generics (S9a)
- [ ] I1. Bounded wildcard `? extends X` vs `X`: ambiguous (unit-pinned).
- [ ] I2. `? super X`: ambiguous.
- [ ] I3. Nested `Store<List<X>>` vs `Store<List>`: ambiguous (both directions).
- [ ] I4. Two agreers: ambiguous (not first-wins).
- [ ] I5. Agreement + `@Primary` among agreers: undecided blocks primary (stays ambiguous unless singleton).
- [ ] I6. `Map<String, V>` points: unaffected shape (element V, no agreement unless exact).
- [ ] I7. Array points (`T[]`): never agree (empty raw type).
- [ ] I8. Generic `@Bean`-method returns: no agreement (residue documented).

## J. Cross-cutting soundness (blocking classes)
- [ ] J1. No `INJECTS` above `heuristic` on fixtures + measured repos (re-grep dumps).
- [ ] J2. No pruned-but-instantiated type: RTA yield 0 verified on measurements; fixture RTA goldens green.
- [ ] J3. No false orphans: orphan goldens green; eligible sets sane on measurements.
- [ ] J4. Q3 paths through S7 targets resolve to the interface (spot-check `rb path`/`callers` on fixture).
- [ ] J5. Determinism: fresh == cached, two names, forced rebuild byte-identical (spot fixture).
- [ ] J6. Full suite green on the closing tree (final verify loop).

## Verdict section (filled at close)
- Findings (each: repro, severity, fix commit or dismissal with reason):
  - **E1 (non-blocking, fixed): `lombok.config` inline trailing comments
    poisoned the chain.** Repro: `lombok.copyableAnnotations = [X] # comment`
    → whole chain unread (old row). Severity: precision miss only (safe
    direction). Fix test-first (commented row fails first): strip inline `#`
    comments in `parse_lombok_config`; 1 fix mutant killed
    (`comment-strip-dropped`).
  - **E2 (non-blocking, fixed): abstract/non-static-nested `@SpringBootApplication`
    carriers flipped deployable.** Repro: unit rows (`abs`, `nest` services
    read `service` pre-fix). Severity: soundness-adjacent (deployability feeds
    RTA yield) but no measured instance anywhere (fixtures + 6 repos clean).
    Fix test-first (rows fail first): shape checks mirroring
    `is_bean_class` (concrete + top-level/static/interface-nested); 2 fix
    mutants killed; microservices 8 flips re-verified intact on the tightened
    rule with a rebuilt head binary.
  - Dismissed with reason: D7 map-order/nesting (deterministic, fail-closed);
    `is_stop` case/substring (conservative or Lombok-invalid); C5
    record-carrier residual (no instance, noted); A2/B-collision simple-name
    notes (conservative-only); F3 abstract-stereotype path (falls through to
    `not_a_bean` unless a genuine S6 gate matches); F6 user-defined
    same-named interface (accepted syntactic boundary); G4 test-source S7
    edges (intended per spec — calls are calls).
- Green confirmations (60 probes across A–J groups, mechanism + evidence):
  S1 join-never-lone (A1–A7 incl. collection/test/unnamed bars); S2
  class-literal-only narrowing with `mention_lifted` iff plain (B1–B5);
  S3 single-predicate roles with Java/test/class gates (C1–C4, C6);
  S4 single-pass termination, multi-target keep-all, extends-ignored,
  nearest-wins, non-TS isolation, nameless-package skip, Node `main`/exports
  precedence, scoped-split edges, `./`-less key leniency, `..` non-escape
  (D1–D7 + extras); S5 level gating with dotted fallback, ambiguity/cap
  conservatism, sole-caller discipline (E1–E6); S6 entry-widening only,
  test-first bucketing, owner-wins order, conditional fanout, Field/Setter
  enumeration, allow-list exactness, bucket-sum invariance (F1–F7); S7
  allow-list/receiver/no-double/test-source/marking/untyped behavior (G1–G6);
  S8 CRLF/dupes/unread/`@Named`/declared-params/per-field/nested-scope +
  prefix-attack fail-closed (H1–H7 + extras); S9a marking/refusal/pick/order/
  Map/array/BeanMethod behavior (I1–I8); J1 zero violations on 6 dumps.
- VERDICT: PASS (with E1+E2 fixed test-first above; no blocking finding:
  no failed fix, no regression, no false edge above heuristic, no
  non-determinism, no failed test/guard/step).
