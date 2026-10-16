<!-- Source: opencode plan-agent session c1657236 (2026-10-16), msg_1415b358. Unverified claims; each is checked in its contract. -->
# M9 Plan — Accuracy & Recall Driven by the M8 Benchmark

M8 built the answer-key scoreboard (`bench/scoreboard/m8b.json`, 314 cells + 249-edge `exact` sample) and recorded 14 findings (`tasks/bench-findings.md` B1–B14) without touching `rb`. **M9 turns the benchmark into a closed-loop gate**: every change is driven by a specific B-finding, verified against the 31-target invariant, and gated by `rb-bench compare` with zero regressions.

---

## 1. Architecture & High-Level Design (`engineer` + `system-design`)

```mermaid
flowchart LR
    subgraph P1["M9a: exact Precision (B7, B8, B13, B14, B11)"]
        T1[Routes: filter is_test_path<br/>+ code-over-spec precedence]
        T2[Config: Spring Cloud Config<br/>shared/*.yml context-path]
        T3[SCIP join: method-to-iface<br/>bypass uses holder type]
    end
    subgraph P2["M9b: Cross-Job SCIP & Go/Spring Recall (B1, B9, B10, B12)"]
        T4[SCIP join: two-pass repo-wide<br/>defined symbol set]
        T5[Go Tier B: package-wide<br/>method receiver attachment]
        T6[SQL & Spring: Sprintf const fold<br/>+ literal '{param}' routes]
    end
    subgraph P3["M9c: Tier B Call Recall (B2, B3, B5)"]
        T7[Go constructor return types<br/>& single-impl interfaces]
        T8[TS namespace imports<br/>behind tsconfig paths]
        T9[Python syntactic calls:<br/>super, star-import, raise]
    end
    P1 --> GATE[rb-bench score & compare<br/>+ 31-target invariant]
    P2 --> GATE
    P3 --> GATE
```

### State, Derived State, and Invariants (`engineer` protocol)

- **State (canonical inputs, unchanged in schema):**
  - Per-blob `FileFacts` (`rb-syntax`, fingerprinted in facts cache).
  - Per-file config map `Content.config` (`application*`, `bootstrap*`, shared config-server files) and OpenAPI/proto `Content.specs`.
  - Per-job decoded `ScipIndex` documents (`rb-scip`).
- **Derived (computed in `rb-indexer`, never duplicated):**
  - Repo-wide SCIP anchored symbol set `RepoDefined: BTreeSet<String>` (pass 1 across jobs) → cross-job `CALLS`/`REFERENCES` in pass 2.
  - Package-level Go receiver method table `(pkg_key, receiver_type_name) -> Vec<Sym>` in `resolve::Index`.
  - Effective service `context_paths` merging config-server `shared/<service>.{yml,properties}` at rank below service-local config.
  - Endpoint table where code registrations (`from_spec == false`) take precedence over spec-only annotations and test-path routes (`is_test_path(path)`) do not emit service `EXPOSES`.
- **Invariants:**
  1. **Zero false `exact`:** `rb-bench compare` rejects any rise in `fp` at `Certainty::Exact` (even if listed in `--accept`). Target: independent-key `exact` goes from **122/130 (93.8%) → 122/122 (100.0%)**, and the `exact` sample's `routes` (58/60) and `scip` (57/60) error counts drop by their 5 diagnosed bugs.
  2. **Monotonic scoreboard:** `tp` never falls and `fp`/`fn` never rise on any unaccepted cell across all 314 cells.
  3. **Determinism & cost (NFRs):** two runs produce `cmp`-identical scoreboards; warm full benchmark run **≤ 20 min** (currently 2m 42s warm); `mattermost` `--no-scip` fresh index **≤ 29 s** and peak RSS **≤ 1.90 GiB** on Apple M4.

---

## 2. Root Causes & System-Specific Trade-offs (`system-design`)

| Finding | Exact Root Cause in Code | Options & Trade-off (with math / mechanism) | Chosen |
|---|---|---|---|
| **B7** (2 wrong `exact` endpoints in `hk_todo`) | `crates/rb-indexer/src/routes.rs:487` iterates all `inputs.routes` including `*_test.go`, emitting `EXPOSES` for test-harness routers (`middleware_test.go:92`, `auth_test.go:80`). | **(A)** Skip `is_test_path(path)` in `add_endpoints` (matches `di.rs:296`, `rta.rs:551`, `liveness.rs:242`). Cost: 0 extra passes; drops 2/8 wrong `exact` edges, but loses 32/60 mattermost e2e-cypress/playwright routes in the `exact` sample.<br>**(B)** Only skip `is_test_path(path)` when the file's service has ≥1 non-test source file (so dedicated e2e services keep their routes while unit tests inside an app service don't pollute `EXPOSES`). | **B** — fixes `hk_todo` (2 fp → 0) without turning mattermost's e2e webhook servers vacuous. |
| **B8** (2 wrong `exact` sample routes in `go_gin_example`) | Swagger comment/spec ops in `routes.rs:853` push `Registration { path_certainty: Exact, from_spec: true }` even when code registers the same handler at a different verb/path (`POST /auth` vs `@Router /auth [get]`). | **(A)** Cap spec-only registrations without a matching interface/code route at `Heuristic` (`from_spec && handler.is_empty()`).<br>**(B)** Keep OpenAPI files `Exact` (contract M6f S1) and only cap comment-extracted `@Router` annotations or suppress spec ops whose `operationId`/handler is already bound at a different `(method, path)` in code. | **B** — preserves M6f OpenAPI fixture goldens while killing the 2 `go_gin_example` contradictions. |
| **B13** (3 wrong `exact` SCIP `IMPLEMENTS` on `mattermost`) | In `crates/rb-indexer/src/scip.rs:649-655, 706-709`, when `bypass` is true (Go implicit interface, M6d S9), `target` is promoted to `type_symbol` (`ChannelMemberHistoryStore#`) because the interface method has no node, **but `src` is left as `source_key` (the method `GetMembershipChanges()`)** instead of its enclosing type `holder`! | **(A)** When `bypass` promotes `target` to `type_symbol`, also promote `src` to the holder type's symbol (`replaced[holder.1]`). Deduplicate `(src, Implements, dst)`.<br>**(B)** Create Symbol nodes for Go interface methods. Cost: bloats symbol count across all Go repos and breaks M1b/M6d goldens. | **A** — 5-line fix at `scip.rs:706`; turns method→interface into type→interface and deduplicates across methods. |
| **B14** (6 wrong `exact` + 6 fn in `mse_piggymetrics`) | `spring_config` (`crates/rb-indexer/src/lib.rs:660`) assigns `config/src/main/resources/shared/account-service.yml` to service `config`, never to `account-service`. Also `routes.rs:535` labels `context_paths`-prefixed routes `Synthetic`, whereas unprefixed routes are `Exact`. | **(A)** Match `**/shared/<svc>.{yml,yaml,properties}` (and `<config-server>/**/<svc>.{yml,properties}` when `spring.cloud.config.server.native.search-locations` names `classpath:/shared`) to service `<svc>` at rank `-1` (below service-local `bootstrap`/`application`).<br>**(B)** Match any `<svc>.yml` anywhere in repo. Risk: false prefix match on unrelated YAML. | **A** — gated by config-server `search-locations` or `shared/` directory convention; fixes all 6 fp + 6 fn on `piggymetrics`. |
| **B12a** (4 fn on `mse_anilallewar` & `mse_rohitghatol`) | `mapping_paths` (`crates/rb-frameworks/src/routes.rs:528`): `if arg.items.is_empty() && !arg.value.starts_with('{')` mistakes string literal `"{taskId}"` (`arg.literal == true`) for an empty Java array `{}` (`arg.literal == false`), returning `[""]`! | Check `!(arg.value.starts_with('{') && !arg.literal)` on line 528. | **1-line fix** — zero risk, immediately recovers 4/15 microSecEnD endpoint misses. |
| **B9** (0/13 `handler_binding` on `hk_todo`) | `attach_go_methods` (`crates/rb-syntax/src/lib.rs:1145`) only attaches `func (h *Handler) signUp` (`auth.go`) to `type Handler struct` (`handler.go`) when both are in the **same file**. Cross-file methods in the same Go package keep `d.parent = None, d.receiver = Some("Handler")`, which `resolve::Index::new` (`resolve.rs:453`) never indexes under `Handler`. | **(A)** In `resolve::Index::new`, index top-level Go definitions with `d.receiver == Some(r)` into a per-package receiver map `(pkg_key, r, name) -> Vec<Sym>`, queried by `members()` (`resolve.rs:963`). Cost: $O(M)$ build over Go methods (mattermost: ~18k methods × ~32B ≈ 600 KB), $O(1)$ lookup.<br>**(B)** Move attachment to `rb-syntax`. Impossible: `rb-syntax` is per-blob. | **A** — fixes `hk_todo` (0/13 → 12/13; 13th is `ginSwagger.WrapHandler` call expression) and boosts Go method call recall in B2. |
| **B10** (0/26 `table_queries` on `hk_todo`) | `add_queries` (`crates/rb-indexer/src/sql.rs:341`) reads `facts.sql` (raw string literals with `${hole}` normalised by `rb-syntax`), so `fmt.Sprintf("SELECT … FROM %s", todoListsTable)` becomes `SELECT … FROM $1`, which `pg_query` rejects (table name cannot be `$1`). | In `sql.rs` (or `rb-syntax` + `fold`), when a SQL candidate comes from `fmt.Sprintf`/`String.format`/concatenation with identifier holes (`%s`), substitute holes that resolve to same-package/same-file string `const` bindings before `rb_sql::analyze`, capping certainty at `Synthetic` (or keeping `Exact` for same-file `const`). | Fold `const` string holes into `%s` positions before `rb_sql::analyze`; recovers 26/26 `table_queries` and 26/26 `table_ops` on `hk_todo`. |
| **B1** (6,108 of 6,147 SCIP disagreements on `mattermost`) | `scip::join` (`crates/rb-indexer/src/scip.rs:547`) runs per SCIP job (`lib.rs:995`) and checks `defined.contains(symbol)` against only **that job's** documents. Calls from `server/` into `server/public/` (a separate `go.mod` and `scip-go` job) are dropped as external! | **Two-pass SCIP join** across the repo's jobs: pass 1 runs document acceptance and `Anchors` per job, unioning all anchored global SCIP symbols into `repo_defined: BTreeSet<String>`; pass 2 runs reference/implementation emission with `repo_defined`. Memory: ~85k symbols × 64B ≈ **5.4 MB** on mattermost; wall-time delta **< 50 ms**. | **Two-pass join** — recovers ~7,700 exact cross-module `CALLS` in SCIP mode and eliminates 99.4% of mattermost's false SCIP disagreements. |

---

## 3. Low-Level Design (LLD Pseudocode)

### 3.1 Two-Pass Cross-Job SCIP Join (`crates/rb-indexer/src/scip.rs`, B1 + B13)
```rust
// Pass 1 (per job): accept documents, anchor definitions, collect global symbols
let mut prepared: Vec<PreparedJob> = jobs
    .iter()
    .map(|job| prepare_job(job, &content.facts, &keys))
    .collect();
let repo_defined: BTreeSet<&str> = prepared
    .iter()
    .flat_map(|p| p.defined.iter().copied())
    .collect();

// Pass 2 (per job): emit CALLS, REFERENCES, IMPLEMENTS against `repo_defined`
for p in &prepared {
    let joined = finish_job(p, &repo_defined);
    // B13 fix inside finish_job:
    // When `bypass` promotes `rel.symbol` to `type_symbol` (Go implicit interface),
    // `src` is `Source::Symbol(holder_symbol)`, never the method's `source_key`.
}
```

### 3.2 Package-Wide Go Method Attachment & SQL Constant Folding (`resolve.rs`, `sql.rs`, B9 + B10)
```rust
// In resolve::Index::new (crates/rb-indexer/src/resolve.rs):
// Map (package_key, receiver_type_name, method_name) -> Vec<Sym<'a>>
for (i, d) in facts.definitions.iter().enumerate() {
    if *language == Language::Go && d.parent.is_none()
        && let Some(recv) = d.receiver.as_deref()
        && let Some(pkg) = package_key(path, *language, facts)
    {
        go_methods.entry((pkg, recv, d.name.as_str())).or_default().push((path, i));
    }
}
// In Index::members(t, name, want):
// Lookup `self.named(c.0, Some(c.1), name, want)` PLUS, for Go `DefKind::Type`,
// `self.go_methods.get(&(pkg_of(c.0), self.def(c).name.as_str(), name))`.
```

---

## 4. Phased Sub-Milestone Breakdown (`karpathy-loops`)

Following M6/M7 discipline (**≤ 5 source files per phase**, failing test + mutant spec first, 31-target invariant + `rb-bench compare` gate at every close):

### **M9a — `exact` & `synthetic` Precision (`tasks/contract-m9a.md`)**
- **Scope:** B7 (test-file routes), B8 (Swagger annotations vs code routes), B13 (SCIP `IMPLEMENTS` from methods + unimplemented suite), B14 (Spring Cloud Config shared context-paths), B12a (`"{taskId}"` literal in `mapping_paths`), and B11 (document inherited-CRUD `synthetic` convention in `rb table-deps` output + contract).
- **Files (4):** `crates/rb-frameworks/src/routes.rs`, `crates/rb-indexer/src/routes.rs`, `crates/rb-indexer/src/scip.rs`, `crates/rb-indexer/src/lib.rs`.
- **Target Scoreboard Movement:**
  - `handkey/hk_todo/endpoints`: `fp` **2 → 0** (P: 95.1% → **100.0%**).
  - `microsecend/mse_piggymetrics/endpoints`: `tp` **0 → 6**, `fp` **6 → 0**, `fn` **6 → 0**.
  - `microsecend/mse_anilallewar` & `mse_rohitghatol`: `tp` **4 → 6**, `fn` **2 → 0** each.
  - Overall independent-key `exact` precision: **122/130 (93.8%) → 130/130 (100.0%)**.
  - `exact` sample: `routes` **58/60 → 60/60**, `scip` **57/60 → 60/60**.

### **M9b — Cross-Module SCIP & Handler/SQL/REST Recall (`tasks/contract-m9b.md`)**
- **Scope:** B1 (cross-Go-module SCIP join), B9 (Go cross-file receiver methods + gin method-value handlers), B10 (`fmt.Sprintf` SQL table constants), B12b (`@RepositoryRestResource` Spring Data REST endpoints).
- **Files (5):** `crates/rb-indexer/src/scip.rs`, `crates/rb-indexer/src/resolve.rs`, `crates/rb-indexer/src/sql.rs`, `crates/rb-frameworks/src/routes.rs`, `crates/rb-syntax/src/strings.rs`.
- **Target Scoreboard Movement:**
  - `scip/mattermost/calls`: disagreements drop by **6,108** (agreement precision **74.3% → ~99%**).
  - `handkey/hk_todo/handler_binding`: `tp` **0/13 → 12/13** (total `handler_binding` recall **66.7% → 97.4%**).
  - `handkey/hk_todo/table_queries` & `table_ops`: `tp` **0/26 → 26/26** (total `table_queries` recall **44.4% → 92.6%**).
  - `microsecend/mse_jferrater/endpoints`: `tp` **0/5 → 5/5** (microSecEnD endpoint recall **→ 100.0%**).

### **M9c — Tier B Call Recall on Go, TS, and Python (`tasks/contract-m9c.md`)**
- **Scope:** B2 (Go constructor return types `NewX() -> *X` and chained field/method receivers), B3 (TS `import * as ns from '@/...'` namespace calls behind `tsconfig.json` `paths`), B5 (Python syntactic subset: `super().m()`, `from m import *`, `raise Exc()`).
- **Files (4):** `crates/rb-indexer/src/resolve.rs`, `crates/rb-syntax/src/bindings.rs`, `crates/rb-syntax/src/queries.rs`, `crates/rb-indexer/src/rta.rs`.
- **Target Scoreboard Movement:**
  - `scip/bookcars/calls`: recover the **610** `@/` namespace-import misses (recall **62.5% → ~95%**).
  - `scip/go_clean_arch/calls`: recover the **12** constructor-assigned test calls (recall **75.5% → 100%**).
  - `pycg/calls` & `calls_file_scope`: lift recall on `imports`, `mro`, and `exceptions` categories.

- **Note (2026-10-16, `tasks/contract-m9c.md` amendment 2):** the scip-go image fix moved from M9d into M9c, as phase PI before P1. The image (Go 1.27.2, scip-go 0.2.7 on x/tools v0.45.0) loaded no imported package, because Go 1.27.2 writes export data V5 and x/tools v0.45.0 reads only up to V4. It is rebuilt on x/tools v0.50.0 with a build-time self-test. M9c's D7 is withdrawn, and D8 becomes a `synthetic` symbol join. M9d keeps B4 and the `go.work` / version-skew question.

### **M9d (Optional / Infra) — SCIP Container Fetch Hardening (B4)**
- **Scope:** B4 (`spring_boot_microservices`, `fastapi_realworld`, `kotlin_spring_realworld`, `nestjs_realworld` fetch/index failures). Audited per target; only recipes that fit the 6 GiB disk floor and 20-min warm budget land.

---

## 5. Self-Review (`engineer`)

- **Can any state be removed?** Yes: neither the cross-job SCIP join (B1) nor Go cross-file method resolution (B9) adds a new node/edge kind or changes `FileFacts`; both are pure index-time tables inside `rb-indexer`.
- **What is the biggest trap?** Re-scoring `scip/mattermost` after B1 changes the SCIP reference (`gold` rises by ~7,700 edges). By M8a D2, `rb-bench compare` will flag `gold` rising unless `m8b.json` is compared against the new scoreboard with an `--accept` entry classifying the B1 reference expansion.
- **Ready to execute:** When you're ready to start building **M9a**, switch out of Plan mode (to the build agent) and we will create branch `m9a` off `m8`, write `tasks/contract-m9a.md`, and start Phase 1 test-first.
