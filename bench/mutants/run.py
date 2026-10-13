"""Re-runs a mutant battery for crates/rb-bench (contract M8a; usage guide 5.4).

Each spec is one exact text replacement in one file under crates/rb-bench/src: apply it, run
`cargo test -q -p rb-bench`, restore the file byte for byte (checked by sha256), and report KILLED
(the tests failed) or SURVIVED. A spec whose text no longer occurs is reported as STALE (for
example after `cargo fmt` reflows a line), never applied. Usage: python3 -I bench/mutants/run.py
bench/mutants/m8a.json [id ...]
"""
import hashlib
import json
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
SRC = ROOT / "crates" / "rb-bench" / "src"


def main():
    specs = json.loads(pathlib.Path(sys.argv[1]).read_text())
    wanted = set(sys.argv[2:])
    for m in specs:
        if wanted and m["id"] not in wanted:
            continue
        path = SRC / m["file"]
        text = path.read_text()
        if text.count(m["old"]) != 1:
            print(m["id"], "STALE", m["what"], f"({text.count(m['old'])} matches)")
            continue
        digest = hashlib.sha256(text.encode()).hexdigest()
        path.write_text(text.replace(m["old"], m["new"]))
        try:
            run = subprocess.run(
                ["cargo", "test", "-q", "-p", "rb-bench"],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
        finally:
            path.write_text(text)
        assert hashlib.sha256(path.read_text().encode()).hexdigest() == digest
        print(m["id"], "KILLED" if run.returncode else "SURVIVED", m["what"])


if __name__ == "__main__":
    main()
