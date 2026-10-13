# M7j contract: the M7 close fixes (evaluator pass 1) (2026-10-13)

**Status:** contract written 2026-10-13; implemented 2026-10-13 (amendment 1). Branch `m7j-close`, stacked on M7i (`77675c8`). Input: `tasks/eval-m7-pass1.md` (VERDICT FAIL, 17 findings: 0 S1, 3 S2, 9 S3, 5 S4). After this batch, a second evaluator pass decides whether M7 closes.

## Why

The evaluator found no wrong `exact` answer. Three answers mislead, and the records overstate what was fixed. Each finding below is mapped to a claim or a records fix.

## Design

**Trade-offs:**
- **E1, `case: folded` per candidate:** attributes are built once from the best match and cloned onto every ambiguous candidate. **Chosen:** the shared attributes stay shared, and `case` is decided per candidate from its own match. The alternative, preferring an exact-case candidate over folded ones, would pick one silently. Express serves whichever route was registered first, which the graph does not know, so `ambiguous` is the honest answer.
- **E2, bounding `rb doctor`:** `Store::connect` has no timeout. A connect timeout on the whole store would change every command's failure mode. **Chosen:** `doctor` runs its database probes on a worker thread and waits at most 10 s (the Docker probe's bound). A silent server becomes `fail` with a fix, and `doctor` moves on.
- **E5, linear JSON spans:** one forward pass from `paths` records the first offset of every `"key":`, with line and column counted as it goes. That is O(document) instead of O(paths × document).

**Numbers:**
- E5: the evaluator's 7.5 MB / 20,000-path spec went from 10.3 s (M7h) to 45 s (M7i). **NFR:** within 10% of M7h.
- E2: **NFR:** `rb doctor` against a silent listener finishes within 15 s.

## Claims

| # | Eval | Claim | Test |
|---|---|---|---|
| E1 | 1 | `case: folded` is on exactly the candidates whose own match folded. | stitch unit with `/api/Users` + `/api/users` |
| E2 | 2, 10 | `rb doctor` is bounded against a database that accepts and never answers. Every `fail` row has a fix. `skip` is documented. A wedged Docker stays `warn`: amendment to M7h S5, since `--no-scip` indexes without it. | doctor unit (row fixes); e2e against a silent listener |
| E3 | 3 | Data: the shared DB's checkouts repaired; the mutant's leftover repository removed. Lesson: mutants of store writes run against a scratch database. | done at the start of M7j |
| E4 | 4 | With `--limit`, the certainty histogram (text and JSON) counts every row found, not only the rows shown. | store/CLI units |
| E5 | 5 | JSON spec spans in one forward pass, charged to `tick`. | spec unit (same spans); timing on the evaluator's spec shape |
| E6 | 12 | `--pick N` with N above the number of candidates is `invalid_param`, also when there is one candidate. | api unit |
| E7 | 7 | `rb path` text shows `case folded` on a folded hop. | path unit |
| E8 | 6, 8, 9, 11, 13–17 | Records and docs:<br>• A full real-Docker run with its log.<br>• `find --kind` pages at 20.<br>• The summary is for `callers` only.<br>• Register counts and statuses: every U/O row, the Observations classified, U1/F9/F11/F23/F28 marked ◐ with residue, the fresh-RSS drift recorded.<br>• The README's lock and `caseSensitive` wording.<br>• Todo function names.<br>• M7i S2 amended: the header names any entry mark.<br>• Cypher classification on English server messages recorded as residue. | evaluator pass 2 |

## Verification

- Tests first, with mutants.
- Real-Docker suites with the full log kept: 0 failed, 0 skipped.
- Invariant against M7i: graphs byte-identical except `CONSUMES` attributes where E1 applies (none expected in the 31 targets).
- Evaluator pass 2.

## Result (amendment 1, 2026-10-13)

- **E1–E7** are as claimed. Mutants m144–m150 killed. m150 raises the doctor bound instead of removing it, so a surviving mutant cannot hang the run.
- **Additions while closing E2:**
  - The `age-load mount` "not verified" row and the "disk unknown" row had no fix; both have one now.
  - A unit test asserts that every `warn`/`fail` row has a fix and every `skip` row names what it needs.
- **E8, records:**
  - Register: counts (32 findings, S3 17); every U/O row has a status; the Observations are classified; F9, F11, F23 and F28 marked ◐ with their residue; U1 and U10 ◐; U4 done (M7j E6); the fresh-RSS drift recorded.
  - Docs: README and usage guide corrected.
  - Contract amendments: M7h (S5 statuses, U1 path-only, the m103 incident) and M7i (S2 wording, Cypher English-message residue).
  - Todo function names fixed; lesson on database-write mutants.
- **Verification:**
  - fmt and clippy clean.
  - Real-Docker suites, full log kept (`var/m7j/logs/p3-workspace.log`): **1,047 passed, 0 failed, 0 skipped**.
  - Invariant against M7i (`var/m7j/logs/invariant.txt`): **31/31 byte-identical**, as predicted (no target has two folded candidates).
  - **E5** on a spec shaped like the evaluator's (20,000 paths, 8.3 MB, with a `package.json` so the spec belongs to a project; the first try without one was skipped as `spec_outside_project`). Logs: `var/m7j/logs/e5b_*`. The M7i and M7j dumps are byte-identical.

    | Binary | Real | User |
    |---|---|---|
    | M7h | 10.97 s | 0.19 s |
    | M7i | 45.39 s | 34.43 s |
    | M7j | 10.80 s | 0.26 s |

  - **E2:** `rb doctor` against a silent listener exits 1 in about 12 s with `fail  database … answered nothing within 10 s` (e2e). On the machine of record, 10/10 rows ok.
- `PIPELINE` `m7j-1` (E1 can change `CONSUMES` attributes), so snapshots indexed by M7i rebuild once.
- **Database:** after the runs, the shared database holds only the 4 user repositories, with no checkout recorded.
