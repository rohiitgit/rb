use serde_json::json;

use super::*;
use crate::{GraphBuilder, NewEdge, NodeKey, canonical_dump};

fn attrs(v: serde_json::Value) -> Attrs {
    v.as_object().cloned().unwrap()
}

/// The `N` and `E` lines of a graph's canonical dump.
fn dump_lines(g: &Graph) -> Vec<String> {
    canonical_dump("c", g, &[])
        .lines()
        .filter(|l| l.starts_with("[\"N\"") || l.starts_with("[\"E\""))
        .map(str::to_owned)
        .collect()
}

fn apply_to(a: &Graph, d: &GraphDiff) -> Result<Vec<String>, String> {
    let lines = dump_lines(a);
    let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
    apply(&refs, d)
}

/// A small graph: `f.ts` (a file), symbols `a`, `b`, a `CALLS` edge `a → b` with evidence.
fn base() -> GraphBuilder {
    let mut b = GraphBuilder::new();
    b.add_node(NodeKey::new(NodeKind::File, "f.ts"), "f.ts", Attrs::new())
        .unwrap();
    for s in ["a", "b"] {
        b.add_node(NodeKey::new(NodeKind::Symbol, s), s, Attrs::new())
            .unwrap();
    }
    b
}

fn calls(certainty: Certainty, line: i32, extra: Attrs) -> NewEdge {
    NewEdge::new(
        NodeKey::new(NodeKind::Symbol, "a"),
        EdgeKind::Calls,
        NodeKey::new(NodeKind::Symbol, "b"),
        Provenance::Heuristic,
        certainty,
    )
    .attrs(extra)
    .evidence(Evidence {
        path: "f.ts".into(),
        span: [line, 0, line, 3],
    })
}

fn graph(edits: impl FnOnce(&mut GraphBuilder)) -> Graph {
    let mut b = base();
    edits(&mut b);
    b.finish().unwrap()
}

/// Contract M5 S16: one row per section, each in its section only, and each applied.
#[test]
fn each_section_holds_its_row() {
    let a = graph(|b| {
        b.add_edge(calls(Certainty::Heuristic, 1, Attrs::new()))
            .unwrap()
    });
    // A node added and one removed.
    let b = graph(|b| {
        b.add_edge(calls(Certainty::Heuristic, 1, Attrs::new()))
            .unwrap();
        b.add_node(NodeKey::new(NodeKind::Table, "public.t"), "t", Attrs::new())
            .unwrap();
    });
    let d = diff(&a, &b);
    assert_eq!(d.nodes_added.len(), 1);
    assert_eq!(d.nodes_added[0].key, "public.t");
    assert!(d.nodes_removed.is_empty() && d.nodes_changed.is_empty() && d.edges_added.is_empty());
    assert_eq!(diff(&b, &a).nodes_removed.len(), 1);

    // An attrs-only node change.
    let changed = {
        let mut x = base();
        x.add_edge(calls(Certainty::Heuristic, 1, Attrs::new()))
            .unwrap();
        x.set_node_attr(
            &NodeKey::new(NodeKind::Symbol, "b"),
            "entry",
            json!(["main"]),
        );
        x.finish().unwrap()
    };
    let d = diff(&a, &changed);
    assert_eq!(d.nodes_changed.len(), 1, "{d:?}");
    assert_eq!(d.nodes_changed[0].before.attrs, Attrs::new());
    assert_eq!(
        d.nodes_changed[0].after.attrs,
        attrs(json!({"entry": ["main"]}))
    );
    assert!(d.edges_changed.is_empty() && d.edges_evidence_only.is_empty());

    // A certainty change (the evidence the same): `changed`, not evidence-only.
    let up = graph(|b| {
        b.add_edge(calls(Certainty::Exact, 1, Attrs::new()))
            .unwrap()
    });
    let d = diff(&a, &up);
    assert_eq!((d.edges_changed.len(), d.edges_evidence_only.len()), (1, 0));
    assert_eq!(
        (
            d.edges_changed[0].before.certainty,
            d.edges_changed[0].after.certainty
        ),
        (Certainty::Heuristic, Certainty::Exact)
    );
    // Attributes and evidence both changed: `changed`, with the whole rows.
    let both = graph(|b| {
        b.add_edge(calls(Certainty::Heuristic, 2, attrs(json!({"sites": 2}))))
            .unwrap()
    });
    let d = diff(&a, &both);
    assert_eq!(d.edges_changed.len(), 1);
    assert_ne!(
        d.edges_changed[0].before.evidence,
        d.edges_changed[0].after.evidence
    );
    // Only the evidence: evidence-only.
    let moved = graph(|b| {
        b.add_edge(calls(Certainty::Heuristic, 7, Attrs::new()))
            .unwrap()
    });
    let d = diff(&a, &moved);
    assert_eq!((d.edges_changed.len(), d.edges_evidence_only.len()), (0, 1));
    // An edge removed (and added the other way).
    let bare = graph(|_| {});
    let d = diff(&a, &bare);
    assert_eq!((d.edges_removed.len(), d.nodes_removed.len()), (1, 0));
    assert_eq!(diff(&bare, &a).edges_added.len(), 1);
    // Every one of these applies.
    for (x, y) in [
        (&a, &b),
        (&a, &changed),
        (&a, &up),
        (&a, &both),
        (&a, &moved),
        (&a, &bare),
    ] {
        assert_eq!(apply_to(x, &diff(x, y)).unwrap(), dump_lines(y));
    }
}

/// A diff applies only to the graph it came from: a stale before-row, a missing row or an
/// addition already present is refused.
#[test]
fn apply_checks_what_it_patches() {
    let a = graph(|b| {
        b.add_edge(calls(Certainty::Heuristic, 1, Attrs::new()))
            .unwrap()
    });
    let up = graph(|b| {
        b.add_edge(calls(Certainty::Exact, 1, Attrs::new()))
            .unwrap()
    });
    let d = diff(&a, &up);
    assert!(apply_to(&up, &d).is_err(), "applied to the wrong graph");
    assert!(apply_to(&a, &d).is_ok());
    let b = graph(|b| {
        b.add_node(NodeKey::new(NodeKind::Table, "public.t"), "t", Attrs::new())
            .unwrap()
    });
    let add = diff(&graph(|_| {}), &b);
    assert!(apply_to(&b, &add).is_err(), "an addition already present");
    assert!(
        apply_to(&graph(|_| {}), &add.invert()).is_err(),
        "a removal that is missing"
    );
}

#[test]
fn diagnostics_differ_as_multisets() {
    let d = |kind: &str, n: i64| Diagnostic {
        kind: kind.into(),
        path: Some("f.ts".into()),
        span: None,
        detail: json!({"n": n}),
    };
    let a = vec![d("x", 1), d("x", 1), d("y", 2)];
    let b = vec![d("x", 1), d("z", 3), d("y", 2)];
    let out = diagnostics_diff(&a, &b);
    assert_eq!(out.removed, vec![d("x", 1)]);
    assert_eq!(out.added, vec![d("z", 3)]);
    assert_eq!(
        out.counts,
        BTreeMap::from([
            ("x".to_owned(), (2, 1)),
            ("y".to_owned(), (1, 1)),
            ("z".to_owned(), (0, 1))
        ])
    );
    assert_eq!(
        diagnostics_diff(&a, &a),
        DiagnosticsDiff {
            counts: BTreeMap::from([("x".to_owned(), (2, 2)), ("y".to_owned(), (1, 1))]),
            ..Default::default()
        }
    );
}

/// A deterministic generator (splitmix64).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// One candidate universe, two graphs drawn from it: each keeps a random share of the candidate
/// nodes (every node kind; keys shared across kinds; three names, so duplicate names) and of the
/// candidate edges between kept nodes (every edge kind and provenance, self-loops and cycles),
/// with certainties, attributes and evidence drawn per graph, so the two overlap and differ in
/// every way the diff distinguishes.
fn universe(seed: u64, n: usize) -> (Graph, Graph) {
    let mut u = Rng(seed);
    let kinds = NodeKind::ALL;
    let mut nodes: Vec<NodeKey> = (0..n)
        .map(|_| {
            let kind = kinds[u.below(kinds.len() as u64) as usize];
            NodeKey::new(kind, format!("k{}", u.below(n as u64 / 2 + 1)))
        })
        .collect();
    // Evidence needs files: a few are always candidates.
    nodes.extend((0..3).map(|i| NodeKey::new(NodeKind::File, format!("src/f{i}.ts"))));
    nodes.sort();
    nodes.dedup();
    let edges: Vec<(usize, EdgeKind, usize, Provenance)> = (0..n * 2)
        .map(|_| {
            let s = u.below(nodes.len() as u64) as usize;
            // One edge in eight is a self-loop.
            let d = if u.below(8) == 0 {
                s
            } else {
                u.below(nodes.len() as u64) as usize
            };
            (
                s,
                EdgeKind::ALL[u.below(EdgeKind::ALL.len() as u64) as usize],
                d,
                Provenance::ALL[u.below(3) as usize],
            )
        })
        .collect();
    let draw = |seed: u64| -> Graph {
        let mut r = Rng(seed);
        let mut b = GraphBuilder::new();
        let kept: Vec<bool> = nodes
            .iter()
            .map(|k| k.kind == NodeKind::File || r.below(5) != 0)
            .collect();
        for (k, keep) in nodes.iter().zip(&kept) {
            if *keep {
                let mut a = Attrs::new();
                if r.below(3) == 0 {
                    a.insert("v".into(), r.below(2).into());
                }
                b.add_node(k.clone(), format!("name{}", r.below(3)), a)
                    .unwrap();
            }
        }
        let files: Vec<&NodeKey> = nodes.iter().filter(|k| k.kind == NodeKind::File).collect();
        for &(s, kind, d, provenance) in &edges {
            if !(kept[s] && kept[d]) || r.below(4) == 0 {
                continue;
            }
            let certainty = [Certainty::Heuristic, Certainty::Exact][r.below(2) as usize];
            let mut a = Attrs::new();
            if r.below(4) == 0 {
                a.insert("sites".into(), (r.below(2) + 1).into());
            }
            let mut e = NewEdge::new(
                nodes[s].clone(),
                kind,
                nodes[d].clone(),
                provenance,
                certainty,
            )
            .attrs(a);
            for _ in 0..r.below(3) {
                let line = r.below(3) as i32;
                e = e.evidence(Evidence {
                    path: files[r.below(files.len() as u64) as usize].key.clone(),
                    span: [line, 0, line, 1],
                });
            }
            // The same identity drawn twice in one graph must agree (the builder merges).
            let _ = b.add_edge(e);
        }
        b.finish().unwrap()
    };
    (draw(seed ^ 0xa5a5), draw(seed ^ 0x5a5a))
}

/// Contract M5 S16's properties on random graphs: `apply(A, diff(A, B)) == B` line for line,
/// `diff(A, A)` empty, `diff(B, A) == invert(diff(A, B))`; and every section is exercised.
#[test]
fn diff_applies_inverts_and_is_empty_on_itself() {
    let mut seen = [0usize; 7];
    for seed in 0..200u64 {
        let n = 4 + (seed as usize % 40);
        let (a, b) = universe(seed, n);
        let d = diff(&a, &b);
        assert_eq!(apply_to(&a, &d).unwrap(), dump_lines(&b), "seed {seed}");
        assert!(
            diff(&a, &a).is_empty() && diff(&b, &b).is_empty(),
            "seed {seed}"
        );
        assert_eq!(diff(&b, &a), d.invert(), "seed {seed}");
        assert_eq!(
            apply_to(&b, &d.invert()).unwrap(),
            dump_lines(&a),
            "seed {seed}"
        );
        for (i, len) in [
            d.nodes_added.len(),
            d.nodes_removed.len(),
            d.nodes_changed.len(),
            d.edges_added.len(),
            d.edges_removed.len(),
            d.edges_changed.len(),
            d.edges_evidence_only.len(),
        ]
        .into_iter()
        .enumerate()
        {
            seen[i] += len;
        }
    }
    assert!(
        seen.iter().all(|&n| n > 0),
        "every section exercised: {seen:?}"
    );
}

/// Invariant 10: the diff's counted steps grow linearly: at most 5× for 4× the input.
#[test]
fn diff_grows_linearly() {
    let steps = |n: usize| {
        let (a, b) = universe(7, n);
        count::take();
        let d = diff(&a, &b);
        assert!(!d.is_empty());
        let s = count::take();
        let size = (a.nodes().len() + a.edges().len() + b.nodes().len() + b.edges().len()) as u64;
        (s, size)
    };
    let (small, small_size) = steps(1_000);
    let (large, large_size) = steps(4_000);
    // The inputs themselves grow about 4×; the steps at most 5× theirs.
    assert!(large_size >= 3 * small_size, "{small_size} -> {large_size}");
    eprintln!("diff steps: {small} -> {large} for {small_size} -> {large_size} rows");
    assert!(
        large <= small * 5,
        "{small} -> {large} steps for {small_size} -> {large_size} rows"
    );
}
