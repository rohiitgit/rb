# Committed go.work + JDK-25 fallback (2026-10-27)

**Status:** planned on branch `d12` (follow-up). Implements two `tasks/todo.md`
Out-of-scope items with measured designs (neither has a bench target that exercises
it, so the bench below is a no-movement proof, not a scoring run).

## Part A — a committed `go.work` at a job root

**Today:** the planner ignores `go.work`. A root with one committed fails its fetch:
workspace mode refuses the job's `GOFLAGS=-mod=mod` (`go: -mod may only be set to
readonly or vendor when in workspace mode`, measured in M9d planning). 0 bench
targets commit one.

**Design:**
- Planner (`jobs()`): read `<root>/go.work` for every Go root (new read, like D7).
  Present → the job is a workspace job whose dirs are the file's `use` entries
  (`.` alone when absent or unparseable). A committed file wins over D5's
  requires-analysis (the runtime builds with the committed file, so the fetch
  must cover its dirs, not D5's).
- Runtime (`go_runs`): never overwrite — write rb's generated file only when the
  export has no `go.work`. `GOFLAGS` empty, `GOWORK` naming the (committed) file,
  fetch = the workspace fetch over the planned dirs. D5b applies unchanged
  (a failure re-runs plain, whose fresh export drops `go.work`).
- `replace` lines ride along untouched; a layout that needs files outside the
  export fails honestly through D5b/status (recorded, not solved).
- No recipe change (`RECIPE_GO_WORK`: same fetch/index shape; the committed file
  is a tracked export, so the input hash already covers it). No `PIPELINE` bump
  (no bench target has one; dumps move nowhere — proven by the bench, not assumed).

**Claims:** A1 planner maps committed `use` dirs to `workspace` (red first).
A2 run writes no file, runs workspace-mode (red first; Fake records `go.work` text).
A3 e2e on a new `go-workspace-committed` fixture: `ok`, no GOFLAGS refusal.
A4 full bench `compare` exits 0 with zero changed lines.

## Part B — retry on JDK 25 when the release comes from outside the job

**Today (amendment 1, risk 4):** the release reader sees only in-repo properties.
A module whose effective release (>21) comes from an external parent (e.g. a
starter parent) is planned as JDK 21 and fails with `release version N not
supported`. The planner is offline by design, so the fix is reactive, D5b-style:
try JDK 21, and on exactly that stderr, retry the job on the JDK 25 image once.

**Design:**
- After a Maven job's fetch or index fails with `release version {N} not supported`
  (`N` > 21) in its tail, `run_projects` runs it again with the java25 image
  (timeouts, Docker-client failures and non-Maven jobs: no retry, as D5b).
- The retry needs the java25 image resolved: the CLI resolves it when a job plans
  JDK 25 **or any Maven job exists** (D12's flag widened; Go/Python/TS-only repos
  still skip it. Amends contract-d12.md's NFR: the saving now applies to
  non-Maven runs).
- Visible like D5b: new `RunStatus` variant stored as `status='ok', phase='jdk'`
  (migration 0008 extends the phase CHECK; `from_cached` maps it; status line
  carries a suffix). No existing row changes meaning (`ok`/`jdk` was never written).
  Only the retry's outcome is cached, under its image: the intermediate JDK 21
  failure is not stored, so a re-index rediscovers the fallback instead of needing
  retry-on-hit logic.
- No recipe change (different image ⇒ different cache key already). No `PIPELINE`
  bump (no bench Maven job is expected to trigger it; the bench proves it).

**Claims:** B1 no retry for timeouts, docker failures, non-Maven, or other tails;
retry exactly once on the release-version tail, as jdk25 (Fake-scripted unit test,
red first). B2 `from_cached("ok", Some("jdk"))` round-trips the variant + suffix.
B3 image available in the retry whenever a Maven job exists (unit: flag rule).
B4 full bench `compare` exits 0 with zero changed lines (a surprise trigger would
be a classified finding, not a silent pass).

## Phases

- **P1.** Failing tests first (A1, A2, B1–B3), then `jobs.rs`, `orchestrate.rs`,
  `projects.rs` (retry), `main.rs` (flag rule, suffix), migration 0008.
  Verify: unit suites, fmt, clippy.
- **P2.** New `go-workspace-committed` fixture + e2e (ok, goldens); manual probes:
  JDK25 image recorded for a Maven-21 repo, omitted for a Go-only repo (tools JSON).
- **P3.** Full bench + `compare` (expect exit 0, zero changed lines; anything else
  is classified, not blessed). Mutants for both changes, battery green.
  Records, commit.

## NFRs

- No dump moves on any target (both changes are no-ops where their triggers are
  absent, which is the whole bench corpus). Proven by P3, not asserted.
- No new `exact` fp possible (no join change): the bench gate enforces it.
