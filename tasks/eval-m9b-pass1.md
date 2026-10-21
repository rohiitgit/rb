VERDICT: PASS (0 blocking; 6 should-fix, 4 notes)

# M9b close: evaluator pass 1 (2026-10-16)

Scope: `tasks/contract-m9b.md` (D1–D9, C1–C12 with C5 and C6 withdrawn, amendments 0–6, Result), `tasks/bench-findings.md` B1, B9, B10, B12 (part b) and B13b, the diff `d445940..HEAD -- crates bench/mutants bench/probe` (commits `18a49a6`, `5a310aa`, `6f41d33`, `193e4ec`, `3b83d68`), the evidence in `var/m9b/`, `bench/scoreboard/m9b{,-perf}.json`, `bench/accept-m9b.tsv`, `bench/keys/microsecend/mse_jferrater.md`, `var/bench/labels/msecend-m9b/`, and the run outputs `var/bench/out/3fc098445243/` against `2e36cd607eab/`.

Method:
- Tools at HEAD `c85835a`: `target/debug/{rb,rb-bench}`, up to date with HEAD.
- Eight fixture repositories indexed into `:55433` as `t_eval_m9b_{b9,b9e,b9d,b10,b12,b12g,b12a,b12h}` (deleted afterwards). The fixtures are in `var/m9b/eval/fx/`.
- `python3 -I` scripts over the dumps and the exported SCIP indexes.
- 7 mutants of my own.
- Scratch: `var/m9b/eval/` (`evidence/`, `records/`, `mut/`, `n2/`, `b9/`, `fx/`).

**What reproduces:**
- `invariant.py` re-run is `cmp`-identical to `var/m9b/logs/invariant.txt`: C2 7,568, C3 28, C4 117, 0 unclassified.
- `rb-bench score` of `3fc098445243` is `cmp`-identical to `bench/scoreboard/m9b.json` and `m9b-perf.json`. The base re-score is `cmp`-identical to `var/m9b/m9a-rescored.json` and its perf file. It differs from `bench/scoreboard/m9a.json` in exactly 3 leaves: the microSecEnD key hash and jferrater endpoints `gold`/`fn` 5 → 17, as D9 says (`evidence/base-rescore-diff.txt`).
- `compare … --accept bench/accept-m9b.tsv --perf var/m9b/m9a-rescored-perf.json bench/scoreboard/m9b-perf.json` exits 0 (`evidence/compare.txt`).
- My own set diff of the mattermost `scip` agreement reproduces the accept line: fp 14 new (all heuristic) and 20 gone (19 ambiguous plus the `NewPluginAPI` self-call); out of scope +1,240 heuristic and −12 ambiguous.
- `occ_at.py` on all 15 call tokens of the 14 new fp, against `var/m9b/idx/ad9e1f9aa896d478.scip`, finds no occurrence on any token. Eight receivers read in source are right (`*App`, `*model.Post`, `*model.Config`, `*model.PropertyField`, `*model.MmBlocksActionCookie`).
- P0's probe re-run (`probe.sh` with the output redirected) differs from `var/m9b/probe-scip.txt` only in the header line. An independent classification gives 104,594 server→server/public occurrences, all at `v0.4.3`, all namespace, 0 member. The control: server's own module has 301,835 member occurrences, so the reader does decode members (`evidence/member-check.txt`).
- Adjudication (D9):
  - The prompt (`transcript.jsonl` row 0) holds only the 17 side-blind statements and forbids `bench/`, `var/bench/out`, `labels`, `crates` and running `rb`.
  - None of the 19 tool calls reads a project path outside the checkout except its own output directory.
  - `adjudicate_m9b.py` differs from `bench/adjudicate.py` only in paths and a disabled `--write`. Re-run, it is `cmp`-identical to `logs/adjudicate.txt`, and its 12 ADD lines equal the key file's entries.
- Mutants: the m9b battery re-run fresh (explicit ids, `--jobs 2`) is 28/28 KILLED (`var/m9b/eval/mut/m9b-rerun.log`). m9a is 31/31 and m8b 29/29 KILLED (`m9a-rerun.log`, `m8b-rerun.log`). None is STALE or INVALID.
- `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` are CLEAN (`var/m9b/eval/fmt-clippy.log`). The diff adds no TODO, `todo!`, `unimplemented!`, `dbg!` or `println!`.
- No new `exact` row: every B9 and B10 edge is `heuristic`, and every B12b `EXPOSES`/`HANDLED_BY` is `synthetic` or `heuristic`, in all 8 fixtures and in the invariant.

## Findings

**E1. Should-fix: all 7 of my mutants on the new conditions survive their named test, and one shows that a C3 refusal row tests nothing.**
- Repro: `python3 -I bench/mutants/run.py var/m9b/eval/mut/eval-m9b.json e1 e2 e3 e4 e5 e6 e7 --jobs 2` (log `var/m9b/eval/mut/eval-m9b.log`). All 7 are SURVIVED:
  - e1 (`sql.rs`): a local binding of `fmt` no longer refuses the fold (`&& scope.binding_index_at("fmt", at).is_none()` → `&& true`).
  - e2 (`sql.rs`): `%d` is accepted as a string hole (`Some('s' | 'v')` → `Some('s' | 'v' | 'd')`).
  - e3 (`lib.rs`): the lib-side dependency gate is removed (`DATA_REST_DEPENDENCIES.iter().any(…)` → `true`), so every build file "names" Spring Data REST.
  - e4 (`lib.rs`): `spring.data.rest.basePath` is no longer read.
  - e5 (indexer `routes.rs`): `RepositoryRestConfigurerAdapter` no longer caps.
  - e6 (`resolve.rs`): a function-local type gets the package type's cross-file methods (`|| def.parent.is_some()` dropped).
  - e7 (frameworks `routes.rs`): `findAllById` is exported as a search resource.
- e2 is the sharp one. C3 lists "`%d`" among the refusals, and row `H` (`"… %s WHERE note = '%d'", usersTable`) is meant to pin it. But `H` has 2 verbs and 1 value, so it refuses for the count mismatch whatever `%d` does. The `%d` refusal itself is untested.
- e3 shows that `data_rest_services` (`lib.rs:836-859`), which reads the build files and the base path, has no test at all. m278 mutates only the indexer's lookup of the map that the test hands in.
- Current behaviour is right for e1 (fixture `b10`, `ShadowFmt()`: no fold), e3 (fixture `b12`, `svcd` aside, see E5), e4 (fixture `b12g`: `basePath=/rest/` gives `/ctx/rest/fs`) and e5 (fixture `b12g`, `svch`: `heuristic` and one `data_rest_base_unread`). So these are gaps in regression protection, not wrong output today.
- Breaks: C3 ("Refuses: … `%d` … Each refusal gives today's output") for `%d`; the rule that every D-rule has a biting test ("a surviving mutant is a test bug until proven otherwise", usage §5.4); amendment 3's "no binding of `fmt` may be in effect", whose only mutant (m286) covers the import half.
- Fix: a row `fmt.Sprintf("SELECT id FROM %d", usersTable)` (one verb, one value); a row with a local `fmt`; a `lib.rs` test of `data_rest_services` (a pom with and without the dependency, `base-path` and `basePath`); an `…Adapter` row; a local-type row; a `findAllById` row. Add e1–e7 (or equivalents) to `bench/mutants/m9b.json`.

**E2. Should-fix: a configurer registered as a `@Bean` is not seen, so D7's cap does not apply, including to the form the pinned Spring Data REST 2.5.5 reference shows.**
- Repro:
  - Fixture `var/m9b/eval/fx/b12a` (`t_eval_m9b_b12a`), service `svci`: `@Bean public RepositoryRestConfigurer repositoryRestConfigurer() { return new RepositoryRestConfigurerAdapter() { … config.setBasePath("/moved"); … }; }`.
  - Fixture `var/m9b/eval/fx/b12` (`t_eval_m9b_b12`), service `svcc`: `@Bean` returning `RepositoryRestConfigurer.withConfig(config -> config.setBasePath("/moved"))`.
- Expected (D7): "A `RepositoryRestConfigurer` in the service caps the endpoints at `heuristic` and pushes one `data_rest_base_unread`." Actual: both services get 6 `synthetic` endpoints at `/is` and `/cs` and no `data_rest_base_unread`, while the application serves them under `/moved`.
- Cause: `configured` (`crates/rb-indexer/src/routes.rs:516-531`) looks only at Java *definitions whose supertype* is `RepositoryRestConfigurer(Adapter)`. An anonymous class in a `@Bean` method and the `withConfig` factory (Spring Data REST 3.1+) have no such definition. A Kotlin `class Cfg : RepositoryRestConfigurer` is skipped by the `Language::Java` filter.
- Source: the 2.5.5 reference §4.6.2 configures the base path with a `@Bean` returning an anonymous `RepositoryRestConfigurerAdapter`, and the `RepositoryRestConfigurer` javadoc has `withConfig` since 3.1 (`var/m9b/eval/records/spring-docs.txt`, (g)).
- Impact: `synthetic`, not `exact`. The bench has no configurer (`citations-jfer.txt`: no `RepositoryRestConfigurer` in mse_jferrater).
- Breaks: D7 ("Base path"), amendment 4 ("A service with a definition whose supertype is …"), which narrows D7 without saying so.
- Fix: also cap when a method's return type, or an `object_creation_expression`'s type, or a `withConfig` call names `RepositoryRestConfigurer(Adapter)`; read Kotlin files too. Add rows for each.

**E3. Should-fix: the fold reads a typed constant's literal, but `%s`/`%v` call its `String()` method.**
- Repro: fixture `var/m9b/eval/fx/b10` (`t_eval_m9b_b10`).
  - `repo/consts.go`: `type Tbl string; func (t Tbl) String() string { return "stringer_" + string(t) }` and `const tStringer Tbl = "t_typed"`.
  - `repo/use.go:38`: `fmt.Sprintf("SELECT id FROM %s", tStringer)`.
- Expected: Go formats a `Stringer` operand through `String()`, so the SQL reads `FROM stringer_t_typed`. Either that table, or (as D6 intends for anything not certain) no fold.
- Actual: `QUERIES Stringer() -> public.t_typed` (`heuristic`). The `sql_folded` count includes the site.
- Cause: `GoConsts::new` (`sql.rs:344-386`) keeps any top-level `const` whose initialiser is one literal, whatever its declared type.
- The same holds for a type with `Error()` or `Format`. The bench has none (mattermost's 2 folded sites are untyped), so this is a wrong edge on input that is rare but legal.
- Breaks: D6 ("Go top-level `const` string bindings"), and its state invariant "a `%s`/`%v` hole is replaced only by a constant…" with the value Go would print.
- Fix: keep only untyped constants and constants declared `string`; add a row.

**E4. Should-fix: N2's guard misses filters that select a timing test by module path, and its docstring claims the opposite.**
- Repro: `python3 -I var/m9b/eval/n2/selects.py` and `cargo test -q -p rb-indexer --lib <filter> -- --list` (`var/m9b/eval/n2/selects.txt`):

  | Filter | `selects_timing` | cargo actually runs |
  |---|---|---|
  | `resolve::tests` | none | both `resolve::tests::…_stay_fast` |
  | `resolve` | none | both `…_stay_fast` |
  | `tests` | none | 4 listed timing tests (2 `stay_fast`, 2 `too_ambiguous`) |
  | `di::tests` | none | `many_beans_of_one_type_are_too_ambiguous` |
  | `stitch` | none | `an_all_parameter_input_is_too_ambiguous_within_a_bound` |

- Expected (amendment 0 N2; `run.py` docstring): "a spec whose filter selects a listed test is INVALID", and the name-only check is "an over-approximation: it can only refuse more". Actual: `selects_timing` tests whether the filter's last `::` segment occurs *in the test's name*. cargo matches the filter against the *full path* (`module::tests::name`), so a filter that names a module selects its timing tests unseen. The kill rule (`verdict`) then accepts a failing `resolve::tests::…_stay_fast` as a kill for filter `resolve`, because the filter occurs in that name. That is the M9a E2 hole again.
- Impact today: none. No spec of `m8a`/`m8b`/`m9a`/`m9b` selects a timing test by full path (`var/m9b/eval/n2/existing.txt`, 0 hits).
- Fix: keep the module path in `timing_tests.txt` (`resolve::tests::name`, from the file) and test `filter in path`. Or ask cargo (`--list`) which tests a filter selects before mutating.
- Records: amendment 5 says "17 tests in 7 crates". The list has 6 crates (rb-cli 3, rb-frameworks 4, rb-indexer 4, rb-scip 3, rb-server 2, rb-syntax 1).

**E5. Should-fix: the 12 ADD lines are described wrongly in three records, and a real endpoint is in neither rb nor the key, so "17 of 17" overstates recall.**
- **Description.** Amendment 6 (`tasks/contract-m9b.md:298`), `tasks/bench-findings.md:153` and the key's own prose (`bench/keys/microsecend/mse_jferrater.md:27`) say the 12 are "5 item resources and 7 search resources". `var/m9b/logs/adjudicate.txt` has:
  - 5 item resources (`/{customers,prices,stores,accounts,items}/{}`);
  - 2 collection resources (`/prices`, `/items`);
  - 5 search resources.
  - The other 2 search resources were already in the model. Amendment 6's arithmetic ("Plan correction 9 had counted 11 (6 search resources), and amendment 4 measured 7, hence 12") is wrong for the same reason.
- **The missing endpoint.** `CustomerService/…/CustomerRepository.java:17` declares `Customer findById(Long id)`.
  - On the pinned Boot 1.4.2 (Spring Data Commons 1.12.5, Spring Data REST 2.5.5), `CrudRepository` has `findOne`, `exists` and `findAll`, and no `findById`, `existsById` or `findAllById` (javadoc, `spring-docs.txt` (h)).
  - So `findById` is a derived query method there, and 2.5.5 exports `GET /customers/search/findById`: "For every query method declared in the repository we expose a query method resource" (§5.5.1).
  - Amendment 4 dismisses it as "a CRUD name". That is D7's version-agnostic rule, but the reason given is false for this project, whose version is pinned. The adjudicator's own notes name Commons 1.12.
  - Because the side-blind list holds only rb's and the model's endpoints, the key never got it. The cell's 17/17 (R 100 %) is against a key with a known omission.
- Breaks: the records rule "every number traces to a log" (the 5/7 split); the C4 and Result reading of "fn 0".
- Fix: correct amendment 6 and bench-findings. Append a correction line to the key file (it is append-only). Record `/customers/search/findById` as a known gold omission in bench-findings, or adjudicate it as an extra statement under the M8b protocol, and say in D7 that the CRUD-name rule loses it on Spring Data 1.x.

**E6. Should-fix: the gate's cited log and command do not match what is committed.**
- C9 (line 118) and the Result (line 351) give the command as `… --accept bench/accept-m9b.tsv --perf`. As written, it exits 2: "a value is required for '--perf <BASE_PERF> <HEAD_PERF>'". It needs `--perf var/m9b/m9a-rescored-perf.json bench/scoreboard/m9b-perf.json`.
- `var/m9b/logs/compare.txt`, which C9 and the Result cite, was written at 16:48, before `c85835a` changed the mattermost accept reason. Its two `accepted … mattermost` lines differ from the committed `accept-m9b.tsv` (`var/m9b/eval/evidence/compare-vs-log.diff`).
- `logs/compare-acceptfix.txt` (16:59) matches the committed file, but no record cites it, and it has no `exit=` line. My re-run with the committed file exits 0, so the gate holds. Only the evidence trail is off.
- The accept line also says the 3 `App#GetClusterId` fp are "recorded for M9c". `tasks/contract-m9c.md` never mentions them; only `tasks/todo.md:1194` does.
- Fix: cite the full command; re-run `compare` into the cited log with an `exit=` line; add the `GetClusterId` question to M9c's contract or drop "recorded for M9c".

**E7. Note: D7 over-emits CRUD resources that a repository hides.**
- Repro: fixture `var/m9b/eval/fx/b12h` (`t_eval_m9b_b12h`). `J1 extends CrudRepository` overrides `@RestResource(exported = false) void delete(Long id)` and `void delete(X entity)`, the pattern from the Spring Data REST reference ("hiding certain repository CRUD methods"). rb still emits `synthetic` `DELETE /js/{}`.
- Only search methods consult `@RestResource`. Collection and item resources are emitted from the base name alone. That is D7 as written, so it is a design gap, not a broken clause. It is 0 on the bench.
- The reverse also holds: a bare `Repository` that declares its own `findAll`, `save` or `findOne` gets collection and item resources in Spring ("exposure follows the methods you have exposed"), but none from rb. A `PagingAndSortingRepository` gets `POST` and item resources on Spring Data ≤ 2.x (it extends `CrudRepository` there), but only `GET /{path}` from rb. Spring also exports `@Query` methods of any name, while rb exports only `subject…By` names. These lose recall, not precision.
- So amendment 4's ruling on a bare `Repository` (search only) is right only when the interface declares no CRUD method.

**E8. Note: smaller D7 inputs, outside the contract's letter.**
- Kotlin repositories (`svcb/…/KRepo.kt` in fixture `b12`) give no endpoint: `spring()` runs for Java only. D7 says "a Java interface", so this is in scope as written. The design doc and usage guide do not say "Java only".
- A dependency that appears only inside an XML comment (`svcd/pom.xml`, `<!-- …spring-boot-starter-data-rest… -->`) gates the service in. That gives 6 `synthetic` endpoints in a service that serves none: `text.contains` on the raw build file.
- What held: Gradle-only (`build.gradle`, `svcf`), `basePath=/rest/` with `server.servlet.context-path=/ctx` (`/ctx/rest/fs`), `base-path: /api/` beside a `spring.profiles: dev` document (`/api/…`, the profile document ignored), a base path only under `on-profile: prod` (ignored), `path = "/slashy/"` (`/api/slashy`), method `@RestResource(path = "/byName/")` (`…/search/byName`), method `exported = false` (hidden), type `exported = true` (exported), `collectionResourceRel` only (none), a fully qualified annotation without an import (exported), an indirect project base `BaseRepo<T> extends CrudRepository` (none, as D7's "directly extends" says), and `path = "a" + "b"` (`/ab`, a compile-time constant). All are `synthetic`, or `heuristic` under a detected configurer.

**E9. Note: B9 leaves two Go method forms unresolved; neither makes a false edge.**
- Fixture `var/m9b/eval/fx/b9`, `alias/`: `type Al = Real` and `func (a Al) ViaAlias()` in another file. `r.ViaAlias()` and `a.ViaAlias()` stay unresolved. The method is keyed under `Al`, and `Real`'s members do not look there.
- Fixture `b9e`: a promoted method of an embedded type (`Outer{ *Base }`, `o.Hello()`) is unresolved whether `Hello` is in `Base`'s file (`emb/`) or not (`emb2/`). That predates M9b.
- D5 says the internal `_test.go` methods on mattermost are "2 cases …, recorded". No M9b record names the two. "Out of scope" lists "Go methods in internal `_test.go` files attaching to production types (2 on mattermost)" as later work, while D5 already attaches them. One of the two statements should go.

**E10. Note: the duplicate session's commit, and what timestamps cannot show.**
- `5a310aa` was written by the killed duplicate session. It holds the B9 code, the two tests, m261–m264 plus m285, and amendment 2, and nothing else (`git show --stat`: `bench/mutants/m9b.json`, `resolve.rs`, `routes.rs`, the contract).
- `git diff 5a310aa HEAD -- crates/rb-indexer/src/resolve.rs` is a single line: P2 makes `package_key` `pub(crate)`. The mutants it added re-run KILLED, and E1's B9 survivor (e6) is in code it wrote.
- The P5 suite log runs 16:05–16:22, and `3b83d68` was committed at 16:27. No log ties the suite's tree to that commit: it has no HEAD line and no `git diff --quiet` check. The binary of record does: it re-builds from `3b83d68` to the same sha256 (`c7-rebuild.txt`). Next time, print `git rev-parse HEAD` and `git status --porcelain crates` at the top of `p5_suite.sh`'s log.

## Checked and held

Pass 2 can verify these without redoing them.

- **B9 (D5, C2).** Fixtures `b9`, `b9d`, `b9e`:
  - A sibling-file method resolves: `h.M()` → `m.go H#M`.
  - An internal `_test.go` method attaches: `h.TM()` → `h_test.go H#TM`.
  - The external `svc_test` package's own `H` gets no `M`, while `s.M()` on `*svc.H` from `svc_test` resolves to `svc/m.go`.
  - Generic receivers `(r *R[T]) Get()` and `(r R[T]) Val()` in another file resolve.
  - A package func `Do()` and a cross-file method `C#Do` of the same name: `Do()` → func, `c.Do()` → method.
  - The same method in `t_linux.go` and `t_windows.go` gives an ambiguous pair (`candidates: 2`, rank 1).
  - Two build-tagged declarations of `H` with the method in a third file give one heuristic edge (no duplicate candidate).
  - Methods on `strings.Builder`, `string` and an undeclared type: no crash, no edge, and a bare `Odd3()` gives no edge.
- **No bare call into a Go method is left.** `var/m9b/eval/b9/bare.py` reads the call token of every `CALLS` edge into a Go `T#M().` key in every Go dump. Head `3fc098445243`: 0 bare out of 25,961 (mattermost `noscip` 22,183, `scip` 3,635). Base `2e36cd607eab`: 32 bare in mattermost `noscip` (`attributeBasedAccessControlEnabled(…)`, `fileExists(…)`), consistent with amendment 6's 31 + 10 removed.
- **B10 (D6, C3).** Fixture `b10`:
  - Refused, as they should be: a parameter of the same name, a closure parameter, a local in an enclosing block, fewer values than verbs, more values than verbs, a non-string constant (`%v` of `5`), `iota`, an implicitly repeated constant, a shadowing local `fmt`, a dot-import of `fmt`, an aliased `f "fmt"`, and `other.Table`.
  - Folded right: a local in an *inner, closed* block (`BlockAfter`), a local declared *after* the call (`LateVar`, Go scoping), `'%%s'` → `'%s'`, a raw-string format, a concatenated format (`"… " + "%s"`), a constant `"t_" + "cat"`, a `string`-typed constant, a `fmt` parameter in *another* function, and a file importing `fmt` beside a sibling file that aliases `fmt` to another module.
  - `sql_folded` counts are right: `repo/use.go {sites 7, constants 7}`, one per other file.
  - Every folded edge is `heuristic`. `Sprintf` is not in `SQL_SINKS`, so a folded site can never be `exact`.
- **B12b (D7, C4).** E8's list. 46 `EXPOSES` and 46 `HANDLED_BY` in fixture `b12`, all `synthetic`.
- **N1.** m294 KILLED in my re-run, and `config_server_context_path_values_are_trimmed` pins both the padded and the blank value.
- **N2's list.** `timing_tests.py` regenerates `timing_tests.txt` byte for byte (`var/m9b/eval/records/timing_regen.txt`). I found no clock-asserting test it misses: the other clock reads are in `runner.rs`/`orchestrate.rs` code under test, in fixed generous `recv_timeout`s, or in print-only tests.
- **Evidence.** The gate, the scores, the invariant, the base re-score, the probe and the adjudication reproduce, as listed above (`var/m9b/eval/evidence/`). Base and head each have 38 manifest dumps and 119 PyCG cases, none missing or empty.
- **Records.** Every other number in the Result, amendments 1–6, CHANGELOG, IMPLEMENT, bench-findings, README and the docs traces to a log (`var/m9b/eval/records/trace.txt`). That includes:
  - the suite: 1,135 passed / 0 failed / 0 `skipping:` over 26 result lines;
  - the batteries 28/31/29, with exactly m261–m278 and m285–m294 in `m9b.json`;
  - the invariant buckets; 41/40 ambiguous rows; 37 endpoints (30 + 7 search);
  - 10 of 314 cells; the certainty table; the A/B medians; 37,031 `scip` `CALLS` all `exact`; the C7 sha256.
- **C5/C6.** Both are withdrawn consistently in the contract, CHANGELOG, IMPLEMENT, the design-doc row, todo and bench-findings. No doc mentions `scip_cross_project`. M9c carries C6 as its C10.
- **Spring Data REST facts used by the adjudication** (version chain Boot 1.4.2 → SDR 2.5.5 → Commons 1.12.5; collection, item and query-method resources; no base path in the checkout) hold against the 2.5.5 reference. `Path.cleanUp` strips leading and trailing slashes, as the code does.
- **Hygiene.** fmt and clippy CLEAN. My 8 `t_eval_m9b_*` repositories were deleted and `rb gc` run. `SELECT name, checkout FROM rb.repo` is identical before and after (`var/m9b/eval/repo-{before,after}.txt`). `git status` shows only the pre-existing `bench/__pycache__/` and `worktrees/`, plus this file.
