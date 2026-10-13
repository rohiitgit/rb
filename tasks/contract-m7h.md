# M7h contract: the CLI remainder (stress batch F, part 1) (2026-10-13)

**Status:** contract written 2026-10-13; implemented 2026-10-13 (amendment 1). Branch `m7h-ux`, stacked on M7g (`52b7b90`). Part 2 (M7i) takes F32, F31, O3, U5, O7 and O4; an M7 close pass follows it.

## Why

| Finding | What's wrong |
|---|---|
| F3 / U1 | A checkout indexed with `--repo st_demo` is not recognised from its own directory (`demo_spring` ≠ `st_demo`). The hint says `rb index .`, which creates a **second** repository under the directory name. |
| U4 | `find --kind endpoint` without text is a clap error, though listing every endpoint is the natural question. `service NAME --limit` says "not a parameter of this request (allowed: name, snapshot)", which doesn't say why. |
| M7f residue | `find` with no matches prints nothing and exits 0: indistinguishable from a broken pipe. A bad `RB_CYPHER_TIMEOUT_MS` is reported as `--timeout-ms`. |
| U9 | A Cypher write fails with `permission denied for schema rb_fx_full_24084`; a `--cols` count mismatch fails with Postgres' `return row and column definition list do not match`. |
| U7 / F2 | No single command says whether the environment works: DB, AGE, migrations, the load-dir mount, Docker (with a timeout), the SCIP images and disk. |

## Design

```mermaid
flowchart LR
    I[rb index PATH] -->|publish ok, or up to date| C[(rb.repo.checkout<br/>= canonical git root)]
    Q[rb question, no --repo] --> G[canonical git root of cwd]
    G --> M{repos with<br/>checkout = root}
    M -- one --> R[that repo]
    M -- several --> D1[directive: indexed as a, b; pass --repo]
    M -- none --> N{repo named<br/>like the directory?}
    N -- yes, checkout NULL or equal --> R
    N -- yes, another checkout --> D2[directive: "x" is indexed from /other; pass --repo x]
    N -- no --> D3[directive: index it]
    I2[rb index ., no --repo] --> M2{one repo at this checkout?}
    M2 -- yes --> R2[re-index that repo]
    M2 -- no --> N2[directory name, as today]
```

**Trade-offs:**
- **A column on `rb.repo` vs a field in `tool_versions`.** The checkout belongs to the repository, not to one snapshot, and a lookup by path must not scan snapshots. **Chosen:** migration `0007_repo_checkout` adds a nullable `checkout text` with an index. Cost: `schema` in `tool_versions` changes, so the first `rb index` after upgrading rebuilds once (`Rebuild::Tools`), as every migration already does.
- **Several repos at one checkout: latest wins vs directive.** The invariant scripts index one checkout under two names on purpose. Silently picking the latest would answer from the wrong snapshot set. **Chosen:** a directive that lists them (design §9.1: never a guess across repositories).
- **Directory-name fallback.** Repos indexed before 0007 have no checkout. They keep today's rule, but only while their checkout is NULL or equal: a repo named `api` recorded at `/a/api` is never answered for `/b/api`.
- **Where the checkout is written.** It is written after a successful publish, or on the "up to date" path, never before indexing. A failed first index must not leave a repository row behind.

**Numbers:** `rb.repo` has tens of rows, so the lookup is one indexed `SELECT`, under 1 ms. `rb doctor` makes about 8 checks; Docker is bounded by the existing 10 s probe timeout (M7b), so a wedged daemon costs at most 10 s, reported as `fail`.

**NFR:** inference adds at most one query to every question (under 1 ms against a 2 s Cypher budget). `rb doctor` finishes in under 15 s even with Docker wedged.

## Claims

| # | Claim | Test |
|---|---|---|
| S1 (F3, U1) | `rb index` records the canonical git root of the indexed path as the repo's checkout (after publish and when up to date). Without `--repo`, questions, `check` and `dump` resolve the cwd's git root by checkout first, then by directory name only for a repo whose checkout is NULL or equal. Several repos at one checkout, or a name match recorded elsewhere, gives a directive naming them. `rb index` without `--repo` re-indexes the one repo already recorded at that checkout. `rb status` outside any match still lists every repo. | store db test (record, lookup, several); CLI unit for the decision table; e2e: index with `--repo x`, query from the checkout without `--repo` |
| S2 (U4) | `find` takes `--kind` without text and lists that kind (paged as usual); with neither, `<TEXT> is required` as today. Over HTTP, `q` may be empty only with `kind`. `service NAME --limit/--offset` says `--limit and --offset page the service list; a service card has no pages`. | rb-api parse units; CLI e2e |
| S3 (residue) | `find` with no matches prints `no matches for "x"` (plus the kind, when given) on stderr in text mode; stdout and `--json` are unchanged; exit 0. A bad `RB_CYPHER_TIMEOUT_MS` taken from the environment is named as `RB_CYPHER_TIMEOUT_MS`, not `--timeout-ms`. | CLI units |
| S4 (U9) | A Cypher write (read-only transaction or `rb_reader` privilege) reads `cypher is read-only: <query> was refused (<database message>)`; a RETURN/`--cols` count mismatch is `invalid_param` on `columns` (`--cols`): `--cols must name one column per RETURN item`. Exit and status follow the kinds. | rb-store db tests; rb-api unit |
| S5 (U7) | `rb doctor` checks, each as `ok`/`warn`/`fail` with a fix: the DB connects; Postgres and AGE versions; migrations up to date; the age-load dir exists, is absolute or resolvable, and is the one the DB sees (a probe file read back through `pg_stat_file`); Docker answers within its timeout, and its context; each pinned SCIP image is present (`warn` when absent); free disk under the SCIP work dir (`warn` under 6 GiB). Exit 0 with no `fail`, else 1. `--json` gives the same rows. | CLI unit for the row model and exit; e2e against the test DB |

## Verification

- Tests first, with mutants.
- Real-Docker suites: 0 failed, 0 skipped.
- Invariant against M7g: graphs byte-identical (no graph change; `tool_versions.schema` is not in the dump).
- The F3 repro (`var/stress/repro/F3.sh`) is re-run and resolves the repo from the checkout.
- Records: CHANGELOG, IMPLEMENT, contract, todo, stress-findings, lessons; README and usage guide where behaviour changed.

## Result (amendment 1, 2026-10-13)

- **Phases 1–4** are as in `tasks/todo.md`. Mutants m99–m122 killed. Four needed a stronger test first:
  - m104: the "up to date" path recording the checkout. The e2e now clears the checkout first, as a pre-M7h repository.
  - m111: `check` reading the variable.
  - m114: `serve` reading the variable. The test pairs it with a non-loopback bind, so a mutant fails fast instead of serving.
  - m119: a load dir the database does not see.
- **Amendments:**
  - `rb index` without `--repo` also refuses, with the same directive, a checkout indexed under several names, or a directory name already recorded at another checkout. Before, it silently re-indexed (and so took over) the other checkout's repository.
  - The timeout flags of `cypher`, `check` and `serve` no longer read the variable through clap. One helper reads it only when the flag is absent; clap's own message had named the flag.
  - `rb_store::redact` is public, so `rb doctor` redacts the URL the same way errors do.
- **Phase 5:**
  - fmt and clippy clean.
  - Real-Docker suites (`var/m7h/logs/p5-workspace.log`): **1,028 passed, 0 failed, 0 skipped**.
  - Invariant against M7g (`var/m7h/logs/invariant.txt`, sha256 `9e732a8b…`): **31/31 byte-identical**, as intended.
  - F3 repro with the M7h binary (`var/m7h/logs/repro-F3.txt`): from `demo-spring`, indexed as `st_demo`, `find`, `callers 'Db#load()'` and `index .` all answer as `st_demo` (`up to date`, no second repository).
  - `rb doctor` on the machine of record: 10 rows ok, about 2 s (NFR: under 15 s).
- **Docs:** README (checkout default, `find --kind`, Cypher read-only, `rb doctor`, the variable) and the usage guide. A stray code fence after the README's repository map, present on `main` too, rendered the status paragraph as code; it was removed.
- **Residue:** the Cypher timeout message still names `--timeout-ms` (the same text serves HTTP); `find --kind` lists in `find`'s rank order (shorter keys first), not alphabetically.
