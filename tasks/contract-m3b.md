# M3b contract: SCIP orchestration and exact edges (draft, planner, 2026-10-01)

Written before any code. M3b is done only when every assertion below is backed by a passing test or a recorded measurement. Sections marked *pending research* are filled in once the indexer facts are verified (image digests, invocations, build requirements).

## Scope

Design §5.2, §5.3 and §13 (M3b): run SCIP indexers in pinned containers, join their occurrences with Tree-sitter facts, and replace a project's heuristic edges and symbol keys with exact ones.

- **In:** scip-java (Maven and Gradle) and scip-typescript (npm, yarn and pnpm workspaces) first; the `rb-scip` crate (orchestration, index cache, decoding, join); per-project key replacement; `scip_status` per project; `rb status` coverage of exact vs heuristic edges; Q1/Q2 goldens on the fixtures that build.
- **Out (later):** scip-python, scip-go and the others (M6); change sets and the incremental re-index of only changed projects (deferred again: every project re-indexes when its input hash changes, which is already incremental at the project level); orphan detection (needs exact edges on every project of a service; M4 with routes).

## Engineer declaration

**Canonical state:**
- `index.scip` per project, content-addressed under `(project_input_hash, indexer_digest)` in `rb.scip_index` (bytes, plus `scip_status` and a stderr tail). `project_input_hash` hashes the project's file blob SHAs and its lockfiles, so an unchanged project is never re-indexed.
- The relational tables remain the source of truth for the graph.

**Derived:**
- Symbol keys: for a project with `scip_status = ok`, the SCIP symbol string replaces the heuristic key of every definition the join matches. Definitions the join does not match keep heuristic keys.
- `CALLS`, `REFERENCES`, `IMPLEMENTS` with provenance `scip` and certainty `exact` from occurrences (§5.3). Files of a SCIP-ok project get no heuristic code edges at all.
- `Package` nodes and `DEPENDS_ON` from external symbols (§5.5): manager, package name, version.

**Invariants:**
1. A project is either fully SCIP (`ok`: keys and edges replaced) or fully heuristic (`failed`, `skipped`, `unsupported`); never a mix within one project.
2. Every `scip` edge is `exact`; every heuristic edge stays `heuristic`/`ambiguous`. Provenance and certainty are never guessed upward.
3. Containers run with the network disabled during the index phase and never receive credentials or host toolchains; the fetch phase reaches package registries only.
4. One failing project never fails the run: it records `scip_status = failed` with a stderr tail and falls back to Tier B.
5. Determinism: the same repo, indexer digest and cache give byte-identical dumps; a cached index gives the same graph as a fresh one.
6. The join never invents ranges: an occurrence matches a capture only when its range equals the capture's range or its name node (§5.3); unmatched occurrences become diagnostics, counted in coverage.

## Assertions

| # | Assertion | Proof |
|---|---|---|
| S1 | `rb-scip` decodes an `index.scip` (documents, occurrences with roles and ranges, symbol information with relationships, external symbols) through the `scip` crate; position encodings (UTF-8 vs UTF-16 offsets) convert to the byte columns the rest of the graph uses | unit, on a committed sample index |
| S2 | Project detection maps each supported project (Maven `pom.xml`, Gradle, `package.json` with `tsconfig.json`) to an indexer job: image digest, working directory, command, fetch step | unit |
| S3 | Orchestration runs the fetch phase with registry-only network and the index phase with `--network none`, with CPU, memory and time budgets, and captures stderr; an unavailable Docker or a build failure yields `scip_status = failed`/`skipped` with a reason and no panic | unit with a fake runner (`rb_scip::orchestrate`, `rb_indexer::projects`); e2e `scip_ts_express_prisma` with the real Docker (**done**) |
| S4 | The index cache is keyed by `(project_input_hash, indexer_digest)`; a re-run with unchanged inputs runs no container | unit (`projects::tests`: a hit runs nothing, a blob change under the root is a new key, one outside it is not); e2e re-index reports `(cached)` (**done**) |
| S5 | The join (§5.3): a definition occurrence whose range equals a Tree-sitter definition's name range replaces its key; a call capture matching a reference occurrence gives `CALLS`, other non-definition occurrences give `REFERENCES`, `is_implementation` relationships give `IMPLEMENTS`; all `scip`/`exact` | unit on synthetic occurrences plus facts |
| S6 | Key replacement: `MAPS_TO`, `QUERIES`, `DEFINED_IN`, `CONTAINS`, `OWNED_BY` and evidence for a replaced symbol follow the new key; `rb find`, `rb callers` and `rb impact` accept both the SCIP key and the §9.1 forms | every stage takes one keys map built in `assemble`; e2e: the SCIP key, `findUserByEmail().` and `src/db/users.ts:8` give the same callers, `rb find` lists the key, the bare name stays ambiguous (**done**) |
| S7 | Invariant 1: within a SCIP-ok project no heuristic `CALLS`, `REFERENCES` or `IMPLEMENTS` edge remains; references SCIP did not cover are counted in coverage, not guessed (revised: a Tier B guess where the type checker had no information is not worth an edge) | e2e sweep of the dump: every code edge is `scip`/`exact` (**done**) |
| S8 | External symbols become `Package` nodes and `Service -DEPENDS_ON-> Package` with versions from lockfiles where present | unit + e2e (open) |
| S9 | Coverage: `rb status` reports, per project, `scip_status` and the exact share of `CALLS`; the snapshot records unmatched occurrences | `scip_status` diagnostic per job; e2e golden `scip-status.txt` (**done**) |
| S10 | Q1/Q2 goldens on `java-spring-jpa` and `ts-express-prisma` with exact edges, and on `polyglot-monorepo` mixing SCIP-ok and heuristic projects, including a `no-build` project that fails on purpose | e2e goldens |
| S11 | Public repos: spring-petclinic (Maven) and hono (npm workspaces) index in containers; time, index size, exact `CALLS` count, join match rate (occurrences matched / definitions) and unresolved carry-overs recorded; 20 sampled exact edges per repo hand-checked, target 100% | recorded measurement |
| S12 | Cached vs fresh index gives identical dumps; two runs are byte-identical | e2e: the cached re-index dumps byte-identically (**done**) |
| S13 | fmt, clippy, tests and the stub scan are clean; a separate evaluator pass finds no open defects; every new dependency has a stated reason (`scip`, `protobuf`); no host npm/npx is ever invoked | verification loop |

## Verified facts (checked against primary sources on 2026-10-01)

**`scip` crate 0.10.0** (Apache-2.0, published 2026-09-03), which pins `protobuf = "=3.7.2"`; a consumer must depend on the same pin to name the `protobuf::Message` trait (`Index::parse_from_bytes`). No streaming reader exists in the Rust bindings: an index is decoded whole.
- Ranges: `typed_range` (`SingleLineRange`/`MultiLineRange`, added in 0.9.0) takes precedence over the deprecated `range` (3 or 4 ints); half-open, 0-based. **scip-java ≥ 0.13 emits typed ranges only; scip-typescript emits the deprecated form only**, so the decoder handles both.
- Position encoding: both indexers leave `Document.position_encoding` unset. The proto's guidance for JVM and TypeScript indexers is UTF-16 code units, and both compute columns that way (javac `LineMap` columns, `ts.SourceFile.getLineAndCharacterOfPosition`). The decoder converts to byte columns using the line text (`Document.text` when present, scip-java fills it; else the blob) and records when a document's encoding was assumed. scip-java corrects javac's tab expansion for leading tabs only; interior tabs can shift columns.
- `SymbolRole` bits: Definition 1, Import 2, WriteAccess 4, ReadAccess 8, Generated 16, Test 32, ForwardDefinition 64.
- Symbol grammar: `<scheme> <manager> <package-name> <version> <descriptor>+` with `.` for an empty field; descriptors `name/` (namespace), `name#` (type), `name.` (term), `name().` / `name(+1).` (method), `[T]`, `(p)`, `name:`, `name!`; locals are `local <id>` and are unique per document only. `scip::symbol::parse_symbol` parses them.
- `SymbolInformation.kind` (Class 7, Method 26, Function 17, Interface 21, …) is the kind source; `enclosing_symbol` is set for locals only, a global symbol's parent is its symbol minus the last descriptor. `Relationship.is_implementation` gives `IMPLEMENTS`; `is_reference` marks overrides for find-references.
- `Index.external_symbols` lists externally defined symbols and is never a definition in the workspace.

**scip-java 0.13.1** (2026-07-02; `ghcr.io/scip-code/scip-java:0.13.1`, no `v`; Docker Hub `sourcegraph/scip-java` is stale at 0.12.3, amd64 only).
- Digests: linux/arm64 `sha256:0f459823b911bbd08730340f7e4524dec041e8eed778efb761a640bc64a6e932`, linux/amd64 `sha256:3ad7af456aa02cf5e303e900b280711f3656d5d86a217dd5044a8b8a5e64de8c`.
- Image: `eclipse-temurin:25` plus Maven 3.9.1, Gradle 9.4.1, coursier and JDKs 25/21/17; `WORKDIR /sources`; the `scip-java` wrapper picks a JVM through coursier (`JVM_VERSION`, default `21,17`). Java 17+ only; Scala and sbt were dropped in 0.13.
- Invocation: `scip-java index [--output index.scip] [--targetroot …] [--build-tool Maven|Gradle|auto] [--cwd dir] [-- build args]`. Detection looks in the working directory only: `pom.xml`, else `settings.gradle`/`gradlew`/`build.gradle(.kts)`. **It runs the full build**: Maven `clean verify -DskipTests` with a javac wrapper carrying the plugin; Gradle `clean scipPrintDependencies scipCompileAll` through an init script applied to all projects. Per-file shards are aggregated into `index.scip`.
- Offline: the docs say every invocation downloads the project's dependencies; the plugin jar itself is a local file. A fetch phase (`mvn dependency:go-offline` / Gradle with network) followed by an index phase with `--network none` and the build command replaced by `-- --batch-mode -o clean verify -DskipTests` is plausible from the source but **unverified**; so is whether the wrapper's coursier JVM step works offline. Step 2 verifies both empirically on the fixture and records the answer.
- Symbols: scheme `scip-java`, e.g. `scip-java maven . . minimized/Fields#privateField.`, `scip-java maven jdk 17 java/lang/String#`; third-party packages resolve to `group:artifact version` only when `dependencies.txt` is in the target root.
- Monorepo: one build root per invocation, paths relative to `--cwd`; several build roots need several runs with distinct outputs. Gradle multi-module builds run as one.

**scip-typescript 0.4.0** (2025-10-02; Docker Hub `sourcegraph/scip-typescript:v0.4.0`, not stale; **linux/amd64 only**, digest `sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845`; no arm64 image exists).
- Image: `node:24.6.0-slim` with yarn and pnpm; `WORKDIR /src`; entrypoint `scip-typescript`; its `npm`/`yarn` wrappers ignore install failures and index with partial dependency information.
- Invocation: `scip-typescript index [projects…] [--cwd dir] [--output index.scip] [--yarn-workspaces] [--pnpm-workspaces] [--infer-tsconfig] [--no-global-caches] [--max-file-byte-size 1mb]`. Projects are positional directories holding a `tsconfig.json` (or tsconfig paths); a missing `tsconfig.json` skips the project unless `--infer-tsconfig` writes one. npm workspaces have no flag: each package is passed as a project. `references` are followed. Exit 1 and no file when zero documents result.
- Offline: `npm ci` (in a container, never on the host) then indexing with `--network none` is consistent with the source (indexing makes no network calls) but **unverified**. Without `node_modules`, references into dependencies degrade to local symbols (extent unverified).
- Symbols: file paths are in descriptors: `scip-typescript npm <package> <version> src/\`class.ts\`/Class#method().`; workspace packages use their own name and version; externals `@types/react 19.2.18 \`index.d.ts\`/…`. Keys therefore change when a file moves (design §5.1 already handles this).
- Known issue: duplicate documents with unnormalised paths under project references (#330); the decoder normalises and de-duplicates.

**Verified empirically on 2026-10-01 (scip-typescript on `fixtures/ts-express-prisma`, arm64 host, emulated amd64 image, 1.63 GB uncompressed):**
- The fetch phase with the network on (`actual-npm install`, the image's own `npm` is a wrapper that swallows failures) took 6.3 s and wrote a lockfile; the index phase with `--network none` took 1.4 s and produced a 5.5 KB `index.scip`. **Offline indexing after a fetch step works for scip-typescript.**
- Bind mounts must come from a directory colima shares with its VM (the home directory by default): a mount from `/private/tmp/…` silently produced an empty directory inside the VM. Work directories therefore live under the project's `var/scip/`.
- The decoder read the index (4 documents, deprecated ranges, unset encoding treated as UTF-16) and the join matched 4 of 4 definitions and both cross-file calls; references into `@prisma/client` were classified external, import bindings local, and 10 member calls on untyped `express` objects had no occurrence at all.
- Symbols carry file paths: `scip-typescript npm auth-api 1.0.0 src/db/\`users.ts\`/findUserByEmail().`.

**Consequences for the plan**
- On this arm64 host scip-typescript runs as `--platform linux/amd64` under emulation, pinned by digest; verified in step 2 (with timing). scip-java runs natively.
- Image sizes and a prune policy are recorded in step 2 (the host has about 5 GiB free).
- The `polyglot-monorepo` fixture's Maven service and the Java fixture need Maven Central in the fetch phase; the TypeScript fixtures gain a `tsconfig.json` and a lockfile generated inside a container.

## Amendments (step 3b, 2026-10-01)

1. **Ownership of a file.** A file belongs to the deepest build root that contains it, among all jobs; it is exact when that root's index is `ok`, heuristic otherwise. A nested package whose own job failed therefore stays heuristic even when an outer index happens to carry its documents, and a project's join only sees the files it owns.
2. **`scip_status` detail is a function of the commit and the cache.** It records `indexer`, `status`, `image` (or `reason` for `skipped`/`unsupported`), `elapsed_ms` (the container run's time, the cached run's when served from the cache), `unpinned`, `coverage` when ok and `stderr_tail` when not. Whether the index was served from the cache is printed by `rb index`, never stored: invariant 5 needs a warm re-index to dump byte-identically. `rb status` prints these diagnostics as `scip:` lines.
3. **Statuses.** `RunStatus::Skipped(reason)` covers `--no-scip` and an unavailable Docker (`docker version` failing); `unsupported` (scip-java, no usable image) is never cached; `failed` and `timeout` are cached and served like `ok`, so a broken project is not rebuilt until its inputs change.
4. **Join targets.** An edge's target must be a symbol that matched a Tree-sitter definition, since only those have `Symbol` nodes. A matched reference to a symbol the index defines at a range Tree-sitter does not treat as a definition (a field, a constant) is counted as `references_unanchored`. Design §5.3's "every definition occurrence becomes a `Symbol` node" is not built: it would add a node per field and constant.
5. **`--no-scip` runs are a different tool configuration:** `tool_versions.scip` records it, so an index with indexers after one without is not "up to date".
6. **Bare names.** `rb callers findUserByEmail` lists both `findUserByEmail` methods of the fixture rather than picking one; the goldens use the `findUserByEmail().` form (S6 note).
