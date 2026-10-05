# M3b evaluator pass 12 (closing re-check of `4402377`)

- Date: 2026-10-06 (wall clock).
- Commit evaluated: `4402377` (`Fix M3b evaluator pass 11: structural escapes, index-confirmed supertypes`). The tree was clean at the start and at the end.
- Release binary: `cargo build --release -p rb-cli` at HEAD **rebuilt** the binary that was on disk (sha1 `ae5e5fcbb37b…`, dated before the commit, so before the guarded `related.get(&holder)`) into sha1 `95563450696e…`. Every number below comes from the rebuilt binary.
- Runtime:
  - Docker Desktop, with `DOCKER_CONTEXT=desktop-linux` set in every shell.
  - Images: `rb-scip-java:0.13.1` (`cbd9963468b4`) and scip-typescript `028c00429913`. Nothing was pulled.
  - Database `127.0.0.1:55433`.
  - Disk: 15 GiB free throughout, so no `cargo clean`.
- No tracked file was modified:
  - 14 mutants. Each was copied aside, edited by script and run with `cargo test -p <crate> --lib`; the copy was then put back and checked with `filecmp`.
  - `git status --short` was empty until this report was written.
- Throwaway material lived only under `var/eval-pass12/` and was removed at the end:
  - nine public clones;
  - scratch project `t12esc` (Maven, 33 files, its own git repository, generated with `chr(92)` and checked with `od -c`);
  - dumps, logs and scripts;
  - SCIP work dir `var/eval-pass12/scip`, seeded from a copy of `var/scip/cache`.
- Database cleanup at the end:
  - every `ev12_*` repository and its AGE graphs;
  - the `rb.scip_index` rows this pass created. The 19 rows present at the start were recorded by `(input_hash, indexer_digest)` and kept.
  - `fp3_*` was not touched.

## 1. Final-binary measurement

Each repository was run `--force --no-cache`, then `--force` (cached). Counts are `scip`-tier edges taken from the fresh dump.

| Repository @ commit | Fresh | Cached | Docs rejected | `supertypes_unconfirmed` | `CALLS` / `IMPLEMENTS` (exact) | Nodes / edges | fresh = cached |
|---|---|---|---|---|---|---|---|
| spring-petclinic @ `818c4136ea97` | 24 s | 0 s | 0 | 0 | **256 / 13** | 417 / 927 | `cmp` identical |
| class-validator @ `2e1a5c27dbd6` | 9 s | 1 s | 0 | 0 | **398 / 2** | 639 / 1,447 | identical |
| jsoup @ `088614f4a145` | 71 s | 1 s | 0 | 0 | **14,013 / 688** | 5,323 / 25,561 | identical |
| spring-retry @ `40f557cc27ab` | 40 s | 1 s | 0 | 0 | **459 / 252** | 1,823 / 5,793 | identical |
| json-schema-validator @ `95825f33f76b` | 44 s | 2 s | 0 | 0 | **6,449 / 731** | 4,301 / 14,515 | identical |
| spring-petclinic-rest @ `afc8fc1d3b8e` | 77 s | 1 s | 0 | 0 | **652 / 166** | 836 / 2,366 | identical |
| vavr @ `7791b586d290` | 90 s | 4 s | 0 | 0 | **27,257 / 5,994** | 13,442 / 59,934 | identical |
| TheAlgorithms/Java @ `68dc5f54dde2` | 35 s | 3 s | 0 | 0 | **10,374 / 269** | 13,054 / 39,575 | identical |
| routing-controllers @ `bf6fbfdc7fa5` | 12 s | 1 s | 0 | 0 | **169 / 38** | 642 / 1,640 | identical |

- Every expected number is reproduced with the HEAD binary.
- All nine cached runs printed `(cached)`.
- The dumps contain no `/Users`, `rohit`, `unix://`, `/private/` or `/var/folders`.

## 2. N1–N4 as amended

`t12esc` was run on the real image, `--no-cache --force` and then cached (`cmp` identical).
- `rb status`: `documents rejected 25 (structural_escape 24, structure_disagreement 1)`, `unmatched 0`, `definitions without occurrence 0`.
- The `structure_disagreement` document is my `Dot1` (see §4).

| # | Status | Evidence |
|---|---|---|
| N1 | **Fixed for every listed layout. The class is not closed (O1).** | `Enm3`, `Enm4`, `Enm7` are rejected with `structural_escape` and carry heuristic edges only. There is no exact `n → b` and no exact `IMPLEMENTS B3#n()`/`I4#n()`. The same layouts written with `\` instead of `*/` still give the identical false exact edges (O1). |
| N2 | Fixed | `Sup1` is rejected (`structural_escape`); its edges are heuristic only. Independently of escapes, the origin-confirmation is the second line, and mutant O1m below proves the unit covers it. |
| N3 | Fixed as documented | Amendment 71's TypeScript bullet now names `-->` and the `>>>>>>>` marker, with the pass-11 layouts and edges. It is accurate as a disclosure. |
| N4 | Fixed | MK2 is killed by `a_type_is_not_paired_by_a_method_named_like_it`. MK13 is killed by `tsx_and_javascript_documents_tree_sitter_cannot_parse_are_rejected`. Both were re-run this pass. |

### Earlier escape layouts

These are rebuilt from the generator's own unit sources:
- `Esc1–3`, `Hid1/2/3/5/6/7`, `S9`, `Hid10`;
- `Anon1–3`, `Loc1`, `Fld1`, `Init1`;
- `Enm1`, `Enm2`, `Enm5`.

All 24 escape files are `structural_escape`, and every edge in them is heuristic. There is no exact edge from any of them.

### Controls

All of these are accepted, with true exact edges:
- `Plain#m() → b()`.
- `Fut#m() → Fut#b()`, class keyed `p/Fut#`.
- `` Rec#`<init>`() → check() ``.
- `` Rec2#`<init>`(+1) → check() ``, and `new Rec2(1)` → `(+1)`.
- `` Rec3#`<init>`() → check() ``, with `new Rec3("a")` → `(+1)` and `new Rec3(1)` → `()`.
- My escape-free `EnmP` (constant body overriding `EnmP#n()`): exact `EnmP#n() → b()` and `IMPLEMENTS EnmP#n()`, both true.

`Enm1` is no longer accepted. It now carries `structural_escape`, so its true `n → b` is heuristic, not exact. That is a coverage cost of amendment 77, not an error.

### Mutants run this pass

| Id | Mutation | Result |
|---|---|---|
| P1 | parity ignored (any `\` begins an escape) | killed (`structural_escapes_reject_a_scip_java_document`) |
| P2 | parity replaced by "fewer than 2 preceding `\`" | killed (same unit) |
| U1 | exactly one `u` | killed |
| C1 | `'` dropped from the set | killed |
| C2 | `<` and `>` dropped | killed |
| C3 | `,` dropped | killed |
| C4 | LF/CR dropped | killed |
| C5 | `.` added | killed (the `case '.'` assertion) |
| T1 | code truncated to `u8` (`Ī` read as `*`) | killed |
| **R1** | a backslash produced by `\` counts towards the next escape's eligibility, so `\*` is not read as an escape | **SURVIVED** (O2) |
| O1m | no origin confirmation in `reaches_supertype` | killed (`a_supertype_the_index_does_not_relate_is_not_walked`) |
| MK2 | a type paired by a method symbol | killed |
| MK13 | syntax-error rule for `Language::TypeScript` only | killed |

## 3. The amendment-78 deviation (confirm each step against the origin)

**Judgement: sound, given that the index's type relationships are truthful and transitive.**

**Argument.** `reaches_supertype` (`crates/rb-indexer/src/scip.rs:1126`, check at `:1140`) leads on from any node only through a symbol in `related`. `related` is the origin holder's own `SymbolInformation` relationships (`:586`).
- Every type the walk reaches is therefore one of the origin's related types.
- The walk accepts only when it reaches the relationship target's type definition, so that target is related to the origin.
- scip-java relates a type to all its supertypes transitively, and scip-typescript does the same for a class. So origin <: target, and the direction is the true one.
- For the reverse twin of an override, the origin is the supertype. It is not related to its subtype, so it cannot step onto it.
- Tree-sitter clauses now only *restrict* which related types are visited. They cannot add one.

**Attacks tried, by construction against the code:**
- **An intermediate type with a clause only Tree-sitter sees.**
  - If the origin is truly related to the far type, the emitted direction is true anyway.
  - If it is not, the step is refused.
- **Same-named types in a chain.** Same-named types in different packages or files have distinct symbols. `types_by_symbol` keys on the full symbol, and confirmation is by symbol.
  - Two definitions sharing one symbol (TypeScript declaration merging) both need the origin's relation.
- **TypeScript interface chains where an intermediate `extends` exists only for Tree-sitter.** A Tree-sitter-only TypeScript clause is possible only through the documented `-->` and conflict-marker vectors. Even then, the step needs the origin class to be related to the far interface, which scip-typescript states only when it is true.
- **A Java Tree-sitter-only clause via `\` (O1's trick).** The clause's supertype name sits inside javac's string literal, so it has no index occurrence. It is `unresolved` before confirmation is reached.

**Cost (suspected, not measured).** A TypeScript *interface* origin relates to nothing, so a TypeScript interface method overriding a super-interface method gets no exact `IMPLEMENTS` at all. The edge sets on class-validator and routing-controllers are unchanged, so the cost there is 0.

## 4. Soundness probe (time-boxed: four ideas)

1. **`\` (backslash) inside a string literal: reproduced, O1 (major).**
2. **`.`, `@` or `:` alone.**
   - `Dot1.java` (`o.b();`): Tree-sitter-java cannot read a backslash in code. It gives a `syntax_error` at `[4,14,4,20]`, and amendment 72 rejects the document (`structure_disagreement`).
   - Inside a comment, string or char literal, these three characters cannot move a delimiter.
   - I found no way for any of them alone to give a false exact edge. Leaving them out is safe.
3. **Escapes in identifiers and other non-structural characters.** These also make Tree-sitter error in code, which rejects the document. Not pursued further.
4. **TypeScript.** Only the documented `-->` and `>>>>>>>` vectors are known. I did not build new TypeScript layouts this pass.

## New findings

### O1: major. A `\` escape (backslash) inside a string literal restructures the file for javac only. It reproduces N1's false exact `CALLS` and `IMPLEMENTS` with no escape from the structural set.

**Cause.**
- `has_structural_escape` (`crates/rb-scip/src/decode.rs:340`, set at `:341`) leaves `\` (U+005C) out of the set.
- The unit even asserts it as harmless (`:1403`, `"String s = \"§u005c\";\n"` → accepted, listed among "escapes javac and Tree-sitter agree on").
- They do not agree. javac translates `\` to a raw `\` before lexing (JLS §3.3), and that backslash then escapes the following `"` inside the literal. Tree-sitter reads `\` as a complete escape sequence, and the `"` closes its string.
- So `"\"; /*"` is one string literal for javac (`"; /*`) and, for Tree-sitter, a string followed by a block comment. This gives exactly the (Tree-sitter comment, javac code) region that `*/` gave in passes 8–11.
- Every downstream check that N1 already beat (`definitions_unpaired`'s enum-constant-body rule, `scip.rs:860`–`879`) is passed the same way.

**Repro.** Real image, `t12esc`, `--no-cache --force` (and cached, identical). UTF-8 pom, ASCII sources. Both documents are accepted (no `syntax_error`). Row 3 of each:

| File | Row 3 (`\` written with `chr(92)`) | javac reads (from the index) | Tree-sitter reads | Exact edges emitted (false) |
|---|---|---|---|---|
| `Bs7.java` (`enum Bs7`, `static void b() {}`) | `  A { String s = "\"; /*"; { new Object() { /* */ void n() { b(); } String t = "\"; /*"; }; } /* */ } ;` | constant body: field `s` (`local 2` `[3,13]`), initializer `{ new Object() { void n() { b(); } String t = …; }; }`; `n` is `local 4`, kind `Method`, `[3,60]`, a method of an anonymous class; `Bs7#A.` constant | `A { String s = "\"; void n() { b(); } String t = "\"; }`: a constant-body method `n` | `` `src/main/java/p/Bs7.java`/Bs7#n(). CALLS p/Bs7#b() `` `[3,66,3,67]` |
| `Bs4.java` (+ `interface J4 { void n(); }`) | the same with `new J4() { /* */ public void n() { b(); } …` | anonymous `J4` in the initializer; `local 4` `[3,63]` has `is_implementation` → `J4#n().` | constant-body method `n` of `Bs4` | `Bs4#n() CALLS p/Bs4#b()` `[3,69]` **and** `Bs4#n() IMPLEMENTS p/J4#n()` `[3,63]` |

Index occurrences were read straight from the stored `rb.scip_index` blob:
- `Bs7`: `p/Bs7#A.` `[3,2]`, `local 2` `[3,13]`, `java/lang/Object#` `[3,38]`, `local 4` (def) `[3,60]`, `p/Bs7#b().` `[3,66]`, `local 5` `[3,80]`.
- `Bs4`: `p/J4#` `[3,38]`, `local 4` def `[3,63]` relating to `p/J4#n().` (implementation + reference), `b` `[3,69]`.
- `rb status` counts these two `n` among `3 local definitions` and shows nothing else: `unmatched 0`, `definitions without occurrence 0`.

**Observed vs expected.**
- `Bs7` and `Bs4` have no method `n`. javac's `n` belongs to an anonymous class in a constant's initializer block, whose calls design §8.2 credits to the enum.
- `Bs4 IMPLEMENTS J4` is false.
- This is N1 again, with the delimiter moved by a backslash instead of `*/`.

**Why major.**
- It is a false exact `CALLS` and `IMPLEMENTS`, and it is undocumented.
- Amendment 71's K1 line says that since amendment 77 "this whole class (… every `*/`, `/*`, brace, parenthesis, **quote** or terminator javac alone sees) is covered primarily by `structural_escape`". Here a quote that only Tree-sitter treats as closing is not covered.
- Its list of escapes "left to the second line" (`.`, `@`, `:`, identifiers, `-`, `!`, controls) does not name the backslash.
- The unit's comment explicitly claims javac and Tree-sitter agree on it.
- It is not in the uncovered list. Realism is as low as N1's, which was graded major.

**Suggested fix (calibrated here).**
- Add `\` to the set, but exempt an escaped backslash followed by `u`. javac reads `\u0131` as a backslash and the text `u0131`, which is illegal in a literal and inert in a comment.
- Measured, all `.java` files: `\` escapes not followed by `u` are **0** on spring-petclinic (50), jsoup (205), spring-retry (160), json-schema-validator (425), spring-petclinic-rest (109), vavr (256: all 10 are Javadoc `\u…`) and TheAlgorithms (1,634).
- Add a regression unit with `Bs7`/`Bs4` (decoder level: rejected; join level with the measured occurrences above: rejected or no `n → b`), and drop `"§u005c"` from the "agree" list.
- Correct amendment 71's K1 line and amendment 77.

### O2: minor (test gap). The produced-backslash rule of amendment 77 is untested.

Mutant R1 makes a backslash produced by `\` count towards the next escape's eligibility (`decode.rs:362`). With it, `\*` (javac: `\` `*`) is not read as an escape, and no unit fails. Amendment 77 states this behaviour ("`\*` is read as an escape") but tests none of it. Add `\*/` → `structural_escape`.

## Regression

| Check | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| Unit tests (no database), summed | **333 passed**, 0 failed: rb 9, rb-frameworks 28, rb-git 26, rb-indexer 124, rb-model 11, rb-scip 38, rb-sql 27, rb-store 12, rb-syntax 58. The 15 e2e tests are no-ops without the variable, for 348 in all. |
| e2e with database and containers | **15/15** (49.6 s) and **15/15** (45.6 s). No `skipping:` and no flake. |
| Host paths in dumps | 0 (ten repositories, fresh and cached) |
| Stub scan | only the known SQL-string `TODO` at `crates/rb-sql/src/lib.rs:1111`; no `todo!`, `unimplemented!` or `allow(dead_code)` |
| `PIPELINE` | `m3b-scip-7` (`crates/rb-indexer/src/lib.rs`) |

### Docs vs code

- **Amendment 77:**
  - The set, the parity rule, multiple `u`, either case, the ordering (after `comment_terminator`) and TypeScript exemption all match the code.
  - The 0-of-7-repositories calibration is consistent with my 0 rejections.
  - **False:** the implied safety of the backslash (O1).
  - **Untested:** the "`\*` is read as an escape" sentence (O2).
- **Amendment 78:** matches the code (`:586`, `:1140`). The deviation is stated, and §3 judges it sound. The "0 unconfirmed on nine repositories" claim is reproduced.
- **Amendment 79:** the units exist and kill MK2 and MK13. The TypeScript bullet is accurate.
- **Amendment 71's table:**
  - The K1 line's "every … quote … javac alone sees" is **false** (O1).
  - The other lines (tabs, terminators, `not_utf8`, encodings, Shift_JIS `Sj3` open, TypeScript, Gradle) are not contradicted by anything this pass. Shift_JIS was not re-run.
- **`decode.rs:1396`–`1403`:** the unit comment and assertion calling a backslash escape one "javac and Tree-sitter agree on" are false (O1).

## Closing judgement

| # | Verdict | Evidence |
|---|---|---|
| S1 | PASS (documented vector open) | Decoder unchanged except the new reason. Shift_JIS stays open as amendment 71 documents. |
| S2 | PASS | Maven and npm detected on all nine repositories and `t12esc`. |
| S3 | PASS | 20 runs `ok`, no failures. |
| S4 | PASS | Every cached run is `(cached)`. |
| S5 | **FAIL** | O1: false exact `Bs7#n() CALLS b()`, `Bs4#n() CALLS b()` and `Bs4#n() IMPLEMENTS J4#n()`. Undocumented, and contradicting amendment 71's K1 line. |
| S6 | PASS | e2e twice. |
| S7 | PASS as worded | The 25 rejected `t12esc` documents carry heuristic edges only. Accepted documents have no heuristic code edges. |
| S8 | PASS | Counts and dumps as the baseline. |
| S9 | PASS | `supertypes_unconfirmed` is printed and recorded (0 on all nine), and the rejection reasons are broken down. |
| S10 | PASS | Goldens pass in both e2e runs. |
| S11 | PASS | All nine public counts match exactly. Precision was not re-sampled this pass (the edge counts are unchanged from pass 11's 120/120 and 52/52). |
| S12 | PASS | fresh = cached on nine repositories and `t12esc`. |
| S13 | **FAIL** | fmt, clippy, tests and the stub scan are clean, but one major is open (O1) and one mutant survives (O2). |
| Inv. 1 | PASS as worded | Never a mix within a file. |
| Inv. 2 | **FAIL** | O1: three false exact edges in the `scip` tier. |
| Inv. 3 | PASS | No host paths. Network phases are unchanged. |
| Inv. 4 | PASS | No failures. Pass-10 evidence stands. |
| Inv. 5 | PASS | As S12. |
| Inv. 6 | PASS | The O1 spans are the tokens' own ranges. |

**Follow-ups for later milestones** (after O1 is closed):
- O2 unit.
- TypeScript interface-origin `IMPLEMENTS` coverage (amendment 78 cost, not yet measured on an interface-heavy TypeScript repository).
- Already open: the effective source encoding (Shift_JIS), TypeScript restructurings Tree-sitter parses cleanly (`-->`, `>>>>>>>`), Gradle (M6), the spring-retry test tree (Develocity), the M6 build recipes, workspace installs, tsconfig include/exclude, fetch-phase egress.

## Verdict

VERDICT: FAIL. One major is open:
- **O1:** a `\` escape in a string literal restructures a scip-java document for javac only. It is outside amendment 77's set and is explicitly asserted harmless. It gives false exact `Bs7#n() CALLS Bs7#b()`, `Bs4#n() CALLS Bs4#b()` and `Bs4#n() IMPLEMENTS J4#n()`, which is N1's shape. The fix is cheap and calibrated at 0 documents on seven Java repositories: add `\` to the set, exempting `\` followed by `u`.

One minor: O2 (untested produced-backslash rule, mutant R1).

What holds:
- N1–N4 are fixed for every listed layout.
- The origin-confirmation deviation is sound.
- The final-binary measurements match on all nine repositories, with 0 rejected, 0 unconfirmed, and fresh = cached.
