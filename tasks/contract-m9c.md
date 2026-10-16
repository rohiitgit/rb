# M9c contract: Tier B call recall (Go, TS, Python), scip-go cross-module joins, unverified scip-go relationships (2026-10-16)

**Status:** contract written 2026-10-16 from `tasks/plan-m9.md` §4 "M9c", `tasks/bench-findings.md` B2, B3, B5, B13b, and M9b's hand-over: B13b with P0's record, and B1 because M9b's D4 failed (`tasks/contract-m9b.md` amendment 1, `var/m9b/probe-scip.txt`). Not yet built. Commits go on branch `m9`, after M9b closes.

**Evidence base.** File:line references are at `5a310aa` (M9b P1, B9 built). Only `crates/rb-indexer/src/resolve.rs` differs from `d445940` among the files cited. M9b P2 and P3 were in progress in the same tree when this was written, so P0 re-reads `resolve.rs` and `rb-indexer/src/lib.rs` at M9b's close commit before any edit. Counts come from the M9a run of record `var/bench/out/2e36cd607eab/` (dumps, before B9), the raw scip-go indexes in `var/m9b/idx/`, and read-only scans of the pinned repositories. The planner's scripts are in the session scratchpad (`agree.py`, `ts_classify.py`, `go_forms.py`, `b13b_rule.py`). P0 commits their equivalents and re-runs them on the M9b run of record, so every number below is re-based on M9b before code.

## Why

| Finding | What is missing | Measured (M9a run of record) |
|---|---|---|
| B2 | Go calls through receivers whose type Tier B cannot name | `scip:mattermost:noscip:calls` fn 17,157; go_clean_arch fn 12; go_gin_example fn 9 |
| B3 | TS calls into modules imported through tsconfig `paths` | `scip:bookcars:noscip:calls` fn 694 |
| B5 | Python `super()`, star imports, re-exports, `raise X`, inherited `__init__` | PyCG `calls` 12/48, `calls_file_scope` 94/186 |
| B1 | SCIP mode drops `server/` → `server/public/` calls | 6,108 of 6,147 agreement fp on mattermost |
| B13b | scip-go relates types to interfaces they do not implement | 277 `*Suite#` `IMPLEMENTS` on mattermost |

## Root causes, verified (file:line at `5a310aa`)

- **B3 is path-alias parsing, not namespace imports.** `AliasMaps::build` parses each `tsconfig.json` with strict `serde_json` (`rta.rs:236`) and skips the file on error. bookcars `admin/tsconfig.json` and `frontend/tsconfig.json` carry `/* */` comments and trailing commas, so their `"@/*": ["./src/*"]` is never read and `Modules::ecmascript` (`modules.rs:105`) resolves no `@/` specifier. `mobile/tsconfig.json` is strict JSON: the same namespace form hits there. bookcars hits by form: 245 `ns.f()` through `@/` in mobile, 331 through relative paths. So `import * as ns` member calls already work (`resolve.rs`, `first_segment` alias arm).
  - Misses by form (694): `ns.f()` with an `@/` namespace import 609 (admin 370, frontend 239); a bare call to a name imported from `@/` 36 (34 named, 2 default); a bare call to a **default import** through a relative path 39; other 10 (a `state.f()` field, a `ref.current?.x()` chain, one call in a JSX prop).
  - Default imports never resolve: `imported` drops names whose `name` is `default` (`resolve.rs:933`), though the target's `FileFacts::default_export` names the definition (`rb-syntax/src/lib.rs:483`).
  - `strip_json_comments` exists (`rb-scip/src/jobs.rs:836`) and `TsScopes` and `exclude::out_dir` use it, but none strips trailing commas. On the bench, 6 `tsconfig.json` fail comment-stripped strict JSON: bookcars admin, frontend, `packages/reactjs-social-login`; mattermost `webapp/platform/{shared,client}`, `webapp/channels`. `webapp/channels` maps `mattermost-redux/*`, imported 4,029 times in `channels/src`.
- **B2, forms.** A classifier over all 17,065 Go misses on mattermost (92 of 17,157 are TS/JS) flags the forms between call and callee. Several flags can hold for one miss.
  - The callee is a method declared outside its receiver type's file in 8,687 (B9, built in M9b). If B9 resolves every miss whose only blocker it is, 4,483 leave. Prediction, since M9b is not yet measured.
  - **Local typed by a call result** (`th := Setup(t)`, `u := article.NewService(…)`): 4,657. Go definitions record no result type (`decorations.rs:642`; test `go_and_python_have_neither`, `decorations.rs:1452`), and `short_var_declaration` types a name only from a composite literal (`bindings.rs:1465-1495`, test row `q := NewThing()` → `q: ?`).
  - **Call segment in the qualifier** (`th.App.Srv().Store()`, `s.GetReplica().Get`): 2,991. `container` splits the qualifier on `.` and `step` knows only fields and nested types (`resolve.rs:1231`), so `Srv()` stops the walk.
  - **Type alias** (`type Context = web.Context`, `api4/handlers.go:17`, the type of every api4 handler's `c`): 3,307. tree-sitter-go 0.25 parses `type A = B` as `type_alias`; its `tags.scm` matches only `type_spec`, and `queries.rs` adds no Go supplement, so the alias has no definition (0 `handlers.go`/`Context#` symbols in the dump).
  - **Embedded struct** (`SqlXStore { *SqlStore }`, `BatchMigrationWorker { *BatchWorker }`, gin's `ArticlePosterBg { *ArticlePoster }`): 1,313. An unnamed `field_declaration` pushes no binding (`bindings.rs:1439-1462`, `declared` is empty) and Go has no supertypes (`decorations.rs:53`).
  - **Package-level var of a sibling file** (`mainHelper`, declared in `main_test.go`): 894. `binding` reads only the file's own locals (`resolve.rs:1054`).
  - Upper bounds per form (misses whose every non-B9 flag is in the set): return types 4,023; call segments 2,171; aliases 3,127; embedding 1,313; package vars 109 (889 with the 780 whose type the script could not place); all five 11,562. These are classifier counts, not resolution runs.
  - A seeded sample of 40 rows (seed 9) read by hand matched its flags; the M9a finding's `Auditable` example is a concrete method on a struct, not an interface call.
  - go_clean_arch, all 12: `u := pkg.NewX(…)` then `u.M()` in `_test` packages. go_gin_example, 9: 4 `var ( appG = app.Gin{C: c} )` (a `var_spec` without a type is not typed from its literal, `bindings.rs:1439-1446`), 2 constructor results, 3 through the embedded `*ArticlePoster`.
- **Single-implementer interfaces are not a recall form here.** 0 of 17,065 Go misses target an interface method: every callee is a method (17,027) or a function (3) declaration, 35 are not functions. Interface `method_elem`s have no definition (no tag pattern), so scip-go's reference to `I#M.` is unanchored and the SCIP reference holds no such call. A Tier B edge to a sole implementer would score as an agreement fp.
- **B5.** The PyCG cases, by the form that misses:
  - `super().m()`: the qualifier is the text `super()` (`rb-syntax/src/lib.rs:1107-1124`); `first_segment` matches only `super` (`resolve.rs:1149`). Case `mro/super_call` (2 fn).
  - Star imports: `from m import *` is an `Import` with `wildcard` and no names (`imports.rs:276-282`); `imported` reads only names for Python. Cases `imports/import_all`, `submodule_import_all` (2 fn each).
  - Re-exports: `from pkg import f`, where `pkg/__init__.py` only imports `f`, finds no top-level definition there. Cases `init_func_import` (1); `init_import` (1) also needs the imported name `smth = Smth()` typed from its module-level binding.
  - `raise A` and `raise A.B` are not calls: the bare class is raised and Python instantiates it. No query captures a `raise_statement`. Cases `exceptions/raise`, `raise_attr` (1 each). `raise_assigned` (`a = A; raise a`) is data flow and stays out.
  - Inherited constructors: `constructor` looks only in the class itself (`resolve.rs:1305-1318`). `B()` with `__init__` only on `A` gives a `REFERENCES` to `B`. Cases `mro/basic_init`, `parents_same_superclass`, `two_parents` (1 each).
- **B1 (M9b amendment 1).** scip-go emits, for packages an index does not define, only namespace occurrences: the import path and the qualifier `model` in `model.NewId`. Member occurrences: 0 in every index on the bench, except 4 in server/public (an ldap function). A Tree-sitter reference at `NewId` finds no occurrence at its span and is counted `references_unmatched` (`scip.rs:515-526`). The occurrence at the qualifier names `` scip-go gomod github.com/mattermost/mattermost/server/public v0.4.3 `…/server/public/model`/ `` (104,594 such occurrences; all name a package server/public defines). Of the 6,147 agreement fp, 5,554 are `pkg.f()` calls through a server/public import, 548 are methods on server/public values, 34 are bare calls in `docs/site` scripts, 11 other.
- **B13b (M9b amendment 1, D8).** Under the Go bypass (`scip.rs:653-668`) a type's `is_implementation` relationship to an interface is accepted with no check. Measured against the raw server and server/public indexes, with a rule that requires every member of the interface (with members of interfaces it embeds) to be a method the type declares or a method of an index type it embeds by field name: 431 of the 729 dump rows verify, 291 do not (276 from `*Suite#` types, 15 others such as `HistogramVec# → AutocompleteDynamicArgProvider#`), 7 come from relationships outside those two indexes. The judged sample agrees: both TRUE scip-go type rows verify, the FALSE `ElasticsearchInterfaceTestSuite#` row does not. go_clean_arch (5 rows) and microservices_demo (3) all verify.

## Plan corrections

1. **B3's cause** is strict JSON parsing of tsconfig, not namespace-import member calls, which work. The plan's file list (`bindings.rs`, `queries.rs`, `rta.rs`) misses `rb-scip/src/jobs.rs`. "Recover 610, recall ~95 %" holds only with default imports too: at most 645 + 41 of 694.
2. **B2's forms.** Interfaces with one implementer recover 0 misses (above). The measured forms are call results, call segments, type aliases, embedded structs and sibling-file package vars, none in the plan. "go_clean_arch 75.5 % → 100 %" is right in kind (constructor results).
3. **B5's exceptions** are not constructor calls inside `raise` (those are calls today): they are bare classes raised. `raise_assigned` is data flow and stays a miss. Inherited `__init__` is a fourth form the plan omits (3 cases).
4. **Scope.** The plan omits B1 (moved by M9b's D4), B13b (handed over), `scip.rs`, `rb-indexer/src/lib.rs`, `rb-store/src/query.rs`, and the rb-syntax files `lib.rs`, `decorations.rs`, `keys.rs`. `rta.rs` changes only in `AliasMaps::build`.
5. **Mutant ids.** M9b amendment 2 took m285, and M9b may add more while it builds. M9c's ids start at **m300**.

## Design

```mermaid
flowchart LR
    TS[tsconfig text] -->|parse_jsonc| AM[AliasMaps] --> MOD[Modules] --> IX
    F[FileFacts + Go results, aliases,<br/>embedded fields, raise refs] --> IX[resolve::Index]
    IX -->|return types, call segments,<br/>aliases, embedding, package vars| GO[Go calls]
    IX -->|super, star, re-exports,<br/>inherited __init__, module vars| PY[Python calls]
    IX -->|default imports| JS[TS calls]
    GO & PY & JS --> TB[Tier B edges, heuristic] --> G[(graph)]
    IX --> RH[routes, DI via call and resolve_type]
    SJ[scip::join per job] -->|unmatched pkg.Name refs| XP[cross-project post-pass] -->|scip, synthetic| G
    SJ -->|verified type relationships| G
    G --> D[dumps] --> SB[score] --> CMP{compare vs M9b record}
```

**State, derived state, invariants.**
- **State.** `FileFacts` gains: Go `Definition::return_type` (result as written); `Definition::alias_of` (Go `type A = B` target); `Binding::embedded`; Go `var_spec` literal types and call initialisers on the first of two names; Python `raise` references. Each new field has `#[serde(default)]`. The extractor fingerprint changes by itself (`rb-syntax/src/lib.rs:30-45`).
- **Derived.** In `resolve::Index`: return types of callables, resolved once per definition; a Go package-var table `(package_key, name) → binding`. In the SCIP stage: each job's unmatched package-qualified references, joined after the job loop.
- **Invariants.**
  - Tier B stays `heuristic` (`ambiguous` for several candidates). No new edge is `exact`.
  - Embedding never makes `IMPLEMENTS`, never enters `supertypes()`, and never feeds RTA.
  - A form applies only to its language (Go, Python or TS as stated).
  - Every resolution through a new form is counted in a per-file `tier_b_forms` diagnostic.
  - A B1 edge has provenance `scip` and certainty `synthetic` unless the symbol strings are equal.
  - A scip-go type relationship becomes `IMPLEMENTS` only if verified. Every dropped one is counted.

**Trade-offs, with this system's numbers.**
- **One JSONC reader, used where.** Switching `TsScopes` and `exclude::out_dir` too would move the SCIP-mode partition and the reference on bookcars and mattermost, a separate gate. **Chosen:** `AliasMaps` only. The 6 tsconfigs are recorded for the other two readers.
- **Go forms by count.** All five forms cover at most 11,562 of 17,065 misses; return types and call segments alone cover 6,828. Each is syntactic and typed by declarations. **Chosen:** all five. They share one mechanism (a type for a name), and each has its own test and mutants.
- **Embedding as supertypes or as bindings.** Supertypes feed `implements` (`resolve.rs:1322`) and RTA, so embedding there would emit false `IMPLEMENTS`. An embedded member binding named by its type (Go's implicit field name) also serves the explicit form `a.ArticlePoster.X`. **Chosen:** bindings with `embedded: true`.
- **Single implementer.** Not in M9c: 0 measured misses, agreement fp by construction, and Go has no Tier B `IMPLEMENTS` to count implementers. Query-time dispatch (`--precision cha|rta`) stays the place for it.
- **B1: one scip-go run under `go.work`, or a qualifier join.** The `go.work` run changes container jobs and is unmeasurable without them (infra, M9d). The qualifier join recovers the 5,554 `pkg.f()` fp and leaves the 548 value methods, which no occurrence names. **Chosen:** the qualifier join, scip-go only.
- **B13b: drop or downgrade.** A downgraded `IMPLEMENTS` that is false 276 times out of 291 is noise. **Chosen:** drop and count, as M9a D3 does for method sources. Cost: true implementers through dependency embeds (unmeasured) lose their edge.

**NFRs.**
- mattermost `noscip-fresh` wall ≤ 1.25× and peak RSS ≤ 1.15× the M9b run of record. A warning is re-measured as an A/B of the M9b and M9c binaries, alternating under the same load, until it is classified as noise (CPU time within 1.1×, medians within the limit) or as a regression, which blocks the close. Every re-measure is logged.
- Scoring ≤ 30 s. Two scorings `cmp`-identical. Warm full bench ≤ 20 min.
- Return types are resolved once per definition (memoised). Embedded and alias walks are bounded by `MAX_TYPE_DEPTH` and the `resolving` guard (`resolve.rs`, `resolve_type`).

## Decisions

- **D1 (B3, JSONC).** `rb_scip::jobs::parse_jsonc(text) -> Option<serde_json::Value>`: `strip_json_comments`, then remove each `,` outside strings that is followed only by whitespace before `}` or `]`. `AliasMaps::build` uses it. `TsScopes` and `out_dir` do not change.
- **D2 (B3, default imports).** For TS/JS, an imported name `default` with local `x` resolves to the top-level definition the target file's `default_export` names. A target without `default_export` gives nothing, as today.
- **D3 (B2, facts).**
  - Go `function_declaration` and `method_declaration` record `return_type` as the `result` text.
  - `type_alias` is a `DefKind::Type` definition with `alias_of` = its type as written.
  - An unnamed `field_declaration` pushes a member binding named by the type's last segment, `*` stripped, with that type and `embedded: true`.
  - A `var_spec` without a type takes its literal's type under the `short_var_declaration` rule.
  - In `a, b := f(…)` with exactly one call on the right, `a` gets `init: Call`; `b` gets none.
- **D4 (B2, resolution, Go only).**
  - **Call result.** A binding with no type and `init: Call` takes the result type of the callables the reference at the chain's last link resolves to. A result is usable when it is one named type (`*` stripped) or a two-element list whose second element is `error`. Several types make several candidates; none means no type.
  - **Call segment.** The qualifier is split on `.` at bracket depth 0. A segment `f(…)` steps to the result types of `f` among the current types' members, a module's top-level callables, or, at the head, the callables of the levels. A segment with brackets that is not a call stops the walk.
  - **Alias.** `resolve_type` replaces a definition carrying `alias_of` with the resolution of its target in the alias's file.
  - **Embedding.** When a type and its B9 receiver methods have no member of the name, the embedded member bindings are searched breadth-first, shallowest depth first. Several hits at one depth are `ambiguous`. `step` searches fields the same way.
  - **Package var.** A name with no local binding resolves through `(package_key, name)` over Go top-level non-member bindings with a type. Exactly one declaration of the name in the package (any kind) is required. A non-test file never sees a test file's var.
- **D5 (B5, Python only).**
  - **`super()`.** Exactly `super()` resolves as `super` does: the supertypes of the enclosing class.
  - **Star imports.** `from m import *` is consulted after own definitions and named imports. It brings every top-level name of the target not starting with `_`. A target that defines `__all__` at top level is skipped.
  - **Re-exports.** A name not defined at top level of an import's target follows that target's own named and star imports, with depth ≤ 8 and a cycle guard.
  - **Module variable.** An imported name that is a top-level binding with a type in the target (`smth = Smth()`) types a qualifier, if it is the only such binding.
  - **`raise`.** A query supplement adds `(raise_statement . [(identifier) @name (attribute attribute: (identifier) @name)]) @reference.call`. The `.` anchor keeps the `from` cause out.
  - **Inherited `__init__`.** `constructor` uses `members(class, "__init__")`. Java, TS, Go, Kotlin and C# are unchanged.
- **D6 (forms counted).** One `tier_b_forms` diagnostic per file that resolved a reference through a new form, with keys `return_type`, `call_segment`, `type_alias`, `embedded`, `package_var`, `default_import`, `super_call`, `star_import`, `reexport`, `module_variable`, `inherited_init`. A reference counts each form it used once.
- **D7 (B13b).** For an index whose tool is `scip-go`, a bypass relationship from a type is accepted only when the interface's members, plus those of index interfaces it embeds, are all methods the type declares or methods of index structs or interfaces named by its fields, transitively. This reads the job's `SymbolInfo` (`Owner#Name` symbols, kinds by name). Anything else is dropped and counted in a new `relationships_unverified` counter beside `relationships_dropped`. Other indexers are unchanged.
- **D8 (B1).**
  - **Hand-back.** In a scip-go job, an unmatched `Call`, `Class` or `Type` reference whose qualifier is one identifier ending at the name's `.` on the same line is handed back, with source, kind and evidence. Only when the occurrence at the qualifier's span is a non-definition package symbol of a package the job does not define.
  - **Join, after the job loop.** The package's manager and module path must equal another ok scip-go job's `go.mod` module (`Content::go_modules`, `lib.rs:406`). That job must anchor exactly one of `<pkg>/Name().`, `<pkg>/Name#`, `<pkg>/Name.` with its own version. Then an edge with provenance `scip`, `exact` if the version strings are equal, else `synthetic`. Two candidate jobs: none, counted `ambiguous`.
  - **Report.** One `scip_cross_project` diagnostic per (from root, to root) pair: `{exact, synthetic, ambiguous, unanchored}`. `references_unmatched` keeps its meaning.
  - **Status.** `rb status`'s `exact calls (scip)` counts certainty `exact` only (`rb-store/src/query.rs:559`).
- **D9 (gate).** Base: the M9b run of record and `bench/scoreboard/m9b.json`, re-scored with HEAD's rb-bench. No key changes. Accept lines go in `bench/accept-m9c.tsv`, each naming its claim and diff.

## Claims

| # | Claim | Test |
|---|---|---|
| C1 | P0 record: forms, B1 and B13b counts re-run on the M9b run of record; base dumps exist; re-score `cmp`-identical | `var/m9c/probe-forms.txt`, `bench/probe/m9c_forms.py` |
| C2 | D1: the admin tsconfig text (comments, trailing commas, a `",}"` inside a string) resolves `@/*`; strict text unchanged; broken text resolves nothing | unit `tsconfig_jsonc_resolves_aliases` (rta), `namespace_calls_behind_jsonc_paths_resolve` (resolve) |
| C3 | D2: `import f from './m'` with `export default f`, `export default function g`; no default export gives no edge | unit `default_imports_resolve_to_the_default_export` (resolve) |
| C4 | D3 facts, one row per bullet; `go_and_python_have_neither` updated for Go and disclosed | unit `go_results_aliases_and_embedded_fields_are_facts` (rb-syntax lib), `go_var_specs_and_call_pairs_bind` (bindings) |
| C5 | D4 per form: call result incl. `(T, error)` and a 3-result refusal; `a.B().C()` with args holding dots; alias chain and `A = B; B = A` giving none; embedding at two depths, a same-depth tie, no `IMPLEMENTS`; package var with a local shadow, two declarations, test → non-test refused | unit `go_call_results_and_segments_type_receivers`, `go_type_aliases_resolve_to_their_target`, `go_embedded_types_promote_members`, `go_package_vars_of_sibling_files_type_names` (resolve) |
| C6 | D5 per form, the PyCG shapes plus refusals: `_x` and `__all__` targets, a re-export cycle, `raise A from e` (no edge to `e`), a Java subclass without a constructor (unchanged) | unit `python_super_star_reexport_and_module_vars_resolve`, `python_raise_and_inherited_init_construct` (resolve), `python_raise_is_a_call_reference` (rb-syntax lib) |
| C7 | D6 counts each form once per reference | unit `tier_b_forms_count_each_form` (resolve) |
| C8 | D7: own methods verify; an index embed verifies; an embedded interface verifies; a dependency embed is dropped and counted; a scip-typescript relation is unchanged | unit `scip_go_type_relationships_need_the_method_set` (scip) |
| C9 | D8: two jobs, `v0.4.3` vs `.` → `synthetic`; equal versions → `exact`; wrong module path → none; two jobs → `ambiguous`; qualifier not adjacent → none; the diagnostic counts each | unit `package_qualified_references_join_another_project` (lib) |
| C10 | `exact calls (scip)` excludes non-exact `scip` rows | rb-store test `scip_status_counts_exact_certainty_only` (ScratchDb) |
| C11 | `PIPELINE` M9b's last → `m9c-1` before the verify build; binary of record from a clean `crates/` | Result: HEAD, sha256 |
| C12 | Invariant over all target and PyCG dumps against the M9b record, by row identity, byte counts non-zero: every differing row in a bucket (B1, B13b, B3 JSONC scope, B3 default import, B2 by `tier_b_forms`, B5, RTA flags, routes/DI through `call`, DIAG); 0 unclassified | `var/m9c/logs/invariant.txt` |
| C13 | `compare var/m9c/m9b-rescored.json bench/scoreboard/m9c.json --accept bench/accept-m9c.tsv --perf` exits 0; each `changed` and `accepted` line classified; no `exact` fp rise | Result, pasted output |
| C14 | NFRs | `compare --perf`, A/B logs |
| C15 | Mutants m300–m338 all KILLED, none STALE or INVALID; `m8b`, `m9a`, `m9b` batteries re-run KILLED | battery logs |

**Predicted movement.** Base is the M9b run of record. Counts are from the M9a dumps; P0 re-bases them on M9b in amendment 1. The close reports every changed cell measured beside these.

| Cell (`bench:target:mode:class`) | M9a | Predicted m9c | Claim |
|---|---|---|---|
| `scip:bookcars:noscip:calls` | tp 1,155, fn 694 | tp +600 to +686 heuristic; fn 8 to 94; fp possible, each classified | C2, C3 |
| `scip:bookcars:noscip:calls_file_scope` | fn 7 | fn 5 (2 `helper.f()` in admin, frontend) | C2 |
| `scip:go_clean_arch:noscip:calls` | tp 37, fn 12 | tp 49, fn 0 | C5 |
| `scip:go_gin_example:noscip:calls` | tp 152, fn 9 | tp 161, fn 0 (4 var literal, 2 results, 3 embedding) | C4, C5 |
| `scip:mattermost:noscip:calls`, B2 | fn 17,157 (M9b moves it) | fn falls; upper bound 11,562 of the M9a misses; fp heuristic rises where new edges reach server/public value methods the reference lacks (accept line, B1 residue) | C5 |
| `scip:mattermost:noscip:calls`, B1 | gold 33,312, fp 6,147 | gold +X (X ≥ the joined pairs), up to 5,554 fp → tp, fn + (X − joined tp) | C9 |
| `scip:mattermost:noscip:calls` | `found_out_of_scope{not_covered}` 21,644 | rises: webapp TS through `mattermost-redux/*` and default imports, Go in uncovered files (accept line) | C2, C3, C5 |
| `scip:microservices_demo:noscip:calls` | fn 27 | all 27 are cross-file methods: M9b's cell, unchanged by M9c | |
| `scip:express_ts_boilerplate:noscip:calls` | fn 24 | unchanged (14 `this.log` parameter properties, 3 `export *` barrels, 7 other) | |
| `pycg:*:calls` | tp 12 / 48 | tp 14 (`mro/super_call` +2) | C6 |
| `pycg:*:calls_file_scope` | tp 94 / 186 | tp 105: `import_all` +2, `submodule_import_all` +2, `init_func_import` +1, `init_import` +1, `raise` +1, `raise_attr` +1, `basic_init` +1, `parents_same_superclass` +1, `two_parents` +1 | C6 |
| `exact` sample, `scip` family | | the FALSE `ElasticsearchInterfaceTestSuite#` row leaves the pool; 291 mattermost `IMPLEMENTS` leave; redraw only if `rb-bench sample` lists drawn edges without a verdict | C8 |
| all other cells | | identical | |

## Phases

Rules for every phase: the failing test is written and seen failing first; one file, one edit per message; no `git checkout --`; `cargo build -p rb-cli` before any e2e; no `cargo` while a container suite runs; every bless is followed by a row-by-row `git diff` of the golden; before changing behaviour a test asserts, grep the tests for the old value (lesson of 2026-10-13).

**P0. Probes and base.**
- **Files:** `bench/probe/m9c_forms.py` (new, `python3 -I`, read-only: agreement misses by location identity, TS and Go form flags, the B1 fp form split, the D7 rule over `var/m9b/idx/`).
- **Action:** `du -sh target`, `df -h .`. Check that every dump of the M9b record exists and is non-empty. Re-score it with HEAD's rb-bench. Run the probe on the M9b and M9a records. Re-read `resolve.rs` and `rb-indexer/src/lib.rs` at M9b's close.
- **Verify:** re-score `cmp`-identical to `m9b.json`. The probe reproduces this contract's M9a counts (bookcars fn 694 = 609 + 36 + 39 + 10; mattermost B1 split 5,554 / 548; D7 431 / 291 / 7) before it prints M9b's.
- **Done:** amendment 1 re-bases the predictions.

**P1. B3.**
- **Files:** `crates/rb-scip/src/jobs.rs`, `crates/rb-indexer/src/rta.rs`, `crates/rb-indexer/src/resolve.rs`.
- **Action:** C2 and C3 tests first, then D1 and D2.
- **Verify:** `cargo test -p rb-scip --lib jobs`, `cargo test -p rb-indexer --lib rta` and `resolve`. Probe: index bookcars `--no-scip` as `t_m9c_bc`; `admin/src/pages/ChangePassword.tsx:67` `helper.info` → `admin/src/utils/helper.ts:23`. Delete the repo. Mutants m300–m303.

**P2. Facts.**
- **Files:** `crates/rb-syntax/src/{lib.rs, decorations.rs, bindings.rs, queries.rs, keys.rs}`.
- **Action:** C4 and C6's rb-syntax test first, then D3 and D5's query.
- **Verify:** `cargo test -p rb-syntax`. Each existing test that changes is listed in the commit with its reason. Mutants m304–m309.

**P3. Go resolution.**
- **Files:** `crates/rb-indexer/src/resolve.rs`.
- **Action:** C5 and C7 tests first, then D4 and D6.
- **Verify:** `cargo test -p rb-indexer --lib resolve` and `routes` (gin binding still passes). Probes on go_clean_arch and go_gin_example `--no-scip`: the 21 predicted edges appear. Mutants m310–m322.

**P4. Python resolution.**
- **Files:** `crates/rb-indexer/src/resolve.rs`.
- **Action:** C6 resolve tests first, then D5.
- **Verify:** `cargo test -p rb-indexer --lib resolve`. Probe: the 9 PyCG cases above with `target/debug/rb`. Mutants m323–m330.

**P5. B13b and B1.**
- **Files:** `crates/rb-indexer/src/scip.rs`, `crates/rb-indexer/src/lib.rs`, `crates/rb-store/src/query.rs`, scip-status goldens if any change.
- **Action:** C8, C9, C10 tests first, then D7 and D8.
- **Verify:** `cargo test -p rb-indexer --lib scip` and `lib`; the rb-store test on a `ScratchDb`; the SCIP e2e suites: goldens unchanged or each changed row classified. Mutants m331–m338.

**P6. Close the code.**
- **Files:** `crates/rb-indexer/src/lib.rs` (`PIPELINE`), `bench/mutants/m9c.json`, `README.md`, `docs/usage-and-testing.md`, `docs/repository-brain-design.md`.
- **Action:** `PIPELINE` `m9c-1`. Design notes for D4, D5, D7 and D8's `synthetic` join. Usage guide for `tier_b_forms`, `relationships_unverified`, `scip_cross_project`.
- **Verify:** `cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && echo CLEAN`. The full real-Docker run of usage guide §5.2, log kept: 0 failed, `grep -c 'skipping:'` = 0. `SELECT name, checkout FROM rb.repo` identical before and after. All four batteries KILLED, re-run after the last fmt.
- **Done:** committed on `m9`.

**P7. Bench.** Build `var/m9c/bin/rb` from the committed tree (refuse a dirty `crates/` or `migrations/`; print HEAD and sha256). `nohup bench/run.sh var/m9c/bin/rb`. Invariant C12. `rb-bench sample`. Score twice, `cmp`. Write `bench/scoreboard/m9c.json` and `m9c-perf.json`. Gate C13, C14.

**P8. Records and evaluators.** `CHANGELOG.md`, `IMPLEMENT.md`, this Result, `tasks/todo.md`, `tasks/bench-findings.md` (B1, B2, B3, B5, B13b with measured numbers), `tasks/lessons.md`, README status line, usage guide, design doc milestone row. Evaluator pass 1 (full, hostile), then pass 2 (verification only).

## Mutants (`bench/mutants/m9c.json`, from m300)

Each spec mutates one line that survives a reflow and compiles, and names its `test` filter. A kill counts only from a failing test that matches the filter, in a compiling tree.

| Id | Crate | Mutation | `test` filter |
|---|---|---|---|
| m300 | rb-indexer | `AliasMaps::build` parses raw text | `tsconfig_jsonc_resolves_aliases` |
| m301 | rb-scip | trailing-comma strip ignores strings | `tsconfig_jsonc_resolves_aliases` |
| m302 | rb-indexer | `default` import names skipped again | `default_imports_resolve_to_the_default_export` |
| m303 | rb-indexer | default import resolved by its local name | `default_imports_resolve_to_the_default_export` |
| m304 | rb-syntax | Go `result` not recorded | `go_results_aliases_and_embedded_fields_are_facts` |
| m305 | rb-syntax | `type_alias` gets no `alias_of` | `go_results_aliases_and_embedded_fields_are_facts` |
| m306 | rb-syntax | unnamed field pushes no binding | `go_results_aliases_and_embedded_fields_are_facts` |
| m307 | rb-syntax | untyped `var_spec` literal not typed | `go_var_specs_and_call_pairs_bind` |
| m308 | rb-syntax | second name of a call pair gets the init | `go_var_specs_and_call_pairs_bind` |
| m309 | rb-syntax | raise query without the `.` anchor | `python_raise_is_a_call_reference` |
| m310 | rb-indexer | call-initialised bindings stay untyped | `go_call_results_and_segments_type_receivers` |
| m311 | rb-indexer | `(T, U)` with `U` not `error` accepted | `go_call_results_and_segments_type_receivers` |
| m312 | rb-indexer | call segment stops the walk | `go_call_results_and_segments_type_receivers` |
| m313 | rb-indexer | qualifier split ignores brackets | `go_call_results_and_segments_type_receivers` |
| m314 | rb-indexer | `alias_of` not followed | `go_type_aliases_resolve_to_their_target` |
| m315 | rb-indexer | embedded bindings not searched | `go_embedded_types_promote_members` |
| m316 | rb-indexer | deeper embed wins over shallower | `go_embedded_types_promote_members` |
| m317 | rb-indexer | same-depth tie keeps the first | `go_embedded_types_promote_members` |
| m318 | rb-indexer | embedded types added to `supertypes` | `go_embedded_types_promote_members` |
| m319 | rb-indexer | package var table not consulted | `go_package_vars_of_sibling_files_type_names` |
| m320 | rb-indexer | test-file var visible to non-test files | `go_package_vars_of_sibling_files_type_names` |
| m321 | rb-indexer | two declarations: first wins | `go_package_vars_of_sibling_files_type_names` |
| m322 | rb-indexer | `tier_b_forms` skips `call_segment` | `tier_b_forms_count_each_form` |
| m323 | rb-indexer | `super()` arm removed | `python_super_star_reexport_and_module_vars_resolve` |
| m324 | rb-indexer | `super()` resolves to the enclosing class | `python_super_star_reexport_and_module_vars_resolve` |
| m325 | rb-indexer | star import ignored | `python_super_star_reexport_and_module_vars_resolve` |
| m326 | rb-indexer | `_` names exported by a star import | `python_super_star_reexport_and_module_vars_resolve` |
| m327 | rb-indexer | `__all__` targets not skipped | `python_super_star_reexport_and_module_vars_resolve` |
| m328 | rb-indexer | re-exports not followed | `python_super_star_reexport_and_module_vars_resolve` |
| m329 | rb-indexer | module variables not typed | `python_super_star_reexport_and_module_vars_resolve` |
| m330 | rb-indexer | `constructor` looks only in the class | `python_raise_and_inherited_init_construct` |
| m331 | rb-indexer | D7 check skipped | `scip_go_type_relationships_need_the_method_set` |
| m332 | rb-indexer | index embeds not in the method set | `scip_go_type_relationships_need_the_method_set` |
| m333 | rb-indexer | D7 applied to scip-typescript | `scip_go_type_relationships_need_the_method_set` |
| m334 | rb-indexer | hand-back ignored | `package_qualified_references_join_another_project` |
| m335 | rb-indexer | module-path check removed | `package_qualified_references_join_another_project` |
| m336 | rb-indexer | cross-version join labelled `Exact` | `package_qualified_references_join_another_project` |
| m337 | rb-indexer | two anchoring jobs: first wins | `package_qualified_references_join_another_project` |
| m338 | rb-store | `exact calls` counts every `scip` row | `scip_status_counts_exact_certainty_only` (ScratchDb) |

No spec's filter may select a wall-clock test (M9b N2 rule).

## Verification (closing bar)

- Tests first. Mutants m300–m338 KILLED; the m8b, m9a and m9b batteries re-run KILLED.
- fmt and clippy clean. Real-Docker suites: 0 failed, 0 skipped, full log kept. `rb.repo` unchanged.
- `PIPELINE` `m9c-1`; the binary of record is built from the last code change (C11).
- Invariant C12: every differing row classified, 0 unclassified.
- `compare` against the re-scored M9b record: exit 0, every accepted cell classified, no `exact` fp rise.
- Every changed cell reported measured beside predicted. Two scorings `cmp`-identical.
- Perf per the NFR rule, every re-measure logged and disclosed.
- Records complete. Evaluator pass 1, then pass 2 (verification only).

## Out of scope

- **M9d:** B4 (SCIP fetch and index failures). A single scip-go run under a generated `go.work` (B1 option a), with the open question of whether scip-go 0.2.7 ever emits dependency member occurrences.
- **Recorded, not fixed:**
  - Single-implementer interface calls (0 measured misses; see trade-offs).
  - Calls on server/public values in SCIP mode (548 agreement fp): no occurrence names the member.
  - **Open (from M9b, evaluator pass 1 E6):** 3 agreement fp `App#GetClusterId` inside server's own module (`channels/app/audit.go:109`, `channels/app/command_mobile_logs.go:48`, `channels/web/context.go:67`). M9b's B9 binds them right (receivers read in source, `var/m9b/logs/fp-new-sources.txt`), but server's scip-go index records no occurrence on the `GetClusterId` token at any of the 3 (the one occurrence on each line is the `model` qualifier of the map key; `var/m9b/logs/fp-occurrences.txt`). B1's qualifier join does not explain them: they do not leave the module. P0 classifies why scip-go 0.2.7 records none there before B1 is built; until then they stay fp under `bench/accept-m9b.tsv`'s mattermost line.
  - Cross-project `IMPLEMENTS` (a server type implementing a server/public interface).
  - `TsScopes` and `exclude::out_dir` on JSONC tsconfigs (6 on the bench); `baseUrl`-only bare specifiers (mattermost `components/…`).
  - TS `export *` barrels (express 3), parameter-property fields typed by decorators (express 14), TS inherited constructors, call segments outside Go (`MyClass().func()`).
  - Python `super(C, self)` (1 on the bench, microblog), `raise_assigned` and every data-flow form B5 lists.
  - True Go implementers through dependency embeds lost by D7 (unmeasured; P0 counts what the dumps show).

## Amendments

Append each amendment after the previous one's last sentence (lesson of 2026-10-11).

**Amendment 1 (2026-10-16, P0; M9b closed at `e428d15`).** P0's record is `var/m9c/probe-forms.txt`, written by `bench/probe/m9c_forms.py` (`python3 -I`, read-only) from HEAD `e428d15`. Logs are in `var/m9c/logs/`.
- **Disk.** `du -sh target` 12G; `df -h .` 23 GiB free (23,135 MiB) at load 2.89 (`p0-disk.txt`). That is above twice the bench floor plus a release build, so no `cargo clean`.
- **Base (D9).** The M9b run of record, `var/bench/out/36ed5ea46ddf/`, has all 38 manifest dumps (27 `noscip`, 11 `scip`) and 119 PyCG cases, none empty; so has M9a's `2e36cd607eab` (probe section `base`). HEAD's release `rb-bench` (up to date after `cargo build --release -p rb-bench`, sha256 `e9dea089…`) re-scores it with `--milestone m9b --rb-commit b651523…` in 1.52 s. `var/m9c/m9b-rescored.json` and `-perf.json` are `cmp`-identical to `bench/scoreboard/m9b.json` and `m9b-perf.json` (`p0-rescore.txt`). So the gate's base is the committed `m9b.json`.
- **Probe.** It ports the planner's `agree.py`, `ts_classify.py`, `go_forms.py` and `b13b_rule.py` with their logic unchanged. It adds the base check, a cell-by-cell check against the scoreboard, the B1 split at the call token, and the `scipgo` section (below). Its location-identity agreement equals the scored cell on all 12 `scip` calls cells (6 targets × 2 classes) of both records. On M9a it reproduces 33 of the 35 numbers this contract quotes (section `checks`):
  - bookcars fn 694 = 609 (admin 370, frontend 239) + 36 (34 named, 2 default) + 39 + 10;
  - every Go form flag and upper bound;
  - B1's 6,147 fp and 5,554 `pkg.f()`;
  - D7's 431 / 291 (276 suites) / 7.
- **B1 split, corrected.** The two numbers that differ are B1's smaller buckets, which the planner split by a rule that wasn't recorded. By the probe's rule (the call token's span; D8's definition for the first bucket), M9a's 6,147 fp are:
  - 5,554 `pkg.Name()` calls whose one-identifier qualifier carries a server/public package occurrence. This is the number D8 is built for, and it reproduces exactly.
  - 552 members of server/public values (not 548).
  - 2 `model.F()` calls into server/public whose qualifier carries no occurrence at all (`channels/app/migrations.go:823`, `channels/app/platform/support_packet.go:162`; see GetClusterId below).
  - 34 bare calls: 19 in server and 15 in `docs/site` scripts. Only 15 fp have a call site in `docs/site`, so "34 bare calls in `docs/site` scripts" mislabels them.
  - 5 other: 3 `this.f()` in e2e-tests/playwright and 2 `platform.X()` inside server.
- **What M9b moved.** B9 removed 4,485 of the 17,065 Go misses: 4,464 whose only blocker was B9, 16 flagged `ctor` and 5 `bare`. 19 misses still have B9 as their only blocker (17 `crossfile+fieldchain`, 2 `crossfile`; not classified). B9 recovered none of the call-segment, alias, embedding or package-var misses, and 16 of the 4,657 call-result ones. So B2's root causes hold at HEAD; only the base and the bounds move. The bounds are now: call results 4,007 (was 4,023), all five 11,546 (11,562), results and segments 6,812 (6,828). Call segments 2,171, aliases 3,127, embedding 1,313 and package vars 109 (889) are unchanged. bookcars, go_clean_arch, go_gin_example, express_ts_boilerplate and PyCG are identical in both records. microservices_demo's 27 are gone (tp 208, fn 0). go_gin_example's 9 read as stated: 4 `var ( appG = app.Gin{C: c} )`, 2 call results, and 3 through `*ArticlePoster` (2 promoted methods and the promoted field `a.Qr.Encode`).
- **Predicted movement, re-based.** The base is the M9b run of record. "Predicted m9c" replaces the table above where they differ.

  | Cell (`bench:target:mode:class`) | M9a | M9b (base) | Predicted m9c | Claim |
  |---|---|---|---|---|
  | `scip:bookcars:noscip:calls` | tp 1,155, fn 694 | tp 1,155, fn 694 | unchanged: tp +600 to +686 heuristic; fn 8 to 94; fp possible, each classified | C2, C3 |
  | `scip:bookcars:noscip:calls_file_scope` | fn 7 | fn 7 | fn 5 | C2 |
  | `scip:go_clean_arch:noscip:calls` | tp 37, fn 12 | tp 37, fn 12 | tp 49, fn 0 | C5 |
  | `scip:go_gin_example:noscip:calls` | tp 152, fn 9 | tp 152, fn 9 | tp 161, fn 0 | C4, C5 |
  | `scip:mattermost:noscip:calls`, B2 | fn 17,157 (Go 17,065) | fn 12,672 (Go 12,580, TS/JS 92) | fn falls; upper bound 11,546 of the M9b Go misses (6,812 from results and segments alone). fp heuristic rises in two places: where new edges reach server/public value methods (accept line, B1 residue), and at sites whose scip reference the scip-go type check dropped (the GetClusterId class, below) | C5 |
  | `scip:mattermost:noscip:calls`, B1 | gold 33,312, fp 6,147 | gold 33,312, fp 6,141 (5,554 joinable) | gold +X (X ≥ the joined pairs), up to 5,554 fp → tp, fn + (X − joined tp). The 563 value-member fp, the 2 calls with no qualifier occurrence and the 3 `App#GetClusterId` stay | C9 |
  | `scip:mattermost:noscip:calls` | `found_out_of_scope{not_covered}` 21,644 | 22,872 | rises (as above) | C2, C3, C5 |
  | `scip:microservices_demo:noscip:calls` | tp 181, fn 27 | tp 208, fn 0 | unchanged | |
  | `scip:express_ts_boilerplate:noscip:calls` | fn 24 | fn 24 | unchanged | |
  | `pycg:*:calls` | tp 12 / 48 | tp 12 / 48 | tp 14 | C6 |
  | `pycg:*:calls_file_scope` | tp 94 / 186 | tp 94 / 186 | tp 105 | C6 |
  | `exact` sample, `scip` family | 60 / 60 right | 60 / 60 right | the FALSE `ElasticsearchInterfaceTestSuite#` row leaves the pool. **322** mattermost scip-go `IMPLEMENTS` leave (was 291) and 407 stay. go_clean_arch's 5 and microservices_demo's 3 stay. Redraw only if `rb-bench sample` lists drawn edges without a verdict | C8 |
- **D7 as decided (changes C8's prediction).** The planner's reading put server's and server/public's indexes together and judged structs only. D7 reads "the job's SymbolInfo" and speaks of "a type". Read that way (probe, "each job's own index, structs and other named types"), 407 rows verify, 322 are dropped and 0 lie outside (was 431 / 291 / 7). The 31 extra drops:
  - **25 test-plugin rows** (`MyPlugin#`, `Plugin#`, `PluginUsingLogAPI#` → `teams/WebHub#`, one → `plugins/AppIface#`). The planner's reading verified them through `plugin.MattermostPlugin`, but its `API API` and `Driver Driver` are named fields, and Go promotes no method through a named field. So they are false. A rule that takes "named by its fields" literally verifies them, because `Owner#Field.` symbols don't say whether a field is embedded. The type's signature text does (`struct{invalid type; …}` against `API API`).
  - **7 rows from named non-struct types** (SCIP kind 54). 6 are false: `ByChannelDisplayName` and `ByPreference`, sort types over `model.ChannelList` and `model.Preferences`, related to `Users#`, `Builder#` and `ReaderProvider#`. 1 is true and kept: `*AllowedIPRanges` → `Auditable#`; `model/ip_filtering.go:12` declares `Auditable()`.
  - **So P5 reads D7** as "methods of index types named by its *embedded* fields, as the type's signature lists them", and judges a kind 54 type by its own methods.
  - **C8 gains three rows:** a struct with a named field `API API` whose type has the interface's methods (dropped), and a named slice type that declares the interface's method (kept) and one that doesn't (dropped).
  - **New mutant m339** ("a named field counts as embedded", filter `scip_go_type_relationships_need_the_method_set`); C15's range becomes m300–m339.
  - The judged sample reads as before: both TRUE type rows verify, and the FALSE `ElasticsearchInterfaceTestSuite#` row is dropped.
- **B13b's cause.** 289 of the 291 rows the planner's reading drops come from a struct that embeds a field of `invalid type`, directly or through an embedded index struct (for example `MmctlUnitTestSuite struct{invalid type; mockCtrl invalid type; …}`). The 6 false kind-54 rows come from types over an invalid underlying type (probe `d7`). go/types answers `Implements` with true for both cases:
  - `hasAllMethods`: "If V is not a valid type, or if it is a struct containing embedded fields with invalid types, the result is true".
  - `implements`: true for an invalid underlying type.

  scip-go relates every candidate that passes its method-count and mask filters (`internal/implementations/implementations.go:91-92`). Why the types are invalid is the next item.
- **The 3 `App#GetClusterId` fp (Out of scope, open): classified.** It is none of the forms suspected (promoted method, type alias, cross-file method). The callee is `func (a *App) GetClusterId()` (`channels/app/cluster_discovery.go:14`), and B9 binds all 3 calls right. Each call is the value of a map-literal entry whose key is a server/public constant: `model.AuditKeyClusterID: a.GetClusterId()`. Why scip-go records no occurrence there:
  1. scip-go emits a reference only for an identifier in go/types' `Info.Uses` (`internal/visitors/visitor_file.go:135`, `:226`).
  2. Go 1.27.2's checker skips a map entry's value when the entry's key is invalid (`go/types/literals.go:286-288`, `if !x.isValid() { continue }`).
  3. The key is invalid because, in rb's scip-go image, no imported package loads:
     - The image builds scip-go v0.2.7 with Go 1.27.2 (`docker/scip-go/Dockerfile:10`), whose compiler writes unified export data version 5 (`cmd/compile/internal/noder/unified.go:29`, `uirVersion = pkgbits.V5`).
     - scip-go loads imports from export data (`internal/loader/loader.go:24-30`, no `NeedDeps`) through x/tools v0.45.0, whose decoder refuses anything above version 4 ("export data version %d is greater than maximum supported version %d", `internal/pkgbits/decoder.go:86`).
     - go/types then makes every import, the standard library included, a fake package with no members.
     - The name isn't simply missing at the resolved version: `AuditKeyClusterID` exists in server/public v0.4.3 (`model/audit_record.go:19` in the job's module cache).
  - **Proof from the index** (probe `scipgo`):
    - The one occurrence on each of the 3 lines is the `model` qualifier.
    - `rec := &model.AuditRecord{…}` is `var rec *invalid type`.
    - 119,977 of server's 202,385 locals are typed `invalid type`, and 284 of go_clean_arch's 571: `dbHost := os.Getenv(…)` is `invalid type`, while `mysqlRepo.NewAuthorRepository(…)` is `*AuthorRepository`.
    - In server, a selector-keyed entry's value keeps its occurrence when the key is an own-module package (4 of 4). It loses it when the key's package is another module (59 of 59) or when the key's head is a local variable (124 of 124).
  - **Proof by reproduction**, offline in the same image (`logs/getclusterid.txt`, sources in `var/m9c/scripts/importer/`):
    - `go list -export -deps ./...` writes export data for 257 of the 258 packages in go_clean_arch's dependency closure (all but `unsafe`).
    - x/tools v0.45.0's `go/packages`, run with scip-go's load mode on a module that only imports `os`, reports `could not import os (… cannot decode "os", export data version 5 is greater than maximum supported version 4)` and types `home := os.Getenv("HOME")` as `invalid type`. On server/public's `model` it reports 245 distinct errors (one per failed import path, plus names left undefined by them, such as `msgpack`), and `could not import` errors for `ldap` and `net/http` are among them.
  - **It also answers M9b amendment 1's open question.** Dependency members carry no occurrence because of rb's image (the toolchain and x/tools pairing), not because of scip-go 0.2.7 itself. One exception stays unexplained: server/public's index holds 4 `github.com/mattermost/ldap` `CompileFilter()` occurrences, while the offline reproduction fails to import `ldap` like every other package.
  - **No M9c phase changes it.**
    - D8 hands back only an unmatched reference whose qualifier carries a package occurrence. At the 3 sites the qualifiers (`a`, `c.App`) carry none, and the callee is in server's own module.
    - D7 changes relationships, not references.
    - The Tier B edges are already right.

    The 3 stay fp under `bench/accept-m9b.tsv`'s mattermost line until the image is fixed.
- **Flags for later phases.** None invalidates a phase, but each bounds what it measures.
  - Every Go agreement cell is scored against references from this degraded type check, so the predictions hold for the image as built.
  - **P3.** A new Tier B edge at a site whose reference the type check dropped scores as agreement fp, as the 3 do. Examples are the values of map entries keyed from another module (183 such lines in server: 59 + 124) and the arguments of `make(model.T, …)` (`support_packet.go:162`).
  - **P5, D8.** D8's trigger is a symptom of the image. Once imports load, `model.NewId`'s member token gets a `server/public v0.4.3` occurrence. The reference then becomes `external`, not `unmatched`, and D8's hand-back stops firing. The cross-version join would then have to read external member occurrences of another job's module, the M9b plan's mechanism. As decided, D8 does not; P5 records this coupling in the design note.
  - **P5, D7.** D7 is needed while the image stays as it is.
  - **M9d.** An image whose toolchain writes export data x/tools v0.45.0 reads, or a scip-go on an x/tools that reads version 5 (which releases these are is not measured), changes every scip-go index on the bench. M9d re-measures B1, B13b and the Go agreement cells with it, before the `go.work` run.
- **File:line at HEAD** (`var/m9c/scripts/check_citations.py`, `logs/citations.txt`).
  - 22 of the 23 citations above hold unchanged at HEAD. `Content::go_modules` moved from `rb-indexer/src/lib.rs:406` to `lib.rs:409`.
  - The control, which re-checks every citation 7 lines lower, reports 20 of 23 MISMATCH.
  - `resolve.rs` at M9b's close differs from `5a310aa` only by `package_key` becoming `pub(crate)` (`resolve.rs:335`) and one test row (from line 1875); no cited line moved.
  - `rb-indexer/src/lib.rs` gained M9b's data-rest service gate (`data_rest_services`, `lib.rs:836`), two `assemble` hunks, their tests, and `PIPELINE` `m9b-2` (`lib.rs:145`; C11 reads `m9b-2` → `m9c-1`).
  - P5's join hangs off the SCIP job loop at `lib.rs:1076` (`scip::join` at `lib.rs:1085`).
- **Safety.** No probe repository was created. `SELECT * FROM rb.repo ORDER BY 1` is identical before and after (`logs/repo-{before,after}-p0.txt`, 4 rows). The other database reads were `\d rb.scip_index` and two `READ ONLY` transactions over `rb.scip_index.stderr_tail`. The first matched no rows (wrong digest pattern). The second found the tail empty for the 20 most recent ok scip-go rows, the 13 exported ones among them. Container runs used `--network none` on scratch copies of the sources; the bench's module caches were mounted read-only or copied. No change under `crates/`.

## Result

Not yet built.
