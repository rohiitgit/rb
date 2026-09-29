# Repository Brain

A deterministic, LLM-free code-intelligence graph for Git repositories. It is built with Rust, Postgres and Apache AGE.
The design is in [docs/repository-brain-design.md](docs/repository-brain-design.md).

**Status: milestones M0 (Foundations) and M1a (Structure).** `rb` indexes a commit into a snapshot containing:

- one `File` node per blob, with its blob SHA, size, language and binary/generated flags;
- one `Service` per detected project (package.json, Maven, Gradle, go.mod / `package main`, Cargo, pyproject, Dockerfile, `rb.toml`), marked deployable or library;
- `Owner` nodes from CODEOWNERS (GitHub semantics), with `OWNED_BY` edges that cite the deciding CODEOWNERS line;
- `IN_SERVICE` edges and diagnostics (skipped files, ignored or invalid CODEOWNERS lines, unreadable manifests).

Each snapshot is stored relationally and projected into an AGE graph. Symbols, calls, SQL and routes arrive in M1b–M4.

## Quickstart

```sh
docker compose up -d --wait            # Postgres 18 + AGE 1.8.0 on 127.0.0.1:55432
cargo build
target/debug/rb migrate
target/debug/rb index /path/to/repo    # --rev <rev>, --repo <name>, --force
target/debug/rb status
target/debug/rb dump > snapshot.jsonl  # canonical, id-independent dump
target/debug/rb service                # list services; `rb service <dir|name>` shows a card
target/debug/rb find auth --kind file  # search nodes by key or name
target/debug/rb cypher "MATCH (f:File)-[:IN_SERVICE]->(s:Service) RETURN s.name, count(f)" --cols service,files
```

Run `rb` from the project root. AGE's CSV loaders only read files under the server's `/tmp/age/`, and `compose.yaml` mounts `./var/age-load` there. From anywhere else, set `RB_AGE_LOAD_DIR` to that directory.

| Setting | Default |
|---|---|
| `RB_DATABASE_URL` | `postgresql://rb:rb@127.0.0.1:55432/rb` |
| `RB_AGE_LOAD_DIR` | `var/age-load` |

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
| `crates/rb-indexer` | pipeline: git → structure → snapshot graph + diagnostics |
| `crates/rb-store` | migrations, snapshot publish/retention, evidence, AGE projection, read-only Cypher, find/service queries |
| `crates/rb-cli` | the `rb` binary |
| `migrations/` | SQL schema, embedded into the binary |
| `fixtures/` | golden fixture repositories |
| `crates/rb-cli/tests/golden/` | reviewed golden outputs (`RB_BLESS=1` rewrites them; review before committing) |
