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
| `rb-model` | IDs, node and edge kinds, `FileFacts`, `certainty`/`provenance` enums, serde and bincode formats |
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
- **Change set.** A tree diff between the commit of the previous live snapshot and the target commit gives added, modified, deleted and renamed files. That diff drives cache lookups and project invalidation (§3.4).
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

v1 languages: TypeScript/JavaScript (incl. TSX), Java, Python and Go. v1.x adds Kotlin, C#, Rust and C/C++. Grammars are compiled in as crates at pinned versions.

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

**Consequence:** a SCIP symbol is a stable, human-readable, cross-file and cross-package natural key. The grammar has no file-path field. However, the spec lets descriptors encode ancestry from the file root, and some indexers (reportedly scip-typescript) emit path segments as namespace descriptors. **Symbol keys can therefore change when a file moves in some languages.** `rb diff` (§11) treats a disappear-plus-appear pair with an identical definition body hash as a *move*, not a delete plus an add.

### 5.2 Indexer orchestration (`rb-scip`)

| Language | Indexer | Needs a working build? |
|---|---|---|
| Java / Kotlin (Scala per the SCIP README) | scip-java v0.13.x (`ghcr.io/scip-code/scip-java`; the Docker Hub `sourcegraph/scip-java` image stopped at 0.12.3) | Yes: runs the Maven/Gradle compile. Bazel needs manual configuration. |
| TypeScript / JavaScript | scip-typescript v0.4.x (`sourcegraph/scip-typescript`) | Needs `tsconfig`/`jsconfig` and an installed `node_modules` (`npm`/`yarn install` in the fetch phase) |
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
  1. A **fetch** phase with network access, allowed only to package registries, which populates a dependency cache volume.
  2. An **index** phase with the network disabled.
  No credentials are ever mounted. This also keeps the host's toolchains (e.g. a host npm) out of the loop entirely.
- If a project fails to build, the run records `scip_status=failed` for it, keeps its stderr tail, and drops that project to Tier B heuristic resolution (§5.4). One failing project never fails the run.
- The output, `index.scip`, is stored content-addressed under `(project_input_hash, indexer_digest)`. `project_input_hash` hashes the project's file blob SHAs plus its lockfiles.

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
- A definition occurrence becomes a `Symbol` node, with a `DEFINED_IN` edge to the File and the range stored.
- `SymbolInformation.relationships` with `is_implementation` become `IMPLEMENTS` edges, from the impl type or method to the interface or overridden method.

Framework facts attach to SCIP symbols through the same range join. For example, the handler method name in a `Route` fact joins to that method's definition occurrence.

### 5.4 Tier B: heuristic resolution (no SCIP, or SCIP failed)

For a call named `f` in file `F`, candidates are the definitions named `f` in:

1. the enclosing class or module scope;
2. the same file;
3. files `F` imports (import paths resolved per language: relative paths, `tsconfig` `paths`, Java package plus class name, Go import path to directory, Python module path);
4. the same package or directory.

The search stops at the first level that yields candidates. If there is **exactly one** candidate, the result is an edge with certainty `heuristic`. If there are **several**, the result is one edge to each, with certainty `ambiguous` and attr `candidates=n`. If there are **none**, the call site is recorded as `unresolved` and counted in coverage. Method calls on receivers of unknown type (`x.save()`) resolve only when the receiver was declared in-file with a type. Otherwise they stay `unresolved` rather than linking by bare name to every `save`.

Heuristic symbol keys use a synthetic SCIP-shaped scheme, e.g. `rb-heuristic . <project> . <path-derived descriptors>`, so every node kind shares one key space. When SCIP later succeeds for that project, the keys are replaced wholesale.

### 5.5 External packages

SCIP symbols from other packages (manager, package name, version) become `Package` nodes, with `REFERENCES`/`CALLS` edges to external `Symbol` stubs. Lockfiles (`package-lock.json`, `pnpm-lock.yaml`, `go.sum`, Maven dependency tree output from the fetch phase, `poetry.lock`/`uv.lock`) give `Service -DEPENDS_ON-> Package` with resolved versions. This answers "who uses library X" and feeds the external section of the Service Card.

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
  Each `IMPLEMENTS` edge carries `impl_instantiated: bool`. Queries default to `precision=rta`, which skips overriders of never-instantiated types. `precision=cha` includes them all, and gives the conservative answer for Q2.

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

### 7.2 Parsing

1. **Normalise the template.** Replace each `${…}`, `+ expr +` or f-string hole with a positional parameter `$n`. If a hole sits in identifier position (e.g. a dynamic table name), mark the statement `dynamic_identifier` and do not guess the table.
2. **Parse with pg_query**, which uses the real PostgreSQL parser. The crate's `select_tables()`/`dml_tables()`/`ddl_tables()` helpers are a useful cross-check, but they don't separate insert, update and delete. So `rb-sql` walks the parse tree itself to collect `(table, schema, op)` per statement:
   - `op ∈ {read, insert, update, delete, ddl, truncate}`;
   - CTE names are excluded from table sets;
   - subquery and `JOIN` tables count as reads;
   - `INSERT … SELECT` gives a write to the target and reads from the sources.
3. **Fallback.** On a pg_query failure, try `sqlparser-rs` with the dialect configured in `rb.toml` (MySQL, SQLite, MSSQL, BigQuery…). Results get certainty `heuristic`.
4. On total failure, record `unparsed_sql` with its location. These records count in the coverage report.
5. **JPQL/HQL is not SQL.** v1 uses a small JPQL tokenizer that extracts `FROM`/`JOIN` entity names and maps them through `Entity` facts to tables, with certainty `heuristic`.

### 7.3 Tables as first-class nodes

- The table key is `schema.table`.
- An unqualified name resolves to the schema configured in `rb.toml`, defaulting to `public`. It is flagged `schema_assumed` because `search_path` is not visible statically.
- DDL in migrations produces the canonical table set. Tables referenced in code but never created in migrations are kept, flagged `undeclared`. These are often views or tables owned by another repo.
- Edges:
  - `Symbol -QUERIES{op}-> Table`, where the symbol is the innermost enclosing function of the SQL literal or sink call;
  - `Symbol(type) -MAPS_TO-> Table`, for ORM entities;
  - `File(migration) -MIGRATES{op}-> Table`.

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
| `Endpoint` | `HTTP <METHOD> <normalised path>` (path parameters → `{}`), or `GRPC <pkg.Service>/<Method>` |
| `Package` | `<manager> <name>`; the version sits on the edge |

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
  facts      bytea NOT NULL,                   -- bincode FileFacts, zstd
  PRIMARY KEY (blob_sha, extractor)
);

CREATE TABLE rb.diagnostic (                   -- unresolved calls, unparsed SQL, failed builds, blind spots
  snapshot_id bigint NOT NULL, kind text NOT NULL, file_id bigint, span int4[], detail jsonb
);
```

Loading uses `COPY … FROM STDIN (FORMAT binary)` into the `building` snapshot. The last step, `UPDATE rb.repo SET live_snapshot_id = …`, runs in a single transaction, so readers never see a partial graph. Retired snapshots are kept for N runs (default 3) to enable diffing (§11), then dropped. A partition-per-snapshot scheme (v2) would make that drop a cheap `DROP TABLE`.

### 8.4 The AGE projection and what it is for

For each live snapshot, the loader creates a graph `rb_<repo>_<snapshot_id>`, with vertex labels `Symbol, File, Service, Table, Owner, Endpoint, Package` and one edge label per edge kind. Each vertex carries only `{nid, key, name}`. Each edge carries `{provenance, certainty}`, plus `op` on `QUERIES`. Everything else is looked up relationally by `nid`, so the projection stays thin and cheap to rebuild.

- **Bulk load.** The loader exports per-label CSV from the relational tables and loads it with AGE's `load_labels_from_file` / `load_edges_from_file`. The CSV files must be readable by the Postgres server process, and AGE 1.8 only reads loader paths under the server's `/tmp/age/`. So `compose.yaml` mounts `./var/age-load` at `/tmp/age/rb-load`, and the indexer writes there. The files use `load_as_agtype` mode: each property cell is a JSON value, so numbers stay numeric (`e.certainty >= 5` works) and strings round-trip exactly. Vertex CSV ids are the relational `nid`s, which may be sparse. The whole load (rows, AGE graph and live flip) runs in **one transaction**. A failed load leaves only a `failed` snapshot row, and retention removes that row later. It never issues row-by-row `CREATE` through `cypher()`, which is orders of magnitude slower. The projection is built *before* the live flip, and the old graph is dropped (`drop_graph(name, true)`) when its snapshot is retired.

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

**In-memory CSR.** At a live flip, rb-server loads `(src, dst, kind, certainty)` into two compressed sparse row arrays, one outbound and one inbound, each partitioned by edge kind. Sizing: 10 M edges × (4 B dst + 1 B kind + 1 B certainty) ≈ 60 MB per direction. BFS over that is sub-millisecond to low-millisecond. Postgres remains the durable store, and CSR is a cache rebuilt from it.

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
| `DEPENDS_ON` | Service → Service/Package | derived (§9.5), lockfiles | `via` = calls/http/package |

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

### 9.6 Q4 — "Show the request path for login"

Q4 is covered in §10, since it needs cross-service stitching.

---

## 10. Cross-service request paths (Q4)

### 10.1 Endpoint normalisation

Server routes and client URLs are normalised to one form:

- Method is upper-cased. Path parameters (`:id`, `{id}`, `<int:id>`, `*`) become `{}`.
- Duplicate slashes collapse, and trailing slashes are stripped.
- Configured global prefixes are prepended. Examples: `server.servlet.context-path` from `application.yml`/`.properties`, NestJS `setGlobalPrefix` and Express mount prefixes. Profile-specific values (`application-prod.yml`) produce one variant per profile, flagged.
- Gateway rewrite rules are applied when their config is present (nginx `location`/`proxy_pass`, k8s Ingress paths, Spring Cloud Gateway routes, v1.x). Each rewrite adds an `Endpoint` alias.

### 10.2 Stitching client call sites to endpoints (`http_stitch`)

For each `HttpCall{method, url_template, base_ref}`:

1. **Constant-fold the URL.** Resolve `base_ref` if it is a same-module constant, an imported constant, or an env var with a literal default (`process.env.X ?? 'http://auth:8080'`, `@Value("${auth.url:http://auth}")`). The result is a normalised path plus an optional host.
2. **Pick the target service from the host.** If the host names a compose/k8s service (§4.3), restrict candidates to that service. The match is `exact`.
3. **Match paths.** Otherwise, match the path against all endpoints. An exact normalised match is `exact`. A unique suffix match (unknown base prefix) is `heuristic`. Several matches are `ambiguous`, with an edge to each.
4. **Unmatched** calls become diagnostics, e.g. calls to third-party APIs. The host is still recorded as an external `Endpoint` node with `external=true`.
5. **Generated clients** (OpenAPI generator, Feign, gRPC stubs) are matched by operation ID or RPC name. This is `exact`, and it is the most reliable path, so fixture coverage should favour it.

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

**Honest framing:** this is the set of *statically possible* paths, not the path a particular request took. Branch conditions are not evaluated. The output makes this clear with a header line.

```
Request path: POST /api/v1/login   (snapshot 42, precision=rta)
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

### 12.5 Scale benchmark (targets, not measurements)

- A large OSS Java monorepo and a large TS monorepo (candidates chosen at M5).
- Targets on a 16-core machine:
  - full index of about 1 M LOC in ≤ 15 min excluding dependency fetch, with the SCIP build dominating;
  - an incremental single-file commit that does not trigger a project re-index in ≤ 30 s;
  - Q1 depth-1 in ≤ 50 ms p95;
  - Q2 on a hub symbol in ≤ 500 ms p95 via CSR;
  - Q4 in ≤ 1 s p95.

---

## 13. Phased roadmap

Each milestone is shippable and has an exit criterion that is a test, not a demo.

| Milestone | Contents | Exit criteria |
|---|---|---|
| **M0 Foundations** | Rust workspace; Docker Compose with Postgres + AGE (pinned digest); `rb.*` migrations; snapshot lifecycle; `rb index` manifest-only; AGE projection of `File/Service` (Owner moved to M1, because it needs CODEOWNERS); `rb dump` | determinism test green on 2 fixtures; `rb cypher` returns files per service. **Done 2026-09-29.** |
| **M1a Structure** | CODEOWNERS; service boundaries + `rb.toml`; edge evidence; `rb find`, `rb service` (structure sections) | GitHub-example CODEOWNERS tests; golden Service Cards on `polyglot-monorepo`. **Done 2026-09-29.** |
| **M1b Syntax** | `gix` change sets; Tree-sitter defs/refs/scopes for TS, Java, Python, Go; facts cache | per-language extraction goldens; warm-cache run = cold-cache run |
| **M2 Data access** | `rb-sql`: sinks, template normalisation, pg_query, sqlparser fallback, JPQL tokenizer; ORM packs (JPA, Prisma, TypeORM, SQLAlchemy); `QUERIES/MAPS_TO/MIGRATES` | `rb table-deps` golden at the symbol level (service roll-up via file→service); SQL coverage metrics |
| **M3 Semantics** | SCIP orchestration (scip-java, scip-typescript first); SCIP⋈TS join; `CALLS/REFERENCES/IMPLEMENTS/CONTAINS`; Tier B heuristic resolver; CSR in rb-server; `rb callers`, `rb impact` | Q1/Q2 goldens on all fixtures, including `no-build`; incremental-equivalence test green |
| **M4 Frameworks** | route packs, HTTP-client packs, stitching; Spring DI + Spring Data; RTA filter; derived `DEPENDS_ON`; `rb path`; full Service Card | Q3/Q4 goldens on `polyglot-monorepo`; precision guards (must-not-exist edges) green |
| **M5 Scale & API** | REST API with guard rails; bulk AGE loading; snapshot retention; `rb diff`; benchmark on 2 large OSS repos | §12.5 targets met or gaps documented with profiles |
| **M6 Breadth** | scip-python/go/clang/rust-analyzer; Kotlin/C#; OpenAPI/gRPC packs; gateway configs; message topics; architecture-rule checks in Cypher as a CI gate | per-language fixtures; rules engine golden tests |

---

## 14. Risks and open questions

| Risk | Impact | Mitigation |
|---|---|---|
| SCIP indexers need a **working build**; many repos don't build hermetically | projects fall back to heuristic resolution | Tier B resolver; per-project isolation; coverage report makes it visible; `rb.toml` lets users supply build commands and env |
| Building untrusted repos executes their code | security | containers with no network in the index phase, no credentials, resource limits, disposable volumes; for hostile inputs, a gVisor/Firecracker runtime (v2) |
| Reflection, codegen, event buses and dynamic imports | false negatives in Q1/Q2/Q4 | blind-spot diagnostics; packs for common codegen (Lombok via scip-java annotation processing, protobuf/OpenAPI stubs); topic packs in M6 |
| CHA over-approximation in large OO codebases | noisy impact reports | RTA filter by default for Q1/Q4; certainty labels; per-query precision flag |
| URL stitching with env-driven base URLs | wrong or missing cross-service edges | host → compose/k8s service matching; literal defaults; OpenAPI/Feign/gRPC exact matching; `ambiguous` rather than guessing |
| AGE maturity (feature gaps, performance of variable-length patterns, PG version lag) | ad-hoc queries slow or unsupported | AGE is a projection, not the source of truth. Hot paths never depend on it. Pin versions; parity tests. |
| Monorepo scale (10 M+ LOC) | index time, memory | per-project parallel SCIP; content-addressed caches; CSR memory is ≈ 6 B/edge/direction; partition-per-snapshot (v2) |
| Symbol-key churn across indexer versions | spurious diffs between snapshots | pin indexer digests; `rb diff` normalises across tool-version changes by re-indexing both snapshots with the same tools when they differ |
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
