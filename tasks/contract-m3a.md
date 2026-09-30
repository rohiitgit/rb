# M3a contract: Tier B heuristic resolution, `rb callers`, `rb impact` (draft, planner, 2026-09-30)

Written before any code. M3a is done only when every assertion below is backed by a passing test or a recorded measurement.

## Scope

M3 in the roadmap (§13) is split in two:
- **M3a (this contract):** call and type resolution from Tree-sitter facts alone (design §5.4, "Tier B"), `CALLS`/`REFERENCES`/`IMPLEMENTS` edges, and the first two questions: Q1 `rb callers` (§9.2) and Q2 `rb impact` (§9.3).
- **M3b (later contract):** SCIP indexers in pinned containers (§5.2), the SCIP⋈Tree-sitter join (§5.3), key replacement per project, and change sets.

M3a needs no external tool. Every M3a edge has provenance `heuristic`; SCIP later replaces it project by project.

**Grammar limits found during implementation:**
- Java's tags capture only simple type names, so scoped types (`a.b.C x`, `new x.Y()`) produce no type reference. Java supertypes still resolve through `IMPLEMENTS`, which reads the full supertype text.
- Java method overrides match by name only; parameter counts are not in the facts.

**Out of scope, and why:**
- RTA and `impl_instantiated` need DI facts (M4); M3a queries use CHA only.
- The in-memory CSR server (`rb-server`) is M5. M3a runs its BFS in the CLI over edges loaded from the relational tables.
- Orphan detection (§9.3) moves to M3b, where exact edges make it meaningful.
- Endpoint and test roll-ups need route packs (M4).

## Engineer declaration

**Canonical state** (cached extractor output, per file):
- **`imports`:** module as written, the imported names with their local aliases, and whether it is a wildcard, namespace or default import.
- **`package`:** the Java `package` or Go `package` clause.
- **`bindings`:** names with a declared or constructed type in a scope. They come from fields, parameters, locals with an explicit type, and `x = new T()` / `x = T()` / `x := T{}` initialisers.

**Derived** (in the indexer, per snapshot):
- **Module resolution:** import → target files.
- **Call and type resolution:** candidates found level by level, per §5.4.
- **Edges:**
  - `CALLS` / `REFERENCES`: Symbol → Symbol, with File as the source for top-level code;
  - `IMPLEMENTS`: type → type, and method → overridden method.
- **Coverage:** counts of resolved, ambiguous and unresolved references.

**Invariants:**
1. Every `CALLS`, `REFERENCES` and `IMPLEMENTS` edge ends at a `Symbol`. Its source is a `Symbol`, or the `File` for top-level code. Provenance is `heuristic`.
2. Certainty is `heuristic` for exactly one candidate and `ambiguous` (attr `candidates = n`) for several. No M3a edge is ever `exact`, `dispatch` or `synthetic`.
3. A member call on a receiver of unknown type is **never** linked by bare name. It counts as unresolved.
4. Resolution stops at the first level with candidates (§5.4). A candidate is never taken from a lower level when a higher level has one.
5. Determinism and warm cache = cold still hold (the new facts are cached; the extractor fingerprint changes).
6. Every walk in the resolver and the queries terminates by construction (visited sets, caps) and is linear in edges. Every recursive or iterative resolution gets a cycle test and a depth test (lessons 2026-09-30).

## Assertions

| # | Assertion | Proof |
|---|---|---|
| R1 | `rb-syntax` extracts imports for TS/TSX/JS (ES `import` forms), Java (single-type, wildcard, static), Python (`import a.b [as c]`, `from [.]a import b [as c]`, relative levels) and Go (`import [alias] "path"`), plus the Java/Go package clause | rb-syntax unit, per language |
| R2 | `rb-syntax` extracts typed bindings: Java fields, parameters and typed locals; TS typed fields, parameters and variables, plus `new T()` initialisers; Python `x = T()` and annotated names, plus parameters with annotations; Go `var x T`, typed parameters, and `x := T{}` / `&T{}` (a call such as `NewT()` does not bind a type). `this`/`self`/`super` are implicit bindings to the enclosing class. | rb-syntax unit, per language |
| R3 | Module resolution: TS/JS relative specifiers, probing `.ts .tsx .js .jsx .mjs .cjs` and `/index.*`; Java package + class, wildcard = every file of the package; Python absolute modules (unique suffix match over repo files, `a/b.py` or `a/b/__init__.py`) and relative levels; Go import path = module path from the nearest `go.mod` + subdirectory. Unresolvable imports (external packages) are recorded as external and give no edge. | indexer unit |
| R4 | Call resolution follows §5.4's levels: enclosing class (including `this.`/`self.`), the same file, imported names (named imports, namespace or module aliases `m.f()`, Java static imports), the same package or directory (Java package, Go package directory). Typed receivers (`x.f()` with `x` bound to type `T`) resolve to `T`'s methods, and to inherited methods through resolved supertypes. `super.f()` resolves to the superclass. Class-qualified calls (`T.f()`) resolve to `T`'s members. | indexer unit, one test per level and per language family |
| R5 | Invariants 2 and 3: one candidate is `heuristic`; several are `ambiguous` with `candidates`; an unknown receiver is unresolved (never bare-name linked) | indexer unit |
| R6 | `REFERENCES`: class and type references resolve with the same levels to type definitions; a qualified type (`store.DB`) resolves through its qualifier. A *call* that resolves to a class (Python `T()`) gives `CALLS` to its declared constructor, else `REFERENCES` to `T`. `new T()` is tagged as a class reference by the grammars, not a call, so it gives `REFERENCES` to `T` (revised during implementation). | indexer unit |
| R7 | `IMPLEMENTS`: every resolved supertype gives type → type. A method with the same name (and, for Java, the same parameter count) in a resolved supertype gives method → method. Unresolved supertypes (external) give nothing. | indexer unit |
| R8 | Coverage: per file, an `unresolved_references` diagnostic counts unresolved calls, unresolved types and references into external packages (imports that resolve to no snapshot file, directly or through a binding's declared type). Go and Python built-ins are neither resolved nor counted. `rb status` shows the share of references to the snapshot's own code that resolved, with resolved and ambiguous sites derived from edge evidence. | e2e |
| R9 | `rb callers <target> [--depth N] [--include-references] [--min-certainty C]`: resolves input per §9.1 (candidate list on ambiguity, never a silent pick, `--pick N`). Output is a caller tree with file:line, service and certainty. Dispatch callers come through `IMPLEMENTS` (CHA) and are marked `dispatch`. A path's certainty is the minimum along it. Output is deterministic. | e2e goldens on every fixture |
| R10 | `rb impact <target>`: tier 1 (distance-1 requiring edges: `CALLS`, `REFERENCES`, `IMPLEMENTS`, and `MAPS_TO`/`QUERIES` for tables); tier 2 (inbound reachability over `CALLS ∪ REFERENCES` plus CHA). A class target includes its members through `CONTAINS`. Roll-ups: own service vs other services, and owners with symbol counts. There is a node cap with a `truncated` flag. | e2e goldens on every fixture |
| R11 | Fixtures gain enough cross-file calls to exercise every level and language, and include the must-not-exist cases (unknown receiver, a same-named method in an unrelated class) | goldens |
| R12 | Public repos at pinned commits (the M1b set: spring-petclinic, hono, gin, requests): no panics; resolution rate per language recorded; **precision** measured by hand-checking 20 random `heuristic` edges per repo against the source, target ≥ 90% correct; time recorded | recorded measurement |
| R13 | fmt, clippy, tests and the stub scan are clean; a separate evaluator pass finds no open defects; no new dependency without a stated reason | verification loop |

## Amendment 1: evaluator pass 1 (12 defects, all fixed test-first)

- **Invariant 6:**
  - Containers are deduplicated at every step, so repeated bindings or declarations no longer multiply along a field chain; it was exponential.
  - Bindings are looked up through per-file indexes instead of a scan.
  - The same-package level is a precomputed per-package index, not a scan of the package.
  - Timing tests: a 24-segment chain with duplicates, a 4,000-line binding chain, and 1,500 files in one package.
- **Invariant 3:**
  - `rb-syntax` records untyped declarations as bindings with an empty type: untyped parameters (Python, JS), untyped locals, and Go short declarations of calls.
  - A variable's receiver is its declared type or nothing, never a same-named type or module.
  - An untyped inner binding hides a typed outer one.
  - A class body's own names are not locals of its methods; a Python class attribute is a field, reachable through `self`.
- **Rebinding:** among several bindings of a name in one scope, the nearest one declared before the reference wins.
- **Java levels:** single-type imports, then the same package, then wildcard imports (JLS §6.4.1).
- **Go:** `_test.go` files are never part of an imported package; the same-package level matches both directory and package clause.
- **Python:** an absolute import matches a module only when the directory above it is not a package (`import json` inside `pkg/` is the standard library, not `pkg/json.py`). Ties between valid candidates stay ambiguous; the old "unique suffix match" wording was wrong.
- **Recursion:** recursive calls are `CALLS` edges to self.
- **`rb impact`:** callers of a method the target overrides (CHA dispatch) are affected at distance 1.
- **`rb status`:** a reference site is ambiguous when it has several targets, not when its edge's certainty is `ambiguous`.
- **Known and accepted:** an edge's certainty is the best of its sites, so an edge whose sites are sometimes ambiguous shows `heuristic`.

## Amendment 2: evaluator pass 2 (5 defects, 2 of them regressions from Amendment 1; all fixed test-first)

- **Declarations carry an extent** (`Binding::visible`): from the end of the declaration to the end of its scope. Scopes are blocks, functions, loops, catch clauses and lambdas; in Python, the function (comprehensions and `except … as` names have their own). A comprehension variable is in effect from the comprehension's opening bracket, because the body precedes the `for`.
  - Every declaration form is recorded: loop variables, `with`/`except`/`catch` names, comprehension variables, destructuring and tuple targets, walrus names, lambda and arrow parameters, Go multiple assignment and type-switch aliases, Java enhanced-for, resource and `instanceof` pattern locals.
  - A reference resolves to the innermost declaration whose extent contains it. This replaces "nearest preceding declaration in the function", which ignored block scope and a statement's own right-hand side (`c := c.Copy()` hid the parameter it read).
  - Extents are nested or disjoint, so declarations of a name form a tree; a lookup is a binary search plus a walk up that tree, linear overall. Forty thousand declarations of one name are timed.
  - This also removes the "anonymous-function parameter" imprecision recorded in IMPLEMENT.md: a lambda's parameters end with the lambda.
- **A definition's own name is not a reference to itself** (`type Conn struct{}`); a recursive field type still is.
- **Python source roots:** an absolute import matches a module whose source root (the directory above the matched segments) is the importer's own root, or one of the conventional roots `""` and `src`. A `tests/logging.py` beside `tests/test_x.py` is therefore a candidate for that test's `import logging`; without an environment, the standard library cannot be told apart, and this is accepted. An `__init__.py` at the repository root no longer disables absolute imports.
- **Java nested classes** resolve through imports (`import a.Util.Nested`, `import static a.Util.Nested.n`, `import a.Util.*`).
- **`rb callers`** lists a recursive target as its own direct caller, once.

## Amendment 3: evaluator pass 3 (5 defects in the extent rules and bare calls; all fixed test-first)

- **Python `except` bodies** are not scopes. Only the `except … as e` name ends with its clause; an assignment inside the clause is function-scoped like any other.
- **Class-body names** are skipped, not final: when the innermost declaration in effect is a class body's own name and the reference is inside a method, the lookup continues with the enclosing declaration.
- **Signature parameters** (TypeScript function types, interface and overload signatures, call and construct signatures; Go function types and interface methods) end with their signature, never the enclosing block.
- **Go `case` clauses** (expression, type, default and select cases) are scopes.
- **Bare calls through variables:** `cb()` where `cb` is a parameter or local is never linked by name to a function; it counts as unresolved. A typed variable is not followed to a `__call__` or functional interface.
- **Accepted:** a closure reading a name declared after the closure's text; JavaScript `var` hoisting out of a block; Java `instanceof` flow scoping; a walrus inside a comprehension. Each gives an unresolved reference, never a wrong edge.

## Amendment 4: evaluator pass 4 (a regression from Amendment 3 and three smaller defects; all fixed test-first)

- **Function-valued declarations are definitions, not variables.** `const f = () => …`, `let g = function () {}` and class expressions are captured as definitions by the tags query, so they are no longer recorded as bindings. Amendment 3's bare-call rule had suppressed every same-file call to them (329 edges in hono; the changelog had wrongly credited the drop to `next()` calls).
- **Java methods are never shadowed by variables** (JLS §6.4.2): the bare-call rule does not apply to Java.
- **JavaScript `var` is hoisted:** it is in effect from the start of the enclosing function, so a call before the declaration is unresolved rather than linked to an outer function.
- **`staticmethod`, `classmethod` and `property`** are Python built-ins.
- **Accepted:** a Python method default argument referencing a class attribute (`def n(self, y=r.m2())`) resolves `r` in the method's scope, so a same-named module variable wins; defaults are evaluated in class scope, and this needs the parameter list's extent, which the facts do not record. A wrong edge is possible only when a class attribute shadows a module name and is used in a default.
- **Accepted:** `wrapped = deco(wrapped); wrapped()` is an unresolved call, since `wrapped` is a variable at the call.

## Amendment 5: evaluator pass 5 (a regression from Amendment 4 and three hoisting gaps; all fixed test-first)

- **Whether a function-valued declaration is a definition is decided by the resolver, not the extractor.** Every declarator is recorded as a variable again. A bare call through a variable is suppressed unless that variable is also a *kept* definition, meaning a symbol whose name lies inside the declaration. `const f = () => …` at module or named-function level resolves; the same inside a callback (where locals are not symbols) stays a variable and is unresolved, never linked to an import or module function of the same name. Generator and class expressions, which the tags query does not capture, behave the same way.
- **`var` hoisting** starts at the start of the enclosing function or file (not after its first statement), and applies to `for (var x of …)` loop variables.
- **Accepted (unresolved, never wrong):** `let k; k = () => …; k()`; a reassigned function-valued `let`; Go `f := func(){}` and Python `f = lambda` followed by `f()`; an anonymous `const K = class {}` never yields a type; a block-level `function` declaration shadows a module function for calls outside the block.
- **Accepted (precision):** Java overload fan-out is `ambiguous`; call-site arity is not yet used to narrow it.

## Amendment 6: evaluator pass 6 (PASS; two low-severity imprecisions fixed afterwards, covered by unit tests)

- A bare call through a function-valued variable resolves to **the definition the variable itself is**: the kept symbol declared by the same name token. It no longer re-runs the name lookup, so each site goes to the `h` in effect there rather than to every nested `h` (which had made them `ambiguous`), and a same-named property inside the value (`const run = { run: () => … }`) is not taken for the variable.

**M3a is closed.** Every R1–R13 assertion and every invariant 1–6 holds. Accepted limitations are listed in the amendments and carried into M3b (SCIP), which replaces heuristic edges with exact ones project by project.
