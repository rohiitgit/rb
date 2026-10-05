# Changelog

## [2026-10-05 14:33] (wall clock)
- M3b evaluator pass 8 fixes (`tasks/eval-m3b-pass8.md` K1–K4; contract amendments 68–71, invariant 1's reason list restated), each test-first. Design decision: two structural checks instead of more per-quirk rules. `rb_indexer::PIPELINE` is `m3b-scip-4`.
  - **Measured first** (real images, `var/fix-pass8`, probe over the real decoder): scip-java gives enclosing ranges on types, methods, explicit constructors, fields, enum constants, record components, locals and anonymous-class methods (none on an implicit `<init>`), in raw columns through the same conversion as occurrences; scip-typescript on files, functions, classes, interfaces, methods, constructors, namespace functions and `const` arrows only, and its class and method ranges start at their decorators, which Tree-sitter-typescript places outside them. In the three K2 layouts javac read ISO-8859-1 and the raw columns are byte offsets (65/67/80/84; UTF-8 build 48/50/63/67).
  - **K1 (major):** `structure_disagreement` (join, Java only): a document is rejected when a reference that would become an edge lies outside the enclosing range of the definition anchoring its Tree-sitter source, or when the innermost anchored range around it is neither its source's nor an ancestor's. Definitions are anchored per document first (`Anchors`); targets are judged against all documents that reached the check, so the outcome is order-independent. Real image: `Esc1`–`Esc3` → `structure_disagreement 3`, no `good → b`; `EscOk`, `Kinds`, `Ann`, `Plain` keep every edge. Units `a_source_the_enclosing_ranges_contradict_is_rejected`, `a_reference_inside_another_definitions_range_is_rejected`, `the_measured_enclosing_ranges_agree_with_tree_sitter`, `typescript_decorators_are_no_structure_disagreement`.
  - **K2 (major):** `encoding_ambiguous` (decoder, scip-java, non-ASCII documents): each single-line occurrence is read as UTF-8 and as one column per byte (`Units::JavacLatin1`, javac's tab rule kept) and checked against its symbol (`may_name`, a local's display name); rejected when one occurrence names its token both ways or the single-byte reading names more. `source_encoding` stays first and wins when both apply. Real image: `Cfg`, `Arg`, `Mod` → `encoding_ambiguous 1` each, their false `→ A#z()` gone, `Plain.java` exact; `Utf.java` (UTF-8 build) is rejected too because the evaluator's `é` run equals the distance between the two `z` calls (genuinely ambiguous; 2 true edges lost, recorded). Units `positions_another_encoding_explains_make_the_document_ambiguous` (rb-scip), `a_non_utf8_maven_build_rejects_its_non_ascii_documents` (extended).
  - **K3 (minor):** units `a_definition_at_another_definitions_name_is_a_disagreement` (N24), `a_type_definition_where_tree_sitter_reads_code_is_a_disagreement` (N25), `a_named_import_alias_names_only_symbols_of_its_name` (N27).
  - **K4 (doc):** amendment 64 no longer says false edges are impossible; amendment 71 lists per class the check and unit that cover it and the known uncovered vectors (multi-byte non-UTF-8 encodings, disagreements involving only unanchored definitions, TypeScript structure unchecked, Gradle); the pass-7 claims "every rule and clause was mutation-checked" and "every J1/J2 case" are corrected in place; open items gain commons-cli (`BUILD SUCCESS`, no `index.scip`) and failsafe (javadoc errors in the offline `verify`).
- Mutations (file copied aside, edited, `cargo test -p <crate> --lib`, restored by copy, `cmp`-checked), each failing the unit named: N24 `may_name` dropped from `accounted` → `a_definition_at_another_definitions_name_is_a_disagreement`; N25 type definitions excluded → `a_type_definition_where_tree_sitter_reads_code_is_a_disagreement`; N27 alias accepts any symbol → `a_named_import_alias_names_only_symbols_of_its_name`; S1 clause 1 off → `a_source_the_enclosing_ranges_contradict_is_rejected`, `a_reference_inside_another_definitions_range_is_rejected`; S2 clause 2 off → `a_reference_inside_another_definitions_range_is_rejected`; S3 applied to TypeScript → `typescript_decorators_are_no_structure_disagreement`; S4 outermost instead of innermost range → `a_reference_inside_another_definitions_range_is_rejected`; S5 no ancestor walk → the same; S6 bound locals not anchors → the same (enum constant body case); S7 check never applied → `a_source_the_enclosing_ranges_contradict_is_rejected`, `a_reference_inside_another_definitions_range_is_rejected`; E1 both-readings rule off, E2 count rule off, E3 applied to scip-typescript, E4 no tab rule in the byte reading, E5 locals never named → `positions_another_encoding_explains_make_the_document_ambiguous`; E6 pom reason does not win → `a_non_utf8_maven_build_rejects_its_non_ascii_documents`. No mutant survived.
- Goldens: unchanged (every fixture `ok` line still reads `documents rejected 0`).
- Public repos (release build, `--scip-work-dir var/fix-pass8/scip` seeded from `var/scip/cache`, `--force --no-cache` then `--force` served `(cached)`, fresh dump `cmp`-identical to cached on all five; then the `11ae499` binary `--force` on the same cached indexes): **0 documents rejected** on each (new reasons 0/0), and the dumps are identical line for line to the `11ae499` binary's except the header line — every edge, node and diagnostic: spring-petclinic 256 `CALLS` / 13 `IMPLEMENTS` (`scip-java ok 22.3s`), class-validator 398 / 2 (8.5 s), jsoup 14,013 / 688 (72.7 s; 31 non-ASCII and 24 escape-bearing Java files), spring-retry 459 / 252 (36.3 s), networknt/json-schema-validator @ `95825f33f76b` 6,449 / 731 (44.4 s; 15 non-ASCII, 6 with escapes).
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, **318 unit tests** (every `test result` line outside `crates/rb-cli/tests/e2e.rs`: rb 9, rb-frameworks 28, rb-git 26, rb-indexer 112, rb-model 11, rb-scip 36, rb-sql 27, rb-store 12, rb-syntax 57), **15 e2e tests with the database and real containers, run twice (44.76 s and 41.22 s; no `skipping:`)**, stub scan (only the known SQL string, `rb-sql/src/lib.rs:1111`). Observation: the M3a wall-clock test `field_chains_binding_chains_and_big_packages_stay_fast` failed in some mutation runs while a fresh build competed for CPU, and passed in every clean run (recorded in `tasks/todo.md`).
- Files affected: crates/rb-scip/src/decode.rs, crates/rb-indexer/src/{scip,projects,lib}.rs, docs/repository-brain-design.md, IMPLEMENT.md, CHANGELOG.md, tasks/{contract-m3b,todo,lessons}.md, tasks/eval-m3b-pass8.md.

## [2026-10-04 14:59] (wall clock)
- M3b evaluator pass 7 fixes (`tasks/eval-m3b-pass7.md` J1–J5; contract amendments 64–67, invariant 1 restated), each test-first. Design decision: instead of mirroring one more lexing difference, a document is SCIP-owned only when the indexer and Tree-sitter read it alike; otherwise the whole file stays Tier B, counted as `documents rejected n (reason k, …)` on the `scip:` line (`coverage.documents_rejected`, `documents_rejected_reasons`). `rb_indexer::PIPELINE` is `m3b-scip-3`.
  - **J1 encodings (major):** `not_utf8` (decoder: the blob is not UTF-8) and `source_encoding` (`rb_scip::maven_source_encoding` reads `project.build.sourceEncoding`, `maven.compiler.encoding` and `<encoding>` from every `pom.xml` under the build root, `${property}` resolved, unresolved = not UTF-8; applied in `run_projects` to non-ASCII documents). `text_differs` stays as a guard only: measured, neither indexer emits `Document.text` (0 of 50/136/205/92 documents on the four repositories, 0 on every scratch project). Measured on the real images (`var/fix-pass7/scratch`): `Lat.java` and `lat.ts` → `not_utf8`; `Enc.java` (valid UTF-8, ISO-8859-1 build) → `source_encoding`, while `Plain.java` (ASCII) in the same build keeps its exact `a → b`; `t7und` (nothing declared) is read as UTF-8, 8/8 definitions and 3 correct exact calls, 0 rejected. The `82f34d4` binary's false `Lat#q() → z()`, `q() → A#z()` and `Enc#q() → z()` are gone (those files now carry only heuristic edges). Units `documents_the_parsers_may_read_differently_are_rejected`, `a_maven_build_declaring_another_source_encoding_is_found` (rb-scip), `a_non_utf8_maven_build_rejects_its_non_ascii_documents`, `a_document_read_in_another_encoding_is_rejected` (rb-indexer; the `Enc` spans are rejected by `definition_disagreement` even without the pom's signal).
  - **J2 comment terminators (major):** `comment_terminator` (decoder, scip-java only: a lone `\r` or a `\u000a`/`\u000d` escape, any number of `u`, either case) and the general backstop `definition_disagreement` (join: a global method or type definition occurrence no Tree-sitter definition accounts for, lying on a Tree-sitter name or reference or inside a Tree-sitter function or method). Real image: `CommentCr.java` and `UEsc.java` → `comment_terminator 2`, no exact `good → b`; TypeScript `cmt.ts` keeps exact `good → a`, `evil → b`. Unit `a_document_the_parsers_read_differently_stays_heuristic_whole` rejects the `CommentCr` layout through the decoder (`comment_terminator`) **and**, with the decoder bypassed (the measured spans `evil` [3,41], `b` [4,9]), by `definition_disagreement`; a clean document of the same index keeps its exact edge and a reference into the rejected file is `references_undocumented`. Unit `forms_tree_sitter_does_not_capture_are_no_disagreement` (annotation interfaces and elements, `<constructor>`).
  - **Calibration of `definition_disagreement`** (probe with the real decoder and `rb_syntax::extract` over the exported indexes): method/type definition occurrences checked — spring-petclinic 281, class-validator 314, jsoup 5,015, spring-retry 799; not accounted for — 0, 3, 1, 48 (scip-typescript `<constructor>` ×3, jsoup's nested annotation interface ×1, spring-retry's 6 annotation interfaces and 42 elements). Documents rejected **before** the place condition: 0 / 3 / 1 / 6; **after**: 0 / 0 / 0 / 0, confirmed by the real binary (`documents rejected 0` on all four) and on the fixtures (every `ok` golden line). On the scratch projects it flags `CommentCr`, `UEsc`, `Enc`, `Lat`, `lat.ts`, `alias.ts` (every J1/J2 case even with the other reasons off).
  - **J3 BOM (minor):** scip-typescript's line 0 starts after a UTF-8 BOM (`Lines::new`); unit `typescript_line_zero_starts_after_a_byte_order_mark` (measured columns; scip-java's rule unchanged). Real image `bom.ts`: exact `aa → bb`, `bb → cc`, `dd → aa`, `dd → bb` (were lost or unanchored).
  - **J4 name guard (minor):** a default import's local name accepts only the symbol of that import's own occurrence (`Naming::default_imports`; measured: scip-typescript gives it the export's resolved symbol, never a `default` descriptor), a named alias `{ name as local }` only at `local` with a symbol that may name `name`; a local symbol names a token by its display name or the text at its definition occurrence (`Document::local_names`), else by its binding. Units `a_default_import_alias_names_only_its_imports_symbol`, `a_local_symbol_names_its_token_by_its_name_or_its_binding`, `local_names_are_read_at_their_definition_occurrences`. Real image `alias2.ts`: exact `r → helper` (default alias `hh`), `r → named` (alias `nn`), `r → K#m()`; the evaluator's `q() → A#zzzzzz()` at `helper` is gone.
  - **J5 (doc/nit):** `IMPLEMENT.md` heading; `rb status` omits `definitions without occurrence` and `documents rejected` when an older snapshot lacks them (unit `scip_status_line_prints_a_timeout_once`); `many_declarations_of_one_name_stay_fast` is a growth ratio (5,000 vs 20,000 sites, best of three, ratio < 8). Design §5.3 and §13 updated.
  - **spring-retry Develocity coverage loss:** recorded in the contract's open items as deterministic and not poisoning (evaluator attack 4); no code change.
- Mutations (file copied aside, edited, `cargo test`, restored; `git diff --stat` unchanged after each), each failing at least one unit: M9 (locals accepted whatever their name; and with no name whatever their binding), M14 (default-import rule deleted), M15 (alias token not checked against the import's local name), M22 (guard removed from local definitions), the old rule (any symbol at a default import's name); `definition_disagreement` never (6 tests fail), its place condition dropped (3), `comment_terminator` off (2 rb-indexer tests), `not_utf8` off, `source_encoding` off, BOM not skipped, a rejected document kept in the join (6); a linear scan per reference in `add_edges` (quadratic) fails `many_declarations_of_one_name_stay_fast` (50 s).
- Goldens: each `ok` line of the three `scip-status.txt` gains `, documents rejected 0` before `relationships dropped`; nothing else changed. Confirmed by two e2e runs with real containers.
- Public repos (release build, `--scip-work-dir var/fix-pass7/scip`, `--force --no-cache` then `--force` served `(cached)`, fresh dump `cmp`-identical to cached on all four; **edge sets identical to the `82f34d4` binary's on every edge kind**): spring-petclinic `scip-java ok 20.9s`, 240/240 (1 by name), without occurrence 0, 338 / 1,220 / 0 / 0 / 39 / 0 / 0, documents rejected 0, `relationships dropped 42 (0 in project, 0 anonymous)`, **256 `CALLS` / 13 `IMPLEMENTS`**, 417 nodes / 927 edges; class-validator `scip-typescript ok 13.8s`, 417/424, without occurrence 1, 625 / 366 / 0 / 4 / 24 / 2 / 0, documents rejected 0, 42 files without a document, 6 local definitions, **398 / 2**, 639 / 1,447; jsoup `scip-java ok 69.7s`, 4874/4991 (1 by name), 20,624 / 12,113 / 6 / 2 / 104 / 0 / 0, documents rejected 0, `554 (0 in project, 52 anonymous)`, 117 local definitions, **14,013 / 688**, 5,323 / 25,561; spring-retry `scip-java ok 25.4s`, 724/724, 614 / 710 / 0 / 0 / 64 / 0 / 0, documents rejected 0, `157 (0 in project, 1 anonymous)`, 68 files without a document, **459 / 252**. Cached runs 0.48–3.04 s wall.
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, **310 unit tests** (every `test result` line outside `crates/rb-cli/tests/e2e.rs`, summed with awk: rb 9, rb-frameworks 28, rb-git 26, rb-indexer 105, rb-model 11, rb-scip 35, rb-sql 27, rb-store 12, rb-syntax 57), **15 e2e tests with the database and real containers, run twice (63.98 s and 66.46 s; no `skipping:`)**; stub scan: only the known SQL-string hit (`rb-sql/src/lib.rs:1111`).
- Files affected: crates/rb-scip/src/{decode,jobs,lib}.rs, crates/rb-indexer/src/{scip,projects,resolve,lib}.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/golden/{java-spring-jpa,polyglot-monorepo,ts-express-prisma}/scip-status.txt, docs/repository-brain-design.md, IMPLEMENT.md, CHANGELOG.md, tasks/{contract-m3b,todo,lessons}.md, tasks/eval-m3b-pass7.md.

## [2026-10-01 18:10] (wall clock)
- M3b evaluator pass 6 fixes (`tasks/eval-m3b-pass6.md` I1–I5; contract amendments 59–63), each test-first.
  - **I1 line terminators (major):** measured first with both real images on a scratch project (`var/fix-pass6/eol`: lone CR between members, two lone CRs before tab-indented lines, a lone CR inside a block comment, CRLF, U+2028/U+2029/U+0085 inside Java and TypeScript string literals and comments) and a probe over the exported indexes: scip-java ends lines at `\n`, `\r`, `\r\n` (not U+2028/U+2029/U+0085), scip-typescript also at U+2028 and U+2029 (not U+0085); all 72 occurrences of the 8 documents explained. `rb_scip::decode` now converts every position through its byte offset (`LineBreaks`, `Lines::position`: the indexer's line, the existing column rule within it, Tree-sitter's `\n` rows), one path for every encoding. Second line of defence: the join takes an exact-span definition or reference occurrence only when it names its token (`rb_indexer::scip::names_token`, `rb_scip::may_name`); the legitimate naming forms were enumerated by tracing strict equality over the fixtures, the scratch project and the four public repositories (2,137 rejections: 1,946 scip-java constructors, 12 scip-typescript constructors, 106 `validate0:` properties, 2 accessors, 71 default-import aliases) and the final rule rejects 0 there. Units `indexer_lines_follow_the_indexers_line_terminators`, `a_symbol_may_name_its_token_in_the_measured_forms_only` (rb-scip), `indexer_line_terminators_join_on_the_tokens_the_indexer_meant` (through `rb_scip::decode` on encoded indexes; `rb-indexer` gains `scip`/`protobuf` dev-dependencies), `an_occurrence_must_name_its_token` (the pass-6 decoder's misaligned spans: no `#m0()` key on `m2`, no `CALLS` at the `beta()` call). Mutation-checked: guard off, `\n`-only lines, and scip-java read by TypeScript's rule each fail a test.
  - **I2 (minor):** a local symbol is in project when bound (amendment 54), not by display name (empty for scip-typescript locals); unit `typescript_local_to_local_relationships_are_counted_in_project`; scratch `tab.ts`: `relationships dropped 2 (2 in project, 0 anonymous)`.
  - **I3 (minor):** `rb_syntax::decorations::name_and_span` builds qualified supertype names from identifiers, skipping comments and whitespace (`t./* inner */CBase` → `t.CBase`); unit `comments_inside_a_qualified_supertype_name_are_skipped` (Java, TypeScript, Python); scratch `Q1.java` gains `Q1# → CBase#` and `Q1#go() → CBase#go()`.
  - **I4 (minor):** coverage `definitions_without_occurrence` and `relationships_anonymous`, printed as `definitions without occurrence n` and `relationships dropped n (m in project, k anonymous)`; amendment 54's local-class sentence corrected (scip-java gives local classes global symbols) and `a_call_to_a_local_definition_is_exact` rewritten in scip-typescript's layout.
  - **I5 (doc):** design §13 numbers (petclinic 256/13, class-validator 398/2, jsoup 14,013/688, spring-retry 459/252; amendments 1–63, six passes), §5.3 cites amendment 59; `PIPELINE` is `m3b-scip-2`, bumped with every decoder or join output change.
  - **spring-retry tests (recorded, not fixed):** its `.mvn/extensions.xml` loads the Develocity conventions extension, whose local build cache in the shared `.m2/.develocity` serves `testCompile` in the index phase, so javac never runs on `src/test` and the 68 test files get no document (contract open items).
- Goldens, re-blessed by a container run and read line by line: each `ok` line of the three `scip-status.txt` gains `definitions without occurrence 0` and `, 0 anonymous`; nothing else changed.
- Public repos (release build, `--scip-work-dir var/fix-pass6/scip`, `--force --no-cache`, then `--force` served `(cached)`; fresh dump `cmp`-identical to cached for all four; edge sets identical to the `79d2c33` binary's dumps, which differ only in the coverage diagnostic): spring-petclinic `scip-java ok 17.6s`, 240/240 (1 by name), without occurrence 0, `relationships dropped 42 (0 in project, 0 anonymous)`, 256 `CALLS`, 13 `IMPLEMENTS`; class-validator `ok 13.7s`, 417/424 + 6 local, without occurrence 1, `0 (0, 0)`, 398, 2; jsoup `ok 82.2s`, 4,874/4,991 + 117 local, without occurrence 0, `554 (0, 52)`, 14,013, 688; spring-retry `ok 35.3s`, 724/724, without occurrence 0, `157 (0, 1)`, 459, 252, 68 files without a document.
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, **300 unit tests** (every `test result` line outside `crates/rb-cli/tests/e2e.rs`, summed with awk: rb 9, rb-frameworks 28, rb-git 26, rb-indexer 99, rb-model 11, rb-scip 31, rb-sql 27, rb-store 12, rb-syntax 57), **15 e2e tests with the database and real containers, run twice (59.58 s and 63.73 s; no `skipping:` line)** after the blessing run (57.19 s); stub scan: only the known SQL-string hit in `rb-sql/src/lib.rs`.
- Files affected: crates/rb-scip/src/{decode,symbol,lib}.rs, crates/rb-indexer/src/{scip,lib}.rs, crates/rb-indexer/Cargo.toml, Cargo.lock, crates/rb-syntax/src/decorations.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/golden/{java-spring-jpa,polyglot-monorepo,ts-express-prisma}/scip-status.txt, docs/repository-brain-design.md, IMPLEMENT.md, tasks/{contract-m3b,todo,lessons}.md, tasks/eval-m3b-pass6.md (tracked).

## [2026-10-01 17:15] (wall clock)
- M3b evaluator pass 5 fixes (`tasks/eval-m3b-pass5.md` H1–H6; contract amendments 53–58), each test-first.
  - **H1 tab columns (major):** measured with the real `rb-scip-java:0.13.1` on a scratch Maven project (`var/fix-pass5/tabs`: leading single/double/triple tabs, interior single and double tabs, a tab after a comma, tabs between two members on one line, a tab-only indent before `@Override`, `<4 spaces><TAB>@Override` as in jsoup, `<2 spaces><TAB>void`, a tab inside a string literal, `é`/`😀` before a tab, the evaluator's `Wide` and `Tabby` lines): all 68 occurrences of the 9 documents follow one rule — javac advances a tab to the next multiple of 8 and every other character by its UTF-16 length, and scip-java subtracts 7 per tab that starts the line; the evaluator's zero-width `Yyyyyyyy` [52..52] is column 56 clamped to the 52-byte line. **Implemented:** `rb_scip::decode` converts a scip-java index's columns by that rule (`Units::Javac`, chosen from `Metadata.tool_info.name`; a column inside a tab's expansion maps to the tab; scip-typescript untouched). **Guard:** `declared_supertypes` accepts the chosen type occurrence only when its last descriptor name equals the written supertype's simple name. Units `javac_columns_expand_tabs_to_multiples_of_eight` and `javac_columns_inside_a_tab_map_to_the_tab` (rb-scip, measured lines and raw columns), `a_supertype_occurrence_must_name_the_written_type` (rb-indexer: the evaluator's raw `Wide` occurrences give no `Wide#act() IMPLEMENTS Doer#act()` and 2 unresolved supertypes; the converted ones give all seven forward edges). The scratch project went from 19/25 definitions, 11 unmatched references, 3 unresolved supertypes and one false exact edge to 25/25, 0, 0 and 20 correct edges; jsoup's `Evaluator.java:708` regained its 6 definitions and `IsFirstChild#matches()`/`IsRoot#matches() IMPLEMENTS Evaluator#matches()`. The fallback counter `occurrences_on_tab_lines` was not needed.
  - **H3 local definitions (major):** a `local N` definition occurrence at a Tree-sitter definition's name (or, by amendment 7's fallback, before it with the local's `display_name` equal to the name; `rb_scip::SymbolInfo.display_name` is decoded again for this) binds that symbol within its document; the definition keeps its heuristic key (`coverage.definitions_local`), and references and `is_implementation` links naming the local in that document give `scip`/`exact` edges. `local → global` links are forward by construction, `global → local` rejected as the reverse twin, `local → local` dropped. New `coverage.relationships_dropped_in_project` (both ends defined in the project on the name line of a same-named Tree-sitter definition), printed as `relationships dropped n (m in project)`; `, k local definitions` when k > 0. S7 restated (amendment 54); the e2e sweep (`assert_exact_code_edges`) now asserts that every code edge naming a documented file through a heuristic key is `scip` and that the heuristic keys naming documented files number exactly the project's `definitions_local`. Units `an_enum_constant_body_method_implements_through_its_local_symbol`, `a_call_to_a_local_definition_is_exact`, `a_local_symbol_never_crosses_documents`, `relationships_dropped_in_project_are_counted`; mutation-checked (no binding → the first two fail; counter forced to 0 → the last fails).
  - **H2 heritage comments (minor):** `comment`/`line_comment`/`block_comment` nodes are skipped in Java `superclass`, `super_interfaces`/`extends_interfaces`, Python bases and TypeScript `implements_clause`/`extends_type_clause` (`extends_clause` reads its `value` field). Unit `comments_in_heritage_clauses_are_not_supertypes` (rb-syntax; the evaluator's `extends /* c */ Base`, `implements\n // c\n I, /* d */ J`, the enum, Python and TypeScript cases; names and spans from `find`).
  - **H4 failed phase (minor):** `RunStatus::Failed(Phase)`; the `scip_status` detail records `phase`; `rb index` and `rb status` print `failed (fetch phase)`/`failed (index phase)` (an index not produced or not decodable is the index phase's). The cache row stores the phase in a new nullable column (`migrations/0005_scip_index_phase.sql`); a `failed` row without one is a miss. Units `a_failed_job_records_its_phase` (rb-scip), the cached-failure steps in `rb_indexer::projects` (row phase `fetch`; a phase-less row runs again), `scip_status_records_no_environment_detail` (detail `phase`), `scip_status_line_prints_the_failed_phase` (rb-cli); e2e `scip_polyglot_monorepo` asserts `"phase":"fetch"` and the `(fetch phase)` index line for `services/broken`.
  - **H5/H6 (doc):** amendment 57 (container paths in a failed tail are kept as written; no host path); design §13 says amendments 1–58 after five passes, §5.3 cites 26, 43, 53 and 54; amendment 2 no longer lists `elapsed_ms`; `rebuilding: 1 SCIP job did not finish last time` (singular; three e2e assertions updated).
- Goldens, re-blessed by a container run and read line by line: every `ok` line of the three `scip-status.txt` gains ` (0 in project)` after `relationships dropped n` (no fixture has a dropped relationship inside the project, and none has a local definition, so no `local definitions` suffix); polyglot `apps/web` and `services/broken` read `failed … (fetch phase)` — `npm install` cannot resolve the workspace package `@acme/shared`, and Maven cannot resolve `com.acme:nope:jar:0.0.0`, both before an indexer runs. No question golden changed.
- Public repos (release build, `--scip-work-dir var/fix-pass5/scip` seeded with the warm cache, `--force --no-cache` then `--force` cached; fresh dump `cmp`-identical to cached for all three): spring-petclinic @ 818c4136ea97 `scip-java ok 21.9s` (22.4 s wall, 0.39 s cached, plain run up to date), 240/240 (1 by name), 338 / 1,220 / 0 / 0 / 39 / 0 / 0, `relationships dropped 42 (0 in project)`, **256 exact `CALLS`, 13 `IMPLEMENTS`** — unchanged from pass 4. class-validator @ 2e1a5c27dbd6 `scip-typescript ok 10.8s` (11.9 s, 0.56 s cached, up to date), 417/424 plus 6 local definitions, 625 / 366 / 0 / 4 / 24 / 2 / 0, `relationships dropped 0 (0 in project)`, **398 exact `CALLS`** (was 394), 2 `IMPLEMENTS` (was 0), 639 / 1,447 / 41; edge sets diffed against the pass-4 binary on the same cached index: +6, none lost — 4 `CALLS` into nested arrow functions scip-typescript keys as locals and `CustomConstraint#validate()`/`#defaultMessage() IMPLEMENTS ValidatorConstraintInterface` (each read in the source); its one unanchored definition is the named class expression `CustomConstraint`, whose name gets a reference, not a definition, occurrence (S7 restated to name that case). jsoup @ 088614f4a145 `scip-java ok 54.0s` (56.2 s, 1.7 s cached), **4,874/4,991 (1 by name) + 117 local definitions**, 20,624 / 12,113 / 6 / 2 / 104 / 0 / 0, **`relationships dropped 554 (0 in project)`** (477 global → external, 25 local → external, 52 from anonymous classes' methods, which have no Tree-sitter definition by design §8.2), `supertypes_unresolved 0`, **14,013 exact `CALLS`, 688 `IMPLEMENTS`** (was 13,979 and 585), 5,323 / 25,561 / 1; heuristic `Symbol` keys in documented files 117 == `definitions_local` 117. Precision: jsoup `CALLS` sample by the evaluator's rule (N = 700, offset 350) 20/20; 10 `IMPLEMENTS` from local definitions (N = 10, offset 5) 10/10, each an enum constant body's override of the enum's abstract method; petclinic's 13 `IMPLEMENTS` unchanged (the pass-4 reading holds). Details in the contract's measurement section.
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, **294 unit tests** (every `test result` line outside `crates/rb-cli/tests/e2e.rs`, summed: rb 9, rb-frameworks 28, rb-git 26, rb-indexer 96, rb-model 11, rb-scip 29, rb-sql 27, rb-store 12, rb-syntax 56), **15 e2e tests with the database and real containers, run twice with `--nocapture` (53.81 s and 55.16 s; 0 `skipping:` lines)**, after one `RB_BLESS=1` run and one more pair (53.89 s, 53.16 s); stub scan: the one known hit (SQL comment in a test string, `rb-sql/src/lib.rs:1111`); `docker ps -a --filter name=rb-scip` empty, `var/scip` holds only `cache`.
- Files affected: crates/rb-scip/src/{decode,orchestrate}.rs, crates/rb-indexer/src/{scip,lib,projects}.rs, crates/rb-syntax/src/decorations.rs, crates/rb-store/src/lib.rs, migrations/0005_scip_index_phase.sql, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-cli/tests/golden/{java-spring-jpa,polyglot-monorepo,ts-express-prisma}/scip-status.txt, docs/repository-brain-design.md, IMPLEMENT.md, CHANGELOG.md, tasks/{contract-m3b,eval-m3b-pass5,todo,lessons}.md.

## [2026-10-01 16:00] (wall clock)
- M3b evaluator pass 4 fixes (`tasks/eval-m3b-pass4.md` G1–G10; contract amendments 43–52), each test-first.
  - **G1 name-span supertypes (major):** `rb_syntax::Supertype.span` is the span of the type's *name* only (Java `generic_type` → its name child, `annotated_type` → the type after its annotations; TypeScript `extends_clause` → its `value`; a value that is not an identifier, member expression or nested identifier — `mixin(Base)`, `(Base)` — declares no `Supertype`). `rb_indexer::scip::declared_supertypes` resolves each one to the **last** reference occurrence inside the name span whose symbol is a type (`#`); the "longer symbol extending it" rule is gone. `interface Face extends Marker<Marker.Disc>` now gives `Face# → Marker#` and no `Face#area() IMPLEMENTS Marker#Disc#area()`; `Cfg extends Base<Base.Options>` keeps `Cfg#run() → Base#run()`. Units: `supertype_spans_cover_the_name_only` (rb-syntax; replaces the Java half of the old span test), `a_nested_type_argument_is_not_the_supertype`, `an_annotated_supertype_resolves_to_the_type` (rb-indexer, on the evaluator's three-occurrence layouts; the `hierarchy` helper gained `extra` occurrences and nested-type symbols), a scip-typescript namespace-merged case (`NS#`, `NS/`, `NS/Inner#`) in `declared_supertypes_resolve_generic_qualified_and_nested_names`; J1 and T1 stay green.
  - **G2 unfinished jobs re-index (major):** `Store::unfinished_scip_jobs` counts the live snapshot's `scip_status` diagnostics with status `environment`, `timeout`, `skipped` or `unsupported` (no new column); `rb index`'s `rebuild_reason` requires that count to be 0 when SCIP is on and prints `rebuilding: n SCIP jobs did not finish last time`. With the scip-java image absent every plain run of a Maven repository therefore re-indexes (cheap: facts and index caches warm) instead of calling the heuristic snapshot current. Units: `rebuild_reason_requires_finished_scip_jobs` (rb-cli, fake count); e2e `scip_unfinished_jobs_are_not_up_to_date` (shim refusing `docker run` with exit 125 → `environment`, then the real `docker` → `rebuilding`, `scip-java ok`; `--scip-timeout-secs 1` → `timeout`, then a default-budget run → `rebuilding`, `ok (cached)`).
  - **G3 annotated supertype:** covered by G1 (`implements @Tag Shape` → `Shape#`).
  - **G4 npm log path:** `--logs-max=0` on every npm fetch (unit `npm_fetches_run_without_lifecycle_scripts`); a failed `apps/web` tail now ends `npm error Log files were not written due to the config logs-max=0`. Measured on the polyglot fixture with the release binary: two fresh runs on a warm `.m2` dump **byte-identically** (`cmp` clean, npm tail included); a fresh run on a cold Maven negative cache followed by a warm one differ in exactly one field, `services/broken`'s `stderr_tail` (`Could not find artifact com.acme:nope:jar:0.0.0 in central (…) -> [Help 1]` against `com.acme:nope:jar:0.0.0 was not found in … during a previous attempt. This failure …`), so the e2e keeps the Maven tails out of its comparison (the first e2e run of a day is the cold case) and asserts the npm tail identical and both Maven tails' resolution summary. S12 and invariant 5 restated in the contract body and amendment 46.
  - **G5 recipe in the cache key:** `rb_scip::RECIPE = "m3b-2"` is mixed into `project_input_hash` (chosen over `<digest>@<recipe>` in `indexer_digest`: the image column keeps meaning the image, smaller diff). Unit `the_recipe_is_part_of_the_key`. Existing `rb.scip_index` rows are misses.
  - **G6 scip-typescript identity:** `Images::identity(ScipTypescript)` is the pinned digest reference (`Images.typescript` removed); only scip-java uses its ID or `absent`. The e2e shim (`docker_shim` helper) refuses `image inspect` for `rb-scip-java:0.13.1` only and asserts `tool_versions.scip_java == "absent"` then a `sha256:` ID. Mutation-checked: with `tools["scip_java"] = …` deleted from `tool_versions`, `scip_image_identity_is_part_of_the_tool_configuration` fails at `e2e.rs:1006` (`scip_java` missing).
  - **G7 host paths:** the stored `skipped` reason is `docker is not available` (`ScipMode::skip_reason`); `rb index` prints the detailed error on stderr. An `environment` reason is the docker client's first line with absolute paths replaced by `<host path>` (`rb_scip::orchestrate::redact_host_paths`); the whole text stays in the run's `stderr_tail`, printed on stderr and not recorded for `environment` (recorded for `failed` and `timeout`). Units `skip_reasons_name_no_host`, `environment_reasons_name_no_host_path`, `scip_status_records_no_environment_detail`; the e2e asserts the dump has `<host path>` and no `/Users/nobody`. `unsupported` reasons were fixed text already.
  - **G8:** `rb status` prints a timeout's detail once (`scip_status_line_prints_a_timeout_once`).
  - **G9:** README "Container runtime" note (current context, `DOCKER_CONTEXT=desktop-linux` / `docker context use`, image build and pull, `--scip-work-dir` under the home directory); design §5.2 runtime-of-record sentence; `--scip-work-dir` help.
  - **G10:** `coverage.relationships_dropped` counts `is_implementation` relationships whose source or target has no project node, printed as `relationships dropped n` (unit `dropped_relationships_are_counted`).
- Goldens, re-blessed by container runs and read line by line: the three `scip-status.txt` change only by the new `, relationships dropped n` suffix on `ok` lines. Java fixture `8`, traced in the join: 7 relationships of `UserRepository#` to Spring Data interfaces (`JpaRepository`, `ListCrudRepository`, `ListPagingAndSortingRepository`, `CrudRepository`, `PagingAndSortingRepository`, `QueryByExampleExecutor`, `Repository`) and 1 of the record `AuthController#LoginRequest#` to `java/lang/Record#`, all external targets without a node; polyglot `0`, `0`, `0`; TypeScript `0`. No question golden changed (no fixture declares a generic or annotated supertype).
- Public repos (release build, `--scip-work-dir var/fix-pass4/scip` seeded with the warm cache, `--force --no-cache`): spring-petclinic @ 818c4136ea97 with `mvnw` `scip-java ok 15.0s` (15.4 s wall, 0.36 s cached, plain run up to date), 240/240 (1 by name), 338 / 1,220 / 0 / 0 / 39 / 0 / 0, `relationships dropped 42` (traced in the join: all 42 target external symbols — `Serializable#` 9, Spring Data repository interfaces 16, `Object#toString()` 2, `Formatter`/`Parser`/`Printer` 5, `Validator` 3, `WebMvcConfigurer` 2, `RuntimeHintsRegistrar` 2, `ApplicationListener`/`EventListener` 3), `supertypes_unresolved 0`, 256 exact `CALLS`, 13 `REFERENCES`, 13 `IMPLEMENTS` — all 13 read at their evidence lines (`class X extends Y`), direction correct, and the hierarchy has exactly 13 in-project pairs; 36 packages; 417 / 927 / 67. class-validator @ 2e1a5c27dbd6 `scip-typescript ok 9.1s` (10.2 s, 0.38 s cached), 417/424, 618 / 366 / 0 / 11 / 24 / 2 / 0, `relationships dropped 2` (traced: `local 12 → ValidatorConstraintInterface#validate()`, `local 15 → …#defaultMessage()`, the `local` class expression at `register-decorator.ts:61`), `supertypes_unresolved 0`, 42 files without a document, 394 exact `CALLS`, 180 `REFERENCES`, 0 exact `IMPLEMENTS`; 5 packages; 639 / 1,441 / 41. Edge sets unchanged from pass 3 (same counts per kind and provenance).
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, 284 unit tests (every `test result` line outside `crates/rb-cli/tests/e2e.rs`, summed with awk: rb 8, rb-frameworks 28, rb-git 26, rb-indexer 91, rb-model 11, rb-scip 26, rb-sql 27, rb-store 12, rb-syntax 55), 15 e2e tests with the database and real containers, run twice (52.47 s and 50.45 s; a third, earlier run 52.25 s; no `skipping:` line), stub scan clean (the one hit is the SQL comment inside a test string), `docker ps -a --filter name=rb-scip` empty, `var/scip` holds only `cache`.
- Environment: Docker Desktop through `DOCKER_CONTEXT=desktop-linux` in every shell (the CLI's current context is colima; unchanged). Session slip, recorded in `tasks/lessons.md`: a temporary trace in `scip.rs` was reverted with `git checkout --`, which discarded the file's uncommitted pass-4 work; it was restored from the diff recorded in the session transcript (same 314+/38− stat) and re-verified by the crate's 91 tests and the e2e suite. The `fx4_*` measurement repositories, their `rb.scip_index` rows and `var/fix-pass4/` were removed; the `fp3_*` rows are untouched.
- Files affected: crates/rb-syntax/src/{lib,decorations}.rs, crates/rb-scip/src/{lib,hash,orchestrate}.rs, crates/rb-indexer/src/{lib,projects,scip}.rs, crates/rb-store/src/query.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-cli/tests/golden/{java-spring-jpa,polyglot-monorepo,ts-express-prisma}/scip-status.txt, README.md, docs/repository-brain-design.md, IMPLEMENT.md, tasks/{contract-m3b,eval-m3b-pass4,todo,lessons}.md.

## [2026-10-01 11:30] (wall clock)
- M3b evaluator pass 3 fixes (`tasks/eval-m3b-pass3.md` F1–F18; contract amendments 25–42), each test-first.
  - **F1 wrapper (major):** `rb_scip::orchestrate::export` no longer writes `mvnw`, `mvnw.cmd`, `gradlew` or `gradlew.bat` at the build root, so scip-java cannot run a repository's wrapper (Docker Desktop's mounts answer `access(X_OK)` for every file); `Fetch::Maven`/`Fetch::Gradle` lose their unread `wrapper` flag. Unit `export_skips_the_build_wrappers_at_the_root`. spring-petclinic @ 818c4136ea97 with its wrapper present: `scip-java ok 22.1s`.
  - **F2 same-named supertype (major):** `rb_syntax::Supertype.span` records the heritage clause item; `rb_indexer::scip::declared_supertypes` resolves each declared supertype to the type occurrence (`…#`) inside that span, the earliest one, replaced by a longer symbol extending it for nested types (`Outer#` → `Outer#Listener#`); `types_by_symbol` replaces the simple-name lookup in `reaches_supertype` (visited set and depth cap 64 kept); unresolved ones are counted as `supertypes_unresolved` and printed when non-zero. Units `a_same_named_type_does_not_make_the_reverse_link_an_edge` (the evaluator's J1: no `Shape#area() → Circle#area()`, `Circle#area() IMPLEMENTS Shape#area()` kept), `declared_supertypes_resolve_generic_qualified_and_nested_names`; the `hierarchy` test helper now emits the heritage occurrences both real indexers were observed to emit (scratch projects j1/t1, recorded in amendment 26).
  - **F3 fetch phase (major):** `--ignore-scripts` on `npm ci`/`npm install`/`yarn install`/`pnpm install` (unit `npm_fetches_run_without_lifecycle_scripts`); S3, invariant 3 and design §5.2 now say the fetch phase runs on the default bridge network and the project's build, with allow-listing an M5 risk (§14).
  - **F4 TypeScript interface chain (major):** `rb_syntax::decorations::supertypes` reads `extends_type_clause` (unit `typescript_interfaces_extend_interfaces`); join unit `typescript_implements_runs_through_an_interface_chain` (the evaluator's T1: `Svc#m()` and `Sub#m()` `IMPLEMENTS Base#m()`).
  - **F5 image identity (major):** `rb_scip::Images` (replaces `resolve_image`) is resolved once by `rb index` before the up-to-date check and handed to `run_projects` through `ScipOptions::images`; `tool_versions` gains `scip_java`/`scip_typescript` (image ID or `absent`) when `scip` is `on`. Units `tool_versions_record_image_identities_when_scip_is_on` (rb-cli) and the `Images` assertions in `rb_scip::orchestrate::tests`; e2e `scip_image_identity_is_part_of_the_tool_configuration` (a `docker` shim whose `image inspect` fails → `scip-java unsupported`; the real `docker` is not "up to date").
  - **F6 runtime failures:** `RunStatus::Environment(reason)` for `docker run` exit 125/126/127 or a client error; never cached. **F7 timeouts:** `RunStatus::Timeout { phase, budget_secs }`, printed as `timeout (fetch phase, 600 s budget)`, never cached; a stale `timeout` row is a miss (`RunStatus::is_cacheable`, `from_cached -> Option`). Units `runtime_failures_are_environment_and_timeouts_name_their_phase` (rb-scip), `runtime_failures_and_timeouts_are_reported_but_not_cached` (rb-indexer).
  - **F8 wall-clock:** `elapsed_ms` left the `scip_status` detail; `rb index` prints the run's time from the run result, `rb status` none; `mask_elapsed_ms` and the `<elapsed>` tokens are gone. e2e `scip_ts_express_prisma` runs the container twice more with `--force --no-cache` and compares the dumps unmasked.
  - **F9 column conversion:** `fixtures/ts-express-prisma/src/routes/auth.ts` has `const welcome = "héllo 😀", session = await createSession(user.id);`; `scip_ts_express_prisma` asserts the exact `CALLS` edge into `createSession()` at Tree-sitter's byte span (column 49, UTF-16 unit 46); coverage gains `occurrences_unconverted` (`unconverted n` on the `scip:` line). Mutation check: `source_of` returning `None` fails `scip_ts_express_prisma` at the `scip-impact-create-session.txt` golden (the `auth.ts` call is lost).
  - **F10:** `scip_java_spring_jpa` and `scip_polyglot_monorepo` each run a `--force --no-cache` step that must run the container again and give the same graph; on the polyglot fixture the two failed jobs' `stderr_tail`s are excluded from the comparison (Maven's negative-cache wording and npm's timestamped log path are not functions of the commit; the resolution summary is asserted; amendment 34).
  - **F11:** `scip::sample::print_real_index_join` deleted; counts below separate unit from e2e tests.
  - **F12:** scip-typescript projects follow `--` (`fetch_then_offline_index_and_the_work_directory_is_removed`).
  - **F13:** records corrected (S1 proof, amendment 8's `./mvnw` claim, the `validateSync` note, `--scip-work-dir` help without colima, S11 table refreshed).
  - **F14:** unit `a_file_of_a_nested_failed_package_stays_heuristic_under_an_ok_outer_root` (rb-indexer `assemble`).
  - **F15:** `resolve_target`'s dotted tier splits on the space (`com.acme.auth.AuthService.authenticate` in `target_resolution_tiers`); `Describer::describe` shortens a scip-java key at its last space (`scip_java_keys_display_as_their_descriptor`).
  - **F16:** `fixture_repo` copies `git ls-files -z` paths only; `check_fixture` asserts the commit's `File` nodes are exactly those paths.
  - **F17:** `code: no heuristic references to own code (n external); m exact calls (scip)` when there is no heuristic site (`code_line_without_heuristic_sites`).
  - **F18:** `BTreeMap` lookup of a document's replaced symbols (first definition wins for shared keys); dead decoded fields removed (`ROLE_IMPORT/WRITE/READ`, `is_import`, `display_name`, `is_type_definition`, `is_definition`, `external_symbols`, `encoding_assumed`); `PIPELINE` is `m3b-scip-1`; README, design §5.2/§5.3/§13/§14 and the card's last line (`Endpoints and request paths: not indexed yet (M4)`) corrected.
- Goldens, re-blessed by container runs on Docker Desktop and each line explained: the four Java/polyglot question goldens change by display only (keys shown as `com/acme/…#…()`); the five card goldens by the last line; the three `scip-status.txt` lose `<elapsed>` and gain `unconverted 0`; the polyglot `dump.jsonl` loses `elapsed_ms` from five skipped `scip_status` details; the TypeScript status line reads `no heuristic references to own code` and moves one reference from `undocumented 1` to `local 4`: `new PrismaClient()` at `src/db/users.ts:5` now resolves into `@prisma/client`'s ungenerated stub (`local 1` in the index), because `--ignore-scripts` skips Prisma's `postinstall` generation (amendment 27; the `@prisma/client@5.22.0` package is still named by the import).
- Public repos (release build, isolated work dir seeded with the warm caches): spring-petclinic @ 818c4136ea97 **with `mvnw`** `scip-java ok 22.1s` (23.5 s wall, 0.37 s cached), 240/240 definitions (1 by name), 256 exact `CALLS`, 36 packages, 417 nodes / 927 edges; class-validator @ 2e1a5c27dbd6 `ok 10.1s` (11.1 s, 0.39 s cached), 417/424, 394 exact `CALLS`, 5 packages, 639 nodes / 1,441 edges, 42 files without a document. Precision samples by the evaluator's rule: the same 20 edges per repository as in pass 3, 20/20 and 20/20 verified at their evidence spans against the clones.
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, 274 unit tests (every `test result` line outside `crates/rb-cli/tests/e2e.rs`, summed with awk), 14 e2e tests with the database and real containers (no `skipping:` line), stub scan clean, `docker ps -a --filter name=rb-scip` empty, `var/scip` holds only `cache`.
- Environment: the Docker CLI's current context was `colima` (whose socket is dead); every container step ran with `DOCKER_CONTEXT=desktop-linux` and the context setting was not changed. The `fp3_*` measurement repositories (4 snapshots, their AGE graphs, 2 `rb.scip_index` rows of 806 KB and 959 KB) are still in the database: the cleanup statement was not permitted in this session.
- Files affected: crates/rb-syntax/src/{lib,decorations}.rs, crates/rb-scip/src/{lib,jobs,orchestrate,runner,decode}.rs, crates/rb-indexer/src/{lib,projects,scip}.rs, crates/rb-store/src/{lib,code}.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-cli/tests/golden/{java-spring-jpa,polyglot-monorepo,ts-express-prisma}/*, fixtures/ts-express-prisma/src/routes/auth.ts, README.md, docs/repository-brain-design.md, IMPLEMENT.md, tasks/{contract-m3b,eval-m3b-pass3,todo,lessons}.md.

## [2026-09-30 22:00]
- The database's host port is now `${RB_DB_PORT:-55432}` in `compose.yaml`, so the stack can start when 55432 is held by something else. No code change.
- Environment moved to Docker Desktop. Rebuilt `rb-scip-java:0.13.1` and re-pulled the pinned scip-typescript image there. All 13 e2e tests pass with real containers against a fresh database, including the two `scip-status.txt` goldens that were edited by hand in 74e0b20 (compared, not blessed).
- Files affected: compose.yaml, README.md, IMPLEMENT.md.

## [2026-09-30 21:42] (wall clock, like the entry below it; the older M3b entries carry 2026-10-01 stamps)
- M3b evaluator pass 2 fixes (contract amendments 21–24), each test-first.
  - **`IMPLEMENTS` through a hierarchy (E1):** the join accepted an `is_implementation` relationship only when the source's type holder directly declared the target's type, so `Svc extends AbstractSvc implements Mid` (`Mid extends Base`) lost `Svc#m() IMPLEMENTS Base#m()` and `Bottom extends Middle extends Top` lost `Bottom#run() IMPLEMENTS Top#run()`. Now the target's type definition is found exactly by symbol (`rb_scip::type_symbol_of`; `last_type_name` is removed) and the holder must reach it through declared supertypes (`scip::reaches_supertype`: simple names over the project's type definitions, visited set, depth cap 64). The reverse direction is still rejected at every level. No fixture edge changes: the fixtures' indexes carry only direct in-project relationships.
  - **One instance per job run (E2):** `run_job` exports into `<work_dir>/<hash16>-<pid>-<nanos>` and names its containers `rb-scip-<hash16>-<pid>-<nanos>-<fetch|index>`, so two runs of the same commit at once no longer share a directory or a container name; the cache key is unchanged.
  - **`references_undocumented` (E3):** a matched reference to an undefined symbol of the project's own package is no longer counted `external`. New field in `scip_status.detail.coverage`; the `scip:` line reads `external n, undocumented n, local n`. It moved 1 reference on the TypeScript fixture (`new PrismaClient()`, the generated client) and 2 on `java-spring-jpa` (the accessors of `record LoginRequest`).
  - **Key suffix after a space (E4):** `rb callers`/`rb impact` resolve a scip-java key's whole descriptor (`com/acme/auth/AuthService#authenticate().`), which follows a space, not a `/`.
- Goldens: `ts-express-prisma/scip-status.txt` re-blessed by a container run; `java-spring-jpa/scip-status.txt` and `polyglot-monorepo/scip-status.txt` edited by hand from the join of the indexes cached in `rb.scip_index` (the scip-java image was absent, so their tests skip).
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, 276 unit tests, and the e2e suite against the database with Docker hidden from `PATH` (13 passed, of which the three container-dependent SCIP tests skipped). `scip_ts_express_prisma` passed once with real containers under `RB_BLESS=1`, before the last code cleanup.
- **Not verified:** the SCIP e2e tests without `RB_BLESS` and with containers; the Java and polyglot SCIP e2e at all; two concurrent `rb index --no-cache --force` runs (E2). During verification the host disk reached 0 bytes free, the colima directory (`~/.colima`) was removed by something outside this work, and Docker Desktop was started in its place without the indexer images, so no container could be run again.
- Known environment hazard: an IDE's Maven build output under a fixture (`fixtures/java-spring-jpa/target/`, untracked) is copied into the fixture repository by the e2e harness and fails `orm_table_deps_goldens` with an extra `target/classes/db/migration/V1__init.sql` line.
- Files affected: crates/rb-scip/src/{lib,orchestrate,runner,symbol}.rs, crates/rb-indexer/src/{lib,projects,scip}.rs, crates/rb-store/src/code.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/golden/{ts-express-prisma,java-spring-jpa,polyglot-monorepo}/scip-status.txt, tasks/contract-m3b.md, tasks/todo.md, tasks/lessons.md, IMPLEMENT.md.

## [2026-09-30 14:15] (wall clock; the M3b entries below carry 2026-10-01 stamps, but their commits are dated 2026-09-30)
- M3b evaluator pass 1 fixes (contract amendments 15–20).
  - **Ownership is by document (D1):** `assemble` marks as exact only the files the `ok` index has a document for (`Joined::documented`); every other file keeps Tier B, so a Python script under a Maven root or a tsconfig-excluded TypeScript file no longer loses every edge. `files_without_document` counts only the indexer's languages (`scip::indexed_languages`). New fixture file `fixtures/java-spring-jpa/scripts/tool.py`; the Java `scip-card.txt` (`python 1`) and `scip-status.txt` (`1 resolved`) goldens re-blessed.
  - **`IMPLEMENTS` direction (D2):** scip-java sets `is_implementation` on both sides of an override, so the join now emits `A IMPLEMENTS B` only when A's type holder declares a supertype named like B's last type descriptor (`rb_scip::last_type_name`, `scip::type_holder`, `scip::declares_supertype`). New golden `scip-impact-default-authenticate.txt` (0 breaks).
  - **`tool_versions.scip` (D3)** is `off`, `unavailable` or `on` (`rb_indexer::ScipMode`, decided by `rb index` before the up-to-date check); e2e `scip_availability_is_part_of_the_tool_configuration` hides Docker from the child's `PATH`.
  - **Containers are named (D4)** `rb-scip-<hash16>-<fetch|index>`; a budget overrun runs `docker kill <name>`, waits for the client, then kills it (`DockerRunner { docker, kill_grace }`); `rb index --scip-timeout-secs <n>` sets both phases' budgets. Unit test through a fake `docker` script.
  - **Decoder (D5):** duplicate documents merge through `BTreeSet`s (timing test: 40k occurrences under 1 s). **`unpinned` (D6)** is printed on `rb index` and `rb status` lines; polyglot `scip-status.txt` re-blessed.
  - **`--no-cache` (D7)** bypasses SCIP cache reads (`WriteOnlyScipCache`; writes still happen), so a cached failure can be retried; flag help and the S11 measurement note corrected.
- Files affected: crates/rb-scip/src/{decode,lib,orchestrate,runner,symbol}.rs, crates/rb-indexer/src/{lib,projects,scip}.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-cli/tests/golden/{java-spring-jpa,polyglot-monorepo}/*, fixtures/java-spring-jpa/scripts/tool.py, tasks/contract-m3b.md, tasks/todo.md, tasks/lessons.md, IMPLEMENT.md.

## [2026-10-01 15:00]
- M3b step 4 (contract S8, S11; amendments 11–14). `Package` nodes and `Service -DEPENDS_ON-> Package` edges (design §5.5): in a SCIP-ok project, every occurrence of a global symbol the index does not define names a package through the `scip` crate's symbol parser (`rb_scip::package`, `rb_scip::Package::key` = `<manager>/<name>@<version>`, scip-java's `maven/` artifact prefix dropped, `@?` for no version). The join returns the first use per file and package (`Joined::packages`); `assemble` adds one edge per service and package, `scip`/`exact`, with that first site as its one evidence. Symbols of the project's own package, namespace-only symbols, locals and symbol relationships name no package; no `Symbol` stubs are built for external symbols. `ServiceCard.packages` and `rb service <name>` print `Packages (n)` / `Packages: none`; the card's last line no longer lists dependencies as unindexed.
- Coverage: `files_without_document` (owned files with facts the index has no document for) in the `scip_status` diagnostic, printed by `rb status` when non-zero. Found on class-validator, whose tsconfig excludes `test/**` and `sample`: 42 files with 927 heuristic call sites had silently lost every edge under invariant 1.
- Measured (S11): spring-petclinic with scip-java (71.1 s container time, 256 exact `CALLS`, 36 packages, 20/20 sampled edges correct) and typestack/class-validator @ 2e1a5c27dbd6 with scip-typescript (8.7 s, 394 exact `CALLS`, 5 packages, 20/20), substituted for hono, whose 1,085-package pnpm workspace is too large to install under emulation on this disk. Heuristic twins indexed for comparison; the petclinic `CALLS` difference is fully classified (34 ambiguous overload fan-outs vs 7 inherited-accessor calls).
- e2e: goldens `scip-card.txt` (TypeScript and Java fixtures) and `scip-card-auth.txt` (polyglot); the three `--no-scip` card goldens gain `Packages: none`.
- Files affected: crates/rb-scip/src/{symbol,lib}.rs, crates/rb-indexer/src/{scip,lib}.rs, crates/rb-store/src/query.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-cli/tests/golden/{ts-express-prisma,java-spring-jpa,polyglot-monorepo}/*, tasks/contract-m3b.md, tasks/todo.md, tasks/lessons.md, IMPLEMENT.md, docs/repository-brain-design.md.

## [2026-10-01 12:00]
- M3b step 3c: scip-java jobs run (Maven only) in the locally built `rb-scip-java:0.13.1` image. `Runner::image_id` (`docker image inspect --format '{{.Id}}'`) and `rb_scip::resolve_image` make a local image's ID the cache key and the reference the containers run by; `scip_status.detail.image` reports it; a missing image makes the job `unsupported` with a reason pointing at `docker/scip-java/README.md` (never built implicitly). `rb_scip::run_job` takes the resolved image and runs a list of fetch runs: for Maven `go-offline` then an online `clean verify -DskipTests` with `<cache>/m2` at `/root/.m2` and `MAVEN_OPTS=-Dorg.slf4j.simpleLogger.logFile=System.err` (Maven logs to stdout, the runner captures stderr), then `scip-java index --build-tool Maven -- --batch-mode --offline clean verify -DskipTests` under `--network none`. Gradle jobs stay `unsupported("scip-java Gradle builds are not verified yet")`. The `ghcr.io` digest constants are gone (recorded in the contract only).
- Join (contract amendment 7): scip-java anchors a member's definition occurrence at the first text match of its name, which lands inside an annotation such as `@PostMapping("/login")`; a bounded fallback accepts a global definition occurrence inside the definition's span before its name whose last descriptor is the definition's name, counted as `definitions_matched_by_name` (in the diagnostic and the `rb status` line as `(n by name)`). New `rb_scip::last_descriptor_name` through the `scip` crate's parser.
- Fixtures: polyglot `services/auth/pom.xml` declares `spring-boot-starter-web`, `spring-boot-starter-jdbc` and `spring-cloud-starter-openfeign` (BOM 2023.0.3) so it compiles; new `services/broken` (a dependency that does not exist) fails on purpose. The `--no-scip` polyglot goldens change only by the new files and the edited pom's blob.
- e2e: `scip_java_spring_jpa` (goldens `scip-callers-authenticate.txt`, `scip-impact-authservice.txt`, `scip-status.txt`; the S7 sweep; `login` carries its SCIP key; cached re-index byte-identical) and `scip_polyglot_monorepo` (`rb index` succeeds with `services/broken` failed and its Maven error in the stderr tail; `services/auth` exact; golden `scip-callers-send-code.txt`); both skip with a notice without Docker or the image. `scip:` status-line goldens mask the run time (`<elapsed>`), which the TypeScript golden had embedded from the database's cache row. The spike's Maven cache moved to `var/scip/cache/m2`.
- Files affected: crates/rb-scip/src/{jobs,runner,orchestrate,symbol,lib}.rs, crates/rb-indexer/src/{projects,scip,lib}.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-cli/tests/golden/{java-spring-jpa,polyglot-monorepo,ts-express-prisma}/*, fixtures/polyglot-monorepo/services/{auth,broken}/*, fixtures/README.md, docker/scip-java/README.md, tasks/contract-m3b.md, tasks/todo.md, IMPLEMENT.md.

## [2026-10-01 07:30]
- M3b step 3b: `rb index` runs the commit's SCIP jobs (`rb_scip::run_job` through `DockerRunner`, each keyed by `(project_input_hash, image digest)` in `rb.scip_index`; a hit runs no container, `unsupported` jobs are never cached, an unavailable Docker or `--no-scip` records every job as `skipped`), decodes ok indexes and joins them with the syntax facts. Symbol keys come from one map computed in `assemble` (heuristic per file, overridden by SCIP symbols where the join matched) that every stage now takes instead of calling `symbol_keys` itself. Files under a SCIP-ok root get exact `CALLS`/`REFERENCES`/`IMPLEMENTS` edges (provenance `scip`, source the enclosing definition or the `File` node for top-level code) and no heuristic code edges (contract invariant 1, revised S7). One `scip_status` diagnostic per job carries indexer, status, image, coverage and, when not ok, the reason or stderr tail; `cached` is deliberately not in it, so a warm re-index dumps byte-identically.
- CLI: `rb index --no-scip` and `--scip-work-dir` (env `RB_SCIP_WORK_DIR`, default `var/scip`; exports under `<hash[..16]>`, package caches under `cache/`); `rb index` prints one `scip:` line per job with `(cached)` and the run time; `rb status` prints the `scip_status` lines and the `code:` line ends with `<n> exact calls (scip)` (`resolution_coverage.exact_calls`); `tool_versions` records `scip: on/off`.
- Join: targets are limited to symbols that matched a Tree-sitter definition (the only ones with nodes); a reference to a symbol the index defines elsewhere (a field, a constant) counts as `references_unanchored` instead of dangling.
- e2e: every existing index run passes `--no-scip`; new `scip_ts_express_prisma` indexes the TypeScript fixture in the real container (skipped without Docker), with goldens for `rb callers`/`rb impact` (now `exact`), the `scip:`/`code:` status lines, an S6/S7 sweep of the dump, and a cached re-index that dumps identically. polyglot goldens gain four `skipped` `scip_status` diagnostics and the `exact calls` suffix.
- Files affected: crates/rb-scip/src/{lib,orchestrate,runner}.rs, crates/rb-indexer/src/{lib,projects,scip,syntax,sql,orm,resolve}.rs, crates/rb-indexer/Cargo.toml, crates/rb-store/src/query.rs, crates/rb-cli/src/main.rs, crates/rb-cli/Cargo.toml, crates/rb-cli/tests/e2e.rs, crates/rb-cli/tests/golden/{ts-express-prisma,polyglot-monorepo}/*, tasks/contract-m3b.md, tasks/todo.md, IMPLEMENT.md.

## [2026-10-01 05:00]
- M3b step 1: new crate `rb-scip` with `decode`, turning an `index.scip` into documents of occurrences with byte-column spans and symbol information. New dependencies `scip` 0.10 and `protobuf` =3.7.2 (the bindings' own pin). Contract `tasks/contract-m3b.md` drafted with verified indexer and crate facts.
- Files affected: crates/rb-scip/*, Cargo.toml, tasks/contract-m3b.md, tasks/todo.md, IMPLEMENT.md.

## [2026-10-01 03:00]
- M3a closed after evaluator pass 6 (PASS). Its two low-severity notes fixed: a bare call through a function-valued variable resolves to the definition the variable itself is (`Index::defines` returns the symbol declared by the same name token), so nested same-named functions no longer make sites ambiguous and an object property named like the variable is not taken for it. Edge sets on the four public repos are unchanged.
- Files affected: crates/rb-indexer/src/resolve.rs, tasks/contract-m3a.md, IMPLEMENT.md, tasks/todo.md, docs §13.

## [2026-10-01 02:30]
- e2e tests are serialized through a lock in `db_url()`: run in parallel, the `rb` processes they spawn create and drop AGE graphs concurrently, and AGE 1.8 on Postgres 18 then intermittently kills a backend (`FATAL: failed to re-find shared lock object`, 11 times in a day; three serial runs and a direct read-only write attempt never reproduced it). Recorded as a risk in design §14.
- Files affected: crates/rb-cli/tests/e2e.rs, docs/repository-brain-design.md.

## [2026-10-01 01:00]
- M3a evaluator pass 5 fixes (contract Amendment 5): every declarator is a variable binding again, and the resolver decides whether a bare call goes through a variable or a kept definition (`Index::defines`), so function-valued constants inside callbacks no longer fall through to same-named imports; `var` hoisting starts at the start of the function or file and covers `for (var …)` loop variables.
- Classified edge diff against e21d631: hono lost exactly the one wrong edge (`jsx/hooks/index.ts` → `dom/render.ts/update()`), 6,309 resolved, 14.8%; gin, requests and petclinic identical.
- Files affected: crates/rb-syntax/src/{bindings,lib}.rs, crates/rb-indexer/src/resolve.rs, tasks/contract-m3a.md.

## [2026-09-30 23:30]
- M3a evaluator pass 4 fixes (contract Amendment 4): function-valued declarations (`const f = () => …`) are definitions, not variable bindings; the bare-call rule skips Java (separate method and variable namespaces); JavaScript `var` is hoisted to the enclosing function; `staticmethod`, `classmethod` and `property` are built-ins.
- Re-measured with a classified edge diff against 5ed2b3b: hono 14.8% (6,310 resolved), regaining exactly the 329 `CALLS` edges the bare-call rule had suppressed; 2 edges lost, not classified. petclinic 39.2%, gin 63.4%, requests 30.4% unchanged.
- Files affected: crates/rb-syntax/src/bindings.rs, crates/rb-indexer/src/resolve.rs, tasks/contract-m3a.md, tasks/lessons.md.

## [2026-09-30 21:30]
- M3a evaluator pass 3 fixes (contract Amendment 3): `except` bodies are not scopes (only the `as` name is clause-scoped); class-body names are skipped rather than ending the lookup (`Locals::at_where`); signature parameters end with their signature (new scope kinds for TS and Go); Go `case` clauses are scopes; bare calls through variables are unresolved.
- Re-measured: hono 13.5% (was 14.8%); petclinic 39.2%, gin 63.4%, requests 30.4% unchanged. **Correction (pass 4):** the hono drop was a regression, not the `next()` explanation first written here: the bare-call rule also suppressed calls to functions held in `const f = () => …` declarations (329 edges). Fixed in the next entry.
- Files affected: crates/rb-syntax/src/bindings.rs, crates/rb-indexer/src/resolve.rs, tasks/contract-m3a.md.

## [2026-09-30 19:00]
- M3a evaluator pass 2 fixes (contract Amendment 2):
  - **rb-syntax:** `Binding::visible`, the extent over which a declaration is in effect, computed from per-language scope kinds; all declaration forms recorded (loops, `with`/`except`/`catch`, comprehensions, destructuring, walrus, lambda parameters, Go multi-assignment and type switches, Java enhanced-for, resources and `instanceof` patterns).
  - **Resolver:** lookups by extent through a nested-interval tree (`Locals`), replacing position order; a definition's own name is not a self-reference; Java nested classes through imports.
  - **Modules:** Python source-root rule (importer's root, `""` or `src`).
  - **rb callers:** recursive targets list themselves once.
- Re-measured: petclinic 39.2% (was 37.8%), hono 14.8%, gin 63.4% (was 64.7%; its 207 self-references from type declarations are gone, 3 genuine ones remain), requests 30.4%.
- Files affected: crates/rb-syntax/src/{bindings,lib}.rs, crates/rb-indexer/src/{resolve,modules}.rs, crates/rb-store/src/code.rs, tasks/contract-m3a.md.

## [2026-09-30 16:00]
- M3a evaluator pass 1 fixes (12 defects; contract Amendment 1):
  - **Linear resolution:** containers are deduplicated; bindings and same-package lookups are indexed.
  - **Untyped declarations** are recorded, hide outer bindings, and never fall back to same-named types.
  - **Rebinding** uses the nearest preceding declaration.
  - **Class scope:** a class body is not a scope for its methods; Python class attributes are fields.
  - **Order and packages:** Java lookup order follows JLS §6.4.1; Go `_test` files are excluded from imported packages; Python absolute imports only match source-root modules.
  - **Self-calls** are kept.
  - **`rb impact`** includes dispatch callers.
  - **`rb status`** classifies sites by target count.
- Re-measured: petclinic 37.8%, hono 14.8%, requests 30.4% unchanged. gin fell from 70.3% to 64.7%: untyped `c, _ := CreateTestContext(…)` no longer borrows the type of a later anonymous handler's `c *Context` parameter.
- Files affected: crates/rb-syntax/src/bindings.rs, crates/rb-indexer/src/{resolve,modules}.rs, crates/rb-store/src/{code,query}.rs, crates/rb-cli/tests/{e2e.rs,golden/java-spring-jpa/impact-default-authenticate.txt}, tasks/contract-m3a.md.

## [2026-09-30 13:00]
- M3a R12 measured on spring-petclinic, hono, gin and requests; 80 of 80 sampled edges correct.
- Fixed: type resolution recursed without bound through self-referential bindings (`session = session.Session()`), overflowing the stack on psf/requests. `Index::resolve_type` now has an in-progress guard and a depth cap of 64, with a test.
- Files affected: crates/rb-indexer/src/resolve.rs, IMPLEMENT.md, tasks/lessons.md.

## [2026-09-30 12:00]
- M3a step 4: the code questions.
  - **`rb_store::code`** holds pure functions over the snapshot graph.
    - `resolve_target` (§9.1): exact key, `path:line`, key suffix, dotted name, plain name. It returns every candidate in the best tier and never picks silently.
    - `callers` (Q1): breadth-first, with CHA dispatch through `IMPLEMENTS`; the path certainty is the minimum along the path; depth 1–25; references on request.
    - `impact` (Q2): the target plus its `CONTAINS` members; tier 1 is requiring edges, tier 2 is inbound reachability with dispatch; roll-ups by service (own vs other) and owner; capped at 200,000 nodes, with `truncated`.
    - `Describer`: a display name, file:line and service per node.
  - **CLI:** `rb callers <target> [--depth N] [--include-references] [--min-certainty C] [--pick N]` and `rb impact <target> [--min-certainty C] [--pick N]`.
  - **Fixtures** gain the must-not-exist cases `LegacyAuth#authenticate` and `Cache#findUserByEmail`.
  - **e2e:** `code_question_goldens` covers all three fixtures, plus candidate listing for an ambiguous name.
- Files affected: crates/rb-store/src/{lib,code}.rs, crates/rb-cli/src/{main,code}.rs, crates/rb-cli/tests/{e2e.rs,golden/*}, fixtures/*.

## [2026-09-30 10:30]
- M3a steps 2–3: Tier B resolution in the indexer.
  - **`modules.rs`** resolves imports to snapshot files:
    - TS/JS relative specifiers, with extension and `index` probing, and `.js` naming `.ts`;
    - Java classes, wildcards, static imports and nested classes;
    - Python modules by suffix, with a shared-prefix tie-break, relative levels and submodules;
    - Go import paths, through each `go.mod`'s module path (new `Content::go_modules`).
  - **`resolve.rs`** emits `CALLS`, `REFERENCES` and `IMPLEMENTS` (provenance `heuristic`).
    - Resolution levels, per §5.4: lexical scope, the same file, imports, the same package.
    - Receivers: typed receivers, `this`/`self`/`super`, class-qualified and fully qualified calls, and module aliases.
    - Inheritance is searched breadth-first, capped at 64 levels. Several candidates are `ambiguous` with `candidates`, and an unknown receiver is never linked by name.
    - `IMPLEMENTS` covers type → supertype and method → overridden method.
  - **Coverage:** per file, `unresolved_references` counts unresolved calls and types and external references; Go and Python built-ins are skipped. `rb status` prints the resolved share.
  - **rb-syntax:** type references keep their qualifier (`store` in `store.DB`).
  - **e2e:** the M1b "no CALLS" guard is replaced by M3a's certainty invariant; the polyglot dump golden gains 3 verified `CALLS` edges; there is a new `status-code.txt` golden.
- Files affected: crates/rb-indexer/src/{lib,modules,resolve}.rs, crates/rb-git/src/boundaries.rs, crates/rb-syntax/src/lib.rs, crates/rb-store/src/query.rs, crates/rb-cli/{src/main.rs,tests/e2e.rs,tests/golden/polyglot-monorepo/*}, tasks/contract-m3a.md.

## [2026-09-30 08:30]
- M3a step 1: `rb-syntax` extracts more facts per file.
  - **`imports`:** module as written, named imports with local names, module alias, wildcard and static flags. Covers ES imports, Java single-type, wildcard and static imports, Python `import`/`from` with relative levels, and Go specs.
  - **`package`:** the Java or Go package clause.
  - **`bindings`:** names with a known type (fields, parameters, typed locals, constructor initialisers, TS parameter properties, `this.x`/`self.x` members, Go struct fields and composite literals). Go and Python built-in types are dropped. Members are scoped to their class.
  - The new modules are part of the extractor fingerprint, so cached facts are rebuilt.
- Files affected: crates/rb-syntax/src/{lib,imports,bindings}.rs, tasks/contract-m3a.md.

## [2026-09-30 07:00]
- M2b evaluator pass 7 fixes:
  - **`first_along`** re-examines a base after resolving it, so results no longer depend on source order.
  - **Django `Meta`:** a class without its own `Meta` inherits only its first abstract parent's `Meta`.
  - **Tests:** shadowed-order proxies and `Meta` chains, merged-Meta across parents, and a reversed 20,000-deep proxy chain.
- Files affected: crates/rb-frameworks/src/lib.rs, tasks/contract-m2b.md, docs §7.4.

## [2026-09-30 06:00]
- M2b evaluator pass 6 fixes:
  - **`first_along`:** a new iterative, memoised resolver. `PyClasses` now resolves every class's Django table (`resolve_django_tables`) and inherited `Meta` values (`inherited_meta`) with it, once per file. It replaces the recursive `django_table` and the per-class `Meta` walks, so there is no recursion depth and cost is linear.
  - **Flask:** detection requires `db.Model` or a name ending in `.db.Model`.
  - **Contract:** Amendment 6 records E2, E3, enums/records and nested Python bases as limitations for M3.
- Files affected: crates/rb-frameworks/src/lib.rs, tasks/contract-m2b.md.

## [2026-09-30 04:30]
- M2b evaluator pass 5 fixes:
  - **`BaseResolver`** tracks which names on the current path a result was computed without, and caches only complete results, so bindings no longer depend on interface names. There is a step budget of 100,000.
  - **Python mapping** is rebuilt around `PyClasses`: per-file base and `Meta` indexes, Django and Flask membership by reverse BFS, and an iterative `Meta` inheritance walk. `reaches`, `local_bases`, `django_is_model`, `own_meta` and `inherited_meta_from` are removed.
  - **`mappings`** only considers class definitions.
- Files affected: crates/rb-frameworks/src/lib.rs, tasks/contract-m2b.md.

## [2026-09-30 03:00]
- M2b evaluator pass 4 fixes:
  - **Repository bases:** `repository_bases` was a round-based fixpoint that could loop forever on same-named interface cycles. It is now a memoised per-name resolver (`BaseResolver`) with a cycle guard and a depth cap of 512. Every declaration of a name must agree, and `org.springframework.data.…` supertypes bind the built-ins directly.
  - **Hole filter:** only non-literal native SQL has `$n` names filtered.
  - **Python walks:** they use visited sets (`reaches`, `inherited_meta_from`, `django_table` with `seen`). `MAX_MODEL_DEPTH` is removed.
  - **Flask:** models under `__abstract__` bases.
  - **Tests:** a cycle, a Spring-name clash, a wide Python hierarchy (40 classes), a 3000-level repository chain, Flask abstract bases, and `$` in static SQL.
- Files affected: crates/rb-frameworks/src/lib.rs, crates/rb-indexer/src/orm.rs, docs §7.4, tasks/contract-m2b.md.

## [2026-09-30 01:10]
- M2b evaluator pass 3 fixes (3 contract violations, and the lower-severity defects that were cheap to fix):
  - **rb-syntax:** `Supertype::implements`.
  - **rb-frameworks:**
    - `Target::Sql { text, literal }`.
    - Repository-base bindings are re-derived every round, so late clashes conflict. The redundant removal loop, which a mutation check showed was dead code, is removed.
    - Flask `__abstract__`; SQLAlchemy `__tablename__ = None`.
    - Django: schema-quoted `db_table`, `Meta(Base.Meta)` and whole-Meta inheritance, nested `models` app labels.
    - JPQL holes, SpEL and `UPDATE VERSIONED`.
    - Prisma attributes sharing a line.
  - **Indexer:** inheritance follows only `extends`. Dynamic native SQL never names hole tables, and its static tables are heuristic.
  - **Contract:** the O5 diagnostic name is corrected; Amendment 3 lists the fixes and the accepted known gaps.
- Files affected: crates/rb-syntax/src/{lib,decorations}.rs, crates/rb-frameworks/src/lib.rs, crates/rb-indexer/src/orm.rs, docs §7.4, tasks/contract-m2b.md.

## [2026-09-29 23:40]
- M2b evaluator pass 2 fixes (11 new code defects; the 13 earlier ones verified fixed), test-first:
  - **rb-syntax:** Python class constants keep non-string values, marked non-literal. String values unwrap parentheses.
  - **rb-frameworks:**
    - `Mapping::table` is `Result<TableName, Unmapped>`, with reasons `DynamicName` and `UnknownProxyParent`. The `superclass` field is removed.
    - Spring Data targets are `Target::EntityClass`.
    - JPQL uses flattened non-literal text and skips comments.
    - Conflicting generic repository bases are dropped.
    - A snake-cased JPA schema is flagged assumed.
    - Django: only `models.Model`; `app_label` inherited from abstract bases; `Meta.proxy` marks a model.
    - New `flask_snake_case`. `prisma_root` is removed.
  - **Indexer:**
    - JPA inheritance walks all Java classes (through `@MappedSuperclass`) and prefers a same-directory superclass.
    - A class-name index for repositories.
    - Prisma resolves by the longest shared directory prefix.
    - New diagnostic `orm_unresolved_proxy_parent`.
- Files affected: crates/rb-syntax/src/{lib,strings,decorations}.rs, crates/rb-frameworks/src/lib.rs, crates/rb-indexer/src/orm.rs, docs §7.4, tasks/contract-m2b.md.

## [2026-09-29 22:30]
- M2b evaluator fixes (13 defects), test-first:
  - **rb-syntax:** decoration arguments and class constants carry `literal`; concatenated literals are flattened; `true`/`false` class constants are kept; definitions record `type_params`.
  - **rb-frameworks:**
    - JPA naming follows Hibernate's `PhysicalNamingStrategySnakeCaseImpl` (`hibernate_physical_name`) for explicit names too, with quoted names kept. This corrects the earlier claim that `snake_case` matched Hibernate: it matches TypeORM and Flask-SQLAlchemy only.
    - Dynamic names give no table.
    - Django: abstract, proxy, `app_label`, same-file subclasses and multi-table inheritance. `db.Model` is Flask-SQLAlchemy.
    - JPQL counts only clause-level FROM/JOIN.
    - Generic base repositories (`repository_bases`).
    - The Prisma receiver must look like a client.
    - `@@map(name:)`; `prisma_root`.
  - **Indexer:**
    - `add_mappings` resolves the whole snapshot at once: JPA SINGLE_TABLE subclasses map to the root's table, and Prisma accessors resolve to the nearest schema (or `ambiguous`).
    - `MAPS_TO` edges are aggregated per (symbol, table), so a class declared twice no longer fails the index.
    - New diagnostics: `orm_dynamic_table_name`, and `unparsed_sql` (callee `Query`) for native queries.
- Files affected: crates/rb-syntax/src/{lib,keys,strings,decorations}.rs, crates/rb-frameworks/src/lib.rs, crates/rb-indexer/src/{lib,orm,sql}.rs, docs/repository-brain-design.md §7.4, tasks/contract-m2b.md.

## [2026-09-29 20:10]
- M2b step 3: the indexer emits ORM edges.
  - **`MAPS_TO`** (class or Prisma model → table, provenance `orm`): exact for explicit names, synthetic when the name comes from a naming rule. JPA names are folded to lowercase, as Hibernate emits them unquoted.
  - **ORM `QUERIES`:** native `@Query` is exact, conventions are synthetic, and JPQL is heuristic. Entities resolve across files, and duplicate entity names give `ambiguous`.
  - **Diagnostics:** `orm_unresolved_entity`.
  - **Prisma models** become symbols defined in `schema.prisma`.
  - **`assemble`** now takes a `Content` struct (facts, `.sql`, `.prisma`).
  - **`rb table-deps`** shows "Mapped by".
  - New goldens for the JPA and Prisma fixtures.
- Files affected: crates/rb-indexer/src/{lib,orm,sql,syntax}.rs, crates/rb-sql/src/lib.rs, crates/rb-syntax/src/{lib,keys}.rs, crates/rb-store/src/query.rs, crates/rb-cli/{src/main.rs,tests/e2e.rs}, goldens.

## [2026-09-29 19:20]
- M2b step 2: new `rb-frameworks` crate. Pure packs over syntax facts cover:
  - JPA `@Entity`/`@Table` (by default the table is the entity name snake-cased, flagged assumed);
  - Spring Data repositories: inherited CRUD, derived queries, native and JPQL `@Query`;
  - a JPQL parser (association paths skipped, FROM lists with joins);
  - Prisma schema models and client calls;
  - TypeORM `@Entity`;
  - SQLAlchemy `__tablename__`;
  - Django models (`Meta.db_table` or `<app>_<model>`).
- The `snake_case` implementation follows TypeORM's `DefaultNamingStrategy` and Hibernate's `PhysicalNamingStrategySnakeCaseImpl`, which Spring Boot uses by default. Both were checked against their sources.
- `rb-syntax`: call references record their receiver `qualifier`.
- Files affected: crates/rb-frameworks/*, crates/rb-syntax/src/lib.rs, Cargo.toml.

## [2026-09-29 18:30]
- M2b step 1: `rb-syntax` extracts framework-neutral facts per definition:
  - decorations: Java annotations, and Python/TypeScript decorators (including the siblings before a method and those on an export statement), with decoded literal arguments; TypeORM-style option objects are flattened into keyed arguments;
  - supertypes, with type arguments;
  - Python class-body string constants.
- The new source file is part of the cache fingerprint.
- Files affected: crates/rb-syntax/src/{lib,decorations,strings,keys}.rs.

## [2026-09-29 17:55]
- M2a, fourth evaluator pass (4 defects, 0 regressions); all fixed test-first and covered by mutation checks.
  - **Lexer errors** keep the statements before the failure point, taken from the lexer's "at or near" text.
  - **`pg_dump` `COPY … FROM stdin` data** is blanked, so it's no longer lexed as SQL. This removes the phantom `DROP`/`DELETE` edges and the whole-file errors.
  - **Placeholders glued to arithmetic** (`bal-?`, `n+?`) are rewritten.
  - **Multi-action `CREATE RULE (…; …)`** stays one statement.
- Files affected: crates/rb-sql/src/lib.rs, docs/repository-brain-design.md.

## [2026-09-29 17:20]
- M2a, third evaluator pass (6 defects, including a panic); all fixed test-first, and mutation checks confirm each fix is covered by a test.
  - **Splitting** is now done on Postgres lexer tokens alone. This removes the parser/scanner/coverage-check combination and the psql blanking pass.
    - Range start can no longer exceed its end (U+00A0).
    - Non-keyword statements (`USE`, `GO`, garbage) no longer hide the others.
    - `\gset` terminates a statement, and `\copy` is kept as `COPY`.
    - `BEGIN ATOMIC` bodies are tracked by keyword depth.
  - **Placeholders** glued to comparisons are rewritten (`id=?`, `>=?`, `=%(id)s`).
  - **Tree walk:** `FOR UPDATE OF alias` is no longer a table. `UNION` arms' `WITH` clauses are scoped. `PARTITION OF`/`INHERITS` parents, `CREATE FOREIGN TABLE` and `ALTER TABLE SET SCHEMA` are handled.
- Files affected: crates/rb-sql/src/lib.rs, docs/repository-brain-design.md.

## [2026-09-29 16:30]
- M2a, second evaluator pass (9 issues; all fixed test-first, plus one hidden bug found by a new test): `rb-sql` core rebuilt on Postgres' own tools. My hand-written splitter and character scanner are deleted.
  - **Splitting:** statements are split by `split_with_parser`, falling back to `split_with_scanner`. This fixes `BEGIN ATOMIC` bodies and `$` in identifiers.
  - **Coverage check:** if tokens are left uncovered, the whole text is reported. This fixes the lexer silently dropping text with unbalanced parentheses.
  - **Placeholders and meta-commands:** both are handled as lexer tokens. Comments and `E''` strings are safe, and meta-commands work anywhere on a line.
  - **Tables:** found by a full JSON walk of the protobuf tree, with proper CTE scoping. Nested scopes and sibling order fixed; `TABLESAMPLE`, `RETURNING`/`ON CONFLICT`/`MERGE` subqueries, `PREPARE`, cursors, `CREATE RULE` and `RENAME CONSTRAINT` now handled.
  - **Table keys:** parts containing `.` or `"` are quoted.
  - **Diagnostics:** one `unparsed_sql` per sink call site, with `failed_statements`.
- Mutation checks confirm each fix is covered by a test.
- Files affected: crates/rb-sql/{Cargo.toml,src/lib.rs}, crates/rb-indexer/src/sql.rs, docs/repository-brain-design.md, goldens.

## [2026-09-29 15:40]
- M2a fixes after the evaluator pass and the S16 measurement, each test-first.
- `rb-sql` rewritten:
  - **Own statement splitter:** handles quotes, `E''`, `$tag$`, nested comments and `psql` meta-commands.
  - **Per-statement parse:** one bad statement no longer hides the others, and ranges always refer to the original text. This fixes a crash.
  - **Parse-tree walk:** handles CTE shadowing, nested DML operations, dotted quoted names, `COPY` direction, `RENAME`, `SELECT INTO`, and `ALTER TYPE` as not a table. Unqualified `pg_*` names are system catalogs.
  - **Token-bounded placeholders:** covers `?`, `:name`, `%s` and `%(name)s`.
- `rb-syntax`: `\x`, `\u`, `\u{}`, `\U` and octal escapes are decoded, and keywords must be whole words.
- Indexer:
  - a BOM in `.sql` files is handled with raw-byte spans;
  - each bad statement becomes its own diagnostic;
  - `createQuery` (JPQL) is skipped and `createNativeQuery` is a sink.
- Store: distinct-site SQL coverage with an honest label, and case-insensitive table lookup.
- Files affected: crates/rb-sql/src/lib.rs, crates/rb-syntax/src/strings.rs, crates/rb-indexer/src/sql.rs, crates/rb-store/src/query.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, docs/repository-brain-design.md, tasks/contract-m2a.md, IMPLEMENT.md, README.md.

## [2026-09-29 14:20]
- M2a step 4: `rb table-deps <table>` (text or `--json`).
  - It lists migrations and accessing code grouped by service, with ops, certainty, schema assumption and file:line.
  - A bare name that matches several schemas lists the candidates and exits non-zero.
  - `rb status` shows SQL coverage (tables, query sites, migration statements, unparsed), derived from the snapshot.
  - The polyglot fixture gained Java `JdbcTemplate`, Node `pg`, Go `database/sql` and migrations.
  - New goldens: table-deps (3), status-sql.
- Files affected: crates/rb-store/src/{lib,query}.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-model/src/lib.rs, fixtures/polyglot-monorepo/*, crates/rb-cli/tests/golden/polyglot-monorepo/*.

## [2026-09-29 13:40]
- M2a step 3: the indexer adds `Table` nodes.
  - `MIGRATES` (File → Table, provenance `sql_file`) for every statement in `.sql` files.
  - `QUERIES` (Symbol or File → Table, provenance `sql_literal`) for SQL candidates. They are aggregated per (source, table), with the union of `ops`, the best certainty, and every site as evidence. Sink calls give `exact`; other candidates give `heuristic`.
  - `unparsed_sql` is reported only for unparseable `.sql` files and sink arguments.
  - Statement ranges now start at the first token, skipping leading comments.
  - New provenance: `sql_file`.
- Files affected: crates/rb-indexer/src/{lib,sql}.rs, crates/rb-sql/src/lib.rs, crates/rb-model/src/lib.rs, crates/rb-cli/tests/golden/polyglot-monorepo/dump.jsonl.

## [2026-09-29 13:05]
- M2a step 2: `rb-syntax` extracts SQL candidates, meaning SQL-looking string expressions. They are cached in `FileFacts.sql` with the normalised text, span, enclosing definition and callee name.
  - Covers TS/JS template holes and tagged templates, Java text blocks (dedented), Python f-strings and implicit concatenation, Go raw strings, and `+` concatenation.
  - Escapes are decoded.
  - Annotations and decorators are skipped.
  - `rb-sql` retries JDBC `?` and named `:param` placeholders as `$n`.
- Files affected: crates/rb-syntax/src/{lib,strings}.rs, crates/rb-sql/src/lib.rs, tasks/contract-m2a.md.

## [2026-09-29 12:20]
- M2a step 1: new `rb-sql` crate. It uses `pg_query` 6.2 (the real PG17 parser) to split SQL into statements and report each table with its operations: read, insert, update, delete, merge, create, alter, drop, truncate.
  - CTE names are excluded and system schemas are ignored.
  - Unqualified names resolve to `public` and are flagged `schema_assumed`.
  - Parse errors are returned, never guessed.
- Files affected: crates/rb-sql/*, Cargo.toml, tasks/contract-m2a.md.

## [2026-09-29 11:30]
- M1b review fixes, each test-first:
  - a Go method whose receiver type is declared in its own body crashed the process (a parent cycle). Methods now attach only to top-level types, and key building is iterative and cycle-safe;
  - a Go method no longer attaches to a same-named type local to another function;
  - nesting is capped at 64 levels (`definitions_too_deep` diagnostic), so key size stays linear;
  - TypeScript `namespace`, type aliases and enums are now definitions;
  - definitions inside anonymous functions (callbacks, lambdas, anonymous classes, Go func literals) are locals, not symbols. Previously unrelated test-local types merged into one symbol.
- Cache correctness: the facts cache key is now `rb_syntax::extractor_id()`, which combines the grammar versions with a fingerprint of the bundled queries and the extractor source, so stale facts can't be served after an edit.
- C18 moved to public repos (spring-petclinic, hono, gin, requests). The two private repos previously used for C18 were deleted from the Brain, and their references removed from IMPLEMENT.md and the M1b contract.
- Files affected: crates/rb-syntax/src/{lib,keys,queries}.rs, crates/rb-indexer/src/syntax.rs, crates/rb-cli/tests/e2e.rs, IMPLEMENT.md, tasks/contract-m1b.md, docs/repository-brain-design.md, README.md.

## [2026-09-29 10:25]
- M1b step 3: syntax facts are cached in `rb.file_facts_cache`, keyed by blob plus extractor version and language, through `Store::load_facts`/`save_facts` and a CLI `StoreCache` adapter.
  - `rb index` reports `parsed N source files`.
  - New `rb index --no-cache` bypasses the cache.
  - The index subcommand's arguments are now an `IndexArgs` struct.
  - New e2e test: a warm cache parses 0 files and dumps byte-identically to a cold run.
- Files affected: crates/rb-store/src/lib.rs, crates/rb-cli/src/main.rs, crates/rb-cli/Cargo.toml, crates/rb-cli/tests/e2e.rs.

## [2026-09-29 10:05]
- M1b step 2: the indexer turns definitions into `Symbol` nodes with `DEFINED_IN` (evidence: the name span) and `CONTAINS` edges.
  - Files are chosen for parsing by language, binary/generated flags and a 1 MiB cap (over the cap gives an `extraction_skipped` diagnostic).
  - Parsing runs in parallel on `std::thread::scope`, in batches of 512.
  - Syntax errors become diagnostics.
  - A `FactsCache` trait keeps the indexer I/O-free.
- The pipeline version is now `m1b-syntax-1`. New golden: `find-login-symbols.txt`. The dump golden includes symbols.
- Files affected: crates/rb-indexer/src/{lib,syntax}.rs, crates/rb-syntax/src/lib.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, crates/rb-cli/tests/golden/polyglot-monorepo/*.

## [2026-09-29 09:40]
- M1b step 1: new `rb-syntax` crate. It parses TypeScript, TSX, JavaScript, Java, Python and Go with Tree-sitter 0.27 and extracts definitions (with parents) and references (with their enclosing definition). It also derives heuristic symbol keys in SCIP shape.
  - Java gets an extra query for enums, records and constructors.
  - TS/TSX run both the JS and TS tag queries.
  - Go methods attach to their receiver type.
  - 15 tests cover these, and mutation checks confirm they catch regressions.
- Files affected: crates/rb-syntax/*, Cargo.toml.

## [2026-09-29 08:55]
- Engineer review of M1a:
  - service `role` (service, library, unassigned) replaces the deployable and synthetic flags;
  - stored `path` and owner `type` removed (both derivable);
  - `project_for` uses binary search, with sorting enforced by `Boundaries::new`;
  - explicit `rb.toml` name override.
- Evaluator fixes, each with a regression test:
  - `docs/*/` and `*/` now match;
  - a literal `*` in a filename no longer breaks wildcard matching;
  - a directory named `(root)` can no longer capture unassigned files (unassigned files now live on `.`);
  - CRLF and BOM handled in CODEOWNERS;
  - BOM handled in manifests;
  - non-UTF-8 paths are skipped instead of aborting the run;
  - `/* c */ package main` is detected;
  - all text ordering is bytewise (`COLLATE "C"`), not locale-dependent;
  - owners are lowercased;
  - `rb find --limit` is range-checked;
  - `pom.xml` with a DOCTYPE parses.
- Files affected: crates/rb-git/src/{glob,codeowners,boundaries,lib}.rs, crates/rb-indexer/src/lib.rs, crates/rb-model/src/{lib,manifest}.rs, crates/rb-store/src/{lib,query}.rs, crates/rb-cli/src/main.rs, crates/rb-cli/tests/e2e.rs, docs/repository-brain-design.md.

## [2026-09-29 08:10]
- M1a structure: CODEOWNERS owners with evidence, service boundary detection with `rb.toml`, and the `rb find` and `rb service` commands. Edges now carry evidence, and diagnostics keep their path (migration 0002).
- Fixed pg_trgm being installed into AGE's `ag_catalog` schema (migration 0003). Migrations now pin `search_path = public`.
- Files affected: crates/rb-git/src/{glob,codeowners,boundaries,lib}.rs, crates/rb-indexer/*, crates/rb-store/src/{lib,query}.rs, crates/rb-model/src/*, crates/rb-cli/*, migrations/0002-0003, fixtures/polyglot-monorepo.

## [2026-09-29 07:25]
- Initial commit a8f5ac2: the design doc and M0 foundations. This covers the Rust workspace, Postgres 18 + AGE 1.8.0 in compose, snapshot publishing in one transaction, and the `rb migrate`, `index`, `status`, `dump` and `cypher` commands.
