# D12-lazy contract: skip the fifth `docker image inspect` (2026-10-27)

**Status:** planned 2026-10-27 on branch `b15` follow-up; implements `tasks/todo.md` D12 item. HEAD `236853c` (B15 closed).

## Why

Every SCIP-mode `rb index` runs 5 Docker CLI calls before any work: `docker version` plus 4
`docker image inspect` (java, java25, go, proxy). The java25 inspect (23.6 ms wall, 10.0 ms
child CPU per run) is needed only when a job's poms declare a Maven release above 21, which
is one bench target (spring_boot_microservices). Every other run pays it for nothing.

## Design

```mermaid
flowchart LR
    CLI[rb index] --> PLAN[plan jobs from manifest]
    PLAN -->|any jdk25 job?| RES[Images::resolve with flag]
    RES --> TOOLS[tool_versions without scip_java25 unless needed]
    TOOLS --> UPTODATE[up-to-date check]
    UPTODATE --> RUN[index_commit plans again, identical]
```

- **State:** `Job.java_release` already exists (contract M9d D12); `jobs()` is a pure
  function of the file list. No new state, no migration.
- **Derived:** `need_jdk25` in the CLI, from a plan over the post-exclusion files
  (the same `analysed` set `index_commit` plans from, so the flag and the run agree
  by construction).
- **`Images::resolve(runner, jdk25: bool)`:** `false` records java25 as "not looked
  up" without inspecting. `for_job` behavior unchanged (a jdk25 job with an
  unlooked-up image is `unsupported`, unreachable when the flag is correct).
- **`tool_versions(scip, images, jdk25: bool)`:** omits `scip_java25` when false.
  First run after upgrade rebuilds every SCIP-on snapshot once (stored tools shape
  changes); afterwards stable, since the plan is a pure function of the commit.
- **Dumps do not move:** `scip_status` records per-job image IDs, and every job that
  runs resolves the same image as before; only snapshot metadata changes. No
  `PIPELINE` bump.
- **Call sites:** index (planned flag), `gc --scip` prune (false; it only reads
  `.java`), `rb doctor` (full; it reports presence). `Images::unresolved()` unchanged.

## Claims

| # | Claim | Test |
|---|---|---|
| C1 | `resolve` with `false` never calls `image_id` for the java25 image; with `true` it does | `resolve_skips_the_jdk25_lookup_when_no_job_needs_it` (fake runner counting calls), red first |
| C2 | `tool_versions` omits `scip_java25` when false, records it when true | updated `tool_versions_record_image_identities_when_scip_is_on`, red first |
| C3 | CLI plans the flag from the commit (jdk25 pom → true; otherwise false) | `jdk25_plan_flag_follows_the_highest_declared_release` on `jobs()` + `jdk()`, red first |
| C4 | No dump moves on any target | full bench, `compare` vs base exits 0 with zero changed lines |
| C5 | One-time rebuild, then stable | e2e: index, re-index up-to-date, downgrade-shape reasoning in records |

## Phases

- **P1.** C1–C3 tests first, then the change (`orchestrate.rs`, `main.rs`, tests). Verify: `cargo test -p rb-scip --lib`, `-p rb-cli` unit suites; fmt; clippy.
- **P2.** e2e subset with live DB (up-to-date + a SCIP run asserting the tools JSON shape).
- **P3.** Full bench + `compare` (expect exit 0, no accept file). Records, commit.

## NFRs

- SCIP-mode `rb index` on a repo with no jdk25 job: 4 Docker CLI calls, not 5 (one fewer `docker image inspect`, ~24 ms wall saved).
- No `exact` fp rise possible (no dump moves); the gate is `compare` exit 0 with zero changed lines.

## Amendments

### Amendment 1 (2026-10-27, Part B): the flag covers any Maven job

D12 as built resolves and records `java25` only when a job plans for JDK 25.
Part B's fallback needs the image whenever a Maven job might fail onto it, so
the flag is `needs_jdk25() || any Maven job`. Go/Python/TS-only runs still skip
the lookup. NFR restated: the inspect is skipped on runs with no Maven jobs.
