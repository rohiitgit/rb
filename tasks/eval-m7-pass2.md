VERDICT: FAIL (1 blocking)

# M7 close: independent evaluation, pass 2 (2026-10-13)

Scope: `git show a60cc90` (crates/ and records), contract-m7j, the M7h/M7i "Amendment 2" sections, `tasks/stress-findings.md`, `var/m7j/logs/`. Binary `var/m7j/bin/rb` (sha `711f0e2d…54c3`, matches `rb.sha256`); `var/m7i/bin/rb` for before/after. Scratch repos `st_eval2_*` (case, spec, dbg, old) were deleted and `rb gc` dropped 4 graphs. The shared DB again holds only alvorabackend and fx_{man,date,full}, all with checkout NULL.

The one blocking item is a records overstatement (U2 ✅), the same kind as pass-1 items 12 and 13. Every code claim (E1–E7) reproduces against the release binary. No wrong `exact` answer and no S1/S2 defect was found.

## The 17 pass-1 items

| # | Sev | Disposition | Evidence |
|---|---|---|---|
| 1 | S2 | RESOLVED | `with_case` (stitch.rs:679) per candidate, unit `case_folded_is_decided_per_candidate`. Repro (Express `/api/Users` + `/api/users`, web `fetch("/api/users")` and `fetch("/api/Users")`): each client's exact-case candidate has no `case`, and its folded candidate has `case: folded`, in both directions. The single-candidate fold (`/api/orders` → `/api/Orders`) still carries `case: folded`. The exact-case client becoming `ambiguous` is now recorded as a decision (contract-m7j E1 trade-off). |
| 2 | S2 | RESOLVED | Python listener that accepts and stays silent on :55998: `rb doctor` printed `fail database … answered nothing within 10 s` with a fix, skipped postgres/age/migrations/mount, ran the rest, and exited 1 in 10.32 s real. e2e `doctor_is_bounded_against_a_silent_database` passes (12.0 s). |
| 3 | S2 | RESOLVED | `SELECT name, checkout FROM rb.repo`: 4 repos, all NULL; `tmpzc8i9d` is gone. The lesson is in tasks/lessons.md, and the incident is in the M7h Amendment 2. |
| 4 | S3 | RESOLVED | `callers 'b()' --pick 1 --depth 3 --limit 1`: `summary: 6 callers found, 1 shown` and `certainty: heuristic 6`; JSON `{'heuristic': 6}`. `impact --limit 1`: `(4, 1 shown)`, `(2, 1 shown)`, then `certainty: heuristic 6` (text and JSON), the same as without `--limit`. |
| 5 | S3 | RESOLVED | `key_spans` is a single pass. Fresh 20,000-path / 8.3 MB spec (`bigspec.py`, with package.json): **10.15 s real, 0.26 s user** (M7i log 45.39 s / 34.43 s). `e5_m7i.dump` and `e5_m7j.dump` are byte-identical. Edge cases checked through `openapi_json` in a scratch crate: minified, CRLF, multibyte keys (byte columns, the same as tree-sitter), `\"` and a trailing `\\` in keys, a quoted key inside a value string, an unterminated document (None) and a BOM (None) are all correct. Escaped spellings (`\/`, `é`) give `[0,0,0,0]`, as documented. |
| 6 | S3 | RESOLVED | `p3-workspace.log`: 23 `test result` lines, which sum to **1,047 passed / 0 failed / 0 ignored**, with 0 `skipping:` lines. verify.sh runs it with real Docker (no shim on PATH). |
| 7 | S3 | RESOLVED | `rb path 'GET /api/users'`: `[ambiguous] (case folded, 2 candidates)` on the folded client only. `GET /api/Orders`: `[heuristic] (case folded)`. Unit `stitch_note_names_a_folded_case`. |
| 8 | S3 | RESOLVED | README:89 and usage guide: "20 a page (`--limit`/`--offset`)". |
| 9 | S3 | RESOLVED | README and the usage guide describe the `callers` summary and the `impact` headers separately. |
| 10 | S3 | RESOLVED | The postgres and migrations `fail` rows, the mount `warn` row and the disk `warn` row now have fixes. Unit `every_warn_and_fail_row_has_a_fix`. `skip` is documented (README:242, usage:177). Wedged Docker stays `warn`, amended in M7h Amendment 2. |
| 11 | S3 | RESOLVED | Register: 32 findings; S1 3 / S2 10 / S3 17 / S4 2, and each ID list matches its rows' severities. 25 ✅ + 7 ◐ (F9, F11, F23, F24, F25, F28, F30) matches the rows. The "uncommitted" M7a line is fixed. |
| 12 | S3 | RESOLVED | `callers 'c2()' --pick 2` / `--pick 3` → `--pick is N, but only 1 candidate matches "c2()"`, exit 2, JSON `invalid_param`. `--pick 1` answers. `path … --pick 2` is refused the same way. See N3 for wording. |
| 13 | S3 | ACCEPTED-AS-RESIDUE | U1 is marked ◐ "by path; root-commit matching not built, so a moved clone gets a directive", in the register and in the M7h Amendment 2. |
| 14 | S3 | **NOT RESOLVED (U2)** | Every U/O row and every Observation now has a status, and the U/O summary counts match. But pass 1 named U2 as open, and the register marks **U2 ✅ M7f**. Its row asks for `file.java:N (basename)`, `./path:N` and trimmed input. Repro: `callers 'server.js:5'` → `nothing matches`; `callers './api/src/server.js:5'` → `nothing matches`. Trimmed input (`' c2() '`) works. The same register's Observation row says "residue: other path spellings", and contract-m7f's residue does not list this. **Blocking:** an S3 the records claim as fixed. Fix: mark U2 ◐ M7f with that residue. |
| 15 | S4 | RESOLVED | The register has "Found at the M7 close: … fresh … 1,946 MiB … up from 1.55 GiB at M5 … residue". |
| 16 | S4 | RESOLVED | README lock wording ("usually finds … up to date; with `--force` or another `--rev` it indexes again"). `caseSensitive` / `case sensitive routing` "any value counts". todo.md names `repo_checkouts`. |
| 17 | S4 | ACCEPTED-AS-RESIDUE | M7i Amendment 2: Cypher's English-message classification recorded as residue; the S2 wording widened to "any entry mark". |

The sub-audit items are also handled. F23 and F28 are ◐ (the BCL/framework-type residue is in contract-m7d lines 56-57, not in the F28 row). F9/O1 and F11/O5 are ◐. The F25 row now names `blockingStub`. F1's ✅ notes that the CI job is not built. The Observations are classified (RB_TEST_CAPS declined, `diff --repo` residue).

## Verification claims spot-checked

- p3-workspace.log 1,047/0/0 and 0 skipping: confirmed (item 6).
- invariant.txt 31/31 `identical`: confirmed. `rb dump` does not include `tool_versions`, so N1 does not affect it.
- E5 table: matches `e5b_m7{h,i,j}.err` (10.97 / 45.39 / 10.80 s real) and my re-run (10.15 s).
- E2 "about 12 s": my run took 10.3 s (the e2e test 12.0 s).
- Targeted tests pass:
  - rb-indexer `case` 5/5, rb-frameworks `spec` 8/8, rb-store `limit` 2/2.
  - rb-cli bin `doctor|pick|stitch_note|limit` 9/9.
  - e2e `doctor` 2/2 and `a_checkout_is_its_repository_whatever_its_name` 1/1.
- The doc test counts (355 rb-indexer lib tests, 52 e2e tests) match the filtered/total counts.

## New findings

**N1 (S4): the binary of record is not the committed tree, and it labels snapshots with the previous pipeline.**
- Evidence:
  - `var/m7j/bin/rb` (19:47:14) was built before `crates/rb-indexer/src/lib.rs` was edited (20:02:03, 14 s before the commit). A snapshot it indexed records `"pipeline": "m7i-1"`.
  - A debug build of a60cc90 records `"m7j-1"`.
  - Contract-m7j says `PIPELINE m7j-1` makes M7i snapshots rebuild once. That holds for the source, but the binary of record would treat a snapshot from a real M7i build as up to date and keep its copied `case: folded`.
  - The same pattern exists one milestone back: `var/m7i/bin/rb` records `"m7g-1"`.
  - Every logged verification (p3, invariant, e5b) ran before the bump. It is a one-line constant with no golden, so no result changes.
- Fix: rebuild `var/m7j/bin/rb` from a60cc90 and re-record its sha. Make verify.sh refuse a dirty `crates/` and print `git rev-parse HEAD` next to the sha.

**N2 (S4): when the database is slow rather than silent, `rb doctor` misreports it and can leave a probe file.**
- Evidence (code reading, doctor.rs:79-87, :225-234): the 10 s bound covers connect, versions, migrations and the mount probe together.
  - A server that connects and then stalls (a lock on the migrations table, or a slow `pg_stat_file`) is reported as "answered nothing within 10 s", and its already-`ok` rows are dropped.
  - If the stall is inside `mount_probe`, the process exits with the worker blocked, so `rb-doctor-<pid>.probe` stays in the age-load dir.
- Fix: send rows over the channel as they are produced and keep the ones received; word the timeout as "did not finish within 10 s"; or remove the probe file from the main thread on timeout.

**N3 (S4): E6's wording does not match the behaviour.**
- Contract-m7j E6 says "`--pick N` with N above the number of candidates is `invalid_param`". That holds for one candidate only: `callers 'b()' --pick 3` with 2 candidates gives `ambiguous` (exit 4) with the list.
- The behaviour is defensible and render.rs:70 says so, but the claim and the doc comment at `rb-api/src/lib.rs:321` ("a pick beyond the candidates lists them again") now disagree with each other and with the one-candidate case.
- Fix: reword E6, and the comment.

**N4 (S4, pre-existing, not a regression): JSON spec spans take the first `"key":` textually after the first `"paths":`, not the structural one.**
- A path key that also appears nested in an earlier operation (`"x-ex": {"/a": 1}`) gets the nested span.
- A `"paths"` key nested under `info` before the top-level one shifts every span.
- M7i's `json_key_at` behaved the same, and the dumps are identical. Record it as residue, or track object depth in `key_spans`, which costs nothing extra in a single pass.

## Regression review of a60cc90 (no defect beyond N1–N3)

- `with_case`: `match`, `base` and `unresolved` are still copied from `best[0]`. That is correct, because `suffix` is one argument to `EndpointIndex::candidates`, so it is the same for every candidate.
- Histogram: `certainties_found` is set only when a list was cut. When only endpoints or tests were cut, the fallback equals the found set. The callers summary is built after `--exclude-tests`, which matches the `excluded:` line.
- `pick_beyond_one`: `Some(0)` is refused at parse (`--pick must be between 1 and …`). Both `pick_target` and `pick_endpoint` are covered, and the HTTP API goes through the same functions.
- Doctor: `rows.pop()` always finds the mount row (both arms end with it). The leaked worker dies at process exit (the only caller is `run_cli`).
- `key_spans`: string scanning stays in sync for every input `serde_json` accepts, slices stay on char boundaries, and `tick` is a test counter, not a budget, so no new refusals.

Not a finding: the register's "Environment and docs issues" items and the concurrency note (an empty `var/age-load/rb_<repo>_<id>` left per killed run, "S4, cosmetic") carry no status. They are not F/U/O rows.
