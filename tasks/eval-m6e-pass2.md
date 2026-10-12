# M6e evaluator pass 2 (verification-only, 2026-10-12): VERDICT PASS

Against HEAD `0ae0a79` + probe tests (`nearest_go_work_governs_each_service`,
`duplicate_classifiers_and_escaping_workspace_uses`, `tsconfig_patterns_escaping_their_directory_stay_unread`,
template-literal N3 pin, `${enc}` compilerArgs row). Pass 1 (`tasks/eval-m6e-pass1.md`) needed no
fixes, so this pass re-runs every probe plus the full battery below.

## Re-runs

- Probes P1–P12: all pass (5 new probe tests green; E1/E3/E13/E14 verified by reading; E9 proven by the phase-11 live refusal).
- Mutants re-killed from the final tree: S8 workspace-win + exclude; S9 classifier-key + GA-retain;
  S10 subtree + fail-open + extends-link + join partition; S11 config/cli + nearest + arg-slot + per-doc
  wiring; S12 marker check; phase-11 allow-list pins move by design (recorded, not a kill).
- Invariant 1: fixtures green unblessed at every phase verify (8/9/10/close); jcommander `--no-scip`
  byte-identical to `de4f5c1`; ofetch diff classified line by line; `PIPELINE` unbumped (`m6c-5`).
- Determinism: forced rebuilds byte-identical (jcommander both modes, ofetch `--no-scip`); two-name
  fixture e2e throughout; fresh == cached on every public measurement.

## Suite (`var/m6e/logs/p12-close-*`)

fmt ok; clippy `-D warnings` ok; 822 units outside e2e (+ 25 cli-bin); store 66/66; server 12/12;
e2e 36/36; api 1/1 over 150 sets (50 identical per fixture); no `skipping:`; no listener; stub scan
only the known `rb-sql/src/lib.rs:1111` match. DB holds only the 3 pre-existing `fx_*` repos.

## Closing bar check (§41–46)

(a) No pass-1 fix existed to re-check — every probe holds. (b) No regressions (e2e unblessed
throughout). (c) No false edge/version/path: 20/20 exact jcommander samples read in source, ofetch
pins sampled in the lockfile, 0 false `exact`. (d) Deterministic. (e) All tests/guards green.
(f) Egress: one live refusal diagnosed, probed, allow-listed; distribution host still refused.
No new residue-class form was met. **M6e CLOSED.**
