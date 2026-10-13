//! SCIP agreement (contract M8a, S4): the `scip`-provenance `CALLS` of a commit indexed with
//! SCIP, against the same commit indexed with `--no-scip`, both by location identity. This is
//! agreement of Tier B with Tier A, never accuracy: the reference goes through rb's own SCIP join
//! and covers only the call sites rb's parser found (`scip.rs:507`).

use std::collections::BTreeSet;

use rb_model::{EdgeKind, NodeKind, Provenance};

use crate::dump::Dump;
use crate::identity::{Class, EdgeId, Locator, Side, found};
use crate::score::{Cell, Scope, score};
use crate::scoreboard::Covered;

/// The scope reason for an edge with an endpoint in a file SCIP did not cover.
pub const NOT_COVERED: &str = "not_covered";

/// The classes this key scores.
pub const CLASSES: [Class; 2] = [Class::Calls, Class::CallsFileScope];

/// Files whose symbols carry SCIP keys in the SCIP-mode dump (SCIP replaces a covered file's
/// heuristic keys wholesale), and every file that defines a symbol.
pub fn coverage(scip: &Dump) -> (BTreeSet<&str>, BTreeSet<&str>) {
    let mut covered = BTreeSet::new();
    let mut all = BTreeSet::new();
    for e in &scip.edges {
        if e.kind != EdgeKind::DefinedIn || e.src_kind != NodeKind::Symbol {
            continue;
        }
        all.insert(e.dst_key.as_str());
        if e.src_key.starts_with("scip-") {
            covered.insert(e.dst_key.as_str());
        }
    }
    (covered, all)
}

/// The cell of one class: gold from the SCIP dump's `scip` rows, found from the `noscip` dump,
/// both judged only where source and target lie in covered files.
pub fn agreement(scip: &Dump, noscip: &Dump, class: Class) -> (Cell, Covered) {
    let (covered, all) = coverage(scip);
    let reference = Dump {
        commit: scip.commit.clone(),
        nodes: scip.nodes.clone(),
        edges: scip
            .edges
            .iter()
            .filter(|e| e.kind != EdgeKind::Calls || e.provenance == Provenance::Scip)
            .cloned()
            .collect(),
    };
    let locator = Locator::new(&reference);
    let gold: Side<_> = found(&reference, &locator, class);
    // SCIP may merge declarations Tier B keeps apart (a TS static and instance method of one
    // name): each Tier B endpoint stands for the SCIP symbol it is an alias of.
    let raw = found(noscip, &Locator::new(noscip), class);
    let mut found = Side {
        located: Default::default(),
        unmatchable: raw.unmatchable,
    };
    for (id, f) in raw.located {
        let id = EdgeId {
            src: locator.canonical(&id.src),
            class: id.class,
            dst: locator.canonical(&id.dst),
        };
        found.insert(id, f);
    }
    let scope = |id: &EdgeId| {
        if covered.contains(id.src.path.as_str()) && covered.contains(id.dst.path.as_str()) {
            Scope::In
        } else {
            Scope::Out(NOT_COVERED)
        }
    };
    let cell = score(&gold, &found, scope);
    let covered = Covered {
        files: covered.len() as u64,
        of: all.len() as u64,
    };
    (cell, covered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dump::parse;
    use rb_model::Certainty;

    const HEADER: &str = "[\"rb-dump\",2,\"x\"]";

    fn node(key: &str, name: &str) -> String {
        format!(r#"["N","symbol",{},"{name}",{{}}]"#, serde_json::json!(key))
    }

    fn defined(key: &str, path: &str, line: i32) -> String {
        format!(
            r#"["E","symbol",{},"DEFINED_IN","file","{path}","treesitter",5,{{}},[["{path}",[{line},0,{line},1]]]]"#,
            serde_json::json!(key)
        )
    }

    fn calls(src_kind: &str, src: &str, dst: &str, prov: &str, cert: i32, path: &str) -> String {
        format!(
            r#"["E","{src_kind}",{},"CALLS","symbol",{},"{prov}",{cert},{{}},[["{path}",[9,0,9,1]]]]"#,
            serde_json::json!(src),
            serde_json::json!(dst)
        )
    }

    fn dump(rows: &[String]) -> Dump {
        parse(&format!("{HEADER}\n{}\n", rows.join("\n"))).unwrap()
    }

    /// `a.go` is covered (its symbols carry SCIP keys), `b.go` is not. The SCIP dump calls
    /// f→g and file→g exactly; it also has a heuristic row (b.go's h→g) that is no reference.
    fn pair() -> (Dump, Dump) {
        let (f, g, h) = (
            "scip-go gomod x . a/f().",
            "scip-go gomod x . a/g().",
            "rb-heuristic . . . `b.go`/h().",
        );
        let scip = dump(&[
            node(f, "f"),
            node(g, "g"),
            node(h, "h"),
            defined(f, "a.go", 1),
            defined(g, "a.go", 5),
            defined(h, "b.go", 1),
            calls("symbol", f, g, "scip", 5, "a.go"),
            calls("file", "a.go", g, "scip", 5, "a.go"),
            calls("symbol", h, g, "heuristic", 2, "b.go"),
        ]);
        let (hf, hg, hh, hx) = (
            "rb-heuristic . . . `a.go`/f().",
            "rb-heuristic . . . `a.go`/g().",
            "rb-heuristic . . . `b.go`/h().",
            "rb-heuristic . . . `a.go`/x().",
        );
        let noscip = dump(&[
            node(hf, "f"),
            node(hg, "g"),
            node(hh, "h"),
            node(hx, "x"),
            defined(hf, "a.go", 1),
            defined(hg, "a.go", 5),
            defined(hh, "b.go", 1),
            defined(hx, "a.go", 7),
            calls("symbol", hf, hg, "heuristic", 2, "a.go"),
            calls("symbol", hf, hx, "heuristic", 1, "a.go"),
            calls("symbol", hh, hg, "heuristic", 2, "b.go"),
            calls("symbol", hf, hh, "heuristic", 2, "a.go"),
        ]);
        (scip, noscip)
    }

    #[test]
    fn coverage_is_read_from_scip_keys() {
        let (scip, _) = pair();
        let (covered, all) = coverage(&scip);
        assert_eq!(covered, BTreeSet::from(["a.go"]));
        assert_eq!(all, BTreeSet::from(["a.go", "b.go"]));
    }

    #[test]
    fn symbol_calls_agree_across_key_schemes() {
        let (scip, noscip) = pair();
        let (c, cov) = agreement(&scip, &noscip, Class::Calls);
        assert_eq!(cov, Covered { files: 1, of: 2 });
        assert_eq!(
            c.gold, 1,
            "the heuristic row of the SCIP dump is no reference"
        );
        assert_eq!(c.tp_total(), 1);
        assert_eq!(c.tp.get(&Certainty::Heuristic), Some(&1));
        assert_eq!(
            c.fp.get(&Certainty::Ambiguous),
            Some(&1),
            "f→x is not in SCIP"
        );
        assert_eq!(c.fn_, 0);
        assert_eq!(
            c.found_out_of_scope.get(NOT_COVERED),
            Some(&2),
            "h→g starts in b.go"
        );
    }

    #[test]
    fn file_scope_calls_are_their_own_cell() {
        let (scip, noscip) = pair();
        let (c, _) = agreement(&scip, &noscip, Class::CallsFileScope);
        assert_eq!((c.gold, c.fn_, c.found()), (1, 1, 0));
    }

    #[test]
    fn no_coverage_is_vacuous() {
        let (_, noscip) = pair();
        let (c, cov) = agreement(&noscip, &noscip, Class::Calls);
        assert_eq!(cov, Covered { files: 0, of: 2 });
        assert_eq!(c.status(), crate::score::Status::Vacuous);
    }

    #[test]
    fn a_merged_scip_declaration_absorbs_tier_b_overloads() {
        // SCIP merges a static and an instance `hashPassword` (sites at lines 11 and 62) into one
        // symbol; Tier B keeps two and calls the second. That is one agreeing edge, not fp + fn.
        let (x, h) = (
            "scip-typescript npm p 1 a/x().",
            "scip-typescript npm p 1 a/User#hashPassword().",
        );
        let scip = dump(&[
            node(x, "x"),
            node(h, "hashPassword"),
            defined(x, "a.ts", 1),
            format!(
                r#"["E","symbol",{},"DEFINED_IN","file","a.ts","treesitter",5,{{}},[["a.ts",[10,0,10,1]],["a.ts",[61,0,61,1]]]]"#,
                serde_json::json!(h)
            ),
            calls("symbol", x, h, "scip", 5, "a.ts"),
        ]);
        let (hx, hs, hi) = (
            "rb-heuristic . . . `a.ts`/x().",
            "rb-heuristic . . . `a.ts`/User#hashPassword().",
            "rb-heuristic . . . `a.ts`/User#hashPassword(+1).",
        );
        let noscip = dump(&[
            node(hx, "x"),
            node(hs, "hashPassword"),
            node(hi, "hashPassword"),
            defined(hx, "a.ts", 1),
            defined(hs, "a.ts", 10),
            defined(hi, "a.ts", 61),
            calls("symbol", hx, hi, "heuristic", 2, "a.ts"),
        ]);
        let (c, _) = agreement(&scip, &noscip, Class::Calls);
        assert_eq!((c.tp_total(), c.fp_total(), c.fn_), (1, 0, 0));
    }
}
