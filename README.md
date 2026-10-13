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
when proven by a compiler-grade index, otherwise `heuristic`/`ambiguous` —
the tool never guesses silently). No LLM, no embeddings: deterministic Rust +
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
target/debug/rb status --repo demo          # services, endpoints, di: lines
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
`--repo` defaults to the git repository of your working directory (when that
repo is indexed); an explicit `--repo` always wins, and outside any indexed
repo you must pass one.
Clean up with `psql "$RB_DATABASE_URL" -c "DELETE FROM rb.repo WHERE
name='demo'"` and `rb gc`. That's the whole loop: **index → ask → diff/serve**.

> **Want bare `rb` instead of `./target/debug/rb`?** Add this alias (DB and
> age-load dir baked in — both must match the running stack):
> `alias rb='RB_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb RB_AGE_LOAD_DIR=/Users/rohit/conductor/workspaces/newide/spokane/var/age-load /Users/rohit/conductor/workspaces/newide/spokane/target/debug/rb'`
> Then open a new shell (or `source ~/.zshrc`) and `rb status` works from anywhere.

## Guided tour: what you can ask

**Find things.** `rb find login --kind symbol` searches nodes by key or name
(exact matches first). `rb service` lists services; `rb service <name>` shows
the card: endpoints it exposes, packages, database tables, dependencies
(`via http`, `via calls`, `via package`), and a call-graph summary.

**Trace code.** `rb callers <target> [--depth N]` walks up the call graph as a
tree (entry points marked: `endpoint …`, `main`, `top-level`). `rb impact
<target>` walks down: what breaks, what becomes dead code (`orphans:`), which
tests touch it, which endpoints it handles.

**Follow requests.** `rb path "POST /api/v1/login"` joins the upstream
(client calls in other services and *their* callers) and the downstream (the
handler's calls, ending in `QUERIES` tables and `CONSUMES` endpoints, hopping
services up to `--hops`). Steps show file:line, service, certainty, and how
each cross-service hop was stitched. These are statically possible paths, not
a trace of one request.

**Follow data.** `rb table-deps users` rolls every accessor of a table up to
entry points per service, and lists services that reach the table only through
another service's API.

**Compare and gate.** `rb diff` diffs two snapshots (endpoints, table access,
dependencies, nodes, edges — never re-indexes). `rb check` runs the
architecture rules (`orphan_endpoints`, `unowned_table_writes`,
`service_cycles`) for CI: exit 0 clean, 1 violations, 2 errors.
`rb cypher "MATCH …"` runs read-only Cypher (2 s statement timeout, 10k rows
max). `rb serve` exposes every question above over loopback HTTP with
byte-identical `--json` bodies.

**Trust, but verify.** Answers carry certainty and evidence; ambiguous names
error with candidates instead of picking; limits are maxima, never silent
clamps (`invalid_param` otherwise); dumps (`rb dump`) are canonical and
byte-identical across machines, so any two runs can be diffed.

## How it works (60 seconds)

```
git commit → structure → Tree-sitter syntax (cached per blob) → manifests →
SCIP indexers in containers (cached; --no-scip skips) → join → endpoints +
HTTP stitching → DI resolution → RTA/entry/orphans → snapshot → AGE graph
```

Each `rb index` publishes one immutable snapshot (live + up to `--keep`
retired, default 2); only the live snapshot keeps an AGE graph. Reads hit the
live pointer in one read-only transaction. Nothing is an LLM call anywhere in
this pipeline — that is why the output is deterministic.

## Languages and frameworks

| Area | Coverage |
|---|---|
| TypeScript/JS | calls, refs, routes, clients; `exact` via scip-typescript; NestJS DI |
| Java | calls, refs; `exact` via scip-java (Maven + Gradle); Spring, Guice, Dagger, Spring XML |
| Kotlin | calls, refs; `exact` via scip-java (Gradle); `beans { }` DSL incl. top-level `val` form |
| C# (Tier B: heuristic, no SCIP) | namespaces (block/file-scoped/dotted/enclosing/global), .NET DI (registrations, ctor + property points, keyed services, minimal hosting) |
| Python / Go | calls, refs, endpoints, clients; `exact` via scip-python / scip-go |
| Data | SQL via a real Postgres parser; JPA, Spring Data, Prisma, TypeORM, SQLAlchemy, Django; migrations |

## Setup reference

```sh
RB_DB_PORT=55433 docker compose up -d --wait   # default port is 55432
cargo build && target/debug/rb migrate
export RB_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb   # 55432 default
export DOCKER_CONTEXT=desktop-linux            # if docker points elsewhere
```

| Variable | Default | Meaning |
|---|---|---|
| `RB_DATABASE_URL` | `postgresql://rb:rb@127.0.0.1:55432/rb` | Postgres connection |
| `RB_AGE_LOAD_DIR` | `var/age-load` | must be the dir mounted at `/tmp/age/rb-load` in DB container |
| `RB_SCIP_WORK_DIR` | `var/scip` | indexer exports + caches; must stay under home (Docker Desktop shares) |
| `RB_KEEP_RETIRED` | `2` | retired snapshots kept (0–20) |
| `RB_CYPHER_TIMEOUT_MS` | `2000` | Cypher statement timeout (1–30,000) |
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

Without `RB_TEST_DATABASE_URL`, DB tests print `skipping:` and pass — that is
not a pass; use 55433 on the machine of record. The full loop lives at
`var/m6l/scripts/verify.sh <tag>` (fmt → clippy → units → store → server →
e2e → api → listener + stub checks; logs under `var/m6l/logs/`). The testing
guide — fixtures, goldens (`RB_BLESS=1` rewrites them; **review the diff
before committing**), precision guards, invariant/measurement practice,
troubleshooting — is [docs/usage-and-testing.md](docs/usage-and-testing.md).

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
```

**Status: M0–M6l closed** (foundations → structure → syntax → SQL/ORM →
resolution → SCIP → endpoints → DI → scale/API → SCIP/topology/rules breadth →
Kotlin/C# → DI breadth → DI recall → XML siting). Detail per milestone:
`CHANGELOG.md`, `IMPLEMENT.md`, contracts in `tasks/`.
