# Repository Brain

A deterministic, LLM-free code-intelligence graph for Git repositories. It is built with Rust, Postgres and Apache AGE.
The design is in [docs/repository-brain-design.md](docs/repository-brain-design.md).
The operator's guide (usage + testing) is [docs/usage-and-testing.md](docs/usage-and-testing.md).

**Status: milestones M0 (Foundations), M1a (Structure), M1b (Syntax), M2a (SQL core), M2b (ORM mappings), M3a (heuristic resolution), M3b (SCIP), M4a (endpoints and request paths), M4b (DI and dispatch), M5 (scale and API), M6a–M6h (SCIP breadth, topology, gateway, YAML, rules engine), M6i (Kotlin and C# syntax), M6j (DI breadth: Guice, Dagger, NestJS, Spring XML, Kotlin beans, .NET), M6k (DI recall: val-form beans, minimal hosting, assisted exclusion, providers, C# properties, keyed services, C# namespaces) and M6l (XML diagnostic siting) are done.** `rb` indexes a commit into a snapshot containing:

- one `File` node per blob, with its blob SHA, size, language and binary/generated flags;
- one `Service` per detected project (package.json, Maven, Gradle, go.mod / `package main`, Cargo, pyproject, Dockerfile, `rb.toml`), marked deployable or library;
- `Owner` nodes from CODEOWNERS (GitHub semantics), with `OWNED_BY` edges that cite the deciding CODEOWNERS line;
- `Symbol` nodes for the definitions Tree-sitter finds in TypeScript/TSX/JavaScript, Java, Python, Go, Kotlin and C# files, with `DEFINED_IN` (pointing at the name) and `CONTAINS` edges. Parse results are cached per blob, so a re-index only parses changed files.
- `Table` nodes from SQL parsed with the real Postgres parser. `MIGRATES` edges come from `.sql` files, `QUERIES` edges from SQL strings in code (exact when passed to a known database call) and from ORM calls, and `MAPS_TO` edges from ORM entity mappings (JPA, Prisma, TypeORM, SQLAlchemy, Django).
- `CALLS`, `REFERENCES` and `IMPLEMENTS` edges between symbols: exact (provenance `scip`) where a SCIP indexer ran for the project in a container, heuristic otherwise (resolved from imports, packages and typed bindings, never by bare name across the repository). C# is Tier B only (no SCIP): namespaces (block, file-scoped, dotted), enclosing-namespace scope and the global namespace all resolve.
- `Package` nodes and `DEPENDS_ON` edges from the external symbols a SCIP index names (provenance `scip`) and from manifests (provenance `manifest`: `package.json` with `package-lock.json` v2/v3, `go.mod`, `pom.xml` literals and same-pom properties; `@?` when the version cannot be read), plus Service → Service `DEPENDS_ON` (`via: package`) when a manifest names another service of the repository.
- `INJECTS` edges (provenance `di`, never above `heuristic`) from dependency injection: Spring constructor (Lombok-generated included), field, setter and `@Bean`-parameter points; Guice (`@Singleton`, `@Provides`, `bind().to()`/`toProvider()`, `@Assisted` exclusion) and Dagger (`@Module`, `@Binds`, `@Provides`); NestJS injectables and controllers; Spring XML `<bean>` (constructor-arg/property refs, aliases, component scan); Kotlin `beans { }` (function form and top-level `val` form, `ref()` points); .NET `Add/TryAdd/Keyed` registrations (including top-level minimal hosting), constructor and `[Inject]`/`[FromServices]`/`[FromKeyedServices]` property points with key matching. Points resolve to the stereotype class, `@Bean` method, Spring Data repository, Feign client or bound-type definition they reach, narrowed by qualifiers/keys/names, with `resolution`, `origin`, `via` and `capped_by` attributes; several candidates give `ambiguous` edges.
- `impl_instantiated: false` on `IMPLEMENTS` edges out of a type proven never instantiated (RTA), and the `Symbol` attributes `entry` (`main`, `test`, `scheduled`, `listener`, `lifecycle`) and `orphan_eligible`.
- `Endpoint` nodes from Spring MVC, Express, gin and Go `net/http` routes, with `EXPOSES` (service → endpoint) and `HANDLED_BY` (endpoint → handler); `CONSUMES` edges (provenance `http_stitch`, never above `synthetic`) from `fetch` (with `node-fetch` and `cross-fetch`), axios, Feign, `RestTemplate`, `WebClient`/`RestClient` and Go `net/http` client calls to other services' endpoints (contract `tasks/contract-m4a.md`). An endpoint key is `HTTP <METHOD> <path> @<service>`; when a route's prefix cannot be composed it is flagged `prefix_unknown` and its key ends in ` ?router=<file>#<binding>`.
- `IN_SERVICE` edges and diagnostics (skipped files, ignored or invalid CODEOWNERS lines, unreadable manifests, syntax errors, oversized files, unresolved references — sited where the reference is written — one `scip_status` per build root, route diagnostics and one bucket per HTTP client call that was not stitched).
- Architecture-rule violations via `rb check` (rules `orphan_endpoints`, `unowned_table_writes`, `service_cycles`; exit 0 clean, 1 violations, 2 errors).

Each snapshot is stored relationally and projected into an AGE graph.

## Quickstart

```sh
docker compose up -d --wait            # Postgres 18 + AGE 1.8.0 on 127.0.0.1:55432 (RB_DB_PORT moves it)
cargo build
target/debug/rb migrate
target/debug/rb index /path/to/repo    # --rev <rev>, --repo <name>, --force, --no-cache, --keep <n>
target/debug/rb status
target/debug/rb dump > snapshot.jsonl  # canonical, id-independent dump
target/debug/rb service                # list services; `rb service <dir|name>` shows a card
target/debug/rb find login --kind symbol  # search nodes by key or name
target/debug/rb table-deps users         # migrations, code and callers up to entry points per service (Q3)
target/debug/rb callers AuthService.login --depth 2   # what calls a function or method (Q1)
target/debug/rb impact "AuthService#login()."         # what breaks if it is removed (Q2): endpoints, orphans, tests; --json
target/debug/rb path "POST /api/v1/login"             # the request path through an endpoint (Q4); --format json, --pick N
target/debug/rb cypher "MATCH (f:File)-[:IN_SERVICE]->(s:Service) RETURN s.name, count(f)" --cols service,files
target/debug/rb check --repo myrepo                   # architecture rules: exit 0 clean, 1 violations
target/debug/rb snapshots             # kept snapshots (live, retired, failed), newest first
target/debug/rb diff                  # the live snapshot against its parent: endpoints, table access, dependencies, nodes, edges
target/debug/rb serve                 # the REST API on 127.0.0.1:55480 (read-only, loopback only)
target/debug/rb gc --dry-run           # graphs and `building` rows interrupted or older runs left (`--egress`: the fetch networks too)
target/debug/rb cache prune --facts --scip --dry-run   # cache rows of other extractor versions and images
```

Run `rb` from the project root. AGE's CSV loaders only read files under the server's `/tmp/age/`, and `compose.yaml` mounts `./var/age-load` there. From anywhere else, set `RB_AGE_LOAD_DIR` to that directory.

| Setting | Default |
|---|---|
| `RB_DATABASE_URL` (`--database-url`) | `postgresql://rb:rb@127.0.0.1:55432/rb` |
| `RB_AGE_LOAD_DIR` (`--age-load-dir`) | `var/age-load` |
| `RB_KEEP_RETIRED` (`rb index --keep`) | `2` retired snapshots kept besides the live one (0–20) |
| `RB_CYPHER_TIMEOUT_MS` (`rb cypher --timeout-ms`, `rb serve --cypher-timeout-ms`) | `2000` (1–30,000) |
| `RB_SCIP_WORK_DIR` (`--scip-work-dir`) | `var/scip` |
| `RB_SCIP_REGISTRIES` (`--scip-registries`) | none (extra hosts for the fetch phase, below) |
| `RB_SCIP_CPUS`, `RB_SCIP_MEMORY`, `RB_SCIP_MAVEN_THREADS` (`--scip-cpus`, `--scip-memory`, `--scip-maven-threads`) | `2`, `4g`, `1` (the SCIP containers' budget, below) |

The database's default port is 55432. If it is taken, start the database on another port with `RB_DB_PORT=55433 docker compose up -d --wait` and point `RB_DATABASE_URL` at it; the development machine of record runs it on 55433, so the examples and measurements in this repository use `postgresql://rb:rb@127.0.0.1:55433/rb`.

`rb index --profile` also prints the AGE projection's load per label file (rows, CSV bytes, time); wall times are printed only, never stored.

### Snapshots, JSON and the REST API (M5)

The examples in this section were run as printed on 2026-10-07 from the project root, with `RB_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb` and a copy of `fixtures/polyglot-monorepo` committed twice in `$FIXTURE` (the second commit adds a `GET /internal/mfa/status` route that reads `email_log`), indexed as the only repository `m5r_polyglot` and deleted afterwards. Snapshot ids come from the database's sequence and differ on every machine.

```
$ target/debug/rb index "$FIXTURE" --repo m5r_polyglot --no-scip --profile
m5r_polyglot: snapshot 15449 live @ 45240eff4a4e — 105 nodes, 174 edges, 28 diagnostics; AGE graph rb_m5r_polyglot_15449; parsed 0 source files; scanned 21 other files as text
  profile: age Endpoint (Endpoint.csv): 7 rows, 616 bytes, 4.7 ms
  profile: age File (File.csv): 37 rows, 2204 bytes, 1.7 ms
  profile: age Owner (Owner.csv): 7 rows, 601 bytes, 1.7 ms
  profile: age Package (Package.csv): 7 rows, 601 bytes, 1.5 ms
  profile: age Service (Service.csv): 9 rows, 377 bytes, 2.7 ms
  profile: age Symbol (Symbol.csv): 34 rows, 3535 bytes, 1.6 ms
  profile: age Table (Table.csv): 4 rows, 163 bytes, 1.5 ms
  profile: age CALLS (CALLS.csv): 5 rows, 257 bytes, 1.6 ms
  profile: age CONSUMES (CONSUMES.csv): 6 rows, 319 bytes, 1.8 ms
  profile: age CONTAINS (CONTAINS.csv): 11 rows, 497 bytes, 1.7 ms
  profile: age DEFINED_IN (DEFINED_IN.csv): 34 rows, 1320 bytes, 2.9 ms
  profile: age DEPENDS_ON (DEPENDS_ON.csv): 8 rows, 389 bytes, 1.5 ms
  profile: age EXPOSES (EXPOSES.csv): 7 rows, 365 bytes, 1.9 ms
  profile: age HANDLED_BY (HANDLED_BY.csv): 7 rows, 354 bytes, 1.6 ms
  profile: age INJECTS (INJECTS.csv): 2 rows, 133 bytes, 1.5 ms
  profile: age IN_SERVICE (IN_SERVICE.csv): 37 rows, 1403 bytes, 2.7 ms
  profile: age MIGRATES (MIGRATES.ops.csv): 3 rows, 222 bytes, 1.9 ms
  profile: age OWNED_BY (OWNED_BY.csv): 48 rows, 1829 bytes, 4.1 ms
  profile: age QUERIES (QUERIES.ops.csv): 6 rows, 393 bytes, 1.7 ms
  profile: age total: 19 files, 279 rows, 15323 bytes, 39.2 ms
  scip: apps/web scip-typescript skipped unpinned: SCIP disabled (--no-scip)
  scip: packages/shared scip-typescript skipped unpinned: SCIP disabled (--no-scip)
  scip: services/auth scip-java skipped: SCIP disabled (--no-scip)
  scip: services/broken scip-java skipped: SCIP disabled (--no-scip)
  scip: services/notification scip-typescript skipped unpinned: SCIP disabled (--no-scip)

$ target/debug/rb index "$FIXTURE" --repo m5r_polyglot --no-scip --keep 1
m5r_polyglot: snapshot 15450 live @ 6771067e0a71 — 106 nodes, 176 edges, 28 diagnostics; AGE graph rb_m5r_polyglot_15450; parsed 0 source files; scanned 21 other files as text
  scip: apps/web scip-typescript skipped unpinned: SCIP disabled (--no-scip)
  scip: packages/shared scip-typescript skipped unpinned: SCIP disabled (--no-scip)
  scip: services/auth scip-java skipped: SCIP disabled (--no-scip)
  scip: services/broken scip-java skipped: SCIP disabled (--no-scip)
  scip: services/notification scip-typescript skipped unpinned: SCIP disabled (--no-scip)

$ target/debug/rb snapshots
15450	6771067e0a71	live	parent 15449	AGE graph rb_m5r_polyglot_15450
15449	45240eff4a4e	retired	parent -	AGE graph -
```

`--keep 1` kept one retired snapshot beside the live one; only the live one has an AGE graph.

#### `rb diff`

`rb diff [<from>] [<to>]` compares two kept snapshots by natural identity (nodes by kind and key, edges by both ends, kind and provenance), so the node and edge sections are exactly the difference of the two canonical dumps. A selector is a snapshot id or a commit prefix (4–40 hex digits) unique among the kept snapshots; `from` defaults to the parent of `to`, `to` to the live snapshot. It summarises endpoints, table access (`QUERIES`, `MAPS_TO`, `MIGRATES`) and service dependencies with the Service Card's rules, then lists nodes and edges added, removed and changed (edges whose only change is their evidence apart, `--evidence` lists them) and the diagnostics per kind (`--diagnostics` lists them). Moves are not inferred (a renamed symbol is a removal and an addition), nothing is re-indexed, and when the two snapshots were built with different tool versions the header names the keys that differ. Text shows the first 50 rows of each section; `--json` pages every section with `--limit` (1–1,000) and `--offset`.

```
$ target/debug/rb diff
diff snapshot 15449 of m5r_polyglot @ 45240eff4a4e → snapshot 15450 of m5r_polyglot @ 6771067e0a71
endpoints: 1 added, 0 removed
  + GET /internal/mfa/status  [notification]
table access: 1 added, 0 removed
  + services/notification  public.email_log  read
service dependencies: 0 added, 0 removed
certainty: no edge changed certainty
nodes: 1 added, 0 removed, 1 changed
  + endpoint HTTP GET /internal/mfa/status @services/notification
  ~ file services/notification/src/server.ts: attrs {"blob":"ea51abb6b895eb6e3610cf6893dc84a4f820950b","lang":"typescript","size":662} → {"blob":"2dfd889e2b7dbf40595a2f605f3ae0d9ead914b6","lang":"typescript","size":805}
edges: 2 added, 0 removed, 1 changed, 0 evidence only
  + EXPOSES services/notification -> HTTP GET /internal/mfa/status @services/notification (treesitter)  [exact]
  + HANDLED_BY HTTP GET /internal/mfa/status @services/notification -> services/notification/src/server.ts (treesitter)  [exact]
  ~ QUERIES services/notification/src/server.ts -> public.email_log (sql_literal): attrs {"ops":["insert"],"schema_assumed":true} → {"ops":["read","insert"],"schema_assumed":true}; evidence changed (1 → 2 sites)
diagnostics: no kind changed count
```

#### `--json`, `--snapshot` and pages

Every question (`find`, `service`, `callers`, `impact`, `table-deps`, `cypher`, `diff`; `rb path` spells it `--format json`) and `status` and `snapshots` take `--json`, which prints the API's envelope: `api`, `repo`, `snapshot` (`id`, `commit_sha`), the normalised `query` with defaults filled in, the `limits` in force, `truncated` (the names of the limits that cut the answer, empty when the answer is complete) and, where rows carry one, a `certainty` histogram, then the `result`. Every question also takes `--snapshot <id>` to answer from a kept retired snapshot instead of the live one. Flat lists (`find`, `service` without a name, `snapshots`) take `--limit` and `--offset` and report `total` and `next_offset` (`null` on the last page).

```
$ target/debug/rb find mfa --kind endpoint --limit 2 --json
{
  "api": 1,
  "repo": "m5r_polyglot",
  "snapshot": {
    "id": 15450,
    "commit_sha": "6771067e0a71e93f55ab92bf1bb8ff4d11078443"
  },
  "query": {
    "kind": "endpoint",
    "limit": 2,
    "offset": 0,
    "q": "mfa"
  },
  "limits": {},
  "truncated": [],
  "result": {
    "total": 4,
    "offset": 0,
    "limit": 2,
    "next_offset": 2,
    "items": [
      {
        "kind": "endpoint",
        "key": "HTTP POST /internal/mfa/sms @services/notification",
        "name": "POST /internal/mfa/sms"
      },
      {
        "kind": "endpoint",
        "key": "HTTP GET /internal/mfa/status @services/notification",
        "name": "GET /internal/mfa/status"
      }
    ]
  }
}

$ target/debug/rb find mfa --kind endpoint --limit 2 --offset 2
endpoint	HTTP POST /internal/mfa/email @services/notification	POST /internal/mfa/email
endpoint	HTTP POST /internal/mfa/email @services/billing/cmd/billing	POST /internal/mfa/email

$ target/debug/rb find status --kind endpoint --snapshot 15449
$ target/debug/rb find status --kind endpoint
endpoint	HTTP GET /internal/mfa/status @services/notification	GET /internal/mfa/status
```

The route added by the second commit is absent from the retired snapshot and present in the live one. A refused request prints its error envelope on stdout with `--json` (text mode prints `Error: …` on stderr); the exit code and the server's status come from one table:

```
$ target/debug/rb find mfa --limit 1001 --json
{
  "api": 1,
  "error": {
    "kind": "invalid_param",
    "message": "limit must be between 1 and 1000 (got 1001)",
    "param": "limit"
  }
}
```

(exit code 2)

| Error `kind` | Exit code | HTTP status |
|---|---|---|
| `invalid_param` (with `param`) | 2 | 400 |
| `not_found`, `no_live_snapshot` | 3 | 404 |
| `snapshot_not_retained` (deleted by retention) | 3 | 410 |
| `ambiguous` (with `candidates`; never a silent pick) | 4 | 409 |
| `cypher`, `cypher_timeout` | 5 | 422 |
| `busy`, `deadline_exceeded` (server only) | — | 503 |
| `misdirected_request`, `method_not_allowed`, `unsupported_media_type` (server only) | — | 421, 405, 415 |
| anything else | 1 | 500 |

Limits (a value above a maximum is `invalid_param`, never clamped): `depth` 1 (callers, max 25) and 12 (path, max 25); `hops` 4 (max 8); `nodes` 200,000 reached nodes (Q2, Q3); `tree` 5,000 rendered tree nodes (Q1, Q4); `work` 20,000,000 counted operations per request, so a cut is a function of the snapshot and the parameters, never of machine speed; `rows` 10,000 (Cypher).

#### Cypher guard rails

`rb cypher` (and `POST …/cypher`) runs in a read-only transaction as the role `rb_reader` (migration `0006_reader_role.sql`), which can read the `rb` schema and the graph schemas and nothing else, with a statement timeout of `--timeout-ms` (default 2,000, at most 30,000, env `RB_CYPHER_TIMEOUT_MS`); a query over it is cancelled with an error, never cut short. At most 10,000 rows are returned, then `truncated: rows`. Only the live snapshot has an AGE graph.

```
$ target/debug/rb cypher "MATCH (s:Service)-[:EXPOSES]->(e:Endpoint) RETURN s.name, count(e)" --cols service,endpoints --timeout-ms 5000
service	endpoints
notification	4
auth-service	2
billing	2

$ target/debug/rb cypher "MATCH (a), (b), (c), (d), (e) RETURN count(*)" --timeout-ms 100
Error: cypher query cancelled by its statement timeout of 100 ms (limit `timeout_ms`: --timeout-ms, at most 30000)
```

(exit code 5)

#### `rb serve`

`rb serve` serves the same answers over HTTP/1.1 until SIGINT or SIGTERM (requests in flight finish). The body of every response is byte-identical to the stdout of the matching `rb … --json` command, and its status maps from that command's exit code (the table above).

- **Loopback only, no authentication:** `--bind` (default `127.0.0.1:55480`; `127.0.0.1:0` picks a free port) must be a loopback address; any other is refused at start-up. Any local process can call the server.
- **Host check:** a request whose `Host` is not `127.0.0.1:<port>`, `localhost:<port>` or `[::1]:<port>` (or is missing) gets `421`, which defeats DNS rebinding; so does an absolute-form request (`GET http://host/…`) naming another host, and two `Host` headers, or an absolute-form host that differs from the `Host` header, get `400`. Path segments are percent-decoded once (`/v1/repos/my%5Frepo/…` is `my_repo`). No `Access-Control-*` header is ever sent; `OPTIONS` and other methods get `405`; `POST …/cypher` needs `Content-Type: application/json` (`415`); an unknown or repeated parameter is `invalid_param`.
- **Read-only:** every pooled connection is a read-only session and Cypher runs as `rb_reader`. Indexing stays a CLI action: there is no `POST …/index`. Before it listens, `rb serve` drops any AGE graph a non-live snapshot still names (left by an `rb index` killed between its flip and its retention) and says so on stderr; that is its only write.
- **Limits:** `--workers` requests at once (default 4, each on its own connection), `--queue` waiting (default 16; one more gets `503 busy` at once), `--deadline-ms` per request including the queue (default 10,000; past it `503 deadline_exceeded`, never a partial body), `--cypher-timeout-ms` (default 2,000, env `RB_CYPHER_TIMEOUT_MS`; a request's `timeout_ms` may set its own up to 30,000), plus the per-request limits above. The server builds one in-memory index per repository's live snapshot and rebuilds it when the live pointer moves; timings are only in the `Server-Timing` header, never in a body.
- **Endpoints** (keys go in query parameters, since service, table and symbol keys contain `/`):

| Method and path | Parameters (every question also takes `snapshot`) |
|---|---|
| `GET /v1/health` | — |
| `GET /v1/repos` | — (every repository's status) |
| `GET /v1/repos/{repo}/status` | — |
| `GET /v1/repos/{repo}/snapshots` | `limit`, `offset` |
| `GET /v1/repos/{repo}/find` | `q`, `kind`, `limit`, `offset` |
| `GET /v1/repos/{repo}/services` | `limit`, `offset` |
| `GET /v1/repos/{repo}/service` | `name` |
| `GET /v1/repos/{repo}/callers` | `target`, `depth`, `include_references`, `min_certainty`, `precision`, `pick` |
| `GET /v1/repos/{repo}/impact` | `target`, `min_certainty`, `precision`, `pick` |
| `GET /v1/repos/{repo}/table-deps` | `table`, `precision`, `hops` |
| `GET /v1/repos/{repo}/path` | `endpoint`, `depth`, `hops`, `full`, `precision`, `pick` |
| `GET /v1/repos/{repo}/diff` | `from`, `to`, `limit`, `offset`, `diagnostics` (no `snapshot`) |
| `POST /v1/repos/{repo}/cypher` | JSON body `{"query", "columns"?, "timeout_ms"?}` |

```
$ target/debug/rb serve --bind 0.0.0.0:55480
Error: refusing to bind 0.0.0.0:55480: the server answers on a loopback address only (127.0.0.0/8 or ::1), since it has no authentication

$ target/debug/rb serve &
rb serve: listening on http://127.0.0.1:55480
...
$ curl -s -o /dev/null -w '%{http_code}\n' -H 'Host: attacker.example:55480' http://127.0.0.1:55480/v1/health
421
...
```

The server was then stopped with SIGINT. (The full callers/JSON examples are in git history; the CLI↔HTTP identity is re-proven by the api suite on every run.)

#### Retention, `rb gc` and `rb cache prune`

A publish writes the snapshot row, its rows, the AGE graph and the live flip in one transaction, so a failed or killed run leaves at most a `failed` row (written after the rollback); a run killed after that commit, before its retention, can leave the previous snapshot's AGE graph, which no read uses and the repository's next publish or the next `rb serve` start drops. Each run keeps the live snapshot plus the `--keep` newest retired ones (default 2, at most 20, env `RB_KEEP_RETIRED`) and deletes older and `failed` rows; only the live snapshot keeps an AGE graph (retention drops every other one of the repository), and graphs are created and dropped one per transaction under a database-wide lock. Every read command reads the live pointer and the snapshot in one read-only transaction. `rb gc` (never implicit; `--dry-run` lists, `--prefix` limits it to repositories whose names start with the prefix) removes graphs no live snapshot names and `building` rows left by binaries before M5; `rb cache prune --facts|--scip` removes cache rows no current `rb` can read (an older `rb` sharing the database may, so it is never implicit) and prints rows and bytes.

`rb gc --egress` also removes the SCIP fetch phase's two Docker networks and any proxy an interrupted run left. `rb cache prune` removes facts rows of other extractor versions and SCIP index rows of other indexer images.

#### `rb check`

`rb check [--repo <name>] [--rule <rule>] [--json]` checks the live snapshot against the built-in architecture rules and prints violations sorted by rule, source and target: exit 0 when clean, 1 on violations, 2 on errors. Rules: `orphan_endpoints` (endpoints with no handler path to code), `unowned_table_writes` (table writes outside owned services), `service_cycles` (dependency cycles between services). It is the CI gate form of the questions above: same graph, same certainty discipline, machine-readable verdict.

### Endpoints and request paths

`rb status` prints an `http:` line for each repository: endpoints, the services exposing them, and every client call site counted in exactly one bucket (zero buckets omitted):

```
  http: 7 routes in 3 services; 9 client calls: 2 synthetic, 2 heuristic, 1 ambiguous, 1 external, 1 dynamic, 2 self
```

`rb path <query>` takes an endpoint key, `<METHOD> <path>` (normalised as routes are), part of an endpoint name, or a handler or client symbol. Several matching endpoints are listed, never picked silently; `--pick N` chooses one (a prefix-unknown endpoint's line names its router). The output joins the upstream tree (client calls in other services and their callers) and the downstream tree (the handler's calls within its service and library services, ending in `QUERIES` tables and `CONSUMES` endpoints, which continue into the next service up to `--hops`, default 4). Each step shows file:line, service and certainty, and how a `CONSUMES` step was stitched (`via host gateway`, `host notification`, `suffix match`, `n candidates`). Helper calls that reach no table or endpoint collapse into `+ k helper calls` unless `--full`. These are statically possible paths, not a trace of one request.

```
Request path: POST /api/v1/login  [auth-service]
precision=cha
Statically possible paths from the call graph and HTTP stitching, not a trace of one request.
UPSTREAM
  client login().  apps/web/src/api/auth.ts:6  [@acme/web]  [heuristic]  (via host gateway)
    ← LoginForm().onSubmit().  apps/web/src/pages/Login.tsx:4  [@acme/web]  [heuristic]
      ← LoginForm().  apps/web/src/pages/Login.tsx:3  [@acme/web]  [heuristic]
ENDPOINT  POST /api/v1/login  [auth-service]
DOWNSTREAM
  AuthController#login().  services/auth/src/main/java/com/acme/auth/AuthController.java:17  [auth-service]  [exact]
    → MfaClient#sendCode().  services/auth/src/main/java/com/acme/auth/MfaClient.java:9  [auth-service]  [heuristic]
      → CONSUMES POST /internal/mfa/email  [notification]  [synthetic, branch heuristic]  (host notification)
        ⇒ services/notification/src/server.ts (top-level code)  [notification]  [exact, branch heuristic]
          → QUERIES public.email_log (insert)  [exact, branch heuristic]
    → UserDao#touch().  services/auth/src/main/java/com/acme/auth/UserDao.java:18  [auth-service]  [heuristic]
      → QUERIES audit.users (insert)  [exact, branch heuristic]
      → QUERIES public.users (update)  [exact, branch heuristic]
certainty: exact 5, synthetic 1, heuristic 5
```

The Service Card (`rb service <name>`) gains `APIs` (each endpoint with its handler and certainty, `(prefix unknown)` when flagged), `Calls APIs of` and `APIs called by` (grouped by service, with sites), and `HTTP client calls not stitched: k (rb status)`. `rb impact` lists the endpoints handled by the removed or impacted code under `endpoints:`.

### Dependency injection, dispatch precision, orphans and dependencies

`rb status` prints a `di:` line (every injection point in exactly one bucket: `plain`, `capped`, `ambiguous`, `too ambiguous`, `external`, `unresolved`, `optional absent`, `not a bean`, `test`) and an `rta:` line (implementing types proven never instantiated, and the dispatch edges that makes prunable):

```
  di: 9 injection points: 5 plain, 2 capped, 1 ambiguous, 1 not a bean
  rta: 1 of 12 implementing types never instantiated (2 dispatch edges prunable)
```

`rb callers`, `rb path`, `rb impact` and `rb table-deps` take `--precision rta|cha`. `cha`, the default for every question, keeps every dispatch step. `rta` is opt-in: a dispatch step into an implementation proven never instantiated is skipped and counted (`rta: k dispatch steps pruned (…; --precision cha shows them)`, JSON `pruned_by_rta`), and the header says `precision=rta (opt-in, may hide callers reached through forms the scan cannot see)`, since the proof cannot see instances created by reflection with computed names, deserialisation or code outside the repository. Every answer prints its `precision=` header. `rb callers` marks entry points (`(entry: main)`, `endpoint POST /login`, `top-level`, …) and keeps expanding past them.

`rb impact` lists each endpoint once under `endpoints:` (`(handler removed)` at distance 1), `orphans: may become dead code (n)` — symbols of the target's deployable service whose every use is in the removed region, by a reachability computation over the requiring edges with a whole-word mention scan of every file keeping anything else alive — and `tests (n)`; `--json` gives the same. `rb table-deps` rolls the table's accessors up to entry points per service (operations, `direct` or `indirect via`, endpoints, other entry kinds, best certainty) and lists the services that reach the table only through another service's API (`Through APIs`, up to `--hops`, default 4).

The Service Card adds `Packages` (version and where it came from: `[lockfile]`, `[literal]`, `[property]`, `[go_mod]`, `[unresolved]`, `(scip)`), `Database tables`, `Dependencies` and `Depended on by` (other services `via calls`, `via http` or `via package`, derived from stored edges), `Shares tables with` (coupling, kept apart from dependencies) and a `Call graph` summary with entry points by kind:

```
├─ Dependencies (1 service, 3 packages)
│    notification (services/notification)  via http  [synthetic]  POST /internal/mfa/email
│    packages: 3
├─ Depended on by (1)
│    @acme/web (apps/web)  via http  [heuristic]  POST /api/v1/login
├─ Shares tables with (1)
│    github.com/acme/billing (services/billing)  via package  [synthetic]  public.users
└─ Call graph (edges within the service)
     13 symbols · CALLS 2, IMPLEMENTS 0, INJECTS 2, REFERENCES 0
     certainty: heuristic 100.0%
     entry points: endpoint 2, main 1
```

### SCIP indexers

`rb index` runs a SCIP indexer in a container for every Maven or Gradle project (`pom.xml`, `build.gradle*`), every npm, yarn or pnpm package with a `tsconfig.json`, every Go module (`go.mod`), and every Python project (`pyproject.toml`/`setup.py`), and replaces that project's heuristic symbol keys and code edges with exact ones (design §5.2–§5.3, contracts `tasks/contract-m3b.md`, `tasks/contract-m6d.md`). Kotlin exact edges come from scip-java in Gradle builds (Kotlin 2.2.0 toolchain). It needs Docker and the images:

```sh
docker build -t rb-scip-java:0.13.1 docker/scip-java      # slim local scip-java image (see docker/scip-java/README.md)
docker build -t rb-egress-proxy:1.11.3 docker/egress-proxy # the fetch phase's egress proxy (see docker/egress-proxy/README.md)
docker pull --platform linux/amd64 sourcegraph/scip-typescript@sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845
docker build -t rb-scip-go:0.2.7 docker/scip-go            # local scip-go image on a pinned Go toolchain (see docker/scip-go/README.md)
docker pull --platform linux/amd64 sourcegraph/scip-python@sha256:1847528ce710f58d13200e8c0237f8028342eee3f8a073a9f6109107f930be23
```

Each job exports the project's files, runs a fetch phase (`mvn … go-offline` and the build, `npm ci --ignore-scripts --logs-max=0`, `go mod download`, or a lockfile-driven `poetry install --no-root --no-interaction` / `pip install --no-deps`; no credentials, no host toolchains), then indexes with `--network none`. The fetch phase runs behind an egress allow-list: its containers attach only to the `--internal` Docker network `rb-scip-fetch` (no route out, no external DNS), whose one other member is a tinyproxy container started for that run (`rb-egress-<run>`, also on the bridge `rb-scip-egress`), which forwards only `CONNECT <host>:443` to Maven Central (`repo.maven.apache.org`, `repo1.maven.org`), the npm registry and the Yarn registry, the Go module proxy and its zip bucket (`proxy.golang.org`, `storage.googleapis.com`), and PyPI (`pypi.org`, `files.pythonhosted.org`), plus the hosts in `--scip-registries`; plain HTTP, other ports, IP addresses and every other host are refused, and a failed fetch's report names what the proxy refused (`egress proxy refused: repo.example.com:443`). Maven reads the proxy from a generated `settings.xml`, npm/Yarn/pnpm from the proxy variables, Go from `GOPROXY` with `GOSUMDB=off` and `GOTOOLCHAIN=local` (a newer-toolchain repo fails closed rather than dialing out), pip/poetry from the proxy variables with `PIP_TIMEOUT=60` (pip's 15 s default trips on slow wheel reads through the emulated stack) and the version check off. Package caches are per repository (`<scip-work-dir>/cache/<repo>/{m2,npm,gomod,pip,pypoetry}`), so one repository's fetch never writes where another's reads, and both Maven phases turn the Develocity/Gradle Enterprise build caches off (a cache hit would skip `javac`, leaving sources without an index). `rb index` creates the two networks when absent and never removes them; `rb gc --egress` removes them and any proxy an interrupted run left. Without the proxy image every job is `unsupported`: no fetch runs unguarded.

| Flag | Meaning |
|---|---|
| `--no-scip` | run no indexer; every project is heuristic (a different tool configuration from a run with indexers) |
| `--scip-work-dir <dir>` (`RB_SCIP_WORK_DIR`, default `var/scip`) | exports and package caches; must lie under the home directory (the path the runtime's VM shares, see below) |
| `--scip-timeout-secs <n>` | budget per phase (defaults 600 s fetch, 900 s index); a run over it is killed and reported as `timeout`, not cached |
| `--scip-cpus <n>` (`RB_SCIP_CPUS`) | CPUs per SCIP container, 0.5–64 (default 2); not part of the cache key |
| `--scip-memory <n>m\|<n>g` (`RB_SCIP_MEMORY`) | memory per SCIP container, 512m–64g (default `4g`); not part of the cache key |
| `--scip-maven-threads <n>` (`RB_SCIP_MAVEN_THREADS`) | Maven reactor threads (`-T n`) in both scip-java phases, 1–16 (default 1, no flag); above 1 the Java recipe gains `+T<n>`, so no cached index crosses thread settings. Like every budget flag it is set on the indexing host, never read from the repository |
| `--scip-registries <host,…>` (`RB_SCIP_REGISTRIES`) | extra registry hosts the fetch phase may reach over HTTPS (lower-case DNS names; no wildcard, port or IP); configured on the indexing host, never read from the repository, whose own build runs in the fetch phase |
| `--no-cache` | parse every file without reading cached facts (the facts are still written, so the next plain run reuses them), and re-run every SCIP job and replace its cached outcome |

The egress proxy image's build was re-run as printed on 2026-10-07: its `apk add` layer came from the build cache and nothing was pulled. Its ID is used only to check that the image is present; it is in no cache key or tool configuration.

Lombok-generated members (contract `tasks/contract-m6a.md`): a scip-java document's occurrences at Lombok's own `@lombok.Generated` marker spans are removed before the unchanged agreement checks, so generated getters, setters and builders no longer reject their documents (apache/shardingsphere: 3,889 rejected documents accepted, 0 remaining; `lombok-generated <n> in <d> documents` in `rb status`); references to generated members count as `undocumented`. A Lombok version that does not mark (measured: 1.18.30) leaves its documents rejected. TypeScript interface-method `IMPLEMENTS` edges come from scip-typescript's own member relationships through the declared supertype clauses. npm/Yarn workspaces install from the root lockfile as one job (`workspace <k> members`): members and in-repository `file:` link targets (`<n> link targets`) are exported and indexed together, and a member whose build fails fails with its group.

Go and Python (contract `tasks/contract-m6d.md`): scip-go runs from the locally built image and scip-python from the digest-pinned image (emulated `linux/amd64`; a bare-files project with no manifest plans no job). SCIP and manifest package edges share one `go/<module>@<version>` key (a mismatch keeps both rows); `poetry.lock`/`uv.lock` pin exact `py/<name>@<version>` rows (anything else stays `?`). `requests`/`httpx` call sites (module verbs, sessions, `self.` members, typed locals) enter as fetch-class clients, capped; chi route groups and FastAPI routers/decorators enter as endpoints. A Python fetch that reads slowly through the emulated stack retries under `PIP_TIMEOUT=60`; a cached fetch failure replays until `--no-cache --force` overwrites it.

Kotlin and Gradle (contracts `tasks/contract-m6e.md`, `tasks/contract-m6i.md`): Gradle projects (Groovy or Kotlin DSL, version catalogs included) build in the scip-java container; Kotlin exact edges come from the same index (Kotlin 2.2.0 toolchain measured). Known limits measured on large repositories: a Maven repository whose only build root is the top-level `pom.xml` is one job (apache/shardingsphere: 2,950 s fetch+index at the default budget); a repository whose `engines` exceed the pinned scip-typescript image's toolchain fails its fetch (mattermost's `webapp`).

#### Container runtime

`rb` talks to whatever daemon the Docker CLI's **current context** selects (`docker context show`), and the images must exist in that daemon. Docker Desktop (`desktop-linux`) is the runtime of record: the e2e tests, the goldens and the public-repo measurements were made on it. If `docker context show` prints something else (another runtime such as colima holds neither image and would let `docker run` pull scip-typescript into its own VM), run `rb` with `DOCKER_CONTEXT=desktop-linux` in the environment, or switch once with `docker context use desktop-linux`. Then, in that context:

```sh
docker build -t rb-scip-java:0.13.1 docker/scip-java      # slim scip-java image (docker/scip-java/README.md); absent → `unsupported`
docker build -t rb-egress-proxy:1.11.3 docker/egress-proxy # egress proxy (docker/egress-proxy/README.md); absent → every job `unsupported`
docker pull --platform linux/amd64 sourcegraph/scip-typescript@sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845
docker build -t rb-scip-go:0.2.7 docker/scip-go            # local scip-go image (docker/scip-go/README.md); absent → Go jobs `unsupported`
docker pull --platform linux/amd64 sourcegraph/scip-python@sha256:1847528ce710f58d13200e8c0237f8028342eee3f8a073a9f6109107f930be23
```

Bind mounts come from the directories the runtime shares with its VM, the home directory by default, so `--scip-work-dir` (default `var/scip` under the project) must stay under it; a work directory elsewhere (`/tmp`) mounts as an empty directory inside the container.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55432/rb cargo test --workspace
```

Use the port the database runs on: 55432 by default, 55433 on the development machine of record (`RB_DB_PORT=55433`), where the loop is `var/m6l/scripts/verify.sh <tag>` (fmt, clippy, the workspace tests — 927 unit tests at M6l close — `rb-store` and `rb-server` with the database, the 47-test e2e and the API identity suites, a check that no `rb serve` is left listening, and the stub scan; logs under `var/m6l/logs/`). Without `RB_TEST_DATABASE_URL`, the end-to-end tests in `crates/rb-cli/tests/e2e.rs` and `api.rs` print `skipping:` and pass without running, which is not a pass. The full operator guide (worked example, fixture catalog, golden workflow, invariant/measurement practice, troubleshooting) is [docs/usage-and-testing.md](docs/usage-and-testing.md).

| Path | Contents |
|---|---|
| `crates/rb-model` | node/edge kinds, certainty, `GraphBuilder`, canonical dump |
| `crates/rb-git` | commit → manifest (gitoxide), CODEOWNERS, glob matcher, boundary detection |
| `crates/rb-syntax` | Tree-sitter extraction (definitions, references, SQL candidates) and heuristic symbol keys; TypeScript, JavaScript, Java, Python, Go, Kotlin, C# |
| `crates/rb-sql` | SQL statements → tables and operations (pg_query parse-tree walk) |
| `crates/rb-frameworks` | ORM packs (JPA, Spring Data, Prisma, TypeORM, SQLAlchemy, Django); route and HTTP-client packs, endpoint normalisation, URL constant folding; DI facts (Spring, Guice/Dagger, NestJS, Spring XML, Kotlin beans, .NET) |
| `crates/rb-scip` | SCIP: project detection, container jobs (fetch + offline index), index cache key, `index.scip` decoding, symbol parsing |
| `crates/rb-indexer` | pipeline: git → structure → syntax (cached) → manifest packages → SCIP jobs (cached) → join → endpoints and HTTP stitching → DI resolution, RTA, entry kinds and orphan eligibility → snapshot graph + diagnostics; architecture rules (`rb check`) |
| `crates/rb-store` | migrations, snapshot publish/retention and `gc`, whole-snapshot reads, evidence, AGE projection, read-only Cypher as `rb_reader`, the prebuilt snapshot index and limits, find/service queries and the card's derived sections, `callers`/`impact`/`path` walks with precision and orphans, Q3's roll-up |
| `crates/rb-api` | the request layer shared by every `--json` output and the server: parameters and limits, target resolution, the envelope, errors with exit codes and statuses, renderers |
| `crates/rb-server` | `rb serve`: the axum binding, Host check, read-only connection pool, per-snapshot index cache, workers, queue and deadline |
| `crates/rb-cli` | the `rb` binary |
| `docker/scip-java/` | the slim scip-java image |
| `docker/egress-proxy/` | the fetch phase's egress proxy image (tinyproxy on a pinned Alpine base) |
| `migrations/` | SQL schema, embedded into the binary |
| `fixtures/` | golden fixture repositories (18 fixtures: structure, routes, SQL/ORM, SCIP, DI frameworks, Kotlin/C#/Go/Python breadth) |
| `crates/rb-cli/tests/golden/` | reviewed golden outputs (`RB_BLESS=1` rewrites them; review before committing) |
