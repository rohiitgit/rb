# M7 close: independent evaluation, pass 1 (2026-10-13)

**VERDICT: FAIL, 17 defects** (S1 0, S2 3, S3 9, S4 5). No wrong `exact` answer was found. The S2s are a lying `case: folded` attribute, `rb doctor` hanging, and corrupted checkout data in the shared DB.

Scope: `git show 735d3ae` (M7h) and `git show 77675c8` (M7i) over crates/ and migrations/; contracts m7h/m7i; logs under var/m7h, var/m7i; the register; README and usage guide. Binary: `var/m7i/bin/rb` (`var/m7h/bin/rb` for before/after). Scratch repos `st_eval_*` were deleted afterwards and `rb gc` dropped 7 graphs. A sub-audit (register, docs) contributed items 11–17. Where I did not re-run its evidence myself, the item says "sub-audit".

## Findings

1. **S2: `case: folded` is copied onto every ambiguous candidate, including exact-case ones.**
   - Code: crates/rb-indexer/src/stitch.rs:1035-1043 builds `attrs` from `best[0]` only, and :1080-1090 clones them into every candidate link.
   - Repro: an Express app with `app.get("/api/Users", a)` and `app.get("/api/users", b)`, and a web client calling `fetch("/api/users")`. `rb dump` gives two CONSUMES edges, each with `{"candidates":2,"case":"folded",…}`. One of them goes to `HTTP GET /api/users`, which matched without folding.
   - The reverse also happens: when `best[0]` is exact-case, a folded candidate loses the attribute. That contradicts contract M7i S1 ("such a CONSUMES says `case: folded`").
   - Related: an exact-case client that M7h linked to one endpoint is now `ambiguous` (`http: 2 routes …; 1 client calls: 1 ambiguous`). That is defensible under Express semantics, where the first registered route wins, but no contract or doc records it.
   - Fix: set `case` per link from that candidate's own `PathMatch`. Then either prefer exact-case over folded matches in `most_specific`, or document the ambiguity.
2. **S2: `rb doctor` hangs when the database accepts TCP but never answers.**
   - Code: `Store::connect` (crates/rb-store/src/lib.rs:583) calls `Client::connect` with no connect timeout. doctor.rs:71 calls it first, yet doctor.rs:4 claims "Every probe is bounded".
   - Repro: a Python listener on 127.0.0.1:55998 that accepts and stays silent. `rb --database-url postgresql://rb:rb@127.0.0.1:55998/rb doctor` was still running after 25 s and had printed nothing (killed).
   - Fix: build a `postgres::Config` with `connect_timeout` (for example 5 s) for doctor, or for `Store::connect` generally, and report a timeout as `fail`.
3. **S2: the shared database's checkout data is corrupted.**
   - `SELECT name, checkout FROM rb.repo` shows all 5 long-lived repos (alvorabackend, fx_date, fx_full, fx_man, tmpzc8i9d) with checkout `/y/demo`. That literal appears only in crates/rb-store/src/db_tests.rs:1372.
   - Effect: from a git dir named `fx_full`, `rb find x` gives `--repo is needed: repository "fx_full" was indexed from another checkout ("/y/demo")`. Before, the directory-name fallback would have answered.
   - Cause (unverified): most likely a mutant that dropped `WHERE name = $1` from `record_checkout` (lib.rs:748) was run against the shared DB, not a `ScratchDb`.
   - Fix: run `UPDATE rb.repo SET checkout = NULL WHERE checkout = '/y/demo'`. Run DB mutants against scratch databases, and add a lesson.
4. **S3: after `--limit`, the certainty histogram counts only the rows shown, while the summary counts everything found.**
   - Code: crates/rb-api/src/lib.rs (Callers and Impact arms) applies `limit_callers`/`limit_impact` before `histogram(...)`. crates/rb-cli/src/code.rs builds the text histogram from the cut tree.
   - Callers: `rb callers 'b()' --depth 3 --limit 1` printed `summary: 5 callers found, 1 shown` and then `certainty: heuristic 1`. The JSON `certainty` is `{'heuristic': 1}`.
   - Impact: `rb impact 'b()' --limit 1` printed `breaks … (4, 1 shown)` and then `certainty: heuristic 2`.
   - Fix: compute the histogram before cutting, or label it `(shown rows)`.
5. **S3: JSON spec spans make indexing quadratic in spec size.**
   - Code: crates/rb-frameworks/src/spec.rs:321-332 and :351-362. For every path, `json_key_at` re-scans from `paths_at`, and `text[..at].matches('\n')` and `rfind` re-scan from offset 0. None of this is charged to the work budget (`tick`).
   - Measured with `rb index --no-scip`, M7h vs M7i:

     | Spec | M7h | M7i |
     |---|---|---|
     | 3.0 MB, 3,000 paths | 1.02 s | 2.59 s |
     | 7.5 MB, 20,000 paths | 10.3 s (user 0.2 s) | 45.0 s (user 34.7 s) |

   - Fix: one forward pass that records key offsets and counts lines incrementally.
6. **S3: the M7i "1,041 passed, 0 failed, 0 skipped" has no full-run log.**
   - var/m7i/logs/p5-workspace.log sums to 1,037 passed and 4 failed (`api_bodies_equal_cli_json`, `json_answers_snapshots_and_error_codes`, `scip_contract_packs`, `spring_di_rta_and_orphan_goldens`). bless.log covers only 3 tests. No log re-runs the parity test.
   - I re-ran `cargo test -p rb-cli --test api`: 1 passed. So the claim is plausible but unproven.
   - The other numbers check out against the logs: M7h 1,028/0/0; invariants 31/31 (sha `9e732a8b`) and 29/31 (sha `cebfdc60`, bookcars and go-gin as classified); O4 RSS 2,148/2,188 → 1,658/1,653 MiB, fresh 1,946 MiB.
   - Fix: re-run the suite and keep the log, or reword the claim to "1,037 plus 4 re-run individually".
7. **S3: `rb path` text never shows `case: folded`, though the usage guide says the link "says" it.**
   - docs/usage-and-testing.md:160-161. `stitch_note` (crates/rb-cli/src/path.rs:14-37) has no `case` arm.
   - My run of `rb path 'GET /api/users'` printed `[ambiguous] (2 candidates)` and nothing about case. Only `--json` (`stitch`) carries it.
   - Fix: add `case folded` to `stitch_note`, or say "in `--json`".
8. **S3: the docs say `find --kind` lists everything, but it pages at 20.**
   - README.md:89 says "lists every endpoint"; docs/usage-and-testing.md:165 says "lists every node of a kind".
   - The default `--limit` is 20. Sub-audit: `rb find --repo alvorabackend --kind endpoint` gave 20 rows, then `rows 1-20 of 185 listed; next page: --offset 20`.
   - Fix: "lists that kind, 20 at a time (`--limit`, `--offset`)".
9. **S3: README.md:96-97 says `--limit` gives "a summary of everything found (per service, per entry kind)".**
   - That is true of `callers` only. `impact` has no summary; it prints `(N, M shown)` per list header (crates/rb-cli/src/code.rs:157-163). Verified with the impact run in item 4.
   - Fix: describe the two commands separately.
10. **S3: `rb doctor` rows do not all match contract M7h S5 ("each … with a fix") or the docs (`ok`/`warn`/`fail`).**
    - The `postgres` and `migrations` `fail` rows have no fix (doctor.rs:95, :99).
    - A fourth status, `skip`, is undocumented (doctor.rs:21, :103-113).
    - A wedged Docker is reported as `warn`; the M7h design text says `fail` (doctor.rs:219).
    - Fix: add the fixes, document `skip`, and align the design text with the code.
11. **S3: the register's summary counts are stale.**
    - tasks/stress-findings.md:22 says "31 findings", and the severity table has S3 = 16.
    - F32 (S3, row 117) is missing, so it should read 32 findings and S3 = 17.
    - Line 264 still says M7a is "uncommitted"; it is 372963c.
12. **S3: U4 ✅ is only a partial fix.**
    - The row (stress-findings.md:203) asks that "`--pick` beyond the candidate count is an error".
    - crates/rb-api/src/render.rs:47 `([only], _) => Ok(*only)` ignores it. Repro: `rb callers 'c2()' --pick 3` (one candidate) exits 0 and answers.
    - Fix: mark ◐ with residue, or reject `pick > 1` when there is a single candidate.
13. **S3: U1 ✅ is only a partial fix.**
    - The row (stress-findings.md:200) asks to match "the git toplevel or root commit".
    - Only the checkout path is matched (crates/rb-cli/src/main.rs `infer_repo`). A moved clone gets the "another checkout" directive (seen live, item 3).
    - Fix: mark ◐ and record the residue.
14. **S3: nine register rows have no status at all.**
    - U2, U3, U6, U8, U10, O1, O2, O5 and O6 (stress-findings.md:201-220) are neither ✅, ◐, declined nor residue.
    - The sub-audit says work landed for U3 (M7f), U6 (M7g), U8/O6 (M7e) and O2/O5 (M7b). U2 (basename `file:N`, trimmed input) and U10's coverage gate look open.
    - Fix: give every row a status.
15. **S4: the O4 ✅ hides a regression in the fresh-index peak.**
    - The fresh peak is now 1,946 MiB against 1.55 GiB at M5 (var/m7i/logs/o4.txt). The register's "What held up well" section and the O4 row do not mention it; only contract-m7i's residue does.
    - By peak memory footprint (o4_*.err), the gain is only 1,567 → 1,430 MB.
    - Fix: put the drift in the register.
16. **S4: smaller doc and record inaccuracies.**
    - README.md:187: "neither repeats the other's work" is untrue under `--force` or another `--rev`.
    - README.md:201 omits that `caseSensitive: false` and `app.set/enable("case sensitive routing")` also count as opting in (routes.rs:754-766, :862-868).
    - tasks/todo.md (M7h phase 1) names `Store::{repos_at_checkout, repo_checkout}`. Neither exists; the function is `repo_checkouts`.
17. **S4: two fragile or over-broad behaviours.**
    - Cypher error classification matches English server text (`message == "return row and column definition list do not match"`, `starts_with("permission denied for …")`, crates/rb-store/src/lib.rs:519-541). A non-English `lc_messages` turns these back into a plain `cypher:` refusal.
    - `target_entry` names any entry mark (`test`, `main`), not only a handled endpoint as contract M7i S2 states. The callers header of every marked target changes without a flag (crates/rb-store/src/code.rs:614).

## Reported by the sub-audit, not independently verified
- F23 ✅: Alembic `op.create_table` is still not read.
- F28 ✅: BCL types are still unresolved, and `AddDbContext` is unread.
- F9/O1: the fix is quadratic, not linear.
- F11/O5: the junk endpoints remain and no flag was added.
- F25: the Java `blockingStub` carrier is absent from the residue.
- F1/Batch B: there is no CI job with a DB.
- The "Observations" (`RB_TEST_CAPS=garbage`, `diff` ignoring `--repo`, …) were never classified.
- Each should be ◐ or residue if confirmed.

## Checked and found sound
- **Checkout inference:** a symlinked checkout resolves; `rb index .` without `--repo` re-uses the recorded repo ("up to date"); two names at one checkout give the "several repositories" directive for questions and for `rb index .`; `status` still lists everything.
- **Index lock:** it is taken on the indexing connection and released at exit. A cached-facts decode error aborts the index (`?` at crates/rb-indexer/src/syntax.rs:240), so the connection is never reused mid-stream.
- **`--limit` and `--exclude-tests`:** `--limit 1` against a first row with children shows just that row. `--exclude-tests` counted 2 and dropped the callers behind them.
- **Handler callers:** the hint `rb path 'GET /api/users'` works as printed.
- **`rb doctor` on this machine:** 10 rows ok in 0.24 s, no probe file left behind.
- **Targeted tests pass:** the e2e index lock, doctor and checkout tests; rb-indexer `case`; rb-frameworks `openapi_json`; rb-cli `--test api`.
