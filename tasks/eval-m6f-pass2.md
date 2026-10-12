# M6f evaluator pass 2 (verification-only, 2026-10-12): VERDICT PASS

Against the fixed tree (pass-1 fixes committed as `15dda9d`). Pass 1
(`tasks/eval-m6f-pass1.md`) found one logic defect (JSON-in-YAML dispatch layer) plus the
polyglot churn triaged after it (URL-claimed chains, framework-client denylist); this pass
re-runs every probe plus the full battery below.

## Re-runs

- Probes P1–P12: all pass (new probe tests green, including the smuggling and claimed-chain
  pins; P8's moved dispatch covered by the updated S1 tests).
- Mutants re-killed from the final tree: S1 verb/unread rules; S2 stream/package; S3 stage-drop
  + unhandled-drop; S4b match-skip + impl-certainty; S5 fold/dynamic; S6 branch/own/certainty/fold;
  S7 guard classes (35/12/4/12-line golden diffs); pass-1 claimed-skip + denylist.
- Invariant 1: fixtures green unblessed; jcommander `--no-scip` byte-identical to `de4f5c1`;
  grpc-go + OAI diffs classified line by line (zero removals); `PIPELINE` bumped exactly once
  (`m6c-5` → `m6f-1`, see below).
- Determinism: forced rebuilds byte-identical (jcommander both modes, ofetch + spec-scratch
  `--no-scip`); two-name fixture e2e throughout; fresh == cached on every public measurement.

## Suite (`var/m6f/logs/p9-close2-*`)

fmt ok; clippy `-D warnings` ok; 842 units outside e2e (+ 25 cli-bin); store 66/66; server 12/12;
e2e 37/37 (36 + `scip_contract_packs`); api 1/1 over 150 sets (50 identical per fixture: express-prisma, spring-jpa, polyglot);
no `skipping:`; no listener; stub scan only the known `rb-sql/src/lib.rs:1111` match.
DB holds only the 3 pre-existing `fx_*` repos.

## PIPELINE bump

`m6c-5` → `m6f-1`: the contract-packs fixture is the first dump change of M6f (new Endpoint
nodes/kinds of edges on new files); all pre-existing fixture dumps byte-identical to `376dbcb`
after the churn fix (verified by the unblessed green run — no re-bless needed).

## Closing bar check

(a) The one pass-1 fix holds (probe suite green on the final tree). (b) No regressions. (c) No
false endpoint/handler/edge above its stated certainty: 20/20 exact jcommander samples,
grpc-go CONSUMES all `synthetic` with sampled correctness, ofetch/spec samples read in source,
0 false `exact`. (d) Deterministic. (e) All tests/guards green. (f) No `exact` CONSUMES anywhere
(pinned by fixture anti-guard); no new egress (M6f adds no fetches).
No new residue-class form was met. **M6f CLOSED.**
