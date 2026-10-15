# M8a phase 1 (S1–S3): evaluator pass 1 (2026-10-13)

**VERDICT: FAIL, 2 blocking** (plus 4 should-fix, 4 notes).

Scope: crates/rb-bench/src/{lib,dump,identity,score}.rs against S1–S3, Identity, Core algorithm and NFR 3. Temporary tests (removed) probed 17 goldens, 31 M7-close invariant dumps (var/m7j/inv/*.head.jsonl) and 10 M6b scip/noscip dumps. Tests 15/15; clippy and fmt clean. `bench/` and `src/{compare,render,scoreboard}.rs` appeared mid-pass and are not reviewed.

## Findings

1. **Blocking: the ambiguous hit rate misses gold candidates that are also resolved elsewhere.**
   - resolve.rs:1353 merges an edge to its maximum certainty. When `f→g` is ambiguous at line 2 (alongside `h`) and unique at line 3, the dump holds a single `heuristic` row for `f→g` with both sites.
   - score.rs:124-128 takes candidates only from edges whose merged certainty is `ambiguous`. Line 2 therefore sees only `h`.
   - Repro with gold {f→g}: `sites=1 hits=0`. S3 requires 1/1.
   - Fix: make two passes. First collect the `(src, site)` keys of in-scope ambiguous edges. Then, for every in-scope found edge of any certainty, OR its `is_gold` into each of its collected sites. Add the repro as a unit test.
2. **Blocking: S1's aliases don't exist.**
   - identity.rs:82-94 keeps the minimum site and records no aliases. The collision check ignores the other sites.
   - Merged declarations do occur: mattermost noscip has 4 (e.g. `products.d.ts`/`Plugin#`, lines 10, 27).
   - Under SCIP, declarations merge across files. A heuristic key at a non-lowest site then never matches and scores fp+fn.
   - Fix:
     - Store `alias (path,line,name) → canonical Loc` and expose `canonical(&Loc)`.
     - Count aliases in `holders`.
     - Assert the alias in the merged-declaration unit test.
     - Or amend S1 instead.
   - Document that "lowest" is `(path, span)` order (identity.rs:91).
3. **Should-fix: "Tier B never emits file-sourced CALLS" is false.**
   - The claim appears at identity.rs:40 and in the contract's PyCG scope.
   - resolve.rs:38-41 assigns unenclosed calls to the file. Noscip `file/heuristic` CALLS counts: mattermost 3,188, bookcars 245, express-ts 17, microservices-demo 10, fastapi-realworld (Python) 1.
   - Fix: correct the comment, and amend the PyCG scope rule before phase 5.
4. **Should-fix: a CALLS row from any other source kind lands in no bucket.**
   - identity.rs:52-56 selects only `symbol` and `file` sources. In a repro, an `endpoint` source gave length 0 in both classes, which breaks NFR 3.
   - None exists in the 58 dumps today.
   - Fix: `Calls` selects `kind == Calls && src_kind != File`. Other kinds then reach `not_locatable`.
5. **Should-fix: the ambiguous-site unit differs from the contract.**
   - The LLD says "per evidence path:line". The code keys a site by `(source Loc, full span)` (score.rs:113,126), and `one_line_in_two_sources_is_two_sites` tests exactly that.
   - The code is the better unit. Fix: amend the LLD.
6. **Should-fix: a negative span is silently located at line 0, the file's line.**
   - identity.rs:137-139 `map_or(0, …)`. A DEFINED_IN span of `[-3,0,-3,1]` gives `Ok(Loc{line:0,…})`.
   - Fix: reject negative coordinates in `evidence_item` with a `Row` error.
7. **Note: D rows are checked for arity, not shape.**
   - dump.rs:40 says "checked for shape", but :65 checks only `len()==5`, so `["D",1,2,3,4]` is accepted.
   - Fix: say "arity", or check the field types.
8. **Note: the reader holds.**
   - All 58 dumps parse, including kotlin-gradle/scip-dump. Every N and E row re-renders byte-identically. mattermost: 177 ms.
   - A blank line, a BOM, an extra header field, a 3-element span, i32 overflow and a bad rank each return a typed error. No panic path found; the asserts hold by construction for a pure `scope`.
   - The in-tree golden test checks one fixture's E rows only; extend it to N and E rows of every golden.
9. **Note: location identity counts** (no_defined_in / location_collision / unknown_node).
   - Goldens: 0/0/0 in every fixture. All 302 DEFINED_IN spans are exactly the name token, checked against the `fixtures/` sources.
   - mattermost: 0/6/0. The 6 are nested same-name functions in the minified `.github/actions/save-junit-report-tms/dist/index.js`; they leave 3 CALLS unmatchable. Every other real dump is 0/0/0.
   - Prisma model spans cover the whole line (orm.rs:228-231); the identity is unaffected.
   - Cross-mode: every M6b noscip EdgeId is also in its scip dump (4/4, 7/7, 5/5, 2/2 file-scope, 2/2).
10. **Note: the mutants can't be re-run.**
    - todo.md:1115 gives only "m151–m162". Fix: list each (file:line, change, killing test) in the contract Result.
