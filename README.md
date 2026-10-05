# Repository Brain

A deterministic, LLM-free code-intelligence graph for Git repositories. It is built with Rust, Postgres and Apache AGE.
The design is in [docs/repository-brain-design.md](docs/repository-brain-design.md).

**Status: milestones M0 (Foundations), M1a (Structure), M1b (Syntax), M2a (SQL core), M2b (ORM mappings), M3a (heuristic resolution) and M3b (SCIP) are done; M4 (frameworks) is next.** `rb` indexes a commit into a snapshot containing:

- one `File` node per blob, with its blob SHA, size, language and binary/generated flags;
- one `Service` per detected project (package.json, Maven, Gradle, go.mod / `package main`, Cargo, pyproject, Dockerfile, `rb.toml`), marked deployable or library;
- `Owner` nodes from CODEOWNERS (GitHub semantics), with `OWNED_BY` edges that cite the deciding CODEOWNERS line;
- `Symbol` nodes for the definitions Tree-sitter finds in TypeScript/TSX/JavaScript, Java, Python and Go files, with `DEFINED_IN` (pointing at the name) and `CONTAINS` edges. Parse results are cached per blob, so a re-index only parses changed files.
- `Table` nodes from SQL parsed with the real Postgres parser. `MIGRATES` edges come from `.sql` files, `QUERIES` edges from SQL strings in code (exact when passed to a known database call) and from ORM calls, and `MAPS_TO` edges from ORM entity mappings (JPA, Prisma, TypeORM, SQLAlchemy, Django).
- `CALLS`, `REFERENCES` and `IMPLEMENTS` edges between symbols: exact (provenance `scip`) where a SCIP indexer ran for the project in a container, heuristic otherwise (resolved from imports, packages and typed bindings, never by bare name across the repository).
- `Package` nodes and `DEPENDS_ON` edges from the external symbols a SCIP index names.
- `IN_SERVICE` edges and diagnostics (skipped files, ignored or invalid CODEOWNERS lines, unreadable manifests, syntax errors, oversized files, unresolved references, one `scip_status` per build root).

Each snapshot is stored relationally and projected into an AGE graph. Routes and cross-service request paths arrive in M4.

## Quickstart

```sh
docker compose up -d --wait            # Postgres 18 + AGE 1.8.0 on 127.0.0.1:55432
cargo build
target/debug/rb migrate
target/debug/rb index /path/to/repo    # --rev <rev>, --repo <name>, --force
target/debug/rb status
target/debug/rb dump > snapshot.jsonl  # canonical, id-independent dump
target/debug/rb service                # list services; `rb service <dir|name>` shows a card
target/debug/rb find login --kind symbol  # search nodes by key or name
target/debug/rb table-deps users         # migrations and code touching a table
target/debug/rb callers AuthService.login --depth 2   # what calls a function or method (Q1)
target/debug/rb impact "AuthService#login()."         # what breaks if it is removed (Q2)
target/debug/rb cypher "MATCH (f:File)-[:IN_SERVICE]->(s:Service) RETURN s.name, count(f)" --cols service,files
```

Run `rb` from the project root. AGE's CSV loaders only read files under the server's `/tmp/age/`, and `compose.yaml` mounts `./var/age-load` there. From anywhere else, set `RB_AGE_LOAD_DIR` to that directory.

| Setting | Default |
|---|---|
| `RB_DATABASE_URL` | `postgresql://rb:rb@127.0.0.1:55432/rb` |
| `RB_AGE_LOAD_DIR` | `var/age-load` |

If 55432 is taken, start the database on another port with `RB_DB_PORT=55433 docker compose up -d --wait` and point `RB_DATABASE_URL` at it.

### SCIP indexers

`rb index` runs a SCIP indexer in a container for every Maven project (`pom.xml`) and every npm, yarn or pnpm package with a `tsconfig.json`, and replaces that project's heuristic symbol keys and code edges with exact ones (design §5.2–§5.3, contract `tasks/contract-m3b.md`). It needs Docker and the two images:

```sh
docker build -t rb-scip-java:0.13.1 docker/scip-java      # slim local scip-java image (see docker/scip-java/README.md)
docker pull --platform linux/amd64 sourcegraph/scip-typescript@sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845
```

Each job exports the project's files, runs a fetch phase on Docker's default bridge network (`mvn … go-offline` and the build, or `npm ci --ignore-scripts --logs-max=0`; no credentials, no host toolchains), then indexes with `--network none`. Indexes are cached in the database by `(project input hash, image)`; the input hash covers the project's blobs and the fetch recipe (`rb_scip::RECIPE`), so an unchanged project runs no container and an index from an older recipe is never served. Without Docker or an image the project stays heuristic and `rb status` says so (`scip: … skipped`/`unsupported`); the images' identities are part of the snapshot's tool configuration, and a snapshot whose jobs did not all finish (`skipped`, `unsupported`, `environment`, `timeout`) is never "up to date" while SCIP is on, so the next `rb index` tries again (`rebuilding: n SCIP jobs did not finish last time`).

| Flag | Meaning |
|---|---|
| `--no-scip` | run no indexer; every project is heuristic (a different tool configuration from a run with indexers) |
| `--scip-work-dir <dir>` (`RB_SCIP_WORK_DIR`, default `var/scip`) | exports and package caches; must lie under the home directory (the path the runtime's VM shares, see below) |
| `--scip-timeout-secs <n>` | budget per phase (defaults 600 s fetch, 900 s index); a run over it is killed and reported as `timeout`, not cached |
| `--no-cache` | also re-run every SCIP job and replace its cached outcome |

#### Container runtime

`rb` talks to whatever daemon the Docker CLI's **current context** selects (`docker context show`), and the two images must exist in that daemon. Docker Desktop (`desktop-linux`) is the runtime of record: the e2e tests, the goldens and the public-repo measurements were made on it. If `docker context show` prints something else (another runtime such as colima holds neither image and would let `docker run` pull scip-typescript into its own VM), run `rb` with `DOCKER_CONTEXT=desktop-linux` in the environment, or switch once with `docker context use desktop-linux`. Then, in that context:

```sh
docker build -t rb-scip-java:0.13.1 docker/scip-java      # slim scip-java image (docker/scip-java/README.md); absent → `unsupported`
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
| `crates/rb-frameworks` | ORM packs: JPA, Spring Data, Prisma, TypeORM, SQLAlchemy, Django |
| `crates/rb-scip` | SCIP: project detection, container jobs (fetch + offline index), index cache key, `index.scip` decoding, symbol parsing |
| `crates/rb-indexer` | pipeline: git → structure → syntax (cached) → SCIP jobs (cached) → join → snapshot graph + diagnostics |
| `crates/rb-store` | migrations, snapshot publish/retention, evidence, AGE projection, read-only Cypher, find/service queries |
| `crates/rb-cli` | the `rb` binary |
| `docker/scip-java/` | the slim scip-java image |
| `migrations/` | SQL schema, embedded into the binary |
| `fixtures/` | golden fixture repositories |
| `crates/rb-cli/tests/golden/` | reviewed golden outputs (`RB_BLESS=1` rewrites them; review before committing) |
