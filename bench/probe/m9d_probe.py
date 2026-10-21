#!/usr/bin/env python3
"""M9d P0: re-base the M9d contract on M9c's close (contract M9d, P0 and amendment 1).

Run with `python3 -I bench/probe/m9d_probe.py [section ...]` from the repository root. Read-only:
it reads dumps, `.scip.status` files, raw SCIP indexes and git blobs, and writes nothing.

The planner's four scripts (`var/m9d/plan/scripts/{check_citations,gowork_measure,gowork_residue,
gowork_agreement}.py`) are ported here with their logic unchanged; `gowork_agreement` read M9c
P0b's `agreement_cells` (`var/m9c/p0b/scripts/p0b_measure.py`), which is ported with it. The only
changes are parameters: which record, which indexes, and (for the cross terms) a `found_record`
that defaults to the record. Each section first reproduces the contract's numbers on M9b's
record, then prints M9c's.

Inputs:
  var/bench/out/36ed5ea46ddf/    M9b's run of record (dumps, .scip.status)
  var/bench/out/6cd5baaa8d1f/    M9c's run of record (the pass-2 run)
  var/m9c/p0b/idx-{old,new}/     scip-go indexes on the old and the fixed image (M9c P0b)
  var/m9c/p7/idx/                every ok scip-go index cached under the fixed image's ID,
                                 exported READ ONLY from rb.scip_index at M9c P7 (the indexes
                                 M9c's record joined; `SHA256SUMS` beside them)
  var/m9d/plan/idx/              the planner's go.work index of mattermost `server`

Sections: citations, record, indexes, measure, residue, agreement, d8 (default: all).
"""
import collections
import json
import os
import re
import subprocess
import sys

sys.dont_write_bytecode = True
sys.path.insert(0, os.path.join(os.getcwd(), "bench", "probe"))
import m9c_forms as P  # noqa: E402
import scip_cross as s  # noqa: E402

ROOT = os.getcwd()
M9B_RECORD = "var/bench/out/36ed5ea46ddf"
M9C_RECORD = "var/bench/out/6cd5baaa8d1f"
REPOS = "var/bench/repos"
OLD, NEW = "var/m9c/p0b/idx-old", "var/m9c/p0b/idx-new"
M9C_IDX = "var/m9c/p7/idx"
GOWORK = "var/m9d/plan/idx/mattermost-server-gowork2.scip"
GOWORK_CACHE_ONLY = "var/m9d/plan/idx/mattermost-server-gowork.scip"
PUB = "github.com/mattermost/mattermost/server/public"
STDLIB = "github.com/golang/go/src"
JOBS = [  # M9c P0b's scip-go jobs (p0b_measure.JOBS)
    ("go_clean_arch", "."), ("go_gin_example", "."),
    ("microservices_demo", "src/checkoutservice"), ("microservices_demo", "src/frontend"),
    ("microservices_demo", "src/productcatalogservice"), ("microservices_demo", "src/shippingservice"),
    ("mattermost", "api/server"), ("mattermost", "docs/site/scripts/gen-plugin-godocs"),
    ("mattermost", "docs/site/scripts/gen-plugin-manifest-docs"), ("mattermost", "tools/mattermost-govet"),
    ("mattermost", "tools/mmgotool"), ("mattermost", "server/public"), ("mattermost", "server"),
]

_cache = {}


def read(path):
    """read_index_full, once per path."""
    if path not in _cache:
        _cache[path] = P.read_index_full(path)
    return _cache[path]


def forget(*paths):
    for p in paths:
        _cache.pop(p, None)


# ------------------------------------------------------------------------------------------
# citations (check_citations.py, unchanged logic; the list at the contract's HEAD, then at M9c's
# close with the moved lines updated and the contract's other citations added)

PLANNED = [  # (path, line, text the line must contain); "repo:" paths are files in var/bench/repos
    ("crates/rb-scip/src/jobs.rs", 12, 'SCIP_JAVA_IMAGE: &str = "rb-scip-java:0.13.1"'),
    ("crates/rb-scip/src/jobs.rs", 20, 'SCIP_GO_IMAGE: &str = "rb-scip-go:0.2.7-xtools0.50.0"'),
    ("crates/rb-scip/src/jobs.rs", 24, "SCIP_PYTHON_IMAGE"),
    ("crates/rb-scip/src/jobs.rs", 232, 'dirs_with("go.mod")'),
    ("crates/rb-scip/src/jobs.rs", 246, "let mut python_roots"),
    ("crates/rb-scip/src/jobs.rs", 280, 'if has(&join(root, "package-lock.json"))'),
    ("crates/rb-scip/src/orchestrate.rs", 27, 'RECIPE_TS: &str = "m6a-ts-1"'),
    ("crates/rb-scip/src/orchestrate.rs", 31, 'RECIPE_GO: &str = "m6d-go-1"'),
    ("crates/rb-scip/src/orchestrate.rs", 40, 'RECIPE_PY: &str = "m6d-py-4"'),
    ("crates/rb-scip/src/orchestrate.rs", 46, 'RECIPE_GRADLE: &str = "m6e-gradle-1"'),
    ("crates/rb-scip/src/orchestrate.rs", 206, "fetch_timeout: Duration::from_secs(600)"),
    ("crates/rb-scip/src/orchestrate.rs", 539, "(no index.scip was produced)"),
    ("crates/rb-scip/src/orchestrate.rs", 579, '"npm_config_update_notifier".into()'),
    ("crates/rb-scip/src/orchestrate.rs", 801, 'gradle(&["clean", "compileJava", "compileTestJava"])'),
    ("crates/rb-scip/src/orchestrate.rs", 860, '("GOFLAGS".into(), "-mod=mod".into())'),
    ("crates/rb-scip/src/orchestrate.rs", 938, 'PythonLock::Poetry => ["poetry", "install", "--no-root", "--no-interaction"]'),
    ("crates/rb-scip/src/orchestrate.rs", 973, 'args: ["scip-python", "index"]'),
    ("crates/rb-scip/src/egress.rs", 36, "pub const REGISTRIES"),
    ("crates/rb-scip/src/runner.rs", 288, ".stdout(std::process::Stdio::null())"),
    ("crates/rb-scip/src/runner.rs", 327, "stderr_tail: tail(&stderr, 40)"),
    ("crates/rb-indexer/src/projects.rs", 255, "rb_scip::Indexer::ScipJava | rb_scip::Indexer::ScipGo =>"),
    ("crates/rb-indexer/src/projects.rs", 284, "if run.status.is_cacheable()"),
    ("crates/rb-indexer/src/projects.rs", 338, "pub(crate) fn job_files"),
    ("crates/rb-indexer/src/lib.rs", 145, 'PIPELINE: &str = "m9b-2"'),
    ("crates/rb-indexer/src/lib.rs", 1500, 'detail["coverage"] = json!({'),
    ("crates/rb-indexer/src/lib.rs", 1530, '("lombok_removed", c.lombok_removed)'),
    ("crates/rb-indexer/src/scip.rs", 336, "let own_packages"),
    ("crates/rb-indexer/src/scip.rs", 553, "out.coverage.references_external += 1;"),
    ("crates/rb-scip/src/decode.rs", 659, "fn symbol_info"),
    ("crates/rb-cli/src/main.rs", 1596, "relationships dropped {} ({} in project, {} anonymous)"),
    ("crates/rb-store/src/check.rs", 38, 'cypher: "MATCH (t:Table)<-[q:QUERIES]-(x)'),
    ("crates/rb-store/src/lib.rs", 1170, 'tx.batch_execute("ANALYZE rb.node'),
    ("crates/rb-store/src/lib.rs", 1627, '"{} LIMIT {}"'),
    ("crates/rb-store/src/lib.rs", 1632, '"SET LOCAL statement_timeout = {}; SET LOCAL ROLE rb_reader"'),
    ("crates/rb-store/src/age.rs", 260, "pub(crate) fn cypher_sql"),
    ("repo:mattermost/server/Makefile", 466, "setup-go-work: ## Sets up your go.work file"),
    ("repo:mattermost/server/Makefile", 473, "$(GO) work use ./public"),
    ("repo:mattermost/server/go.mod", 50, "github.com/mattermost/mattermost/server/public v0.4.3"),
    ("repo:mattermost/tools/sharedchannel-test/go.mod", 51, "=> ../../server/public"),
    ("repo:mattermost/webapp/.npmrc", 6, "engine-strict=true"),
    ("repo:spring_boot_microservices/pom.xml", 15, "<java.version>25</java.version>"),
    ("repo:fastapi_realworld/poetry.lock", 992, 'category = "dev"'),
]
MOVED = {  # (path, planned line) -> (line at M9c's close, text); D8′, D9 and D10's sites
    ("crates/rb-indexer/src/lib.rs", 145): (157, 'PIPELINE: &str = "m9c-3"'),
    ("crates/rb-indexer/src/lib.rs", 1500): (1647, 'detail["coverage"] = json!({'),
    ("crates/rb-indexer/src/lib.rs", 1530): (1677, '("lombok_removed", c.lombok_removed)'),
    ("crates/rb-indexer/src/scip.rs", 336): (376, "let own_packages"),
    ("crates/rb-indexer/src/scip.rs", 553): (621, "out.coverage.references_external += 1;"),
}
ADDED = [  # the contract's other citations, and the M9c-close sites amendment 1 cites
    ("docker/scip-java/Dockerfile", 5, "FROM eclipse-temurin:21-jdk@sha256:"),
    ("crates/rb-scip/src/jobs.rs", 53, "A Gradle dependency resolution (not verified yet: such jobs are `unsupported`)"),
    ("crates/rb-bench/src/compare.rs", 211, "if hc.gold < bc.gold {"),
    ("repo:mattermost/server/public/shared/request/context.go", 199, "type CTX interface {"),
    ("repo:mattermost/server/public/shared/mlog/mlog.go", 34, "type LoggerIFace interface {"),
    ("crates/rb-indexer/src/lib.rs", 1090, "let go_jobs: Vec<(&str, &str)> = content"),
    ("crates/rb-indexer/src/lib.rs", 1133, "handed_back.push((project.root.as_str(), std::mem::take(&mut joined.external)));"),
    ("crates/rb-indexer/src/lib.rs", 1182, "join_across_projects(&go_jobs, &handed_back, &keys, &roots);"),
    ("crates/rb-indexer/src/lib.rs", 1185, "for ((service, key), (package, at)) in packages {"),
    ("crates/rb-indexer/src/lib.rs", 1234, ".chain(cross_edges.into_iter().map(|e| (e, Certainty::Synthetic)));"),
    ("crates/rb-indexer/src/lib.rs", 1531, "fn join_across_projects("),
    ("crates/rb-indexer/src/lib.rs", 1579, "n.synthetic += 1;"),
    ("crates/rb-indexer/src/lib.rs", 1587, "None => n.unanchored += 1,"),
    ("crates/rb-indexer/src/lib.rs", 1601, 'kind: "scip_cross_project".into(),'),
    ("crates/rb-indexer/src/lib.rs", 1657, '"references_unanchored": c.references_unanchored,'),
    ("crates/rb-indexer/src/scip.rs", 217, "pub(crate) fn join_handing_back("),
    ("crates/rb-indexer/src/scip.rs", 618, "if own_packages.contains(package) {"),
    ("crates/rb-indexer/src/scip.rs", 623, "out.external.push(External {"),
    ("crates/rb-indexer/src/scip.rs", 651, "// Packages: every occurrence of a global symbol the index does not define"),
    ("crates/rb-indexer/src/scip.rs", 664, "if own_packages.contains(package) {"),
    ("crates/rb-indexer/src/scip.rs", 674, "out.packages"),
    ("crates/rb-indexer/src/scip.rs", 827, "fn hands_back(symbol: &str, modules: &BTreeSet<&str>) -> bool {"),
    ("crates/rb-indexer/src/scip.rs", 837, "pub(crate) fn unversioned(symbol: &str) -> Option<(&str, &str, &str)> {"),
    ("crates/rb-cli/tests/common/mod.rs", 197, "let _ = std::fs::remove_dir_all(cache);"),
    ("crates/rb-cli/tests/common/mod.rs", 53, "pub fn remove_leftover_test_repositories(url: &str) {"),
    ("crates/rb-cli/tests/e2e.rs", 4428, "std::fs::remove_dir_all(&cache).unwrap();"),
    ("crates/rb-scip/src/orchestrate.rs", 838, 'let gomod = crate::runner::absolute(cache_dir)?.join("gomod");'),
    ("crates/rb-scip/src/orchestrate.rs", 365, 'let egress_dir = work_dir.with_file_name(format!("{instance}-egress"));'),
    ("crates/rb-scip/src/orchestrate.rs", 461, "let _ = std::fs::remove_dir_all(work_dir);"),
    ("crates/rb-scip/src/orchestrate.rs", 462, "let _ = std::fs::remove_dir_all(&egress_dir);"),
    ("crates/rb-scip/src/orchestrate.rs", 991, "std::fs::remove_dir_all(dir)?;"),
    ("crates/rb-scip/src/orchestrate.rs", 22, 'pub const RECIPE_JAVA: &str = "m5-egress-1";'),
]


def lines(path):
    if path.startswith("repo:"):
        return open(f"{ROOT}/{REPOS}/{path[5:]}", encoding="utf-8", errors="replace").read().split("\n")
    return subprocess.run(["git", "-C", ROOT, "show", f"HEAD:{path}"], capture_output=True, text=True,
                          check=True).stdout.split("\n")


def check(cites, label):
    for shift, what in ((0, "check"), (7, "control (+7 lines)")):
        bad = 0
        for p, n, t in cites:
            L = lines(p)
            ok = n + shift - 1 < len(L) and t in L[n + shift - 1]
            bad += not ok
            if shift == 0 and not ok:
                print(f"  MISMATCH {p}:{n} wants {t!r}")
        print(f"{label} {what}: {len(cites) - bad} of {len(cites)} hold, {bad} MISMATCH")


def section_citations():
    head = subprocess.run(["git", "-C", ROOT, "rev-parse", "--short", "HEAD"], capture_output=True,
                          text=True).stdout.strip()
    print(f"## citations at HEAD {head}")
    check(PLANNED, f"planned list ({len(PLANNED)}, lines at 15371eb):")
    updated = [(p, *MOVED.get((p, n), (n, t))) for p, n, t in PLANNED] + ADDED
    check(updated, f"updated list ({len(updated)}: {len(MOVED)} moved, {len(ADDED)} added):")
    for (p, n), (m, _) in sorted(MOVED.items()):
        print(f"  moved {p}:{n} -> {m}")


# ------------------------------------------------------------------------------------------
# record: what M9c's record holds, against M9b's (dumps, SCIP jobs)


STATUS = re.compile(r"\s*scip: (\S+) (scip-\S+) (ok|failed|timeout|unsupported|environment|skipped)\b(.*)")


def jobs_of(record):
    out = {}
    for name in sorted(os.listdir(record)):
        if not name.endswith(".scip.status"):
            continue
        t = name[: -len(".scip.status")]
        for line in open(os.path.join(record, name)):
            m = STATUS.match(line)
            if m:
                out[(t, m.group(1))] = (m.group(2), m.group(3), m.group(4).strip())
    return out


def section_record():
    print("## record: dumps and SCIP jobs, M9b's record vs M9c's")
    for rec in (M9B_RECORD, M9C_RECORD):
        dumps = [os.path.join(dp, f) for dp, _, fs in os.walk(rec) for f in fs if f.endswith(".jsonl")]
        empty = [d for d in dumps if os.path.getsize(d) == 0]
        print(f"  {rec}: {len(dumps)} dumps, {len(empty)} empty")
    b, c = jobs_of(M9B_RECORD), jobs_of(M9C_RECORD)
    for label, j in (("M9b", b), ("M9c", c)):
        st = collections.Counter(v[1] for v in j.values())
        print(f"  {label}: {len(j)} jobs, {dict(sorted(st.items()))}")
    for k in sorted(set(b) | set(c)):
        bv, cv = b.get(k), c.get(k)
        if (bv and bv[1] != "ok") or (cv and cv[1] != "ok") or (bv and cv and bv[:2] != cv[:2]):
            print(f"  {k[0]} {k[1]}: M9b {bv[1] + ' ' + bv[2][:40] if bv else 'none'} | "
                  f"M9c {cv[1] + ' ' + cv[2][:40] if cv else 'none'}")


# ------------------------------------------------------------------------------------------
# indexes: M9c's record's scip-go indexes, mapped to jobs and compared with P0b's


def occ_multiset(idx):  # p0b_measure.occ_multiset
    c = collections.Counter()
    for d in idx["documents"]:
        for rng, sym, roles in d["occurrences"]:
            c[(d["path"], tuple(rng), sym, roles)] += 1
        for sy in d["symbols"]:
            rel = tuple(sorted((x["symbol"], x["impl"]) for x in sy["relationships"]))
            c[("SYM", d["path"], sy["symbol"], sy["kind"], sy["sig"], rel)] += 1
    return c


def fname(t, r):
    return f"{t}-{r.replace('/', '_')}.scip"


def record_index_paths():
    """(target, root) -> path of the index M9c's record joined, by the module it defines."""
    module_of = {s.go_module(os.path.join(REPOS, t), r): (t, r) for t, r in JOBS}
    out, other = {}, []
    for name in sorted(os.listdir(M9C_IDX)):
        if not name.endswith(".scip"):
            continue
        path = os.path.join(M9C_IDX, name)
        idx = P.read_index_full(path)
        mod = P.index_module(idx)
        if mod in module_of and module_of[mod] not in out:
            out[module_of[mod]] = path
        else:
            other.append((name, mod))
    return out, other


def section_indexes():
    print("## indexes: M9c's record's scip-go indexes (var/m9c/p7/idx) against M9c P0b's idx-new")
    paths, other = record_index_paths()
    for (t, r), path in sorted(paths.items()):
        a = occ_multiset(read(path))
        b = occ_multiset(read(os.path.join(NEW, fname(t, r))))
        print(f"  {t} {r}: {os.path.basename(path)} vs idx-new: {'IDENTICAL' if a == b else 'DIFFERS'} "
              f"({sum(a.values())} items; only record {sum((a - b).values())}, only idx-new {sum((b - a).values())})")
        forget(path, os.path.join(NEW, fname(t, r)))
    for name, mod in other:
        print(f"  not a bench job: {name} (module {mod})")
    print(f"  {len(paths)} of {len(JOBS)} jobs mapped")


# ------------------------------------------------------------------------------------------
# measure (gowork_measure.py, unchanged logic, server's no-go.work index and server/public's
# index as arguments)


def defined_m(i):
    return {sym for d in i["documents"] for _, sym, r in d["occurrences"]
            if r & s.ROLE_DEFINITION and not sym.startswith("local ")}


def measure(idx, pubjob):
    pubdef = defined_m(pubjob)
    pubdef_nover = {(s.split_symbol(x)[2], s.split_symbol(x)[4]) for x in pubdef if s.split_symbol(x)}
    print("server/public job index (no go.work): documents", len(pubjob["documents"]), "definitions", len(pubdef),
          "versions of its own defs", collections.Counter(s.split_symbol(x)[3] for x in pubdef if s.split_symbol(x) and s.split_symbol(x)[2] == PUB))
    for k, i in idx.items():
        docs = i["documents"]
        under_pub = [d for d in docs if d["path"].startswith("public/")]
        occ = sum(len(d["occurrences"]) for d in docs)
        bad = tot = 0
        for d in docs:
            for sy in d["symbols"]:
                if sy["symbol"].startswith("local "):
                    tot += 1
                    bad += "invalid type" in sy["sig"]
        defs = defined_m(i)
        own_mods = collections.Counter((s.split_symbol(x)[2], s.split_symbol(x)[3]) for x in defs if s.split_symbol(x))
        print(f"\n## {k}: tool {i['tool']!r}")
        print(f"  documents {len(docs)} (under public/: {len(under_pub)}), occurrences {occ}, definitions {len(defs)}")
        print(f"  definitions by (package, version): {dict(own_mods.most_common(6))}")
        print(f"  locals typed `invalid type`: {bad} / {tot}")
        vers = collections.Counter()
        member = collections.Counter()
        for d in docs:
            if d["path"].startswith("public/"):
                continue
            for rng, sym, r in d["occurrences"]:
                if r & s.ROLE_DEFINITION or sym.startswith("local "):
                    continue
                p = s.split_symbol(sym)
                if not p or p[2] != PUB:
                    continue
                kind = "namespace" if p[4].endswith("/") else "member"
                vers[(kind, p[3])] += 1
                if kind == "member":
                    member["exact symbol defined by server/public job"] += sym in pubdef
                    member["defined modulo version"] += (p[2], p[4]) in pubdef_nover
                    member["not defined by server/public job"] += (p[2], p[4]) not in pubdef_nover
                    member["total"] += 1
        print(f"  server-document references to {PUB}, by (kind, version): {dict(sorted(vers.items()))}")
        print(f"  member references: {dict(member)}")


def invalid_locals(idx):  # p0b_measure.invalid_locals
    tot = bad = 0
    for d in idx["documents"]:
        for sy in d["symbols"]:
            if sy["symbol"].startswith("local "):
                tot += 1
                bad += "invalid type" in sy["sig"]
    return bad, tot


def section_measure():
    print("## measure (gowork_measure): the planner's inputs (M9c P0b idx-new)")
    idx = {"nowork": read(os.path.join(NEW, "mattermost-server.scip"))}
    if os.path.exists(GOWORK_CACHE_ONLY):
        idx["gowork-cache-only"] = read(GOWORK_CACHE_ONLY)
    else:
        print(f"  ({GOWORK_CACHE_ONLY} is absent: the cache-only column is not re-measured)")
    idx["gowork-fetched"] = read(GOWORK)
    measure(idx, read(os.path.join(NEW, "mattermost-server_public.scip")))
    forget(os.path.join(NEW, "mattermost-server.scip"), os.path.join(NEW, "mattermost-server_public.scip"))
    print("\n## measure (gowork_measure): M9c's record's indexes")
    paths, _ = record_index_paths()
    measure({"nowork (M9c record)": read(paths[("mattermost", "server")]), "gowork-fetched": read(GOWORK)},
            read(paths[("mattermost", "server/public")]))
    print("\n## health (D10 base): locals typed `invalid type` in every scip-go index of M9c's record")
    for (t, r), path in sorted(paths.items()):
        bad, tot = invalid_locals(read(path))
        print(f"  {t} {r}: {bad} / {tot}")
        if (t, r) != ("mattermost", "server"):
            forget(path)
    bad, tot = invalid_locals(read(GOWORK))
    print(f"  mattermost server under go.work (planner's index): {bad} / {tot}")


# ------------------------------------------------------------------------------------------
# residue (gowork_residue.py, unchanged logic, server/public's index and the no-go.work index as
# arguments)


def residue(pub, runs):
    pubdef = {s.split_symbol(x)[4] for d in pub["documents"] for _, x, r in d["occurrences"]
              if r & s.ROLE_DEFINITION and not x.startswith("local ") and s.split_symbol(x)}
    pubsyms = {s.split_symbol(sy["symbol"])[4] for d in pub["documents"] for sy in d["symbols"]
               if not sy["symbol"].startswith("local ") and s.split_symbol(sy["symbol"])}

    def shape(desc):
        if re.search(r"\$[0-9a-f]{6,}", desc) or "$" in desc: return "anonymous-struct member ($hash)"
        if desc.endswith("()."): return "method/function"
        if desc.endswith("#"): return "type"
        if desc.endswith("."): return "field/var/const"
        return "other"
    for name, idx in runs:
        c, ex, pk, insyms = collections.Counter(), {}, collections.Counter(), 0
        names = collections.Counter()
        for d in idx["documents"]:
            for rng, sym, r in d["occurrences"]:
                if r & s.ROLE_DEFINITION or sym.startswith("local "): continue
                p = s.split_symbol(sym)
                if not p or p[2] != PUB or p[4].endswith("/") or p[4] in pubdef: continue
                k = shape(p[4]); c[k] += 1; ex.setdefault(k, f"{d['path']}:{rng[0]+1} {sym}")
                pk[p[4].split("`")[1] if "`" in p[4] else p[4].split("/")[0]] += 1
                insyms += p[4] in pubsyms
                names[p[4]] += 1
        print(f"## {name}: members not defined by server/public's index: {sum(c.values())}, "
              f"of which listed in its SymbolInformation (no definition occurrence) {insyms}; distinct symbols {len(names)}")
        for k, v in c.most_common(): print(f"  {v:7d} {k}  e.g. {ex[k]}")
        print("  top packages:", pk.most_common(6))
        print("  top symbols:", [(n[-80:], v) for n, v in names.most_common(8)])


def section_residue():
    print("## residue (gowork_residue): the planner's inputs (M9c P0b idx-new)")
    residue(read(os.path.join(NEW, "mattermost-server_public.scip")),
            (("nowork", read(os.path.join(NEW, "mattermost-server.scip"))), ("gowork-fetched", read(GOWORK))))
    forget(os.path.join(NEW, "mattermost-server.scip"), os.path.join(NEW, "mattermost-server_public.scip"))
    print("## residue (gowork_residue): M9c's record's indexes")
    paths, _ = record_index_paths()
    residue(read(paths[("mattermost", "server/public")]),
            (("nowork (M9c record)", read(paths[("mattermost", "server")])), ("gowork-fetched", read(GOWORK))))


# ------------------------------------------------------------------------------------------
# agreement (gowork_agreement.py over p0b_measure.agreement_cells, unchanged logic; the record
# and the found record are parameters)


def defined(idx):  # p0b_measure.defined
    out = set()
    for d in idx["documents"]:
        for _, sym, roles in d["occurrences"]:
            if roles & s.ROLE_DEFINITION and not sym.startswith("local "):
                out.add(sym)
    return out


def strip_version(sym):  # p0b_measure.strip_version
    p = s.split_symbol(sym)
    if not p:
        return None
    return (p[1], p[2], p[4])


def site_index(jobs, target, which):  # p0b_measure.site_index
    """(repo path, line, start col) -> symbol, for reference occurrences of global symbols."""
    out = {}
    roots = [r for (t, r) in jobs if t == target]
    for r in roots:
        idx = jobs[(target, r)][which]
        if not idx:
            continue
        for d in idx["documents"]:
            path = d["path"] if r == "." else f"{r}/{d['path']}"
            for rng, sym, roles in d["occurrences"]:
                if roles & s.ROLE_DEFINITION or sym.startswith("local "):
                    continue
                out[(path, rng[0], rng[1])] = (r, sym, rng[-1])
    return out


def agreement_cells(jobs, target, src, record, found_record=None):  # p0b_measure.agreement_cells
    sn, se = P.load_dump(os.path.join(record, target + ".scip.jsonl"))
    nn, ne = P.load_dump(os.path.join(found_record or record, target + ".noscip.jsonl"))
    covered = {e[5] for e in se if e[3] == "DEFINED_IN" and e[1] == "symbol" and e[2].startswith("scip-")}
    ssym, aliases = P.locator(sn, se)
    nsym, _ = P.locator(nn, ne)
    # functions and methods per file, by definition line, for the enclosing source of a site
    funcs = collections.defaultdict(list)
    for (k, key), name in sn.items():
        if k == "symbol" and key.startswith("scip-go") and key.endswith(")."):
            loc = ssym.get(key)
            if loc:
                funcs[loc[0]].append((loc[1], key))
    for v in funcs.values():
        v.sort()

    def enclosing(path, line1):
        best = None
        for l, key in funcs.get(path, []):
            if l <= line1:
                best = key
            else:
                break
        return best

    def sel(e):
        return e[3] == "CALLS" and e[1] != "file"

    gold, found, found_sites = {}, {}, collections.defaultdict(set)
    for e in se:
        if sel(e) and e[6] == "scip":
            a, b = ssym.get(e[2]), ssym.get(e[5])
            if a and b:
                gold.setdefault((a, b), []).extend(e[9])
    for e in ne:
        if sel(e):
            a, b = nsym.get(e[2]), nsym.get(e[5])
            if a and b:
                a, b = aliases.get(a, a), aliases.get(b, b)
                found.setdefault((a, b), []).append(e)
                for ev in e[9]:
                    found_sites[(ev[0], ev[1][0], ev[1][1])].add((a, b))

    def inn(k):
        return k[0][0] in covered and k[1][0] in covered

    old_sites = site_index(jobs, target, "old")
    new_sites = site_index(jobs, target, "new")
    roots = [r for (t, r) in jobs if t == target]
    modules = {jobs[(target, r)]["module"]: r for r in roots}
    pub_defined = {}
    for r in roots:
        if jobs[(target, r)]["new"]:
            for sym in defined(jobs[(target, r)]["new"]):
                pub_defined[strip_version(sym)] = sym

    # S1: gold pairs lost (no evidence span keeps its symbol) and gained (own symbol at a call)
    lost = set()
    for k, evs in gold.items():
        keep = False
        for ev in evs:
            key = (ev[0], ev[1][0], ev[1][1])
            if old_sites.get(key) == new_sites.get(key):
                keep = True
        if not keep:
            lost.add(k)
    gained, gained_x, why = set(), set(), collections.Counter()
    for key, (r, sym, end) in new_sites.items():
        if old_sites.get(key) == (r, sym, end):
            continue
        p = s.split_symbol(sym)
        if not p or not p[4].endswith(")."):
            continue
        path, line0, col = key
        bl = src.blines(path)
        after = bl[line0][end:end + 1] if line0 < len(bl) else b""
        if after not in (b"(", b"["):
            why["not call-shaped (no ( or [ after the token)"] += 1
            continue
        b = None
        cross = False
        if sym in ssym and ssym[sym]:
            b = ssym[sym]
        elif p[2] in modules and modules[p[2]] != r:
            canon = pub_defined.get(strip_version(sym))
            if canon and ssym.get(canon):
                b, cross = ssym[canon], True
            else:
                why["another job's symbol, not anchored there"] += 1
                continue
        else:
            why["not an anchored symbol of the repo (stdlib, dependency, unanchored)"] += 1
            continue
        src_key = enclosing(path, line0 + 1)
        a = ssym.get(src_key) if src_key else None
        if not a:
            why["no enclosing function or method"] += 1
            continue
        k = (a, b)
        (gained_x if cross else gained).add(k)
        why["cross-job symbol join" if cross else "own symbol"] += 1

    def score(g):
        g = {k for k in g if inn(k)}
        f = {k for k in found if inn(k)}
        return len(g), len(g & f), len(f - g), len(g - f)

    g0 = set(gold)
    s0 = score(g0)
    s1 = score((g0 - lost) | gained)
    s2 = score((g0 - lost) | gained | gained_x)
    return s0, s1, s2, len(lost), len(gained - g0), len(gained_x - g0), why


def fmt(x):
    g, tp, fp, fn = x
    return f"gold {g}, tp {tp}, fp {fp}, fn {fn}, P {tp/(tp+fp) if tp+fp else 0:.3f}, R {tp/g if g else 0:.3f}"


def delta(a, b):
    return ", ".join(f"{n} {y - x:+d}" for n, x, y in zip(("gold", "tp", "fp", "fn"), a, b))


def mattermost_jobs(old_of, new_of):
    jobs = {}
    for t, r in JOBS:
        if t != "mattermost":
            continue
        j = {"module": s.go_module(os.path.join(REPOS, t), r)}
        for k, f in (("old", old_of), ("new", new_of)):
            p = f(t, r)
            j[k] = read(p) if p and os.path.exists(p) else None
        jobs[(t, r)] = j
    return jobs


def section_agreement():
    src = P.Source(os.path.join(REPOS, "mattermost"))
    out = {}
    print("## agreement (gowork_agreement): M9b's record, the planner's inputs (old: idx-old, new: idx-new)")
    jobs = mattermost_jobs(lambda t, r: os.path.join(OLD, fname(t, r)), lambda t, r: os.path.join(NEW, fname(t, r)))
    for label in ("fixed image (M9c S0/S1/S2)", "fixed image + go.work server index"):
        if label.startswith("fixed image +"):
            jobs[("mattermost", "server")]["new"] = read(GOWORK)
        s0, s1, s2, lost, gain, gainx, why = agreement_cells(jobs, "mattermost", src, M9B_RECORD)
        print(f"### {label}")
        print(f"  S0 {fmt(s0)}")
        print(f"  S1 {fmt(s1)}  (gold pairs lost {lost}, new own pairs {gain})")
        print(f"  S2 {fmt(s2)}  (new cross-job pairs {gainx})")
        print(f"  new call-shaped occurrences by outcome: {dict(why)}")
        out[label] = (s0, s2)
    jobs[("mattermost", "server")]["new"] = read(os.path.join(NEW, "mattermost-server.scip"))
    print("### cross term: M9b's emulated S2 gold, M9c's record's noscip edges (found)")
    _, _, x1, *_ = agreement_cells(jobs, "mattermost", src, M9B_RECORD, found_record=M9C_RECORD)
    print(f"  S2 gold x M9c found: {fmt(x1)}")
    del jobs
    forget(*[os.path.join(d, fname("mattermost", r)) for d in (OLD, NEW) for t, r in JOBS if t == "mattermost"])

    print("\n## agreement: M9c's record (old = new = the record's indexes; then server's new = go.work)")
    paths, _ = record_index_paths()
    rec = lambda t, r: paths.get((t, r))  # noqa: E731
    jobs = mattermost_jobs(rec, rec)
    board = json.load(open("bench/scoreboard/m9c.json"))
    print(f"  scoreboard m9c.json cell: {P.scoreboard_cell(board, 'mattermost', 'calls')}")
    c0, _, c0b, *_ = agreement_cells(jobs, "mattermost", src, M9C_RECORD)
    print(f"  S0 (M9c record as scored) {fmt(c0)}; S2 with no index change {fmt(c0b)}")
    _, _, x2, *_ = agreement_cells(jobs, "mattermost", src, M9C_RECORD, found_record=M9B_RECORD)
    print(f"  cross term: M9c's gold x M9b's noscip edges: {fmt(x2)}")
    jobs[("mattermost", "server")]["new"] = read(GOWORK)
    g0, g1, g2, lost, gain, gainx, why = agreement_cells(jobs, "mattermost", src, M9C_RECORD)
    print(f"  go.work S1 {fmt(g1)}  (gold pairs lost {lost}, new own pairs {gain})")
    print(f"  go.work S2 {fmt(g2)}  (new cross-job pairs {gainx})")
    print(f"  new call-shaped occurrences by outcome: {dict(why)}")

    b_s0, b_s2 = out["fixed image (M9c S0/S1/S2)"]
    _, b_gw = out["fixed image + go.work server index"]
    print("\n## decomposition (pairs: gold, tp, fp, fn)")
    print(f"  A  M9b record, emulated fixed image + D8′ (contract S2): {fmt(b_s2)}")
    print(f"  X1 A's gold, M9c's noscip edges:                       {fmt(x1)}   [A -> X1: {delta(b_s2, x1)}] noscip edges (B2, B3, B5, ...)")
    print(f"  B  M9c record as scored:                              {fmt(c0)}   [X1 -> B: {delta(x1, c0)}] gold: rb's join vs the emulation, IMG, D3-doc, conversions")
    print(f"  X2 M9c's gold, M9b's noscip edges:                    {fmt(x2)}   [A -> X2: {delta(b_s2, x2)}] gold alone; [X2 -> B: {delta(x2, c0)}] noscip alone")
    print(f"  C  M9c record + go.work server index:                 {fmt(g2)}   [B -> C: {delta(c0, g2)}] go.work alone")
    print(f"  contract's go.work delta on M9b's edges (S2 -> go.work): {delta(b_s2, b_gw)}")


# ------------------------------------------------------------------------------------------
# d8: M9c's `synthetic` scip edges, and the symbol at their evidence under go.work; D9's base


def section_d8():
    print("## d8: M9c's record's synthetic scip edges against the go.work server index")
    sites = {}
    for d in read(GOWORK)["documents"]:
        for rng, sym, roles in d["occurrences"]:
            if roles & s.ROLE_DEFINITION or sym.startswith("local "):
                continue
            sites.setdefault((f"server/{d['path']}", rng[0], rng[1]), []).append(sym)
    c, ex = collections.Counter(), {}
    deps, cross = collections.Counter(), []
    for line in open(os.path.join(M9C_RECORD, "mattermost.scip.jsonl")):
        r = json.loads(line)
        if r[0] == "D" and r[1] == "scip_cross_project":
            cross.append(r[2:])
        if r[0] != "E" or r[6] != "scip":
            continue
        if r[3] == "DEPENDS_ON" and r[4] == "package" and "server/public@" in r[5]:
            deps[r[5]] += 1
        if r[7] != 3:
            continue
        outcome = set()
        for ev in r[9]:
            syms = sites.get((ev[0], ev[1][0], ev[1][1]), [])
            outcome.add("byte-equal" if r[5] in syms else ("other symbol" if syms else "no occurrence"))
        k = (r[3], "byte-equal" if outcome == {"byte-equal"} else "/".join(sorted(outcome)))
        c[k] += 1
        ex.setdefault(k, (r[5], r[9][0]))
    for k, v in sorted(c.items()):
        print(f"  {k[0]} {k[1]}: {v}  e.g. {ex[k][0][-90:]} at {ex[k][1]}")
    print(f"  scip_cross_project rows: {cross}")
    print(f"  scip DEPENDS_ON into server/public: {dict(deps)}")


SECTIONS = {
    "citations": section_citations, "record": section_record, "indexes": section_indexes,
    "measure": section_measure, "residue": section_residue, "agreement": section_agreement, "d8": section_d8,
}


def main():
    names = sys.argv[1:] or list(SECTIONS)
    for n in names:
        SECTIONS[n]()
        print(flush=True)


if __name__ == "__main__":
    main()
