#!/usr/bin/env python3
"""M9c P0 probe (contract C1): forms, B1 and B13b counts on bench runs of record.

Run with `python3 -I` from the repository root. Reads only: the run's dumps, the pinned
checkouts, the committed scoreboards and sample verdicts, and the raw scip-go indexes that
M9b P0 exported (`var/m9b/idx/`). It writes nothing.

  m9c_forms.py --record LABEL=DIR[,SCOREBOARD] [--record ...] [--idx DIR] [--repos DIR]

Each record is probed in the order given (M9a first, so the contract's counts are reproduced
before M9b's are printed). Sections per record:

  base       every dump the manifest names exists and is non-empty, and the PyCG cases
  agreement  `scip` calls agreement by location identity (the planner's agree.py, mirroring
             rb-bench's scip_ref.rs), checked cell by cell against the record's scoreboard
  ts         bookcars misses by how the callee's name reaches the call (ts_classify.py)
  go         Go misses by the forms between call and callee (go_forms.py), with the upper
             bounds per form the contract quotes
  b1         mattermost agreement fp split by form, at the call token's span (contract D8)
  d7         the D7 rule over the raw scip-go indexes, against the dump's IMPLEMENTS rows

Then, once (the indexes are the same for both records; M9b's run reused M9a's cached rows):

  scipgo     scip-go's type check in rb's job image: locals typed `invalid type`, selector-keyed
             map entries whose value lost its occurrences, and the 3 `App#GetClusterId` sites
  checks     the contract's M9a numbers, each marked reproduced or DIFFERS

Every count is printed with its denominator.
"""
import argparse
import collections
import json
import os
import random
import re
import sys

sys.dont_write_bytecode = True  # read-only: no __pycache__ next to the probe
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import scip_cross as s  # noqa: E402

# The contract's M9a numbers (tasks/contract-m9c.md, "Root causes" and "Why").
CONTRACT_M9A = {
    "bookcars fn": 694,
    "bookcars ns@/": 609,
    "bookcars ns@/ admin": 370,
    "bookcars ns@/ frontend": 239,
    "bookcars bare @/": 36,
    "bookcars bare @/ named": 34,
    "bookcars bare @/ default": 2,
    "bookcars bare default relative": 39,
    "bookcars other": 10,
    "mattermost Go misses": 17065,
    "mattermost TS/JS misses": 92,
    "go crossfile": 8687,
    "go B9-only blocker": 4483,
    "go ctor": 4657,
    "go callchain": 2991,
    "go alias": 3307,
    "go embed": 1313,
    "go pkgvar": 894,
    "go bound ctor": 4023,
    "go bound callchain": 2171,
    "go bound alias": 3127,
    "go bound embed": 1313,
    "go bound pkgvar": 109,
    "go bound pkgvar+unplaced": 889,
    "go bound all five": 11562,
    "go bound ctor+callchain": 6828,
    "b1 fp": 6147,
    "b1 pkg.f()": 5554,
    "b1 value method": 548,
    # The contract says "34 are bare calls in docs/site scripts": the probe counts bare calls
    # anywhere (key "b1 bare calls") and prints the docs/site part beside it.
    "b1 bare calls": 34,
    "b1 other": 11,
    "d7 verified": 431,
    "d7 unverified": 291,
    "d7 unverified suite": 276,
    "d7 outside": 7,
}

# ------------------------------------------------------------------------------------------
# Reading


def load_dump(path):
    nodes, edges = {}, []
    with open(path) as f:
        next(f)
        for line in f:
            r = json.loads(line)
            if r[0] == "N":
                nodes[(r[1], r[2])] = r[3]
            elif r[0] == "E":
                edges.append(r)
    return nodes, edges


class Source:
    """Lines of checkout files, as text (for the line classifiers) and bytes (for spans)."""

    def __init__(self, root):
        self.root = root
        self.text_cache, self.byte_cache = {}, {}

    def lines(self, path):
        if path not in self.text_cache:
            try:
                with open(os.path.join(self.root, path), encoding="utf-8", errors="replace") as f:
                    self.text_cache[path] = f.read().split("\n")
            except OSError:
                self.text_cache[path] = []
        return self.text_cache[path]

    def blines(self, path):
        if path not in self.byte_cache:
            try:
                with open(os.path.join(self.root, path), "rb") as f:
                    self.byte_cache[path] = f.read().split(b"\n")
            except OSError:
                self.byte_cache[path] = []
        return self.byte_cache[path]


def read_index_full(path):
    """Documents with occurrences and symbols, the symbols with their signature text
    (SymbolInformation field 7, a Document whose field 5 is the text)."""
    buf = memoryview(open(path, "rb").read())
    idx = {"path": path, "tool": "", "documents": []}
    for num, _, v in s.fields(buf):
        if num == s.INDEX_METADATA:
            for n2, _, v2 in s.fields(buf, *v):
                if n2 == s.META_TOOL:
                    parts = {}
                    for n3, _, v3 in s.fields(buf, *v2):
                        if n3 in (s.TOOL_NAME, s.TOOL_VERSION):
                            parts[n3] = s.text(buf, v3)
                    idx["tool"] = f"{parts.get(s.TOOL_NAME, '')} {parts.get(s.TOOL_VERSION, '')}"
        elif num == s.INDEX_DOCUMENTS:
            d = {"path": "", "occurrences": [], "symbols": []}
            for n2, _, v2 in s.fields(buf, *v):
                if n2 == s.DOC_PATH:
                    d["path"] = s.text(buf, v2)
                elif n2 == s.DOC_OCCURRENCES:
                    d["occurrences"].append(s.read_occurrence(buf, v2))
                elif n2 == s.DOC_SYMBOLS:
                    sym = {"symbol": "", "kind": 0, "relationships": [], "sig": ""}
                    for n3, _, v3 in s.fields(buf, *v2):
                        if n3 == s.SYM_SYMBOL:
                            sym["symbol"] = s.text(buf, v3)
                        elif n3 == s.SYM_KIND:
                            sym["kind"] = v3
                        elif n3 == s.SYM_RELATIONSHIPS:
                            sym["relationships"].append(s.read_relationship(buf, v3))
                        elif n3 == 7:
                            for n4, _, v4 in s.fields(buf, *v3):
                                if n4 == 5:
                                    sym["sig"] = s.text(buf, v4)
                    d["symbols"].append(sym)
            idx["documents"].append(d)
    return idx


def index_module(idx):
    """The module an index defines: the package name of its own definitions (version `.`)."""
    c = collections.Counter()
    for d in idx["documents"]:
        for _, sym, roles in d["occurrences"]:
            if roles & s.ROLE_DEFINITION and not sym.startswith("local "):
                p = s.split_symbol(sym)
                if p and p[3] == "":
                    c[p[2]] += 1
    return c.most_common(1)[0][0] if c else None


def ok_jobs(record, target, tool):
    roots = []
    path = os.path.join(record, target + ".scip.status")
    if not os.path.exists(path):
        return roots
    for line in open(path):
        m = re.match(r"\s*scip: (\S+) (scip-\S+) ok\b", line)
        if m and m.group(2) == tool:
            roots.append(m.group(1))
    return roots


def owner(roots, path):
    best = None
    for r in roots:
        if r == "." or path == r or path.startswith(r + "/"):
            if best is None or best == "." or len(r) > len(best):
                best = r
    return best


# ------------------------------------------------------------------------------------------
# base


def section_base(record, manifest, repos):
    print("## base: dumps of the record")
    want, missing, empty = [], [], []
    for line in open(manifest):
        if line.startswith("#") or not line.strip():
            continue
        f = line.rstrip("\n").split("\t")
        name, kind, modes = f[0], f[1], f[4]
        if kind in ("stress", "microsecend", "handkey") and modes != "-":
            for mode in modes.split(","):
                want.append(f"{name}.{mode}.jsonl")
    for w in want:
        p = os.path.join(record, w)
        if not os.path.exists(p):
            missing.append(w)
        elif os.path.getsize(p) == 0:
            empty.append(w)
    cases = []
    snippets = os.path.join(repos, "pycg", "micro-benchmark", "snippets")
    for dp, _, fn in os.walk(snippets):
        if "callgraph.json" in fn:
            rel = os.path.relpath(dp, snippets).split(os.sep)
            cases.append("__".join(rel) + ".jsonl")
    pmiss = [c for c in cases if not os.path.exists(os.path.join(record, "pycg", c))]
    pempty = [c for c in cases if c not in pmiss and os.path.getsize(os.path.join(record, "pycg", c)) == 0]
    n_noscip = sum(1 for w in want if w.endswith(".noscip.jsonl"))
    print(f"manifest dumps {len(want)} (noscip {n_noscip}, scip {len(want) - n_noscip}): "
          f"missing {len(missing)}, empty {len(empty)} {missing + empty}")
    print(f"pycg cases {len(cases)}: missing {len(pmiss)}, empty {len(pempty)} {pmiss + pempty}")
    ok = not (missing or empty or pmiss or pempty) and len(want) > 0 and len(cases) > 0
    print("base:", "all present and non-empty" if ok else "INCOMPLETE")
    return ok


# ------------------------------------------------------------------------------------------
# agreement (the planner's agree.py, as functions)


def locator(nodes, edges):
    sites = collections.defaultdict(list)
    for e in edges:
        if e[3] == "DEFINED_IN" and e[1] == "symbol":
            for ev in e[9]:
                sites[e[2]].append((ev[0], ev[1][0] + 1))
    allm = {}
    for (k, key), name in nodes.items():
        if k != "symbol":
            continue
        locs = sorted(set((p, l, name) for p, l in sites.get(key, [])))
        allm[key] = locs if locs else None
    holders = collections.Counter()
    for locs in allm.values():
        if locs:
            for l in locs:
                holders[l] += 1
    sym, aliases = {}, {}
    for key, locs in allm.items():
        if not locs or any(holders[l] > 1 for l in locs):
            sym[key] = None
            continue
        for a in locs[1:]:
            aliases[a] = locs[0]
        sym[key] = locs[0]
    return sym, aliases


def agreement(record, target, src, cls="calls"):
    """gold, tp, fp, fn by (source location, target location), both ends in covered files.
    Rows: (src_path, src_name, dst_path, dst_line, dst_name, site_path, site_line, text, span)."""
    sn, se = load_dump(os.path.join(record, target + ".scip.jsonl"))
    nn, ne = load_dump(os.path.join(record, target + ".noscip.jsonl"))
    covered = set()
    for e in se:
        if e[3] == "DEFINED_IN" and e[1] == "symbol" and e[2].startswith("scip-"):
            covered.add(e[5])
    ssym, aliases = locator(sn, se)
    nsym, _ = locator(nn, ne)

    def loc(table, kind, key):
        if kind == "symbol":
            return table.get(key)
        if kind == "file":
            return (key, 0, "<file>")
        return None

    def sel(e):
        return e[3] == "CALLS" and ((e[1] != "file") if cls == "calls" else (e[1] == "file"))

    gold, found = {}, {}
    for e in se:
        if sel(e) and e[6] == "scip":
            a, b = loc(ssym, e[1], e[2]), loc(ssym, e[4], e[5])
            if a and b:
                gold.setdefault((a, b), []).extend(e[9])
    for e in ne:
        if sel(e):
            a, b = loc(nsym, e[1], e[2]), loc(nsym, e[4], e[5])
            if a and b:
                a, b = aliases.get(a, a), aliases.get(b, b)
                found.setdefault((a, b), []).append((e[6], e[7], e[9]))

    def inn(k):
        return k[0][0] in covered and k[1][0] in covered

    g = {k: v for k, v in gold.items() if inn(k)}
    f = {k: v for k, v in found.items() if inn(k)}
    tp = [k for k in g if k in f]
    fn = [k for k in g if k not in f]
    fp = [k for k in f if k not in g]

    def rows(keys, evsof):
        out = []
        for k in sorted(keys):
            evs = evsof(k)
            if not evs:
                continue
            p, sp = evs[0][0], evs[0][1]
            ls = src.lines(p)
            text = ls[sp[0]].strip()[:160] if 0 <= sp[0] < len(ls) else ""
            out.append((k[0][0], k[0][2], k[1][0], k[1][1], k[1][2], p, sp[0] + 1, text, sp))
        return out

    return {
        "gold": len(g), "tp": len(tp), "fp": len(fp), "fn": len(fn),
        "fn_rows": rows(fn, lambda k: g[k]),
        "fp_rows": rows(fp, lambda k: [ev for x in f[k] for ev in x[2]]),
    }


def scoreboard_cell(board, target, cls):
    if not board:
        return None
    for c in board["cells"]:
        if c["bench"] == "scip" and c["target"] == target and c["mode"] == "noscip" and c["class"] == cls:
            in_scope = c["gold"] - sum(c["out_of_scope"].values()) - sum(c["unmatchable"].values())
            return {"gold": in_scope, "tp": sum(c["tp"].values()), "fp": sum(c["fp"].values()), "fn": c["fn"]}
    return None


def section_agreement(record, board, repos, targets):
    print("## agreement: scip calls by location identity, against the scoreboard cell")
    out = {}
    for t in targets:
        src = Source(os.path.join(repos, t))
        for cls in ("calls", "file"):
            a = agreement(record, t, src, cls)
            name = "calls" if cls == "calls" else "calls_file_scope"
            cell = scoreboard_cell(board, t, name)
            mine = {k: a[k] for k in ("gold", "tp", "fp", "fn")}
            verdict = "no scoreboard" if cell is None else ("= scoreboard" if cell == mine else f"DIFFERS from scoreboard {cell}")
            print(f"{t} {name}: in-scope gold {a['gold']}, tp {a['tp']}, fp {a['fp']}, fn {a['fn']}  ({verdict})")
            out[(t, cls)] = (a, src)
    return out


# ------------------------------------------------------------------------------------------
# ts (the planner's ts_classify.py)

IMP = re.compile(r"""import\s+(type\s+)?(?P<clause>[^'";]*?)\s+from\s+['"](?P<spec>[^'"]+)['"]""", re.S)


def ts_imports(text):
    out = {}
    for m in IMP.finditer(text):
        clause, spec = m.group("clause").strip(), m.group("spec")
        ns = re.match(r"\*\s+as\s+(\w+)", clause)
        if ns:
            out[ns.group(1)] = ("namespace", spec)
            continue
        named = re.search(r"\{([^}]*)\}", clause)
        if named:
            for part in named.group(1).split(","):
                part = part.strip()
                if not part:
                    continue
                bits = re.split(r"\s+as\s+", part)
                out[bits[-1].strip().replace("type ", "")] = ("named", spec)
        default = re.match(r"(\w+)\s*(,|$)", clause)
        if default:
            out[default.group(1)] = ("default", spec)
    return out


def spec_kind(spec):
    if spec.startswith("."):
        return "relative"
    if spec.startswith("@/"):
        return "alias@/"
    if spec.startswith(":"):
        return "alias:"
    return "bare"


def ts_classify(rows, repo):
    cache, c, ex, by_app = {}, collections.Counter(), collections.defaultdict(list), collections.Counter()
    for r in rows:
        dst, name, site, text = r[2], r[4], r[5], r[7]
        if site not in cache:
            with open(os.path.join(repo, site), encoding="utf-8", errors="replace") as f:
                cache[site] = ts_imports(f.read())
        imps = cache[site]
        m = re.search(r"([\w$.]+)\s*\.\s*" + re.escape(name) + r"\b", text)
        if m:
            head = m.group(1).split(".")[0]
            if head in imps:
                form, spec = imps[head]
                key = f"qualified {form} {spec_kind(spec)}"
            elif head in ("this",):
                key = "qualified this"
            else:
                key = "qualified other-head"
        elif re.search(r"\b" + re.escape(name) + r"\b", text):
            if name in imps:
                form, spec = imps[name]
                key = f"bare {form} {spec_kind(spec)}"
            else:
                key = "bare local/other"
        else:
            key = "name not on line"
        c[key] += 1
        if key == "qualified namespace alias@/":
            by_app[site.split("/")[0]] += 1
        ex[key].append(f"{site}:{r[6]} {text[:90]} -> {dst}:{r[3]} {name}")
    return c, ex, by_app


def section_ts(agree, results):
    print("## ts: bookcars misses by how the callee's name reaches the call")
    for cls, label in (("calls", "calls"), ("file", "calls_file_scope")):
        if ("bookcars", cls) not in agree:
            continue
        a, src = agree[("bookcars", cls)]
        c, ex, by_app = ts_classify(a["fn_rows"], src.root)
        print(f"bookcars {label}: fn {a['fn']}")
        for k, v in c.most_common():
            print(f"  {v:5d} {k}")
            for e in ex[k][:2]:
                print(f"          {e}")
        ns = c["qualified namespace alias@/"]
        bare = c["bare named alias@/"] + c["bare default alias@/"]
        rel = c["bare default relative"]
        other = a["fn"] - ns - bare - rel
        print(f"  contract forms: ns.f() through @/ {ns} ({', '.join(f'{k} {v}' for k, v in sorted(by_app.items()))}); "
              f"bare name from @/ {bare} (named {c['bare named alias@/']}, default {c['bare default alias@/']}); "
              f"bare default import, relative {rel}; other {other}")
        if cls == "calls":
            results.update({
                "bookcars fn": a["fn"], "bookcars ns@/": ns, "bookcars ns@/ admin": by_app["admin"],
                "bookcars ns@/ frontend": by_app["frontend"], "bookcars bare @/": bare,
                "bookcars bare @/ named": c["bare named alias@/"],
                "bookcars bare @/ default": c["bare default alias@/"],
                "bookcars bare default relative": rel, "bookcars other": other,
            })


# ------------------------------------------------------------------------------------------
# go (the planner's go_forms.py, as a class)

FUNC = re.compile(r"^func\s*(\((\w*)\s*\*?(\w+)(?:\[[^\]]*\])?\))?\s*(\w+)(?:\[[^\]]*\])?\s*\((.*)")
BLOCKERS = {"ctor", "callchain", "alias", "embed", "pkgvar", "head-type-unresolved", "bare", "iface"}
FIVE = {"ctor", "callchain", "alias", "embed", "pkgvar"}


class GoForms:
    """Per Go miss, the syntactic forms between the call and its callee. Flags (several may
    hold): crossfile (callee method declared outside its receiver type's file: M9b B9), ctor
    (head is a local assigned from a call), callchain (a call inside the qualifier), alias
    (head's declared type is a `type T = U` alias), embed (callee's receiver type is embedded
    in the head's type), iface (head's declared type is an interface), pkgvar (head is a
    package-level var of another file), fieldchain (qualifier walks fields only),
    head-type-unresolved (head type not found)."""

    def __init__(self, repo):
        self.repo = repo
        self.src = Source(repo)
        self.tcache, self.imp_cache = {}, {}

    def lines(self, p):
        return self.src.lines(p)

    def types(self, d):
        if d in self.tcache:
            return self.tcache[d]
        out = {}
        full = os.path.join(self.repo, d)
        for f in sorted(os.listdir(full)) if os.path.isdir(full) else []:
            if not f.endswith(".go"):
                continue
            p = os.path.join(d, f) if d else f
            ls = self.lines(p)
            i, ingroup = 0, False
            while i < len(ls):
                l = ls[i]
                if re.match(r"^type\s*\($", l):
                    ingroup = True
                    i += 1
                    continue
                if ingroup and l.startswith(")"):
                    ingroup = False
                    i += 1
                    continue
                m = re.match(r"^(?:type\s+|\t)(\w+)(?:\[[^\]]*\])?\s+(=\s*)?(struct|interface)?\s*(.*)$", l) \
                    if (l.startswith("type ") or ingroup) else None
                if m and (l.startswith("type ") or ingroup):
                    name, alias, kind, rest = m.group(1), m.group(2), m.group(3), m.group(4)
                    body = []
                    if kind and rest.strip().startswith("{") and not rest.strip().endswith("}"):
                        j, depth = i + 1, 1
                        while j < len(ls) and depth > 0:
                            depth += ls[j].count("{") - ls[j].count("}")
                            body.append(ls[j])
                            j += 1
                        i = j - 1
                    k = "alias" if alias else (kind or "other")
                    target = rest.strip() if alias else ""
                    out.setdefault(name, (p, k, body, target))
                i += 1
        self.tcache[d] = out
        return out

    def imports(self, p):
        if p in self.imp_cache:
            return self.imp_cache[p]
        out = {}
        for l in self.lines(p)[:300]:
            m = re.match(r'^\s*(?:import\s+)?(\w+\s+)?"([^"]+)"', l)
            if m:
                path = m.group(2)
                alias = (m.group(1) or "").strip() or path.rsplit("/", 1)[-1]
                out[alias] = path
        self.imp_cache[p] = out
        return out

    def resolve_dir(self, site, qual_pkg):
        imp = self.imports(site).get(qual_pkg)
        if not imp:
            return None
        for marker in ("/server/public/", "/server/v8/", "/server/"):
            if marker in "/" + imp:
                tail = ("/" + imp).split(marker, 1)[1]
                cand = "server/" + ("public/" if marker == "/server/public/" else "") + tail
                if os.path.isdir(os.path.join(self.repo, cand)):
                    return cand
        return None

    def lookup(self, site, tname):
        tname = tname.lstrip("*").strip()
        tname = re.sub(r"\[.*", "", tname)
        if "." in tname:
            pkg, name = tname.split(".", 1)
            d = self.resolve_dir(site, pkg)
        else:
            d, name = os.path.dirname(site), tname
        if d is None:
            return None
        e = self.types(d).get(name)
        return (d, name, e) if e else None

    def deref_alias(self, site, found, depth=0):
        if not found or depth > 3:
            return found, False
        d, name, (p, k, body, target) = found
        if k == "alias":
            nxt = self.lookup(p, target)
            return self.deref_alias(p, nxt, depth + 1)[0], True
        return found, False

    def head_type(self, site, line, head):
        ls = self.lines(site)
        i = line - 1
        while i >= 0 and not ls[i].startswith("func "):
            i -= 1
        if i < 0:
            return None, "nofunc"
        m = FUNC.match(ls[i])
        if m and m.group(2) == head:
            return m.group(3), "receiver"
        for l in reversed(ls[i + 1: line]):
            mm = re.search(r"(?<![\w.])((?:\w+\s*,\s*)*" + re.escape(head) + r"(?:\s*,\s*\w+)*)\s*(:=|=)\s*(.*)$", l)
            if mm and not re.search(re.escape(head) + r"\s*==", l):
                rhs = mm.group(3).strip()
                lit = re.match(r"&?([\w.]+)\{", rhs)
                if lit:
                    return lit.group(1), "local-composite"
                if re.match(r"[\w.]+\(", rhs) or re.match(r"[\w.()]+\.\w+\(", rhs):
                    return None, "local-call"
                return None, "local-other"
            mv = re.search(r"\bvar\s+" + re.escape(head) + r"\s+(\*?[\w.]+)", l)
            if mv:
                return mv.group(1), "local-var"
            mv = re.search(r"^\s+" + re.escape(head) + r"\s*=\s*&?([\w.]+)\{", l)
            if mv:
                return mv.group(1), "local-composite"
            if re.search(r"\brange\b", l) and re.search(r"\b" + re.escape(head) + r"\b", l.split("range")[0]):
                return None, "range"
            if re.search(r"func\s*\([^)]*\b" + re.escape(head) + r"\s+(\*?[\w.]+)", l):
                t = re.search(r"func\s*\([^)]*\b" + re.escape(head) + r"\s+(\*?[\w.]+)", l).group(1)
                return t, "closure-param"
        sig = ls[i]
        j = i
        while not re.search(r"\)\s*[^,]*\{\s*$", ls[j]) and j < min(len(ls) - 1, i + 15):
            j += 1
            sig += " " + ls[j].strip()
        params = sig[m.end(4):] if m else sig
        pm = re.search(r"\b" + re.escape(head) + r"(?:\s*,\s*\w+)*\s+(\*?[\w.\[\]]+)", params)
        if pm:
            return pm.group(1), "param"
        d = os.path.dirname(site)
        full = os.path.join(self.repo, d)
        for f in sorted(os.listdir(full)):
            if f.endswith(".go"):
                for l in self.lines(os.path.join(d, f)):
                    mv = re.match(r"^(?:var\s+|\t)" + re.escape(head) + r"\s+(\*?[\w.]+)", l)
                    if mv and mv.group(1) not in ("=",):
                        return mv.group(1), "pkgvar" + ("" if os.path.join(d, f) == site else "-sibling")
        if head in self.imports(site):
            return None, "package"
        return None, "unknown"

    @staticmethod
    def split_qual(expr):
        parts, depth, cur = [], 0, ""
        for ch in expr:
            if ch in "([{":
                depth += 1
            elif ch in ")]}":
                depth -= 1
            if ch == "." and depth == 0:
                parts.append(cur)
                cur = ""
            else:
                cur += ch
        parts.append(cur)
        return [p for p in parts if p]

    @staticmethod
    def qualifier(text, name):
        i = text.find("." + name + "(")
        if i < 0:
            i = text.find("." + name)
        if i < 0:
            return None
        j, depth = i - 1, 0
        while j >= 0:
            ch = text[j]
            if ch in ")]}":
                depth += 1
            elif ch in "([{":
                if depth == 0:
                    break
                depth -= 1
            elif depth == 0 and not (ch.isalnum() or ch in "_.*"):
                break
            j -= 1
        return text[j + 1:i].lstrip("*")

    def callee(self, dst, line):
        ls = self.lines(dst)
        l = ls[line - 1] if line - 1 < len(ls) else ""
        m = FUNC.match(l)
        if not m:
            return None, None
        return (m.group(3) if m.group(1) else None), m.group(4)

    def embeds(self, entry, rtype, seen=0):
        if not entry or seen > 3:
            return False
        d, name, (p, k, body, _) = entry
        if k != "struct":
            return False
        for l in body:
            m = re.match(r"^\s*\*?([\w.]+)\s*(`.*)?$", l)
            if m and not l.strip().startswith("//"):
                emb = m.group(1)
                if emb.split(".")[-1] == rtype:
                    return True
                if self.embeds(self.lookup(p, emb), rtype, seen + 1):
                    return True
        return False

    def flags(self, r):
        src, dst, dline, name, site, sline, text = r[0], r[2], int(r[3]), r[4], r[5], int(r[6]), r[7]
        rtype, _ = self.callee(dst, dline)
        flags = set()
        if rtype:
            decl = self.types(os.path.dirname(dst)).get(rtype)
            if decl and decl[0] != dst:
                flags.add("crossfile")
        q = self.qualifier(text, name)
        if not q:
            flags.add("bare")
        else:
            parts = self.split_qual(q)
            if any("(" in p for p in parts[1:]) or "(" in parts[0]:
                flags.add("callchain")
            elif len(parts) > 1:
                flags.add("fieldchain")
            head = re.sub(r"[(\[].*", "", parts[0])
            t, how = self.head_type(site, sline, head)
            flags.add("head:" + how)
            if how == "local-call":
                flags.add("ctor")
            if how.startswith("pkgvar-sibling"):
                flags.add("pkgvar")
            if t:
                found = self.lookup(site, t)
                found2, was_alias = self.deref_alias(site, found)
                if was_alias:
                    flags.add("alias")
                if found2 and len(parts) == 1 and rtype:
                    k = found2[2][1]
                    if k == "interface":
                        flags.add("iface")
                    elif found2[1] != rtype and self.embeds(found2, rtype):
                        flags.add("embed")
                elif not found2:
                    flags.add("head-type-unresolved")
        if src.endswith("_test.go"):
            flags.add("test")
        return flags


def section_go(agree, results, targets, sample):
    print("## go: Go misses by the forms between call and callee")
    for t in targets:
        if (t, "calls") not in agree:
            continue
        a, src = agree[(t, "calls")]
        rows = [r for r in a["fn_rows"] if r[0].endswith(".go")]
        other = len(a["fn_rows"]) - len(rows)
        gf = GoForms(src.root)
        fc, combos, ex = collections.Counter(), collections.Counter(), collections.defaultdict(list)
        blockers = collections.Counter()
        b9_only = 0
        per_row = []
        for r in rows:
            fl = gf.flags(r)
            per_row.append((r, fl))
            for f in fl:
                fc[f] += 1
            key = tuple(sorted(f for f in fl if f != "test" and not f.startswith("head:")))
            combos[key] += 1
            ex[key].append(f"{r[5]}:{r[6]} {r[7][:100]} -> {r[2]}:{r[3]} {r[4]}")
            b = frozenset(fl & BLOCKERS)
            blockers[b] += 1
            if not b and "crossfile" in fl:
                b9_only += 1
        print(f"{t}: fn {a['fn']}, Go {len(rows)}, other languages {other}")
        for f, v in fc.most_common():
            print(f"  {v:6d} {f}")

        def bound(allowed):
            return sum(v for b, v in blockers.items() if b and b <= allowed)

        bounds = {
            "ctor": bound({"ctor"}), "callchain": bound({"callchain"}), "alias": bound({"alias"}),
            "embed": bound({"embed"}), "pkgvar": bound({"pkgvar"}),
            "pkgvar+unplaced": bound({"pkgvar", "head-type-unresolved"}),
            "all five": bound(FIVE), "ctor+callchain": bound({"ctor", "callchain"}),
        }
        print(f"  misses whose only blocker is B9 (crossfile, no other blocking flag): {b9_only}")
        print("  upper bounds (misses with at least one blocking flag, every one in the set): "
              + "; ".join(f"{k} {v}" for k, v in bounds.items()))
        print(f"  no blocking flag and not crossfile: {blockers[frozenset()] - b9_only}")
        n = 40 if t == "mattermost" else 20
        for k, v in combos.most_common(n):
            print(f"  {v:6d} {'+'.join(k) or '(none)'}")
            for e in ex[k][:1 if t == 'mattermost' else 12]:
                print(f"           {e}")
        if t == "mattermost":
            results.update({
                "mattermost Go misses": len(rows), "mattermost TS/JS misses": other,
                "go crossfile": fc["crossfile"], "go B9-only blocker": b9_only,
                "go ctor": fc["ctor"], "go callchain": fc["callchain"], "go alias": fc["alias"],
                "go embed": fc["embed"], "go pkgvar": fc["pkgvar"],
                **{f"go bound {k}": v for k, v in bounds.items()},
            })
            if sample:
                rng = random.Random(9)
                pick = list(per_row)
                rng.shuffle(pick)
                print("  seeded sample (seed 9, 40 rows) for reading by hand:")
                for r, fl in pick[:40]:
                    print(f"    {r[5]}:{r[6]} {r[7][:90]} -> {r[2]}:{r[3]} {r[4]} [{','.join(sorted(fl))}]")


# ------------------------------------------------------------------------------------------
# b1: the mattermost agreement fp at the call token, against the scip-go occurrences


B1_PKG = "pkg.Name(): one-identifier qualifier with a package occurrence of job server/public"
B1_PKG_LOST = "pkg.Name() into job server/public, no occurrence at the qualifier"
B1_VALUE = "member of a job server/public value"


def section_b1(agree, results, jobs_by_root):
    """Buckets, in order: (1) the call token follows `q.` where `q` is one identifier carrying
    a non-definition package occurrence of another job's module (D8's hand-back form); (2) the
    same text form into another job's package, where the index has no occurrence at `q`;
    (3) any other call into another job's file (a member of a value); (4) a bare call (no `.`
    before the token); (5) other."""
    print("## b1: mattermost agreement fp by form, at the call token's span (contract D8)")
    if ("mattermost", "calls") not in agree:
        return
    a, src = agree[("mattermost", "calls")]
    roots = sorted(jobs_by_root)
    modules = {jobs_by_root[r]["module"]: r for r in roots}
    docs = {r: {d["path"]: d for d in jobs_by_root[r]["idx"]["documents"]} for r in roots}
    gf = GoForms(src.root)
    c, ex = collections.Counter(), collections.defaultdict(list)
    for r in a["fp_rows"]:
        site, sp, dst, name = r[5], r[8], r[2], r[4]
        line = src.blines(site)[sp[0]] if sp[0] < len(src.blines(site)) else b""
        before = line[:sp[1]]
        q = re.search(rb"(?<![\w.)\]])([A-Za-z_]\w*)\.$", before)
        site_job, dst_job = owner(roots, site), owner(roots, dst)
        kind = None
        if q and site_job is not None:
            rel = site if site_job == "." else site[len(site_job) + 1:]
            d = docs[site_job].get(rel)
            if d:
                for rng, sym, roles in d["occurrences"]:
                    if rng[0] == sp[0] and rng[1] == q.start(1) and not roles & s.ROLE_DEFINITION:
                        p = s.split_symbol(sym)
                        if p and p[4].endswith("/") and p[2] in modules and modules[p[2]] != site_job:
                            kind = f"pkg.Name(): one-identifier qualifier with a package occurrence of job {modules[p[2]]}"
            if kind is None and site.endswith(".go") and dst_job not in (None, site_job):
                imp = gf.imports(site).get(q.group(1).decode())
                # A function, not a method: a local can shadow the package name (`request`).
                is_func = gf.callee(dst, int(r[3])) == (None, name)
                if is_func and imp and imp.startswith(jobs_by_root[dst_job]["module"] + "/"):
                    kind = f"pkg.Name() into job {dst_job}, no occurrence at the qualifier"
        if kind is None:
            if dst_job is not None and site_job is not None and dst_job != site_job:
                kind = f"member of a job {dst_job} value"
            elif not before.endswith(b"."):
                kind = "bare call, docs/site" if site.startswith("docs/site/") else f"bare call, {site_job}"
            else:
                kind = "other"
        c[kind] += 1
        ex[kind].append(f"{site}:{r[6]} {r[7][:90]} -> {dst}:{r[3]} {name}")
    print(f"fp {a['fp']}")
    for k, v in c.most_common():
        print(f"  {v:5d} {k}")
        for e in ex[k][:3]:
            print(f"          {e}")
    bare = sum(v for k, v in c.items() if k.startswith("bare call"))
    results.update({
        "b1 fp": a["fp"], "b1 pkg.f()": c[B1_PKG], "b1 pkg.f() qualifier lost": c[B1_PKG_LOST],
        "b1 value method": c[B1_VALUE], "b1 bare calls": bare,
        "b1 bare calls docs/site": c["bare call, docs/site"],
        "b1 other": a["fp"] - c[B1_PKG] - c[B1_VALUE] - bare,
    })


# ------------------------------------------------------------------------------------------
# d7: the planner's b13b_rule.py, over a chosen set of indexes


def d7_verdicts(idxs, type_kinds=(49,)):
    """Verdicts for type -> interface `is_implementation` relationships whose source kind is in
    `type_kinds` (49 struct, 54 other named type; the planner's script read structs only)."""
    kinds, rels, sig = {}, {}, {}
    for d in [d for idx in idxs for d in idx["documents"]]:
        for sy in d["symbols"]:
            kinds[sy["symbol"]] = sy["kind"]
            sig[sy["symbol"]] = sy["sig"]
            if sy["relationships"]:
                rels[sy["symbol"]] = sy["relationships"]
    members = collections.defaultdict(set)
    for sym, k in kinds.items():
        if "#" in sym and not sym.endswith("#"):
            owner_, rest = sym.rsplit("#", 1)
            members[owner_ + "#"].add((rest.rstrip(".").rstrip("()"), k))
    by_last = collections.defaultdict(list)
    for sym, k in kinds.items():
        if sym.endswith("#") and k in (49, 21):
            by_last[sym.rsplit("/", 1)[-1].split("`")[-1].rstrip("#")].append(sym)

    def need(i, depth=0):
        out = set()
        for n, k in members[i]:
            emb = [x for x in by_last.get(n, []) if kinds.get(x) == 21]
            if emb and depth < 8:
                for e in emb:
                    out |= need(e, depth + 1)
            else:
                out.add(n)
        return out

    def method_set(t, depth=0, seen=None):
        seen = seen or set()
        if t in seen or depth > 8:
            return set()
        seen.add(t)
        own = {n for n, k in members[t] if k == 26}
        for n, k in members[t]:
            if k == 26:
                continue
            for c in by_last.get(n, []):
                if kinds.get(c) == 49:
                    own |= method_set(c, depth + 1, seen)
                elif kinds.get(c) == 21:
                    own |= need(c)
        return own

    verdict = {}
    for t, rs in rels.items():
        if not t.endswith("#") or kinds.get(t) not in type_kinds:
            continue
        have = method_set(t)
        for r in rs:
            if not r["impl"] or not r["symbol"].endswith("#"):
                continue
            i = r["symbol"]
            if kinds.get(i) != 21:
                verdict[(t, i)] = "target-not-interface-in-index"
                continue
            verdict[(t, i)] = "verified" if need(i) <= have else "unverified"
    structs = collections.defaultdict(list)
    for sym, k in kinds.items():
        if sym.endswith("#") and k == 49:
            structs[sym.rsplit("/", 1)[-1].split("`")[-1].rstrip("#")].append(sym)
    return verdict, sig, structs


def embedded_fields(sig):
    """The embedded field entries of a struct type's signature text: entries of one type name
    (an optional tag after it), in either form scip-go prints (`struct{A; b T}` or one field
    per line)."""
    m = re.search(r"struct\s*\{(.*)\}", sig, re.S)
    if not m:
        return []
    out = []
    for part in re.split(r"[;\n]", m.group(1)):
        part = re.sub(r'\s*("([^"\\]|\\.)*"|`[^`]*`)\s*$', "", part.strip())
        if part == "invalid type" or re.fullmatch(r"\*?[\w.]+", part):
            out.append(part)
    return out


def embeds_invalid(sym, sig, structs, depth=0, seen=None):
    """Whether a struct embeds, directly or through embedded structs of the loaded indexes
    (matched by last name), a field of `invalid type`: the case in which go/types'
    `hasAllMethods` answers true for every interface (go/types/lookup.go, `hasAllMethods`,
    `hasInvalidEmbeddedFields`; section scipgo)."""
    seen = seen if seen is not None else set()
    if sym in seen or depth > 8:
        return False
    seen.add(sym)
    for f in embedded_fields(sig.get(sym, "")):
        if f == "invalid type":
            return True
        for cand in structs.get(f.lstrip("*").rsplit(".", 1)[-1], []):
            if embeds_invalid(cand, sig, structs, depth + 1, seen):
                return True
    return False


def section_d7(record, results, jobs, samples):
    print("## d7: scip-go type -> interface relationships, the D7 rule over the raw indexes")
    for t in sorted(jobs):
        roots = jobs[t]
        if not roots:
            continue
        path = os.path.join(record, t + ".scip.jsonl")
        rows = []
        with open(path) as f:
            next(f)
            for line in f:
                if '"IMPLEMENTS"' not in line:
                    continue
                r = json.loads(line)
                if r[6] == "scip" and r[2].startswith("scip-go"):
                    rows.append(r)
        if not rows:
            print(f"{t}: 0 scip-go IMPLEMENTS rows")
            continue
        readings = []
        if t == "mattermost":
            pair = [roots[r]["idx"] for r in ("server", "server/public") if r in roots]
            readings.append(("server + server/public (the planner's reading)", pair))
        readings.append((f"all {len(roots)} jobs' indexes together", [j["idx"] for j in roots.values()]))
        for label, idxs in readings:
            verdict, sig, structs = d7_verdicts(idxs)
            c = collections.Counter()
            inv = collections.Counter()
            for r in rows:
                v = verdict.get((r[2], r[5]), "no relationship in these indexes")
                c[(v, "suite" if "Suite#" in r[2] else "other")] += 1
                inv[(v, embeds_invalid(r[2], sig, structs))] += 1
            print(f"{t}, {label}: {len(rows)} IMPLEMENTS rows")
            for k in sorted(c):
                print(f"  {c[k]:5d} {k[0]} ({k[1]})")
            for k in sorted(inv):
                print(f"  {inv[k]:5d} {k[0]}, source struct embeds a field of invalid type: {k[1]}")
            if t == "mattermost" and label.startswith("server + server/public"):
                ver = sum(v for k, v in c.items() if k[0] == "verified")
                unv = sum(v for k, v in c.items() if k[0] == "unverified")
                results.update({
                    "d7 verified": ver, "d7 unverified": unv,
                    "d7 unverified suite": c[("unverified", "suite")],
                    "d7 outside": len(rows) - ver - unv,
                })
                jc = collections.Counter()
                for line in open(samples):
                    if line.startswith("#"):
                        continue
                    f = line.rstrip("\n").split("\t")
                    if len(f) < 5 or "|IMPLEMENTS|" not in f[3] or "scip-go" not in f[3]:
                        continue
                    parts = f[3].split("|")
                    jc[(verdict.get((parts[1], parts[4]), "not a type relationship"), f[4])] += 1
                print("  judged sample rows (bench/samples/exact.tsv), rule verdict and judged verdict:")
                for k, v in sorted(jc.items()):
                    print(f"    {v} {k}")
        # The per-job reading, as D7 is decided ("the job's SymbolInfo").
        first, _, _ = d7_verdicts(readings[0][1])
        c, moved = collections.Counter(), collections.Counter()
        per_job = {r: d7_verdicts([j["idx"]], (49, 54))[0] for r, j in roots.items()}

        def short(sym):
            return "/".join(sym.split("`")[-2:]).rsplit("/", 2)[-2:][0] + "/" + sym.split("/")[-1] \
                if "`" in sym else sym

        for r in rows:
            vs = [per_job[root].get((r[2], r[5])) for root in roots]
            vs = [v for v in vs if v]
            v = vs[0] if len(vs) == 1 else ("several jobs" if vs else "no relationship in any job")
            c[v] += 1
            was = first.get((r[2], r[5]), "no relationship in any job")
            if v != was:
                moved[(was, v, short(r[2]), short(r[5]))] += 1
        print(f"{t}, each job's own index, structs and other named types (D7 as decided): "
              + "; ".join(f"{k} {v}" for k, v in sorted(c.items())))
        if moved:
            print(f"  rows whose verdict differs from the reading '{readings[0][0]}': {sum(moved.values())}")
            for k, v in sorted(moved.items()):
                print(f"    {v} {k[0]} -> {k[1]}: {k[2]} -> {k[3]}")


# ------------------------------------------------------------------------------------------
# scipgo: the type check behind the missing occurrences

GETCLUSTERID = [
    ("server", "channels/app/audit.go", 109),
    ("server", "channels/app/slashcommands/command_mobile_logs.go", 48),
    ("server", "channels/web/context.go", 67),
]
SKIP_HEAD = set("true false nil len cap make new append string int int64 int32 uint byte rune float64 bool any "
                "error func map struct chan interface".split())
SELKEY = re.compile(rb"^\s*([A-Za-z_]\w*)\.([A-Za-z_]\w*)\s*:\s+")


def key_class(sym, module):
    if sym is None:
        return "key head has no occurrence"
    if sym.startswith("local "):
        return "local variable"
    p = s.split_symbol(sym)
    if not p:
        return "other"
    if p[2] == "github.com/golang/go/src":
        return "stdlib package"
    if p[2] == module:
        return "own-module package"
    return "package of another module"


def section_scipgo(all_jobs, repos):
    print("## scipgo: scip-go 0.2.7's type check in rb's job image")
    print("Locals whose signature (SymbolInformation.signature_documentation) holds `invalid type`:")
    for (t, root), j in sorted(all_jobs.items()):
        tot = bad = 0
        for d in j["idx"]["documents"]:
            for sy in d["symbols"]:
                if sy["symbol"].startswith("local "):
                    tot += 1
                    bad += "invalid type" in sy["sig"]
        print(f"  {t} {root}: {bad} of {tot} locals")
    print("Selector-keyed composite-literal entries `X.Y: v` whose value starts with an identifier:")
    print("  (key class) -> value head has an occurrence: yes / no")
    for (t, root), j in sorted(all_jobs.items()):
        base = os.path.join(repos, t) if root == "." else os.path.join(repos, t, root)
        c = collections.Counter()
        ex = {}
        for d in j["idx"]["documents"]:
            try:
                with open(os.path.join(base, d["path"]), "rb") as f:
                    src = f.read().split(b"\n")
            except OSError:
                continue
            occ = collections.defaultdict(dict)
            for rng, sym, _ in d["occurrences"]:
                occ[rng[0]][rng[1]] = sym
            for i, l in enumerate(src):
                if l.lstrip().startswith((b"case ", b"//")):
                    continue
                m = SELKEY.match(l)
                if not m:
                    continue
                h = re.match(rb"&?([A-Za-z_]\w*)", l[m.end():])
                if not h or h.group(1).decode() in SKIP_HEAD:
                    continue
                kc = key_class(occ[i].get(m.start(1)), j["module"])
                hit = (m.end() + h.start(1)) in occ[i]
                c[(kc, hit)] += 1
                ex.setdefault((kc, hit), f"{d['path']}:{i + 1}: {l.strip()[:90].decode('utf-8', 'replace')}")
        if c:
            for kc in sorted({k for k, _ in c}):
                print(f"  {t} {root}: {kc}: yes {c[(kc, True)]}, no {c[(kc, False)]}"
                      f"   e.g. {ex.get((kc, False)) or ex.get((kc, True))}")
    print("The 3 App#GetClusterId fp sites (server's index), every occurrence on the line:")
    for root, path, line in GETCLUSTERID:
        j = all_jobs[("mattermost", root)]
        d = next(d for d in j["idx"]["documents"] if d["path"] == path)
        src = open(os.path.join(repos, "mattermost", root, path), "rb").read().split(b"\n")
        print(f"  {root}/{path}:{line} | {src[line - 1].strip().decode()}")
        for rng, sym, roles in sorted(d["occurrences"]):
            if rng[0] == line - 1:
                print(f"      {list(rng)} {src[line - 1][rng[1]:rng[3]].decode()!r} roles={roles} {sym}")
        sigs = {sy["symbol"]: sy["sig"] for sy in d["symbols"]}
        for rng, sym, roles in sorted(d["occurrences"]):
            if sym.startswith("local ") and roles & s.ROLE_DEFINITION and line - 12 <= rng[0] < line:
                print(f"      local defined at {rng[0] + 1}: {sigs.get(sym, '')!r}")


# ------------------------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--record", action="append", required=True, help="LABEL=DIR[,SCOREBOARD]")
    ap.add_argument("--repos", default="var/bench/repos")
    ap.add_argument("--idx", default="var/m9b/idx")
    ap.add_argument("--manifest", default="bench/manifest.tsv")
    ap.add_argument("--samples", default="bench/samples/exact.tsv")
    ap.add_argument("--sample", action="store_true", help="print the seeded 40-row Go sample")
    args = ap.parse_args()

    records = []
    for spec in args.record:
        label, rest = spec.split("=", 1)
        d, _, board = rest.partition(",")
        records.append((label, d, json.load(open(board)) if board else None))

    # Indexes: each raw scip-go index is matched to the ok scip-go job whose go.mod module it
    # defines (job roots from the first record's status files).
    first = records[0][1]
    targets = ["bookcars", "go_clean_arch", "go_gin_example", "microservices_demo",
               "express_ts_boilerplate", "mattermost"]
    candidates = {}
    for t in targets:
        for root in ok_jobs(first, t, "scip-go"):
            try:
                candidates[s.go_module(os.path.join(args.repos, t), root)] = (t, root)
            except OSError:
                pass
    all_jobs = {}
    print(f"# m9c_forms.py: indexes in {args.idx}")
    for name in sorted(os.listdir(args.idx)):
        if not name.endswith(".scip"):
            continue
        idx = read_index_full(os.path.join(args.idx, name))
        mod = index_module(idx)
        where = candidates.get(mod)
        occ = sum(len(d["occurrences"]) for d in idx["documents"])
        print(f"  {name}: {idx['tool']}, module {mod}, documents {len(idx['documents'])}, "
              f"occurrences {occ} -> {('%s %s' % where) if where else 'NO JOB'}")
        if where:
            all_jobs[where] = {"idx": idx, "module": mod}
    jobs = collections.defaultdict(dict)
    for (t, root), j in all_jobs.items():
        jobs[t][root] = j

    results_by_label = {}
    for label, record, board in records:
        print()
        print(f"# record {label}: {record}")
        results = {}
        section_base(record, args.manifest, args.repos)
        agree = section_agreement(record, board, args.repos, targets)
        section_ts(agree, results)
        section_go(agree, results, ["go_clean_arch", "go_gin_example", "microservices_demo", "mattermost"],
                   args.sample)
        section_b1(agree, results, jobs["mattermost"])
        section_d7(record, results, jobs, args.samples)
        results_by_label[label] = results

    print()
    section_scipgo(all_jobs, args.repos)

    print()
    print("## checks: the contract's M9a numbers")
    for label, results in results_by_label.items():
        if label != "m9a":
            continue
        bad = 0
        for k, want in CONTRACT_M9A.items():
            got = results.get(k)
            ok = got == want
            bad += not ok
            print(f"  {k}: contract {want}, probe {got}: {'reproduced' if ok else 'DIFFERS'}")
        print(f"  {len(CONTRACT_M9A) - bad} of {len(CONTRACT_M9A)} reproduced")
    labels = list(results_by_label)
    if len(labels) > 1:
        print("## side by side: " + " | ".join(labels))
        keys = list(CONTRACT_M9A) + [k for k in results_by_label[labels[0]] if k not in CONTRACT_M9A]
        for k in keys:
            print(f"  {k}: " + " | ".join(str(results_by_label[l].get(k)) for l in labels))


if __name__ == "__main__":
    main()
