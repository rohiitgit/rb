# M6f evaluator pass 1 (full review, 2026-10-12): VERDICT PASS with one fix

Reviewed HEAD `f08c722` + S4b (`S4b` commit) against the contract's soundness claims and closing bar.
Method: one probe per claim surface, each as a runnable unit test or a recorded measurement.

## Probes and outcomes

| # | Surface (closing-bar battery) | Probe | Outcome |
|---|---|---|---|
| P1 | YAML indentation smuggling | `spec_surfaces_resist_smuggling`: nested `paths:` yields [] | PASS |
| P2 | `$ref` games | Existing S1 rows (`/linked` `$ref`, method-level `$ref` → op without operationId) | PASS, no change |
| P3 | Proto service doubling | Same test: doubled `service A` keeps both readings (merge by key downstream) | PASS |
| P4 | Handler over-joining | By construction (keys carry service; normaliser equality); S3 shared-key + S4b tests | PASS |
| P5 | Mapper placeholder confusion | Existing S5 rows (`#{}`/`${}` folded); namespace-less mapper row added | PASS |
| P6 | Stub-name collisions | S6 ambiguous test (operations); gRPC shares the code path | PASS |
| P7 | Version confusion | By construction (`info.version` never read; packages ignore specs); fixture dump shows no spec-derived package | PASS |
| P8 | JSON content in `.yaml` | New probe **FOUND** the dispatch in the wrong layer (stage sniffed content; reader did not) | **FIXED below** |
| P9 | GRPC key collisions | By construction (keys carry `package.Service`) | PASS |
| P10 | Unhandled-spec noise | `spec_operation_unhandled` only when no registration has a handler | PASS |
| P11 | Mapper without namespace | New row: still reads, empty namespace | PASS |
| P12 | Redirect smuggling (phase-11 style) | No new hosts in M6f (no fetches added); N/A | — |

## The one fix (P8)

`openapi_yaml` on flow-JSON content returned `Some([])` (silent miss: a `.yaml` file holding JSON
parsed as YAML shapes and found no paths). The content sniff lived in the stage (`routes.rs`),
not the reader. Fixed test-first: the sniff moved into `openapi_yaml` (flow-JSON reads as JSON);
the stage dispatches by extension only. The probe test now pins all three legs (nested-`paths`
ignored, flow-JSON read, doubled proto kept). No other behavior changed (S3 routes tests green
unmodified).

## Non-blocking notes

- N1: Python snake_case `*Api` locals stay out of operationId candidacy (case-sensitive
  class-style rule); recorded residue.
- N2: `<<<<<<<`/`=======` without `>>>>>>>` do not reject (a real conflict always carries
  `>>>>>>>`); N3 calibration stands.
- N3: Extension-less tsconfig-style file patterns and TS extension inference stay fail-open
  deviations, pinned.
- N4: `PIPELINE` unbumped so far (`m6c-5`); every fixture dump and measurement diff classified.
  The contract-packs fixture (phase 7) is the first dump change → `m6f-1` expected at its bless.

## Suite at review time

fmt/clippy clean; 306 indexer + 164 frameworks + 25 cli-bin units; fixture e2e green unblessed;
measurements deterministic and classified. Full closing loop runs for pass 2.
