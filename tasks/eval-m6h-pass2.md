# M6h evaluator pass 2 (verification-only)

Run 2026-10-12 on the pass-1 tree. Scope per the closing bar: re-run every pass-1 repro and
control, re-kill a sample of pass-1 mutants from a clean tree, re-derive invariant 1 and
determinism, and confirm the final verify loop is green. No new probing (that was pass 1).

## Repro checks

- H1: all 3 rule queries execute live with no AGE error (check_gates e2e + public legs).
- H2: duplicate-writer relational unit + DISTINCT assertion green.
- H3: truncation renderer units green (both formats + empty).
- Twins: check_gates parity on 4 fixtures green unblessed.

## Mutant re-kills

R-DISTINCT, R-truncation, R-orphan: 3/3 killed from the clean tree, each reverted with the
tree re-verified. (The first R-truncation attempt used `--lib` on a binary-only package and
proved nothing; re-run with `--bin rb` killed.)

## Suite + invariant 1 + determinism

Final verify `var/m6h/logs/p6-final-*`: fmt ok, clippy ok, workspace units 874/874, store
72/72, server 12/12, e2e 39/39 with containers, api 1/1 (150 sets identical), no `skipping:`,
no listener, stub scan clean (only the known `rb-sql` match). Twin parity on 4 fixtures +
check goldens green unblessed on the final tree.

## Verdict

PASS. No blockers: every pass-1 repro holds, mutants re-killed, suite green, twins agree.
**M6h closed.**
