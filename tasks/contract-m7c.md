# M7c contract: HTTP recall on supported stacks (stress batch C, part 1) (2026-10-13)

**Status:** contract written 2026-10-13 from `tasks/stress-findings.md` batch C; started. Stacked on M7b (`m7b-regressions-and-hangs`, `a66c460`). Batch C part 2 (F28 .NET DI, F26 Nest `@InjectRepository`, F23 SQL roles) is M7d.

## Why

On bookcars (`4ea6c11`) the stress campaign found Express and axios shapes silently lost on stacks `rb` claims to support:
- **F20:** 110 routes with constant paths are dropped, with no diagnostic.
- **F21:** about 195 calls through a default-exported axios instance are dropped from every count.
- **F12:** `const app = require('express')()` yields no app.
- **F27:** a non-gRPC `new OAuth2Client()` is labelled a gRPC stub.

## Claims

| # | Claim | Mechanism | Residue |
|---|---|---|---|
| S1 (F20) | **Express paths written as names fold like client URLs.** `verb(NAME, …)` and `route(NAME).verb(…)` keep the name. The routes stage folds it through the same cross-file constant lookup stitching uses (imported constants, members of imported or default-imported objects). A name that does not fold gives a `route_path_dynamic` diagnostic, never a silent drop. A folded path is `heuristic` when it crossed files, `exact` otherwise (as for mounts). | Pack: `RouteFact.path_name`. Indexer: the shared constant lookup (extracted from `stitch.rs` `Folding`) is used by `routes.rs` | Names from config files or the environment stay dynamic (counted) |
| S2 (F21) | **A default-imported axios instance is the target module's default export.** `import api from './axiosInstance'` resolves `default` through `FileFacts::default_export`, as `mount_targets` does for routers. | `rb-indexer/src/stitch.rs` `imported_instance` | One level, as for routers |
| S3 (F12) | **`require('express')()` is an app and `require('express').Router()` a router.** | `rb-frameworks/src/routes.rs` `router_kind` | — |
| S4 (F27) | **A TS/JS `*Client` is a gRPC stub only when imported from a generated-code module** (specifier containing `grpc`, `_pb` or `proto`, case-insensitive, or a whole path segment `gen`/`generated`; the latter kept so M6f's `./gen` pin stands). Locally declared or third-party `*Client` types are not stubs. | `rb-frameworks/src/clients.rs` `generated_calls` | Stubs re-exported through an index module named otherwise are missed (recorded) |

## Verification

- Tests first per claim, with a mutant per rule.
- fmt, clippy `-D warnings`, and workspace tests with the DB **and real Docker**, with 0 failures and no `skipping:` lines.
- Invariant against the M7b binary on 18 fixtures and 13 stress repos, with every differing row classified. bookcars is expected to gain routes, client sites and stitched `CONSUMES`, and to lose 1 gRPC site.
- `PIPELINE` bumped to `m7c-1`, since dumps change.

## Progress (amendment 1, 2026-10-13)

- **Phase 1 (S3, S4):** inline `require("express")()` / `.Router()` factories (`inline_express`). TS/JS `*Client` stubs need a generated-code import (`grpc`/`_pb`/`proto`, or a `gen`/`generated` path segment, so the M6f `./gen` pin stands). Mutants m26–m30 killed.
- **Phase 2 (S2):** `imported_instance` resolves `default` through the target's `default_export`. Mutant m31 killed.
- **Phase 3a (S1 groundwork):**
  - `crates/rb-indexer/src/constants.rs`: shared `Constants` (`Lookup`, no properties), extracted from stitch's `Folding`, which now wraps it and adds service properties.
  - `external` also resolves default imports.
  - `Folder::object_member` folds `obj.key` on a const TS/JS object-literal binding, local or imported (last entry wins; nothing if a spread follows). Client URLs benefit too.
  - Mutants m32–m35 killed.
- **Phase 3b (S1):**
  - `RouteFact.path_name`: Express keeps `Arg::Name`. Other non-literal paths are now counted as `route_path_dynamic` instead of dropped.
  - `add_endpoints` folds the name (constant or literal origin), caps it at `heuristic`, or counts it dynamic.
  - Mutants m36–m39 killed.
- **Phase 4:**
  - `PIPELINE` `m7c-1`. fmt and clippy are clean.
  - Workspace with DB and **real Docker** (`var/m7c/logs/p4-workspace.log`): **990 passed, 0 failed, 0 skipped**, all SCIP e2e included.
  - Invariant against M7b (`var/m7c/logs/invariant.txt`, binary sha256 `a3785208…`): **29/31 targets byte-identical**, all 18 fixtures included.
  - **bookcars:**
    - +107 endpoints (all `backend`, `heuristic`) and +107 `HANDLED_BY`.
    - +175 `CONSUMES` (`heuristic`). 6/6 sampled are correct against the source (`verifyRecaptcha`, `update-car-image`, `sign-up` and others).
    - −1 false gRPC site (`OAuth2Client`).
    - +2 `route_path_dynamic` (`settingRoutes`, see residue).
    - +16 `http_unmatched`, all classified: 5 client URLs with a literal trailing `/ ` in source; 3 POSTs to a GET-only route (`confirm-email`, a real mismatch in bookcars); 4 calls to the dynamic settings routes; 4 case mismatches (`/api/Notifications` against `/api/notifications`, see F32).
  - **mattermost:** −29 `http_unmatched`, all false gRPC sites (27 on its own `WebSocketClient`, 2 on its Cypress `E2EClient`), removed by S4.

**Residue:**
- An anonymous `export default { … }` object (no binding) does not fold. Its routes are counted dynamic (bookcars `settingRoutes`, 2 routes).
- **New finding F32 (S3):** Express matches paths case-insensitively by default, but stitching compares case-sensitively. bookcars has 4 such calls.
