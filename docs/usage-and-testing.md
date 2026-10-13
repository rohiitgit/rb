# Using and testing Repository Brain

This is the operator's guide: how to run the `rb` tool end to end, and how to
run every test suite. It reflects the tree as of M6l (M0–M6l closed). Design
rationale lives in `docs/repository-brain-design.md`; the per-milestone
contracts, evaluations and measurements in `tasks/`; the older quickstart in
`README.md` (its status line predates M6 — this document supersedes it for
usage).

## 1. What the system does

`rb` indexes a Git commit into a **snapshot**: a relational store plus an
Apache AGE graph of files, services, symbols, tables, endpoints, packages and
the edges between them (`DEFINED_IN`, `CONTAINS`, `CALLS`, `REFERENCES`,
`IMPLEMENTS`, `INJECTS`, `EXPOSES`, `HANDLED_BY`, `CONSUMES`, `QUERIES`,
`MAPS_TO`, `MIGRATES`, `DEPENDS_ON`, `IN_SERVICE`, …), with per-edge certainty
(`exact` only from SCIP indexers, otherwise `heuristic`/`ambiguous`/…),
evidence spans, and a diagnostics table (unresolved references, syntax errors,
unreadable manifests, …).

Language coverage (all Tier B unless noted):

| Language | Calls/refs | DI |
|---|---|---|
| TypeScript/TSX/JavaScript | heuristic; `exact` via scip-typescript | NestJS (`@Injectable`, `@Controller`, constructor points, token qualifiers) |
| Java | heuristic; `exact` via scip-java (Maven + Gradle) | Spring (stereotypes, `@Bean`, `@Value`-exclusion, qualifiers, Lombok points, Guice/Dagger in `java-guice`/`java-dagger` fixtures), Spring XML (`<bean>`, `<constructor-arg>`, aliases, component scan) |
| Kotlin | heuristic; `exact` via scip-java in Gradle builds | `beans { }` DSL incl. top-level `val` form, `ref()` points, `@Assisted` exclusion |
| C# (Tier B only, no SCIP) | heuristic (namespaces incl. file-scoped/dotted, enclosing-namespace scope, global namespace) | `Add/TryAdd/Keyed` registrations incl. top-level minimal hosting, constructor + `[Inject]`/`[FromServices]`/`[FromKeyedServices]` property points, key matching |
| Python / Go | heuristic + endpoints/clients; `exact` via scip-python / scip-go | — |
| SQL / ORM | pg_query parse-tree walk; JPA, Spring Data, Prisma, TypeORM, SQLAlchemy, Django | — |

Questions answered: `find`, `service` (card), `callers` (Q1), `impact` (Q2),
`table-deps` (Q3), `path` (Q4), `diff`, `cypher`, JSON API + `serve`, and
architecture gates via `check` (rules: `orphan_endpoints`,
`unowned_table_writes`, `service_cycles`; exit 0 clean, 1 violations,
2 errors).

## 2. Prerequisites

- Rust 1.98.1 via rustup (`rust-toolchain.toml` pins it; `clippy` + `rustfmt`
  components required).
- Docker, with context `desktop-linux` (Docker Desktop is the runtime of
  record: `DOCKER_CONTEXT=desktop-linux` or `docker context use
  desktop-linux`). SCIP jobs need the four images (see README "SCIP
  indexers"); `--no-scip` runs need no images.
- Disk: SCIP fetches are heavy (tens of GB for large repos); keep several GB
  free (`df -h /`).
- Ports: Postgres 55432 by default (`RB_DB_PORT=55433` moves it; the machine
  of record uses **55433**, so every example here uses
  `RB_TEST_DATABASE_URL`/`RB_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb`).

## 3. Setup

```sh
RB_DB_PORT=55433 docker compose up -d --wait   # Postgres 18 + AGE 1.8.0
cargo build                                     # target/debug/rb
target/debug/rb migrate                         # apply schema migrations
```

`compose.yaml` mounts `./var/age-load` at the server's `/tmp/age/rb-load`
(AGE CSV loaders only read there). Run `rb` from the project root, or set
`RB_AGE_LOAD_DIR` accordingly from elsewhere.

## 4. Using `rb`: worked example

The Spring-XML fixture needs no containers (`--no-scip`) and exercises
symbols, DI, goldens and guards. Fixtures are content directories, not git
repos (tests materialise them), so copy one into a scratch repo first.
Substitute your own repo path for real use (`rb index` requires a git
repository; `--rev` picks a commit).

```sh
export RB_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb
rm -rf /tmp/demo-spring && mkdir -p /tmp/demo-spring && cp -r fixtures/spring-xml/* /tmp/demo-spring/
(cd /tmp/demo-spring && git init -q -b main && git add -A && git commit -qm fixture)

# Index (name defaults to the directory name; pick an explicit one for tests)
target/debug/rb index /tmp/demo-spring --repo demo_spring --no-scip --no-cache
# demo_spring: snapshot 31145 live @ a96890ed53e0 — 17 nodes, 27 edges, 4 diagnostics; …; parsed 4 source files

target/debug/rb status --repo demo_spring   # services, endpoints, di:/rta: lines
target/debug/rb service --repo demo_spring  # service list; `rb service shop` for the card

# Questions (a name matching several symbols needs --pick N, never a silent pick)
target/debug/rb find Store --kind symbol --repo demo_spring
target/debug/rb callers "Db#load()." --repo demo_spring
# callers of Db#load().  src/Db.java:4  [shop]
# precision=cha
#   Store#stock().  src/Store.java:15  [shop]  heuristic
target/debug/rb impact "Store#" --repo demo_spring --pick 1
# removing Store# … breaks: … 1  Audit#  src/Audit.java:3  [shop]  heuristic …
target/debug/rb dump --repo demo_spring > /tmp/demo.jsonl   # canonical dump

# Architecture gates
target/debug/rb check --repo demo_spring        # exit 0 when clean
target/debug/rb cypher "MATCH (s:Service) RETURN s.name" --repo demo_spring

# Housekeeping when done experimenting
psql "$RB_DATABASE_URL" -c "DELETE FROM rb.repo WHERE name LIKE 'demo_spring%'"
target/debug/rb gc
```

Notes that bite newcomers:

- `--no-scip` vs SCIP runs are **different tool configurations**; never compare
  their dumps. `--force` rebuilds the same commit; `--no-cache` re-parses and
  re-runs indexers; `--keep N` retains N retired snapshots (default 2).
- `rb diff` compares kept snapshots by natural identity and never re-indexes.
- `rb serve` is loopback-only, read-only, with per-request limits; CLI
  `--json` output is byte-identical to the matching HTTP response body.
- Every question takes `--json` (stable envelope: `api`, `repo`, `snapshot`,
  `query`, `limits`, `truncated`, `result`) and `--snapshot <id>` for retired
  snapshots. Limits are maxima, never clamps — over-limit is `invalid_param`.
- Ambiguous names are never picked silently (`ambiguous` error + candidates).
`--repo` defaults to the git checkout you run from (sanitised like `index`
names it, honoured only when indexed); explicit `--repo` wins. Outside any
indexed checkout the CLI refuses to guess and says so: *no indexed repository
matches this checkout* (with an `rb index .` hint) or *no git repository
found* (run from inside one).

## 5. Testing

### 5.1 Unit tests (no database)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Per-crate: `cargo test -p rb-syntax --lib`, `-p rb-frameworks`,
`-p rb-indexer`, etc. (∼119 + ∼196 + ∼330 lib tests at M6l close).

### 5.2 Database-backed suites

Without `RB_TEST_DATABASE_URL`, DB tests print `skipping:` and pass — **that
is not a pass**. Always set it:

```sh
export RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb
cargo test -p rb-store --lib        # migrations, publish/retention/gc (72 tests)
cargo test -p rb-server -- --nocapture  # serve binding, limits, parity (12 tests)
cargo test -p rb-cli --test e2e     # fixtures: determinism, guards, goldens, AGE parity (47 tests)
cargo test -p rb-cli --test api     # CLI↔HTTP identity over 150 parameter sets (1 test)
```

The one-command loop is `var/m6l/scripts/verify.sh <tag>` (fmt → clippy →
workspace units → store → server → e2e → api → listener check → stub scan;
logs under `var/m6l/logs/`). Earlier milestones keep their own copies
(`var/m*/scripts/verify.sh`).

### 5.3 Fixtures, goldens, guards

- `fixtures/<name>/` holds fixture *content*; tests materialise it into a
  temp git repo with fixed author/date (stable SHA), index it twice into two
  repos (determinism), force-rebuild (cache parity), compare against the
  in-memory pipeline (store fidelity), then check guards and invariants.
- `crates/rb-cli/tests/golden/<fixture>/` holds `dump.jsonl` (canonical
  dump), `must-exist.txt` / `must-not-exist.txt` (precision guards: one
  `<KIND> <src> -> <dst> [<certainty>] {attrs}` per line; `*` matches any key;
  every non-`*` must-not-exist source must name a real node so typos can't
  pass vacuously), plus per-fixture extras (`scip-status.txt`, …).
- Re-blessing: `RB_BLESS=1 cargo test -p rb-cli --test e2e <name>` rewrites
  the golden from whatever the tree yields — **green means nothing until the
  blessed diff is reviewed row by row** (`git diff`; every added/removed row
  classified; a bless that only moves coverage rows while feature rows are
  missing is a red flag). Every new guard must be proven biting: perturb the
  fixture (or the code) so it fails, then restore byte-identically and re-run
  green.
- Fixture catalog (see `fixtures/README.md` for the older half):

| Fixture | Covers |
|---|---|
| `ts-express-prisma`, `polyglot-monorepo`, `java-spring-jpa`, `ts-workspaces`, `java-lombok`, `java-gradle-catalog`, `ts-pnpm-yarn`, `contract-packs`, `topo-packs`, `go-chi-sqlx`, `python-fastapi-sqlalchemy` | structure, routes, SQL/ORM, SCIP, lockfiles, topics, gateway (per `fixtures/README.md`) |
| `kotlin-gradle` | Kotlin exact edges via scip-java (Gradle, Kotlin 2.2.0) |
| `csharp` | C# Tier B + .NET DI: registrations, ctor points, keyed services + `[FromKeyedServices]`, `[Inject]` properties (marked/unmarked/statics), minimal-hosting top level, enclosing namespaces (`SubHelper`), cross-namespace decoy (`OtherDecoy`), unregistered-consumer decoy |
| `java-guice` | singletons, `@Provides`, `bind().to()` + `toProvider()`, qualifiers, `@Assisted` exclusion (`AssistedShop`), field/setter points |
| `java-dagger` | modules, `@Binds`, `@Provides`, components, inject points |
| `ts-nest` | injectables, controllers, ctor points, token qualifiers |
| `spring-xml` | `<bean>`/aliases/scans, ctor-arg + property refs, unknown-ref diagnostic siting |
| `kotlin-beans` | function-form `beans{}` + top-level `val` form with qualifier wiring |

### 5.4 Invariant, measurement, determinism

- **Invariant-1** (`var/m6*/scripts/p9-invariant.sh`): base-vs-head dumps for
  every fixture (`--no-scip`, plus SCIP legs per precedent); dumps asserted
  non-empty; every differing row classified. Current standing: all fixtures
  byte-identical except the ones the milestone intentionally extends.
- **Public measurement** (`var/m6*/scripts/p8-measure.sh`): base-vs-head
  `--no-scip` dumps of pinned public repos (guice, spring-fu, AutoMapper,
  …), one at a time with `VACUUM` between (disk policy); added/removed rows
  classified by kind with source-read samples at the pins; 0 false edges is
  the bar. Pins are recorded in the scripts; re-measurement reuses them.
- **Determinism** is tested, not asserted: same commit into two repos dumps
  byte-identically; forced rebuilds match; AGE parity (`age_parity_failures`)
  runs inside every fixture test.
- **Mutant batteries** (`var/m6*/scripts/p*-mutants.sh` for M6j and earlier;
  manual batteries for M6k/M6l, recorded in contracts): each new branch gets a
  mutant that must fail its test. Beware vacuous mutants (a predicate over an
  empty set, a qualifier that matches nothing either way) — a surviving
  mutant is a test bug until proven otherwise. Re-run batteries after any
  `cargo fmt` (reflows break match patterns).

### 5.5 Troubleshooting

| Symptom | Cause / fix |
|---|---|
| `skipping:` lines in e2e/api | `RB_TEST_DATABASE_URL` unset — set it; skips are not passes |
| DB connection refused | compose not up, or wrong port (55432 vs 55433); `pg_isready -h 127.0.0.1 -p <port>` |
| SCIP jobs `unsupported` | missing images or wrong Docker context (`DOCKER_CONTEXT=desktop-linux`); `--no-scip` needs none |
| `t_*` repos / graphs left behind | killed runs; e2e start-up cleans `t_*` once per run; `rb gc --prefix t_` otherwise (only the 3 `fx_*` repos should persist) |
| Disk pressure during SCIP | `docker system df`; per-repo caches under `var/scip/cache`; `rb cache prune --facts --scip --dry-run` first |
| `rb serve` still listening after tests | kill it; the verify loop asserts no listener (`lsof -iTCP -sTCP:LISTEN`) |
| Golden mismatch after a change | intended (re-bless + review) or regression (bisect to the hunk with a two-file miniature, as in M6k H2/H3) |

## 6. Repository map for contributors

`crates/rb-{model,git,syntax,sql,frameworks,scip,indexer,store,api,server,cli}`,
`docker/` (SCIP + egress images), `migrations/` (embedded schema),
`fixtures/` + `crates/rb-cli/tests/{e2e.rs,api.rs,golden/}`,
`tasks/` (contracts, evaluations, lessons, todo), `docs/` (design + this
guide), `var/<milestone>/{scripts,logs,measure,base}` (verify loops, evidence,
binaries — gitignored scratch). Milestone discipline, earned the hard way:
contract up front with the closing bar decided there; test-first with killing
mutants; reviewed re-blesses; base-vs-head invariant and measurement before
close; two evaluator passes; `PIPELINE` bump at the first dump change with a
re-proof on the bumped tree; one file, one edit per block; never truncate
check output.
