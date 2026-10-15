VERDICT: FAIL (1 blocking; 4 should-fix, 3 notes)

# M8b close: evaluator pass 1 (2026-10-14)

Scope: contract-m8b, B7–B13, `bench/`, `crates/rb-bench/src`, the scoreboards, usage §5.5, README, CHANGELOG. Tools: `cargo test -p rb-bench`, `target/debug/rb-bench`, and `python3 -I` over the dumps, keys and models.

**What reproduces:**
- 76 tests pass. A re-score is `cmp`-identical to `m8b.json` (`d0621cc9…`) and `m8b-perf.json`. All 260 M8a cells are identical, and `compare m8a m8b` exits 2.
- An independent Python draw (pools, largest remainder, M3b indices) gives the same 249 edges as `rb-bench sample`, which is identical to `var/bench/labels/exact/drawn.txt`. Tallies from `exact.tsv`: 60/9/58/57/59. All five Wilson intervals match.
- hk_polls `table_queries`: tp 18, fp 4 (interface lines), fn 2 (`data.sql`, V2). microSecEnD endpoints: fernandoabcampos 16/7/0/9, jferrater 5/0/0/5. Every internal service of all 12 apps maps to the right rb service (36/36). Certainty `exact`: 39+26+10+53 = 128 tp, 2 fp.
- Correct against the source: 14 key lines (PollRepository :16–24, UserRepository :12–22, todo_item_postgres :18–76, OrderRepository :11–33, the hk_todo handlers) and 7 verdicts (both go_gin FALSE, the Elasticsearch suite FALSE, a webhook route, 000030.down, threadmemberships, nestjs `@Entity('article')`). The 28 mutant specs are live.

## Findings

1. **Blocking: "`exact` is right on 128 of 130" counts 6 wrong-path endpoints as right.**
   - piggymetrics sets `server.servlet.context-path` in the config server's shared files (`config/src/main/resources/shared/account-service.yml:21` `/accounts`, plus `/statistics` and `/notifications`).
   - rb exposes `GET /{}`, `POST /`, `GET /current` and `PUT /current` (account), `GET /current` and `GET /{}` (statistics), and `/recipients/current` (notification), all `exact` and none with the prefix.
   - The model lists the unprefixed forms, so all 6 tp are wrong under the contract's identity, "the full effective path" (contract:30). Meanwhile the true `/accounts/{accountName}` and `/statistics/{accountName}` count as fn.
   - So the certainty table, Result:166 and the CHANGELOG overstate `exact`: about 122/130, not 128/130. No path exists to correct a microSecEnD key.
   - Fix:
     - Add an append-only override file per microSecEnD app, as for the hand keys, and score these 6 as fp.
     - Add finding B14 (context path from Spring Cloud Config shared files).
     - Re-render, then correct Result, README and CHANGELOG.

2. **Should-fix: microSecEnD recall is mostly key error, and real misses go unrecorded.**
   - At least 15 of the 28 fn are model artifacts:
     - class prefixes listed as endpoints: `/api` ×3, `/catalog`, `/movies`, `/ratingsdata`, `/recipients` and `/comments` ×2;
     - six Feign-client paths credited to `card-statement-composite`. They are the client interfaces `CardService.java:18-24` and `StatementService.java:17-23`, so the composite does not serve them.
   - The real misses B12 omits:
     - a `value = "{taskId}"` with no leading slash under class `/` (anilallewar `TaskController.java:48`, `UserController.java:51`; the rohitghatol fork too);
     - the context path in finding 1.
   - Fix: override entries for the artifacts, re-score, and rewrite B12 and the Result's "65.4 %" line.

3. **Should-fix: the certainty table counts `table_ops` rows as edges.**
   - `render_certainty` (render.rs:101-110) sums `table_queries` and `table_ops`, which count the same `QUERIES` edges again, once per op.
   - "synthetic 32 of 72 edges" is really 16 of 24 edges (66.7 %). B11's "all 40 wrong synthetic edges" is 8 edges. `heuristic` 125/126 includes 8 duplicates (117/118).
   - Fix: exclude `table_ops` as `handkey_raw` is excluded, add a unit (and a mutant), re-render, and correct Result:153/167, B11 and the CHANGELOG.

4. **Should-fix: the adjudication was not fully blind, and its inputs are stale.**
   - `?` appears only where rb has no handler, because every key ENDPOINT line has one (handkey.rs:253). That reveals the side in hk_todo S19 and S45.
   - `rb-bench statements` today differs from the recorded hk_todo file. The `*any` normalisation (structural.rs, 03:09) postdates the statements (02:57).
   - Recorded S19 (`GET /swagger/{} ?`) was then rb-only and was judged TRUE. That contradicts Result:184 ("every rb-only one FALSE").
   - Fix:
     - Never print `?`: complete the handler from either side, or drop handler-less ENDPOINT statements in favour of the `handler_binding` ones.
     - Regenerate the statements and re-judge any new ones.
     - Correct Result:184.

5. **Should-fix: the audit claims can't be checked.**
   - Result:181/183/188 say the labeller, adjudicator and judge transcripts were audited. `var/bench/labels/*/` holds only `labeller.jsonl`: there is no adjudicator transcript and no exact-judge transcript, nor the judge prompt ("told the edge is false").
   - Fix: keep those transcripts under `var/bench/labels/<target>/` and `exact/`, and record the audit command and its output in the Result.

6. **Note: the `exact` families are mostly one repository, and mostly test harnesses.**
   - Of the 60 `routes` edges, 32 are mattermost e2e webhook servers (`e2e-tests/cypress` 16, playwright `webhook` 16), judged TRUE, while B7 counts test-code routes as wrong.
   - `scip` is 54/60 mattermost and `sql` 58/60.
   - Fix: state each family's composition beside its bounds, and make one test-code rule for both the sample and B7.

7. **Note: D3's `dev set` label is lost.**
   - spring_boot_microservices is pooled into every hand-key total (render and Result:170-173).
   - Fix: report it apart, or mark it in the Result.

8. **Note: smaller inaccuracies.**
   - B9 says "every route uses a method value": `/swagger/*any` is a library handler (`handler.go:24`), so 12 of 13 are method values.
   - The CHANGELOG says "every certainty label is now measured", but `dispatch` is not stored.
   - `Services::of_file` falls back to the root service (structural.rs:105), so T2's `unmatchable{no_evidence_file}` can't fire when a root service exists.
   - D1's check as amended (pin_microsecend.py:97, end column ≤ line length) barely detects drift, so the 90 % gate is weak evidence. Say so.
   - Fix: wording in B9, the CHANGELOG and the contract.
