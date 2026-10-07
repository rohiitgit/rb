# M5 evaluator pass 1 (full review)

- **Date:** 2026-10-07
- **Commit:** `9a2719f` (HEAD, clean tree at start)
- **Environment:** Apple M4, Docker context `desktop-linux`, database `postgresql://rb:rb@127.0.0.1:55433/rb` (empty at start: 0 repos, 0 graphs), debug build from a clean `target/`.

## Measured baseline

| Check | Result |
|---|---|
| `cargo fmt --all --check` | clean (exit 0) |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean (exit 0) |
| Unit tests (`--lib --bins`, no database; summed from the log) | **634 passed**, 0 failed |
| `rb-store` with the database | **64/64**, no `skipping:` |
| `rb-server` with the database | **8/8**, no `skipping:` |
| e2e (`rb-cli --test e2e`) | **29/29** in 243.9 s, no `skipping:` (the `scip_*` tests and `scip_egress_probes_from_a_fetch_container` ran) |
| API identity (`rb-cli --test api`) | **1/1** |
| Stub scan (`todo!`, `unimplemented!`, `TODO`, `FIXME`, `dbg!`) | 1 hit, a test input string in `rb-sql` (`/* TODO`): clean |

The claimed baseline is reproduced.

## Criteria

| # | Verdict | Evidence |
|---|---|---|
| S1 whole-snapshot reads | PASS | Two processes: 12 publishes (`--keep 0`, alternating two commits) against a `rb dump` loop. 35 dumps were each byte-identical to the reference for their commit, with 0 mixed and 0 errors. Mutant m12 (READ COMMITTED) was killed. |
| S2 AGE DDL lock | PASS (not re-measured) | Lock present in `drop_one_graph`/`gc_apply`/`project`. The phase-0/1 measurement is recorded; not re-run in this pass. |
| S3 retention | **FAIL** | Z2: a killed publish leaves a second AGE graph. Otherwise held: `--keep 0` left only the live row and one graph. Mutants m08 and m09 killed; m17 survived (Z6). |
| S4 `rb gc` / `cache prune` | PASS | `rb gc --prefix ev5_` listed and dropped Z2's leftover graph. No `building` row was found after the kill. |
| S5 `--no-cache` writes | PASS | e2e `no_cache_writes_the_facts_it_extracted`. Petclinic fresh `--no-cache` dump == cached dump (`cmp`). |
| S6 projection parity | PASS | After the gc cycle, AGE has 105 vertices / 174 edges, equal to the relational rows. e2e checks full parity. |
| S7 Cypher guard rails | PASS | Through the API: `CREATE`, `SET` and `DETACH DELETE` are refused, `pg_read_file(...::pg_text)` gets permission denied, and the rows are intact. Mutants m10e (role removed) and m11e (row cap `>=`) were killed by the e2e; both survive the `rb-store` unit suite alone. |
| S8 e2e robustness | PASS (not re-measured under load) | e2e green. |
| S9 prebuilt index | PASS | Goldens unchanged (e2e). Server index reuse was observed (first request 37.8 ms, then about 2 ms). |
| S10 guard rails complete | PASS | Unit and property tests. Every `truncated` list was empty in 285 latency-script requests. |
| S11 JSON everywhere | PASS | — |
| S12 request layer / D5 errors | **FAIL** | Z1: `status` of a never-indexed repository is a 200 with zero counts, not `not_found`. |
| S13 `rb serve` | PASS, with minor Z3 and Z4 | Binds on `0.0.0.0`, `[::]`, `[::ffff:127.0.0.1]` and `[::ffff:0.0.0.0]` are refused; hostnames are not accepted by `--bind`. Host values: `localhost.`, `127.0.0.2`, a bare `127.0.0.1` without the port, a trailing dot, a foreign host and a missing Host all get 421. No `Access-Control-*` header (OPTIONS → 405 `allow: GET`, unknown path → 404). text/plain POST → 415, DELETE → 405. Queue full → 503 `busy`; held past the deadline → 503 `deadline_exceeded` with no result. Mutants m04–m07 killed. |
| S14 API ≡ CLI | PASS | API suite green. My own comparisons were byte-identical for: status, unicode `q`, `%2B`, empty `q` (400/2), offset `i64::MAX`, services paging, a missing target (404/3), `snapshot=0` (400/2), a never-issued snapshot (404/3), diff with no parent (400/2), a 20,000-character target, and the README's `cmp` example. Differences seen were only the expected `+`→space form decoding, repeated params (the CLI can't express them), Z3 and Z7. |
| S15 pagination | PASS | Real petclinic diff paged at `--limit 7`: 6 pages concatenate to the unpaged sections, and the totals match. |
| S16 `rb diff` exact | PASS | spring-petclinic `88e37c1` → `676db04` (a Java change: 5 nodes added, 3 changed, 25 edges added, 37 evidence-only). Applying the diff to A's N and E lines gives B exactly (0 mismatches). `diff(B,A)` is `diff(A,B)` inverted (added/removed swapped, before/after swapped). `diff(B,B)` is empty. `tools_differ` is `[]`. Mutants m14 and m15 killed. |
| S17 diff summaries | PASS | e2e goldens. |
| S18 / S19 | PASS | Invariant 1 rows below. |
| S20 residue records | PASS (records read, not re-derived) | — |
| S21 egress | PASS | The probe suite ran in the e2e (not skipped). By construction, a repository's `.npmrc`, `.mvn/maven.config` or `settings.xml` override cannot widen egress: the fetch network is `--internal`, so the proxy's allow-list is the only route. No egress escape attempts beyond the project's probe suite were made in this pass. |
| S22 per-repository caches | PASS (recorded; not re-measured) | — |
| S23–S26 benchmark | PASS (recorded) | `var/m5/scripts/latency.py` re-run against a petclinic snapshot: 285 requests, all 200, nearest-rank percentiles (`ceil(p·n)−1`), load recorded. The script is correct. The §12.5 SCIP miss is documented with its `sample` profile (S24, open item 1). |
| S27 determinism / invariant 1 | PASS | Two repositories against `var/m5/base/rb-afae548` (sha256 `c575d98c…`). spring-petclinic @ `818c413`: exactly 1 row differs (`orphan_coverage` supertype 10→5, convention 0→5; S19 d). `java-spring-jpa`: exactly 2 rows (`UserRepositoryCustomImpl#` loses `orphan_eligible`; `orphan_coverage`; S19 d). Same commit under two repository names, fresh vs cached: byte-identical. No `/Users/` in dumps or API bodies. |
| S28 verification loop | PASS (evaluator passes pending) | Baseline above. |

**Invariants:** 1 PASS · 2 PASS (Z3 caveat) · 3 PASS · 4 PASS · **5 FAIL (Z2)** · 6 PASS · 7 PASS · 8 PASS · 9 PASS · 10 PASS (by the suite's counted tests; not independently re-derived) · 11 PASS.

**Mutants:** 17 of my own (m01–m17), with 4 re-runs against the e2e. **15 killed, 2 survived** (m16, m17 → Z5, Z6). Every file was restored and byte-compared, and `git status` was clean after each run.

## Findings

### Z1 (major): `status` of a repository that was never indexed answers 200 with zero counts

- **Repro:** `rb status --repo ev5_never_indexed --json` gives exit 0 with `{"result": {"name": "ev5_never_indexed", "live": null, "nodes": 0, "edges": 0, "diagnostics": 0, "snapshots": 0}}`. `GET /v1/repos/ev5_never_indexed/status` gives 200 with the same body. `rb snapshots` and `rb callers` on the same name give `not_found`, exit 3 (`repository "ev5_never_indexed" is not indexed`).
- **Expected:** `not_found`, exit 3 / 404 (D5; amendment 5(c): "A repository that was never indexed is `not_found`").
- **Where:** `crates/rb-api/src/lib.rs:809-816`. `status()` calls `store.status(name)`, and `status_rows` (`crates/rb-store/src/lib.rs:1470-1496`) never checks that the repository exists.
- **Why major:** a false answer (a repository that "exists with no snapshots") instead of an error, on both surfaces. Combined with Z3, any client that percent-encodes the path gets this false answer for a real repository. The API identity suite's never-indexed row does not cover `status`.
- **Regression test:** `status` of an unknown repository → `not_found`/404/exit 3 in the API identity table and as an `rb-api` unit.

### Z2 (major): a publish killed between the flip commit and retention leaves a second AGE graph, and Cypher then answers on a retired snapshot

- **Repro:**
  1. `ev5_poly` is live at 15721.
  2. A separate session holds `ACCESS SHARE` on `rb_ev5_poly_15721._ag_label_vertex`.
  3. `rb index` of a second commit publishes. 15722 becomes live and committed, and retention's `drop_graph` waits on the lock.
  4. `kill -9` the `rb index` process, then release the lock.
  5. State: `15721 retired rb_ev5_poly_15721`, `15722 live rb_ev5_poly_15722`, and `ag_graph` holds **both** graphs.
  6. `rb cypher 'MATCH (n) RETURN count(n)' --repo ev5_poly --snapshot 15721 --json` answers exit 0 with a full result.
- **Recovery:** `rb gc --dry-run` lists the graph, and `rb gc` drops it.
- **Expected:** the soundness table says retention "leaves exactly one AGE graph per repository: the live snapshot's", and closing bar 3(c) names "leaving a second AGE graph". D17 says `rb cypher` on a retired snapshot is `invalid_param`.
- **Where:**
  - `crates/rb-store/src/lib.rs:790-808`: the flip commits in `load_and_flip`, then `retain` drops the old graph in a later transaction, so a crash in between is unrecoverable until the next publish or a manual `rb gc`.
  - `crates/rb-store/src/lib.rs:1416-1427`: `Store::cypher` gates on `age_graph IS NOT NULL` only, not on the snapshot's status.
  - `crates/rb-api/src/lib.rs:892-905`: `kept_snapshot` accepts `retired`.
- **Not seen:** no `building` row; no CSV left in `var/age-load`.
- **Suggested fix direction:** gate Cypher on `status = 'live'` (D17), and either drop the previous graph inside the publish transaction or make every read path and `rb status` treat a non-live graph as absent. The contract's residue currently names only pre-M5 `building` rows as left for `rb gc`.
- **Regression test:** a `rb-store` database test with the existing `hooks` (fail or kill after the flip commit, before `retain`); assert that Cypher on the retired snapshot is `invalid_param` and that the next publish leaves one graph.

### Z3 (minor): path segments are not percent-decoded; an RFC-equivalent repository name is a different repository

- **Repro:** `GET /v1/repos/ev5%5Fpoly/status` → 200 with `"repo": "ev5%5Fpoly"`, zero counts (via Z1). `GET /v1/repos/ev5%5Fpoly/callers?target=x` → 404 `repository "ev5%5Fpoly" is not indexed`. `rb status --repo ev5_poly --json` answers the real repository.
- **Expected:** RFC 3986 §6.2.2.2: percent-encoded unreserved characters are equivalent to the characters. Either decode, or refuse `%` in the segment as `invalid_param`.
- **Where:** `crates/rb-server/src/lib.rs:253`, `:386-405` (`route(parts.uri.path())` on the raw path).
- **Regression test:** an `rb-server` unit with `%5F` and `%2F` in the repository segment.

### Z4 (minor): duplicate `Host` headers and absolute-form URIs with a foreign authority are answered

- **Repro (raw socket):**
  - `Host: 127.0.0.1:P` then `Host: evil.example` → `200 OK`; in the reverse order → 421.
  - `GET http://evil.example/v1/repos/ev5_poly/status` with `Host: 127.0.0.1:P` → `200 OK`.
- **Expected:** RFC 7230 §5.4: a request with more than one `Host` header MUST get 400. For an absolute-form request the URI's authority is the effective host, so it should get 421.
- **Where:** `crates/rb-server/src/lib.rs:236-241` (`headers.get(HOST)` takes the first; `parts.uri.authority()` is ignored).
- **Impact:** browsers cannot send either form, so DNS rebinding is not reopened. It is a gap in the "only requests whose `Host` names that address" claim, not a remote exposure.
- **Regression test:** rows in `a_foreign_host_is_misdirected` for both forms.

### Z5 (minor, test coverage): requesting a `failed` snapshot is untested (mutant m16 survived)

- **Mutant:** `crates/rb-api/src/lib.rs:894`, `snap.status == "live" || snap.status == "retired"` → `snap.status != "building"`. It survives the `rb-api` units, the API identity suite and `json_answers_snapshots_and_error_codes`.
- **Expected:** amendment 6(c), "a requested snapshot … `failed`, is `not_found`", needs a row.
- **Regression test:** an injected failed publish, then `--snapshot <failed id>` → `not_found`/404.

### Z6 (minor, test coverage): retention with more than one non-live graph is untested (mutant m17 survived)

- **Mutant:** `crates/rb-store/src/lib.rs:1015`, `while let Some(graph) = self.drop_one_graph(...)` → `if let`. It survives the `rb-store` database suite (including `retention_holds_over_random_sequences`) and `gc_and_cache_prune_commands`.
- **Why it matters:** this is exactly the recovery path after Z2. The next publish must drop two graphs (the leftover and the previous live one); under the mutant one remains.
- **Regression test:** the property test should include leftover graphs (an interrupted retention) among its steps.

### Z7 (doc): over-long request URIs get a bare `414` with no JSON body

- **Repro:** `GET …/callers?target=<200,000 × a>` → `414`, 0 bytes. The CLI answers the same target with JSON `not_found` (a 20,000-character target is byte-identical on both surfaces).
- **Expected:** the S14 residue names only connection resets and client timeouts as non-answers. Hyper's URI limit (and its header-size limit) should be stated there as transport-level refusals.
- **Where:** `crates/rb-server/src/lib.rs` (hyper defaults); `tasks/contract-m5.md` soundness table, REST row.

## Open items: complete and honest?

The 21 consolidated items match what I measured or read. The §12.5 SCIP miss carries its profile, and the benchmark gaps are stated as gaps. Missing items: Z2's crash window is not among the residues (the contract says only pre-M5 `building` rows are left to `rb gc`), and Z7's transport-level refusals are not stated.

## Closing readiness

Not ready. Z1 and Z2 are claim failures of the kind closing bar 3(c) lists (a false API answer instead of an error; a second AGE graph, and with it Cypher on a retired snapshot). Each needs a test-first fix as an amendment, plus regression rows for Z5 and Z6. Z3, Z4 and Z7 are cheap and should go in the same amendment. Everything else verified in this pass held: the baseline, invariant 1 on two repositories, `rb diff` apply/inverse on a real code change, whole-snapshot reads under concurrent publishes, server exposure rules, read-only Cypher, the egress probe suite, and the README examples (run as printed against an `ev5_` copy).

**Not done in this pass (stated, not claimed):** egress escape attempts beyond the project's own probe suite; re-running the S2/S8 load measurements and the S22 measurement; independent re-derivation of the counted-growth tests (invariant 10).

VERDICT: FAIL
