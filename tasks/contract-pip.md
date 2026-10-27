# pip environments reach the index: `pip --target` mount (2026-10-27)

**Status:** scoped 2026-10-27 on branch `d12` (follow-up); implements `tasks/todo.md` pip item
(amendment 1, risk 3). Not yet built.

## Why

Every scip-python job's fetch installs into its own container's site-packages, which vanish
with the container: the index phase always runs without the project's environment, even when
the fetch succeeds. A Requirements job's index today equals D4's fallback (2 packages named
against 12–14 in a one-container control). Poetry installs into a virtualenv under
`/root/.cache/pypoetry` (already a mounted cache); whether scip-python finds it there is
unmeasured.

## P0 findings (measured 2026-10-27, scip-python image, scratch project)

- **pip (`requirements.txt`, `--no-deps`):** `pip install --target /tmp/py-target` then
  `scip-python index` with `PYTHONPATH=/tmp/py-target` joins what the env-less index
  leaves `unresolved`: `requests` resolves to `scip-python python requests 2.31.0 ...`,
  and even the `requests.get(url, timeout=5)` call resolves to `requests.api/get()`
  with its `timeout` parameter (`/tmp/pip-probe/compare.log`, `index-{base,env}.scip`).
  Baseline (no env): `requests` is `[unable to resolve module]`.
- **Poetry:** `poetry install --no-root` puts requests + transitive deps in
  `/root/.cache/pypoetry/virtualenvs/probe-poetry-VsnhxLU2-py3.10`, but the index
  without `PYTHONPATH` still reports `1 unable to resolve module`: the venv is
  invisible too (`/tmp/pip-probe/poetry.log`).
- **Venv naming is reproducible:** poetry 1.6.1 names it
  `<name>-<urlsafe_b64(sha256(container_path))[:8]>-py3.10`; hashing `/src` gives
  `VsnhxLU2`, the observed infix. So the venv path is deterministic per job root —
  but rather than mirroring the hash, P1 uses in-project venvs (deterministic path,
  no name parsing).
- **Python coupling:** the image's interpreter is 3.10 (contract M9d S11), so the
  site-packages path ends `lib/python3.10/site-packages`. If the image ever upgrades
  Python this breaks visibly (imports go unresolved again); e2e covers it.

## Design (decided from P0)

```mermaid
flowchart LR
    FETCH[pip install --target pytarget per root] -->|mounted host dir, survives| ENV[/python-env]
    ENV --> INDEX[scip-python index with PYTHONPATH=/python-env]
    POETRY[poetry install + in-project venv] --> VENV[<export>/.venv]
    VENV --> INDEX2[scip-python index with PYTHONPATH=.venv site-packages]
```

- **Fetch:** `pip install --no-deps --ignore-requires-python --target /python-env ...`
  (Requirements and None lockfiles). `/python-env` is `<cache_dir>/pytarget/<root>`
  (`root` at the repository root), a host dir mounted in fetch and index; per-root so
  different roots' pins never clobber each other. Poetry keeps `poetry install`, plus
  `POETRY_VIRTUALENVS_IN_PROJECT=true` in the fetch env, so the venv lands at
  `<export>/<root>/.venv` — inside the run's own export, deterministic, no name
  parsing — and survives fetch→index (one run shares its export dir).
- **Index:** `PYTHONPATH=/python-env` for Requirements/None;
  `PYTHONPATH=<mount>/.venv/lib/python3.10/site-packages` (root job) or
  `<mount>/<root>/.venv/...` (nested roots) for Poetry, where `<mount>` is the job's
  container mount (`/src`), a pure function of `job.root`.
- **Recipe bump** (`RECIPE_PY` → next): the fetch output (hence the index bytes) changes,
  so old cache rows must not serve. Established: the cache stores raw `index.scip`
  bytes and decode runs fresh, so decode-only changes need no bump — fetch changes do.
- **Fallbacks stay:** a failed fetch still indexes without the env (`OkWithoutEnvironment`);
  the status line keeps its suffix. Nothing changes for non-Python jobs.

## Claims

| # | Claim | Test |
|---|---|---|
| C1 | Requirements/None fetches pass `--target /python-env`; the mount is read-write in fetch, read-only in index | `python_fetches_install_to_a_mounted_target` (orchestrate), red first |
| C2 | Index runs carry `PYTHONPATH` (target dir, or the in-project venv's site-packages for Poetry) | same test's index half, red first |
| C3 | Poetry venv visibility measured before code: one-container control vs job index, packages named | probe log, recorded here before P2 |
| C4 | `RECIPE_PY` bumped; old rows do not serve | cache-key unit (`recipe` pins the new string), red first |
| C5 | Every Python index changes; agreement movement classified per bucket | full bench + `compare` with accept lines; sample redraw judged if the draw moves |
| C6 | No `exact` fp rise possible from env gain alone (references joined to real definitions stay exact or become tp) — verified, not assumed | gate output |

## Phases

- **P0. Probe:** uncommitted scratch: one-container `pip install --target` + scip-python index of a bench-shaped project (fastapi-like pins), vs the job path; Poetry venv visibility. Log kept; C3 recorded from it.
- **P1.** C1, C2, C4 tests first, then the change (`orchestrate.rs`, recipe). Verify: `cargo test -p rb-scip`, fmt, clippy.
- **P2.** E2e subset (Python fixtures) + targeted bench-target probes (`microblog`, `fastapi` as `t_*`, deleted after).
- **P3.** Full bench + `compare` (expect Python-cell movement with accept lines; `exact` sample redraw judged under the B15 protocol if it moves). Records, commit.

## NFRs

- Python fetch wall may grow (full-tree download into the mount); index wall grows with larger indexes. Both reported, not gated, unless a non-Python cell moves (that would be a bug).
- No new `exact` fp: env gain only adds resolutions; any fp rise fails the gate.
