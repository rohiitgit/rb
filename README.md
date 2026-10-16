# Repository Brain

**Ask questions about a codebase, get answers with evidence.** Repository Brain
(`rb`) reads a Git repository and builds a queryable map of it: every file,
service, function, class, database table, HTTP endpoint and dependency becomes
a node in a graph, with edges for calls, references, implementations,
dependency injection, request paths and table access. Then you interrogate it:

```sh
rb callers "Db#load()."            # who calls this function?
rb impact "Store#" --pick 1        # what breaks if I delete this class?
rb path "POST /api/v1/login"       # how does this request flow, end to end?
rb table-deps users                # what code touches the users table?
rb check                           # do the architecture rules pass? (CI gate)
```

Every answer cites **file:line evidence** and a **certainty** (`exact` only
when proven: by a compiler-grade index (SCIP) for calls, references and
implementations; by structure read directly (definitions and containment,
a file's service, CODEOWNERS); or by what the code states literally (SQL at a
known sink or in a `.sql` file, an ORM mapping by stated name, a route with a
literal path, a manifest version); otherwise `heuristic`/`ambiguous` —
the tool never guesses silently). The accuracy benchmark (`bench/`, usage
guide §5.5) measures how often each label is right against independent answer
keys: PyCG, the microSecEnD diagrams, blind hand keys and a judged sample of
`exact` edges (`bench/scoreboard/`). No LLM, no embeddings: deterministic Rust +
Postgres + Apache AGE, byte-identical results on every run.

- New here? Start at [5-minute quickstart](#5-minute-quickstart), then the
  [guided tour](#guided-tour-what-you-can-ask).
- Operating or contributing? See [docs/usage-and-testing.md](docs/usage-and-testing.md)
  (full testing guide) and [Development](#development) below.
- Why it works this way? See [docs/repository-brain-design.md](docs/repository-brain-design.md).

## 5-minute quickstart

Prerequisites: Rust 1.98.1 (via rustup), Docker Desktop, a few GB of disk.

```sh
# 1. Start the database (Postgres 18 + AGE) and build
RB_DB_PORT=55433 docker compose up -d --wait
cargo build
target/debug/rb migrate

# 2. Give it a repo. Here we borrow the tiny Spring-XML demo from fixtures/
export RB_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb
rm -rf /tmp/demo-spring && mkdir -p /tmp/demo-spring && cp -r fixtures/spring-xml/* /tmp/demo-spring/
(cd /tmp/demo-spring && git init -q -b main && git add -A && git commit -qm fixture)
target/debug/rb index /tmp/demo-spring --repo demo --no-scip --no-cache
# demo: snapshot 31145 live @ a96890ed53e0 — 17 nodes, 27 edges, 4 diagnostics; …; parsed 4 source files
```

(`--no-scip` skips the compiler-grade indexers so no Docker images are
needed; without it, `rb` runs SCIP indexers in containers for exact edges.
`--no-cache` forces a fresh parse so you see real counts on first run.)

```sh
# 3. Ask questions
target/debug/rb status --repo demo          # sql/code/http/di/rta lines, plus what was not analysed
target/debug/rb service shop --repo demo    # the service card: APIs, packages, tables, call graph

target/debug/rb callers "Db#load()." --repo demo
# callers of Db#load().  src/Db.java:4  [shop]
# precision=cha
#   Store#stock().  src/Store.java:15  [shop]  heuristic

target/debug/rb impact "Store#" --repo demo --pick 1
# removing Store# … breaks: … 1  Audit#  src/Audit.java:3  [shop]  heuristic …

target/debug/rb check --repo demo           # exit 0 when clean
# check: 0 violations
```

A name matching several symbols asks you to `--pick` one instead of guessing.
Targets are forgiving: `Db#load()` or `Db#load` resolve when exactly one
symbol matches (the same rule as `rb find`), and a miss suggests a
copy-pasteable `rb find` command. `--repo` defaults to the repository indexed
from the git checkout you run from (whatever name you gave it with
`rb index --repo`), and `rb index .` there re-indexes that repository; an
explicit `--repo` always wins. When a checkout matches nothing, or several
repositories, you get a directive instead of a guess, such as
`--repo is needed: no indexed repository matches this checkout ("x"); index it (rb index .) or pass --repo`.
The one exception is `rb status`, which there lists every indexed repository.
Clean up with `psql "$RB_DATABASE_URL" -c "DELETE FROM rb.repo WHERE
name='demo'"` and `rb gc`. That's the whole loop: **index → ask → diff/serve**.

> **Want bare `rb` instead of `./target/debug/rb`?** From the repository root,
> add an alias with the DB and the age-load dir baked in (both must match the
> running stack; the double quotes expand `$PWD` once, when you define it):
> `alias rb="RB_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb RB_AGE_LOAD_DIR=$PWD/var/age-load $PWD/target/debug/rb"`
> Put it in `~/.zshrc`, open a new shell, and `rb status` works from anywhere.

## Guided tour: what you can ask

**Find things.** `rb find login --kind symbol` searches nodes by key or name
(exact matches first); `rb find --kind endpoint` lists endpoints (20 a page;
`--limit`/`--offset` page through them). `rb service` lists services; `rb service <name>` shows
the card: endpoints it exposes, packages, database tables, dependencies
(`via http`, `via calls`, `via package`), and a call-graph summary.

**Trace code.** `rb callers <target> [--depth N]` walks up the call graph as a
tree (entry points marked: `endpoint …`, `main`, `top-level`). `rb impact
<target>` walks down: what breaks, what becomes dead code (`orphans:`), which
tests touch it, which endpoints it handles. On a hub, `--limit N` shows N rows:
`rb callers` first summarises everything found (per service, per entry kind),
and `rb impact`'s headers keep each list's full length. `rb callers
--exclude-tests` leaves out (and counts) callers in test code. The target's own
entry marks follow its name; a handler's callers are HTTP clients, which
`rb path` follows.

**Follow requests.** `rb path "POST /api/v1/login"` joins the upstream
(client calls in other services and *their* callers) and the downstream (the
handler's calls, ending in `QUERIES` tables and `CONSUMES` endpoints, hopping
services up to `--hops`). Steps show file:line, service, certainty, and how
each cross-service hop was stitched. These are statically possible paths, not
a trace of one request.

**Follow data.** `rb table-deps users` rolls every accessor of a table up to
entry points per service, and lists services that reach the table only through
another service's API. A Spring Data repository type has its own `QUERIES` row
with all four ops (`read+insert+update+delete`, `synthetic`). That row stands
for the CRUD methods it inherits from the framework, not for a query it states.
Its declared methods (`findByEmail`, `@Query`) get rows of their own.

**Compare and gate.** `rb diff` diffs two snapshots (endpoints, table access,
dependencies, nodes, edges — never re-indexes). `rb check` runs the
architecture rules (`orphan_endpoints`, `unowned_table_writes`,
`service_cycles`) for CI: exit 0 clean, 1 violations, 2 on any error (an
unreachable database included, so an outage never reads as violations). An
endpoint counts as called only through an unambiguous client link, and
`rb check` notes on stderr how many client calls it could not link. To adopt
the gate on an existing codebase, save today's output and gate only on new
violations:

```sh
rb check --repo demo > .rb-baseline           # accept what exists today
rb check --repo demo --baseline .rb-baseline  # exit 1 only on new violations
```

`rb cypher "MATCH …"` runs read-only Cypher (2 s statement timeout, 10k rows
max; a write is refused as `cypher is read-only`); nodes, edges and paths come
back as their agtype text. `rb serve`
exposes every question above over loopback HTTP with byte-identical `--json`
bodies (`rb path` takes `--json` too).

**Trust, but verify.** Answers carry certainty and evidence; ambiguous names
error with candidates instead of picking; limits are maxima, never silent
clamps (`invalid_param` otherwise); dumps (`rb dump`) are canonical and
byte-identical across machines, so any two runs can be diffed.

Errors have one shape everywhere: one `Error:` line in text mode, the
`{"api": 1, "error": {"kind", "message", …}}` envelope under `--json` and over
HTTP, and an exit code (HTTP status) per kind:

| Exit | Kind | HTTP |
|---|---|---|
| 0 | success (`rb check`: no violations) | 200 |
| 1 | `rb check` violations; any other unexpected failure (`internal`) | 500 |
| 2 | `invalid_param` (a bad flag or argument); every `rb check` error | 400 |
| 3 | `not_found`, `no_live_snapshot`, `snapshot_not_retained` | 404 / 404 / 410 |
| 4 | `ambiguous` (candidates listed; pass `--pick`) | 409 |
| 5 | `cypher`, `cypher_timeout` | 422 |
| 6 | `unavailable`: the database cannot be reached | 503 |

## What rb leaves out, and says so

`rb` would rather tell you it did not look than answer zero.

- **Vendored and built code is excluded from analysis** (it keeps its file node
  for ownership and `rb find`, but is never parsed or treated as a project):
  Go `vendor/` next to `vendor/modules.txt`, any `node_modules/`, files that
  `.gitattributes` marks `linguist-vendored` or `linguist-generated`, and
  TypeScript compiler output under a tsconfig `outDir` (never `.ts` sources).
  Add your own globs in `rb.toml`: `ignore = ["generated/**", "*.pb.go"]`.
- **Code in a language without a grammar** (PHP, Ruby, Rust, C/C++, Scala,
  Swift, Dart, Elixir) is counted, not silently skipped.
- **Frameworks without a pack** (echo, fiber, flask, Django views, tornado, koa,
  fastify, hapi, routing-controllers, NestJS controllers, Micronaut, Quarkus)
  are named per service, so "0 routes" never reads as "has no routes".

`rb status` prints each of these when it applies:

```
  excluded from analysis: go_vendor 1020 files
  not analysed (no grammar): php 84 files
  frameworks without a pack (routes and clients not read): echo in api
```

## How it works (60 seconds)

```
git commit → structure → Tree-sitter syntax (cached per blob) → manifests →
SCIP indexers in containers (cached; --no-scip skips) → join → endpoints +
HTTP stitching → DI resolution → RTA/entry/orphans → snapshot → AGE graph
```

Each `rb index` publishes one immutable snapshot (live + up to `--keep`
retired, default 2); only the live snapshot keeps an AGE graph. Two runs on one
repository at once take turns (the second says `waiting:`); the second then
usually finds the first's snapshot up to date (with `--force` or another
`--rev` it indexes again). Reads hit the
live pointer in one read-only transaction. Nothing is an LLM call anywhere in
this pipeline — that is why the output is deterministic.

## Language support

What `rb` reads, per language. Everything parsed gets definitions, calls,
references and implementations from Tree-sitter (`heuristic`); the SCIP column
adds compiler-grade `exact` edges where a build can run in a container.

| Language | Parsed | `exact` via SCIP | HTTP routes | HTTP / RPC clients | Dependency injection | Data access |
|---|---|---|---|---|---|---|
| **Java** | yes | scip-java (Maven, Gradle) | Spring MVC (class × method prefixes, context path; OpenAPI interfaces handled by implementors), Spring Data REST (`@RepositoryRestResource` with a literal `path`, `synthetic`) | RestTemplate, WebClient, RestClient, Feign, gRPC stubs, OpenAPI-generated clients | Spring (stereotypes, `@Bean`, qualifiers, Lombok), Guice, Dagger, Spring XML | JPA + Spring Data (mappings, derived and `@Query` methods, JPQL), MyBatis XML mappers, SQL in strings |
| **TypeScript / JavaScript** | yes (TS, TSX, JS) | scip-typescript (npm, pnpm, Yarn) | Express (routers, mounts, constant paths, `require('express')()`; paths matched case-insensitively, as Express does, unless the router mentions `caseSensitive` or the app sets `case sensitive routing`: any value counts, since the value is not read) | fetch, axios (incl. shared default-exported instances), HTTP client wrappers, gRPC stubs, OpenAPI-generated clients | NestJS (providers, constructor points, tokens) | Prisma (models and queries), TypeORM (models only), SQL in strings |
| **Python** | yes | scip-python | FastAPI (decorators, `APIRouter` prefixes, `include_router` across modules) | requests, httpx | none | SQLAlchemy and Django models (mappings, not queries), SQL in strings |
| **Go** | yes | scip-go | gin (incl. groups passed as parameters, method-value handlers declared in other files), chi, net/http `ServeMux` | net/http, gRPC stubs (`pb.NewXClient`) | none | SQL in strings, incl. `fmt.Sprintf` with package-constant table names |
| **Kotlin** | yes | scip-java (Gradle) | none | none | `beans { }` DSL (function and top-level `val` forms) | SQL in strings |
| **C#** | yes (`heuristic` only) | none | none | none | .NET DI (`Add*`/`TryAdd*`/keyed, minimal hosting, implicit and global usings, implementation owners, constructor and `[Inject]` points) | SQL in strings |

Across languages:
- **Endpoints.** `.proto` gRPC services (attributed to the services that
  register a server) and OpenAPI/Swagger specs. An endpoint is `exact` only
  when code registers it; one that only a spec states is `synthetic`. Routes
  registered in test files are not endpoints; one `routes_in_test_code`
  diagnostic counts them per file.
- **Data.** SQL migration files, aiosql/HugSQL query files and Prisma schemas.
- **Config and wiring.** Spring `application.*` config and profiles, a
  `native` Spring Cloud Config server's shared files (context paths only,
  matched to clients by `spring.application.name`), Kafka and RabbitMQ topics
  (Java), nginx and Spring Cloud Gateway routes, Compose and Kubernetes
  service names, and CODEOWNERS for file ownership.
- **Calls between services** are stitched from clients to endpoints and are
  never `exact`.

**Known gaps:**

| Gap | What `rb` does |
|---|---|
| PHP, Ruby, Rust, C/C++, Scala, Swift, Dart, Elixir | recognised as code, not parsed; `rb status` counts them as `not analysed (no grammar)` |
| echo, fiber (Go); flask, Django views, tornado (Python); koa, fastify, hapi, routing-controllers, NestJS controllers (JS/TS); Micronaut, Quarkus (JVM) | `rb status` names them per service as `frameworks without a pack` |
| Kotlin and C# routes and clients; ASP.NET controllers; EF Core, gorm, TypeORM repository calls, SQLAlchemy queries; Spring `@HttpExchange` clients; .NET open-generic registrations | not read, and not yet reported |
| SQL dialects other than Postgres | statements become `unparsed_sql` diagnostics |

## Setup reference

```sh
RB_DB_PORT=55433 docker compose up -d --wait   # default port is 55432
cargo build && target/debug/rb migrate
export RB_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb   # 55432 default
export DOCKER_CONTEXT=desktop-linux            # if docker points elsewhere
target/debug/rb doctor                         # checks all of the above, with fixes
```

`rb doctor` prints one row per check (database, Postgres and AGE versions,
migrations, the age-load directory and whether the database sees it, Docker,
the SCIP images, the SCIP work dir and free disk), each `ok`, `warn` or `fail`
with its fix (`skip` when it needs a check that failed), and exits 1 when one
fails. A database that never answers fails after 10 s.

| Variable | Default | Meaning |
|---|---|---|
| `RB_DATABASE_URL` | `postgresql://rb:rb@127.0.0.1:55432/rb` | Postgres connection |
| `RB_AGE_LOAD_DIR` | `var/age-load` | must be the dir mounted at `/tmp/age/rb-load` in DB container |
| `RB_SCIP_WORK_DIR` | `var/scip` | indexer exports + caches; must stay under home (Docker Desktop shares) |
| `RB_KEEP_RETIRED` | `2` | retired snapshots kept (0–20) |
| `RB_CYPHER_TIMEOUT_MS` | `2000` | Cypher statement timeout (1–30,000) for `cypher`, `check` and `serve` when their flag is absent; a bad value is named as the variable |
| `RB_SCIP_CPUS` / `RB_SCIP_MEMORY` | `2` / `4g` | per-SCIP-container budget (host-side, not cached) |

SCIP images (only needed without `--no-scip`):

```sh
docker build -t rb-scip-java:0.13.1 docker/scip-java
docker build -t rb-egress-proxy:1.11.3 docker/egress-proxy
docker pull --platform linux/amd64 sourcegraph/scip-typescript@sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845
docker build -t rb-scip-go:0.2.7 docker/scip-go
docker pull --platform linux/amd64 sourcegraph/scip-python@sha256:1847528ce710f58d13200e8c0237f8028342eee3f8a073a9f6109107f930be23
```

SCIP fetch runs behind an HTTPS allow-list proxy (Maven Central, npm/Yarn,
Go proxy, PyPI + `--scip-registries`); anything else is refused and named in
the report. `rb gc --egress` removes leftover networks/proxies;
`rb cache prune --facts --scip` drops unreadable cache rows.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55432/rb cargo test --workspace
```

Without `RB_TEST_DATABASE_URL`, DB tests print `skipping:` and pass. That is
not a pass: use 55433 on the machine of record. The SCIP end-to-end tests also
need a responsive Docker daemon with the images above, and must not share the
machine with other builds (contention starves their fetches). The full run:

```sh
RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb DOCKER_CONTEXT=desktop-linux \
  cargo test --workspace --no-fail-fast -- --test-threads=1
```

The testing guide (fixtures, goldens with `RB_BLESS=1`, which you **review
before committing**, precision guards, invariant and measurement practice,
troubleshooting) is [docs/usage-and-testing.md](docs/usage-and-testing.md).

| Path | Contents |
|---|---|
| `crates/rb-model`, `rb-git`, `rb-syntax`, `rb-sql` | graph types + canonical dump; git/CODEOWNERS; Tree-sitter extraction + symbol keys; SQL → tables/ops |
| `crates/rb-frameworks`, `rb-scip` | ORM/route/client/DI packs; SCIP container jobs + index decoding |
| `crates/rb-indexer` | the pipeline (structure → syntax → manifests → SCIP → join → endpoints → DI/RTA/orphans → snapshot) + `rb check` rules |
| `crates/rb-store`, `rb-api`, `rb-server` | publish/retention/gc, AGE projection, queries + walks; shared request layer; loopback HTTP server |
| `crates/rb-cli` | the `rb` binary; `tests/e2e.rs`, `tests/api.rs`, `tests/golden/` |
| `docker/`, `migrations/` | SCIP + egress images; embedded SQL schema |
| `fixtures/` | 18 golden fixture repos (see the testing guide for the catalog) |
| `tasks/`, `docs/`, `var/` | contracts, evaluations, lessons; design doc; per-milestone scripts/logs/evidence (gitignored scratch) |

**Status: M0–M9b (M9a closed 2026-10-16; M9b implemented, evaluator pass 1 answered, pass 2 pending).** M0–M6l built the tool (foundations → structure → syntax →
SQL/ORM → resolution → SCIP → endpoints → DI → scale/API → SCIP/topology/rules
breadth → Kotlin/C# → DI breadth → DI recall → XML siting). M7a–M7j fixed
what a stress campaign over 18 fixtures and 13 public repositories found
(`tasks/stress-findings.md`): wrong `exact` answers, hangs and blow-ups,
silent recall losses, vendored and built code, the error contract,
honesty about what was not analysed, the CLI's rough edges (checkouts
known by path, `rb doctor`), and the last precision and cost items (Express
case, spec-only routes, bounded hub answers, one index per repository,
streamed caches). M8a–M8b added an accuracy benchmark with answer keys
(`bench/`, `tasks/bench-findings.md`). M9a fixes the wrong `exact` edges
it found (test-file routes, spec-only routes, literal-brace Spring paths,
Go method `IMPLEMENTS`, config-server context paths). M9b adds recall it
found missing: Go methods declared outside their type's file, `fmt.Sprintf`
SQL with package constants, Spring Data REST endpoints; cross-module SCIP
calls moved to M9c. Detail per milestone: `CHANGELOG.md`,
`IMPLEMENT.md`, contracts in `tasks/`.
