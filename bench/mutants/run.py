"""Re-runs a mutant battery (contracts M8a, M9a; usage guide 5.4).

Each spec is one exact text replacement in one file under crates/<crate>/src (`crate` defaults
to rb-bench): apply it, check that the mutation landed, run the tests, restore the file byte for
byte (checked by sha256). rb-bench runs `cargo test -q -p rb-bench`; any other crate runs
`cargo test -q -p <crate> --lib [test]`, where `test` is the spec's optional filter. The verdict
is KILLED only when a test failed (`test result: FAILED` or `panicked at` in the output),
SURVIVED when the tests passed, and INVALID otherwise (a build error, or a mutation that did not
land): an uncompilable mutant gives no verdict. A spec whose text no longer occurs exactly once
is reported as STALE (for example after `cargo fmt` reflows a line), never applied. Usage:
python3 -I bench/mutants/run.py bench/mutants/m8a.json [id ...] [--jobs N]

`--jobs N` runs specs in parallel, each in its own detached git worktree with its own
persistent target dir (`/tmp/rbmut-<uid>-<i>`, `/tmp/rbmut-target-<i>`). The main tree is
never touched, so linting can run concurrently (lesson: never lint while the battery holds
the tree). Uncommitted changes to tracked files under `crates/` ride along as a patch;
untracked files under `crates/` are copied. Anything else the tests need from an uncommitted
tree is out of scope: commit first.

A full run with every spec KILLED is cached in `var/bench/mutants-cache.json`, keyed by the
sha256 of `crates/<crate>/src` for every crate the specs name and of the specs file. A later full
run with identical hashes reports the cached verdicts without running anything: same code plus
same specs is the same outcome. Touching any source file or spec invalidates the cache.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
import pathlib
import shutil
import signal
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
CACHE = ROOT / "var" / "bench" / "mutants-cache.json"


def _term(signum, _frame):
    # A plain SIGTERM (tool timeouts, `kill`) would otherwise skip the
    # try/finally that restores the mutated file and leave the tree dirty.
    # Raising here routes through finally, so the current file is restored.
    raise SystemExit(128 + signum)


signal.signal(signal.SIGTERM, _term)


def _run(cmd, cwd, env=None):
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, env=env)


def crate_of(spec):
    return spec.get("crate", "rb-bench")


def tree_hash(crates):
    h = hashlib.sha256()
    for crate in sorted(crates):
        src = ROOT / "crates" / crate / "src"
        for p in sorted(q for q in src.rglob("*") if q.is_file()):
            h.update(str(p.relative_to(ROOT)).encode())
            h.update(p.read_bytes())
    return h.hexdigest()


def specs_hash(specs_path):
    return hashlib.sha256(pathlib.Path(specs_path).read_bytes()).hexdigest()


def check(specs_path, crates, wanted):
    """A full cached all-KILLED run for these exact inputs, or None."""
    if wanted or not CACHE.exists():
        return None
    try:
        cache = json.loads(CACHE.read_text())
    except (OSError, ValueError):
        return None
    if (
        cache.get("specs_file") == os.path.basename(specs_path)
        and cache.get("tree") == tree_hash(crates)
        and cache.get("specs") == specs_hash(specs_path)
        and cache.get("results")
        and all(r[1] == "KILLED" for r in cache["results"])
    ):
        return cache["results"]
    return None


def save(specs_path, crates, results):
    CACHE.parent.mkdir(parents=True, exist_ok=True)
    CACHE.write_text(
        json.dumps(
            {
                "specs_file": os.path.basename(specs_path),
                "tree": tree_hash(crates),
                "specs": specs_hash(specs_path),
                "results": results,
            },
            indent=1,
        )
    )


def one(workdir, targetdir, spec):
    """Apply one spec in workdir, test, restore. Returns (id, verdict, what)."""
    crate = crate_of(spec)
    path = workdir / "crates" / crate / "src" / spec["file"]
    text = path.read_text()
    if text.count(spec["old"]) != 1:
        return (spec["id"], "STALE", f"{spec['what']} ({text.count(spec['old'])} matches)")
    digest = hashlib.sha256(text.encode()).hexdigest()
    cmd = ["cargo", "test", "-q", "-p", crate]
    if crate != "rb-bench":
        cmd += ["--lib"] + ([spec["test"]] if spec.get("test") else [])
    path.write_text(text.replace(spec["old"], spec["new"]))
    try:
        mutated = path.read_text()
        if mutated == text or spec["new"] not in mutated:
            return (spec["id"], "INVALID", f"{spec['what']} (mutation did not land)")
        env = dict(os.environ, CARGO_TARGET_DIR=str(targetdir))
        run = _run(cmd, cwd=workdir, env=env)
    finally:
        path.write_text(text)
    assert hashlib.sha256(path.read_text().encode()).hexdigest() == digest
    out = run.stdout + run.stderr
    if run.returncode == 0:
        return (spec["id"], "SURVIVED", spec["what"])
    if "test result: FAILED" in out or "panicked at" in out:
        return (spec["id"], "KILLED", spec["what"])
    return (spec["id"], "INVALID", f"{spec['what']} (no failing test: build error?)")


def worktree(uid, i):
    """A detached worktree at HEAD carrying the working tree's crates/ state."""
    wt = pathlib.Path(f"/tmp/rbmut-{uid}-{i}")
    if wt.exists():
        _run(["git", "worktree", "remove", "--force", str(wt)], cwd=ROOT)
        shutil.rmtree(wt, ignore_errors=True)
    r = _run(["git", "worktree", "add", "--detach", str(wt), "HEAD"], cwd=ROOT)
    if r.returncode:
        raise SystemExit(f"git worktree add failed: {r.stderr.strip()}")
    diff = _run(["git", "diff", "HEAD", "--", "crates"], cwd=ROOT)
    if diff.stdout.strip():
        proc = subprocess.run(
            ["git", "apply", "-"], cwd=wt, input=diff.stdout, capture_output=True, text=True
        )
        if proc.returncode:
            raise SystemExit(f"cannot carry working-tree diff: {proc.stderr.strip()}")
    todo = _run(["git", "status", "--porcelain", "--", "crates"], cwd=ROOT).stdout.splitlines()
    for line in todo:
        if line.startswith("??"):
            src = ROOT / line[3:].strip()
            dst = wt / line[3:].strip()
            if src.is_file():
                dst.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(src, dst)
    return wt


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("specs")
    ap.add_argument("ids", nargs="*")
    ap.add_argument("--jobs", type=int, default=1)
    args = ap.parse_args()
    specs = json.loads(pathlib.Path(args.specs).read_text())
    wanted = set(i for i in args.ids if not i.startswith("-"))
    todo = [m for m in specs if not wanted or m["id"] in wanted]
    crates = {crate_of(m) for m in specs}

    if args.jobs < 2:
        if not wanted:
            cached = check(args.specs, crates, wanted)
            if cached is not None:
                for _id, verdict, what in cached:
                    print(_id, verdict, what)
                print(f"cached: tree and specs unchanged ({len(cached)} verdicts)")
                return
        results = [one(ROOT, ROOT / "target", m) for m in todo]
        for _id, verdict, what in results:
            print(_id, verdict, what)
        if not wanted:
            save(args.specs, crates, results)
        return

    uid = os.getpid()
    wts = [worktree(uid, i) for i in range(args.jobs)]
    try:
        groups = [todo[i :: args.jobs] for i in range(args.jobs)]
        results = []
        # One thread per worktree, its specs in sequence: two specs in one worktree at once
        # would mutate and restore each other's file.
        with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as ex:
            futs = []
            for i, group in enumerate(groups):
                td = pathlib.Path(f"/tmp/rbmut-target-{i}")
                td.mkdir(parents=True, exist_ok=True)
                futs.append(ex.submit(lambda wt, td, g: [one(wt, td, m) for m in g], wts[i], td, group))
            for f in concurrent.futures.as_completed(futs):
                results.extend(f.result())
        order = {m["id"]: n for n, m in enumerate(todo)}
        results.sort(key=lambda r: order[r[0]])
        for _id, verdict, what in results:
            print(_id, verdict, what)
        if not wanted:
            save(args.specs, crates, results)
    finally:
        for wt in wts:
            _run(["git", "worktree", "remove", "--force", str(wt)], cwd=ROOT)


if __name__ == "__main__":
    main()
