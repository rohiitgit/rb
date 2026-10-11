# M6f contract: contract packs — OpenAPI, gRPC, generated interfaces, MyBatis XML (draft, 2026-10-12)

**Status:** contract written 2026-10-12; forks F1–F5 decided below (no orchestrator round-trip:
M6e ran end to end by user direction); phase 0 not started.

Written before any code. M6f is done only when every assertion below is backed by a passing test or a
recorded measurement. "Verified facts" are what the 2026-10-12 probes read in the tree at commit
`376dbcb` (M6e closed; `PIPELINE` `m6c-5`).

This contract follows the M6e format and applies, from the start, the lessons M3b–M6e paid for
(soundness bar + closing bar up front; allow-lists; suppressions counted; growth by counted ops;
no hand-counted positions; invariant 1 vs previous close with classified rows; fixture plan is code;
config-surface probing; root-manifest shape checks; no builds during container e2e; non-empty dumps).

## Soundness claims and closing bar (read first)

M6f's outputs whose failure would be silent: a spec/proto endpoint that misstates its path or method
(a misread YAML indentation, a proto service missed or doubled, a `$ref` followed into another file);
a handler joined to the wrong spec operation (method+path matched loosely); a MyBatis statement
attributed to the wrong table (placeholder SQL misread); a generated-client call stitched to the
wrong operation (name collision across services); a version pinned from a spec `info.version`
mistaken for a package version. **Everything not proven takes the safe direction:** an unreadable
spec/proto/mapper pins nothing (no endpoints, `unread` + counted diagnostics); an unresolvable
handler stays unhandled (`HANDLED_BY` absent, endpoint kept with its certainty); a client match
above `synthetic` never happens (M4a invariant holds: `CONSUMES` never `exact` — this contract
deviates from design §10.2's "this is exact" line, written before M4a capped certainty, with reasons
in D5); duplicate operations across files never merge silently (first kept, conflict counted);
`info.version` never becomes a package version.

| Surface | Claim (exactly this, no more) | Mechanism | Residue (out of scope by construction) |
|---|---|---|---|
| OpenAPI reading (S1) | `openapi.{json,yaml,yml}` paths+methods+operationIds read exactly as written; anything else ignored | JSON via `serde_json`; YAML by an indented line reader over allow-listed shapes (no new dependency, M6e-F4 precedent) | `$ref` (followed nowhere: local or remote), `servers`/`webhooks`/`callbacks`, non-`paths` endpoints, YAML anchors/merge keys |
| Proto reading (S2) | `service`/`rpc` names from `.proto` files | `tree-sitter-proto` if it binds the pinned runtime (phase-1 probe), else a hand-rolled block reader; allow-listed shapes only | `stream` modifiers recorded, not keyed; `option` bodies unread; nested messages unmodelled |
| Spec endpoints in graph (S3) | Each spec operation is an `Endpoint` (`HTTP …` from OpenAPI, `GRPC pkg.Service/Method` per design §8.2); keyed with the owning service | `rb-indexer/src/routes.rs` extension; service = nearest project of the spec file | Specs outside any project stay endpoint-less (counted) |
| Handler joining (S4) | A spec operation joins its code handler by the M4a normaliser (same path+method equality, same D9 specificity); joined `HANDLED_BY` is `synthetic` at most | Reuse `endpoint.rs` matching; certainty from the match row | Controllers implementing *uncommitted* generated interfaces stay unhandled (S6 probes whether SCIP sees them; committed interfaces read directly) |
| MyBatis XML (S5) | `<mapper>/<select\|insert\|update\|delete>` statements become SQL candidates resolved by `rb-sql` to tables (`QUERIES` from the mapper method) | Reuse the `Element` XML reader + `rb-sql` statement walk; `#{}`/`uris` placeholders folded like sink-call SQL | `<include>`/fragments, dynamic tags (`<if>`/`<choose>`) beyond placeholder folding, `resultMap` joins |
| Generated clients (S6) | OpenAPI-generator operationId calls + gRPC stub calls (`*Stub`/`*BlockingStub`/`*FutureStub` receivers, RPC method name) become client sites stitched by operation/RPC name | `clients.rs` packs; name match scoped to the operation's service first, then repository-wide `ambiguous` | Feign-by-operationId stays URL-based (M4a residue); hand-rolled clients unchanged |
| Fixtures + guards (S7) | One fixture carrying all five shapes with per-class guards, each biting | `fixtures/contract-packs/` + e2e fns + goldens | — |
| Public repos (S8) | Classified diffs + read-in-source samples; 0 false endpoints/handlers/edges | Measurement plan below | New forms recorded for their owners |

**What M6f must not change.** `INJECTS`/DI/RTA/orphan rules untouched. No new edge kind (`EXPOSES`/`HANDLED_BY`/`CONSUMES`/`QUERIES` reused; diagnostics counted non-zero-only). `PIPELINE` becomes `m6f-1` at the first dump change, bumped on each later one.

This is invariant 1, checked against `376dbcb` dumps of every fixture (with and without SCIP) and of the M6f
measurement repositories at their pins, every differing row classified by script (dumps asserted non-empty first).

**Closing bar.** Decided now:
1. **Evaluator pass 1 is a full review,** adversarial probing included (YAML indentation smuggling, `$ref` games, proto service doubling, handler over-joining, mapper placeholder confusion, stub-name collisions, version confusion).
2. **Evaluator pass 2 is verification-only.**
3. **Blocks closing only:** (a) a pass-1 fix that does not hold; (b) a regression; (c) a false endpoint/handler/edge above its stated certainty through a controlled form; (d) non-determinism; (e) a failed test/guard/step; (f) an exact `CONSUMES` anywhere.
4. **Not blocking:** a new form in a residue class (recorded); a precision miss contradicting no rule; §12.5 timing (measured, not asserted).
5. **Timings are measured, not asserted.**

## Scope

**In (M6f):** the 8 items above (design §13 M6f row; contract-m4b item 9).

**Out of M6f (each assigned):** topology/config packs → M6g; `rb check` → M6h; Kotlin/C# → M6i (Kotlin exact edges need Gradle builds — ready since M6e); Guice/Dagger/NestJS/Spring-XML/.NET → M6j; `$ref` following, `servers`/`webhooks`, proto options/nesting, MyBatis dynamics, Feign-by-operationId → residue.

## Engineer declaration

**Canonical state:** new readers (OpenAPI JSON + YAML lines, proto, mapper XML) as `FileFacts`-adjacent or pure functions; `Endpoint` nodes with spec provenance; join rules; per-file client sites for stubs; `PIPELINE` `m6f-1`… at each dump change. New counters only when non-zero.
**Derived:** endpoints, handler joins, table edges, stitched client edges.
**Invariants:**
1. **Dumps.** `--no-scip` and SCIP dumps differ from `376dbcb` only in classified rows; nothing unclassified; every compared dump asserted non-empty.
2. **Kinds.** No new node or edge kind.
3. **Certainty monotonicity.** No `CONSUMES` above `synthetic`; no `HANDLED_BY` above `synthetic`; no `exact` version from `info.version`.
4. **Determinism.** Fresh == cached, two names, forced rebuilds: byte-identical on fixtures and measurement repos.
5. **Bounded work.** Every new walk has a visited set + caps; growth ≤ 5× ops at 4× input per new walk.
6. **Stated complexity by counted operations** for each new walk.

## Decisions (made here, with reasons)

- **D1: YAML is hand-rolled (F1).** A line reader over `paths:` → path → method → `operationId` (2/4/6-space indentation, `\t` rejected, deeper keys ignored) — no new dependency, no general YAML surface. Anything outside the shapes is `openapi_unread` + no endpoints (today's safe direction: specs are invisible today).
- **D2: proto prefers the grammar (F2).** `tree-sitter-proto` if it compiles against the pinned runtime (phase-1 probe: parse a `service`+`rpc` sample, read node kinds); else a `service N { rpc M … }` block reader with brace depth cap. Either way allow-listed.
- **D3: specs are authoritative for existence, not for handling (F3).** A spec operation always becomes an endpoint of its service (certainty `exact` for existence — it is stated, not inferred); `HANDLED_BY` only on a normaliser-equal code route (`synthetic` at most). An unhandled spec endpoint is kept and counted, never dropped.
- **D4: MyBatis rides rb-sql (F4).** Statement SQL text with `#{p}`/`uris` folded to placeholders feeds the existing statement walker; the mapper method (namespace + id) is the query site. `<include>`/dynamics are residue with examples.
- **D5: generated clients cap at synthetic (F5).** Design §10.2's "exact" predates M4a's certainty cap; operation/RPC-name matching is precise but the stitch still crosses the repository boundary by name, so `synthetic` is the max. Deviation recorded here.
- **D6: fixture plan is code:** `fixtures/contract-packs/` (OpenAPI YAML + JSON specs, one `.proto` with two services, a MyBatis mapper + a dynamic-tag decoy, a gRPC stub call site, an operationId call site, controllers implementing a *committed* generated interface).
- **D7: measurement reuses pins where they exist;** new pins (an OpenAPI-carrying repo, a proto-carrying repo) chosen in preflight by `git ls-remote` + API, never by cloning first.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | **OpenAPI reading.** JSON + YAML shapes pin paths/methods/operationIds; everything else ignored + counted. | unit; mutation |
| S2 | **Proto reading.** Services + RPCs; streams recorded. | unit; mutation |
| S3 | **Spec endpoints in graph.** `HTTP`/`GRPC` nodes keyed per service; specs outside projects counted. | unit; mutation; e2e goldens |
| S4 | **Handler joining.** Normaliser-equal joins only; unhandled kept + counted. | unit; mutation; e2e goldens |
| S5 | **MyBatis XML.** Statements → tables via rb-sql; dynamics out. | unit; mutation; e2e goldens |
| S6 | **Generated clients.** operationId + stub sites stitched ≤ synthetic. | unit; mutation; e2e goldens |
| S7 | **Fixture + guards.** Per-class must-exist/must-not-exist, each biting. | e2e with containers; goldens |
| S8 | **Public repos.** Classified diffs + samples; 0 false endpoints/handlers/edges. | recorded measurement |
| S9 | **Determinism + invariant 1.** Fresh == cached, two names, forced rebuilds; dumps vs `376dbcb` classified. | e2e; recorded |
| S10 | **Growth by counted operations** (invariant 5). | unit |
| S11 | **Verification loop + closing.** `var/m6f/scripts/verify.sh`: fmt, clippy, counted units, store/server with DB, e2e + api with containers, listener + stub checks. | verification loop |

## Verified facts (read at `376dbcb`, 2026-10-12 probes)

1. Zero OpenAPI/proto/gRPC/MyBatis fixtures or code paths (`grep -ri` over `fixtures/`, `rb-frameworks`, `rb-syntax`: only `endpoint.rs` URL parsing).
2. `Endpoint` key space reserves `GRPC pkg.Service/Method` (design §8.2); HTTP keys end ` @service` (M4a D1).
3. `CONSUMES` never `exact` (M4a invariant, enforced in `stitch.rs`); design §10.2 "generated clients exact" predates it.
4. No YAML parser in the workspace (`rb-frameworks`/`rb-indexer`/`rb-syntax` manifests); `serde_json` everywhere; `toml` in `rb-indexer`.
5. `tree-sitter-proto 0.6.0` on crates.io (grammar binding unknown — phase-1 probe).
6. An `Element` XML reader exists for pom parsing (M4a-era, reused for mapper XML); `rb-sql` walks statement trees from text.
7. M4b item 9 residue cases: spring-petclinic-rest `visits`/`users` (generated interfaces), realworld-spring `articles`/`follows` (MyBatis XML).

**Not verified (checked in preflight, before the step that needs it):** tree-sitter-proto runtime binding + node kinds; YAML shape incidence on measured repos; scip-java visibility of uncommitted generated sources (S4/S6 probe); measurement pins; operationId/stub call shapes on measured repos.

## Fixture plan

- `fixtures/contract-packs/`: `openapi.yaml` (2 paths, 3 operations, 1 operationId) + `openapi.json` (1 path, non-2-space indent decoy shapes), `svc.proto` (2 services, 4 RPCs, 1 streaming), MyBatis `UserMapper.xml` (select/insert + `<if>` decoy) over a small schema, a Java controller implementing a committed `UsersApi` interface, TS + Java gRPC stub call sites, an operationId call site. Goldens: endpoints, joins, table-deps, path/callers guards, status lines.

## Public-repository measurement plan

| Step | Repository @ pin | Runs | Notes |
|---|---|---|---|
| Phase 0 | — | `376dbcb` binary dumps (`--no-scip`); e2e green baseline | small |
| Phase 8 | OpenAPI-carrying repo @ pin (chosen preflight) | `--no-scip` both binaries | classified diffs, samples |
| Phase 8 | proto-carrying repo @ pin | `--no-scip` both binaries | classified diffs, samples |
| Phase 8 | existing public repos | N/A (no new reads on old shapes) | — |

Disk policy as M6b D9 (6 GiB floor, one repo at a time, remove + `VACUUM`, part-by-part figures).

## Determinism

No wall-clock/host-path/run-order in dumps (unchanged rules). Fresh == cached, two names, forced rebuilds on fixtures (e2e); `--no-scip` both binaries byte-identical modulo classified rows.

## Verification loop

`var/m6f/scripts/verify.sh <tag>` (copied from M6e's, M6f paths/logs): fmt, clippy `-D warnings`, counted units, `rb-store`/`rb-server` with DB, e2e + api with containers, listener + stub checks, mutation runs per M4a harness rules.

## Forks (decided here, with reasons)

- **F1: YAML hand-rolled.** No new dependency; line reader over allow-listed indentation; everything else `openapi_unread`.
- **F2: proto prefers the grammar.** tree-sitter-proto if it binds (probe); else a brace-capped block reader. Both allow-listed.
- **F3: specs authoritative for existence only.** Endpoints always created; handling only by normaliser equality.
- **F4: MyBatis rides rb-sql.** Placeholders folded; dynamics residue.
- **F5: generated clients cap at synthetic.** Deviation from design §10.2 recorded with reason (M4a cap postdates it).

## Amendments

1. **(phase 0) Baselines, pins and shape checks.** Baseline binary `var/m6f/base/rb-376dbcb` (release, sha256 recorded; HEAD code == `376dbcb` — the contract commit adds no code). Full-SHA pins verified by `git ls-remote`: grpc/grpc-go `8c59ee6356bf92d0a5819b0cdbd3118876783028` (master; `examples/helloworld` carries client + server + `.proto` in one place, verified via API directory listing), OAI/OpenAPI-Specification `ffb3e579dfb4ef5ae1a68d11af125e2a593505fc` (main; `tests/` carries real-world spec files for reader breadth). honojs/examples probed and dropped (no committed OpenAPI surface at top level). Shape checks: zero OpenAPI/proto/gRPC/MyBatis fixtures or code paths; `GRPC pkg.Service/Method` key space reserved (design §8.2); `CONSUMES`-never-`exact` enforced in `stitch.rs`; no YAML parser in the workspace; `Element` XML reader exists for reuse. tree-sitter-proto 0.6.0 binds tree-sitter 0.27 cleanly (scratch `/tmp/protoprobe`: `service > service_name`, `rpc > rpc_name` node kinds confirmed) — F2 resolves to the grammar; `stream` modifiers need a span-text check (no node kind for them, recorded). Forks F1–F5 decided in the contract with reasons. DB holds only the 3 pre-existing `fx_*` repos; disk 7.4G free.

2. **(phase 1) OpenAPI reading.** `rb-frameworks/src/spec.rs`: `openapi_json` (`serde_json`, `paths` object required, path must start with `/`, seven verbs, operationId optional) + `openapi_yaml` (0/2/4/6-space shapes, quoted paths, comment stripping, first operationId wins; tabs or entries-with-zero-ops make the document unreadable) + `is_spec_input` (six basenames, with/without service dirs). Ops sorted canonically. Tests first (4 units; new module did not compile before `pub mod spec`); mutants killed: verb allow-list widened, unreadable-entries rule dropped (each fails its test); growth ratio 4x→≤5x via `work::tick`. No graph change: e2e untouched by construction. Verify: fmt, clippy clean; 158 rb-frameworks units.
