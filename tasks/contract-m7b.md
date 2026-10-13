# M7b contract: regressions and hangs (stress batch B) (2026-10-13)

**Status:** S1–S5 implemented and verified 2026-10-13 (amendment 1). Stacked on M7a (`m7a-exact-labels`, `372963c`).

## Why

The stress campaign (baseline `eb496af`) found:
- **F1:** two DB suites red. `rb status` without `--repo` no longer lists all repositories (`bfa87ce`).
- **F2:** `rb index` hangs forever on an unresponsive Docker daemon.
- **F9/F10:** cubic (method chains) and quadratic (nested calls) indexing time in `rb-syntax`.
- **F11:** 44 s for one 183-line Express file with cyclic mounts.
- **F15:** a 1 GiB committed blob doubles peak RSS to about 2 GiB.

The goal: green suites, every external call bounded, and indexing time and memory bounded by input size.

## Claims

| # | Claim | Mechanism | Residue |
|---|---|---|---|
| S1 (F1) | **`rb status` without `--repo` never errors on context.** It reports the checkout's repository when that is indexed, and every repository otherwise (no git, unindexed checkout). Other questions keep the directive error. | `rb-cli/src/main.rs` `answer()` | Inference by directory name stays (F3, batch E) |
| S2 (F2) | **Every Docker CLI call is bounded.** The availability probe (`docker version`), `docker kill` and the `gc --egress` removals run under a timeout. On expiry the child is killed and the probe reports "docker is not responding", so jobs become `unsupported` as when Docker is absent. No orphaned child. | `rb-scip/src/runner.rs` (one bounded-run helper) | The SCIP job's own run is already bounded (`--scip-timeout-secs`) |
| S3 (F9/F10) | **Syntax walks are linear in the tree.** Role decisions (`mentions`, `java_role`, `es_role`) and chain extraction (`is_inner_link`, `assignment_target`) read ancestors from the walk's own stack, never `Node::parent()`, which tree-sitter computes by re-descending from the root. Facts must be byte-identical: the invariant shows every fixture and stress repo identical. | `rb-syntax/src/mentions.rs`, `calls.rs` (hot spots profiled) | — |
| S4 (F11) | **Route mount expansion does work proportional to its output.** Root cause to be profiled first. The output (endpoints, `route_prefix_unknown`) stays identical. | `rb-indexer/src/routes.rs` (TBD by profile) | — |
| S5 (F15) | **Memory per blob is bounded.** A blob no stage can use (over every scan cap) is classified from its header without loading. Otherwise the cost is at most one copy. Root cause to be confirmed first. | `rb-git/src/lib.rs` | — |

## Verification

- Tests first per claim. Timing claims get a pathological-input test with a wide margin (lesson 2026-09-30), plus a mutant per rule.
- fmt, clippy `-D warnings`, and workspace tests **with the DB** (`--no-fail-fast`, fake `docker` shim while Docker Desktop is wedged), with **0 failures** (F1 makes e2e/api green) and no `skipping:` lines.
- Invariant against the M7a binary (`var/m7a/bin/rb`): all fixtures and stress repos, with every differing row classified. S3–S5 are expected to be byte-identical.
- Re-run repros F1, F2, F9, F10, F11 and F15 (`var/stress/repro/`), with timings recorded.
- `PIPELINE` bump only if dumps change.

## Progress (amendment 1, 2026-10-13)

| Claim | Done | Evidence |
|---|---|---|
| S1 (F1) | `answer()`: `status` without `--repo` outside an indexed checkout is `None` (all repositories). README and testing guide updated. | The 2 red tests (e2e `json_answers_snapshots_and_error_codes`, api `api_bodies_equal_cli_json`) now pass. Outside git, `rb status` lists every repo. |
| S2 (F2) | `bounded_output` (spawn, drain, poll, kill and reap) for the probe (`PROBE_TIMEOUT` 10 s) and every other CLI call (`cli_timeout`, default 60 s). | Unit test with a hanging fake: bounds 1 s, no live fake PIDs. Mutants m15/m16 killed. Real wedged Docker Desktop: `rb index` exits 0 in 12 s with "docker is not responding … restart the Docker daemon" (was an indefinite hang). |
| S3 (F9/F10) | `Parents` (one walk, id → parent) replaces `Node::parent()` in the profiled hot spots: `mentions` (`es_role`, `java_role`, supertype and logger helpers), `call_qualifier`, `is_inner_link`, `assignment_target`. Two conversions were reverted when their mutants showed they are not hot: `local_by_ancestry` (stops at the nearest definition), `type_qualifier`. | Timing test of 7 pathological cases, about 4.7 s in a debug build (the 1,500-link JS chain alone took 365 s before). Mutants m17–m20 and m23 killed. Release: JS chain 1k/2k/4k is 0/1/3 s (was 16/124/>300 s). Go nested calls at 10k/20k are under 1 s (was 8 s / about 30 s). A 3,000-segment member path (found by probing) went from 85 s to under 1 s. |
| S4 (F11) | `Composer::prefixes` stops exploring mounts once its known prefixes are capped and the result is unknown (left unmemoised). | Counted work at 6 → 12 routers: 123× growth before, within 4× after, endpoints pinned (365/725). On the F11 file: 44 s → 1 s, dump byte-identical. Mutant m24 killed. |
| S5 (F15) | `find_header` gives the size. Blobs over 64 MiB have their head streamed through `git cat-file` (cut at 8,000 bytes), with the full load as fallback. | Manifest identical with streaming forced on and off (sniff-edge fixture). Mutant m25 killed. 1 GiB blob: 2,063 → 16 MiB peak RSS, 2.66 → 1.30 s. |

**Verification:**
- fmt and clippy `-D warnings` are clean.
- Workspace with DB (`var/m7b/logs/p6-workspace.log`, fake docker shim): **986 passed, 0 failed, 0 `skipping:`**.
- Invariant against the M7a binary (`var/m7b/logs/invariant.txt`, M7b binary sha256 `57c1fff5…`): **31/31 targets byte-identical** (18 fixtures, 13 stress repos). No `PIPELINE` bump; the facts cache re-keys through the `rb-syntax` code fingerprint.
- The DB was cleaned to the 4 pre-existing repos.

**Residue:**
- S3: role walks still climb one ancestor per level, so a single member chain is O(n·depth). A 4,000-link chain takes 3 s and an 8,000-link chain 12 s (both measured; quadratic, previously cubic). Realistic chains are unaffected. Linear time would need per-ancestor memoisation of the role walk.
- S5: blobs of 64 MiB and under are still loaded whole to sniff, costing at most about 128 MiB.
- S2: a CLI that spawns grandchildren holding its pipes would delay the drain after the kill. The Docker CLI does not.
