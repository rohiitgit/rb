# Repository Brain

A deterministic, LLM-free code-intelligence graph for Git repositories. It is built with Rust, Postgres and Apache AGE.
The design is in [docs/repository-brain-design.md](docs/repository-brain-design.md).

**Status: milestone M0 (Foundations).** `rb` indexes a commit into a snapshot containing:

- one `File` node per blob, with its blob SHA, size, language and binary/generated flags;
- a single root `Service`;
- `IN_SERVICE` edges.

Each snapshot is stored relationally and projected into an AGE graph. The code graph itself arrives in M1–M4: CODEOWNERS, service boundaries, symbols, calls, SQL and routes.

## Quickstart

```sh
docker compose up -d --wait            # Postgres 18 + AGE 1.8.0 on 127.0.0.1:55432
cargo build
target/debug/rb migrate
target/debug/rb index /path/to/repo    # --rev <rev>, --repo <name>, --force
target/debug/rb status
target/debug/rb dump > snapshot.jsonl  # canonical, id-independent dump
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

Without `RB_TEST_DATABASE_URL`, the end-to-end tests in `crates/rb-cli/tests/m0.rs` print a notice and pass without running.

| Path | Contents |
|---|---|
| `crates/rb-model` | node/edge kinds, certainty, `GraphBuilder`, canonical dump |
| `crates/rb-git` | commit → manifest (gitoxide) |
| `crates/rb-store` | migrations, snapshot publish/retention, AGE projection, read-only Cypher |
| `crates/rb-cli` | the `rb` binary |
| `migrations/` | SQL schema, embedded into the binary |
| `fixtures/` | golden fixture repositories |
