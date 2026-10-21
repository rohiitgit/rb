#!/usr/bin/env python3
"""M9b P0 probe (contract C1, D4, D8): cross-module references in scip-go indexes.

Run with `python3 -I`. Reads raw `index.scip` files with a minimal protobuf reader (no
dependencies), so it sees the index exactly as the indexer wrote it, before rb's decoder.

  scip_cross.py ident INDEX...
      project root, tool, document and occurrence counts of each index.
  scip_cross.py cross --repo CHECKOUT ROOT=INDEX [ROOT=INDEX ...]
      C1: per job, reference occurrences into another job's module path (go.mod of ROOT
      under CHECKOUT), their versions, how many name a descriptor the other job defines, and
      the bucket rb's join gives them as far as the index alone decides it; `external 0`.
  scip_cross.py rel --type SUFFIX [--type SUFFIX ...] ROOT=INDEX
      D8: the symbol information and relationships of every symbol ending in a SUFFIX.

Every count is printed with its denominator; document and occurrence totals come first.
"""
import collections
import os
import sys

# SCIP schema field numbers (scip.proto).
INDEX_METADATA, INDEX_DOCUMENTS, INDEX_EXTERNAL = 1, 2, 3
META_TOOL, META_ROOT = 2, 3
TOOL_NAME, TOOL_VERSION = 1, 2
DOC_PATH, DOC_OCCURRENCES, DOC_SYMBOLS = 1, 2, 3
OCC_RANGE, OCC_SYMBOL, OCC_ROLES = 1, 2, 3
SYM_SYMBOL, SYM_RELATIONSHIPS, SYM_KIND, SYM_DISPLAY = 1, 4, 5, 6
REL_SYMBOL, REL_REF, REL_IMPL, REL_TYPEDEF, REL_DEF = 1, 2, 3, 4, 5
ROLE_DEFINITION = 1


def varint(buf, i):
    shift = result = 0
    while True:
        b = buf[i]
        i += 1
        result |= (b & 0x7F) << shift
        if b < 0x80:
            return result, i
        shift += 7


def fields(buf, start=0, end=None):
    """Yield (field number, wire type, value) of one message; length-delimited values are
    (start, end) offsets into `buf`."""
    i = start
    end = len(buf) if end is None else end
    while i < end:
        key, i = varint(buf, i)
        num, wt = key >> 3, key & 7
        if wt == 0:
            v, i = varint(buf, i)
            yield num, wt, v
        elif wt == 2:
            n, i = varint(buf, i)
            yield num, wt, (i, i + n)
            i += n
        elif wt == 1:
            yield num, wt, buf[i:i + 8]
            i += 8
        elif wt == 5:
            yield num, wt, buf[i:i + 4]
            i += 4
        else:
            raise ValueError(f"wire type {wt} at {i}")


def text(buf, span):
    return bytes(buf[span[0]:span[1]]).decode("utf-8")


def packed(buf, span):
    out, i = [], span[0]
    while i < span[1]:
        v, i = varint(buf, i)
        out.append(v)
    return out


def read_relationship(buf, span):
    r = {"symbol": "", "ref": False, "impl": False, "typedef": False, "def": False}
    for num, wt, v in fields(buf, *span):
        if num == REL_SYMBOL:
            r["symbol"] = text(buf, v)
        elif num == REL_REF:
            r["ref"] = bool(v)
        elif num == REL_IMPL:
            r["impl"] = bool(v)
        elif num == REL_TYPEDEF:
            r["typedef"] = bool(v)
        elif num == REL_DEF:
            r["def"] = bool(v)
    return r


def read_symbol(buf, span):
    s = {"symbol": "", "kind": 0, "display": "", "relationships": []}
    for num, wt, v in fields(buf, *span):
        if num == SYM_SYMBOL:
            s["symbol"] = text(buf, v)
        elif num == SYM_KIND:
            s["kind"] = v
        elif num == SYM_DISPLAY:
            s["display"] = text(buf, v)
        elif num == SYM_RELATIONSHIPS:
            s["relationships"].append(read_relationship(buf, v))
    return s


def read_occurrence(buf, span):
    rng, symbol, roles = [], "", 0
    for num, wt, v in fields(buf, *span):
        if num == OCC_RANGE:
            rng = packed(buf, v) if wt == 2 else rng + [v]
        elif num == OCC_SYMBOL:
            symbol = text(buf, v)
        elif num == OCC_ROLES:
            roles = v
    if len(rng) == 3:
        rng = [rng[0], rng[1], rng[0], rng[2]]
    return tuple(rng), symbol, roles


def read_document(buf, span):
    d = {"path": "", "occurrences": [], "symbols": []}
    for num, wt, v in fields(buf, *span):
        if num == DOC_PATH:
            d["path"] = text(buf, v)
        elif num == DOC_OCCURRENCES:
            d["occurrences"].append(read_occurrence(buf, v))
        elif num == DOC_SYMBOLS:
            d["symbols"].append(read_symbol(buf, v))
    return d


def read_index(path):
    buf = memoryview(open(path, "rb").read())
    idx = {"root": "", "tool": "", "documents": [], "external": []}
    for num, wt, v in fields(buf):
        if num == INDEX_METADATA:
            for n2, _, v2 in fields(buf, *v):
                if n2 == META_ROOT:
                    idx["root"] = text(buf, v2)
                elif n2 == META_TOOL:
                    parts = {}
                    for n3, _, v3 in fields(buf, *v2):
                        if n3 in (TOOL_NAME, TOOL_VERSION):
                            parts[n3] = text(buf, v3)
                    idx["tool"] = f"{parts.get(TOOL_NAME, '')} {parts.get(TOOL_VERSION, '')}"
        elif num == INDEX_DOCUMENTS:
            idx["documents"].append(read_document(buf, v))
        elif num == INDEX_EXTERNAL:
            idx["external"].append(read_symbol(buf, v))
    return idx


def split_symbol(symbol):
    """(scheme, manager, package name, version, descriptors) of a global symbol; `.` is the
    empty value (SCIP grammar), and a doubled space is an escaped space."""
    if symbol.startswith("local "):
        return None
    out, i, cur = [], 0, []
    while len(out) < 4:
        if i >= len(symbol):
            return None
        c = symbol[i]
        if c == " ":
            if symbol[i + 1:i + 2] == " ":
                cur.append(" ")
                i += 2
                continue
            out.append("".join(cur))
            cur = []
            i += 1
            continue
        cur.append(c)
        i += 1
    scheme, manager, name, version = (("" if p == "." else p) for p in out)
    return scheme, manager, name, version, symbol[i:]


def counts(idx):
    occ = sum(len(d["occurrences"]) for d in idx["documents"])
    return len(idx["documents"]), occ


def cmd_ident(paths):
    for p in paths:
        idx = read_index(p)
        docs, occ = counts(idx)
        own_docs = sorted(d["path"] for d in idx["documents"] if not d["path"].startswith(".."))
        # Reference occurrences of global symbols in packages the index defines nothing in,
        # split by descriptor: a namespace (`pkg/`) or a member (anything else).
        defined = defined_symbols(idx)
        own = {split_symbol(x)[1:4] for x in defined}
        kinds, members = collections.Counter(), []
        for d in idx["documents"]:
            for rng, sym, roles in d["occurrences"]:
                if roles & ROLE_DEFINITION or not sym or sym.startswith("local ") or sym in defined:
                    continue
                q = split_symbol(sym)
                if q is None or q[1:4] in own:
                    continue
                kind = "namespace" if q[4].endswith("/") else "member"
                kinds[kind] += 1
                if kind == "member":
                    members.append(f"{d['path']}:{rng[0] + 1} {sym}")
        print(f"{os.path.basename(p)}\ttool={idx['tool']}\tdocuments={docs}\toccurrences={occ}"
              f"\tfirst={own_docs[:2]}\tother-package refs: namespace {kinds['namespace']}, "
              f"member {kinds['member']}")
        for m in members:
            print(f"    member: {m}")


def go_module(checkout, root):
    mod = os.path.join(checkout, root, "go.mod") if root != "." else os.path.join(checkout, "go.mod")
    with open(mod) as f:
        for line in f:
            if line.startswith("module "):
                return line.split()[1].strip()
    return None


def last_name(descriptors):
    """The last descriptor's name, for the token test (rb_scip::may_name's base form)."""
    d = descriptors.rstrip("#./:!")
    if d.endswith(")"):
        d = d[:d.rfind("(")].rstrip(".")
    if d.endswith("]"):
        d = d[:d.rfind("[")]
    for sep in ("#", ".", "/", ":", "!"):
        k = d.rfind(sep)
        if k >= 0:
            d = d[k + 1:]
    return d.strip("`")


def token_at(lines, rng):
    if rng[0] != rng[2] or rng[0] >= len(lines):
        return None
    line = lines[rng[0]]
    # scip-go columns are UTF-8 byte offsets of the line.
    return line[rng[1]:rng[3]].decode("utf-8", "replace")


def load(jobs):
    out = []
    for spec in jobs:
        root, path = spec.split("=", 1)
        idx = read_index(path)
        docs, occ = counts(idx)
        print(f"job {root}: {os.path.basename(path)} {idx['tool']}, documents {docs}, occurrences {occ}")
        if docs == 0 or occ == 0:
            sys.exit(f"empty index for {root}")
        out.append((root, idx))
    return out


def defined_symbols(idx):
    return {s for d in idx["documents"] for (_, s, roles) in d["occurrences"]
            if roles & ROLE_DEFINITION and not s.startswith("local ")}


def cmd_cross(argv):
    checkout = argv[argv.index("--repo") + 1]
    jobs = load([a for a in argv if "=" in a])
    modules = {root: go_module(checkout, root) for root, _ in jobs}
    for root, m in modules.items():
        print(f"module {root}: {m}")
    defined = {root: defined_symbols(idx) for root, idx in jobs}
    # Each job's own (manager, package name, version) set, as rb_scip::package reads it from
    # the defined symbols (the join's `own_packages`).
    own = {root: collections.Counter(split_symbol(s)[1:4] for s in defined[root]) for root in defined}
    # Each job's defined descriptors keyed version-free, for the cross-version question.
    by_desc = {root: {(p[1], p[2], p[4]) for p in map(split_symbol, defined[root])} for root in defined}
    for root, c in own.items():
        print(f"own packages {root}: {len(c)}: " + "; ".join(
            f"{m} {n} {v or '.'} ({k})" for (m, n, v), k in c.most_common(5)))
    print()
    for root, idx in jobs:
        refs = 0
        into = collections.Counter()   # other root -> occurrences
        versions = collections.Counter()  # (other root, version) -> occurrences
        named_defined = collections.Counter()  # other root -> occurrences naming a defined descriptor
        token_ok = collections.Counter()
        bucket = collections.Counter()  # where the join puts the ones that name the token
        all_buckets = collections.Counter()  # every global reference occurrence of this job
        pkg_versions = collections.Counter()
        kinds = collections.Counter()  # (other root, namespace|member) -> occurrences
        ext_kinds = collections.Counter()  # namespace|member over every external occurrence
        member_sites = collections.Counter()  # other root -> `pkg.Name` sites after a qualifier
        member_with_occ = collections.Counter()  # ... with any occurrence starting at `Name`
        samples = collections.defaultdict(list)
        own_set = set(own[root])
        for d in idx["documents"]:
            src = os.path.join(checkout, root, d["path"]) if root != "." else os.path.join(checkout, d["path"])
            try:
                lines = open(src, "rb").read().split(b"\n")
            except OSError:
                lines = []
            starts = {(r[0], r[1]) for (r, _, _) in d["occurrences"]}
            for rng, sym, roles in d["occurrences"]:
                if roles & ROLE_DEFINITION or sym.startswith("local ") or not sym:
                    continue
                refs += 1
                parts = split_symbol(sym)
                if parts is None:
                    all_buckets["unparsed"] += 1
                    continue
                _, mgr, name, ver, desc = parts
                if sym in defined[root]:
                    all_buckets["defined (matched or unanchored)"] += 1
                elif (mgr, name, ver) in own_set:
                    all_buckets["undocumented (own package)"] += 1
                else:
                    all_buckets["external (other package)"] += 1
                    pkg_versions[(name, ver)] += 1
                    ext_kinds["namespace" if desc.endswith("/") else "member"] += 1
                for other, m in modules.items():
                    if other == root or not m or name != m:
                        continue
                    into[other] += 1
                    versions[(other, ver or ".")] += 1
                    kinds[(other, "namespace" if desc.endswith("/") else "member")] += 1
                    hit = (mgr, name, desc) in by_desc[other]
                    if hit:
                        named_defined[other] += 1
                    tok = token_at(lines, rng)
                    if tok is not None and tok == last_name(desc):
                        token_ok[other] += 1
                        # A qualifier `pkg` followed by `.Name`: the member token, where a
                        # Tree-sitter call or type reference sits. Is there any occurrence there?
                        line = lines[rng[0]]
                        j = rng[3]
                        if line[j:j + 1] == b".":
                            k = j + 1
                            while k < len(line) and (line[k:k + 1].isalnum() or line[k:k + 1] == b"_"):
                                k += 1
                            if k > j + 1:
                                member_sites[other] += 1
                                if (rng[0], j + 1) in starts:
                                    member_with_occ[other] += 1
                        if sym in defined[root]:
                            bucket[(other, "defined")] += 1
                        elif (mgr, name, ver) in own_set:
                            bucket[(other, "undocumented")] += 1
                        else:
                            bucket[(other, "external")] += 1
                    if hit and len(samples[other]) < 3:
                        samples[other].append(f"{d['path']}:{rng[0] + 1} {tok!r} {sym}")
        print(f"== job {root}: {refs} global reference occurrences")
        for k, v in sorted(all_buckets.items()):
            print(f"   all refs by package test: {k}: {v}")
        print(f"   external (other package) by descriptor: namespace {ext_kinds['namespace']}, "
              f"member {ext_kinds['member']}")
        top = pkg_versions.most_common(8)
        print("   top other packages: " + "; ".join(f"{n} {v or '.'} ({k})" for (n, v), k in top))
        for other in sorted(into):
            print(f"   into {other} ({modules[other]}): {into[other]} of {refs}")
            for (o, v), k in sorted(versions.items()):
                if o == other:
                    print(f"     version {v}: {k}")
            print(f"     by descriptor: namespace {kinds[(other, 'namespace')]}, "
                  f"member {kinds[(other, 'member')]}")
            print(f"     naming a descriptor {other} defines (version ignored): {named_defined[other]} of {into[other]}")
            print(f"     token at the range equals the last descriptor name: {token_ok[other]} of {into[other]}")
            for b in ("defined", "undocumented", "external"):
                print(f"     join's package test on those: {b} {bucket[(other, b)]}")
            print(f"     qualifier sites `pkg.Name` (the member token a Tree-sitter reference names): "
                  f"{member_sites[other]}; with any occurrence starting at `Name`: "
                  f"{member_with_occ[other]}")
            for s in samples[other]:
                print(f"     sample: {s}")
        print()


def cmd_rel(argv):
    subs = [argv[i + 1] for i, a in enumerate(argv) if a == "--type"]
    (root, idx), = load([a for a in argv if "=" in a])
    for d in idx["documents"]:
        for s in d["symbols"]:
            if any(s["symbol"].endswith(sub) for sub in subs):
                rels = s["relationships"]
                print(f"{d['path']}\t{s['symbol']}\tkind={s['kind']}\trelationships={len(rels)}")
                for r in rels:
                    flags = ",".join(k for k in ("ref", "impl", "typedef", "def") if r[k])
                    print(f"    -> {r['symbol']} [{flags}]")


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    cmd, argv = sys.argv[1], sys.argv[2:]
    if cmd == "ident":
        cmd_ident(argv)
    elif cmd == "cross":
        cmd_cross(argv)
    elif cmd == "rel":
        cmd_rel(argv)
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
