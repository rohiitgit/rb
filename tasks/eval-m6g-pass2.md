# M6g evaluator pass 2 (verification-only)

Run 2026-10-12 on the pass-1 tree. Scope per the closing bar: re-run every pass-1 repro and
control, re-kill every pass-1 mutant from a clean tree, re-derive invariant 1 and determinism,
and confirm the close verify loop is green. No new probing (that was pass 1).

## Repro checks (each must fail pre-fix, pass now)

- P9: `proxy_pass http://auth:8080/health` unread; `uri: lb://svc/prefix` unread (nginx + spring units).
- P18: pattern/partitions/bindings listeners + `sendDefault` all dynamic (topics unit).
- P30: 3-arg Rabbit sends dynamic; 2-arg read (rabbit unit).
- P31: contested `/api/` dropped; identical `/pay/` deduped (stitch unit).
- P32: SpEL/partial-template literals dynamic (topics unit); spring-kafka re-measurement: 0
  suspicious topic names.
- P2: longest-prefix order (stitch unit).

## Mutant re-kills (from the pass-1 tree, reverted after)

M1–M10 as listed in `tasks/eval-m6g-pass1.md`: each fails its unit. (Result recorded at close.)

## Suite + invariant 1 + determinism

Close verify `var/m6g/logs/p6-close2-*`: fmt, clippy, workspace units, store 66, server 12,
e2e with containers, api. Invariant-1 spot check re-run on the final tree. topo e2e unblessed.
(Result recorded at close.)

## Verdict

PENDING (recorded at close).
