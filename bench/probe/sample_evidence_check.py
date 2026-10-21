"""Checks that each judged scip row's evidence names its own edge (contract M9c pass 2, S1).

A samples file (`bench/samples/exact.tsv`, `bench/samples/synthetic-d8.tsv`) holds, per row:
family, target, pin, rb edge identity, TRUE|FALSE, evidence. For each `scip` row whose identity
is `symbol|<source>|<KIND>|symbol|<target>|...`, the names of an end are its symbol's last
descriptor and the type that holds it (`.../Schedulers#Start().` gives `Start` and `Schedulers`;
`.../mlog`/Error().` gives `Error`). The rule, printed first: a row is OK when its evidence
contains a name of the target, as a whole word, and either a name of the source or (given the
draw's listing with `--drawn`) the file the source is defined in or one of the edge's own site
files; else MISMATCH. A citation checker that only resolves `file:line` cannot see evidence that
belongs to another edge.

The control re-runs the rule with each row's evidence taken from the row before it (the defect
S1 found), and must report MISMATCH rows, or the check proves nothing.

Usage: python3 -I bench/probe/sample_evidence_check.py FILE [FIRST_LINE LAST_LINE]
[--drawn LISTING ...] (1-based file lines, inclusive; default every row; LISTING is a judge's
input, `drawn-*.txt`, with `from: … (defined at PATH:LINE, …)` and `evidence: PATH:LINE, …`
under each edge). Exit 1 when a row is MISMATCH.
"""
import re
import sys

RULE = (
    "rule: a scip row is OK iff its evidence names its target (last descriptor or its type, as a "
    "whole word) and names its source likewise or cites the source's definition file or one of "
    "the edge's site files (from --drawn); the control shifts each row's evidence by one row"
)


def drawn_files(listings):
    """Per identity: the source's definition file and the edge's site files, from the listings."""
    out, ident = {}, None
    for listing in listings:
        for line in open(listing):
            if re.match(r"S\d+\t", line):
                ident = line.rstrip("\n").split("\t")[5]
                out[ident] = set()
            elif ident and line.startswith("  from:"):
                m = re.search(r"\(defined at ([^:]+):\d+", line)
                if m:
                    out[ident].add(m.group(1))
            elif ident and line.startswith("  evidence:"):
                out[ident].update(re.findall(r"([\w./-]+):\d+", line))
    return out


def names(symbol):
    """The descriptor names of a scip symbol's last two descriptors (method and its type)."""
    tail = symbol.split("`")[-1].lstrip("/") if "`" in symbol else symbol.split(" ")[-1]
    parts = [p for p in re.split(r"[#./]", re.sub(r"\([^)]*\)", "", tail)) if p]
    return set(parts[-2:]) if len(parts) > 1 else set(parts)


def ends(identity):
    cells = identity.split("|")
    if len(cells) < 5 or cells[0] != "symbol" or cells[3] != "symbol":
        return None
    return names(cells[1]), names(cells[4])


def ok(evidence, src, dst, files):
    def has(ns):
        return any(re.search(rf"(?<![A-Za-z0-9_]){re.escape(n)}(?![A-Za-z0-9_])", evidence) for n in ns)

    return has(dst) and (has(src) or any(f in evidence for f in files))


def main():
    args = sys.argv[1:]
    listings = args[args.index("--drawn") + 1 :] if "--drawn" in args else []
    args = args[: args.index("--drawn")] if "--drawn" in args else args
    path = args[0]
    first, last = (int(args[1]), int(args[2])) if len(args) > 2 else (1, 10**9)
    sites = drawn_files(listings)
    rows = []
    for n, line in enumerate(open(path), 1):
        if not (first <= n <= last) or not line.strip() or line.startswith("#"):
            continue
        cells = line.rstrip("\n").split("\t")
        if len(cells) < 6 or cells[0] != "scip":
            continue
        e = ends(cells[3])
        if e is not None:
            rows.append((n, e, cells[5], cells[4], sites.get(cells[3], set())))
    print(RULE)
    print(f"listings {len(listings)}, edges listed {len(sites)}, rows listed {sum(1 for r in rows if r[4])}")
    bad = 0
    for n, (src, dst), evidence, verdict, files in rows:
        good = ok(evidence, src, dst, files)
        bad += not good
        if not good:
            print(f"MISMATCH line {n} {verdict} src {sorted(src)} dst {sorted(dst)}: {evidence[:160]!r}")
    print(f"rows {len(rows)}: OK {len(rows) - bad}, MISMATCH {bad}")
    shifted = sum(
        not ok(rows[i - 1][2], rows[i][1][0], rows[i][1][1], rows[i][4]) for i in range(1, len(rows))
    )
    print(f"control (evidence of the row before): MISMATCH {shifted} of {max(len(rows) - 1, 0)}")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
