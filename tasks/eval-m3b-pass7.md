# M3b evaluator pass 7

- Date: 2026-10-01 (wall clock, 18:28–19:25)
- Commit evaluated: `82f34d4` (`Fix M3b evaluator pass 6: indexer line terminators, name guard`), clean tree.
- Runtime: Docker Desktop via `DOCKER_CONTEXT=desktop-linux` in every shell; `rb-scip-java:0.13.1` = `sha256:cbd9963468b4…`, scip-typescript `sha256:028c0042…`; database `127.0.0.1:55433`. Disk 15 → 13–14 GiB free (no `cargo clean` needed).
- Nothing tracked was modified. Mutations: file copied aside, edited, `cargo test`, copied back; `git diff --quiet` checked after each (22 mutations, all restored). Throwaway material lived under `var/eval-pass7/` only: the four clones, scratch projects `t7`, `t7ts`, `t7bom`, `t7lat`, `t7lts`, `t7enc` (each its own git repo), a standalone probe crate (`[workspace]` of its own, path dependency on `rb-scip`, its own target dir), exported indexes, dumps, logs. **SCIP work dir `var/eval-pass7/scip` started empty (isolated `.m2`/`.npm`, not seeded from `var/scip`)**; the Develocity experiment used a second empty work dir `var/eval-pass7/scipD`. All removed at the end with the `ev7_*` repositories, their AGE graphs and the 11 `rb.scip_index` rows this pass created (the 17 pre-existing rows were listed before the first run and left alone). `fp3_*` untouched.

## Baseline (measured)

| Check | Result |
|---|---|
| `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` without a database | 300 unit tests (rb 9, rb-frameworks 28, rb-git 26, rb-indexer 99, rb-model 11, rb-scip 31, rb-sql 27, rb-store 12, rb-syntax 57). **`resolve::tests::many_declarations_of_one_name_stay_fast` failed three times in a row (2.1–2.3 s against its 1.5 s wall-clock budget, debug build, host load average ≈ 6.5 from unrelated desktop apps)** and passed in every later run once the load dropped. Pre-existing (M3a, `resolve.rs` untouched since `9a8ec74`); recorded as a follow-up, not an M3b defect. |
| `RB_TEST_DATABASE_URL=… cargo test -p rb-cli --test e2e`, twice, containers | 15 passed / 15 passed, 75.8 s and 62.9 s; no `skipping:` line. |
| spring-petclinic @ `818c4136ea97`, `--no-cache --force`, empty `.m2` | `scip-java ok 281.5s` (cold Maven download); 240/240 (1 by name), without occurrence 0; 338 / 1,220 / 0 / 0 / 39 / 0 / 0; `relationships dropped 42 (0 in project, 0 anonymous)`; **256 `CALLS`, 13 `IMPLEMENTS`**; 417 / 927 / 67. |
| class-validator @ `2e1a5c27dbd6` | `scip-typescript ok 178.6s` (cold npm); 417/424 + 6 local, without occurrence 1; 625 / 366 / 0 / 4 / 24 / 2 / 0; `0 (0, 0)`; 42 files without a document; **398 / 2**; 639 / 1,447 / 41. |
| jsoup @ `088614f4a145` | `scip-java ok 385.0s`; 4874/4991 + 117 local, without occurrence 0; 20,624 / 12,113 / 6 / 2 / 104 / 0 / 0; `554 (0 in project, 52 anonymous)`; **14,013 / 688**; 5,323 / 25,561 / 1. |
| spring-retry @ `40f557cc27ab` | `scip-java ok 43.2s`; 724/724, without occurrence 0; 614 / 710 / 0 / 0 / 64 / 0 / 0; `157 (0 in project, 1 anonymous)`; 68 files without a document; **459 / 252**. |
| Determinism | fresh (`--no-cache --force`) = cached (`--force`) dump, `cmp`, on all four; a plain run afterwards is "up to date"; snapshots record `pipeline: m3b-scip-2`. spring-retry: see attack 4. |

Every number matches the orchestrator's baseline.

## Criteria

| # | Verdict | Evidence |
|---|---|---|
| S1 | **FAIL** | Line terminators are now right (attack 2: every adversarial terminator case converts onto its token on both real images). But the decoder still assumes the indexer read the blob as UTF-8: a byte that is not UTF-8, or a build whose source encoding is not UTF-8, shifts every later occurrence on the line onto other tokens (J1). A TypeScript file with a BOM loses every line-0 occurrence (J3). |
| S2 | PASS | Maven and npm scratch projects detected; `t7bom` (BOM Java) fails in the fetch phase as javac rejects it. |
| S3 | PASS | `failed (fetch phase)` on `t7bom`, exit 0, Tier B. |
| S4 | PASS | `(cached)` re-runs on all four repositories; the e2e suite twice. |
| S5 | **FAIL** | J1: `Lat#q() CALLS Lat#z()` and `q() CALLS A#z()` are exact and false. J2: an exact `CALLS` with the wrong caller when javac ends a `//` comment that Tree-sitter does not. J4: the name guard accepts a symbol named `zzzzzz` at the token `helper`. |
| S6 | PASS | Not re-run beyond the e2e. |
| S7 | PASS as restated | Four repos: heuristic `Symbol` keys in documented files = `definitions_local` + `definitions without occurrence`. But J2's lost definition (`evil`) and edge (`good → a`) are counted nowhere. |
| S8 | PASS | Package counts unchanged. |
| S9 | PASS, with a nit | New fields printed. `rb status` on a snapshot built before amendment 62 prints `definitions without occurrence 0` next to `matched 417/424` with no local definitions (`fp3_class_validator`), because a missing field reads as 0 (`crates/rb-cli/src/main.rs:645`). |
| S10 | PASS | Goldens pass on two container runs. |
| S11 | PASS | Precision samples below: 80/80 `CALLS`, 32/32 `IMPLEMENTS`. |
| S12 | PASS | fresh = cached on all four repositories. spring-retry: the same dump across an empty `.m2`, a warm `.m2` + `.develocity`, `.develocity` deleted, and the shared work dir (attack 4). |
| S13 | **FAIL** | fmt, clippy, stub scan clean. Tests are clean except the M3a timing flake. Two major findings (J1, J2). |
| Inv. 1 | PASS | 0 heuristic code edges in documented files on the four repositories. |
| Inv. 2 | **FAIL** | J1 (false exact `CALLS` from a shifted occurrence), J2 (exact `CALLS` with a wrong source). |
| Inv. 3 | PASS | 0 hits for `/Users`, `rohit`, `unix://`, `/private/`, `/var/folders` in all dumps. (The Develocity extension writes `build-scan-data` and a `token-cache` into the shared `.m2` during the fetch phase; that is the known egress risk of design §14, not new.) |
| Inv. 4 | PASS | `t7bom` failed in fetch, exit 0, Tier B. |
| Inv. 5 | PASS | As S12. |
| Inv. 6 | **FAIL** | J1: occurrences are joined at spans the indexer did not mean (another token on the same line, or the next row). |

## Pass-6 findings I1–I5

| # | Status | Evidence (incl. mutation) |
|---|---|---|
| I1 | **fixed for line terminators; the defect class survives through source encodings (J1) and comment terminators (J2)** | Real images. Java `t7`: lone CRs between members and at EOF with no LF (`LoneCrEof`), `\r\r\n` everywhere (`CrCrLf`), raw `\r` and `\r\n` inside two text blocks with calls after the closing `"""` (`TextBlock`), no trailing newline (`NoNewline`), a 180,141-byte line of `é😀\tq ` runs inside a comment followed by two methods (`LongLine`, evidence at byte column 180,047). TypeScript `t7ts`: lone CR at EOF and `\r\r\n` (`eof.ts`), raw `\r`, U+2028 and U+2029 inside a template literal with `${this.a()}` (`tmpl.ts`), U+2028 and `\r` in JSX text between `{f()}` and `{g()}` (`jsx.tsx`), a regex with an *escaped* ` `, a 180 kB line (`long.ts`). Every exact edge of those files is correct. The probe (real `rb_scip::decode` + text at every single-line span) finds every occurrence on a token its symbol may name: `t7` 108 OK, plus 3 `<init>` at `this(…)` and enum constants (forms Tree-sitter never captures); `t7ts` 67 OK, plus the 5 BOM occurrences of J3. Four public repositories: petclinic 5,693 OK / 0 bad; jsoup 77,419 / 154 `<init>` + 3 namespaces; spring-retry 8,096 / 57 `<init>`; class-validator 5,092 OK, with 141 import-alias sites, 572 module specifiers and 3 `<constructor>` (as amendment 59 says); **0 positional**. Mutations (each run checked for the failing test): Java ignores a lone CR (M1), TypeScript ignores U+2028/U+2029 (M2), Java counts U+2028/U+2029 (M3), CRLF counted as two breaks (M4), U+2029 not a break (M5), TypeScript read with javac's rule (M6), Tree-sitter rows also split at CR (M7): each fails `indexer_lines_follow_the_indexers_line_terminators`. |
| I2 | fixed | Mutation `locals[di].contains_key(symbol)` → `false` fails `typescript_local_to_local_relationships_are_counted_in_project`. No real-image re-measurement. |
| I3 | fixed (unit) | Mutation `name_and_span` always takes raw text: `comments_inside_a_qualified_supertype_name_are_skipped` fails. Edge case: `t.@Ann CBase` (an annotation inside a scoped name) falls back to raw text. Not re-measured on the image. |
| I4 | fixed, nit | jsoup `52 anonymous`, spring-retry `1 anonymous`, class-validator `without occurrence 1`. Mutations: anonymous never counted → `relationships_dropped_in_project_are_counted` fails; `definitions_without_occurrence = 0` → 25 tests fail. Nit: S9 row (old snapshots print 0). |
| I5 | fixed in design §13 and `PIPELINE`; one stale heading | Snapshots record `m3b-scip-2`; design §13 numbers match the measurements. `IMPLEMENT.md:241` still reads "complete (pending the evaluator's pass-5 re-check)". |

Name-guard mutations (`names_token`, `may_name`). These fail tests:
- every global occurrence accepted (M8), the guard removed from definitions (M20), the guard removed from references (M21): each fails `an_occurrence_must_name_its_token`;
- a constructor accepted at any token (M10), any accessor (M13), any meta suffix (M11), the numeric suffix on any descriptor kind (M12): each fails `a_symbol_may_name_its_token_in_the_measured_forms_only` (M11 and M12 fail two tests).

**These survive (0 failures):**
- M9: a local symbol accepted whatever its display name;
- M22: the guard removed from local definitions;
- M14: the default-import alias rule deleted outright, although amendment 59 measured 71 such sites on class-validator;
- M15: the alias rule no longer checking that the token is the import's local name.

See J4.

## New findings

### J1 — major — the decoder converts columns as if the indexer had read the file as UTF-8, so a non-UTF-8 byte, or a build with a non-UTF-8 source encoding, puts occurrences on other tokens (false exact `CALLS`)

**Cause 1 (byte offsets).** `byte_column` (`crates/rb-scip/src/decode.rs:298–300`) decodes the line with `String::from_utf8_lossy` and returns `char_indices()` offsets, which are offsets into the *lossy* string. Each invalid byte becomes U+FFFD: one code unit (counted right) but three bytes, while the original is one byte. Every later position on that line moves 2 bytes right per invalid byte. `Lines::position` (`:273–276`) then adds that offset to the raw line start, so a position can also cross the `\n` onto the next row.

**Cause 2 (encodings).** javac reads the sources in the build's declared encoding (`project.build.sourceEncoding`). A UTF-8 `é` in an ISO-8859-1 build is two UTF-16 units for javac and one for the decoder, so every later position on that line lands one character right per such character.

**Repro A, Latin-1 byte, `t7lat`.**
- Setup: `pom.xml` with `<project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding>`. `Lat.java` line 6 is `    /* ` + 12 × `0xE9` + ` */ int p() { return z(); } int q() { return z(1); }`, with `int z()` and `int z(int x)` declared above.
- Observed on the real image: `rb-heuristic …Lat#q() CALLS scip-java … t/Lat#z().` (`scip`, 5), evidence `[5,64,5,65]`. That is the `z` of `z(1)`, so the target should be `z(+1)`. The edge is false.
- Also: `p → z()` and `q → z(+1)` are lost, and `p`, `q` keep heuristic keys.
- The probe shows `Lat#p()`'s definition at the `q` token (`[5,51]`), and `Lat#q()`'s definition and the `z(+1)` reference pushed onto the **next row** (`[6,3]`, `[6,16]`).
- `rb status` shows only `definitions without occurrence 2, unmatched 1`.

**Repro B, TypeScript, `t7lts/src/lat.ts`.**
- Setup: `class A { z() }`, `class B { z() }`, then `/* ` + 21 × `0xE9` + ` */ export function p(a: A) { return a.z(); } export function q(b: B) { return b.z(); }`.
- Observed: `q() CALLS A#z()` exact, evidence `[2,105,2,106]`, which is `b.z()`. The edge is false. The probe puts `B#z()` on the next row (`[3,35]`, the text `A`).

**Repro C, valid UTF-8 with an ISO-8859-1 build, `t7enc`.**
- Setup: `Enc.java` is valid UTF-8, with 24 × `é` in the comment instead of `0xE9`; same layout as repro A.
- Observed: `Enc#q() CALLS Enc#z()` exact and false, evidence `[5,100,5,101]`.
- This needs no invalid byte, only a legacy build setting that is common in older Maven projects.

**Expected.** An occurrence is mapped to the token the indexer meant or counted unconverted or unmatched (invariants 2 and 6). Map `(indexer line, column)` → offset over the raw bytes. Count units the way the indexer decoded them: a replaced invalid sequence is one unit spanning its own bytes. When the indexer's encoding is unknown or differs from UTF-8, refuse (count `unconverted`) rather than guess. For scip-java the declared `project.build.sourceEncoding` is readable from the effective POM; at minimum, a file that is not valid UTF-8 should leave its positions unconverted.

**Prevalence.** 0 non-UTF-8 Java/TS files in the four public repositories. Their builds are UTF-8. Reachable from legal sources and legal builds, like I1.

**Regression tests.**
- `rb_scip::decode` unit: a line `"/* \xE9\xE9 */ a(b())"` under both tools' metadata, expecting raw byte spans and no row change.
- Join unit: the `Lat` layout, expecting no `q → z()`.
- An encoding test for an ISO-8859-1 Maven build (measured on the image) or an explicit `unconverted` rule.

### J2 — major — javac ends a `//` comment where Tree-sitter does not (a lone CR; a `\u000a` escape), so the indexer's code inside Tree-sitter's comment moves Tree-sitter's method boundaries, and an exact `CALLS` gets the wrong caller

**Repro, `t7/src/main/java/t/CommentCr.java`, real image.** Line 4 is `\tint good() { // note\r return a(); } int evil() {`, line 5 is `\t\treturn b();`, then `}`, `int a()`, `int b()`.
- The JLS (§3.4) makes the lone CR a line terminator, so javac compiles `good() { return a(); }` and `evil() { return b(); }`.
- The index agrees: a definition occurrence `CommentCr#evil().` at `[3,41]` with enclosing range `[3,37,5,2]`, a reference to `a()` at `[3,30]`, and `b()` at `[4,9]` inside `evil`'s range.
- Tree-sitter-java's `line_comment` runs to `\n`, so its `good` contains `return b();` and there is no `evil` definition.
- Observed: `CommentCr#good() CALLS CommentCr#b()` (`scip`, 5), evidence `[4,9,4,10]`. This is false (`good` calls `a()`, `evil` calls `b()`).
- `good → a` is lost. `evil` has no node. Nothing is counted: `rb status` shows `unmatched 0`, and the only definition without an occurrence comes from `UEsc`.

**Same through a Unicode escape, `UEsc.java`.** The line is `// note \u000a return a(); } int evil() {` (JLS §3.3 processes escapes before comments). Observed: `UEsc#good() CALLS UEsc#b()`, exact and false.

**TypeScript is not affected.** Tree-sitter-typescript ends `//` comments at `\r`, U+2028 and U+2029 like the TypeScript scanner: `cmtcr.ts`, `cmtls.ts`, `cmtps.ts` all give `good → a`, `evil → b`.

**Realistic shape.** A file with mixed line endings where CR-only lines follow a `//` comment, e.g. `void a() {\r  // note\r}\r void b() {\n  c();\n}`. Tree-sitter then attributes `c()` to `a`.

**Why this is I1's class.** It is the same class as I1 (a line terminator javac counts and Tree-sitter does not), surviving in the one place amendment 59 did not look: the structure Tree-sitter derives, not the positions. The source of an exact edge is Tree-sitter's enclosing definition, never checked against the index.

**Expected.** No exact edge whose source the indexer contradicts. The index gives definition occurrences with enclosing ranges, so a reference can be checked against the innermost SCIP definition range containing it, and counted unmatched when that definition is not the Tree-sitter enclosing definition's symbol. Alternatively, treat a document whose Tree-sitter comments contain an indexer line terminator (lone `\r`, `\u000a`/`\u000d` escapes) as undocumented.

**Location and test.** Location: `crates/rb-indexer/src/scip.rs:359–415` (`source(r.enclosing)`). Regression: a join unit with the `CommentCr` occurrences (the `evil` definition with its enclosing range, `b` at `[4,9]`) and Tree-sitter facts that put `b` in `good`, expecting no `good → b`.

### J3 — minor — a UTF-8 BOM in a TypeScript file loses every line-0 occurrence

**Cause.** TypeScript's `sys.readFile` strips the BOM, so scip-typescript's line-0 columns exclude it. The decoder converts against the blob, where the BOM is one UTF-16 unit (three bytes), so every line-0 position lands one character left.

**Repro, `t7ts/src/bom.ts`.** Line 0 is `﻿export function aa() { return bb(); } export function bb() { … cc() } export function cc() …`.
- The probe shows every line-0 occurrence one character left: `aa()` at `" a"`, `bb()` at `" b"`.
- Result: `aa`, `bb`, `cc` keep heuristic keys (`definitions without occurrence 3`), `aa → bb` and `bb → cc` are lost, and line 1's `dd → aa`, `dd → bb` are `unanchored`.
- No false edge: a one-character shift of an identifier span cannot equal another identifier's span.
- Java with a BOM does not compile (`t7bom`: `failed (fetch phase)`).

**Fix and test.** Fix: skip a leading BOM when counting scip-typescript's line-0 columns (measure it on the image). Unit: decode `"\u{FEFF}export function aa() {}"` with scip-typescript metadata.

### J4 — minor — the amendment-59 name guard is vacuous for scip-typescript locals and accepts any symbol at a default-import name; its permissive forms are partly untested

**Rule.** `names_token` (`crates/rb-indexer/src/scip.rs:551–566`):
- It accepts a local occurrence whenever the local has no display name (`is_none_or(|n| n.is_empty() || …)`). scip-typescript never gives one (amendment 60), so the second line of defence does not exist for TypeScript locals.
- It accepts **any** global symbol at a token that equals a default import's local name, wherever that token is in the file: `n.local == text && n.name == "default"`.

**Repro (real image, combined with J1's shift), `t7lts/src/alias.ts`.** The file has `import helper from './h'`, `class A { zzzzzz() }`, and a Latin-1 line `… p(a: A) { return a.zzzzzz(); } … q() { return helper(); }`.
- Observed: `q() CALLS A#zzzzzz()` exact, evidence at the `helper` token `[2,105,2,111]`.
- The guard took a symbol named `zzzzzz` for the token `helper` because `helper` is a default-import name.

**Untested.** Mutations M9, M14, M15 and M22 survive. Deleting the default-import rule entirely fails no unit test, although amendment 59 says it carries 71 class-validator sites.

**Suggested fixes.**
- A default-import alias should accept only the symbol that the import's own occurrence (at the import clause) resolves to, at tokens bound to that import.
- scip-typescript locals should be checked by their definition occurrence's token text.
- Units for both forms and for the alias rule's local-name condition.

### J5 — doc/nit
- `IMPLEMENT.md:241`: "complete (pending the evaluator's pass-5 re-check)" after six passes.
- `rb status` prints `definitions without occurrence 0` for diagnostics written before amendment 62 (S9 row). A snapshot is rebuilt on the next `rb index` (pipeline bump), but `rb status` misreports until then. Print `?` or omit the field when it is absent.
- The M3a timing test `many_declarations_of_one_name_stay_fast` (1.5 s wall clock in a debug build) fails under host load. Make it relative, or run it only in release.

### Attack 4 — Develocity build cache vs determinism (reproduced as described, **not** reproduced as nondeterminism)

**Runs.** spring-retry `--no-cache --force` in a fresh work dir `scipD`:
- D1: empty `.m2`. The fetch phase creates `.m2/.develocity` with one build-cache entry (148 KB, the `testCompile` output).
- D2: the same, warm.
- D3: `.develocity` deleted before the run.
- Also the shared work dir after petclinic, class-validator and jsoup.

**Result.** All four dumps are byte-identical (`cmp`). Every run has `68 files without a document` and 724/724.

**Why.** Within one `rb` run the fetch phase always stores `testCompile` before the index phase asks for it, so the outcome does not depend on prior cache state. Main `compile` is never served from the cache (no entry for it).

**Wording.** Invariant 5 ("the same repo, indexer digest and cache give byte-identical graphs …") and S12 hold as measured.

**What remains.** A deterministic coverage loss: spring-retry's tests are never SCIP-indexed. It is visible (`files without a document`) and recorded as an open item. Severity: follow-up.

**Cross-repository poisoning.** The Develocity cache serves an entry only for identical task inputs (sources, classpath, compiler arguments), so another repository can only hit it with identical content. That gives the same class files and the same missing documents: no cross-repository change in the graph. Malicious poisoning of the shared `.m2` by a hostile build is the known design §14 risk (M5).

**Note.** The fetch phase also lets the Develocity extension write `build-scan-data` and a `token-cache` into the shared `.m2`. That is part of the same open egress item.

### Checked and not reproduced
- Implicit constructors: scip-java puts `X#` and `` X#`<init>`(). `` definitions at a class's name, so the `<init>` rule could match the class. It cannot: occurrences sort by symbol and `X#` is a prefix, so the class always takes `X#` (`Ctor2`: `Bar#`, record `Q#`, enum `E#` all correct).
- A record's compact constructor gets no node; `new Pt(…)` is `unanchored`.
- `bar()`: scip-java anchors the definition at the next text match of `bar` (the call in `baz`). Tree-sitter's definition stays heuristic; the call is `unanchored`. No false edge.
- Regressions:
  - reverse `IMPLEMENTS`, type-argument supertypes, locals: edge counts identical, samples correct;
  - tabs: spring-retry probe, 0 positional mismatches;
  - environment/timeout rebuild: `scip_unfinished_jobs_are_not_up_to_date` passes twice;
  - host paths: none;
  - `mvnw`: petclinic with its wrapper is `ok`.
- Dev-dependencies: `rb-indexer` takes `scip` and `protobuf` with `.workspace = true`, the same pins as `rb-scip` (`Cargo.lock`: scip 0.10.0, protobuf 3.7.2, one version each).
- Stub scan: only the known SQL-string hit (`rb-sql/src/lib.rs:1111`).
- Dead code: `names_token`, `may_name`, `Lines`, `LineBreaks`, `name_and_span` and the new coverage fields are all referenced.

## Precision samples

Rule: all `scip` edges of the kind, sorted bytewise by `(source key, target key)`, `N = ⌊count/20⌋` (or `/10`), indices `⌊N/2⌋ + i·N`. Each evidence token was read in the source, together with the target's declaration and, for receivers and overloads, the receiver's declared type. The edge sets are identical to pass 6's, so the sampled edges are the same ones; they were re-read here.

- **petclinic `CALLS` (256, N = 12): 20/20.** Examples: `onApplicationEvent → printProperties` :106, `Owner#addPet → BaseEntity#getId` :105 (inherited through `Pet → NamedEntity → BaseEntity`), `processUpdateOwnerForm → setId` :158, `PetControllerTests#setup → OwnerRepository#findById` :86 (field `owners`), `shouldFindAllPetTypes → EntityUtils#getById` :151, `showResourcesVetList → VetRepository#findAll` :70.
- **petclinic `IMPLEMENTS` (13, N = 1): 10/10.** Subtype → supertype in each, e.g. `Owner# → Person#`, `Owner# → BaseEntity#`, `Pet# → NamedEntity#`.
- **class-validator `CALLS` (398, N = 19): 20/20.** Examples: `validateOrReject → getFromContainer`, `validate0: → isEmpty`/`min`/`isAscii`/`isIP`/`isPort`/`isISO6391`/`isNumber`, `ValidateBy`/`buildMessage` from the decorators, `customValidations → MetadataStorage#getTargetValidatorConstraints` :267.
- **class-validator `IMPLEMENTS` (2): 2/2.** `CustomConstraint#validate()`/`#defaultMessage() → ValidatorConstraintInterface`.
- **jsoup `CALLS` (14,013, N = 700): 20/20.** Spot-checked overrides:
  - `CommentTest → Node#toString` (no override in `Comment`/`LeafNode`);
  - `handlesManyChildren → Element#childNodeSize` (`doc.body()` is an `Element`; `LeafNode` has its own);
  - `FormElement# → StringUtil#join(+2)` (`String[] FormSubmitTags`);
  - `Jsoup#parse(+7) → DataUtil#load(+1)`.
- **jsoup `IMPLEMENTS` (688, N = 68): 10/10.** Hierarchy re-read:
  - `AttributeKeyPair extends AttributeEvaluator`;
  - `ProcessingInstructionTarget extends NodeEvaluator extends Evaluator`;
  - `Tag extends Token`;
  - `RequestAuthHandler implements AuthenticationHandler.AuthShim`;
  - `HttpConnection.Request … implements Connection.Request`.
- **spring-retry `CALLS` (459, N = 22): 20/20.** Receivers re-read:
  - `RetryConfiguration.advice` is an `AnnotationAwareRetryOperationsInterceptor`;
  - `BackOffPolicyBuilder` `policy` is a `FixedBackOffPolicy`;
  - `circuitBreakerCache` is a `RetryContextCache`;
  - `started` is an `ExponentialAverage`.
- **spring-retry `IMPLEMENTS` (252, N = 25): 10/10.**

**Totals: 80/80 `CALLS`, 32/32 `IMPLEMENTS`.**

## Closing readiness (design §13: "exact edges on the fixtures that build; cached and fresh indexes dump identically")

**Blockers before M3b can be declared done**
1. J1: convert positions over raw bytes with the indexer's decoding (an invalid byte is one unit spanning its own bytes). Refuse to convert, counted as `unconverted`, when the indexer's source encoding is not UTF-8 or unknown. Units with Latin-1 bytes and with an ISO-8859-1 build.
2. J2: do not let Tree-sitter's enclosing definition source an exact edge the index contradicts. Check against the SCIP definition enclosing ranges, or treat documents whose Tree-sitter comments hide indexer line terminators as undocumented. Unit with the `CommentCr` layout.

**Follow-ups (can ride with the fixes)**
- J3: TypeScript BOM on line 0.
- J4: make the name guard real for scip-typescript locals and default-import aliases; add units for the alias rule (the surviving mutations M9, M14, M15, M22).
- J5: `IMPLEMENT.md` heading, `rb status` for pre-amendment-62 diagnostics, the M3a timing test.
- spring-retry test documents (Develocity cache): disable build caches in both phases, then re-measure. Deterministic today, so not a blocker.
- Already open: Gradle, workspace installs, tsconfig include/exclude, fetch-phase egress allow-list (M5/M6).

## Verdict

VERDICT: FAIL — J1 (major): the decoder assumes the indexer read every file as UTF-8. A Latin-1 byte, or a UTF-8 file in an ISO-8859-1 Maven build, shifts occurrences onto other tokens and rows and yields false exact `CALLS` on both indexers (`Lat#q() → z()`, `q() → A#z()`, `Enc#q() → z()`). J2 (major): a lone CR or `\u000a` ending a Java `//` comment for javac but not for Tree-sitter gives an exact `CALLS` the wrong caller (`CommentCr#good() → b()`), and the loss is counted nowhere. I1's line-terminator conversion itself holds on every adversarial case and on ~96k public-repository occurrences, and its mutations are caught. I2–I5 are fixed (J5 nits). The name guard has untested and vacuous forms (J4). Determinism, including spring-retry across four `.m2` states, holds; precision is 80/80 `CALLS` and 32/32 `IMPLEMENTS`; the e2e suite passes twice.
