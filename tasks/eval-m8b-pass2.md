VERDICT: PASS (0 blocking; 1 should-fix, 4 notes)

# M8b close: evaluator pass 2 (2026-10-16, adversarial re-check of amendment 2)

Scope: `tasks/contract-m8b.md` amendment 2 + corrected Result; scoreboard
`bench/scoreboard/m8b.json` (sha256 `d7346ffb16cd799bfd73df5ffc928ecb0332da61183efce33b8169ab3a6c275f`);
dumps `var/bench/out/a038653b8478`; repos `var/bench/repos`; findings
`tasks/bench-findings.md` B7–B14; pass-1 eval `tasks/eval-m8b-pass1.md`.
Method: reproduced everything with tools on commit `b8f9f44` (branch `m8`,
parent `6c059ea`). Read-only except this report and `/tmp/evalm8b` scratch.
A draft of this report already existed as an untracked file; it was ignored
and every check below was run independently.

## 1. Re-score reproduces byte-identically; exactly the 6 claimed cells move

- `rb-bench score var/bench/out/a038653b8478 --milestone m8b --rb-commit
  e08efcb801c28422afab450251ee0ba093cd57f3 --write /tmp/... --perf /tmp/...`
  is `cmp`-identical to committed `bench/scoreboard/m8b.json` and
  `bench/scoreboard/m8b-perf.json`. Committed sha256 re-hashes to
  `d7346ffb…c275f` as claimed.
- Old-vs-new per-cell diff (new = committed; old = re-score with
  `bench/keys/microsecend/*.md` absent, via `--keys` copy in /tmp, no source
  touched): **exactly 6 of 314 cells change, all
  `microsecend/noscip/endpoints/accuracy`**:
  - `mse_fernandoabcampos`: gold 16→7, fn 9→0 (old row is pass 1's `16/7/0/9`);
  - `mse_koushikkothagal`: gold 7→4, fn 3→0;
  - `mse_mdeket`: gold 14→12, fn 2→0;
  - `mse_anilallewar`: gold 7→6, fn 3→2;
  - `mse_rohitghatol`: gold 7→6, fn 3→2;
  - `mse_piggymetrics`: gold 9→6, tp `{exact:6}`→`{}`, fp `{}`→`{exact:6}`,
    fn 3→6.
- Claimed deltas hold exactly (fn→0 ×3, fn 3→2 ×2, piggymetrics tp 6→0 /
  fp 0→6 / fn 3→6). Microsecend endpoint totals recomputed from cells:
  gold 6+8+7+5+4+12+2+6+0+6+4+2=62, tp 47, fp 6, fn 15 — the Result's
  62/47/6/15.

## 2. Every override entry verified against pinned source; verdicts support them

All six checkouts are at their manifest pins (`git rev-parse` equals the
`m8b.json` pin in each case). Each REMOVE/ADD was checked with
`git show <pin>:<path>` (via `${P}:<path}` quoting):

- **piggymetrics** (`bench/keys/microsecend/mse_piggymetrics.md:44-56`), pin
  `6bb2cf9d`: `context-path: /accounts | /statistics | /notifications`
  confirmed in `config/src/main/resources/shared/{account-service,
  statistics-service,notification-service}.yml`; `AccountController.java` has
  no class mapping with `/{name}` (`:20`), `/current` (`:25,:30`), `/`
  (`:35`); `StatisticsController.java` no class mapping with `/current`
  (`:20`), `/{accountName}` (`:26,:32`); `RecipientController.java:15-16`
  class `/recipients` with only `/current` handlers (`:21,:26`). All 7
  REMOVEs and 6 ADDs correct. The S1/S2, S4/S5 split
  (`var/bench/labels/mse_piggymetrics/verdicts.md:1-5`) is handled honestly:
  FALSE readings dispute the unprefixed path (missing context path); the TRUE
  readings confirm the normalised prefixed endpoint is served (account code
  serves `{name}`, statistics serves `{accountName}` — the contract's `{}`
  normalisation (`contract-m8b.md:32`) erases the name difference). Both stay
  in gold via the disclosed no-op ADDs (`mse_piggymetrics.md:52,55`), not
  hidden.
- **fernando** (`mse_fernandoabcampos.md:21-29`), pin `8668c4df`: 3 bare
  `/api` class prefixes confirmed (`CardServiceController.java:18`,
  `StatementServiceController.java:19`,
  `CardStatementServiceController.java:20`); 6 Feign attributions confirmed —
  `service/CardService.java:18,21,24` and `service/StatementService.java:17,
  20,23` are interfaces extended by `@FeignClient(name="card-service")` /
  `(name="statement-service")` (`CardClient.java:5-6`), served by
  card-/statement-service, not the composite. Verdicts
  (`var/bench/labels/mse_fernandoabcampos/verdicts.md`) S3/S6/S10 (prefixes
  FALSE) and S7/S11/S13/S14/S15/S16 (Feign FALSE) support every REMOVE.
- **koushik** (`mse_koushikkothagal.md:16-18`), pin `8b01c6d1`:
  `CatalogResource.java:20` class `/catalog` + sole `/{userId}` (`:29`),
  `MovieResource.java:13` + sole `/{movieId}`, `RatingsResource.java:10` +
  only `/movies/{movieId}` (`:13`), `/user/{userId}` (`:18`).
- **mdeket** (`mse_mdeket.md:17-18`), pin `5aa5ee9e`:
  `RecommendationController.java:23` class `/recommendation`, all handlers
  on sub-paths (`:33`–`:72`); `/newuser` only under
  `@FeignClient("user-service")` while `UserController.java` serves only
  `/user` + `/user/{userId}`.
- **anilallewar / rohitghatol** (`mse_anilallewar.md:15`,
  `mse_rohitghatol.md:15`), pins `ac0a2491` / `a3c9df93`: `/comments` bare
  prefix confirmed (`CommentsController.java:30` / `:28`, sole handler
  `/{taskId}`); the no-leading-slash joins are REAL and honestly kept as fn —
  `TaskController.java:48` `value = "{taskId}"` under class `/` (`:22`),
  `UserController.java:51` `value = "{userName}"` under class `/`.

## 3. Second hand-key adjudications verify clean

- `rb-bench statements <out> <target>` output is byte-identical to
  `var/bench/labels/{spring_boot_microservices,hk_polls,hk_todo}/
  statements.txt` (31/37/72 lines; v1 files 30/35/68). Zero bare-`?` tokens
  in any file (v1 `GET /swagger/{} ?` is now S34 with concrete
  `pkg/handler/handler.go:24` registration line).
- Verdict counts match the contract exactly: 11/17/69 TRUE and 20/20/3 FALSE
  (spring/hk_polls/hk_todo). New-statement spot-checks: hk_todo S33/S43/S44
  rb-only FALSE (test-only `/identity` at `middleware_test.go:92`, wrong-path
  `/sign-up` vs `/auth/sign-up`), S34/S47/S63 key-held TRUE; hk_polls
  S15/S34 key-held TRUE; spring S7 key-held TRUE.
- `python3 -I bench/adjudicate.py <target>` prints zero entries for all three
  targets, so `handkey_raw` == `handkey` as scored.

## 4. Corrected numbers match `render` output exactly

- `render` table == contract Result rows (microsecend endpoints 62/47/6/15;
  service_flows 285/14/4/0/10). `render --certainty` == contract certainty
  tables including all five Wilson intervals, recomputed independently:
  94.0 / 70.1 / 88.6–99.1 / 86.3–98.3 / 91.1–99.7.
- `exact` 122/130 = 93.8%; synthetic 16/24 = 66.7% (`table_ops` excluded,
  `render.rs:108-109`, filter line verified); heuristic 117/118 = 99.2%.
- Decomposition verified from cells: exact tp = endpoints 12+13+14=39,
  handler_binding 12+14=26, table_maps 5+5=10, microsecend
  4+8+7+4+12+2+0+4+4+2=47; 39+26+10+47=122; fp = 2 (hk_todo test routes) + 6
  (piggymetrics unprefixed) = 8; total 130.
- B7 ("2 of the 8 wrong exact edges… other 6 are the context paths"), B11
  ("16 of 24 synthetic… 8 edges… excluded"), B14 all match the numbers.
  Sample composition claims check out from `exact.tsv`: scip 54/60
  mattermost, sql 58/60, routes 32/60 mattermost e2e/webhook lines.

## 5. Tests / mutants / lints

- `cargo test -p rb-bench`: **79 passed, 0 failed** (lib; bins/docs 0 tests).
- `python3 -I bench/mutants/run.py bench/mutants/m8b.json`: **all 29
  (m201–m229) KILLED**, incl. rewritten m228 (raw-keys double-count) and new
  m229 (table_ops exclusion). Tree verified clean after the battery (`git
  status` shows only the two untracked files).
- `cargo fmt --all --check` clean; `cargo clippy --workspace --all-targets
  -- -D warnings` clean on the committed tree (run after the battery
  finished; see N4).

## 6. Gates and invariant

- `compare m8b m8b` → **0**; `compare m8a m8b` → **2** (new pins/keys
  listed) as claimed.
- Exact-fp-added copy (hk_polls endpoints tp 12→11, fp 0→1) → **1**, with
  `fp exact 0 → 1 (never acceptable)` among the regressions.
- All 260 M8a cells byte-identical inside `m8b.json`; 54 new cells
  (260+54=314). `git diff e08efcb --name-only -- crates`: 16 files, all under
  `crates/rb-bench/` — invariant holds.

## 7. Docs sweep

- CHANGELOG M8b entry (122/130 with the 6+2 miss split, 16/24, 88.7/75.8,
  m201–m229, 79 tests, 1,122/0/0 — log re-tallied from
  `var/m8b/logs/workspace.log`: 1122 passed / 0 failed / 0 ignored),
  IMPLEMENT M8b section, todo M8b checkboxes (T4's "133 verdicts" = v1 round
  35+68+30; amendment-2 line covers the second round), lessons 2026-10-15
  entry: all consistent.
- README benchmark lines (generic, no figures) and usage guide §5.5 (process
  description; the "get keys in M8b" sentence is historical under M8a-era
  text) free of stale numbers.
- No TODO/FIXME/stub in `crates/rb-bench/src/`, `bench/adjudicate.py`,
  `bench/pin_microsecend.py` (`grep` exit 1).
- Transcript audit: labeller/adjudicator prompts properly confine reads to
  the checkout; judge prompt begins "ASSUME EACH EDGE IS FALSE". All tool
  path/command inputs across all 10 transcript files touch nothing forbidden
  except two `ls` of an agent's own output dir (benign). No `rb`/`rb-bench`
  runs, no reads of `tasks/`, `crates/`, run outputs, keys or samples.

## Findings

- **S1 (should-fix, prose only — no re-score, no cell/table changes):
  `tasks/bench-findings.md` B12 misstates piggymetrics.**
  Scoreboard (`mse_piggymetrics`: tp 0, fp 6, fn 6) and contract Result's fn
  split (15 = 4 new prefixed + 4 no-leading-slash + 5 Spring Data REST + 2
  listed prefixed) are correct, but `bench-findings.md:139-142` says:
  (a) "Real rb misses (9 fn)" — omits the 6 genuinely-served prefixed
  piggymetrics paths rb also misses (true total 15);
  (b) "On genuinely served endpoints rb is … 6/6 (piggymetrics)" — false, rb
  is **0/6** (it emits only the 6 unprefixed fp forms); the other per-app
  figures (7/7, 4/4, 12/12, 4/6, 4/6, 0/5) are correct;
  (c) the line-141 Feign bullet lists "`/accounts/{}` and `/statistics/{}`"
  — they are context-path artifacts (B14), not Feign paths, and they were
  **not** removed (both stay in gold as no-op ADDs,
  `mse_piggymetrics.md:52,55`). Fix: "9 fn" → 15 with the piggymetrics 6
  counted, "6/6 (piggymetrics)" → "0/6", move the two paths out of the Feign
  bullet.
- **N1 (note): mse blind verdicts have no transcripts.** `var/bench/labels/
  mse_*/` hold only `gold.txt`/`statements.txt`/`verdicts.md` (uncommitted);
  there are no `judge-*.jsonl` for the mse round, so its blindness is
  asserted, not auditable — unlike hand-key and exact rounds. Impact: none on
  correctness — every mse entry was re-verified against pinned source (§2).
  Record it so a fresh clone knows the mse round re-verifies from the
  committed override files' file:line evidence.
- **N2 (note): stale doc comment.** `crates/rb-bench/src/handkey.rs:271-273`
  (`statements()` doc) still says lines use "`?` where a side states no
  handler", but no bare `?` is emitted (test
  `statements_cover_every_dispute_and_name_no_side`, `handkey.rs:395`,
  enforces it; existence uses `*`). One-line comment fix.
- **N3 (note): contract amendment-2 F4's recap** (`contract-m8b.md:113`)
  repeats the amendment-1 sentence "Lines use `?`…" while its resolution
  correctly states "no bare `?`". Same one-line class as N2; normative
  Result:239 is accurate.
- **N4 (note, process): never lint while `bench/mutants/run.py` holds the
  tree** (it applies mutants mid-run; a concurrent clippy/fmt would read
  mutated sources). This evaluation sequenced cargo test → mutants →
  clippy/fmt, and re-checked `git status` after the battery.
