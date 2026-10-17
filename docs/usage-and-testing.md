# Using and testing Repository Brain

This is the operator's guide: how to run the `rb` tool end to end, and how to
run every test suite. It reflects the tree as of M7j (M0–M7j, M7 closed). Design rationale
lives in `docs/repository-brain-design.md`; the per-milestone contracts,
evaluations and measurements in `tasks/`; the short tour in `README.md`. M7a–M7j
came out of a stress campaign over 18 fixtures and 13 public repositories; its
findings, repros and what each milestone fixed are in `tasks/stress-findings.md`.

## 1. What the system does

`rb` indexes a Git commit into a **snapshot**: a relational store plus an
Apache AGE graph of files, services, symbols, tables, endpoints, packages and
the edges between them (`DEFINED_IN`, `CONTAINS`, `CALLS`, `REFERENCES`,
`IMPLEMENTS`, `INJECTS`, `EXPOSES`, `HANDLED_BY`, `CONSUMES`, `QUERIES`,
`MAPS_TO`, `MIGRATES`, `DEPENDS_ON`, `IN_SERVICE`, …), with per-edge certainty
(`exact` only when proven: by SCIP indexers for calls, references and
implementations; by structure read directly (definitions, containment, a
file's service, CODEOWNERS); or by a literal the code states, such as SQL at a
known sink, an ORM mapping by stated name, a literal route, a manifest version;
otherwise `heuristic`/`ambiguous`/…),
evidence spans, and a diagnostics table (unresolved references, syntax errors,
unreadable manifests, …).

Language coverage (all Tier B unless noted):

| Language | Calls/refs | DI |
|---|---|---|
| TypeScript/TSX/JavaScript | heuristic; `exact` via scip-typescript | NestJS (`@Injectable`, `@Controller`, constructor points, token qualifiers; `@InjectRepository` points are `external`) |
| Java | heuristic; `exact` via scip-java (Maven + Gradle) | Spring (stereotypes, `@Bean`, `@Value`-exclusion, qualifiers, Lombok points, Guice/Dagger in `java-guice`/`java-dagger` fixtures), Spring XML (`<bean>`, `<constructor-arg>`, aliases, component scan) |
| Kotlin | heuristic; `exact` via scip-java in Gradle builds | `beans { }` DSL incl. top-level `val` form, `ref()` points, `@Assisted` exclusion |
| C# (Tier B only, no SCIP) | heuristic (namespaces incl. file-scoped/dotted, enclosing-namespace scope, global namespace) | `Add/TryAdd/Keyed` registrations incl. top-level minimal hosting, read with an explicit `using`, implicit usings (Web/Worker/BlazorWebAssembly SDK with `ImplicitUsings`) or a project `global using`; a class registered as an implementation (`AddScoped<IX, X>`) is an owner; constructor parameters (not body locals) + `[Inject]`/`[FromServices]`/`[FromKeyedServices]` property points, key matching. Open generics (`typeof(IRepo<>)`) are not read |
| Python / Go | heuristic + endpoints/clients; `exact` via scip-python / scip-go | — |
| SQL / ORM | pg_query parse-tree walk; JPA, Spring Data, Prisma, TypeORM, SQLAlchemy, Django; `.sql` files are migrations by path convention (migrations dir, Flyway `V…__`, numbered, `.up.sql`) or DDL, else query files (aiosql `-- name:`, HugSQL `-- :name`) | — |

Routes are read for Spring, Express (literal and constant paths, `require('express')()`),
FastAPI (prefixes composed across modules; an unresolvable prefix stays
`prefix_unknown`), gin, chi and net/http, plus gRPC services from `.proto`
files, attributed to the services that register a server. Clients: fetch,
axios (including default-exported instances), Spring clients, Go/TS/Java gRPC
stubs.

**Not analysed, and reported** (see `rb status`):
- **Excluded files.** These keep their `File` node, are never parsed, and the node carries `excluded: <reason>`:
  - Go `vendor/` next to `vendor/modules.txt`;
  - `node_modules/`;
  - `.gitattributes` `linguist-vendored`/`linguist-generated`;
  - TypeScript `outDir` compiler output;
  - `rb.toml` `ignore = [globs]`.
- **Code without a grammar:** PHP, Ruby, Rust, C/C++, Scala, Swift, Dart and Elixir.
- **Frameworks without a pack**, named per service: echo, fiber, flask, Django views, koa, fastify, hapi, routing-controllers, NestJS controllers, Micronaut and Quarkus.

Questions answered: `find`, `service` (card), `callers` (Q1), `impact` (Q2),
`table-deps` (Q3), `path` (Q4), `diff`, `cypher` (scalars as text; nodes,
edges and paths as agtype text), JSON API + `serve`, and architecture gates
via `check`:
- **Rules:** `orphan_endpoints` (an endpoint no *unambiguous* client link consumes), `unowned_table_writes` and `service_cycles`.
- **Exit codes:** 0 clean, 1 violations, 2 on any error, including an unreachable database.
- **`--baseline FILE`** accepts the violations listed in a saved `rb check` output.
- **The orphan rule** notes on stderr how many client call sites it could not link.

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

target/debug/rb status --repo demo_spring   # sql/code/http/di/rta lines, plus what was not analysed
target/debug/rb service --repo demo_spring  # service list; `rb service shop` for the card

# Questions (a name matching several symbols needs --pick N, never a silent pick)
target/debug/rb find Store --kind symbol --repo demo_spring
target/debug/rb callers "Db#load()." --repo demo_spring
# callers of Db#load().  src/Db.java:4  [shop]
# precision=cha
#   Store#stock().  src/Store.java:15  [shop]  heuristic
target/debug/rb callers 'Db#load()' --repo demo_spring   # a near-miss key resolves when one symbol matches
target/debug/rb impact "Store#" --repo demo_spring --pick 1
# removing Store# … breaks: … 1  Audit#  src/Audit.java:3  [shop]  heuristic …
target/debug/rb dump --repo demo_spring > /tmp/demo.jsonl   # canonical dump

# Architecture gates
target/debug/rb check --repo demo_spring        # exit 0 clean, 1 violations, 2 errors
target/debug/rb check --repo demo_spring > .rb-baseline                 # adopt today's state
target/debug/rb check --repo demo_spring --baseline .rb-baseline        # gate new violations only
target/debug/rb cypher "MATCH (s:Service) RETURN s.name" --repo demo_spring
target/debug/rb cypher "MATCH (n) RETURN n LIMIT 1" --repo demo_spring  # {…}::vertex

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
  A key in symbol syntax that misses (`Db#load()`, `Db#load`) resolves when
  `rb find`'s rule (case-insensitive substring of key or name) yields exactly
  one symbol; a miss suggests a quoted, copy-pasteable `rb find` command.
- `--repo` defaults to the repository indexed from the git checkout you run
  from: `rb index` records each repository's checkout (its canonical git
  root), whatever `--repo` named it, and `rb index .` there re-indexes it.
  A repository indexed before M7h (no checkout recorded) is found by the
  directory name, sanitised like `index` names it. An explicit `--repo` wins.
  Otherwise the CLI refuses to guess and says so:
  `--repo is needed: no indexed repository matches this checkout ("x"); …`,
  `… this checkout ("…") is indexed as several repositories (a, b); …`,
  `… repository "api" was indexed from another checkout ("…"); …` or
  `… no git repository found in "…"; …`. `rb status` is the exception: there
  it lists every repository.
- `rb callers` and `rb impact` take `--limit N`: N rows shown, and
  `truncated: limit`. `callers` first summarises every row found (per service
  and per entry kind); `impact` headers keep each list's full length; the
  `certainty:` line counts every row found. `rb callers --exclude-tests`
  leaves out callers in test code and counts them. The target's own entry
  marks follow it on the first line (an endpoint handler names its endpoint).
  `--pick N` beyond the only candidate is refused.
- `rb status`'s `http:` line counts routes only a spec states:
  `18 routes (3 from specs only)`. Express routes match client paths
  case-insensitively (as Express does by default); such a link carries
  `case: folded` (JSON, dumps) and `rb path` notes `case folded`.
- An endpoint is `exact` only when code registers it. An OpenAPI operation
  with no code route is `synthetic`; a code route the spec also states stays
  `exact`. Routes registered in test files (`rb_model::is_test_path`: `_test.go`,
  `src/test/`, `*.spec.ts`, …) are not endpoints and set no prefix; each such
  file gets one `routes_in_test_code` diagnostic with its route count.
- A context path set in a `native` Spring Cloud Config server's shared files
  (`search-locations: classpath:/shared`) applies to the client whose
  `spring.application.name` names the file (`account-service.yml`), or to
  every client through the shared `application.yml`. It is `synthetic`, and each
  client gets a `config_server_context_path` diagnostic. The remote value
  overrides local files, and an empty one (`context-path: ""`) puts the routes
  back at the root, as Spring serves them. Two such servers in
  one repository apply nothing (`config_server_ambiguous`).
- A Spring Data repository annotated `@RepositoryRestResource(path = "accounts")`
  in a service whose build file names `spring-boot-starter-data-rest` exposes
  Spring Data REST's resources as `synthetic` endpoints: `GET`/`POST /accounts`,
  `GET|PUT|PATCH|DELETE /accounts/{id}` (CRUD bases), and
  `GET /accounts/search/findByEmail` per query method. The context path and
  `spring.data.rest.base-path` prefix them. A `RepositoryRestConfigurer` in the
  service, which can move the base path in code, caps them at `heuristic`
  and adds one `data_rest_base_unread` diagnostic `{service}`: a Java or
  Kotlin class extending it (or `RepositoryRestConfigurerAdapter`), a Java
  `@Bean` method returning one, or a `RepositoryRestConfigurer.withConfig(…)`
  call. Unannotated repositories, Kotlin repositories and default paths are
  not read.
- Go SQL built with `fmt.Sprintf("SELECT … FROM %s", usersTable)` is parsed with
  the package constant folded in, when `usersTable` is an untyped or `string`
  `const` declared once in the package (any file but tests) and nothing
  shadows it. A constant of a named type is not folded: `%s` prints it
  through its `String()` method.
  Only `%s`, `%v` and `%%` fold; anything else leaves the string as written.
  The edges stay `heuristic`, and each file with a folded call gets one
  `sql_folded` diagnostic `{sites, constants}`.
- A Go method declared in another file than its type (`func (h *Handler)
  signUp` in `auth.go`, `type Handler` in `handler.go`) is a member of the
  type: `h.signUp()` and a gin handler `r.POST("/x", h.signUp)` resolve to it.
  It is never a package-level name, so a bare `signUp()` does not.
- Without SCIP, Go receivers are also typed by a constructor's result
  (`u := article.NewService(…)` then `u.Fetch()`; results `T` or `(T, error)`),
  by calls inside a qualifier (`th.App.Srv().Store()`), through type aliases
  (`type Context = web.Context`), through embedded structs
  (`ArticlePosterBg{ *ArticlePoster }` promotes `ArticlePoster`'s methods) and
  through a package-level var declared in a sibling file (a test file's var is
  seen by test files only). Embedding never makes `IMPLEMENTS`.
- TypeScript `paths` are read from `tsconfig.json` files with comments and
  trailing commas, and `import f from './m'` resolves to what `m` exports as
  default. Python resolves `super().m()`, names from `from m import *` (not
  `_` names, and not from a module that defines `__all__`), names a package's
  `__init__.py` only re-imports, an imported module-level instance
  (`smth = Smth()` then `smth.f()`), `raise A` as a construction of `A`, and a
  subclass constructor call that reaches an inherited `__init__`.
  All these edges are `heuristic`. Each file that used one of these forms gets one
  `tier_b_forms` diagnostic counting the references per form: `return_type`,
  `call_segment`, `type_alias`, `embedded`, `package_var`, `default_import`,
  `super_call`, `star_import`, `reexport`, `module_variable`, `inherited_init`.
- Go with SCIP across modules of one repository (each `go.mod` is its own
  scip-go job): a call from one module into another module of the repository
  (`model.NewId()` from `server` into `server/public`) links to that module's
  own definition with provenance `scip` and certainty `synthetic`, not `exact`.
  The compiler resolved the name against the version the caller's `go.mod`
  requires from the module cache, which can differ from the in-repo code.
  Each (from module, to module) pair gets one `scip_cross_project` diagnostic
  `{to, synthetic, ambiguous, unanchored}`. `ambiguous` means two modules
  declare the same module path (no edge). `unanchored` means the target module
  has no definition node for the name, for example a name the required
  version has and the in-repo code does not. `rb status`'s
  `exact calls (scip)` counts `exact` calls only, so these are not in it.
- Go with SCIP: a type that satisfies an interface gets `IMPLEMENTS` to it from
  the type (`Card# → Payer#`). scip-go also links each method to the interface
  method; that link gives no edge, since the interface method has no node, and
  is counted in `relationships dropped` on the `scip:` line.
- `rb table-deps` shows a Spring Data repository type with its own `QUERIES`
  row: all four ops, `synthetic`. That row stands for the CRUD it inherits
  (`save`, `findById`, `delete`, …), not for a query it states. Its declared
  methods have their own rows.
- Two `rb index` runs of one repository take turns: the second prints
  `waiting: another rb index of <repo> is running`, then usually finds the
  first's snapshot up to date.
- `rb find --kind endpoint` (no text) lists the nodes of a kind, 20 a page
  (`--limit`, `--offset`); a search
  that matches nothing says `no matches for "x"` on stderr (stdout stays
  empty, exit 0).
- Cypher is read-only: a write is refused as `cypher is read-only: …` (exit 5);
  a RETURN whose item count differs from `--cols` is a `--cols` error (exit 2).
- `RB_CYPHER_TIMEOUT_MS` sets the default timeout of `cypher`, `check` and
  `serve`; a bad value is reported as the variable, not as the flag.
- `rb doctor` checks the environment (database, AGE, migrations, the age-load
  mount, Docker, the SCIP images, the work dir, disk) and prints a fix for
  each `warn` and `fail` row; `skip` rows name the check they need. It exits 1
  when one fails; a database that never answers fails after 10 s.
- **Errors have one shape.** Text mode prints one `Error:` line, naming flags and arguments as you typed them (`--cols`, `<TEXT>`). `--json` and HTTP return the envelope `{"api": 1, "error": {"kind", "message", …}}`. Exit code and HTTP status per kind:

  | Exit | Kind | HTTP |
  |---|---|---|
  | 1 | `internal` | 500 |
  | 2 | `invalid_param` | 400 |
  | 3 | `not_found`, `no_live_snapshot`, `snapshot_not_retained` | 404 / 404 / 410 |
  | 4 | `ambiguous` | 409 |
  | 5 | `cypher`, `cypher_timeout` | 422 |
  | 6 | `unavailable` (the database cannot be reached; the message names the redacted URL, the cause, and how to start it) | 503 |

- **Excluded files keep their `File` node,** so `rb find vendor/x.go` still answers; nothing in them is parsed. `rb status` prints `excluded from analysis: <reason> <n> files`.

## 5. Testing

### 5.1 Unit tests (no database)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Per-crate: `cargo test -p rb-syntax --lib`, `-p rb-frameworks`,
`-p rb-indexer`, etc. (121 + 203 + 355 lib tests at M7j close; 1,047 across the workspace).

### 5.2 Database-backed suites

Without `RB_TEST_DATABASE_URL`, DB tests print `skipping:` and pass — **that
is not a pass**. Always set it:

```sh
export RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb
cargo test -p rb-store --lib        # migrations, publish/retention/gc, check rules (84 tests)
cargo test -p rb-server -- --nocapture  # serve binding, limits, parity (12 tests)
cargo test -p rb-cli --test e2e     # fixtures: determinism, guards, goldens, AGE parity (52 tests)
cargo test -p rb-cli --test api     # CLI↔HTTP identity over 150 parameter sets (1 test)
```

The SCIP e2e tests (`scip_*`) also need a **responsive** Docker daemon with
the images built, and must not share the machine with other builds (container
fetches time out under contention and the failed fetch is cached). The full
run, the bar every M7 milestone closed on (0 failed, 0 `skipping:` lines):

```sh
export RB_TEST_DATABASE_URL=postgresql://rb:rb@127.0.0.1:55433/rb DOCKER_CONTEXT=desktop-linux
cargo test --workspace --no-fail-fast -- --test-threads=1 2>&1 | tee test.log
grep -c 'skipping:' test.log    # must be 0
```

(`var/` holds per-milestone scratch scripts and logs, gitignored; they are not
part of the tree.)

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

- **Invariant-1:** base-vs-head `--no-scip` dumps of every fixture and of the
  13 pinned stress repositories (pins in `tasks/stress-findings.md`), with
  every differing row classified. Classify **by row identity** (node kind+key,
  edge endpoints+kind), not by a line diff: a big removal makes `diff` report
  unchanged rows as moved. M7 closes recorded e.g. "28/31 byte-identical,
  every row of the 3 diffs classified" (M7e) and "31/31" for milestones that
  change no graph (M7f).
- **Public measurement** (`var/m6*/scripts/p8-measure.sh`): base-vs-head
  `--no-scip` dumps of pinned public repos (guice, spring-fu, AutoMapper,
  …), one at a time with `VACUUM` between (disk policy); added/removed rows
  classified by kind with source-read samples at the pins; 0 false edges is
  the bar. Pins are recorded in the scripts; re-measurement reuses them.
- **Determinism** is tested, not asserted: same commit into two repos dumps
  byte-identically; forced rebuilds match; AGE parity (`age_parity_failures`)
  runs inside every fixture test.
- **Mutant batteries:** each new rule or guard gets a mutant (one exact text
  replacement, run the test, restore byte-identically) that must fail its
  test; contracts record them (M7: m1–m98). A surviving mutant is a test bug
  until proven otherwise. Usually the test lacks the input that only that
  rule explains, sometimes the code is dead (delete it), rarely the mutant is
  equivalent (record why). Re-check mutation text after any `cargo fmt`
  (reflows break exact matches). Run the battery with
  `python3 -I bench/mutants/run.py bench/mutants/m8b.json --jobs 4` (parallel
  worktrees; the main tree stays clean): a full all-KILLED run is cached in
  `var/bench/mutants-cache.json` and re-reports instantly while code and
  specs are unchanged. A spec names `file` under `crates/<crate>/src`;
  `crate` defaults to `rb-bench`, and any other crate runs
  `cargo test -q -p <crate> --lib <test>`, and its spec must name the `test`
  filter (M9a's `bench/mutants/m9a.json` uses both crates). With a filter,
  KILLED needs a failing test whose name contains it (cargo's
  `---- <name> stdout ----`), so a wall-clock test that flakes under load
  cannot score a kill. An rb-bench spec without a filter is KILLED by any
  failing test (`test result: FAILED` or `panicked at`). A build error, a
  spec with no filter outside rb-bench, failures only outside the filter, or
  a mutation that leaves the file unchanged, is INVALID: it gives no
  verdict, so rewrite the spec. So is a spec whose filter selects a
  wall-clock test, before anything runs: such a test can fail under the
  battery's own load (M9b N2). The list lives in one place,
  `bench/mutants/timing_tests.txt`, written by
  `python3 -I bench/mutants/timing_tests.py`: every test whose assertion reads
  the clock (`.elapsed()`, `Instant`) or bounds a duration, or whose name
  contains `stay_fast` (17 in 6 crates at M9b), each with its full path as
  `cargo test -- --list` prints it (`resolve::tests::name`). Re-run it after
  adding a timing test; it compiles the test targets it reads. A filter
  selects a listed test when the test's full path contains it, as cargo
  matches, so a module filter such as `resolve` or `tests` is INVALID in a
  crate with a timing test there; name the test.
- **Stress repros:** `tasks/stress-findings.md` lists each finding with its
  repro and what fixed it. Re-run the repros of the findings a change touches.

### 5.5 Accuracy benchmark (`bench/`, `rb-bench`)

The invariant shows *that* a graph changed; the benchmark says whether it got
better or worse against answer keys (contract M8a). Everything it needs is
committed: `bench/manifest.tsv` (targets and pins), `bench/fetch.sh`,
`bench/run.sh`, `bench/cleanup.sh`, and the `rb-bench` crate (scoring,
rendering, the gate).

```bash
# 1. Index and dump every target with one binary (minutes warm; a cold SCIP
#    pass is an hour). Run it detached; it runs from a copy of itself.
nohup bench/run.sh var/m8a/bin/rb > var/bench/run.log 2>&1 & disown
# 2. Score: writes the scoreboard (counts only, deterministic) and the perf file.
cargo run -q -p rb-bench -- score var/bench/out/<sha12> --milestone m8a \
  --rb-commit $(git rev-parse HEAD) \
  --write bench/scoreboard/m8a.json --perf bench/scoreboard/m8a-perf.json
# 3. Read it.
cargo run -q -p rb-bench -- render bench/scoreboard/m8a.json
cargo run -q -p rb-bench -- render bench/scoreboard/m8a.json --certainty
# 4. Gate a milestone against the previous close: 0 pass, 1 regression,
#    2 not comparable (other pins or keys: re-score the base first).
cargo run -q -p rb-bench -- compare bench/scoreboard/m8a.json bench/scoreboard/m8b.json \
  --accept bench/accept.tsv --perf bench/scoreboard/m8a-perf.json bench/scoreboard/m8b-perf.json
```

- **Keys (M8b adds three).** *microSecEnD*: 12 Java microservice
  applications' hand-made dataflow diagrams (`kind microsecend` in the
  manifest, pinned and checked by `bench/pin_microsecend.py`); scores
  `service_flows` and `endpoints`. *Hand keys*: `bench/keys/<target>/key.txt`,
  written blind to rb's output in the format of `bench/keys/FORMAT.md`
  (endpoints, handlers, table access, entity mappings), adjudicated through
  `rb-bench statements` → a blind adjudicator → `bench/adjudicate.py`, which
  appends `ADD`/`REMOVE` entries to `adjudication.md` (`key.txt` is never
  edited; the raw key is scored too, as `handkey_raw`). *The `exact` sample*:
  `rb-bench sample OUT` draws up to 60 `exact` edges per family (`scip`,
  `sql`, `orm`, `routes`, `manifest`) by the M3b rule, and judges' verdicts
  live in `bench/samples/exact.tsv` (keyed by rb's edge identity and pin, so
  they carry over); the gate fails while a drawn edge lacks a verdict.
  Verdicts are appended, never edited. Adding any changes the
  `exact:verdicts` key hash, so `compare` against the previous close exits 2.
  Re-score the base first: run `rb-bench score` with HEAD's `rb-bench` over
  the base's dumps against the new verdicts, check that the result equals the
  committed base except for `keys["exact:verdicts"]`, then `compare` against
  it (M9a: `var/m9a/m8b-rescored.json`).
- **Keys (M8a).** *SCIP agreement*: on each stress repo with SCIP mode, the
  `scip`-provenance `CALLS` of the SCIP run against the `--no-scip` run, in
  files SCIP covered. It is labelled `agreement`, never accuracy: it goes
  through rb's own SCIP join and sees only call sites rb's parser found.
  *PyCG*: the 119 micro-benchmark cases' `callgraph.json` against each case's
  `--no-scip` dump, in PyCG's names (`accuracy`). Endpoints, table access,
  service flows and `exact` precision get keys in M8b; until then they show
  `no key`.
- **Identity.** Edges are matched by location, not by key: a symbol is the
  path and line of its name token (from `DEFINED_IN`), a file is its path.
  Anything that can't be matched is counted under a reason
  (`unmatchable{…}`, `out_of_scope{…}`), never dropped, and a cell with no
  in-scope gold is `vacuous`, never 100 %.
- **The gate** is per cell: any `tp` drop, any rise in `fp`, `fn` or an
  excluded count, less gold, less SCIP coverage, a lower ambiguous-site hit
  rate, or a cell that vanishes or turns vacuous fails. A line
  `milestone<TAB>bench:target:mode:class<TAB>reason` in the accept file
  excuses one cell for that milestone (others are reported stale); a new
  wrong `exact` edge is never excusable. Perf only warns (wall above 1.25×,
  RSS above 1.15×, same hardware only).
- **At every milestone close**, commit `bench/scoreboard/<ms>.json` and
  `-perf.json`, quote `render` output in the contract's Result (never typed
  by hand), and classify every `changed` line `compare` prints, as the
  invariant's diffs are classified. Every contract's Verification section
  carries this line: *"Scoreboard against the previous close: `rb-bench
  compare` exit 0, or every accepted cell classified."*
- **Mutants of `rb-bench`** re-run with
  `python3 -I bench/mutants/run.py bench/mutants/m8a.json` and `m8b.json` (a
  spec whose text no longer occurs reports `STALE` and must be refreshed).
- `bench/run.sh` refuses to start unless `rb doctor` passes and 6 GiB are
  free; it exits 1 when any run or dump failed (`FAILED:` lines say which).

### 5.6 Troubleshooting

| Symptom | Cause / fix |
|---|---|
| Not sure what is wrong with the setup | `rb doctor`: one row per prerequisite, each with its fix |
| `skipping:` lines in e2e/api | `RB_TEST_DATABASE_URL` unset — set it; skips are not passes |
| DB connection refused / exit 6 `unavailable` | compose not up (a Docker Desktop restart stops it: `docker compose up -d --wait`), or wrong port (55432 vs 55433); `pg_isready -h 127.0.0.1 -p <port>` |
| `scip: docker is not responding` (index continues heuristic after ~10 s) | the Docker daemon is wedged, typically after the disk filled: restart Docker Desktop, then `docker version` must answer |
| SCIP jobs `unsupported` | missing images or wrong Docker context (`DOCKER_CONTEXT=desktop-linux`); `--no-scip` needs none |
| `t_*` repos / graphs left behind | killed runs; e2e start-up cleans `t_*` once per run; otherwise delete the `rb.repo` rows and run `rb gc` |
| Unexpected `excluded from analysis` counts | `rb.toml` `ignore`, a `.gitattributes` linguist mark, a `vendor/modules.txt`, or a tsconfig `outDir`; the `File` node's `excluded` attribute names the reason |
| `--repo is needed: … several repositories` | the checkout was indexed under more than one name; pass `--repo`, or delete the extra repository (`DELETE FROM rb.repo WHERE name = …`, then `rb gc`) |
| `rb check` fails CI on day one | adopt it with a baseline: `rb check > .rb-baseline`, then `rb check --baseline .rb-baseline` |
| Disk pressure during SCIP | `docker system df`; per-repo caches under `var/scip/cache`; `rb cache prune --facts --scip --dry-run` first |
| `rb serve` still listening after tests | kill it; the verify loop asserts no listener (`lsof -iTCP -sTCP:LISTEN`) |
| Golden mismatch after a change | intended (re-bless + review) or regression (bisect to the hunk with a two-file miniature, as in M6k H2/H3) |

## 6. Repository map for contributors

`crates/rb-{model,git,syntax,sql,frameworks,scip,indexer,store,api,server,cli}`,
`docker/` (SCIP + egress images), `migrations/` (embedded schema),
`fixtures/` + `crates/rb-cli/tests/{e2e.rs,api.rs,golden/}`,
`tasks/` (contracts, evaluations, lessons, todo, `stress-findings.md`),
`docs/` (design + this guide), `var/<milestone>/{scripts,logs,measure,base}` (verify loops, evidence,
binaries — gitignored scratch). Milestone discipline, earned the hard way:
contract up front with the closing bar decided there; test-first with killing
mutants; reviewed re-blesses; base-vs-head invariant and measurement before
close; two evaluator passes; `PIPELINE` bump at the first dump change with a
re-proof on the bumped tree; one file, one edit per block; never truncate
check output.
