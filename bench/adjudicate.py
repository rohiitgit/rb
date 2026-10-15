"""Turns an adjudicator's verdicts into `adjudication.md` entries (contract M8b, T4).

Inputs, for one target:
  bench/keys/<target>/key.txt                 the labeller's key (never edited)
  var/bench/labels/<target>/statements.txt    `S<n>. <statement>` (rb-bench statements)
  var/bench/labels/<target>/verdicts.md       `S<n>: TRUE|FALSE | <completed line or -> | <evidence>`

The verdicts are replayed against the key in statement order. Each verdict applies its
statement, or its completed line when it gave one, to the current key:
  - TRUE adds what the key lacks;
  - FALSE removes what the key holds;
  - a single-op QUERIES statement adds or removes that op on the (function, table) line.
Every change becomes one `- ADD` or `- REMOVE` entry citing the statement and the evidence.
Prints the entries; with --write appends them under a dated heading to
bench/keys/<target>/adjudication.md. Usage:
  python3 -I bench/adjudicate.py <target> [--write]

`ENDPOINT <M> <path> *` states only that the endpoint exists. A TRUE verdict adds the
adjudicator's completed line when the key has no line for that endpoint; a FALSE verdict removes
every key line for it.

With --microsecend the key is the model's endpoint list
(var/bench/labels/<target>/gold.txt, `ENDPOINT ANY <path> @<service>`, from `rb-bench review`),
and the entries go to bench/keys/microsecend/<target>.md, which the scorer applies to the model.
"""
import datetime
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]


def norm_path(p):
    p = re.split(r"[?#]", p)[0]
    segs = []
    for s in p.split("/"):
        if not s:
            continue
        param = (s.startswith("{") and s.endswith("}")) or s[0] in ":*" or (s.startswith("<") and s.endswith(">"))
        segs.append("{}" if param else s)
    return "/" + "/".join(segs)


def parse(line):
    """A key line as a comparable fact plus its ops (QUERIES)."""
    w = line.split()
    if w[0] == "ENDPOINT" and w[3].startswith("@"):
        return ("ENDPOINT", w[1].upper(), norm_path(w[2]), w[3]), None
    if w[0] == "ENDPOINT":
        method, path, at = w[1].upper(), norm_path(w[2]), w[3]
        return ("ENDPOINT", method, path, at), None
    if w[0] == "QUERIES":
        table, ops, at = w[1].lower(), w[2], w[3]
        return ("QUERIES", table, at), (None if ops == "*" else ops.split(","))
    if w[0] == "MAPS":
        return ("MAPS", w[1].lower(), w[2]), None
    raise ValueError(line)


def text(fact, ops):
    if fact[0] == "ENDPOINT":
        return f"ENDPOINT {fact[1]} {fact[2]} {fact[3]}"
    if fact[0] == "QUERIES":
        return f"QUERIES {fact[1]} {','.join(ops)} {fact[2]}"
    return f"MAPS {fact[1]} {fact[2]}"


def main():
    target = sys.argv[1]
    microsecend = "--microsecend" in sys.argv
    keydir = ROOT / "bench" / "keys" / target
    labels = ROOT / "var" / "bench" / "labels" / target
    keyfile = labels / "gold.txt" if microsecend else keydir / "key.txt"
    outfile = (ROOT / "bench" / "keys" / "microsecend" / f"{target}.md") if microsecend else keydir / "adjudication.md"
    # Current key: fact -> (line text, ops). ENDPOINT/MAPS facts carry no ops.
    key = {}
    for raw in keyfile.read_text().splitlines():
        raw = raw.strip()
        if raw and not raw.startswith("#"):
            fact, ops = parse(raw)
            key[fact] = (raw, ops)
    statements = {}
    for line in (labels / "statements.txt").read_text().splitlines():
        n, s = line.split(". ", 1)
        statements[n] = s
    entries = []

    def remove(fact, why):
        raw, _ = key.pop(fact)
        entries.append(f"- REMOVE `{raw}` — {why}")

    def add(fact, ops, why):
        line = text(fact, ops)
        key[fact] = (line, ops)
        entries.append(f"- ADD `{line}` — {why}")

    def same_endpoint(fact):
        return [f for f in key if f[0] == "ENDPOINT" and f[1:3] == fact[1:3]]

    def assert_true(line, why, op_only):
        """The fact of `line` holds (with these ops, or this one op when op_only)."""
        fact, ops = parse(line)
        if fact[0] == "ENDPOINT" and fact[3] == "*":
            return  # exists: a completed line, when the key lacks one, is asserted by the caller
        if fact[0] != "QUERIES":
            if fact not in key:
                add(fact, None, why)
            return
        if ops is None:
            return  # `*` without a completion states nothing about ops
        if fact in key:
            have = key[fact][1]
            want = sorted(set(have) | set(ops)) if op_only else sorted(ops) if not op_only else have
            if sorted(have) != want:
                remove(fact, why)
                add(fact, want, why)
        else:
            add(fact, sorted(ops), why)

    def assert_false(line, why, op_only):
        fact, ops = parse(line)
        if fact[0] == "ENDPOINT" and fact[3] == "*":
            for f in same_endpoint(fact):
                remove(f, why)
            return
        if fact not in key:
            return
        if fact[0] == "QUERIES" and op_only:
            have = key[fact][1]
            left = [o for o in have if o not in ops]
            if left != have:
                remove(fact, why)
                if left:
                    add(fact, left, why)
            return
        remove(fact, why)

    verdict = re.compile(r"^(S\d+): (TRUE|FALSE) \| (.*?) \| (.*)$")
    for line in (labels / "verdicts.md").read_text().splitlines():
        m = verdict.match(line.strip())
        if not m:
            continue
        n, v, completed, evidence = m.groups()
        st = statements[n]
        why = f"{n} {v}: {evidence.strip()}"
        single_op = st.startswith("QUERIES ") and st.split()[2] != "*"
        exists_only = st.startswith("ENDPOINT ") and st.split()[3] == "*"
        if v == "TRUE" and exists_only:
            if not same_endpoint(parse(st)[0]) and completed != "-":
                assert_true(completed, why, False)
        elif v == "TRUE":
            assert_true(completed if completed != "-" else st, why, single_op and completed == "-")
        else:
            assert_false(st, why, single_op)
            if completed != "-":
                assert_true(completed, why, False)
    out = "\n".join(entries)
    print(out)
    if "--write" in sys.argv:
        path = outfile
        path.parent.mkdir(parents=True, exist_ok=True)
        kind = "the microSecEnD model's endpoints" if microsecend else "the hand key"
        head = "" if path.exists() else (
            f"# Adjudication of {target}: {kind} (contract M8b, T2/T4)\n\n"
            "Append-only. Each entry comes from a blind adjudicator's verdict on a statement\n"
            f"(`var/bench/labels/{target}/statements.txt`, `verdicts.md`), replayed by\n"
            "`bench/adjudicate.py`. The key itself is never edited.\n"
        )
        with path.open("a") as f:
            f.write(head + f"\n## {datetime.date.today().isoformat()}: first adjudication\n\n" + out + "\n")


if __name__ == "__main__":
    main()
