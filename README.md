# Repository Brain

A deterministic, LLM-free code-intelligence graph for Git repositories. It is built with Rust, Postgres and Apache AGE.
The design is in [docs/repository-brain-design.md](docs/repository-brain-design.md).

**Status: milestones M0 (Foundations), M1a (Structure), M1b (Syntax), M2a (SQL core), M2b (ORM mappings), M3a (heuristic resolution), M3b (SCIP), M4a (endpoints and request paths) and M4b (DI and dispatch) are done; M5 (scale and API) is next.** `rb` indexes a commit into a snapshot containing:

- one `File` node per blob, with its blob SHA, size, language and binary/generated flags;
- one `Service` per detected project (package.json, Maven, Gradle, go.mod / `package main`, Cargo, pyproject, Dockerfile, `rb.toml`), marked deployable or library;
- `Owner` nodes from CODEOWNERS (GitHub semantics), with `OWNED_BY` edges that cite the deciding CODEOWNERS line;
- `Symbol` nodes for the definitions Tree-sitter finds in TypeScript/TSX/JavaScript, Java, Python and Go files, with `DEFINED_IN` (pointing at the name) and `CONTAINS` edges. Parse results are cached per blob, so a re-index only parses changed files.
- `Table` nodes from SQL parsed with the real Postgres parser. `MIGRATES` edges come from `.sql` files, `QUERIES` edges from SQL strings in code (exact when passed to a known database call) and from ORM calls, and `MAPS_TO` edges from ORM entity mappings (JPA, Prisma, TypeORM, SQLAlchemy, Django).
- `CALLS`, `REFERENCES` and `IMPLEMENTS` edges between symbols: exact (provenance `scip`) where a SCIP indexer ran for the project in a container, heuristic otherwise (resolved from imports, packages and typed bindings, never by bare name across the repository).
- `Package` nodes and `DEPENDS_ON` edges from the external symbols a SCIP index names (provenance `scip`) and from manifests (provenance `manifest`: `package.json` with `package-lock.json` v2/v3, `go.mod`, `pom.xml` literals and same-pom properties; `@?` when the version cannot be read), plus Service → Service `DEPENDS_ON` (`via: package`) when a manifest names another service of the repository.
- `INJECTS` edges (provenance `di`, never above `heuristic`) from Spring dependency injection: constructor (Lombok-generated included), field, setter and `@Bean`-parameter points to the stereotype class, `@Bean` method, Spring Data repository or Feign client they resolve to, narrowed by `@Qualifier`, `@Primary` and the parameter name, with `resolution`, `origin`, `via` and `capped_by` attributes; several candidates give `ambiguous` edges.
- `impl_instantiated: false` on `IMPLEMENTS` edges out of a type proven never instantiated (RTA), and the `Symbol` attributes `entry` (`main`, `test`, `scheduled`, `listener`, `lifecycle`) and `orphan_eligible`.
- `Endpoint` nodes from Spring MVC, Express, gin and Go `net/http` routes, with `EXPOSES` (service → endpoint) and `HANDLED_BY` (endpoint → handler); `CONSUMES` edges (provenance `http_stitch`, never above `synthetic`) from `fetch` (with `node-fetch` and `cross-fetch`), axios, Feign, `RestTemplate`, `WebClient`/`RestClient` and Go `net/http` client calls to other services' endpoints (contract `tasks/contract-m4a.md`). An endpoint key is `HTTP <METHOD> <path> @<service>`; when a route's prefix cannot be composed it is flagged `prefix_unknown` and its key ends in ` ?router=<file>#<binding>`.
- `IN_SERVICE` edges and diagnostics (skipped files, ignored or invalid CODEOWNERS lines, unreadable manifests, syntax errors, oversized files, unresolved references, one `scip_status` per build root, route diagnostics and one bucket per HTTP client call that was not stitched).

Each snapshot is stored relationally and projected into an AGE graph.

## Quickstart

```sh
docker compose up -d --wait            # Postgres 18 + AGE 1.8.0 on 127.0.0.1:55432
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
target/debug/rb gc --dry-run           # graphs and `building` rows interrupted or older runs left (`--egress`: the fetch networks too)
target/debug/rb cache prune --facts --scip --dry-run   # cache rows of other extractor versions and images
```

Snapshots: a publish writes the snapshot row, its rows, the AGE graph and the live flip in one transaction, so a failed or killed run leaves at most a `failed` row (written after the rollback). Each run keeps the live snapshot plus the `--keep` newest retired ones (default 2, at most 20, env `RB_KEEP_RETIRED`) and deletes older and `failed` rows; only the live snapshot keeps an AGE graph, and graphs are created and dropped one per transaction under a database-wide lock. Every read command reads the live pointer and the snapshot in one read-only transaction. `rb gc` (never implicit; `--dry-run` lists, `--prefix` limits it to repositories whose names start with the prefix) removes graphs no live snapshot names and `building` rows left by binaries before M5; `rb cache prune --facts|--scip` removes cache rows no current `rb` can read (an older `rb` sharing the database may, so it is never implicit) and prints rows and bytes.

Run `rb` from the project root. AGE's CSV loaders only read files under the server's `/tmp/age/`, and `compose.yaml` mounts `./var/age-load` there. From anywhere else, set `RB_AGE_LOAD_DIR` to that directory.

| Setting | Default |
|---|---|
| `RB_DATABASE_URL` | `postgresql://rb:rb@127.0.0.1:55432/rb` |
| `RB_AGE_LOAD_DIR` | `var/age-load` |

If 55432 is taken, start the database on another port with `RB_DB_PORT=55433 docker compose up -d --wait` and point `RB_DATABASE_URL` at it.

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

`rb status` prints a `di:` line (every Spring injection point in exactly one bucket: `plain` one bean reached without a capping gate, `capped`, `ambiguous`, `too ambiguous`, `external`, `unresolved`, `optional absent`, `not a bean`, `test`) and an `rta:` line (implementing types proven never instantiated, and the dispatch edges that makes prunable):

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
│    github.com/acme/billing (services/billing)  public.users
└─ Call graph (edges within the service)
     13 symbols · CALLS 2, IMPLEMENTS 0, INJECTS 2, REFERENCES 0
     certainty: heuristic 100.0%
     entry points: endpoint 2, main 1
```

### SCIP indexers

`rb index` runs a SCIP indexer in a container for every Maven project (`pom.xml`) and every npm, yarn or pnpm package with a `tsconfig.json`, and replaces that project's heuristic symbol keys and code edges with exact ones (design §5.2–§5.3, contract `tasks/contract-m3b.md`). It needs Docker and the two images:

```sh
docker build -t rb-scip-java:0.13.1 docker/scip-java      # slim local scip-java image (see docker/scip-java/README.md)
docker build -t rb-egress-proxy:1.11.3 docker/egress-proxy # the fetch phase's egress proxy (see docker/egress-proxy/README.md)
docker pull --platform linux/amd64 sourcegraph/scip-typescript@sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845
```

Each job exports the project's files, runs a fetch phase (`mvn … go-offline` and the build, or `npm ci --ignore-scripts --logs-max=0`; no credentials, no host toolchains), then indexes with `--network none`. The fetch phase runs behind an egress allow-list: its containers attach only to the `--internal` Docker network `rb-scip-fetch` (no route out, no external DNS), whose one other member is a tinyproxy container started for that run (`rb-egress-<run>`, also on the bridge `rb-scip-egress`), which forwards only `CONNECT <host>:443` to Maven Central (`repo.maven.apache.org`, `repo1.maven.org`), the npm registry and the Yarn registry, plus the hosts in `--scip-registries`; plain HTTP, other ports, IP addresses and every other host are refused, and a failed fetch's report names what the proxy refused (`egress proxy refused: repo.example.com:443`). Maven reads the proxy from a generated `settings.xml`, npm/Yarn/pnpm from the proxy variables. Package caches are per repository (`<scip-work-dir>/cache/<repo>/{m2,npm}`), so one repository's fetch never writes where another's reads, and both Maven phases turn the Develocity/Gradle Enterprise build caches off (a cache hit would skip `javac`, leaving sources without an index). `rb index` creates the two networks when absent and never removes them; `rb gc --egress` removes them and any proxy an interrupted run left. Without the proxy image every job is `unsupported`: no fetch runs unguarded. Indexes are cached in the database by `(project input hash, image)`; the input hash covers the project's blobs, the fetch recipe (`rb_scip::RECIPE`) and the allow-list, so an unchanged project runs no container, an index from an older recipe is never served, and a fetch that failed on a host outside the list is retried once the host is added. Without Docker or an image the project stays heuristic and `rb status` says so (`scip: … skipped`/`unsupported`); the images' identities are part of the snapshot's tool configuration, and a snapshot whose jobs did not all finish (`skipped`, `unsupported`, `environment`, `timeout`) is never "up to date" while SCIP is on, so the next `rb index` tries again (`rebuilding: n SCIP jobs did not finish last time`).

| Flag | Meaning |
|---|---|
| `--no-scip` | run no indexer; every project is heuristic (a different tool configuration from a run with indexers) |
| `--scip-work-dir <dir>` (`RB_SCIP_WORK_DIR`, default `var/scip`) | exports and package caches; must lie under the home directory (the path the runtime's VM shares, see below) |
| `--scip-timeout-secs <n>` | budget per phase (defaults 600 s fetch, 900 s index); a run over it is killed and reported as `timeout`, not cached |
| `--scip-registries <host,…>` (`RB_SCIP_REGISTRIES`) | extra registry hosts the fetch phase may reach over HTTPS (lower-case DNS names; no wildcard, port or IP); configured on the indexing host, never read from the repository, whose own build runs in the fetch phase |
| `--no-cache` | parse every file without reading cached facts (the facts are still written, so the next plain run reuses them), and re-run every SCIP job and replace its cached outcome |

#### Container runtime

`rb` talks to whatever daemon the Docker CLI's **current context** selects (`docker context show`), and the two images must exist in that daemon. Docker Desktop (`desktop-linux`) is the runtime of record: the e2e tests, the goldens and the public-repo measurements were made on it. If `docker context show` prints something else (another runtime such as colima holds neither image and would let `docker run` pull scip-typescript into its own VM), run `rb` with `DOCKER_CONTEXT=desktop-linux` in the environment, or switch once with `docker context use desktop-linux`. Then, in that context:

```sh
docker build -t rb-scip-java:0.13.1 docker/scip-java      # slim scip-java image (docker/scip-java/README.md); absent → `unsupported`
docker build -t rb-egress-proxy:1.11.3 docker/egress-proxy # egress proxy (docker/egress-proxy/README.md); absent → every job `unsupported`
docker pull --platform linux/amd64 sourcegraph/scip-typescript@sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845
```

Bind mounts come from the directories the runtime shares with its VM, the home directory by default, so `--scip-work-dir` (default `var/scip` under the project) must stay under it; a work directory elsewhere (`/tmp`) mounts as an empty directory inside the container.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55432/rb cargo test --workspace
```

Without `RB_TEST_DATABASE_URL`, the end-to-end tests in `crates/rb-cli/tests/e2e.rs` print a notice and pass without running.

| Path | Contents |
|---|---|
| `crates/rb-model` | node/edge kinds, certainty, `GraphBuilder`, canonical dump |
| `crates/rb-git` | commit → manifest (gitoxide), CODEOWNERS, glob matcher, boundary detection |
| `crates/rb-syntax` | Tree-sitter extraction (definitions, references, SQL candidates) and heuristic symbol keys |
| `crates/rb-sql` | SQL statements → tables and operations (pg_query parse-tree walk) |
| `crates/rb-frameworks` | ORM packs (JPA, Spring Data, Prisma, TypeORM, SQLAlchemy, Django); route and HTTP-client packs, endpoint normalisation, URL constant folding; Spring DI facts (beans, injection points, scan roots) |
| `crates/rb-scip` | SCIP: project detection, container jobs (fetch + offline index), index cache key, `index.scip` decoding, symbol parsing |
| `crates/rb-indexer` | pipeline: git → structure → syntax (cached) → manifest packages → SCIP jobs (cached) → join → endpoints and HTTP stitching → DI resolution, RTA, entry kinds and orphan eligibility → snapshot graph + diagnostics |
| `crates/rb-store` | migrations, snapshot publish/retention, evidence, AGE projection, read-only Cypher, find/service queries and the card's derived sections, `callers`/`impact`/`path` walks with precision and orphans, Q3's roll-up |
| `crates/rb-cli` | the `rb` binary |
| `docker/scip-java/` | the slim scip-java image |
| `migrations/` | SQL schema, embedded into the binary |
| `fixtures/` | golden fixture repositories |
| `crates/rb-cli/tests/golden/` | reviewed golden outputs (`RB_BLESS=1` rewrites them; review before committing) |
