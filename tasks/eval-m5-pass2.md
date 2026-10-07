# M5 evaluator pass 2 (verification only)

- **Date:** 2026-10-07
- **Commit:** `5cb1a2c` (HEAD, clean tree at start), which claims to fix pass 1 (`tasks/eval-m5-pass1.md`, Z1–Z7) as contract amendment 16.
- **Scope:** the closing bar's pass 2, which is verification only. A result blocks only if it is one of: (a) a pass-1 fix that does not hold; (b) a regression; (c) a claim that fails through a form its own mechanism controls; (d) a nondeterministic dump or body; (e) a failed check. New forms in residue classes go to the "M6 review" list. Adversarial egress escape testing is open item 22, carried to M6, and is out of scope here.
- **Environment:**
  - Apple M4, Docker context `desktop-linux`, database `postgresql://rb:rb@127.0.0.1:55433/rb`. The database was empty at the start (0 repositories, 0 graphs) and is empty at the end.
  - Debug build from a clean `target/` (none existed). Load averages 3.2–7.2, since other applications were running.
  - The evaluator's objects were named `ev5b_*` and the working directory was `var/eval-m5-2/`. Both were removed at the end.

## Verdict first

**VERDICT: PASS.**
- Every Z1–Z7 repro, rebuilt end to end, gives the expected result, and the controls are unchanged.
- 12 of 12 mutants of amendment 16's conditions were killed.
- The verification loop is green twice.
- Invariant 1 holds: fixture dumps are byte-identical to phase 20's, and spring-petclinic differs from `afae548` only in the classified S19 (d) row. Both benchmark repositories give fresh == cached, equal to their recorded dumps.
- API ≡ CLI on 29 parameter sets of my own: 26 GETs plus 3 Cypher POSTs.
- No host paths. No blocking finding.
- The two notes below are non-blocking and go to the M6 review list.

## 1. Pass-1 repros, rebuilt end to end

All repros used the HEAD binary (`var/eval-m5-2/rb-head`, copied from the clean build).

| Finding | Repro (this pass) | Expected | Observed | Controls |
|---|---|---|---|---|
| **Z1** `status` of a never-indexed repository | `rb status --repo ev5b_nope --json`; `GET /v1/repos/ev5b_never_indexed/status` and `/snapshots`; `GET /v1/repos/ev5b_%C3%A9/status` | `not_found`, exit 3 / 404 | `not_found` `repository "…" is not indexed`, exit 3 / 404 on both surfaces, byte-identical bodies | status of a real repository: 200, unchanged |
| **Z2** a publish killed after its flip | `var/eval-m5-2/z2.sh`, the pass-1 repro with no hook (steps below) | the retired snapshot's Cypher is `invalid_param`; `rb snapshots`/`rb status` name no graph for it; the leftover is listed by `rb gc --dry-run` and healed by the next publish or by `rb serve` start | as expected, twice (details below) | Cypher on the old snapshot while it was live: exit 0; Cypher on the new live snapshot after the kill: exit 0; no `building` row |
| **Z3** path segments not decoded | `GET /v1/repos/ev5b%5Fpc/status`, `%5f`, `%65v5b_pc` (encoded first letter), `callers` with `%5F` | the real repository | 200, the real repository's body (`callers`: its own `nothing matches "x"`, not "not indexed") | `%2F` stays in the segment (`repository "ev5b/x" is not indexed`); `%255F` is decoded once (`"ev5b%5Fpc" is not indexed`); `%G1`, `%5` and `%FF` give 400 `invalid_param` with `param: "path"` |
| **Z4** duplicate `Host`; absolute-form authority | raw socket: `Host` twice in the orders own+evil, evil+own and own+own; `GET http://evil.example/…`, `http://evil.example:<port>/…`, `http://u@127.0.0.1:<port>/…`; `http://localhost:<port>/…` with `Host: 127.0.0.1:<port>` | 400 / 421 / 421 / 421 / 400 | 400 `invalid_param` (`param: "Host"`) ×3; 421 ×3; 400 (`Host must equal the request target's authority`) | `Host` own or `localhost`: 200; a foreign or missing `Host`: 421; absolute form with its own authority: 200; absolute form with no `Host`: 421 |
| **Z5** a `failed` snapshot is requested | a publish with `--age-load-dir` set to an unreadable scratch directory records a `failed` row (16892); `callers`, `find` and Cypher with `snapshot=16892` | `not_found` | 404 / exit 3 `snapshot 16892 of "ev5b_poly" is failed and holds no graph` on both surfaces, byte-identical | a never-issued id (99999999): 404 on both |
| **Z6** retention with more than one non-live graph | mutant m17 (`while let` → `if let` in `drop_non_live_graphs`) | killed | killed by `a_publish_stopped_after_its_flip_…` and `retention_holds_over_random_sequences` | — |
| **Z7** transport refusals | a 200,000-character target; 101 headers; a header line without a colon; `HTTP/9.9` | bare status, empty body, no `Server-Timing` | 414, 431 and 400 with empty bodies and no `Server-Timing`. `HTTP/9.9` gets 400 followed by a connection reset (the residue as worded) | a 65,000-character target reaches the server: 404 with a JSON body and `Server-Timing` |

**Z2 in detail.** These are the pass-1 steps, run twice: once on `ev5b_poly` healed by `rb serve`, once on `ev5b_poly2` healed by the next publish.
1. The repository is live at snapshot A.
2. A psql session holds `ACCESS SHARE` on `rb_<repo>_A._ag_label_vertex`.
3. `rb index` of a second commit flips to snapshot B. One lock waiter is seen.
4. `kill -9` the `rb index`, then end the holder.

**State after the kill:**
- The rows read `A:retired:rb_<repo>_A B:live:rb_<repo>_B`, and both graphs exist.
- `rb cypher … --snapshot A --json` exits 2 with `invalid_param`: "snapshot is not live: snapshot A has no AGE projection (only the live snapshot has one)". In pass 1 it exited 0. The API's `POST …/cypher?snapshot=A` gives 400 with the same bytes.
- `rb snapshots` shows `age_graph: null` for A. `rb status` names only B's graph.
- `rb gc --dry-run` lists `rb_<repo>_A`.

**Healing:**
- *By `rb serve`:* the server printed `rb serve: dropped leftover AGE graph rb_ev5b_poly_16879` on stderr before `listening`. One graph was left, and `gc --dry-run` then reports "nothing to remove".
- *By the next publish:* the leftover and B's graph were dropped. Only the new live snapshot's graph remained, with rows `A:retired:- B:retired:- C:live:rb_…_C`, and `gc` reports "nothing to remove".

## 2. Mutations (amendment 16)

- **Method:** each mutant ran against a clean build. For each one the file was copied aside, mutated by `perl -pi` (checked to differ), and the named suites were run. The file was then restored from its copy with a fresh mtime and byte-compared, and `git diff --quiet` was confirmed. The harness is `var/eval-m5-2/mut.sh`. No harness or server was left running.
- **Result:** 12 of 12 killed.

| # | Condition (file) | Mutant | Killed by |
|---|---|---|---|
| H1 | duplicate `Host` (`rb-server`) | `hosts > 1` → `> 2` | `host_headers_and_absolute_form_are_strict` |
| H2 | absolute-form authority allow-list | filter → `false` | same |
| H3 | authority = `Host` | filter → `false` | same |
| D1 | path segments decoded | `.map(decode_segment)` → identity | `path_segments_are_percent_decoded_once` |
| D2 | hex digits | `to_digit(16)` → `to_digit(36)` | same |
| S1 | m17 (`rb-store`) | `while let` → `if let` in `drop_non_live_graphs` | `a_publish_stopped_after_its_flip_…`, `retention_holds_over_random_sequences` |
| S2 | Cypher's `live` gate | `CASE WHEN status='live' THEN age_graph END` → `age_graph` | `a_publish_stopped_after_its_flip_…` |
| S3 | `SnapshotInfo` mask | `CASE …` → `s.age_graph` | same |
| S4 | sweep over every repository (own form) | `for repo in repos.iter().take(1)` | **survives** the `rb-store` suite; killed by e2e `a_publish_killed_after_its_flip_self_heals` only because other repositories sorted before the test's (note N2) |
| A1 | Z1 check (`rb-api`) | `if false && …` | `rb-server` `status_of_a_repository_never_indexed_is_not_found`; API suite (`("status", [])` → `null` kind) |
| A2 | m16 `kept_snapshot` | `status != "building"` | API suite (`callers snapshot=<failed>` answered `nothing matches "a"`) |
| C1 | `rb serve`'s sweep call (`rb-cli`) | call removed | e2e `a_publish_killed_after_its_flip_self_heals` (two graphs after the server started) |

## 3. Regression

- **`var/m5/scripts/verify.sh`, run twice** (logs `var/m5/logs/ev2a-*`, `ev2b-*`):

| Check | Run 1 (`ev2a`) | Run 2 (`ev2b`) |
|---|---|---|
| fmt / clippy `-D warnings` | ok / ok | ok / ok |
| unit outside e2e and api | 639 passed, 0 failed | 639 passed, 0 failed |
| `rb-store` with the database | 65/65 | 65/65 |
| `rb-server` with the database | 12/12 | 12/12 |
| e2e | 30/30 (299.8 s), the 7 `scip_*` tests ran (egress probes included) | 30/30 (275.2 s), the 7 `scip_*` tests ran |
| api | 1/1: 3 × 50 sets identical; statuses 200 ×100, 400 ×21, 404 ×18, 409 ×8, 410 ×1, 422 ×3 | same |
| `skipping:` | 0 everywhere | 0 everywhere |
| listener | none | none |
| stub scan | only `rb-sql/src/lib.rs:1111` (a test input) | same |

- **Invariant 1:**
  - **Fixtures:** `fixture_dumps.sh` with the HEAD binary. `ts-express-prisma`, `java-spring-jpa` and `polyglot-monorepo` are byte-identical to `var/m5/p20/dumps/`.
  - **spring-petclinic @ `818c4136ea97`:** shallow clone at `var/eval-m5-2/petclinic`, indexed `--no-scip`.
    - fresh (`--no-cache`), cached and a second repository name give byte-identical dumps, and these equal `var/m5/p20/m4b/spring-petclinic.jsonl`.
    - Against the `afae548` binary (`var/m5/base/rb-afae548`, sha256 `c575d98c…`, re-run on this clone and equal to `var/m5/base/m4b/spring-petclinic.jsonl`), exactly one row differs: `orphan_coverage` `supertype` 10 → 5, `convention` 0 → 5. That is S19 (d), as classified.
- **Benchmark determinism (closing bar 2):** HEAD binary, `--no-scip`, one repository at a time, removed after each.

| Repository | fresh (`--no-cache --force`) | cached (`--force`) | fresh == cached | Equal to the recorded dump | Lines |
|---|---|---|---|---|---|
| shardingsphere @ `b09dfffcf9e5` | 138.5 s (debug) | 117.0 s | yes | `var/m5/bench/out/ss-noscip-fresh.jsonl`: yes | 314,103 |
| mattermost @ `b2ce71db9c18` | 114.4 s | 74.3 s | yes | `mm-noscip-fresh.jsonl`: yes | 235,422 |

  Timings are not measurements of record: debug build, load 4–7. Latency was not re-measured (closing bar 2).
- **API ≡ CLI, my own sets** (`var/eval-m5-2/ident.py`, `cases.tsv`): 26 GET sets are byte-identical to the CLI's stdout, with each status mapped from the exit code (200 ×14, 400 ×4, 404 ×7, 409 ×1). They cover:
  - an encoded first letter and `%5F` in the repository segment;
  - `find` with `kind`, `limit` and `offset`, with a unicode `q` (`%C3%96`) and with `a%2Bb`;
  - `callers` with `depth=3&include_references=true`, with `min_certainty=exact`, with an empty target and with `depth=0`;
  - `impact` with default precision and with `precision=rta`; `table-deps` with `hops=1`; `path` with an encoded `{ownerId}`;
  - `services` with paging; `snapshots` with an offset; `diff` live and between explicit ids with paging;
  - a retired-snapshot `callers` (409 ambiguous); `failed` and never-issued snapshots;
  - never-indexed `status`, `snapshots` and `callers` (encoded and unicode names);
  - `limit=100000`.

  Cypher `POST` for live, retired and failed snapshots is byte-identical as well (200 / 400 / 404). The 26 bodies are byte-identical across two server runs.
- **No host paths:** none in the 3 fixture dumps, the 3 public-repository dumps or the 26 API bodies.
- **Docs vs code:**
  - Amendment 16 (a)–(f) matches the code and the tests read and run above.
  - The README's Host paragraph matches the probes: 421/400 rules and decoding once. The read-only and retention paragraphs match the Z2 runs: the stderr line, healing by publish and by serve, `gc` listing the leftover.
  - The soundness table's REST, Retention and Server-exposure rows match amendment 16, apart from note N1.
  - Open items 1–23 are present with targets; item 22 (egress review → M6) and item 23 (Z7 residue) match this pass.

## 4. S1–S28 and invariants 1–11 (as now worded)

| # | Status | Evidence (pass 2) |
|---|---|---|
| S1 | PASS | DB suite 65/65 twice; pass 1's concurrent-read measurement stands |
| S2 | PASS | lock in `drop_one_graph`/`gc_apply`/publish; measurement recorded (not re-run, as in pass 1) |
| S3 | PASS (fixed) | Z2 repro twice; mutants S1–S4 (S4 see N2) |
| S4 | PASS | `rb gc --dry-run` lists the leftover, then "nothing to remove" after healing |
| S5 | PASS | e2e; petclinic, shardingsphere and mattermost fresh `--no-cache` == cached |
| S6 | PASS | e2e parity twice |
| S7 | PASS | e2e; Cypher on a retired snapshot `invalid_param` |
| S8 | PASS | e2e 30/30 twice, no `skipping` |
| S9, S10 | PASS | units and e2e twice |
| S11 | PASS | envelopes in all 29 identity sets |
| S12 | PASS (fixed) | Z1 on both surfaces; mutant A1 |
| S13 | PASS (fixed) | Z3/Z4 probes and controls; mutants H1–H3, D1, D2, C1; note N1 |
| S14 | PASS | API suite twice (150 sets); 29 own sets identical; Z7 as residue |
| S15 | PASS | paging sets identical (`find`, `services`, `snapshots`, `diff`) |
| S16, S17 | PASS | e2e; `diff` sets identical |
| S18, S19 | PASS | invariant 1 (petclinic: the one S19 (d) row; fixtures equal phase 20) |
| S20 | PASS | records unchanged |
| S21 | PASS | `scip_egress_probes_from_a_fetch_container` ran twice; escape testing is open item 22 (M6) |
| S22 | PASS (recorded) | — |
| S23–S26 | PASS (recorded) | determinism re-checked on both benchmark repositories (section 3); latency not re-run (closing bar 2) |
| S27 | PASS | section 3 |
| S28 | PASS | verification loop twice; this pass |

**Invariants:**

| # | Invariant | Status |
|---|---|---|
| 1 | dumps unchanged except named items | PASS |
| 2 | API ≡ CLI | PASS |
| 3 | no silent truncation | PASS |
| 4 | whole-snapshot reads | PASS |
| 5 | retention | **PASS (fixed: Z2)** |
| 6 | projection parity | PASS |
| 7 | read-only, local-only | PASS (N1 noted) |
| 8 | determinism | PASS |
| 9 | bounded work | PASS |
| 10 | complexity by counted operations | PASS (by the suite's tests) |
| 11 | egress | PASS |

## Notes (non-blocking; M6 review list)

- **N1 (doc precision).** `rb serve` runs its startup sweep (`sweep_leftover_graphs`, `crates/rb-cli/src/main.rs:437`) before `rb_server::serve` checks that the bind address is loopback (`crates/rb-server/src/lib.rs:64`). So `rb serve --bind 0.0.0.0:…` still drops leftover graphs and then refuses to start. No request is answered, so the claim holds: the sweep, before the first request, is the server's only write. But the README's "before it listens" also covers a server that never listens. *Suggested fix:* validate the bind before the sweep, or say so in the README.
- **N2 (test coverage).** The startup sweep's "every repository" is not pinned by a test that is independent of database state.
  - Mutant S4 (sweep only the first repository name) survives the `rb-store` suite: `sweep_graphs` is tested with one repository.
  - The e2e kills it only when another repository sorts before the test's (here the evaluator's `ev5b_*`). In an otherwise empty database it would survive.
  - *Suggested test:* a `rb-store` test with leftovers in two repositories.
- **Not a finding.** Decoding also applies to the fixed segments (`/%76%31/repos/…` routes as `/v1/repos/…`), which RFC 3986 §6.2.2.2 permits. `HTTP/9.9` gets a 400 followed by a reset, within residue item 23.

**M6 review list (from this pass):** N1; N2; and open item 22 (adversarial egress review of the fetch phase), carried as decided.

**Not done in this pass (stated, not claimed):**
- egress escape attempts (open item 22);
- latency re-measurement (closing bar 2: these are recorded numbers);
- re-running the S2/S8/S22 load measurements;
- SCIP fresh == cached on the benchmark repositories (only `--no-scip`; SCIP determinism is covered by the fixtures' `scip_*` e2e tests, run twice).

VERDICT: PASS
