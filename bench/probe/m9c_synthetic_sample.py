#!/usr/bin/env python3
"""A one-off judged sample of D8' `synthetic` scip CALLS (M9c evaluator pass 1, E5).

`rb-bench sample` draws `exact` edges only (`sample::family` requires certainty `exact`), so
D8''s `synthetic` scip edges, which `scip_ref.rs` keeps in the agreement gold, never enter it.
This draws `--n` of them (default 60) from one target's SCIP dump with a fixed seed, and writes
the judge inputs in the format `rb-bench sample` prints (M8b T5 / M9a protocol):

  OUT/drawn-synth-<k>.txt   the edge blocks of batch k (`E<n>`, kind, ends with their
                            definition sites, evidence sites, 1-based)
  OUT/prompt-<k>.txt        the judge prompt for batch k
  OUT/drawn.tsv             family, target, pin, identity of every drawn edge

Read-only over the dump; writes only under OUT. Run with `python3 -I`.

  python3 -I bench/probe/m9c_synthetic_sample.py DUMP TARGET PIN OUT [--n 60] [--seed S]
         [--batches 3]
"""

import argparse
import json
import os
import random
import sys

SYNTHETIC = 3  # rb_model::Certainty::Synthetic

PROMPT = """You are an adversarial JUDGE. A tool labelled each edge below `synthetic`: it claims each is right, joined by symbol from one Go module's SCIP index to a definition in another module of the same repository. ASSUME EACH EDGE IS FALSE; mark it TRUE only if you can prove it from the source with file:line.

READ only: the edge list {drawn}, and the checkout {checkout} (pinned at the commit named in the edge). Read-only commands (grep, sed, cat, git ls-files, git show) inside that checkout are fine. Do NOT read anything else under {repo}; no rb, rb-bench, cargo, go, node, network.
WRITE exactly one file: {verdicts}

Each edge block: `S<n><TAB><family><TAB><target><TAB><pin><TAB><mode><TAB><identity>` then lines `edge:` (kind, provenance, certainty, attributes), `from:` (with `defined at path:line, name N` when known), `to:` (same), `evidence:` (file:line, 1-based).
What the edges mean (symbol keys are SCIP symbols like `scip-go gomod <module> <version> <pkg>/Type#Method().`; use the `defined at` sites). The version in a key does not matter: a call into the module an import path names counts as a call to that module's definition in this checkout, whichever version `go.mod` requires.
- `CALLS`: the function/method defined at the from-site contains, at the evidence line, a call that resolves (by Go's rules: the import path of the qualifier's package, the receiver's type and its method set, embedding) to the function/method defined at the to-site. A from that is a `file` means top-level code of that file.
- `REFERENCES`: the from-symbol's code at the evidence line refers (by name, type use, value use) to the to-symbol.
- `IMPLEMENTS`: the from-type implements the to-interface. Go: T implements I when the method set of T, or of *T, satisfies I (scip-go relates the type T in both cases).
FALSE if the evidence line holds no such call/reference, if it resolves to a different definition (e.g. a same-named function or method of another package, another module or another type), or if the from-site is not the function enclosing the evidence line. A closure's code counts as its enclosing function's.

OUTPUT {verdicts_name}: one line per edge, tab-separated, EXACTLY:
`<family>\\t<target>\\t<pin>\\t<identity>\\tTRUE|FALSE\\t<evidence: file:line and the decisive text; for FALSE, what is wrong>`
(copy family, target, pin, identity verbatim from the edge header; no header line). Judge all {count} edges. Final message: TRUE/FALSE counts and every FALSE with its reason.
"""


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dump")
    ap.add_argument("target")
    ap.add_argument("pin")
    ap.add_argument("out")
    ap.add_argument("--n", type=int, default=60)
    ap.add_argument("--seed", type=int, default=91017)
    ap.add_argument("--batches", type=int, default=3)
    ap.add_argument("--checkout", default="var/bench/repos/{target}")
    a = ap.parse_args()

    names = {}
    sites = {}
    pool = {}
    with open(a.dump, encoding="utf-8") as f:
        header = json.loads(f.readline())
        if header[0] != "rb-dump" or header[2] != a.pin:
            sys.exit(f"dump header {header} does not name pin {a.pin}")
        for line in f:
            row = json.loads(line)
            if row[0] == "N":
                names[(row[1], row[2])] = row[3]
                continue
            if row[0] != "E":
                continue
            _, sk, skey, kind, dk, dkey, prov, cert, attrs, ev = row
            if kind == "DEFINED_IN" and sk == "symbol":
                for path, span in ev:
                    sites.setdefault(skey, []).append((path, span[0] + 1))
            if kind == "CALLS" and prov == "scip" and cert == SYNTHETIC:
                ident = "|".join([sk, skey, kind, dk, dkey, prov])
                pool[ident] = (sk, skey, kind, dk, dkey, prov, attrs, ev)

    ids = sorted(pool)
    drawn = sorted(random.Random(a.seed).sample(ids, min(a.n, len(ids))))
    os.makedirs(a.out, exist_ok=True)

    def end(kind, key):
        if kind == "symbol" and sites.get(key):
            path, line = min(sites[key])
            return f"{kind} {key} (defined at {path}:{line}, name {names.get((kind, key), '?')})"
        return f"{kind} {key}"

    repo = os.getcwd()
    checkout = os.path.join(repo, a.checkout.format(target=a.target))
    per = -(-len(drawn) // a.batches)
    with open(os.path.join(a.out, "drawn.tsv"), "w", encoding="utf-8") as tsv:
        tsv.write(f"# seed {a.seed}; pool {len(ids)} synthetic scip CALLS of {a.dump}\n")
        for b in range(a.batches):
            batch = drawn[b * per:(b + 1) * per]
            if not batch:
                continue
            drawn_path = os.path.join(a.out, f"drawn-synth-{b + 1}.txt")
            with open(drawn_path, "w", encoding="utf-8") as out:
                for i, ident in enumerate(batch, start=b * per + 1):
                    sk, skey, kind, dk, dkey, prov, attrs, ev = pool[ident]
                    tsv.write(f"scip\t{a.target}\t{a.pin}\t{ident}\n")
                    evidence = ", ".join(f"{p}:{s[0] + 1}" for p, s in ev) or "none"
                    out.write(
                        f"S{i}\tscip\t{a.target}\t{a.pin}\tscip\t{ident}\n"
                        f"  edge: {kind} ({prov}, synthetic) {json.dumps(attrs)}\n"
                        f"  from: {end(sk, skey)}\n"
                        f"  to:   {end(dk, dkey)}\n"
                        f"  evidence: {evidence}\n"
                    )
            verdicts = os.path.join(repo, a.out, f"verdicts-synth-{b + 1}.tsv")
            with open(os.path.join(a.out, f"prompt-{b + 1}.txt"), "w", encoding="utf-8") as p:
                p.write(
                    PROMPT.format(
                        drawn=os.path.join(repo, drawn_path),
                        checkout=checkout,
                        repo=repo,
                        verdicts=verdicts,
                        verdicts_name=os.path.basename(verdicts),
                        count=len(batch),
                    )
                )
    print(f"pool {len(ids)}, drawn {len(drawn)}, batches of {per}, seed {a.seed}")


if __name__ == "__main__":
    main()
