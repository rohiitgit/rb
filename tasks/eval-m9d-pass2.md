VERDICT: PASS

# M9d close: evaluator pass 2 (2026-10-21)

Verification only. 0 blocking, 1 should-fix, 5 notes.

## Scope

- **Pass 1:** `tasks/eval-m9d-pass1.md` (FAIL: B1; S1–S5; N1–N6).
- **Answers:**
  - `tasks/contract-m9d.md`, amendment 4;
  - the E1, E2 and E3 notes;
  - `## Result`, Out of scope.
- **Commits:**
  - `git diff 83defc0..ade02eb`: `73adbcf` (amendment 4), `94986f2` and `c209540` (E1), `45bd5b5` (E2), `ade02eb` (E3).
  - `crates/` and `migrations/` are identical between `45bd5b5` and `ade02eb`.
- **Code:**
  - `crates/rb-scip/src/{orchestrate,jobs,decode,symbol,lib}.rs`;
  - `crates/rb-indexer/src/scip.rs`.
- **Records:**
  - `bench/accept-m9d.tsv` and `bench/scoreboard/m9d{,-perf}.json`;
  - `bench/mutants/m9d.json`, plus the `m9c.json` re-anchor;
  - CHANGELOG, IMPLEMENT, README;
  - `docs/usage-and-testing.md` and `docs/repository-brain-design.md`;
  - `tasks/{todo,lessons,bench-findings}.md`.
- **Binary and run of record:**
  - `var/m9d/bin/rb`, sha256 `8445f0f45fd1…0690`;
  - run `var/bench/out/8445f0f45fd1/`;
  - logs in `var/m9d/logs/{e1,e2,e3}/`, with the regenerated inputs in `var/m9d/e3/{regen,recon}/`.
- **C16 (perf) is not decided here.**
  - The Result records C16 as "over the strict limits on two cells; waived under amendment 3, which was written after the first OVER verdict", and puts the waiver to the user for explicit sign-off. That sign-off is still open: `tasks/todo.md` has an unchecked item for it.
  - I checked only that the numbers are true and that the waiver is stated honestly. It is: the Result, CHANGELOG, IMPLEMENT and todo all use amendment 4's wording, and nowhere is C16 called "met".

## Method

- **Scratch and scripts:** everything is under `var/m9d/eval2/`, with scripts in `var/m9d/eval2/scripts/` and logs in `var/m9d/eval2/{logs,mut,score}/`.
- **Code read in full:**
  - `npm_registry_rewrite` and `registry_tarball_host`;
  - the npm fetch environment;
  - `go_work` and `GoMod.requires`;
  - `SymbolParts`, `parse` and its counter;
  - `SymbolMemo` and each of its call sites in `join_handing_back`, `definition_disagreement`, `declared_supertypes` and `Anchors`;
  - the five S3 tests.
- **Mutants**, `--jobs 1`, with `RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb` exported and debug info off:
  - All ids were passed explicitly, so neither battery could be served from `var/bench/mutants-cache.json`.
  - Pass 1's 13 specs ran 07:00:45–07:01:24, then the 48 m9d specs ran 07:01:24–07:03:47.
  - Single runs of m402 and of a re-filtered e09 followed.
  - `git diff HEAD -- crates | shasum -a 256` read `e3b0c442…` (an empty diff) before, between, after and at the end (`mut/crates-{before,mid,after}.txt`, `logs/leftovers-final.txt`).
- **C13:** `cargo build --release -p rb-cli -p rb-bench` at HEAD `ade02eb`, with no `CARGO_*` overrides and no battery running (`logs/rebuild.txt`). Free space stayed at 11–12 GiB throughout.
- **Scoring and the gate:** `var/m9d/bin/rb-bench` (`82839c81…`) only, into `var/m9d/eval2/score/`.
- **Probes on the binary of record**, all real SCIP mode with `--no-cache` and one at a time:
  - pass 1's `npm_url` and `go_ws2` fixtures;
  - a new fixture, `var/m9d/eval2/fx/npm_mix`. It holds one lockfile entry from the foreign mirror `npm.styque.de` (`crypto 1.0.1`, with nestjs's integrity) beside the `cdn.sheetjs.com` URL dependency.
  - Six repositories, all named `t_eval2_m9d_*`.

## What reproduces

- **B1 is fixed.**
  - **Code:**
    - `npm_registry_rewrite` (`orchestrate.rs:717`) reads `resolved` from `packages` and from nested `dependencies`.
    - It keeps only registry-shaped tarball hosts: `http(s)://<host>/…/<name>/-/<file>.tgz`, lower-cased, with user information stripped.
    - It drops every host in `Egress::hosts`, which is the built-in list plus `RB_SCIP_REGISTRIES`.
    - It returns `Some(host)` only when exactly one host is left.
    - `git+ssh`, `git+https` and a non-registry URL fail the prefix or shape match, so they are never named.
    - `engine_strict=false` stays on every npm fetch. `RECIPE_TS` is now `m9d-ts-2`. The lockfile and the allow-list are both in the cache key, so the rewrite needs no other key input.
  - **Unit rows:** `npm_rewrite_names_the_lockfiles_one_foreign_registry_host` has 14 rows, covering v1 nested, v2 and v3 lockfiles, scoped names, opted-in hosts, URL dependencies, git dependencies, two hosts and not-JSON. It was red first (`e1/b1-red.txt`).
  - **Mutants:** m399 and m400 are KILLED in my battery.
  - **End to end on `rb`** (`logs/probes.txt`, `logs/mix.txt`):

    | Fixture | Binary | `RB_SCIP_REGISTRIES` | Result |
    |---|---|---|---|
    | `npm_url` | `rb` | `cdn.sheetjs.com` | `scip-typescript ok` |
    | `npm_url` | `rb` | unset | `failed (fetch phase)`, `egress proxy refused: cdn.sheetjs.com:443` |
    | **`npm_mix`** | **`rb`** | `cdn.sheetjs.com` | **`scip-typescript ok`**: the mirror entry was fetched from npmjs, and the URL dependency as written |
    | `npm_mix` | `rb.p9` (`always`) | `cdn.sheetjs.com` | `failed (fetch phase)`: `404 … https://registry.npmjs.org/xlsx-0.20.3/xlsx-0.20.3.tgz` |
    | `npm_mix` | `rb` | unset | `failed (fetch phase)`: refused `cdn.sheetjs.com:443` only, so the mirror was rewritten |

    The `npm_mix` rows also show npm's hostname form of `replace-registry-host` end to end, beside a host that is not rewritten. The bench cannot show this: nestjs's lockfile has 194 `npm.styque.de` entries, 906 `registry.npmjs.org` entries and no git dependency.
  - **Docs:**
    - `docs/usage-and-testing.md` and `docs/repository-brain-design.md` now state the narrowed rule and the old form's effect.
    - Neither still says that opting a host in works under `always`.
    - The design doc's rejected-alternative sentence now reads "a host opted in … is fetched as written", which is true.
  - **Bench:**
    - On the record run, webapp's and cypress's tails end `egress proxy refused: codeload.github.com:443, github.com:443`.
    - cypress's `scip_status` row is byte-identical to M9c's (my row diff).
- **S1 is fixed.**
  - **Code:**
    - `GoMod.requires` is now a `BTreeMap` of path to version.
    - `go_work` appends `replace (\n\t<path> <version> => ./<dir>\n)` for each workspace directory whose module the root requires at a non-empty version.
    - `RECIPE_GO_WORK` is now `m9d-go-2`, pinned by `assert_eq!`. The bump is needed because the `go.work` is not a cache-key input (E1 deviation 1 and the new lesson).
  - **Tests:** `workspace_jobs_write_go_work_and_clear_goflags` gains a `wsrep` row (a quoted, `// indirect` requirement beside a replaced module; both phases see the replace) and a D6 row in `wsout`'s shape.
  - **Mutant:** m401 is KILLED.
  - **Probe:** `go_ws2` on `rb` with `--no-cache` gives `wsonly`, `wsrep` and `wsout` all `ok`. Every `scip_cross_project` row reads `exact 1, synthetic 0`, including `wsrep → inner, rep` and `wsout → inner, shared2`.
- **S2 is correct, and a kept answer equals a fresh parse.**
  - **Equivalence:**
    - `SymbolParts::of` makes one `parse`. A local symbol is never parsed, and a string the grammar rejects gives `None` for both readers.
    - `package_of` and `NameForms::from_parsed` are the old readers' bodies, unchanged. `NameForms::last()` is `None` exactly when `last_descriptor_name` is (no descriptors, or a parse failure), and `may_name` is defined as `NameForms::of(..).names(..)`.
    - Every memoised answer is a function of the symbol string alone, so the memo cannot differ from a fresh parse.
    - Each replaced call site reads the same field it read before. The memo now also covers `own_packages`, which used to be uncached, and the two `packages` loops.
  - **Tests:**
    - `the_join_parses_each_distinct_symbol_once` (7 parses) and `symbol_parts_answer_as_each_reader_from_one_parse` (13 symbols) pin the behaviour.
    - So does the extended `the_name_memo_parses_each_symbol_once_and_judges_each_text` (4 parses for 5 symbols; answers compared with all three free functions).
    - m402 (the memo never answers) is KILLED in my battery and in a single timed run (8.4 s, a real rebuild).
    - m9c's re-anchored m341 is KILLED in E2's log.
  - **End to end:** E3's record run on `rb` gives 155 dumps byte-identical to P9's, plus mattermost noscip identical to its stand-in. The only other difference is 2 rows of `mattermost.scip`, the TypeScript tails. So the new join produces identical graphs on every bench SCIP index. E2's harness also reports 0 differences over 14 indexes (`e2/equivalence.txt`).
  - **C16 as measured.** The Result, CHANGELOG, IMPLEMENT and todo give these numbers, and they match `e3/perf-ab-c16-summary.txt` and `perf-ab-s2-summary.txt`:
    - microservices_demo `scip`: CPU 1.200, RSS 1.544, instructions 1.153;
    - spring_boot `scip`: CPU 1.333, RSS 1.237, instructions 1.706;
    - `rb.p9` vs `rb`: spring instructions 0.702, CPU 0.891;
    - the four `noscip` siblings: noise.

    The mattermost NFR rows (1.107 / 1.068 / 1.186 wall, 0.925 / 1.044 / 0.954 RSS) match `e3/perf-table.txt`. The A/Bs ran 06:41–06:43, before the suite started at 06:45:32, so they did not overlap it.
- **S3: the five survivors now die.**
  - **My re-run of pass 1's specs** (`mut/eval-m9d.log`): e03, e07, e08 and e12 are KILLED with their original filters.
  - **e09 SURVIVES with its original filter `jobs::tests`.** The new row is `orchestrate::tests::a_poetry_root_with_a_requirements_txt_keeps_poetry_and_todays_arguments`, which that filter does not select.
  - The same mutation is KILLED both with `orchestrate::tests` (`mut/e09b.log`) and as m404. Amendment 4 allows "the specs (or equivalents)", so this is answered.
  - The other 8 of pass 1's specs are still KILLED.
- **The m9d battery with the database URL** (`mut/battery-m9d.log`): **48 of 48 KILLED** (m360–m407), m387 included, and none STALE, INVALID or cached.
- **S4: E3 is right, and pass 1's fastapi split was one off.**
  - The 8 fp to `tables.py:15` (`TypedTable.__init__`) are:
    - 5 module-level calls `Users()`, `Articles()`, `Tags()`, `ArticlesToTags()` and `Favorites()`;
    - `NewTable()` at `test_tables.py:12` and `:19`;
    - **`TypedTable("table_name")` at `test_tables.py:5`, which calls TypedTable's own `__init__`.**
  - So the 47 fp split as follows (counted by callee from pass 1's own `fp-new.txt`, then read in `var/bench/repos/fastapi_realworld`):
    - **23 inherited:** BaseRepository 16 (only `UsersRepository` and `TagsRepository` reach it; neither defines `__init__`), TypedTable 7.
    - **24 own:** ArticlesRepository 13, ProfilesRepository 7, CommentsRepository 1, TypedTable 1, Parameter 1, FakePoolAcquireContent 1.
  - **Other corrections:** the microblog and microservices_demo own-`__init__` wording, mattermost's "61 of 64" (IsValid 6 → 5) and the B15 explanation are in `bench/accept-m9d.tsv` and the Result.
- **S5 spot checks.** Each of these matches its log:
  - **Warm bench:** 3 min 12 s (`p9/bench-record.start` 16:08:44, `.end` 16:11:56).
  - **scip-go rate:** 2,321,257 − 127,010 = 2,194,247 occurrences, and 2,699 ms over that is 1.230 µs (`occ-counts.txt`, `perf-verdict-run3.txt:26`).
  - **Citations:**
    - C5's is `p4/mm-measure.txt:1` (`0 / 202391`);
    - C8's is `p5/mm-packages.txt` (31 rows, 30 plus sharedchannel-test);
    - C9's is `p5/c7-c8-c9-indexer-red.txt:7` (`invalid_locals_are_written_only_when_non_zero ... FAILED`).
  - **Clean-up after E3's suite:** `e3/leftovers-after-suite.txt` reads 0 for every class.
- **The re-close.**
  - **C13:** the rebuild at HEAD reproduces `rb` `8445f0f45fd11e31…0690` and `rb-bench` `82839c81…452b` bit for bit (`cmp` with `var/m9d/bin/`; release 2 min 57 s).
  - **Scoring:**
    - Two scorings with `--rb-commit 45bd5b5…` are `cmp`-identical to each other and to the committed `bench/scoreboard/m9d.json` and `m9d-perf.json`.
    - My own JSON leaf diff against P9's scoreboard (`git show 248a2ab:bench/scoreboard/m9d.json`, the same bytes as `b321e17`'s) gives **exactly 2 leaves, `/rb/binary_sha256` and `/rb/commit`** (`scripts/jdiff.py`).
    - The perf file differs in 251 timing leaves, as two runs must. The claim names the scoreboard only.
  - **Gate:**
    - The Result's command, run verbatim, exits **0** with 46 `changed`, 12 `accepted`, 0 `REGRESSION` and no unused line.
    - Its output is identical to `e3/compare.txt` apart from that file's header lines.
    - Without `--accept` it exits 1 with 12 `REGRESSION` lines (`score/compare{,-noacc}.txt`).
    - The accept file's sha256 `c9789d13…` matches the one the log records.
  - **Dumps:**
    - 156 of 157 are byte-identical to P9's reconstituted run.
    - `mattermost.scip.jsonl` differs in 2 of 264,054 rows, webapp's and cypress's `scip_status` (`scripts/rowdiff.py`).
    - The Result's "155" counts only the dumps P9's directory still holds.
  - **Invariant** (`e3/invariant.txt`):
    - NEWIDX 8,717, WORK 23,855, FALLBACK 4, RECIPE 4, NOSCIP-D2 5 and 0 UNCLASSIFIED; the control reads 32,525.
    - `e3/invariant.py` differs from P9's only in reading `INV_BASE`.
- **The regenerated mattermost dumps are sufficiently verified.** The evidence (`e3/regen-mm.txt`, `regen-verify.txt`):
  - `regen_mm.sh` re-ran `bench/run.sh`'s own `index`/`dump` commands with the kept binaries. Every SCIP job was `(cached)`, and the checkout is `b2ce71db`.
  - Byte counts equal `logs/invariant.txt`'s: 78,130,524, 87,335,887 and 87,653,049.
  - Both reconstituted runs re-score `cmp`-identical to their committed scoreboards.
  - The invariant on the reconstituted runs reproduces P9's `invariant.txt` except for the print order of two listed rows (`invariant-p9-recon.diff`): 338 removed, 903 added and 23,180 changed, each bucket the same. That is a row-level check of everything that differs between the two regenerated scip dumps.
  - **My own check:** P10's `edge_counts.py` over the two regenerated scip dumps reproduces `var/m9d/logs/p10/mattermost-edge-deltas.txt` line for line (`logs/edge-deltas-regen.txt`). Its input was computed on the original dumps on 2026-10-18.
  - **What remains unproven:** no hash of the original dumps survives, so a change common to both regenerated dumps, of exactly zero net bytes, cannot be excluded. Given content-addressed cache rows and four independent agreements, that is not a credible risk. The E3 invariant is sound for the same reason: its base is the verified reconstitution, and its head is a real run.
- **Full suite** (`e3/test.log`):
  - The `test result:` lines sum to **1,192 passed, 0 failed, 0 ignored**, with `skipping:` 0, `cargo test rc=0`, HEAD `45bd5b5` and a clean diff (`suite-load.txt`).
  - By target: e2e 53, rb-indexer 386, rb-scip 103, rb-store 85, `rb --bin` 49.
- **Batteries** (`e2/battery-*.log`):
  - m8b 29, m9a 31, m9b 39, m9c 56 and m9d 48: **203 KILLED**, and 0 SURVIVED, INVALID, STALE or cached. m387 and m341 are among them.
  - `batteries-load.txt`'s tree hash `17701eea…` equals `git diff c209540 45bd5b5 -- crates | shasum -a 256`, which I recomputed. So E2's batteries ran on exactly the committed code.
- **Notes N1–N6:**
  - N1, N2, N5 and N6 are in Out of scope, and N3 is in `tasks/todo.md`.
  - The lessons file gains the recipe-bump, battery-database and vanished-input rules. The N4 lesson is the existing one dated 2026-10-16.

## Findings

### Blocking

None.

### Should-fix

**S1. Two record sentences say more than was done.** Neither changes a number.
- **P9's mattermost `noscip` dump was not regenerated with P9's binary.**
  - The E3 note says `regen_mm.sh` re-ran "with the kept binaries (`var/m9c/bin/rb`, `var/m9d/bin/rb.p9`)". The Result and CHANGELOG say both runs' mattermost dumps "were regenerated with the kept binaries".
  - In fact `rb.p9` was run in `scip` mode only (`e3/regen-mm.txt`).
  - `var/m9d/e3/regen/5d994f76699b/mattermost.noscip.jsonl` is a symlink to the dump M9c's binary wrote (`ls -la var/m9d/e3/regen/5d994f76699b/`). The reconstituted P9 run uses that dump.
  - The substitution is justified: P9's `invariant.txt:22` reads "identical mattermost.noscip.jsonl" between M9c and P9, and P9's reconstituted run re-scores `cmp`-identical, mattermost's `noscip` cells included. It is recorded only implicitly, in `regen-verify.txt`'s first line.
  - **Settle:** one clause in the E3 note and the Result.
- **The Result's Verification → Mutants bullet is still P9c's** (`contract-m9d.md:1618`, "m360–m398: 39 specs … 39 of 39 KILLED at P9c").
  - The spec file now holds 48 specs (m360–m407), and the Tests bullet just above it was updated for E3.
  - C17's row has the current state (48/48, 203/203), so the bullet is stale, not false.
  - **Settle:** add E2's line, or point to C17.

### Notes

- **N1. Pass 1's e09 spec still survives as written.** Its filter `jobs::tests` cannot select the new row, which lives in `orchestrate::tests`. The same mutation is m404 with the right filter and is KILLED, as is `e09b` (`mut/e09b.log`). No action needed. This is recorded so that a later re-run of `var/m9d/eval1/mut/eval-m9d.json` is not read as a regression.
- **N2. e12's test models the Groovy rather than running it.**
  - `the_gradle_init_script_marks_only_protobuf_tools_under_the_gradle_home` cuts the four predicates out of `GRADLE_INIT_SCRIPT` and evaluates them in Rust. It treats `Path.startsWith` as whole path elements.
  - It fails closed: an unmodelled expression panics, and the counts of `findAll {` and `if (` are asserted.
  - It kills e12 and m403, but Gradle's own evaluation is still covered only by the P0/P7 probes and the bench.
- **N3. "Their extra work is the newly decoded indexes" (C16) is true by construction, not by a profile of `rb`.**
  - M9c had no spring index, and microservices_demo's extra jobs are new. E3 ran no profile on E2's binary.
  - Whether any avoidable per-symbol work remains on `rb` is therefore unmeasured. This bears on the user's waiver decision, not on the record's truth.
- **N4. The parse counter (`symbol::PARSES`, a thread-local `Cell`) also runs in production.** It costs one increment per parse, which is negligible beside the parse itself. It exists for tests.
- **N5. My `--no-cache` probes rewrote content-keyed `rb.scip_index` rows.** These were the rows for `npm_url`, `go_ws2` and `npm_mix`, including `npm_mix` under `rb.p9`'s `m9d-ts-1` recipe. They are cache rows, as every phase's are, and were left in place.

## Safety

- **`rb.repo`:** `SELECT * FROM rb.repo ORDER BY 1`, in a READ ONLY transaction, was identical before and after (`var/m9d/eval2/repo-{before,after}.txt`): the user's 4 rows `fx_man`, `fx_date`, `fx_full` and `alvorabackend`. They were never touched.
- **My repositories:**
  - I created 6 repositories, all named `t_eval2_m9d_*`.
  - Each was deleted with its AGE graph and its cache, after `chmod -R u+w` (`scripts/cleanup.sh`, `logs/cleanup.txt`).
  - Then `rb gc --prefix t_eval2_m9d_ --egress` ran. It removed the 2 egress networks my runs created. One extra `rb gc` call without `RB_DATABASE_URL` failed to connect to `:55432` and did nothing; I re-ran it against `:55433`.
- **Leftovers** (`logs/leftovers-final.txt`):
  - 0 `t_*` or `bench_*` repositories, 0 `rb_t_*` graphs and 0 `t_*` caches;
  - databases `postgres,rb,template0,template1` only, the same as before, so no scratch database was left;
  - 0 `rb-` containers, 0 `rb-scip` networks and no `/tmp/rbmut*`.
- **Other database reads** were all READ ONLY. The only writes outside the tests were rb's own work for my probe repositories and their deletion.
- **Mutants:**
  - Only `--jobs 1`, with no cargo build in the main tree during a battery.
  - The `crates/` diff hash was `e3b0c442…` (an empty diff) before, between, after and at the end.
  - m387 ran on the test harness's `ScratchDb`.
- **Docker:** nothing was built, deleted, retagged or pruned. `rb-scip-java:0.13.1-jdk25` `9e1396344c97`, `rb-scip-java:0.13.1` `f9612a94eac9` and `rb-scip-go:0.2.7-xtools0.50.0` `47eab8656c7e` are unchanged.
- **Disk:** free space stayed at 11–12 GiB. The release build added 813 MB under `target/release`.
- **Tree:**
  - No tracked file changed except this report, which is uncommitted.
  - I never used `git checkout --`, `git reset`, `git stash` or `git push`.
  - `worktrees/` and `bench/__pycache__/` were not touched.
