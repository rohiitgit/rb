# M7a contract: `exact`-label correctness (stress batch A) (2026-10-13)

**Status:** S1–S3 implemented and verified 2026-10-13 (amendment 1). Uncommitted; evaluator review pending.

## Why

The stress campaign found three places where `rb` states a wrong answer as `exact`. All three were reproduced on public repos at pinned commits:
- **F17** (microservices-demo `38e7348e`): proto copies attribute gRPC endpoints to services that don't implement them.
- **F18** (fastapi-realworld `029eb778`): FastAPI `include_router` prefixes from another module are dropped, so routes merge under wrong `exact` paths.
- **F19** (microservices-demo): Go protobuf getters are reported as RPC calls, while the real `pb.NewXClient(c).Rpc(…)` calls are missed.

The goal is that every `EXPOSES` and `CONSUMES` row these packs emit is either right or labelled for what it is: `prefix_unknown`, a counted diagnostic, or below `exact`.

## Claims

| # | Claim | Mechanism | Residue (stays, recorded) |
|---|---|---|---|
| S1 (F19) | **A Go/TS gRPC client binding is the constructor's result, never a later link's.** A binding is typed as an `XClient` only when a `New*Client` constructor is the **last** link of its initialiser (or of the one chain in its `:=` statement). A chain whose link *before the last* is a `New*Client` constructor reads its last link as the RPC (`pb.NewXClient(conn).GetX(ctx, req)`). | `rb-frameworks/src/clients.rs` `generated_calls` (`Init::Call`, `None` branches) and the multi-link branch | Clients stored in struct fields (`s.cartClient.GetCart`) stay as they are today. |
| S2 (F18) | **FastAPI composition goes through the shared router composer.** Apps and `APIRouter`s become `RouterDecl`s (`App`/`Router`). Decorated routes are `RouteBase::Router`, with the router's own literal `prefix=` folded into the route path. `include_router(child, prefix=…)` becomes a `Mount` whose child may be local, a from-imported name, or `module.router` through a from-imported submodule. A non-literal prefix (own or mount) and a router never mounted give `prefix_unknown`, never a root path. | `rb-frameworks/src/routes.rs` `fastapi`; `rb-indexer/src/routes.rs` `mount_targets` (Python submodule attribute) | `app.mount` (Starlette sub-apps) stays unread. A prefix from settings (`settings.api_prefix`) stays unknown. |
| S3 (F17) | **gRPC endpoints belong to the project that registers a server for the service.** Per file, allow-listed registrations name a proto service `X`: Go `Register{X}Server`, Python `add_{X}Servicer_to_server`, JS/TS `addService(<…>.{X}.service, …)`, Java class supertype `{X}Grpc.{X}ImplBase` / `{X}ImplBase`, C# supertype `{X}.{X}Base`. `X` must name exactly one `package.Service` across the repository's protos, including protos outside projects. A registered service's RPCs are endpoints of each registering project. The RPC span is the evidence, and path certainty is `exact` when that project also holds a declaring proto, `heuristic` otherwise. With no registration anywhere: a service declared in **one** project keeps today's behaviour, and one declared in **several** projects gets no endpoint plus one `grpc_implementer_unknown {service, declared_in}` diagnostic (counted). | `rb-frameworks` (new `FileRoutes.grpc_servers`), `rb-indexer/src/routes.rs` `add_spec_endpoints` | Registrations through helpers or reflection, and a service name matching several packages, are counted as `grpc_registration_ambiguous`. |

## Decisions

- **D1: allow-lists only.** Registration shapes are positive patterns tied to generated-code naming. No suffix guessing beyond `{X}.{X}Base` and `{X}Grpc.{X}ImplBase`, where the doubled `X` is the guard.
- **D2: compatibility by construction.** A single-copy proto with no registration keeps today's output, as do in-file FastAPI compositions. Fixture dumps must be byte-identical except where a claim applies, and any row that moves is classified.
- **D3: lessons applied.**
  - Cycle and depth tests for the composer path (FastAPI include cycles are already capped by `MAX_MOUNT_DEPTH`).
  - Suppression is counted (`grpc_implementer_unknown`).
  - Simple-name attribution is below `exact` unless co-located (S3).
  - Edge-set diffs are classified before any sentence about a metric.

## Verification

1. Tests first per claim, in rb-frameworks and rb-indexer units, with a mutant killed per rule.
2. `cargo fmt --check`, `clippy -D warnings`, and the workspace tests **with the DB** (`RB_TEST_DATABASE_URL`, fake `docker` shim while Docker Desktop is wedged), with no unexplained `skipping:` lines.
3. **Invariant against the stress baseline** `var/stress/bin/rb` (`eb496af`): `--no-scip` dumps of all 18 fixtures and the 12 stress repos, with every differing row classified.
4. Re-run repros F17/F18/F19 (`var/stress/repro/`). Expected:
   - F17: `ProductCatalogService/GetProduct` resolves to productcatalog only, and adservice lists only AdService.
   - F18: `/api/articles/{}` paths, with no `DELETE /{}` holding two handlers.
   - F19: getters are gone and the real Go calls are found.
5. `PIPELINE` bumped `m6l-1` → `m7a-1`, since dumps change on real repos.

## Progress (amendment 1, 2026-10-13)

1. **(phase 1, S1)** `clients.rs`: a binding is a gRPC client only when a `New*Client` constructor is the *last* link, in both the `Init::Call` and the `:=` statement association. `pb.NewXClient(c).Rpc(…)` is read as the RPC when it is **exactly** that two-link shape and the RPC is not itself a constructor.
   - The two-link guard comes from invariant measurement. mattermost's Azure SDK chains (`….NewContainerClient(c).NewBlobClient(p)`) produced 2 false gRPC sites under a first `[.., ctor, rpc]` rule.
   - The test failed first. Mutants m1–m3, m13 and m14 were killed; m13/m14 needed separate negatives (a 3-link SDK chain, and ctor-after-ctor).
2. **(phase 2, S2)** The FastAPI pack emits `RouterDecl` (`App`/`Router`, top-level including Python's module-level `Constant` definitions) and `Mount` (`include_router`, non-literal prefix → `None`). Routes are `RouteBase::Router` with the router's own literal prefix folded in. Its private composer was removed.
   - `rb-indexer` `mount_targets` resolves `module.router` through a from-imported submodule, restricted to that submodule's own file.
   - The tests failed first. The 3 pack tests now pin per-file facts, and in-file composition is pinned at indexer level (`fastapi_in_file_includes_stay_exact`). Mutants m4–m7 were killed.
3. **(phase 3, S3)** `FileRoutes.grpc_servers` holds the allow-listed registrations for Go, Python, JS/TS, Java and C#.
   - `add_spec_endpoints` collects every proto declaration, including those outside projects. Registered services are attributed to the registering projects: `exact` with an own copy, `heuristic` otherwise, with registration sites as `EXPOSES` evidence.
   - Unregistered services keep M6f behaviour when one project declares them; with several, they get no endpoint plus `grpc_implementer_unknown`. A name matching several packages gives `grpc_registration_ambiguous`.
   - The tests failed first. Mutants m8–m12 were killed; m9 first survived, and the test gained a one-copy-implemented-elsewhere case.
4. **(phase 4)**
   - `PIPELINE` `m6l-1` → `m7a-1`. fmt and clippy are clean.
   - Workspace with DB: **980 passed, 2 failed**. The 2 are the pre-existing F1 (`status` without `--repo`, batch B), the same tests as at baseline `eb496af`. There are no `skipping:` lines, and all e2e golden tests pass unchanged.
   - Invariant (`var/m7a/logs/invariant*.txt`): **all 18 fixtures byte-identical**, and 9 of 12 stress repos identical. Classified differences:
     - **microservices-demo:** 41 misattributed gRPC endpoints removed. 13 added: Health ×5 and the Go/Python implementers, `heuristic`, where no own proto exists. Every service now exposes exactly what it registers in source.
     - **microservices-demo stitching:** 6 `ambiguous` `CONSUMES` into the wrong services were replaced by 19 single-target links (synthetic/heuristic), 7/7 sampled real calls. 10 getter `http_unmatched` are gone, and 0 getters remain.
     - **fastapi-realworld:** 12 merged root-path `exact` endpoints became 19 per-router `prefix_unknown` `heuristic` endpoints (the app mounts under `settings.api_prefix`). `DELETE /{}` no longer holds two handlers.
     - **mattermost:** after the phase-1 tightening, the second run (`invariant2.txt`, binary sha256 `efa016ac…`) has **29/31 targets byte-identical**, including mattermost. Only the two classified repos differ, with the same rows as above.
   - Final DB suites on the final code: e2e 46/47, api 0/1 (both F1, pre-existing), store 72, server 12, CLI units 28. No `skipping:` lines.
   - Repro queries on the M7a binary: `GetProduct` resolves to productcatalog only (upstream: checkout and frontend), adservice exposes 1 API (was 15), the two `DELETE /{}` routes are separate `prefix_unknown` endpoints, and the only remaining gRPC `http_unmatched` is the Java `blockingStub` site (batch C, stress F25).
   - The invariant repos were removed (62 repos and 62 graphs, in batches of 15). The DB holds only the 4 pre-existing repos.

**Residue added:**
- When a router's top prefix is unknown, the composer keeps the route's local path and drops the inner known prefixes (fastapi-realworld: `/articles`, `/articles/{slug}/comments`). Keeping the known tail below an unknown mount would sharpen suffix stitching for Express and FastAPI alike. Recorded for the composer, not done here.
- gRPC handler methods aren't linked, so `spec_operation_unhandled` still fires on registered services.
- Health registered through helpers (`healthMgr.getHealthService()`) and C# `HealthBase` aren't read.
