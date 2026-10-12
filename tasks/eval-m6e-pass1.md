# M6e evaluator pass 1 (full review, 2026-10-12): VERDICT PASS with notes

Reviewed HEAD `0ae0a79` (phases 8–11) against the contract's soundness claims and closing bar §41–46.
Method: one probe per claim surface, each as a runnable unit test or a recorded measurement.
No blocking finding: no fix was needed, so every probe below is a passing pin, not a repro.

## Probes and outcomes

| # | Surface (closing-bar battery) | Probe | Outcome |
|---|---|---|---|
| P1 | Gradle wrapper smuggling | Read `orchestrate.rs` `BUILD_WRAPPERS` + export exclusion + existing tests | Covered (M3b amendment 25); jcommander measurement indexed with the image's Gradle, wrapper never exported |
| P2 | Catalog confusion | S3 tests + phase-4 fixture (dotted `version.ref` positive rows) | Covered; no new shapes in jcommander (variable-form deps stay out, recorded) |
| P3 | Lockfile confusion | Read npm v2/v3 + pnpm major-9 gates (`packages.rs`) | Deny-default intact; ofetch pins measured |
| P4 | Workspace shadowing | `nearest_go_work_governs_each_service` (new) | PASS: app's own replace beats the root's dangling one |
| P5 | Replace/exclude games | `duplicate_classifiers_and_escaping_workspace_uses` (new): `use ../outside` ignored, gin edge intact, no diagnostics | PASS |
| P6 | Classifier collisions | Same test: identical classified declaration twice stays one exact edge | PASS |
| P7 | Glob escapes | `tsconfig_patterns_escaping_their_directory_stay_unread` (new): `../o`, `/abs` unread, fail-open | PASS |
| P8 | Encoding lies | `${enc}` compilerArgs row added to the S11 test (fail-safe non-UTF-8, like pom `${missing}`) | PASS; five-slot order measured on the real image (D9 corrected: pom beats compilerArgs both ways) |
| P9 | N3 probes | Template-literal line-start `-->` pin added (safe-direction over-rejection, documented); `<<<<<<<`/`=======` without `>>>>>>>` do not reject (a real conflict always carries `>>>>>>>`; recorded residue) | PASS; 0/618 calibration stands |
| P10 | Registry smuggling via redirects | Phase-11 live refusal (`plugins-artifacts.gradle.org:443` named) → probed 200 → allow-listed | Mechanism proven end to end; distribution host still refused |
| P11 | go.work attribution | Read `lib.rs:760-764`: `dependencies` gets full `paths` + `content.manifests` (go.work is an input) | Flows in the real pipeline, not just `run()` |
| P12 | Mixed-encoding unsafe corner | Found by design review during S11 (job-level deepest-wins would trust ISO-root files under a UTF-8 verdict); verdict went per-document with a `run_projects` mixed-build test + wiring mutant | Fixed before any use (no prior binary shipped it) |

## Non-blocking notes (recorded, not blockers)

- N1: `TsScopes::build` runs on every index (even `--no-scip`/non-TS); cost is one scan of `tsconfig*.json` texts only.
- N2: `tick(1)` per scope in `out_of_scope` is a test-only counter (no-op inline in production).
- N3: Extension-less file patterns do not take on TypeScript's inferred extensions (fail-open deviation, pinned).
- N4: Gradle `options.encoding`, Yarn `portal:`/`git:`/`patch:`, `buildSrc`/`ext`/applied-scripts, vendor-mode workspaces, non-JDK charsets stay residue per the contract scope table.
- N5: `PIPELINE` unbumped for all of M6e (`m6c-5`); every fixture dump and measurement diff classified.

## Suite at review time

fmt clean, clippy `-D warnings` clean (workspace). Unit counts re-derived in pass 2. e2e 36/36 + api 1/1 green unblessed at phase-10 verify; phase-11 measurements deterministic and classified.
