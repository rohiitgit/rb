# Repository Brain — Design Document

| | |
|---|---|
| **Status** | Draft v1 — design only, no implementation yet |
| **Date** | 2026-09-27 |
| **Scope** | A generic, LLM-free code-intelligence engine for any Git repository |
| **Decisions fixed by the requester** | Postgres + Apache AGE from day one; generic corpus; Rust vs Python/Go/C++ to be decided here (Appendix A) |

---

## 1. Goals and non-goals

### 1.1 Goals

The Repository Brain indexes a Git repository into a typed graph. It answers structural questions about that graph by traversal only, with no language model in the loop.

It must answer these questions:

| # | Question | Graph operation |
|---|---|---|
| Q1 | What calls this function? | Inbound `CALLS` (+ dispatch expansion), optionally transitive |
| Q2 | What breaks if I remove this? | Inbound reachability over "requires" edges, then roll up to endpoints, services and owners; outbound orphan detection |
| Q3 | Which services depend on this table? | Inbound `QUERIES`/`MAPS_TO`, then inbound `CALLS` up to entry points, then roll up to services |
| Q4 | Show the request path for login | `Endpoint` → handler → outbound `CALLS`/`QUERIES`/`CONSUMES` (downstream), plus inbound `CONSUMES` → callers (upstream) |

It must also produce a **Service Card** for any service: APIs, tables, owners, dependencies and internal call graph (§9.5).

### 1.2 Quality requirements

- **Deterministic.** The same commit and the same pinned tool versions must yield a byte-identical graph dump and identical query output. Every output list has a total ordering.
- **Honest.** Every edge records *how* it was derived (`provenance`) and *how certain* it is (`certainty`, §6.3). The engine never presents a heuristic edge as fact, and it never silently picks one of several ambiguous candidates.
- **Explainable.** Every edge links back to the source location (file, blob, range) that justified it.
- **Incremental.** Re-indexing after a commit costs roughly the size of the change, not the size of the repo (§3.4 lists the exceptions).
- **Degrades gracefully.** A language without a semantic indexer still gets syntactic and heuristic coverage. Each index run reports its own coverage (§12.4).

### 1.3 Non-goals (v1)

- No LLM-based summarisation, ranking or search. Full-text search over names and paths is in scope.
- No runtime tracing and no dynamic analysis.
- No whole-program points-to or variable type analysis (VTA). CHA plus RTA pruning is the v1 precision ceiling (§6).
- No IDE or LSP server. The query surfaces are a CLI and a REST API.
- No SQL dialects other than PostgreSQL for full parsing. Other dialects get a best-effort fallback (§7.2).

---

## 2. How this design differs from the source blueprint

This document started from the "Architecting the Repository Brain" blueprint. Several of that blueprint's claims don't hold up in practice. The design corrects them as follows:

| Blueprint claim | Reality | What this design does |
|---|---|---|
| Tree-sitter incrementally re-parses only the changed byte ranges of each commit | Tree-sitter's edit-based incremental parsing needs an in-memory previous tree plus edit operations. That is an editor feature. A batch indexer re-parsing a changed file costs milliseconds anyway. | The unit of incrementality is the **file blob** (content-addressed cache keyed by blob SHA, §3.4). |
| SCIP is generated from the Tree-sitter AST | SCIP indexers are separate tools. They run the language's own compiler or type checker, independently of Tree-sitter. | Tree-sitter and SCIP are **parallel fact sources**, joined by `(path, range)` (§5.3). |
| The resulting graph is "sound and precise" and "mathematically proven" | Static call graphs are under-approximate: reflection, `eval`, string dispatch and dynamic imports are missed. They are also over-approximate, because CHA over-links virtual calls. Both kinds of error exist. | Every edge carries `provenance` and `certainty`. Each run reports coverage metrics. Answers state their certainty. |
| Recursive CTEs explode, while AGE gives millisecond N-depth traversal | Both behave badly if the query **enumerates paths**; openCypher variable-length `MATCH` does exactly that. Both behave well if they compute **reachable node sets**. A `WITH RECURSIVE … UNION` (not `UNION ALL`) query is set-semantic and terminates on cycles. | The hot-path queries (Q1–Q3 transitive) run on an in-memory adjacency index or on set-semantic SQL. AGE serves ad-hoc Cypher, pattern and architecture-rule queries (§8.4). |
| `DEFINES` edge used to resolve a name to a symbol | This edge is not in the blueprint's schema. | The schema adds `DEFINED_IN`, `IN_SERVICE` and `HANDLED_BY`. Name resolution goes through a search index, not an edge (§9.1). |
| `EXPOSES` (Service→Endpoint) is enough to trace a request | That edge doesn't record which function handles the route. | The schema adds `HANDLED_BY` (Endpoint→Symbol). |
| DI simulation makes the call graph sound | Spring profiles, `@Conditional`, and beans built by factories or reflection cannot always be resolved statically. | DI facts produce `di` provenance edges and instantiation facts. Conditional beans are flagged, not asserted (§6.4). |
| pg_query handles "all SQL" in the codebase | pg_query parses PostgreSQL SQL only. JPQL/HQL, other dialects and SQL built by string concatenation are not covered. | Dialect fallback, a template-normalisation step, and an explicit `unparsed_sql` diagnostic (§7). |

---

## 3. Architecture overview

```
                    ┌──────────────────────────── rb-indexer (Rust) ─────────────────────────────┐
 Git repo ──► [1] Ingest ──► [2] Extract (per blob, parallel) ──► [3] Resolve (per project) ──► [4] Assemble ──► [5] Load
 (commit)     • gix tree     • Tree-sitter tags + framework       • SCIP indexers (Docker,        • nodes/edges     • COPY → Postgres
              • diff vs      •   packs (routes, clients, ORM, DI)  │  pinned by digest)            • certainty       • project → AGE
                last run     • SQL → pg_query                      • SCIP ⋈ Tree-sitter merge     • derived edges   • flip live snapshot
              • CODEOWNERS   • facts cache (blob_sha, ext_ver)     • heuristic fallback           • coverage report
              • boundaries                                         • DI / HTTP stitching
                    └────────────────────────────────────────────────────────────────────────────┘
                                                                                                    │
                        ┌────────────── Postgres 18 + Apache AGE (one DB) ──────────────┐           │
                        │  relational schema rb.*  (source of truth)                    │◄──────────┘
                        │  AGE graph rb_<repo>_<snapshot>  (projection)                 │
                        └───────────────▲───────────────────────────────▲───────────────┘
                                        │                               │
                         rb-server (Rust, axum)                 psql / Cypher (ad-hoc)
                         • in-memory CSR adjacency per live snapshot
                         • REST API, depth/fan-out limits
                                        ▲
                                  rb CLI (thin client, or embedded mode against Postgres directly)
```

### 3.1 Pipeline stages

| Stage | Input | Output | Unit of work | Parallelism |
|---|---|---|---|---|
| 1 Ingest | commit SHA | file manifest `(path, blob_sha, lang)`, ownership map, service boundaries, change set | commit | — |
| 2 Extract | blob | `FileFacts` (definitions, references, call sites, string literals, framework facts, SQL facts) | blob | per-file (rayon) |
| 3 Resolve | project + FileFacts | resolved symbol ids and edges | project (build unit) | per-project (containers) |
| 4 Assemble | resolved facts | snapshot graph + derived edges + coverage report | snapshot | — |
| 5 Load | snapshot | Postgres rows + AGE graph + live pointer flip | snapshot | COPY streams |

### 3.2 Rust workspace layout

| Crate | Responsibility |
|---|---|
| `rb-model` | IDs, node and edge kinds, `certainty`/`provenance` enums, graph builder, canonical dump |
| `rb-git` | `gix` wrapper: manifests, tree diffs, blob reads, CODEOWNERS, boundary detection |
| `rb-syntax` | Tree-sitter grammars, `tags.scm` runner, enclosing-scope index |
| `rb-frameworks` | Framework packs: data-driven query files plus small Rust handlers (§4.5) |
| `rb-sql` | Candidate detection, template normalisation, pg_query plus fallback, table/op extraction |
| `rb-scip` | Indexer orchestration (Docker), SCIP protobuf decoding, occurrence join |
| `rb-resolve` | Heuristic resolver, call-graph assembly, DI synthesis, HTTP stitching, derived edges |
| `rb-store` | Migrations, COPY loaders, AGE projection, snapshot lifecycle |
| `rb-query` | Q1–Q4 and Service Card algorithms, CSR graph, SQL fallbacks |
| `rb-server` / `rb-cli` | REST API and CLI |

### 3.3 Determinism rules

1. Every external tool (grammars, SCIP indexers, pg_query) is pinned. Indexer images are pinned **by digest**. All versions are recorded in `index_run.tool_versions`.
2. Natural keys (§8.2) are the only identities that cross stages. Surrogate IDs are assigned after sorting the natural keys.
3. Every output list is sorted by `(distance, certainty rank, natural key)`.
4. Wall-clock time and host paths never enter the graph.
5. **Test:** index the same commit twice, then `rb dump` twice. The dumps must be byte-identical (§12.2).

### 3.4 Incrementality

- **Extract** is fully incremental. `FileFacts` are cached under `(blob_sha, extractor_version, framework_pack_versions)`. An unchanged blob is never re-parsed, even after a rename.
- **Resolve (SCIP)** is incremental **per project**, not per file. A project (a Maven module, an npm workspace package, a Go module) is re-indexed when any file inside it changes. So is every project that depends on it, because a changed signature can change resolution downstream. SCIP indexers run whole-project type checking; no current indexer offers per-file incremental output.
- **Assemble and Load**: v1 re-assembles the snapshot from cached facts and bulk-loads a new snapshot. This takes seconds to minutes and keeps correctness simple. v2 can apply edge deltas once the dump-equality test (§12.2) guards it.

---

## 4. Ingestion and syntactic extraction

### 4.1 Git ingestion (`rb-git`)

- `gix` opens the repository without shelling out. For the target commit, the ingester walks the tree and emits `(path, blob_sha, mode, size)`.
- It skips `.gitignore`d paths only when they appear in the working tree; the Git tree is authoritative. It also skips submodules, which are recorded as `external` nodes. Blobs over a configurable size (default 1 MiB) and blobs detected as binary or generated are skipped too. A file counts as generated if it matches `linguist-generated` in `.gitattributes`, carries a `@generated` or `Code generated … DO NOT EDIT` header, or sits in a configured path.
- Paths that are not valid UTF-8 are skipped with a `skipped_non_utf8_path` diagnostic. Keys are UTF-8 text, and a lossy conversion could merge two distinct files.
- **Change set.** A tree diff between the commit of the previous live snapshot and the target commit gives added, modified, deleted and renamed files. *M1b finding:* the facts cache is keyed by blob, so unchanged files are never re-parsed without a change set. Change sets are therefore deferred to M3, where they decide which SCIP projects to re-index.
- **Contributors (optional).** For recent history, the ingester counts commits per path over a window, by default 90 days before the target commit's timestamp. It uses committer identity only and runs deterministically for a given commit. The result is reported as `contributors`, never as owners.

### 4.2 CODEOWNERS

- Lookup order follows GitHub: `.github/CODEOWNERS`, then `CODEOWNERS`, then `docs/CODEOWNERS`. The first one found is used.
- Patterns follow gitignore-style semantics with GitHub's precedence rule: **the last matching pattern wins**. A matching line with no owners explicitly clears ownership.
- There are exceptions to gitignore: CODEOWNERS does **not** support `!` negation, `[a-z]` character ranges or `\#` escaping.
- The existing Rust crates are stale (`codeowners` 0.1.3 dates from 2017) or tiny (`codeowners-rs` 0.1.1). The matcher is therefore a small in-house segment matcher (`rb-git/src/glob.rs`). `rb.toml` exclude patterns use the same matcher, so the Brain has one glob dialect.
- One documented GitHub rule differs from gitignore: a pattern whose last segment is exactly `*` is not recursive. `docs/*` owns `docs/a.md` but not `docs/sub/b.md`. `docs/*/` names directories, so it is recursive below each one.
- A leading UTF-8 BOM and CRLF line endings are handled. Spans still count raw bytes.
- Owners are lowercased, because GitHub handles and emails are case-insensitive: `@Alice` and `@alice` are one owner.
- The CODEOWNERS file used (the first found) is recorded. Other CODEOWNERS files produce a `codeowners_ignored` diagnostic, and invalid lines produce `codeowners_invalid_line`.
- The output is `File -OWNED_BY-> Owner` for every file. A service's owners are the owners of its boundary root path, plus a share histogram computed over its files (§9.5).
- The matcher is property-tested against a table of GitHub-documented examples (§12.3).

### 4.3 Service boundary detection

A **project** is a directory holding a recognised build manifest. **Every project becomes a `Service` node**, with `deployable: bool` separating runnable services from libraries. This lets the Service Card treat a monorepo's shared library like any other unit. A SCIP build unit usually coincides with a project, but not always.

| Signal | Creates |
|---|---|
| `package.json` (with `workspaces` → each workspace package) | project, and a service if it has a `start`/`main` entry or a Dockerfile |
| `pom.xml` `<modules>`, `settings.gradle(.kts)` `include` | project per module; a service if it has a Spring Boot main class or packaging `jar`/`war` with a Dockerfile |
| `go.mod`; `package main` directories | project per module; a service per `main` package |
| `Cargo.toml` `[workspace] members`; `[[bin]]` | project per crate; a service per bin |
| `pyproject.toml` / `setup.cfg` | project; a service if it has an ASGI/WSGI app or a `__main__` entry |
| Bazel `BUILD` targets | a project per package, a service per `*_binary` / `*_image` target (v2) |
| `Dockerfile`, `docker-compose.yml`, k8s `Deployment` manifests | service name hints and cross-links from compose/k8s service names to directories |
| `rb.toml` in the repo root | **explicit overrides**, which always win: `exclude = [patterns]` ignores manifests during detection, and `[[service]] path/name/deployable` adds or overrides a project. A malformed `rb.toml` fails the run, because a silent fallback would hide a user's intent. |

Every file belongs to the innermost project that contains it (`File -IN_SERVICE-> Service`). The repo root `.` is the implicit outermost project. If no manifest sits there, the root still becomes a `Service` with `role: unassigned` and name `(unassigned)` whenever some file lies outside every project. Git forbids `.` as a path component, so no real directory can collide with that key. An earlier `(root)` key could collide, and the evaluator found that.

A service's owners (`Service -OWNED_BY-> Owner`, certainty `synthetic`) are the owners of its **anchor file**: its first manifest, or else its first file. `attrs.via` names the anchor file.

### 4.4 Tree-sitter extraction (`rb-syntax`)

For each supported language, the extractor runs one pass per blob and emits:

1. **Definitions and references** from the grammar's `tags.scm`: `@definition.{class,function,method,interface,module}` and `@reference.{call,class,implementation}`, each with a name and a byte/line range.
2. **Enclosing-scope index.** An interval tree of definition ranges, so that any range can be mapped to its innermost enclosing definition. This is what turns a call site into a `caller → callee` edge.
3. **Imports**, from a per-language `imports.scm` (written by us). These feed the heuristic resolver (§5.4).
4. **String literals**, recorded with their parent call expression. Adjacent literal concatenations and template strings are folded into a template, with `${…}` holes marked. SQL (§7) and HTTP stitching (§10) both use them.

v1 languages: TypeScript/JavaScript (incl. TSX), Java, Python and Go. v1.x adds Kotlin, C#, Rust and C/C++. Grammars are compiled in as crates at pinned versions, and the versions are part of `EXTRACTOR_VERSION` (a test checks them against `Cargo.lock`).

What M1b learned about the stock `tags.scm` queries:
- **TypeScript:** its `tags.scm` only covers the TypeScript additions (signatures, interfaces, abstract classes, modules), so TS/TSX run the JavaScript query plus the TypeScript one.
- **Java:** the stock query misses enums, records and constructors. A small supplement adds them. Annotation types (`@interface`) are not captured yet.
- **Go:** methods attach to their receiver type. The type becomes the parent when it is declared in the same file; otherwise the receiver name is kept on the definition and used in the key.
- **Python:** functions directly inside a class become `method`.
- **False positives:** the TSX grammar reports a syntax error for a bare `&` in JSX text, and tree-sitter-typescript 0.23.2 (the newest on crates.io) rejects some TypeScript 5.x syntax such as `export type * from`. The file's other definitions are still extracted, but definitions inside the unparsed region can be lost. Evaluating a newer grammar revision is an open item.
- **TypeScript:** type aliases, enums and `namespace` blocks are added by a supplement, because neither stock query covers them.
- **Locals are not symbols (§8.2):** a definition inside an anonymous function (callback, lambda, anonymous class, Go func literal) is local and is dropped. A function value that is itself a captured definition (`const f = () => {}`) is not anonymous.
- **Nesting cap:** definitions deeper than 64 levels are dropped, with a diagnostic, so key size stays linear. Beyond about 32k levels Tree-sitter itself slows down (about 17 s per file); no timeout is used, because it would make results depend on machine speed.
- **Cache identity:** the facts-cache key includes a fingerprint of all bundled queries and of the extractor source, so an edit to either invalidates stale facts automatically.

Output: definitions as `(kind, name, name span, node span, parent)` and references as `(kind, name, span, enclosing definition)`, both 0-based with byte columns. References are cached but not turned into edges until resolution (M3).

### 4.5 Framework packs (`rb-frameworks`)

Framework knowledge is kept **data-driven and versioned**, separate from the core. A pack contains:

- `detect`: a manifest dependency predicate, e.g. `pom.xml` depends on `spring-boot-starter-web`.
- `*.scm` Tree-sitter queries with named captures.
- A small Rust handler per query that turns captures into typed `FrameworkFact`s.

| Pack | Facts emitted |
|---|---|
| `spring-web` | `Route{method, path, handler}`. Class-level `@RequestMapping` is composed with method-level `@Get/Post/…Mapping`. |
| `spring-di` | `Bean{type, name, qualifiers, primary, profile, conditional}`, `InjectionPoint{owner, type, qualifier}` (field, constructor or setter) |
| `spring-data` | `Repository{iface, entity}` from `extends JpaRepository<E, ID>` (and relatives); derived query methods; `@Query(value, nativeQuery)` |
| `jpa` | `Entity{class, table, schema}` from `@Entity` and `@Table(name, schema)`. The default table name follows the configured naming strategy. |
| `express` / `fastify` / `nestjs` | `Route`. Router prefixes are composed via `app.use(prefix, router)` within a module. NestJS `@Controller(prefix)` is combined with `@Get()`. |
| `fastapi` / `flask` / `django` | `Route`, `Entity` (SQLAlchemy `__tablename__`, Django `Meta.db_table`) |
| `go-http` / `gin` / `chi` | `Route` |
| `http-clients` | `HttpCall{method, url_template, base_ref}` for axios, fetch, got, RestTemplate, WebClient, Feign `@FeignClient`, requests/httpx, Go `http.NewRequest` |
| `prisma` / `typeorm` / `sequelize` | `Entity`. `schema.prisma` `model X @@map("x")` is parsed directly. |
| `sql-sinks` | Call sites whose string argument is SQL: `pg`'s `query`, `JdbcTemplate.*`, `knex.raw`, `prisma.$queryRaw`, `sqlx`, `database/sql`, `cursor.execute`, etc. |
| `openapi` / `grpc` | `Route`s from OpenAPI specs; `grpc` service/method endpoints from `.proto` files. These are **authoritative** sources for endpoints. |

Packs are the main extension point. Adding a framework means adding a pack plus golden fixtures (§12.1), with no core changes.

---

## 5. Semantic resolution

### 5.1 SCIP recap

SCIP (the Source Code Intelligence Protocol, originally from Sourcegraph and now maintained at `github.com/scip-code/scip`) is a protobuf index. Each `Document` (one per file) holds `Occurrence`s. An occurrence carries a range, a symbol, a `symbol_roles` bitset (Definition, Import, ReadAccess, WriteAccess, …) and an optional **enclosing range**: the nearest non-trivial enclosing AST node, which the spec says is meant for call hierarchies. Current producers emit `typed_range`/`typed_enclosing_range`; the `repeated int32` forms `range`/`enclosing_range` are deprecated, and consumers should fall back to them. It also holds `SymbolInformation`, with `Relationship`s flagged `is_reference`, `is_implementation`, `is_type_definition` and `is_definition`.

Global symbol strings follow the grammar `<scheme> ' ' <manager> ' ' <package-name> ' ' <version> ' ' <descriptor>+`. A `.` stands for an empty field, and a space inside a field is escaped as a double space. Descriptor suffixes encode kind: `/` namespace, `#` type, `.` term, `name(disambiguator).` method, `[T]` type parameter, `(p)` parameter, `:` meta and `!` macro. Local symbols are `local <id>` and are unique only within a document.

**Consequence:** a SCIP symbol is a stable, human-readable, cross-file and cross-package natural key. The grammar has no file-path field. However, the spec lets descriptors encode ancestry from the file root, and some indexers (reportedly scip-typescript) emit path segments as namespace descriptors. **Symbol keys can therefore change when a file moves in some languages.** `rb diff` (§11) treats a disappear-plus-appear pair with an identical definition body hash as a *move*, not a delete plus an add. *As decided in contract-m5 (D11, fork F4):* M5's `rb diff` infers no moves, since no fact holds a definition-body hash; a changed key is a removal plus an addition.

### 5.2 Indexer orchestration (`rb-scip`)

| Language | Indexer | Needs a working build? |
|---|---|---|
| Java / Kotlin (Scala per the SCIP README) | scip-java 0.13.1 in the locally built slim image `rb-scip-java:0.13.1` (`docker/scip-java`; the published `ghcr.io/scip-code/scip-java` image is 6.75 GB and the Docker Hub `sourcegraph/scip-java` image stopped at 0.12.3). Built: Maven; Gradle is M6. | Yes: runs the Maven/Gradle compile. Bazel needs manual configuration. |
| TypeScript / JavaScript | scip-typescript 0.4.0 (`sourcegraph/scip-typescript`, pinned by digest, amd64 only) | Needs `tsconfig`/`jsconfig` and an installed `node_modules` (`npm ci --ignore-scripts` in the fetch phase) |
| Python | scip-python v0.6.x (`sourcegraph/scip-python`) | Needs the pip environment (or `--environment`) for third-party resolution |
| Go | scip-go v0.2.x (moved to `scip-code`; the Docker Hub image is stale, so build our own image) | Needs a module download (`go mod download`) |
| C / C++ | scip-clang v0.4.x (`sourcegraph/scip-clang`) | Needs `compile_commands.json` |
| Rust | `rust-analyzer scip` | Needs `cargo metadata` |
| C#, Ruby | scip-dotnet v0.2.x, scip-ruby v0.5.x (`sourcegraph/*` images) | Build requirements to be confirmed at M6 |

Operational rules:

- Every indexer runs **in a container** pinned by digest, one project per job, with a CPU/memory/time budget. There are two reasons:
  1. **Hermeticity**, which serves determinism.
  2. **Security.** Building an arbitrary repo runs arbitrary code: Gradle scripts, npm `postinstall`, `setup.py`.
- Each job has two phases:
  1. A **fetch** phase with network access, which populates a dependency cache (`var/scip/cache`) shared across jobs. **As built in M3b** (contract amendment 27): it runs on Docker's default bridge network, unrestricted; a registry allow-list (an egress-filtered network or a proxy) is M5 work and an open risk in §14. It runs the project's own build (Maven `go-offline` then `clean verify -DskipTests`) or package install (`npm ci`/`yarn`/`pnpm install`, always with `--ignore-scripts`, since type indexing needs no lifecycle scripts). The export never writes the repository's `mvnw`/`gradlew` at the build root, so the image's tools are always the ones that run.
  2. An **index** phase with the network disabled (`--network none`).
  No credentials are ever mounted. This also keeps the host's toolchains (e.g. a host npm) out of the loop entirely.
- If a project fails to build, the run records `scip_status=failed` for it, keeps its stderr tail, and drops that project to Tier B heuristic resolution (§5.4). One failing project never fails the run. A container-runtime failure (`docker run` exits 125–127) is `environment` and a budget overrun `timeout`, neither cached; a missing image is `unsupported`.
- The output, `index.scip`, is stored content-addressed under `(project_input_hash, indexer_digest)`. `project_input_hash` hashes the project's file blob SHAs plus its lockfiles and the fetch recipe version (`rb_scip::RECIPE`, bumped whenever the container commands change); for the locally built scip-java image the digest is the image ID. The image identities are also part of the snapshot's `tool_versions`, so a snapshot indexed without an image is rebuilt once it exists; and a snapshot whose jobs did not all finish (`skipped`, `unsupported`, `environment`, `timeout`, none of them cached) is never "up to date" while SCIP is on (contract amendment 44). The container's run time is printed by `rb index` and never stored in the graph (§3.3); stored reasons name no host path (amendment 49).
- **Runtime of record:** `rb` uses the Docker CLI's current context; Docker Desktop (`desktop-linux`) is the one the fixtures, goldens and measurements were made on, the images must exist in that daemon, and the work directory (`--scip-work-dir`) must lie under the home directory, the path its VM shares (README, "Container runtime").

### 5.3 Joining SCIP with Tree-sitter

Each fact source supplies something the other lacks:

| Fact | SCIP | Tree-sitter |
|---|---|---|
| *Which* symbol a name refers to | ✅ exact | ❌ name only |
| *Syntactic role*: is it a call, a type use, a decorator, an import? | partial (roles bitset has no "call") | ✅ |
| Enclosing function of a reference | via `typed_enclosing_range` when the indexer emits it (optional field) | ✅ via the scope index (always available, used as fallback) |
| Framework semantics (routes, SQL strings, DI) | ❌ | ✅ via packs |

**Join algorithm.** Per file, sort SCIP occurrences and Tree-sitter captures by start offset. Match each capture to the occurrence whose range equals it, or else the occurrence whose range is the capture's name sub-node. Then:

- A matched `@reference.call` becomes a `CALLS` edge from the enclosing definition's SCIP symbol to the occurrence's symbol, with provenance `scip` and certainty `exact`.
- Any other non-definition occurrence becomes a `REFERENCES` edge (type use, function passed as a value, import), with the same source. Keeping `REFERENCES` is what makes Q2 catch higher-order use such as `arr.map(validate)`, which is not a call.
- A definition occurrence whose range is a Tree-sitter definition's name (or, for scip-java's annotation-anchored members, lies inside the definition before its name) gives that definition its SCIP key. Definition occurrences with no Tree-sitter definition (fields, constants) do **not** become `Symbol` nodes (contract M3b amendment 4: a node per field for no question asked); references to them are counted as `unanchored`. A definition occurrence of a **local** symbol at a Tree-sitter definition's name (scip-java's enum constant bodies, scip-typescript's nested functions and classes) binds that symbol within its document: the definition keeps its heuristic key (counted as `definitions_local`), and occurrences and relationships naming the local symbol in that document give exact edges to it (contract M3b amendment 54). scip-java's columns are javac's (tabs expanded to multiples of 8, leading tabs corrected); the decoder converts them to bytes (amendment 53). An indexer's lines are its own: javac also ends a line at a lone `\r`, TypeScript also at U+2028 and U+2029, while Tree-sitter's rows end at `\n` only, so the decoder converts each position through its byte offset in the text (amendment 59). An occurrence matched at a span must name the token there (its last descriptor, or a constructor, accessor, numbered object-literal property or import alias of it); otherwise it is unmatched and counted (amendment 59). A document is SCIP-owned only when the indexer and Tree-sitter read it alike: a blob that is not UTF-8, a non-ASCII file in a Maven build declaring another source encoding, a scip-java file with a lone `\r` or a `\u000a`/`\u000d` escape (javac ends a `//` comment there, Tree-sitter does not), or a method or type definition occurrence no Tree-sitter definition accounts for, on a token or inside a method, leaves the whole file to Tier B, counted as `documents rejected` with its reason (amendment 64); so do a Java file in which a reference that would become an edge sits in another definition by Tree-sitter's structure than by the index's own enclosing ranges (javac translates Unicode escapes such as `\u002a/` before lexing; amendment 68), and a non-ASCII Java file whose positions an ISO-8859-1 reading explains as well as the UTF-8 one, a tie included (the encoding can be set where the build is not read; amendments 69 and 73). A Java file is SCIP-owned only when its definitions agree both ways: every method, constructor and type the index defines with a range is a Tree-sitter definition (annotation interfaces and their elements excepted, which Tree-sitter does not capture), every Tree-sitter definition has a definition occurrence, and Tree-sitter parsed it without a syntax error (a member only javac sees, or only Tree-sitter, is otherwise an edge from the wrong source; amendment 72). Pairing is by kind: a type pairs with a type symbol, a method or constructor with a method or constructor symbol (never a field), and a local symbol pairs only a method of an enum constant's body (a local of kind `Method` whose innermost enclosing definition is an enum constant of the method's Tree-sitter parent), so a local variable or an anonymous class's method elsewhere at the name of a method only Tree-sitter sees does not make it real (an anonymous class inside a constant's initializer block or arguments has the same innermost range and is only caught because the escape that produces such a layout is rejected first: a scip-java document holding a Unicode escape of structural ASCII, or of a backslash not immediately followed by `u` (javac's produced backslash escapes the next quote inside a literal), is `structural_escape`, amendments 77 and 80; the `IMPLEMENTS` walk steps only onto supertypes the origin type's symbol information relates it to, amendment 78); a type whose own occurrence is at an earlier mention of its name (a class annotation naming one of its nested types) is found by the amendment-7 fallback, never through its implicit constructor; a record's compact constructor is a Tree-sitter definition (amendment 75). A TypeScript document with a Tree-sitter syntax error is rejected too: the compiler skips merge-conflict markers as trivia while Tree-sitter keeps the text between them (amendment 75). scip-typescript's line 0 starts after a byte-order mark (amendment 65).
- `SymbolInformation.relationships` with `is_implementation` become `IMPLEMENTS` edges, from the impl type or method to the interface or overridden method. The flag carries no direction (scip-java sets it on both sides of an override), so the edge is emitted only when the source's type declares the target's type as a supertype, directly or through supertypes of supertypes, each declared supertype resolved to a symbol through the last type occurrence inside the supertype's name span, accepted only when that symbol names the written type (amendments 16, 21, 26, 43, 53). A `local → global` link is forward by construction, since a local type is never a supertype of a global one (amendment 54).
- *As built in M6d (S9) and M9a (contract-m9a D3):* Go types declare no supertypes, so scip-go's relationships to an interface are taken without the walk. Only a type source makes an edge: `Card# → Payer#` comes from the type's own relationship. scip-go also links each method to the interface method it satisfies. That link gives no `IMPLEMENTS`, since the interface method has no node, and it is counted in `relationships_dropped`.

Framework facts attach to SCIP symbols through the same range join. For example, the handler method name in a `Route` fact joins to that method's definition occurrence.

### 5.4 Tier B: heuristic resolution (no SCIP, or SCIP failed)

For a call named `f` in file `F`, candidates are the definitions named `f` in:

1. the enclosing class or module scope;
2. the same file;
3. files `F` imports (import paths resolved per language: relative paths, `tsconfig` `paths`, Java package plus class name, Go import path to directory, Python module path);
4. the same package or directory.

The search stops at the first level that yields candidates. If there is **exactly one** candidate, the result is an edge with certainty `heuristic`. If there are **several**, the result is one edge to each, with certainty `ambiguous` and attr `candidates=n`. If there are **none**, the call site is recorded as `unresolved` and counted in coverage. Method calls on receivers of unknown type (`x.save()`) resolve only when the receiver was declared in-file with a type. Otherwise they stay `unresolved` rather than linking by bare name to every `save`.

Heuristic symbol keys use a synthetic SCIP-shaped scheme: `rb-heuristic . . . `<path>`/<descriptors>`. For example, ``rb-heuristic . . . `src/auth.ts`/AuthService#login().``.
- The package fields are empty and the file path is a backtick-escaped namespace, so a key depends only on its own file. A project-based prefix would change every key whenever a `package.json` moved.
- Callables that share a name in one scope get SCIP's `(+n)` disambiguator in source order.
- Repeated type-like declarations (TypeScript declaration merging) share one key: the first declaration's kind wins, and every site becomes `DEFINED_IN` evidence.
- When SCIP later succeeds for that project, the keys are replaced wholesale.

**As built in M3a** (contract `tasks/contract-m3a.md`):
- **Facts:** `rb-syntax` caches `imports` (module, named imports with local names, alias, wildcard/static), the Java/Go `package` clause, and typed `bindings`. Bindings cover fields, parameters, typed locals, constructor initialisers, `this`/`self` members, TS parameter properties and Go struct fields and literals. Type references keep their qualifier (`store` in `store.DB`).
- **Module resolution (`rb-indexer/src/modules.rs`):**
  - TS/JS: relative specifiers, probing extensions and `index` files; `./x.js` may name `x.ts`. `tsconfig` `paths` is not read yet.
  - Java: package + class, wildcards, static imports, nested classes.
  - Python: the module path matched by suffix, with the longest shared directory prefix breaking ties; relative levels; `from pkg import submodule`.
  - Go: the longest `go.mod` module prefix, then the package directory.
- **Receivers:** only typed bindings, `this`/`self`/`super`, class names, fully qualified Java names and module aliases give a receiver a type. Inheritance is searched breadth-first, at most 64 levels.
- **Constructors:** a call to a class resolves to its declared constructor; `new T()` is a class reference, because the grammars tag it that way.
- **Coverage:** a per-file `unresolved_references` diagnostic counts unresolved calls, unresolved types and references into external packages; Go and Python built-ins are neither resolved nor counted. `rb status` prints the resolved share.
- **Termination:** type resolution is guarded against cycles through bindings (`session = session.Session()`).
- **Questions:** `rb callers` and `rb impact` run over the snapshot graph loaded into memory (`rb_store::code`). `path:line` picks the symbol declared at or nearest above that line, because symbols record their name position, not their extent.

- *As built in M9b (contract-m9b D5):* a Go method declared in another file than its receiver type (`parent: None`, `receiver: Some(T)`, kept so by `rb-syntax`, whose `parent` is a same-file index) is a member of `T`. `resolve::Index` keeps `(package key, receiver) → method → definitions`, and a member lookup on a Go type adds the methods its own file's package (directory and package name) declares for it. A same-named type in another directory, or in the external `_test` package beside it, gets none. Such a method is never a package-level name: it is in neither the same-package table nor its file's top scope, so a bare `M()` does not reach it, as Go forbids. Symbol keys are unchanged. Route handlers written as method values (`h.signUp`) resolve the same way.

### 5.5 External packages

SCIP symbols from other packages (manager, package name, version) become `Package` nodes, with `REFERENCES`/`CALLS` edges to external `Symbol` stubs. Lockfiles (`package-lock.json`, `pnpm-lock.yaml`, `go.sum`, Maven dependency tree output from the fetch phase, `poetry.lock`/`uv.lock`) give `Service -DEPENDS_ON-> Package` with resolved versions. This answers "who uses library X" and feeds the external section of the Service Card.

**As built in M3b** (contract `tasks/contract-m3b.md`, amendments 11–12): a `Package` node per distinct `(manager, name, version)` named by an occurrence of an undefined global symbol in a SCIP-ok project, keyed `<manager>/<name>@<version>` (`maven/org.springframework/spring-web@6.1.13`, `npm/@prisma/client@5.22.0`, `maven/jdk@21`), and `Service -DEPENDS_ON-> Package` (`scip`/`exact`) from the referencing file's service with one evidence site. The version is the one the indexer saw, not a lockfile's; external `Symbol` stubs and edges into them are not built (a node per external symbol for no question yet asked). `rb service` lists the packages.

*As built in M4b (contract-m4b S15, S16, amendment 7; done 2026-10-07):* direct dependencies are also read from manifests: `package.json` with `package-lock.json` (v2/v3, Node's walk from the service directory; a workspace link takes its folder's version), `go.mod` `require` lines (not `// indirect`), and `pom.xml` `<dependencies>` literal or same-pom property versions. They give `Service -DEPENDS_ON-> Package` with provenance `manifest`, M3b's key form, `@?` when the version is unresolved, and attributes `declared`, `scope`, `version_from` (`exact` from a lockfile or literal, `synthetic` from a property or `go.mod`). Since amendment 21, an artifact declared twice with different versions, or a property defined twice, is `@?` with `versions`; an npm alias (`"foo": "npm:bar@^1"`) is keyed by the real package with `alias`; a `go.mod` `replace` applies by Go's rule (another version of the same module sets the version, `replaced`; a directory or another module gives `@?`). `pnpm-lock.yaml`, `yarn.lock` (`lockfile_unread`), Gradle and Python lockfiles are M6. A dependency on another service's package gives `Service -DEPENDS_ON{via=package}-> Service` (`synthetic` for a workspace, `file:` or local `replace`, `heuristic` otherwise). Measured: 128/128 sampled edges on seven public repositories match their manifests.

---

## 6. Call-graph construction

### 6.1 Edges produced directly

`CALLS`, `REFERENCES` and `IMPLEMENTS`, as described in §5.3.

### 6.2 Dynamic dispatch: CHA by default, RTA as a filter

A call through an interface or virtual method resolves (in SCIP) to the **declared** method, e.g. `UserRepository#findByEmail().`. The graph stores that single edge. It does not materialise the fan-out.

- **CHA at query time.** A traversal that reaches an abstract or overridable method also follows the reverse `IMPLEMENTS` edges to all overriding methods, marking each step with certainty `dispatch`. Storing edges once and expanding at query time keeps the edge table small. It also means the precision mode can be chosen per query.
- **RTA filter.** A type is **instantiated** if any of these holds:
  - an occurrence of its constructor exists (`new T`, a composite literal, or a Python class call);
  - it is a DI bean (§6.4);
  - it is named in a framework registration (routes, a `@Component` scan);
  - it is listed in reflection config (e.g. `META-INF/services`, GraalVM `reflect-config.json`).
  Each `IMPLEMENTS` edge carries `impl_instantiated: bool`. Queries default to `precision=cha`, which includes every overrider (contract-m4b amendment 21; the design first chose `rta`). `precision=rta`, opt-in, skips overriders of types proven never instantiated.

*As built in M4b (contract-m4b D4, S9, S10, amendment 9; done 2026-10-07):* a type counts as instantiated unless a proof of absence holds. Every mention of its name in the repository must be its declaration, an import or a supertype clause of a type that is itself a definition; there must be no string or text-file mention (binary names `Outer$Inner` split at `$`, every non-binary file read), no TS/JS module object that may name its module, and no annotation outside an inert list; it must not be default-exported, a test class, a Spring Data fragment or in a library service; and every subtype must also be proven. Java and TS/JS only. The attribute is written only as `impl_instantiated: false`. Pruned steps are counted in every answer (`pruned_by_rta`, the `rta:` line). Defaults: `cha` for every question; `rta` is opt-in (`--precision rta`, amendment 21: its header says it may hide callers reached through forms the scan cannot see). Measured: the fixture's never-referenced `KafkaAuditSink` is proven; no type is proven on eight public repositories (each kept by a named rule: a test's `includeFilters`, library services, annotations, mentions), which is the allow-list's recorded cost.

### 6.3 Provenance and certainty

Every edge row carries these fields:

| Field | Values |
|---|---|
| `provenance` | `scip`, `treesitter`, `heuristic`, `di`, `http_stitch`, `sql_literal`, `orm`, `codeowners`, `manifest`, `config` |
| `certainty` | `exact` > `dispatch` > `synthetic` > `heuristic` > `ambiguous` (ordered) |
| `evidence` | `(file_id, blob_sha, start_line, start_col, end_line, end_col)`. There may be several per edge; they are kept in `edge_evidence`. |

A transitive answer's certainty is the **minimum** certainty along its best path. Every answer returns a certainty histogram. Callers can require `min_certainty`.

### 6.4 Dependency injection (Spring first; Guice, Dagger, NestJS and .NET later)

1. Collect `Bean` facts:
   - stereotype-annotated classes, whose bean type is the class plus all its supertypes (known via `IMPLEMENTS` and extends);
   - `@Bean` methods, whose type is the declared return type.
2. For each `InjectionPoint(type T, qualifier q)`, the candidates are beans assignable to `T`. They are narrowed by the qualifier or bean name, then by `@Primary`, and matched with `List<T>`/`Map<String,T>` injection when the point asks for a collection.
3. Emit the results:
   - an `INJECTS` edge (consumer type → chosen bean type, provenance `di`);
   - an instantiation fact for the RTA filter;
   - `conditional=true` when the bean carries `@Profile`, `@Conditional*` or `@ConditionalOnProperty`.
4. If more than one candidate survives, all are emitted with certainty `ambiguous`. This mirrors what Spring itself would reject or resolve at runtime.
5. **Spring Data.** A repository interface without an implementation class gets a synthetic implementation. Its derived query methods (`findBy…`, `countBy…`, `existsBy…` → read; `save*` → write; `delete*` → delete) produce `QUERIES` edges to the entity's table, with provenance `orm` and certainty `synthetic`.

DI facts do **not** rewrite `CALLS` edges. A call on an injected field still targets the interface method. DI affects dispatch expansion only, through the instantiation set and the qualifier narrowing recorded on `INJECTS`.

*As built in M4b (contract-m4b D1–D3, D8, D9, S2–S8, amendments 1–8, 19; done 2026-10-07):*
- `INJECTS` goes from the class (or `@Bean` method) owning the injection point to the bean's definition: a stereotype class, a `@Bean` method, or a Spring Data or Feign interface.
- It is capped at `heuristic` (orchestrator decision F2): one candidate or a collection's candidates are `heuristic`, several `ambiguous`. The narrowing is data: `resolution` (`unique`, `qualifier`, `primary`, `name`, `collection`, `ambiguous`) and `capped_by` (the allow-listed gates that failed: scan roots, conditionals, the DI surface, class literals or strings naming a candidate, annotations not read, …), which also decide the `plain`/`capped` coverage buckets of the `di:` line.
- Conditional beans are `ambiguous` and flagged, never `synthetic`.
- A Spring Data repository is a bean of its own interface type: no synthetic node and no edge to the framework base. Its fragment implementations count as instantiated.
- Receiver-aware narrowing of dispatch is not planned.

### 6.5 Known blind spots

These are recorded as diagnostics, not guessed at:

- reflection (`Class.forName`, `getattr`, `obj[name]()`);
- `eval`;
- dynamic `import()` with non-literal specifiers;
- event buses and message topics (v2: a `PUBLISHES`/`SUBSCRIBES` pack keyed by topic literal);
- callbacks registered through third-party libraries;
- code generated at build time and not committed (e.g. Lombok, MapStruct, protobuf stubs). SCIP sees these when the indexer runs the annotation processors or codegen; Tree-sitter does not.

---

## 7. SQL and data-access extraction (`rb-sql`)

### 7.1 Sources of SQL

| Source | Detection |
|---|---|
| Migration and schema files (`*.sql`, Flyway `V*__*.sql`, Liquibase SQL changesets, Prisma migrations) | by path and extension |
| String literals passed to SQL sinks | `sql-sinks` pack (§4.5), certainty `exact` for the sink match |
| Other string literals that *look* like SQL | Regex prefilter `^\s*(WITH|SELECT|INSERT|UPDATE|DELETE|MERGE)\b` (case-insensitive). The literal only counts if it **also** parses (below), and it gets certainty `heuristic`. |
| `@Query(nativeQuery=true)` | Spring Data pack |
| ORM entity mappings | `jpa`, `prisma`, `typeorm`, `sqlalchemy`, `django` packs → `MAPS_TO` (entity type → Table) |

### 7.2 Parsing (as built in M2a)

1. **Normalise the template** (in `rb-syntax`, cached as `FileFacts.sql`):
   - each `${…}`, `+ expr +` or f-string hole becomes `$n`;
   - escapes are decoded (`\n`, `\xHH`, `\uHHHH`, `\u{…}`, octal);
   - Java text blocks are dedented;
   - a candidate must start with a whole SQL keyword (`SELECT`, `INSERT`, `UPDATE`, `DELETE`, `WITH`, `MERGE`, `CREATE`, `ALTER`, `DROP`, `TRUNCATE`);
   - annotations and decorators are skipped, because they are framework metadata.
2. **Split statements on Postgres' own lexer tokens** (`rb-sql`). Quotes, dollar quotes, `E''` strings and comments are single tokens, so they can never be mistaken for boundaries.
   - A `;` token ends a statement, except inside `BEGIN ATOMIC … END` bodies, which are tracked by counting `BEGIN ATOMIC`/`CASE`/`END`.
   - A `psql` meta-command (a `\` token to the end of its line) also ends one, as `\gset` does. `\copy …` is kept as a `COPY` statement.
   - A range runs from the statement's first token to its last, so it is never empty or inverted, even around whitespace that Rust and Postgres disagree on, such as U+00A0.
   - One bad statement never hides the rest. If the lexer fails (e.g. an unterminated quote), the statements before the failure point are kept. The failure point comes from the lexer's own error, and only the interrupted statement plus the rest of the text is reported.
   - `pg_dump` data blocks (after `COPY … FROM stdin;`, up to `\.`) are data, not SQL.
   - `;` inside the parentheses of a multi-action `CREATE RULE` does not split it.
   - *History:* an earlier version combined `split_with_parser`, `split_with_scanner` and a coverage check. The third evaluator pass showed the lexer split silently drops statements that start with a non-keyword, and that mixing Rust and Postgres whitespace rules could invert a range.
3. **Parse with pg_query** (the real PG17 parser). If a statement fails, it is retried once with JDBC `?`, named `:param` and DB-API `%s`/`%(name)s` placeholders rewritten to `$n`.
   - Placeholders are found as lexer *tokens*, so comments, strings, `E''` escapes and the `?|`/`@?` operators are never touched.
   - `id=?` lexes as one `=?` operator token, so placeholders glued to a comparison are split off.
   - A `?` glued to a word (`"Delete from cache?"`) is prose, not a placeholder.
4. **Walk the whole parse tree** (serialised to JSON via the protobuf structs' serde support, so no node type is skipped). M2a showed three problems with the alternatives:
   - pg_query's `*_tables()` helpers lose CTE scope, give nested DML the top-level operation, and return dotted strings.
   - Its `nodes()` iterator skips `TABLESAMPLE`, `RETURNING`, `ON CONFLICT`, `MERGE` actions, `PREPARE` and cursors.
   - Each statement node contributes its own target with its own operation, including DML nested in CTEs:
     - `read`, `insert`, `update`, `delete`, `merge` for DML;
     - `create`, `alter`, `drop`, `truncate` for DDL;
     - `COPY FROM` is `insert` and `COPY TO` is `read`;
     - `SELECT INTO` is `create`;
     - `ALTER TABLE … RENAME` is `alter`.
   - Other relations are reads, but only in query-bearing statements. `FOR UPDATE OF alias` names an alias, not a table. `UNION` arms are walked as statements, so their own `WITH` clauses scope correctly.
   - `CREATE TABLE … PARTITION OF / INHERITS parent` records `alter` on the parent. `CREATE FOREIGN TABLE` records `create`, and `ALTER TABLE … SET SCHEMA` records `alter`. `GRANT`, `LOCK`, `COMMENT` and FK `REFERENCES` in `CREATE TABLE` record nothing.
   - CTE names are scoped as in Postgres. A CTE hides a same-named table only in later sibling CTEs and the main query (and its own body when `RECURSIVE`), never in outer queries.
   - `ALTER TYPE` (a composite type) is not a table.
   - System catalogs are ignored, including unqualified `pg_*` names, because `pg_catalog` is searched first.
   - Quoted identifiers keep their dots. Table keys double-quote parts that contain `.` or `"`, so `"a.b".c` and `a."b.c"` never collide.
5. **Failures:** a statement that does not parse gives `unparsed_sql` for `.sql` files and for sink-call arguments. Elsewhere it is dropped, because it is almost always prose.
   - There is no dialect fallback yet (`sqlparser-rs` is deferred). MySQL/H2 statements are reported, never guessed.
   - JPQL is left to M2b: arguments to `createQuery` are not treated as SQL.

Known gaps:
- `DROP VIEW` and `CREATE TABLE … (LIKE t)` report no table.
- Tables used inside `$$`-quoted function bodies (SQL or PL/pgSQL) are not analysed. Only `BEGIN ATOMIC` bodies are.
- FK `REFERENCES` dependencies are not recorded.
- Unqualified names always assume `public` (`rb.toml` `sql.default_schema` is not implemented yet).

- *As built in M9b (contract-m9b D6):* a Go SQL string passed as the format of `fmt.Sprintf` folds package constants into its holes before parsing. `%s` and `%v` each take the next argument, and `%%` is `%`. Every argument must be a plain name naming a top-level `const` that is untyped or declared `string`, with a one-literal value, in a non-test file, declared exactly once at package level (any kind: `const`, `var`, function, type; all files of the package, tests included). No other binding of the name may be in effect at the argument; the constant's own declaration in the call's file is not a shadow. `fmt` must be the standard package: the chain's root, imported under that name, with no import or binding giving the name to anything else. Any other verb, flag, width or index, a count mismatch, or any refusal leaves the literal as it was. Certainty is unchanged: `Sprintf` is no sink, so the `QUERIES` are `heuristic`. Each file with a folded site gets one `sql_folded` diagnostic `{sites, constants}`. A non-constant hole (`SET %s` built at run time) stays unfolded, and so does a constant of a named type (`const t Tbl = "x"`), because `%s` prints it through its `String()`, `Error()` or `Format` method (evaluator pass 1, E3).

### 7.3 Tables as first-class nodes

- The table key is `schema.table`.
- An unqualified name resolves to the schema configured in `rb.toml`, defaulting to `public`. It is flagged `schema_assumed` because `search_path` is not visible statically.
- DDL in migrations produces the canonical table set. Tables referenced in code but never created in migrations are kept, flagged `undeclared`. These are often views or tables owned by another repo.
- Edges:
  - `Symbol -QUERIES{op}-> Table`, where the symbol is the innermost enclosing function of the SQL literal or sink call;
  - `Symbol(type) -MAPS_TO-> Table`, for ORM entities;
  - `File(migration) -MIGRATES{op}-> Table`.

### 7.4 ORM mappings (as built in M2b)

`rb-syntax` records framework-neutral facts: decorations with arguments, supertypes with type arguments, type parameters, class constants, and call qualifiers. Every argument and constant carries a `literal` flag (a static string; concatenations of literals are flattened). `rb-frameworks` turns these facts into mappings and accesses with pure functions. The indexer then resolves them across the whole snapshot.

| ORM | Table name | Notes |
|---|---|---|
| JPA | `@Table(name, schema)`, else the entity name (`@Entity(name)` or the class name) | Written as Hibernate's `PhysicalNamingStrategySnakeCaseImpl` writes it. That is Spring Boot 4's default (Boot 3.x's `CamelCaseToUnderscoresNamingStrategy` also lowercases quoted names), and it applies to explicit names and schemas too. Quoted names (`"x"`, `` `x` ``) are kept exactly. Otherwise `.` becomes `_`, an underscore is inserted before an interior capital that has a lowercase letter or digit on both sides, and the name is lowercased. Inserted underscores are flagged `name_assumed`. |
| JPA inheritance | a subclass entity shares its root entity's table | This is SINGLE_TABLE, the default; Hibernate ignores `@Table` on such a subclass (HHH000139). With `@Inheritance(JOINED \| TABLE_PER_CLASS)` on the root, each class has its own table. The walk follows `extends` (never `implements`) through every Java class, including `@MappedSuperclass`. A superclass resolves by simple name: the only class with that name, else the only one in the same directory. The edge is `synthetic` and carries `inherits`. |
| TypeORM | `@Entity('x')` / `@Entity({name, schema})`, else `snakeCase(class)` | Names are quoted, so case is kept. |
| SQLAlchemy | `__tablename__`; for Flask-SQLAlchemy `db.Model`, `camel_to_snake_case(class)` | Flask's rule differs from TypeORM's (`My_Table` becomes `my__table`). `__abstract__ = True` and `__tablename__ = None` (single-table inheritance) get no table. A Flask class reaching `db.Model` through abstract bases only gets its own table; a subclass of a concrete model shares its parent's. |
| Django | `Meta.db_table`, else `<app_label>_<model>` | A model extends `models.Model` (bare `Model` is not recognised until there are import facts), or a model of the same file. `app_label` and `db_table` follow Django's Meta inheritance: a class without its own `Meta` inherits the whole `Meta` of its first abstract parent, and `class Meta(Base.Meta)` extends it. Otherwise the app label is the directory above the last `models` package, or the file's directory. A `db_table` of `'"auth"."users"'` means `auth.users`. `abstract` models get no table. A `proxy` uses its parent's table; if the parent is not in the file, the result is `orm_unresolved_proxy_parent`. Multi-table-inheritance children get their own table. |
| Prisma | `@@map("x")` / `@@map(name: "x")`, `@@schema`, else the model name | An accessor resolves to the schemas whose directory shares the most leading path segments with the file. Several distinct tables make the result `ambiguous`. |

Accesses:
- **Spring Data:** a repository gets inherited CRUD access; declared methods get derived-query access, native `@Query` (exact, via `rb-sql`) or JPQL (heuristic).
  - The inherited CRUD is one `QUERIES` row from the repository type itself: all four ops, `synthetic`, provenance `orm`. That row stands for the CRUD the type inherits, not for a query it states (contract-m9a D5). Which inherited operation each caller uses is not recorded; that stays open.
  - Type arguments are resolved as *class* names; JPQL names are *entity* names.
  - Generic base repositories are resolved per name, memoised, with a cycle guard: a name binds the index of its entity type parameter only if every declaration of it agrees (a built-in base counts as a declaration). Otherwise it is dropped. `org.springframework.data.…` supertypes always mean the built-in base.
- **JPQL:** entities come only from clause-level FROM and JOIN, at the top level or in a subquery. Function arguments, `IN(…)` and `TREAT(…)` are never entities. Comments and SpEL are skipped. Concatenated queries are read from their flattened text, where a word holding a `$n` hole is never an entity. A native query's `$n`-glued table names are never tables, and its other tables become `heuristic` when the text is not static.
- **Prisma client calls:** these count only when the receiver segment before the model accessor contains `prisma` or is `db`, `tx`, `client`, `trx` or `database`.

Diagnostics:
- `orm_dynamic_table_name`: the name is not a static string, so no table is invented;
- `orm_unresolved_proxy_parent`;
- `orm_unresolved_entity`;
- `unparsed_sql` with callee `Query`: native SQL that `rb-sql` cannot parse.

Declarations that share a symbol key are aggregated per (symbol, table) rather than conflicting. This happens, for example, with a class defined in both branches of an `if`.

---

## 8. Storage: PostgreSQL + Apache AGE

### 8.1 Deployment

- A single Postgres instance with the AGE extension, from the official `apache/age` image. The tag is `release_PG18_1.8.0`, pinned **by digest**. `latest` is not the same image as any release tag, so never use it.
  - AGE is released per PG major: PG18 → 1.8.0, PG17 → 1.7.0, PG16 → 1.6.0.
  - **PG18 + AGE 1.8.0** is chosen because 1.8 adds `age_shortest_path` / `age_all_shortest_paths`. Those functions are absent from the PG17 1.7 line (§8.4).
  - pg_query (§7) embeds the **PG17** parser. SQL that relies on PG18-only syntax may fail to parse; it is recorded as `unparsed_sql`, never guessed.
  - The host's Homebrew PG 14 is not used.
- AGE is loaded per session with `LOAD 'age'; SET search_path = ag_catalog, "$user", public;`. The server's connection pool does this in an `after_connect` hook.
- The relational schema lives in schema `rb`. Each graph lives in its own AGE graph namespace.

### 8.2 Natural keys

| Node kind | Natural key |
|---|---|
| `Symbol` | SCIP symbol string. Locals are not nodes; they never cross files and are not needed for any of Q1–Q4. |
| `File` | repo-relative path |
| `Service` | the project **directory** (`.` for the repo root, which is `role: unassigned` when no project is detected there). Names can collide, so name lookups return candidates instead of guessing (§9.1). |
| `Table` | `schema.table` |
| `Owner` | `@user`, `@org/team`, or email, verbatim from CODEOWNERS |
| `Endpoint` | `HTTP <METHOD> <normalised path>` (path parameters → `{}`), or `GRPC <pkg.Service>/<Method>`. *As decided in contract-m4a (D1):* HTTP keys end in ` @<service key>`, e.g. `HTTP POST /api/v1/login @services/auth`, so two services' `GET /health` stay distinct; the display name is `POST /api/v1/login`. *As decided in contract-m4a (amendment 10, fork F3):* an endpoint whose prefix is unknown (a router mounted nowhere visible, a gin group passed as a parameter) is keyed by its tail plus ` ?router=<file>#<binding>`, e.g. `HTTP GET / @. ?router=users/routers.go#UserRegister.router`, so routes with one tail on different routers stay distinct; known-prefix keys are unchanged. |
| `Package` | `<manager> <name>`; the version sits on the edge. *As built in M3b and kept by contract-m4b:* `<manager>/<name>@<version>`, `@?` when unknown. |

### 8.3 Relational schema (source of truth)

The executable version is [`migrations/0001_init.sql`](../migrations/0001_init.sql), and it is authoritative if the two ever disagree. It adds `ON DELETE CASCADE` rules and a repo-name check (`[a-z0-9_]{1,40}`, because repo names end up in AGE graph names).

```sql
CREATE SCHEMA rb;

CREATE TABLE rb.repo (
  id               bigserial PRIMARY KEY,
  name             text NOT NULL UNIQUE,
  origin_url       text,
  live_snapshot_id bigint                     -- FK added below; flipped atomically after a successful load
);

CREATE TABLE rb.snapshot (                     -- one per indexed commit
  id            bigserial PRIMARY KEY,
  repo_id       bigint NOT NULL REFERENCES rb.repo,
  commit_sha    text   NOT NULL,
  parent_id     bigint REFERENCES rb.snapshot,  -- snapshot this one was incrementally derived from
  status        text   NOT NULL CHECK (status IN ('building','live','retired','failed')),
  tool_versions jsonb  NOT NULL,              -- grammars, packs, indexer digests, pg_query version
  coverage      jsonb,                         -- §12.4
  age_graph     text,                          -- e.g. rb_myrepo_42
  created_at    timestamptz NOT NULL DEFAULT now(),   -- metadata only; never part of graph output
);
-- Not unique on (repo, commit, tools): a forced rebuild of the same inputs is a new snapshot, and the
-- determinism tests compare exactly such pairs. The indexer skips a rebuild unless --force is given.
CREATE UNIQUE INDEX snapshot_one_live ON rb.snapshot (repo_id) WHERE status = 'live';
ALTER TABLE rb.repo ADD FOREIGN KEY (live_snapshot_id) REFERENCES rb.snapshot;

CREATE TABLE rb.node (
  snapshot_id bigint NOT NULL REFERENCES rb.snapshot ON DELETE CASCADE,
  id          bigint NOT NULL,                 -- dense per snapshot, assigned in sorted key order
  kind        text   NOT NULL,                 -- symbol|file|service|table|owner|endpoint|package
  key         text   NOT NULL,                 -- natural key (§8.2)
  name        text   NOT NULL,                 -- display name, e.g. "login" / "AuthService#login()"
  file_id     bigint,                          -- defining file for symbols
  span        int4[],                          -- {start_line,start_col,end_line,end_col}
  attrs       jsonb  NOT NULL DEFAULT '{}',    -- symbol kind, signature, doc, flags (external, generated, undeclared…)
  PRIMARY KEY (snapshot_id, id),
  UNIQUE (snapshot_id, kind, key)
);
CREATE INDEX node_name_trgm ON rb.node USING gin (name gin_trgm_ops);   -- pg_trgm for name search (§9.1)
CREATE INDEX node_key_trgm  ON rb.node USING gin (key  gin_trgm_ops);

CREATE TABLE rb.edge (
  snapshot_id bigint   NOT NULL,
  src         bigint   NOT NULL,
  dst         bigint   NOT NULL,
  kind        text     NOT NULL,               -- §8.5
  provenance  text     NOT NULL,
  certainty   smallint NOT NULL,               -- 5 exact … 1 ambiguous
  attrs       jsonb    NOT NULL DEFAULT '{}',  -- op for QUERIES, impl_instantiated, conditional, candidates…
  PRIMARY KEY (snapshot_id, src, kind, dst, provenance),
  FOREIGN KEY (snapshot_id, src) REFERENCES rb.node,
  FOREIGN KEY (snapshot_id, dst) REFERENCES rb.node
);
CREATE INDEX edge_in  ON rb.edge (snapshot_id, dst, kind) INCLUDE (src, certainty);
CREATE INDEX edge_out ON rb.edge (snapshot_id, src, kind) INCLUDE (dst, certainty);

CREATE TABLE rb.edge_evidence (
  snapshot_id bigint NOT NULL,
  src bigint NOT NULL, kind text NOT NULL, dst bigint NOT NULL, provenance text NOT NULL,
  file_id bigint NOT NULL,
  span    int4[] NOT NULL,
  FOREIGN KEY (snapshot_id, src, kind, dst, provenance) REFERENCES rb.edge ON DELETE CASCADE
);

CREATE TABLE rb.file_facts_cache (             -- content-addressed; shared across snapshots and repos
  blob_sha   text  NOT NULL,
  extractor  text  NOT NULL,                   -- extractor + pack versions hash
  facts      bytea NOT NULL,                   -- JSON FileFacts (M1b; bincode+zstd only if size demands it)
  PRIMARY KEY (blob_sha, extractor)
);

CREATE TABLE rb.diagnostic (                   -- unresolved calls, unparsed SQL, failed builds, blind spots
  snapshot_id bigint NOT NULL, kind text NOT NULL, file_id bigint, span int4[], detail jsonb
);
```

Loading uses `COPY … FROM STDIN (FORMAT binary)` into the `building` snapshot. The last step, `UPDATE rb.repo SET live_snapshot_id = …`, runs in a single transaction, so readers never see a partial graph. Retired snapshots are kept for N runs (default 3) to enable diffing (§11), then dropped. A partition-per-snapshot scheme (v2) would make that drop a cheap `DROP TABLE`.

*As decided and measured in contract-m5 (D10, S1, S3, S24, amendment 14):* the snapshot row is written inside the publish transaction (a failure records a separate `failed` row after the rollback, so no `building` row survives its process); every read of a snapshot is one `REPEATABLE READ READ ONLY` transaction; retention keeps the live snapshot plus the `N` newest retired ones (`--keep`, env `RB_KEEP_RETIRED`, default 2, so three in all) and removes `failed` rows; `rb gc` removes what older binaries or interrupted runs left, never implicitly. The publish ends with `ANALYZE` of `node`, `edge`, `edge_evidence` and `diagnostic` before its commit: without it, reads in the minute after a large publish planned a nested loop on stale statistics (a dump's evidence query on a 74 k-node snapshot ran over 10 minutes; the same query takes about 1 s with current statistics). Measured on the benchmark (Apple M4, Postgres 18 + AGE 1.8 in Docker Desktop): one snapshot is 173 MiB of rows (apache/shardingsphere, 74 k nodes, 233 k edges) and 121 MiB (mattermost, 59 k nodes, 169 k edges) after `VACUUM FULL`; retention's delete of one snapshot takes 1.40 s and 1.27 s. Partition-per-snapshot is therefore not planned.

### 8.4 The AGE projection and what it is for

For each live snapshot, the loader creates a graph `rb_<repo>_<snapshot_id>`, with vertex labels `Symbol, File, Service, Table, Owner, Endpoint, Package` and one edge label per edge kind. Each vertex carries only `{nid, key, name}`. Each edge carries `{provenance, certainty}`, plus `op` on `QUERIES` (*as built in contract-m5, amendment 3:* `ops`, the relational array, e.g. `['read', 'insert']`, on `QUERIES` and `MIGRATES`; absent elsewhere). Everything else is looked up relationally by `nid`, so the projection stays thin and cheap to rebuild.

- **Bulk load.** The loader exports per-label CSV from the relational tables and loads it with AGE's `load_labels_from_file` / `load_edges_from_file`. The CSV files must be readable by the Postgres server process, and AGE 1.8 only reads loader paths under the server's `/tmp/age/`. So `compose.yaml` mounts `./var/age-load` at `/tmp/age/rb-load`, and the indexer writes there. The files use `load_as_agtype` mode: each property cell is a JSON value, so numbers stay numeric (`e.certainty >= 5` works) and strings round-trip exactly. Vertex CSV ids are the relational `nid`s, which may be sparse. The whole load (rows, AGE graph and live flip) runs in **one transaction**. A failed load leaves only a `failed` snapshot row, and retention removes that row later. It never issues row-by-row `CREATE` through `cypher()`, which is orders of magnitude slower. The projection is built *before* the live flip, and the old graph is dropped (`drop_graph(name, true)`) when its snapshot is retired. *As decided and measured in contract-m5 (D10, S2, S6, S24):* until M5 the old graph was kept until retention deleted its row; M5 drops it right after the flip, one graph per transaction (80 graphs in one transaction exhaust the default lock table), and every `create_graph`/`drop_graph` takes a database-wide advisory lock (§14); edges carry `ops` where the relational edge has one, and parity is checked over the whole vertex set and edge multiset with their properties, on every fixture and on both benchmark repositories (74,317 vertices / 193,637 edges and 58,521 / 168,929: equal). The bulk load measured 1.17 s for 307 k rows (26.7 MB of CSV) and 0.77 s for 227 k rows; the graph adds 64 MiB and 44 MiB to the snapshot. `rb index --profile` prints the load per label file.

**What AGE 1.8 gives us, and where it hurts:**

- **Supported:** `MATCH` with variable-length `[*a..b]`, and `MERGE`.
- **No openCypher `shortestPath()` pattern syntax.** Instead, 1.8 ships the SQL functions `age_shortest_path(...)` and `age_all_shortest_paths(...)` (hop-count BFS). They support only a single edge type and no `min_hops`.
- **Parameters** work only through prepared statements: the third `cypher()` argument must be a bound `$1` agtype map.
- **Variable-length expansion is costly.** AGE's own `age_vle.c` documents unbounded `[*]` as worst-case O(E!) and recommends bounding the depth.
  - Issue #2350 (open) benchmarks a shortest-path query at ~23 ms, against ~0.1–0.3 ms for a BFS recursive CTE.
  - Issue #2481 (open) reports a 6× slowdown from a redundant type check.

All of this confirms the division of labour below.

**Division of labour** is the key design decision here:

| Workload | Engine | Why |
|---|---|---|
| Q1–Q3 transitive closure, Q4 downstream tree | **rb-server in-memory CSR**. Fallback: set-semantic recursive SQL. | Reachability needs node-set semantics with a visited set. openCypher's `[*1..n]` has path semantics: it enumerates distinct *paths* (edge-isomorphic), which grows exponentially on dense call DAGs even when the reachable node set is small. |
| 1-hop lookups, Service Card sections | Relational SQL | Plain indexed joins. |
| Ad-hoc exploration, multi-hop *pattern* queries, architecture rules | **AGE / Cypher** | Patterns are where Cypher is clearly better than SQL. Examples: "controllers that reach a `Table` without passing through a `@Service`", "services whose symbols `CONSUMES` an endpoint of a service they also share a table with". |

**In-memory CSR.** At a live flip, rb-server loads `(src, dst, kind, certainty)` into two compressed sparse row arrays, one outbound and one inbound, each partitioned by edge kind. Sizing: 10 M edges × (4 B dst + 1 B kind + 1 B certainty) ≈ 60 MB per direction. BFS over that is sub-millisecond to low-millisecond. Postgres remains the durable store, and CSR is a cache rebuilt from it. *As built in contract-m5 (S9, D3, S25):* `rb_store::code::SnapshotIndex` holds the inbound and outbound adjacency as CSR arrays of edge indices, built once per snapshot (per CLI invocation; once per live snapshot in `rb serve`, rebuilt when the live pointer moves). Measured: 0.57 s to read and index shardingsphere's SCIP snapshot (74 k nodes, 194 k edges), 0.63 s for mattermost's; the server's peak RSS 284 MiB and 249 MiB. Every question walks the index under a deterministic work budget (20,000,000 counted operations), so the SQL fallback below is not used.

**SQL fallback (embedded CLI mode, no server).** For reachability:

```sql
-- all transitive callers of :target (set semantics: UNION dedups against all prior rows → terminates on cycles)
WITH RECURSIVE up(id) AS (
    SELECT :target::bigint
  UNION
    SELECT e.src
    FROM rb.edge e JOIN up ON e.dst = up.id
    WHERE e.snapshot_id = :snap AND e.kind IN ('CALLS','REFERENCES') AND e.certainty >= :min_cert
)
SELECT n.* FROM up JOIN rb.node n ON n.snapshot_id = :snap AND n.id = up.id;
```

Distance, when needed, is computed afterwards by a bounded BFS in the client over the returned set. The CTE deliberately avoids path arrays and `CYCLE` bookkeeping; that bookkeeping is what makes CTE traversal slow.

### 8.5 Graph schema

| Edge | From → To | Producer | Notes |
|---|---|---|---|
| `DEFINED_IN` | Symbol → File | SCIP / Tree-sitter | |
| `IN_SERVICE` | File → Service | boundaries | |
| `CONTAINS` | Symbol → Symbol | SCIP descriptors / Tree-sitter scopes | class → method; needed to roll methods up to classes |
| `CALLS` | Symbol → Symbol | SCIP ⋈ TS, heuristic | targets the declared method (§6.2) |
| `REFERENCES` | Symbol → Symbol | SCIP ⋈ TS | non-call uses, incl. function-as-value and type use |
| `IMPLEMENTS` | Symbol → Symbol | SCIP relationships | type → interface; method → overridden method. `impl_instantiated` |
| `INJECTS` | Symbol → Symbol | DI pack | `conditional`, `qualifier` |
| `QUERIES` | Symbol → Table | SQL, Spring Data | `op` |
| `MAPS_TO` | Symbol → Table | ORM packs | |
| `MIGRATES` | File → Table | SQL | `op` (create/alter/drop) |
| `EXPOSES` | Service → Endpoint | route packs | |
| `HANDLED_BY` | Endpoint → Symbol | route packs ⋈ SCIP | |
| `CONSUMES` | Symbol → Endpoint | http-clients + stitching (§10) | `match=exact|suffix|ambiguous` |
| `OWNED_BY` | File/Service → Owner | CODEOWNERS | |
| `DEPENDS_ON` | Service → Service/Package | derived (§9.5), lockfiles | `via` = calls/http/package. *As built in M4b (contract-m4b D6):* Service → Package (provenance `scip` or `manifest`) and Service → Service `via=package` are stored; `via=calls`/`via=http` are derived at query time. |

**Producer-coverage invariant** (checked in CI, §12): every edge kind in this table has at least one producer and at least one golden fixture asserting it.

---

## 9. Query layer

All queries run against the repo's live snapshot unless `?snapshot=` is given. Every response includes `snapshot_id`, `commit_sha`, a certainty histogram and a `truncated` flag.

### 9.1 Resolving user input to nodes (shared by all questions)

Users type `login`, `AuthService.login`, `src/auth/service.ts:42` or a full SCIP symbol. Resolution:

1. An exact SCIP key match wins.
2. For `path:line`, find the innermost `Symbol` whose `span` contains the line in that file.
3. Otherwise, run a trigram search on `name` and a suffix search on `key`, filtered by `kind` where the command implies one.
4. **If more than one candidate remains, return the ranked candidate list and stop.** The engine never picks one silently. The CLI offers `--pick N`.

### 9.2 Q1 — "What calls this function?"

1. Resolve the target `t`.
2. **Direct callers:** inbound `CALLS` to `t`.
3. **Dispatch callers:** walk forward along `IMPLEMENTS` from `t` to every method it implements or overrides, transitively. Inbound `CALLS` to those methods are callers that *may* reach `t`. They are certainty `dispatch`, and under RTA they count only if `t`'s owner type is instantiated.
4. **Optionally transitive** (`--depth N`, default 1, max 25): BFS upward, recording distance and predecessor for each node, so the response forms a caller tree.
5. **Optionally** `--include-references`, which adds inbound `REFERENCES` (callers through function values).

```sql
-- ad-hoc equivalent (1 hop, exact only). AGE parameters work only via a prepared statement.
PREPARE callers(agtype) AS
SELECT * FROM cypher('rb_myrepo_42', $$
  MATCH (c:Symbol)-[e:CALLS]->(t:Symbol {key: $key})
  WHERE e.certainty >= 5
  RETURN c.key, c.name, e.certainty
$$, $1) AS (key agtype, name agtype, certainty agtype);
EXECUTE callers('{"key": "scip-java maven com.acme:auth 1.4.0 com/acme/auth/AuthService#login()."}');
```

**Output:** a caller tree. Each node shows its file:line, service and certainty. Entry points (endpoint handlers, `main`, test methods, scheduled jobs) are marked as tree roots.

*As built in M4b (contract-m4b D10, S11, amendment 10; done 2026-10-07):* entry kinds are `endpoint`, `top-level` (query time) and `main`, `test`, `scheduled`, `listener`, `lifecycle` (stored, import-gated, as the `Symbol` attribute `entry`). `rb callers` marks them wherever they appear (`(entry: …)`), and expansion continues past them.

### 9.3 Q2 — "What breaks if I remove this?"

"Breaks" has two strengths, and they are reported separately:

| Tier | Definition | Meaning |
|---|---|---|
| **Compile / link break** (distance 1) | nodes with a *requiring* edge into `t`: `CALLS`, `REFERENCES`, `IMPLEMENTS` (a type implementing interface `t`), `INJECTS`, `MAPS_TO`/`QUERIES` (when `t` is a Table), `HANDLED_BY` (when `t` is a handler) | these stop compiling, or fail at startup or query time |
| **Behavioural impact** (distance ≥ 2) | inbound reachability from the tier-1 set over `CALLS ∪ REFERENCES` (+ dispatch expansion) | these may change behaviour at runtime |

If `t` is a class, the target set is `t` plus all its members (via `CONTAINS`), so removing a class covers its methods too.

**Roll-ups** over the impacted set `I`:

- **Endpoints:** endpoints `HANDLED_BY` any node in `I`. These are the user-visible APIs affected.
- **Services:** services of the files that define nodes in `I`, split into "`t`'s own service" and "other services". The latter are the cross-team blast radius.
- **Owners:** owners of those files, counting impacted symbols per owner.
- **Tests:** symbols in test source sets (by path convention or pack) that are in `I`. These are the tests to run.

**Orphan detection** (what becomes dead code): run a forward BFS from `t` over `CALLS ∪ REFERENCES` within the same service. A reached node `n` becomes an orphan if **all** of its inbound requiring edges come from the removed set ∪ orphans, and `n` is not an entry point. The orphan set is computed as a fixpoint.

*As built in M4b (contract-m4b D11, S12, S13, amendments 10–11; done 2026-10-07):* orphans are the candidates not reachable from live seeds (every requiring edge kind and dispatch carry liveness), which is the greatest fixpoint, so dead cycles are found. They are computed only in deployable services. A symbol can be an orphan only when it is `orphan_eligible`: every whole-word mention of its name in the repository's text is accounted for by an edge into it, it carries no non-inert annotation, its name is not convention-invoked, it is not default-exported, and it is not a Go method. `rb impact` lists `orphans: may become dead code (n)` and `tests (n)` and takes `--json`. Measured: 12 of 12 orphans listed on sampled public repositories have no other use.

**Defaults:** `precision=cha` for Q2, because under-reporting is the dangerous failure here. Depth is unlimited within the CSR's visited-set BFS, capped at 200 k nodes, with `truncated=true` if the cap is hit.

### 9.4 Q3 — "Which services depend on this table?"

1. Resolve the table: accept `users`, `public.users` or `auth.users`, matched against `Table` keys.
2. **Data-access symbols:** inbound `QUERIES` (with `op`) plus, for `MAPS_TO` entities, the symbols that `REFERENCES`/`CALLS` members of the entity's repository or DAO. That is the ORM path.
3. **Callers up to entry points:** a transitive inbound BFS over `CALLS` (+ dispatch) from the data-access symbols.
4. **Service roll-up:** group all reached symbols by service. For each service, report:
   - `ops` (the union of read/insert/update/delete);
   - `direct` (the service itself contains a data-access symbol) versus `indirect` (it reaches the table only through another service's library code, for monorepo shared modules);
   - the endpoints in that service that reach the table.
5. Services that reach the table only through `CONSUMES` into another service are **not** listed as table dependents. Instead they are shown as "`X` depends on this table through `POST /api/users` of `user-service`". This distinction matters for ownership.
6. **Migrations:** list which files and services `MIGRATES` the table. This answers "who owns the schema".

*As built in M4b (contract-m4b D5, D12, S14, amendment 12; done 2026-10-07):* Q3 defaults to `precision=cha`; the caller walk has depth cap 25 and node cap 200,000; per service it lists operations, `direct`/`indirect via`, endpoints, other entry kinds and the best certainty. Callers include `REFERENCES`. Endpoints reaching the table respect inline-handler spans. Services reaching the table only through an API are listed with the endpoint chain, up to `--hops`.

### 9.5 Service Card

```
auth-service  (path: services/auth, project: maven :auth-service, snapshot 42 @ a1b2c3d)
├─ APIs            (EXPOSES → Endpoint, with HANDLED_BY symbol)
│    POST /api/v1/login        → AuthController#login()        [exact]
│    GET  /api/v1/session      → SessionController#current()   [exact]
├─ Database tables (QUERIES/MAPS_TO from symbols IN_SERVICE; op union)
│    public.users           read, update
│    public.sessions        read, insert, delete
│    public.oauth_tokens    read, insert        (schema_assumed)
├─ Owners          (CODEOWNERS for services/auth/**; share of files)
│    @org/backend-security  96%   ·  @org/platform  4% (build files)
│    contributors (90d): 5 identities
├─ Dependencies
│    services:  notification-service  via HTTP POST /internal/mfa/email   [exact]
│               shared-lib (monorepo) via CALLS  12 call sites            [exact]
│    shares tables with: user-service (public.users)                     ← coupling, not dependency
│    packages:  org.springframework.boot 3.3.4, io.jsonwebtoken:jjwt 0.12.6, …
├─ Depended on by
│    gateway            via HTTP (CONSUMES POST /api/v1/login)
│    web-frontend       via HTTP (CONSUMES POST /api/v1/login, GET /api/v1/session)
└─ Call graph      (CALLS/IMPLEMENTS/INJECTS with both ends IN_SERVICE; rendered as DOT/JSON)
     312 symbols · 1,048 edges · certainty: exact 97.1%, dispatch 2.3%, heuristic 0.6%
```

The Dependencies section is built from these derived service edges:

- `Service -DEPENDS_ON{via=calls}->` from a `CALLS`/`REFERENCES` edge whose ends lie in different services;
- `{via=http}` from `CONSUMES` into an `Endpoint` that another service `EXPOSES`;
- `{via=package}` from lockfiles.

Shared tables are reported as **coupling**, deliberately kept separate from dependency.

*As built in M4b (contract-m4b S15–S18, amendment 13; done 2026-10-07):* `via=calls` and `via=http` are computed at query time from `CALLS`/`REFERENCES` and `CONSUMES` edges crossing services. `via=package` comes from manifests. The card adds Packages with versions, Database tables, Dependencies, Depended on by, Shares tables with, and a Call graph summary with entry points by kind.

### 9.6 Q4 — "Show the request path for login"

Q4 is covered in §10, since it needs cross-service stitching. It is built in M4a (`tasks/contract-m4a.md`, done 2026-10-06): `rb path` with text and JSON output, goldens on `polyglot-monorepo` (web TypeScript → auth Java → notification TypeScript for `POST /api/v1/login`, plus a Go caller of `POST /internal/mfa/email`), and a SCIP-mode golden in which the controller → Feign hop is `exact`.

---

## 10. Cross-service request paths (Q4)

### 10.1 Endpoint normalisation

Server routes and client URLs are normalised to one form:

- Method is upper-cased. Path parameters (`:id`, `{id}`, `<int:id>`, `*`) become `{}`.
- Duplicate slashes collapse, and trailing slashes are stripped.
- Configured global prefixes are prepended. Examples: `server.servlet.context-path` from `application.yml`/`.properties`, NestJS `setGlobalPrefix` and Express mount prefixes. Profile-specific values (`application-prod.yml`) produce one variant per profile, flagged.
- Gateway rewrite rules are applied when their config is present (nginx `location`/`proxy_pass`, k8s Ingress paths, Spring Cloud Gateway routes, v1.x). Each rewrite adds an `Endpoint` alias.

*As decided in contract-m4a (S3, D5):*
- Catch-alls (`*`, `**`, `{*rest}`, `{rest...}`, `<path:p>`) become `{*}`, last segment only. Go's `{$}` is dropped.
- Query strings and fragments are dropped; a space is percent-encoded.
- A client hole inside a segment makes the segment `{}`, flagged `partial`.
- M4a reads `src/main/resources/application.properties` only. YAML, profile files and gateway configs are deferred, and a Spring YAML file mentioning `context-path` is reported as `spring_config_unread`. A service with profile files, YAML, `bootstrap*` files or a Spring Cloud Config client gets one `config_unread` diagnostic; its endpoints are flagged `prefix_maybe_incomplete` and stitches into them, or from a site whose folded property such a file may set, stay below `synthetic` (contract M4a amendment 11).
- *As decided in contract-m4a (amendment 10, fork F3):* when a route's prefix cannot be composed (an Express router mounted nowhere visible or through a default import, a gin group passed as a function parameter), the path is the tail below its router, the endpoint is flagged `prefix_unknown`, and its key carries the router's identity (the declaring file and the binding's scope-qualified name, no position), so it never merges with a route of the same tail on another router or with a known-prefix route.
- *As built in M9a (contract-m9a D1, D2):* the endpoint stage reads no file for which `rb_model::is_test_path` holds: no route, router, mount, gin group composition, interface route or gRPC server registration comes from a test file. Each test file with route facts gets one `routes_in_test_code` diagnostic, detail `{"routes": n}`. An OpenAPI operation's own registration is `synthetic`, and an endpoint takes its best registration, so a spec-only endpoint is `synthetic` while a route code registers stays `exact`. Interface mappings and `.proto` RPCs keep their certainty.
- *As built in M9a (contract-m9a D4):* a `native` Spring Cloud Config server's shared files set context paths. The server is the one service whose configuration activates the `native` profile and lists `search-locations`; each `classpath:` entry is read from `<server>/src/main/resources/<dir>/` (`file:` entries and entries holding `{` are not). A config client with an effective `spring.application.name` takes `server.servlet.context-path` from that directory's `application.*` and `<name>.*`, unconditional documents only. Precedence, lowest first: local files, remote `application.*`, remote `<name>.*`. The highest value wins even when it is empty, and an empty value is Spring's root context, so it clears a lower path (evaluator pass 1, E3; `PIPELINE` `m9a-2`). The prefixed path is `synthetic`, and each client gets a `config_server_context_path` diagnostic `{service, file, context_path}`. With more than one such server nothing applies, and one `config_server_ambiguous` diagnostic lists them.
- *As built in M9b (contract-m9b D7):* Spring Data REST resources are endpoints when an interface carries an imported `@RepositoryRestResource` with a non-blank literal `path` and no `exported = false`, directly extends an imported Spring Data repository base, and lives in a service whose build file names `spring-boot-starter-data-rest` or `spring-data-rest-webmvc`. They are `GET /{path}` for any base with `findAll` (not a bare `Repository`); `POST /{path}` and `GET|PUT|PATCH|DELETE /{path}/{id}` for `CrudRepository`, `ListCrudRepository` and `JpaRepository`; and `GET /{path}/search/{m}` for each query method (`find…By`, `read…By`, `get…By`, `query…By`, `count…By`, `exists…By`). CRUD names (`findById`, `findAll`, `findOne`, `count`, `existsById`, `findAllById`) and `@RestResource(exported = false)` are excluded, and `@RestResource(path)` renames. Collection and item resources are handled by the interface, searches by their method. The context path and `spring.data.rest.base-path` prefix them. They are `synthetic`, never `exact`, because Spring generates the controller. A service with a `RepositoryRestConfigurer` caps them at `heuristic`, with one `data_rest_base_unread` diagnostic. A configurer is a Java or Kotlin class extending `RepositoryRestConfigurer` or `RepositoryRestConfigurerAdapter`, a Java method returning one (the `@Bean` form, including the 2.x reference's anonymous `…Adapter`; the annotation is not checked, since a cap only lowers certainty), or a `RepositoryRestConfigurer.withConfig(…)` call (3.1+; how a Kotlin `@Bean` with an inferred type is seen). A Kotlin `@Bean` with a declared return type and an `object :` body is not seen (no Kotlin return types in the facts). Unannotated repositories (Spring's default exposure strategy) and default paths (Evo Inflector plurals) are not read.

### 10.2 Stitching client call sites to endpoints (`http_stitch`)

For each `HttpCall{method, url_template, base_ref}`:

1. **Constant-fold the URL.** Resolve `base_ref` if it is a same-module constant, an imported constant, or an env var with a literal default (`process.env.X ?? 'http://auth:8080'`, `@Value("${auth.url:http://auth}")`). The result is a normalised path plus an optional host.
2. **Pick the target service from the host.** If the host names a compose/k8s service (§4.3), restrict candidates to that service. The match is `exact`.
3. **Match paths.** Otherwise, match the path against all endpoints. An exact normalised match is `exact`. A unique suffix match (unknown base prefix) is `heuristic`. Several matches are `ambiguous`, with an edge to each.
4. **Unmatched** calls become diagnostics, e.g. calls to third-party APIs. The host is still recorded as an external `Endpoint` node with `external=true`.
5. **Generated clients** (OpenAPI generator, Feign, gRPC stubs) are matched by operation ID or RPC name. This is `exact`, and it is the most reliable path, so fixture coverage should favour it.

*As decided in contract-m4a (D2, D6, D9, S13, S14), this replaces the certainties above:*
- **Certainty.** A `CONSUMES` edge is never `exact`: a string match across a network boundary depends on deployment.
  - `synthetic` needs an exact path match, a known method, a host resolved to a non-proxy service from a literal, a constant or `application.properties`, and a single candidate.
  - `heuristic` is any other single candidate.
  - `ambiguous` covers 2–16 candidates, one edge each.
  - More than 16 candidates give a diagnostic and no edge.
  - The string-level match kind is kept as `match = exact | suffix`.
- **Candidates.** A client's own service never supplies candidates.
- **Hosts.** A host matches a service by its name or the last segment of its key, and only when that match is unique.
  - A matched service with no endpoints is a proxy, and the call is path-matched across services.
  - A loopback host means the caller's own service.
  - A literal host naming no service is external, counted and never path-matched.
- **Specificity.** The candidate with more literal segments wins, then an exact method over `ANY`. *As decided in contract-m4a (amendment 9):* for a suffix match only the literals of the endpoint tail the client matched count.
- **Deferred.** Feign is matched by its declared URL and mapping, not by operation ID. External `Endpoint` nodes and operation-ID matching come with the OpenAPI/gRPC packs (M6).

### 10.3 Assembling the request path

Input: an endpoint query such as `login`. Resolution matches `Endpoint` keys by trigram. If several endpoints match, the candidate list is returned first (§9.1).

- **Downstream tree:**
  1. `HANDLED_BY` handler.
  2. BFS over outbound `CALLS` (+ dispatch, RTA) and `INJECTS`, within the handler's service, to depth N (default 12). The tree is pruned to paths that end in a *sink*:
     - `QUERIES` → Table;
     - `CONSUMES` → Endpoint, which **recurses** into that endpoint's own downstream tree, up to 4 service hops, with cycle guard on endpoints;
     - an external package call on an allow-list (JWT, crypto, cache, mail clients);
     - a leaf.
  3. Nodes that don't lead to any sink are collapsed into "+ k helper calls". This keeps the output readable without hiding anything.
- **Upstream tree:** inbound `CONSUMES` to the endpoint gives client call sites in other services. From there, inbound `CALLS` go to their entry points: frontend event handlers, gateway routes, other endpoints' handlers, or scheduled jobs. The depth is the same as downstream.
- **Output:** the two trees joined at the endpoint, rendered as indented text, JSON or Mermaid/DOT. Every hop carries file:line and certainty.

*As decided in contract-m4a (D7, D8, S15–S18):*
- **Sinks** are `QUERIES → Table` and `CONSUMES → Endpoint`, so a leaf is not a sink and helper subtrees collapse; `--full` shows them.
- **Scope.** The walk covers the handler's service plus library services. A call into another deployable service is shown and not followed.
- **Inline handlers.** A handler that is an inline function is its enclosing definition (or `File`), restricted to the edges whose evidence lies in its span.
- **Dispatch** is CHA, filtered by RTA under `--precision rta` (opt-in since contract-m4b amendment 21; the default is `cha`). `rb path` does not follow `INJECTS`, which is a type-level edge with no method step.
- **Limits.** `--hops` defaults to 4 (maximum 8).
- **Output** is text and JSON; Mermaid/DOT are deferred.
- *As built in M4a (amendments 6 and 10):* input resolution tries an exact endpoint key, then `<METHOD> <path>` normalised as routes are, then a substring of endpoint names, then a handler or client symbol; several candidates are listed (a prefix-unknown endpoint with its router) and `--pick N` chooses, never a silent pick. Each `CONSUMES` step says how it was stitched (host, proxy, suffix match, candidate count).

**Honest framing:** this is the set of *statically possible* paths, not the path a particular request took. Branch conditions are not evaluated. The output makes this clear with a header line.

```
Request path: POST /api/v1/login   (snapshot 42, precision=cha)
UPSTREAM
  web-frontend  LoginForm.onSubmit()        src/pages/Login.tsx:31     [exact]
    → authApi.login()                       src/api/auth.ts:12         [exact]
      → CONSUMES POST /api/v1/login         (axios, base=VITE_API_URL default)  [heuristic: suffix match]
ENDPOINT  auth-service  POST /api/v1/login  → AuthController#login()   AuthController.java:44
DOWNSTREAM
  AuthController#login()
    → AuthService#authenticate()            [dispatch → AuthServiceImpl, 1 of 1 instantiated]
      → UserRepository#findByEmail()        → QUERIES public.users (read)        [synthetic: spring-data]
      → PasswordEncoder#matches()           → external org.springframework.security  (allow-listed)
      → SessionService#create()             → QUERIES public.sessions (insert)   [exact: sql_literal]
      → MfaClient#sendCode()                → CONSUMES POST /internal/mfa/email  [exact: feign]
          ⇒ notification-service  MfaController#email() → EmailSender#send() → external jakarta.mail
    + 7 helper calls collapsed
```

---

## 11. Interfaces

### 11.1 CLI (`rb`)

| Command | Purpose |
|---|---|
| `rb index <repo-path> [--commit SHA] [--full] [--no-scip] [--projects a,b]` | Build a snapshot. Incremental against the live one by default. |
| `rb status [--repo R]` | Live snapshot, coverage report, last run's diagnostics summary |
| `rb find <text> [--kind K]` | Node search (§9.1) |
| `rb callers <sym> [--depth N] [--precision rta|cha] [--min-certainty C] [--include-references]` | Q1 |
| `rb impact <sym|table> [--precision cha|rta] [--max-nodes N]` | Q2 |
| `rb table-deps <table>` | Q3 |
| `rb path <endpoint-query> [--depth N] [--hops H] [--format text|json|mermaid|dot]` | Q4 |
| `rb service <name> [--format …]` | Service Card |
| `rb cypher '<query>'` | Run ad-hoc Cypher against the live AGE graph (read-only role) |
| `rb diff <snapA> <snapB>` | Changes in edges, endpoints, table access and service dependencies. Useful as a PR check. |
| `rb dump [--snapshot S]` | Canonical sorted dump, used for determinism tests |

`rb` talks to `rb-server` when `RB_SERVER` is set. Otherwise it runs embedded mode directly against Postgres, using the SQL fallbacks.

*As decided and built in contract-m5 (D2, D4, D11, D12, D17, D18, Scope):* the server is `rb serve`, a subcommand of the one `rb` binary; the `RB_SERVER` thin-client mode is not built, because every `--json` output and the matching API body come from one request layer (`rb-api`) and are tested byte-identical (150 parameter sets over the three fixtures). Every question and `rb status`/`rb snapshots` take `--json` (`rb path --format json`) (one envelope: `api`, `repo`, `snapshot`, `query`, `limits`, `truncated`, `certainty`, `result`; errors with exit codes 2–5); every question takes `--snapshot <id>` (a kept retired snapshot answers as it did while live); flat lists take `--limit`/`--offset`. `rb diff [<from>] [<to>]` compares two kept snapshots by natural identity (nodes added, removed, changed; edges likewise, evidence-only changes apart) and summarises endpoints, table access and service dependencies; it never re-indexes and names the `tool_versions` keys that differ (`tools_differ`). Also built: `rb snapshots`, `rb gc [--egress] [--prefix] [--dry-run]`, `rb cache prune --facts|--scip [--dry-run]`, `rb index --keep N --profile`, `rb cypher --timeout-ms`. The `--commit`, `--full`, `--projects` and `--max-nodes` flags, `rb path --format mermaid|dot` and `rb service --format` of the table above are not built (`--rev` and `--force` exist; `rb path --format json`).

### 11.2 REST API (`rb-server`)

- `GET /v1/repos/{repo}/symbols?q=`
- `GET /v1/repos/{repo}/callers?sym=&depth=`
- `GET /v1/repos/{repo}/impact?target=`
- `GET /v1/repos/{repo}/tables/{key}/dependents`
- `GET /v1/repos/{repo}/paths?endpoint=`
- `GET /v1/repos/{repo}/services/{name}`
- `POST /v1/repos/{repo}/cypher` (read-only, statement timeout)
- `POST /v1/repos/{repo}/index` (enqueues a run)

Guard rails, all configurable, are included in every response:

- default depth ≤ 12 and hard max 25 for path trees;
- max 200 k visited nodes for reachability;
- max 5 k nodes in any rendered tree;
- a 2 s statement timeout for Cypher;
- per-request CPU budget.

A `truncated: true` flag marks any response that hit a limit, together with which limit it was. The server never returns a silently partial answer.

*As decided, built and measured in contract-m5 (D1–D9, D17, D18, S13, S25, amendments 4 and 7):* `rb serve` (axum 0.8 over the existing tokio, HTTP/1.1; 21 new crates, +1.17 MiB). `truncated` is a list of limit names (`depth`, `hops`, `nodes`, `tree`, `work`, `rows`); the per-request CPU budget is a deterministic count of examined elements (`work`, 20,000,000), and a wall-clock deadline (`--deadline-ms`, default 10 s, queue included) yields `503 deadline_exceeded`, never a body; `--workers` (4) run at once and `--queue` (16) wait, then `503 busy`. Keys are passed as query parameters: `GET /v1/health`, `/v1/repos`, and under `/v1/repos/{repo}/`: `status`, `snapshots`, `find?q=&kind=`, `services`, `service?name=`, `callers?target=&depth=`, `impact?target=`, `table-deps?table=&hops=`, `path?endpoint=&depth=&hops=`, `diff?from=&to=`, `POST cypher` with `{"query", "columns"?, "timeout_ms"?}`; every question takes `snapshot=`, flat lists and diff sections take `limit`/`offset`, and an unknown parameter is `400 invalid_param`. Statuses map one-to-one from the CLI's exit codes (400, 404, 409, 410, 422; 421/405/415 for transport rules). The server binds loopback only (any other address is refused at start-up), answers `421` to a `Host` that is not its loopback address and port, sends no CORS headers, reads in read-only sessions and runs Cypher as the role `rb_reader` (migration 0006). `POST /v1/repos/{repo}/index` is not built in M5 (an unauthenticated endpoint that runs repository builds). Latency is in §12.5.

---

## 12. Correctness and testing strategy

### 12.1 Golden fixture repositories (`fixtures/`)

Each fixture is a small but realistic repo, committed with its own Git history, plus an `expected.yaml`. The `expected.yaml` lists edges that **must exist** and edges that **must not exist**; the latter are precision guards. Fixtures:

| Fixture | Exercises |
|---|---|
| `java-spring-jpa` | `@RequestMapping` composition; an interface with 2 impls, one never instantiated (RTA); `@Qualifier`, `@Primary`, `@Profile`; Spring Data derived queries; `@Query(nativeQuery)`; JPQL; Lombok |
| `ts-express-prisma` | router prefix composition, axios with `baseURL` constant, Prisma `@@map`, `pg` raw SQL with template holes, function-as-value callbacks |
| `python-fastapi-sqlalchemy` | decorators, `__tablename__`, f-string SQL (normalisation), import resolution |
| `go-chi-sqlx` | route groups, `database/sql`, interfaces satisfied implicitly (Go structural typing via SCIP) |
| `polyglot-monorepo` | frontend → gateway → auth (Spring) → notification (Node) → tables. The end-to-end Q1–Q4 golden outputs live here. |
| `no-build` | the same as `polyglot-monorepo`, but with SCIP disabled. Asserts the Tier B heuristic results and their certainty labels. |
| `codeowners-cases` | precedence, negation-free semantics, empty-owner lines, directory vs file patterns |

### 12.2 Test types

- **Unit tests:** normalisers (route, SQL template, JPQL), the CODEOWNERS matcher, the SCIP⋈TS range join, the certainty minimum over paths, and the orphan fixpoint.
- **Golden tests:** `rb callers/impact/table-deps/path/service` outputs, snapshotted as JSON per fixture. A diff fails CI and needs an explicit `--bless`.
- **Determinism:** index each fixture twice, from clean caches and from warm caches. `rb dump` outputs must be byte-identical.
- **Incremental equivalence:** replay each fixture's commit history. After each commit, the incremental snapshot must `rb dump` byte-identically to a `--full` snapshot of the same commit. This test is the gate for any v2 delta-loading work.
- **Schema invariants:** every edge kind has a producer and a fixture (§8.5). Every edge has at least one evidence row, except derived kinds. There are no dangling nodes.
- **AGE parity:** for each fixture, 1-hop neighbour sets from Cypher equal the relational ones. This checks that the projection is faithful.

### 12.3 Property tests

- Route normaliser idempotence: `norm(norm(x)) == norm(x)`.
- CODEOWNERS: table tests of every rule in GitHub's documented example file, with the behaviour each rule's comment describes, plus last-match-wins across the whole file. This replaces the planned randomized cross-check, which would only have compared two implementations written by the same author.
- Reachability: CSR BFS agrees with the SQL `WITH RECURSIVE … UNION` result on random graphs, including cycles.

### 12.4 Coverage report (per snapshot, stored in `snapshot.coverage`)

- Per project:
  - SCIP status;
  - % of Tree-sitter call sites resolved as `exact` / `dispatch` / `heuristic` / `ambiguous` / `unresolved`.
- SQL: literals found, parsed by pg_query, parsed by fallback, and unparsed; tables marked `schema_assumed` or `undeclared`.
- HTTP: client calls stitched as exact, heuristic, ambiguous or unmatched.
- Blind-spot diagnostics by kind (§6.5).

These numbers are the honest answer to "how much should I trust Q2 on this repo?", and `rb status` shows them.

### 12.5 Scale benchmark (targets, and the M5 measurements)

- A large OSS Java monorepo and a large TS monorepo (candidates chosen at M5).
- *As decided in contract-m5 (S23, D14):* candidates apache/shardingsphere (Java) and mattermost/mattermost (TypeScript with Go), measured on the development machine (Apple M4, 10 cores; Docker VM 10 CPUs, 7.75 GiB) with the hardware and load stated next to each number.
- Targets on a 16-core machine, with the numbers *measured in contract-m5 (S24, S25; phases 17–18)* beside each. Hardware and load: Apple M4, 10 cores, 16 GiB; Docker Desktop VM 10 CPUs, 7.75 GiB; Postgres 18 + AGE 1.8 in that VM; a shared machine with other applications running, 1-minute load averages between 1.6 and 10.3 at the runs' starts and ends (each run's figures are in the contract). Repositories: apache/shardingsphere @ `b09dfffcf9e5` (720,005 non-blank lines, Java 708,960; Maven) and mattermost/mattermost @ `b2ce71db9c18` (1,698,802: Go 818,222, TypeScript 793,519, JavaScript 85,891; npm and Go). Release binary; latency through a warm `rb serve`, one sequential client, two runs of 200 samples (Q4: all 66 endpoints).
  - full index of about 1 M LOC in ≤ 15 min excluding dependency fetch, with the SCIP build dominating — **met without SCIP** (35.0 s shardingsphere, 29.2 s mattermost, fresh; 23.4 s and 19.6 s from the facts cache); **with SCIP met on mattermost** (610.6 s including every fetch, but only 4 of 11 TypeScript projects build and Go has no indexer) and **missed on shardingsphere** (≈ 1,238 s ≈ 20.6 min without the fetch; the profile shows `rb` waiting on one scip-java job, the whole 446-module reactor on a 2-CPU budget, ≈ 1,086 s in its index phase; open item 1 of the contract, M6);
  - an incremental single-file commit that does not trigger a project re-index in ≤ 30 s — **met**: 18.5 s and 19.2 s (shardingsphere), 19.6 s and 17.9 s (mattermost), `--no-scip`; with SCIP every commit on shardingsphere re-indexes its single project;
  - Q1 depth-1 in ≤ 50 ms p95 — **met**: client p95 1.61/3.33 ms (shardingsphere, runs 1/2) and 1.88/1.79 ms (mattermost);
  - Q2 on a hub symbol in ≤ 500 ms p95 via CSR — **met**: 95.6/93.7 ms (the 20 symbols with most inbound requiring edges, up to 836) and 152.1/151.2 ms (up to 2,999);
  - Q4 in ≤ 1 s p95 — **met on mattermost**: 3.64/3.63 ms over all 66 endpoints; not measurable on shardingsphere (no endpoint).
- Also measured: Q3 p95 7–8 ms, the Service Card p95 38–66 ms; the snapshot index built in 0.57–0.63 s, server RSS 249–284 MiB; a cold CLI question 0.4–0.6 s (it loads the graph per invocation); no answer was truncated, so the work cap stands. Peak RSS of `rb index` 1.26–1.65 GiB without SCIP, 2.4 GiB with a 530 MB cached SCIP index. Dumps were byte-identical fresh and cached, with and without SCIP, on both repositories.

---

## 13. Phased roadmap

Each milestone is shippable and has an exit criterion that is a test, not a demo.

| Milestone | Contents | Exit criteria |
|---|---|---|
| **M0 Foundations** | Rust workspace; Docker Compose with Postgres + AGE (pinned digest); `rb.*` migrations; snapshot lifecycle; `rb index` manifest-only; AGE projection of `File/Service` (Owner moved to M1, because it needs CODEOWNERS); `rb dump` | determinism test green on 2 fixtures; `rb cypher` returns files per service. **Done 2026-09-29.** |
| **M1a Structure** | CODEOWNERS; service boundaries + `rb.toml`; edge evidence; `rb find`, `rb service` (structure sections) | GitHub-example CODEOWNERS tests; golden Service Cards on `polyglot-monorepo`. **Done 2026-09-29.** |
| **M1b Syntax** | Tree-sitter defs/refs/scopes for TS/TSX/JS, Java, Python, Go; Symbol/DEFINED_IN/CONTAINS; facts cache (change sets moved to M3) | contract `tasks/contract-m1b.md` C1–C20; warm-cache run parses 0 files and dumps identically to a cold run |
| **M2 Data access** | `rb-sql`: sinks, template normalisation, pg_query, sqlparser fallback, JPQL tokenizer; ORM packs (JPA, Prisma, TypeORM, SQLAlchemy); `QUERIES/MAPS_TO/MIGRATES` | `rb table-deps` golden at the symbol level (service roll-up via file→service); SQL coverage metrics |
| **M3a Tier B** | Import, package and binding facts; module resolution; heuristic `CALLS/REFERENCES/IMPLEMENTS`; resolution coverage; `rb callers`, `rb impact` over the in-memory graph | contract `tasks/contract-m3a.md` R1–R13: Q1/Q2 goldens on all fixtures with must-not-exist cases; ≥ 90% precision on a hand-checked sample of 4 public repos. **Done 2026-10-01** after six evaluator passes; resolved share of own-code references: gin 63%, petclinic 39%, requests 30%, hono 15%; sampled precision 96/96. |
| **M3b SCIP** | SCIP orchestration in pinned containers (scip-java, scip-typescript first); SCIP⋈TS join; key replacement per project; `Package`/`DEPENDS_ON` from external symbols (change sets and orphan detection deferred: contract scope) | exact edges on the fixtures that build; cached and fresh indexes dump identically. **Done 2026-10-06** (built 2026-10-01; contract `tasks/contract-m3b.md` S1–S13, amendments 1–81 after twelve evaluator passes; final binary measured on nine public repositories): Maven and npm projects; spring-petclinic 256 exact `CALLS` and 13 `IMPLEMENTS`, class-validator 398 and 2, jsoup 14,013 and 688, spring-retry 459 and 252, networknt/json-schema-validator 6,449 and 731, spring-petclinic/spring-petclinic-rest 652 and 166, vavr 27,257 and 5,994, TheAlgorithms/Java 10,374 and 269, typestack/routing-controllers 169 and 38, 0 documents rejected and 0 supertypes unconfirmed on each (evaluator passes 6 and 7 sampled 80/80 `CALLS` and 32/32 `IMPLEMENTS` correct); open items (effective source encoding, TypeScript restructurings Tree-sitter parses cleanly, TypeScript interface-method `IMPLEMENTS`, Gradle, spring-retry's test tree, M6 build recipes, workspace installs, tsconfig include/exclude, a registry allow-list for the fetch phase) are listed in contract amendment 81 and carried to M4–M6. |
| **M4a Endpoints and request paths** | Spring MVC, Express and Go (gin, `net/http`) route packs; `fetch`/axios, Feign/`RestTemplate`/`WebClient` and Go `net/http` client packs; endpoint normalisation; `http_stitch`; `rb path`; the Service Card's API section; the `rb impact` endpoint roll-up | contract `tasks/contract-m4a.md` S1–S28: Q4 goldens on `polyglot-monorepo` (TypeScript → Java → TypeScript, plus Go); must-exist and must-not-exist edge lists green on all three fixtures; no `CONSUMES` above `synthetic`; three public repositories measured (20/20 sampled endpoints). **Done 2026-10-06** (built 2026-10-06; contract `tasks/contract-m4a.md` S1–S28 and invariants 1–13 all PASS, amendments 1–18 after eight evaluator passes, fork F3 decided; closing binary `28e47ba`, `PIPELINE` `m4a-http-11`): fixture goldens and guards green with and without SCIP, e2e 17/17; seven public repositories (spring-petclinic-microservices, node-express-realworld, golang-gin-realworld, piggymetrics, ftgo-application, piomin, Store-Microservices) give 195 endpoints and 94 client sites, `CONSUMES` 13 heuristic and 2 ambiguous, **0 `synthetic`, 0 false `synthetic`**, fresh == cached; spring-petclinic-microservices 16/16 endpoints and 5/5 `CONSUMES` correct, node-express-realworld 20/20 as labelled, golang-gin-realworld 20/20 sampled. A request is `synthetic` only through a carrier proven plain by a repository-wide token scan plus a raw-text scan of unparsed files, with no discovery configuration (amendments 13–17). Open items (24, each with its milestone: M4b, M5 including the adversarial carrier review, M6) are consolidated in contract amendment 18 and the contract's Open items table. |
| **M4b DI and dispatch** | Spring DI synthesis (`INJECTS`, never above `synthetic`: qualifiers, `@Primary`, name fallback, profiles/conditionals never `synthetic`, `@Bean` methods, Lombok constructors); Spring Data repositories and Feign clients as beans (no synthetic node, fork F5); RTA as a proof of absence (`impl_instantiated: false`; planned as the Q1/Q4 default, opt-in `--precision rta` since contract-m4b amendment 21, `cha` the default everywhere); entry points; orphan detection; Q3 service roll-up with endpoints and "through APIs"; `Package` versions from `package-lock.json`, `go.mod` and pom literals; service dependencies (`via=package` stored, `via=calls`/`via=http` at query time, fork F1); the rest of the full Service Card; M4a open items 1, 5, 7, 9, 15, 24 (item 2 moves to M6) | contract `tasks/contract-m4b.md` S1–S28, with its soundness claims and closing bar stated up front: Q3 goldens on `polyglot-monorepo` and `java-spring-jpa`; RTA precision guards (an uninstantiated implementation is not a dispatch target) green; DI, RTA and orphan guards with biting mutations; seven public repositories measured (0 false `synthetic` `INJECTS`, 0 pruned-but-instantiated types, 0 false orphans in the samples). **Done 2026-10-07** (built 2026-10-06, phases 0–16; contract `tasks/contract-m4b.md` S1–S28 and invariants 1–11 all PASS after two evaluator passes, amendments 1–22, fork F2 decided: `INJECTS` capped at `heuristic` with `resolution`; closing binary `304fe4a`, `PIPELINE` `m4b-di-8`): pass 1's 15 findings (3 blockers: RTA proofs missed by binary names and non-definition subclasses, an orphan rule that did not walk supertypes) fixed test-first as amendment 21, which also made RTA opt-in; pass 2 verified every repro end to end (Java and Spring 6.1.13 at run time, `go list`), 21/21 mutants killed, fmt, clippy, 559 unit tests and e2e 18/18 twice with containers green. Public repositories (spring-petclinic, spring-petclinic-microservices, realworld-spring, jhipster-sample-app; piggymetrics, spring-petclinic-rest, node-express-realworld, golang-gin-realworld, routing-controllers in phase 15): `INJECTS` all `heuristic` or `ambiguous`, 0 `synthetic`, samples correct (69/69, 54/54, 20/20 across the three measurements); no type proven by RTA on any of them (the allow-list's recorded cost); orphans 12/12, 22/22, 9/9 with no other use; package versions 128/128, 69/69, 20/20; invariant 1 against `28e47ba`: 0 rows removed; fresh == cached. Open items 1–23 by milestone (M5 including the adversarial review of DI, RTA and orphans; M6) in contract amendment 22 and the Open items table. |
| **M5 Scale & API** | REST API with guard rails (`rb serve`, read-only, loopback only, one request layer shared with every `--json` output); consistent snapshot reads, AGE DDL lock, retention and `rb gc`; bulk AGE loading measured with full parity; `rb diff`; fetch-phase egress allow-list and per-repository SCIP caches (fork F2); the bounded adversarial review of M4a's carriers and M4b's DI, RTA and orphans; benchmark on 2 large OSS repos. Contract `tasks/contract-m5.md` (written 2026-10-07): S1–S28, invariants 1–11, soundness claims and closing bar stated up front, decisions D1–D18, forks F1–F6; M3b, M4a and M4b open items assigned to M5 dispositioned in D16 (precision items measured in M5 and moved to M6, fork F5) | §12.5 targets met or gaps documented with profiles; API answers byte-identical to the CLI's `--json`; no silently partial answer; retention never removes the live snapshot. **Done 2026-10-07** (built 2026-10-07, phases 0–20; contract `tasks/contract-m5.md` S1–S28 and invariants 1–11 all PASS after two evaluator passes, amendments 1–17; pass 1's 7 findings (2 major: `status` of an unknown repository answered 200; a publish killed between its flip and its retention left a second AGE graph that Cypher answered from) fixed test-first as amendment 16, pass 2 verified every repro end to end, 12/12 mutants killed, the verification loop green twice): `rb serve` and `rb-api` (150 parameter sets byte-identical to the CLI); whole-snapshot reads, the AGE DDL lock, retention, `rb gc`, `rb cache prune`; full AGE parity with `ops`; `rb diff`; the egress proxy (probe suite: every forbidden path closed) and per-repository caches; S19's review fixes (`PIPELINE` `m5-3`), invariant 1 classified (12 rows on seven repositories); §12.5 measured (§12.5 above: every latency target and the no-SCIP and incremental targets met, the SCIP full index missed on shardingsphere with its profile); fresh == cached on both benchmark repositories and every fixture; open items 1–25 by milestone in contract amendment 17 (M6 except residue items 15, 16, 23 and the not-planned item 21) |
| **M6 Breadth** (split into M6a–M6j by the M6 planner, 2026-10-07; each sub-milestone has its own contract, written when it starts, with its soundness claims and closing bar up front: evaluator pass 1 full, pass 2 verification-only) | design §13's original M6 scope (scip-python/go/clang/rust-analyzer; Kotlin/C#; OpenAPI/gRPC packs; gateway configs; message topics; architecture-rule checks in Cypher as a CI gate), the items moved by contract-m4b (gin groups as parameters, Guice/Dagger/NestJS/.NET DI, Spring XML and Kotlin bean definitions, active-profile evaluation, pnpm/yarn/Gradle/Python lockfiles) and contract-m5's open items (amendment 17), assigned row by row below | per sub-milestone |
| **M6a SCIP coverage on Java and TypeScript** | Contract `tasks/contract-m6a.md` (written 2026-10-07). From contract-m5's open items: Lombok-generated members no longer reject scip-java documents, removed by Lombok's own `@lombok.Generated` marker before the unchanged agreement checks (3); TypeScript interface-method `IMPLEMENTS` from scip-typescript's member relationships, the declared-clause walk kept (8; contract-m3b amendment 81); npm and Yarn v1 workspace installs from a root lockfile, membership by an allow-list of glob forms (4); `package-lock.json` link targets outside a job's root but inside the repository exported with a repository-shaped layout (5); host-side `--scip-cpus`/`--scip-memory`/`--scip-maven-threads`, measured on a single-root Maven reactor (1); `rb serve` checks its bind before its startup sweep, and a two-repository sweep test (24, 25) | contract S1–S20: `java-lombok` and `ts-workspaces` fixtures with SCIP goldens; shardingsphere's 3,889 rejected documents re-measured, every remaining rejection classified, 40/40 sampled exact `CALLS` from formerly rejected documents correct; mattermost's `webapp` workspace and `e2e-tests` link jobs measured, every interface-method `IMPLEMENTS` read; `--no-scip` dumps byte-identical to `cfa8ab9`, SCIP dumps differing only in classified rows. Measured 2026-10-09 (contract amendments 8–11): shardingsphere rejected 0 with 107,101 removals; eladmin's Lombok 1.18.30 unmarked (residue); mattermost grouped (cypress ok with registries; `webapp` engine-blocked, M6e); threaded reactor deterministically `failed` (flag stays). Closed 2026-10-09 (amendment 13; evaluator passes 1–2 PASS) |
| **M6b Request-path precision** | gin groups passed as parameters (contract-m4a open item 2, contract-m4b 1); Express routers in a router's construction chain and anonymous default-exported routers (contract-m4b 2); in-process Go test requests and empty URLs (contract-m5 6, 7); contested and cross-file axios configurations and configurations passed by name (contract-m5 11, 12; contract-m4a 20, the rest of 21); constructor-assigned members (contract-m4a 16); user information with a hole (contract-m4a 19); the discovery and JSON-key parts of S19's over-capping (contract-m5 10); TypeScript client breadth: `got`, `ky`, `superagent`, `undici`, Angular `HttpClient` (contract-m4a 14, part). Measured 2026-10-10 (contract amendments 9–10): gin 19 unknown into 26 composed prefixes with 33 in-process test-client sites; express 19 unknown into 19 composed `/api` keys; ftgo's 4 dynamics into 3 heuristic + 1 genuine 3-way ambiguous; mattermost's 8 ambiguous edges removed with 53 in-process + 8 empty-URL sites; microservices + piggymetrics byte-identical; discovery narrowing lifted nothing anywhere measured. Closed 2026-10-10 (amendment 11; evaluator passes 1–2 PASS, E1–E2 fixed) | Q4 goldens and stitch guards with biting mutations; gin-realworld's `prefix_unknown` endpoints and mattermost's 8 false `ambiguous` `CONSUMES` re-measured; no `CONSUMES` above its stated bar |
| **M6c DI, RTA and orphan precision** | `@Import`/`@Bean` registrants (contract-m5 13); gate M on main sources (contract-m4b 5); RTA yield and Spring Boot module roles (contract-m4b 6, 7); module objects through `tsconfig` `paths` and `package.json` `exports` (contract-m4b 19); rule (e)'s on-demand-import narrowing (contract-m5 10, part); autowiring of `@Bean`-returned and validator-factory objects (contract-m4b 4); Q3 through inherited Spring Data methods (contract-m4b 8); Lombok's copied qualifiers from `lombok.config` (contract-m4b 14); generic matching and custom qualifiers (contract-m4b 16, 17); the DI, `INJECTS` and RTA samples the M5 benchmark could not take (contract-m5 9, part). Closed 2026-10-11 (contract amendments 1–15; evaluator passes 1–2 PASS, E1–E2 fixed): registrants join ambiguous, test literals cap nothing, deployable via plugin-or-appclass, exact manifest resolution with longest-wins, level-tracked rule (e), heuristic non-bean consumers, inherited calls with `inherited`, config-copied qualifiers counted, exact type-arg picks capped (custom metas deferred as residue); six public repos measured with every new edge classified (realworld +6 S6 with bucket shifts, jhipster +119 S7/+14 S4, microservices 8 role flips blast-free, RTA yield 0) | DI, RTA and orphan guards with biting mutations; 0 false `synthetic` `INJECTS`, 0 pruned-but-instantiated types and 0 false orphans in samples on a DI-heavy public repository |
| **M6d Go and Python SCIP** | scip-go in a locally built image (a pinned Go toolchain on a pinned base: the Docker Hub `sourcegraph/scip-go` image is stale, 300.7 MB compressed on 2026-10-07) and scip-python (`sourcegraph/scip-python`, amd64 only, 188.8 MB compressed on 2026-10-07; its uncompressed size measured before any pull, then pinned by digest); `go mod download` and pip/Poetry/uv environments in the fetch phase behind the egress proxy (the Go module proxy and PyPI added to the built-in allow-list); one `go/<module>@<version>` key shared by `scip` and `manifest` package edges (contract-m4b 13); `poetry.lock`/`uv.lock` versions (contract-m4b 12, part); Python `requests`/`httpx` clients (contract-m4a 14, part); the `go-chi-sqlx` and `python-fastapi-sqlalchemy` fixtures (§12.1); the adversarial review of the fetch phase's egress once its allow-list is final (contract-m5 22); mattermost's Go re-measured with scip-go (contract-m5 9, part) | per-language fixtures with exact edges; fresh == cached; the egress probe suite extended to the new registries; the review's findings fixed or recorded as residue. **Done 2026-10-11** (phases 0–12; contract `tasks/contract-m6d.md` S1–S14 and invariants 1–7 all PASS after two evaluator passes, amendments 1–15; pass 1's 11 findings (2 blocking: tool_versions identities, absolute-path replace joins) fixed test-first with hardening (chi Handle, double-include, dispatcher kwargs, GOTOOLCHAIN env), pass 2 verified every repro end to end, 22/22 mutants killed, the verification loop green: fmt, clippy, 765 unit + 25 cli-bin, store 66, server 12, e2e 34/34, api 1/1 over 150 sets; `PIPELINE` unchanged `m6c-5`, `RECIPE_PY` `m6d-py-4`): `go-chi-sqlx` (chi groups, `database/sql`, implicit-Payer `IMPLEMENTS`) and `python-fastapi-sqlalchemy` (FastAPI prefixes/includes, `__tablename__`, f-string SQL, session clients) fixtures with biting guards; growth proven per new walk (chi sweep, FastAPI memoisation, 5 ratio/pin tests); invariant 1 vs `027a54f` (4/7 identical, rest added-only W rows + 1 HANDLED_BY-kept-entry shift); pip read-timeout hardening from measurement (`PIP_TIMEOUT=60`) |
| **M6e Build and lockfile breadth** | scip-java Gradle builds (Gradle in the locally built image, never the repository's wrapper) and Gradle dependencies and version catalogs (contract-m4a 17, contract-m4b 12); `pnpm-lock.yaml` and `yarn.lock` versions (`lockfile_unread` today), pnpm workspaces, Yarn ≥ 2, Yarn `file:`/`link:` packages; `go.work` and `go.mod` `exclude`, Maven classifiers (contract-m4b 22); tsconfig `include`/`exclude`, effective source encoding, TypeScript restructurings Tree-sitter parses cleanly, further build recipes (contract-m5 18; contract-m3b amendment 81); SCIP image toolchain breadth (a repository whose `engines` exceed the pinned indexer image's toolchain fails its fetch: mattermost's `webapp` under M6a amendment 9) | a Gradle fixture with exact edges; per-format lockfile units; sampled versions on public repositories with 0 false `exact` | Measured 2026-10-12 (contract amendments 11–12): jcommander Gradle SCIP ok 45.3s (1,218/1,218 definitions, 1,355 exact CALLS, 20/20 sampled); ofetch 12 pnpm pins, 0 false exact; class-validator tsconfig partition 42 = 38 + 4. Closed 2026-10-12 (amendment 13; evaluator passes 1–2 PASS) |
| **M6f Contract packs** | OpenAPI specs and gRPC `.proto` services as authoritative endpoints (§4.5); controllers implementing generated interfaces and MyBatis mapper XML (contract-m4b 9); gRPC client stubs as client sites | spec- and proto-driven Q4 goldens; spec endpoints joined to their handlers | Measured 2026-10-12 (contract amendments 9–10): fixture Q4 path golden shows both handlers; grpc-go 52 synthetic CONSUMES sampled correct, 869 correctly unmatched. Closed 2026-10-12 (amendment 11; evaluator passes 1–2 PASS) |
| **M6g Topology and configuration packs** | message topics as `PUBLISHES`/`SUBSCRIBES` keyed by topic literal (§6.5); gateway configurations (the fixture's `nginx.conf`, Spring Cloud Gateway); compose and k8s service names as hosts and `rb.toml` host mappings (contract-m4a 10, 13, 18); Spring YAML, profile and `bootstrap` files read (contract-m4a 3) and active-profile evaluation (moved by contract-m4b) | topic and gateway goldens on `polyglot-monorepo`; each new edge kind with a producer, a fixture and AGE parity (§12.2) | topic and gateway goldens on `topo-packs` (+ polyglot gateway fallback) | Measured 2026-10-12 (contract amendments 8–9): 10 old fixtures `--no-scip` identical plus polyglot/ts-express SCIP identical (invariant 1); kafka 625k rows identical (vacuous, proven); gateway +8 `gateway_unread` (all verified); spring-kafka +808 rows (116 topics, 60 PUB, 107 SUB, 0 false in samples). **Closed 2026-10-12** (amendment 7; evaluator passes 1–2 PASS) |
| **M6h Architecture rules as a CI gate** | `rb check`: rules written in Cypher over the live snapshot's AGE projection (§8.4; §14 open question 4), deterministic violation lists, a non-zero exit for CI, the API's read-only and timeout guard rails | rules-engine golden tests; each rule's result equal to a relational re-computation on the fixtures |
| **M6i Kotlin and C# syntax** | Tree-sitter Kotlin and C# (definitions, references, imports, Tier B resolution); Kotlin exact edges through scip-java in Gradle builds (after M6e); Kotlin class-literal registrants (contract-m5 14, Kotlin part) | per-language fixtures; Tier B goldens; Kotlin exact edges where the Gradle build works | Closed 2026-10-12 (amendments 1–8; evaluator passes 1–2 PASS) |
| **M6j DI breadth** | Guice, Dagger, NestJS; Spring XML and Kotlin bean definitions (moved by contract-m4b); .NET DI over M6i's C# facts (Tier B only) | DI guards per framework; `INJECTS` capped as in M4b | Closed 2026-10-12 (amendments 1–11; evaluator passes 1–2 PASS) |
| **M6k DI recall** | Anchor-at-type for enclosure-less registrations (val-form `beans{}`, minimal-hosting `AddX`); `@Assisted` exclusion; `toProvider`; C# property injection; keyed .NET services; dotted C# namespaces; C# enclosing-namespace scope | Fixture guards per surface; invariant-1 classified; public measurement 0 false | Closed 2026-10-12 (amendments 1–10; evaluator passes 1–2 PASS) |
| **M6l XML diagnostic siting** | Unresolved Spring-XML refs sited in the document instead of the consumer file | spring-xml golden; invariant-1 | Closed 2026-10-13 (amendments 1–3; evaluator passes 1–2 PASS) |
| **M9a `exact` precision from the M8 benchmark** | Contract `tasks/contract-m9a.md`. B7: routes in test files are not endpoints. B8: spec-only endpoints are `synthetic`. B12a: literal `"{id}"` Spring paths. B13: Go method sources take no interface bypass. B14: context paths from a `native` config server's shared files. B11: inherited Spring Data CRUD documented. | contract-m9a C1–C12. Closed 2026-10-16 (amendments 1–4; evaluator passes 1–2 PASS). |
| **M9b Recall from the M8 benchmark** | Contract `tasks/contract-m9b.md`. B9: Go methods declared outside their receiver type's file are its members. B10: `fmt.Sprintf` SQL folds package constants. B12b: Spring Data REST repositories expose `synthetic` endpoints. B1 (cross-module SCIP) moved to M9c: scip-go records no member occurrence of another module (amendment 1). B13b recorded for M9c. | contract-m9b C1–C4, C7–C12 (C5 and C6 withdrawn with B1). Implemented; evaluator pending. |
| **Deferred beyond M6, or dropped** | **Deferred:** scip-clang (C/C++ needs `compile_commands.json` and a C/C++ Tree-sitter language; `sourcegraph/scip-clang` is 356.3 MB compressed); `rust-analyzer scip` (Rust is not yet a Tree-sitter language, §4.4's v1.x); scip-dotnet (`sourcegraph/scip-dotnet` is 1,253.4 MB compressed, beyond this machine's disk budget; C# stays Tier B); the SCIP incremental below a single-root reactor (contract-m5 2: one build root is one SCIP project, §3.4); authentication, remote binding, `POST …/index` and move detection in `rb diff` (contract-m5 19, 20: only if asked for). **Dropped:** Groovy registrants (contract-m5 14, Groovy part: no Groovy grammar is planned; gate M's raw-text scan already caps such points). Contract-m5's residue items 15, 16, 23 and not-planned item 21 stay recorded | — |

---

## 14. Risks and open questions

| Risk | Impact | Mitigation |
|---|---|---|
| SCIP indexers need a **working build**; many repos don't build hermetically | projects fall back to heuristic resolution | Tier B resolver; per-project isolation; coverage report makes it visible; `rb.toml` lets users supply build commands and env |
| Building untrusted repos executes their code | security | containers with no network in the index phase, no credentials, resource limits, disposable volumes; for hostile inputs, a gVisor/Firecracker runtime (v2) |
| **Fetch-phase egress (left open when M3b closed; addressed in M5, below):** the fetch phase runs the project's build (`mvn verify`, all its plugins) and package installs (without lifecycle scripts) on Docker's default bridge network, which reaches any host, and writes the shared `.m2`/`.npm` caches every later job reads | a hostile repository can exfiltrate data or poison the shared caches during its fetch | contract M3b amendment 27 states this honestly; M5 adds an egress allow-list (a user-defined network with a registry proxy) and per-repository caches; until then index only repositories you would build on the host. *As decided in contract-m5 (D15, fork F2):* an `--internal` Docker network whose only peer is a pinned `tinyproxy` container with a default-deny host allow-list (the recipes' registries plus `rb.toml` `[scip] registries`), no DNS in the job, per-repository caches; residue: exfiltration through allow-listed registries, domain fronting, malicious packages. *As built (contract-m5 amendment 12, `RECIPE` `m5-egress-1`):* one tinyproxy per fetch run (`rb-egress-proxy:1.11.3`, built locally from the pinned Alpine base) allowing only `CONNECT <host>:443` to Maven Central and the npm and Yarn registries; extra hosts come from the indexing host (`--scip-registries`, `RB_SCIP_REGISTRIES`), never from `rb.toml`, which the indexed repository controls; without the proxy image no fetch runs. A probe suite from a real fetch container shows the other host, plain HTTP, an IP through the proxy, direct TCP, DNS and UDP all refused; package caches are per repository (spring-retry: 68 → 0 files without a document once the Develocity build cache was off). On mattermost the proxy refused `github.com` git dependencies, as designed |
| Reflection, codegen, event buses and dynamic imports | false negatives in Q1/Q2/Q4 | blind-spot diagnostics; packs for common codegen (Lombok via scip-java annotation processing, protobuf/OpenAPI stubs); topic packs in M6 |
| CHA over-approximation in large OO codebases | noisy impact reports | RTA filter as an opt-in (`--precision rta`; contract-m4b amendment 21); certainty labels; per-query precision flag |
| URL stitching with env-driven base URLs | wrong or missing cross-service edges | host → compose/k8s service matching; literal defaults; OpenAPI/Feign/gRPC exact matching; `ambiguous` rather than guessing |
| AGE maturity (feature gaps, performance of variable-length patterns, PG version lag) | ad-hoc queries slow or unsupported | AGE is a projection, not the source of truth. Hot paths never depend on it. Pin versions; parity tests. |
| AGE 1.8 on PG 18 under concurrent graph DDL: backends intermittently die with `FATAL: failed to re-find shared lock object` (observed 2026-09-30 during parallel test runs that create and drop graphs from several processes; never serially; a read-only Cypher write attempt alone does not trigger it) | an `rb index` run can fail when another process is creating or dropping graphs in the same database | The e2e tests are serialized. Concurrent indexing into one database is not supported until the cause is found upstream; M5's bulk loading should take a per-database advisory lock around graph DDL. *As decided and measured in contract-m5 (D10, S2):* every `create_graph`/`drop_graph` takes that lock, one graph per transaction. Measured under four concurrent publishers (10 forced rebuilds each) plus a Cypher reader loop, three runs per binary: **0** lock crashes with the M4b binary (`afae548`, not reproduced: 120 publishes, 868 reader runs) and 0 with the M5 binary (120 publishes, 1,984 reader runs); the lock stays as the mitigation. |
| Monorepo scale (10 M+ LOC) | index time, memory | per-project parallel SCIP; content-addressed caches; CSR memory is ≈ 6 B/edge/direction; partition-per-snapshot (v2) |
| Symbol-key churn across indexer versions | spurious diffs between snapshots | pin indexer digests; `rb diff` normalises across tool-version changes by re-indexing both snapshots with the same tools when they differ. *As decided and built in contract-m5 (D12):* `rb diff` never re-indexes; its header names the `tool_versions` keys that differ (JSON `tools_differ`), and re-indexing both snapshots with the same tools is a user action (`rb index --rev <commit> --force`). Measured: a forced rebuild with the same tools diffs empty on every fixture and on both benchmark repositories |
| Licensing of the bundled components | distribution constraints | expected: Tree-sitter MIT, libpg_query BSD-3, SCIP Apache-2.0, AGE Apache-2.0, gix MIT/Apache-2.0. **Not yet checked.** A licence audit (`cargo deny`) is an M0 exit item. |

**Open questions** for the requester:

1. **Multi-repo.** Should one Brain span several repos, with cross-repo `CONSUMES` and SCIP cross-package links? The schema supports it through `repo_id`, but stitching and snapshots across repos need a "workspace snapshot" concept.
2. **Freshness trigger.** Index on push via a CI step, run a daemon that polls, or both?
3. **Test code.** Should test symbols be first-class members of the graph (useful for "which tests cover X"), or hidden by default?
4. **Architecture rules.** Is a CI gate (M6) a goal, or only exploration?

---

## Appendix A — ADR-001: Implementation language

**Decision:** **Rust** for all core components (indexer, query engine, server, CLI). SCIP indexers stay in their upstream languages, inside containers.

**Context:** the workload is CPU-bound parsing of many files, graph assembly over millions of edges, and an in-memory traversal server. The critical native libraries are Tree-sitter (C), libpg_query (C) and SCIP (protobuf).

| Criterion | Rust | Go | Python | C++ |
|---|---|---|---|---|
| Tree-sitter bindings | Official, first-class; grammars ship as crates | Community bindings via cgo | Official `py-tree-sitter` | Native C API, no packaging story for grammars |
| pg_query | `pg_query` 6.2 (PG17 parser; `tables()`, `select_tables()`, `dml_tables()`, `ddl_tables()`, `statement_types()` helpers) | `pg_query_go` (the most mature binding) | `pglast` | libpg_query directly |
| SCIP | `scip` 0.10 (official, generated from `scip.proto`) | Official Go bindings (SCIP's own tooling is Go) | protobuf-generated | protobuf-generated |
| Git | `gix` (pure Rust, fast) | `go-git` | `pygit2` / subprocess | libgit2 |
| Parallel parsing | rayon, no GIL, no cgo overhead | goroutines, but every Tree-sitter call crosses cgo | GIL-bound; needs multiprocessing plus serialization | threads, manual safety |
| In-memory graph server | compact CSR, predictable memory | good, with GC overhead on huge slices | poor at 10 M+ edges | best raw performance |
| Distribution | single static binary | single static binary | interpreter + native wheels | per-platform builds |
| Development speed | medium | high | highest | lowest |
| Safety in parser-heavy code | memory-safe | memory-safe | memory-safe | manual |

**Why not C++:** every library it would call is already C, and Rust calls those at zero cost. So C++ gains no performance over Rust and loses memory safety, package management and the maintained bindings.

**Why not Python:** it is fine for a prototype, but the GIL makes the per-file extraction stage (the bulk of CPU time) scale poorly, and a Python CSR server at 10 M+ edges is memory-hungry.

**Go** is a credible second choice. SCIP's own tooling is written in Go, and `pg_query_go` is the reference binding. It loses mainly on cgo overhead in the Tree-sitter hot loop.

**Consequences:**

- Contributors need Rust.
- `cargo`/rustup are not installed on the current dev machine, so the toolchain has to be installed via rustup at M0.
- If Go is chosen instead, the architecture in this document is unchanged: nothing here depends on the implementation language.

---

## Appendix B — Glossary

- **CHA / RTA / VTA**: Class Hierarchy Analysis, Rapid Type Analysis and Variable Type Analysis. These are successively more precise ways of resolving virtual calls.
- **CSR**: compressed sparse row, an array-based adjacency representation.
- **Snapshot**: an immutable graph for one commit and one set of tool versions.
- **Pack**: a versioned bundle of Tree-sitter queries and handlers that teaches the Brain one framework.
- **Certainty**: an ordered label on each edge (`exact` > `dispatch` > `synthetic` > `heuristic` > `ambiguous`), stating how the edge is known.

---

## Appendix C — Verified external facts (checked 2026-09-27)

These facts were checked against primary sources while writing this document. **Re-check them at M0**, since several projects moved in 2026.

| Component | Fact used in this design | Source |
|---|---|---|
| Apache AGE | Latest 1.8.0 (PG18, PG15 lines). PG17 → 1.7.0, PG16 → 1.6.0. Docker `apache/age:release_PG18_1.8.0`. | github.com/apache/age/releases, hub.docker.com/r/apache/age |
| Apache AGE | `age_shortest_path` / `age_all_shortest_paths` SQL functions (1.8+, single edge type); no `shortestPath()` pattern syntax | apache/age PR #2430 |
| Apache AGE | VLE worst case O(E!) unbounded; open performance issues #2350 and #2481 | `src/backend/utils/adt/age_vle.c`, apache/age issues |
| Apache AGE | Loader paths are restricted to the server's `/tmp/age/` (no setting changes this); agtype-mode CSV loading; sparse vertex ids work. Verified by running them in M0. | `compose.yaml`, `crates/rb-store/src/age.rs` |
| Apache AGE | `cypher()` params only via prepared statements; `load_labels_from_file` / `load_edges_from_file` CSV loaders | age-website docs (`advanced/prepared_statements.md`, `intro/agload.md`) |
| SCIP | Repo now `github.com/scip-code/scip`; crate `scip` 0.10.0; `Relationship.is_{reference,implementation,type_definition,definition}`; `typed_enclosing_range` (legacy `enclosing_range` deprecated) | crates.io/crates/scip, `scip.proto` |
| SCIP indexers | scip-java 0.13.1 (`ghcr.io/scip-code/scip-java`), scip-typescript 0.4.0, scip-python 0.6.6, scip-go 0.2.7, scip-clang 0.4.0, scip-ruby 0.5.0, scip-dotnet 0.2.14, `rust-analyzer scip` | respective GitHub repos and registries |
| pg_query.rs | 6.2.0 on libpg_query 17-6.2.2 (PG17 parser); `tables()`, `select_tables()`, `dml_tables()`, `ddl_tables()`, `statement_types()` | github.com/pganalyze/pg_query.rs |
| sqlparser-rs | 0.63.0, now `apache/datafusion-sqlparser-rs` | crates.io/crates/sqlparser |
| Rust crates | `tree-sitter` / `tree-sitter-tags` 0.27.0; `gix` 0.88.0 (`blob-diff` feature) | crates.io |
| CODEOWNERS | Last matching pattern wins; no `!`, `[]`, `\#` | docs.github.com, "About code owners" |
| PostgreSQL | `CYCLE … SET … USING …` and `SEARCH` introduced in PG14 | postgresql.org/docs/14/queries-with.html |

**Not verified:**

- whether scip-typescript encodes file paths in its descriptors (§5.1 handles either case);
- gix's exact tree-diff and blob-read API surface;
- scip-dotnet and scip-ruby build requirements;
- component licences (an M0 audit item).
