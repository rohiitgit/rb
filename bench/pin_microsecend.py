"""Pins every microSecEnD application to a commit and checks its evidence (contract M8b, D1, T1).

The dataset names no commits: its evidence URLs point at a branch (mostly `master`). For each
application this script
  1. takes the branch its evidence URLs name most often (a 40-hex SHA is not a branch), else the
     repository's default branch from the dataset's own `dataset.json`;
  2. pins the last commit on that branch at or before the date the dataset commit added the
     application's model (`git log --diff-filter=A`);
  3. checks every evidence URL against that pin: the file exists (a doubled `<ref>/` path prefix,
     a known quirk of the dataset, is tried without it), the line exists, and the span's end
     column fits on that line. `heuristic` and `implicit` items are counted, not resolved. A file
     that exists in no commit of any branch of the repository is a dataset error (config-repo
     files filed under the application's URL): counted apart, since no pin could resolve it.
An application is kept when at least 90 % of its URL evidence that is not a dataset error
resolves.

Only `git` runs on the clones (they are data, never built or executed). Clones land in
var/bench/repos/<name>. Prints manifest rows for kept applications and a comment line per
application. Usage: python3 -I bench/pin_microsecend.py > /tmp/rows.tsv
"""
import collections
import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]
DATASET = ROOT / "var" / "bench" / "repos" / "microsecend"
REPOS = ROOT / "var" / "bench" / "repos"
KEEP = 0.90

# Manifest names ([a-z0-9_], short enough for `bench_<name>_noscip` to fit rb's 40 characters).
NAMES = {
    "anilallewar_microservices-basics-spring-boot": "mse_anilallewar",
    "apssouza22_java-microservice": "mse_apssouza22",
    "callistaenterprise_blog-microservices": "mse_callista",
    "ewolff_microservice": "mse_ewolff",
    "ewolff_microservice-kafka": "mse_ewolff_kafka",
    "fernandoabcampos_spring-netflix-oss-microservices": "mse_fernandoabcampos",
    "georgwittberger_apache-spring-boot-microservice-example": "mse_georgwittberger",
    "jferrater_tap-and-eat-microservices": "mse_jferrater",
    "koushikkothagal_spring-boot-microservices-workshop": "mse_koushikkothagal",
    "mdeket_spring-cloud-movie-recommendation": "mse_mdeket",
    "mudigal-technologies_microservices-sample": "mse_mudigal",
    "piomin_sample-spring-oauth2-microservices": "mse_piomin",
    "rohitghatol_spring-boot-microservices": "mse_rohitghatol",
    "shabbirdwd53_springboot-microservice": "mse_shabbirdwd53",
    "spring-petclinic_spring-petclinic-microservices": "mse_petclinic",
    "sqshq_piggymetrics": "mse_piggymetrics",
    "yidongnan_spring-cloud-netflix-example": "mse_yidongnan",
}

URL = re.compile(r"https://github\.com/([^/]+)/([^/]+)/blob/([^/]+)/([^#]+)(?:#L(\d+))?")
SPAN = re.compile(r"\(\s*(\d+)\s*[,:]\s*(\d+)\s*\)")
SHA = re.compile(r"^[0-9a-f]{40}$")


def git(*args, cwd):
    return subprocess.run(["git", *args], cwd=cwd, capture_output=True, text=True, check=True).stdout


def load(path):
    """A dataset JSON file; trailing commas (two files have them) are removed and reported."""
    text = path.read_text()
    try:
        return json.loads(text), False
    except json.JSONDecodeError:
        return json.loads(re.sub(r",(\s*[}\]])", r"\1", text)), True


def evidence(node):
    """Every evidence dict of a traceability item, its sub-items included."""
    yield node
    for sub in (node.get("sub_items") or {}).values():
        if isinstance(sub, dict):
            yield from evidence(sub)


def ever_existed(clone, path):
    return bool(git("log", "--all", "--format=%h", "-1", "--", path, cwd=clone).strip())


def resolve(clone, item):
    """True, False, "dataset_error" (the file is in no commit), or None for evidence that names no
    file (heuristic, implicit)."""
    m = URL.match(str(item.get("file", "")))
    if not m:
        return None
    ref, path = m.group(3), m.group(4)
    line = int(m.group(5) or item.get("line") or 0)
    candidates = [path] + ([path[len(ref) + 1:]] if path.startswith(ref + "/") else [])
    for p in candidates:
        f = clone / p
        if f.is_file():
            lines = f.read_text(errors="replace").splitlines()
            if not 1 <= line <= len(lines):
                return False
            s = SPAN.match(str(item.get("span", "")))
            return s is None or int(s.group(2)) <= len(lines[line - 1]) + 1
    if not any(ever_existed(clone, p) for p in candidates):
        return "dataset_error"
    return False


def main():
    meta = json.loads((DATASET / "dataset" / "dataset.json").read_text())
    dataset_pin = git("rev-parse", "HEAD", cwd=DATASET).strip()
    print(f"# microSecEnD dataset {dataset_pin} (Apache-2.0); pins by D1 (bench/pin_microsecend.py)")
    for app in sorted(NAMES):
        name = NAMES[app]
        slug = meta[app]["slug"]
        url = f"https://github.com/{slug}.git"
        model = DATASET / "dataset" / app
        trace, repaired = load(model / f"{app}_traceability.json")
        added = git("log", "--diff-filter=A", "--format=%cI", "--", f"dataset/{app}/{app}.json", cwd=DATASET).split()[-1]
        items = [e for sec in ("nodes", "edges") for v in trace[sec].values() for e in evidence(v)]
        refs = collections.Counter(
            m.group(3) for e in items if (m := URL.match(str(e.get("file", "")))) and not SHA.match(m.group(3))
        )
        clone = REPOS / name
        if not (clone / ".git").exists():
            subprocess.run(["git", "clone", "-q", "--filter=blob:none", "--no-checkout", url, str(clone)], check=True)
        branches = git("branch", "-r", "--format=%(refname:short)", cwd=clone).split()
        branch = next((r for r, _ in refs.most_common() if f"origin/{r}" in branches), meta[app]["data"]["default_branch"])
        pin = git("rev-list", "-1", f"--before={added}", f"origin/{branch}", cwd=clone).strip()
        git("checkout", "-q", "--detach", pin, cwd=clone)
        verdicts = [resolve(clone, e) for e in items]
        errors = sum(1 for v in verdicts if v == "dataset_error")
        urls = [v for v in verdicts if v is True or v is False]
        ok = sum(urls)
        rate = ok / len(urls) if urls else 0.0
        kept = rate >= KEEP
        note = (
            f"# {name} {slug}: branch {branch}, model added {added[:10]}, pin {pin[:12]}, "
            f"evidence resolved {ok}/{len(urls)} ({100 * rate:.1f} %), "
            f"{errors} dataset errors, {len(verdicts) - len(urls) - errors} heuristic/implicit"
            f"{', JSON repaired' if repaired else ''}: {'kept' if kept else 'EXCLUDED'}"
        )
        print(note)
        if kept:
            print(f"{name}\tmicrosecend\t{url}\t{pin}\tnoscip")
    print(f"microsecend\tdataset\thttps://github.com/tuhh-softsec/microSecEnD.git\t{dataset_pin}\t-")


if __name__ == "__main__":
    sys.exit(main())
