//! Snapshot-lifecycle tests against the compose database (contract M5 S1–S3). They need
//! `RB_TEST_DATABASE_URL` (as the e2e suite does) and print `skipping:` without it; they share one
//! lock, since they create and drop AGE graphs, and every repository they make is named `t_store_*`
//! and deleted when the test ends.

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use rb_model::{
    Certainty, Diagnostic, EdgeKind, Evidence, Graph, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance, canonical_dump,
};
use serde_json::json;

use crate::{LoadDir, Store};

pub(crate) struct Db {
    pub url: String,
    _guard: MutexGuard<'static, ()>,
}

pub(crate) fn db() -> Option<Db> {
    static LOCK: Mutex<()> = Mutex::new(());
    let Ok(url) = std::env::var("RB_TEST_DATABASE_URL") else {
        eprintln!("skipping: set RB_TEST_DATABASE_URL to run database tests");
        return None;
    };
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut store = Store::connect(&url).unwrap();
    store.migrate().unwrap();
    Some(Db { url, _guard: guard })
}

pub(crate) fn load_dir() -> LoadDir {
    LoadDir::new(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../var/age-load"))
}

/// `t_store_<tag>_<pid>`: unique per test process, and recognisable as a test repository.
pub(crate) fn repo_name(tag: &str) -> String {
    format!("t_store_{tag}_{}", std::process::id())
}

/// Deletes the named repositories when dropped, even after a failed assertion.
pub(crate) struct Cleanup<'a> {
    pub url: &'a str,
    pub repos: Vec<String>,
}

impl Drop for Cleanup<'_> {
    fn drop(&mut self) {
        if let Ok(mut store) = Store::connect(self.url) {
            for r in &self.repos {
                let _ = store.delete_repo(r);
            }
        }
    }
}

/// A small graph: `files` files, one function per file, `DEFINED_IN` and a `CALLS` chain (with a
/// cycle back to the first), each edge evidenced; `variant` changes one name so two graphs differ.
pub(crate) fn sample_graph(files: usize, variant: u32) -> Graph {
    let mut b = GraphBuilder::new();
    let file = |i: usize| NodeKey::new(NodeKind::File, format!("src/f{i}.ts"));
    let sym = |i: usize| NodeKey::new(NodeKind::Symbol, format!("sym f{i}"));
    for i in 0..files {
        b.add_node(file(i), format!("f{i}.ts"), Default::default())
            .unwrap();
        b.add_node(sym(i), format!("fn{i}_{variant}"), Default::default())
            .unwrap();
        let ev = Evidence {
            path: format!("src/f{i}.ts"),
            span: [1, 0, 1, 10],
        };
        b.add_edge(
            NewEdge::new(
                sym(i),
                EdgeKind::DefinedIn,
                file(i),
                Provenance::Treesitter,
                Certainty::Exact,
            )
            .evidence(ev.clone()),
        )
        .unwrap();
        b.add_edge(
            NewEdge::new(
                sym(i),
                EdgeKind::Calls,
                sym((i + 1) % files),
                Provenance::Heuristic,
                Certainty::Heuristic,
            )
            .evidence(ev),
        )
        .unwrap();
    }
    b.finish().unwrap()
}

pub(crate) fn sample_diagnostics() -> Vec<Diagnostic> {
    vec![Diagnostic {
        kind: "unresolved_call".into(),
        path: Some("src/f0.ts".into()),
        span: Some([2, 0, 2, 4]),
        detail: json!({"callee": "x"}),
    }]
}

pub(crate) fn dump_of(store: &mut Store, id: i64) -> anyhow::Result<String> {
    store.read_snapshot(id, |s| {
        let snap = s.snapshot(id)?;
        Ok(canonical_dump(
            &snap.commit_sha,
            &s.load_graph(id)?,
            &s.load_diagnostics(id)?,
        ))
    })
}

/// Contract M5 S1: a reader that has read a snapshot's nodes when another connection deletes the
/// snapshot still reads its edges, evidence and diagnostics whole; a reader that starts after the
/// delete gets "does not exist". Before M5, `load_graph` read nodes, edges and evidence in three
/// statements outside any transaction, and this read returned the nodes with no edges.
#[test]
fn a_snapshot_deleted_during_a_read_is_read_whole() {
    let Some(db) = db() else { return };
    let repo = repo_name("s1");
    let _cleanup = Cleanup {
        url: &db.url,
        repos: vec![repo.clone()],
    };
    let mut writer = Store::connect(&db.url).unwrap();
    let graph = sample_graph(6, 0);
    let diagnostics = sample_diagnostics();
    let snap = writer
        .publish_snapshot(
            &repo,
            &"a".repeat(40),
            &json!({}),
            &graph,
            &diagnostics,
            &load_dir(),
        )
        .unwrap();
    let expected = canonical_dump(&snap.commit_sha, &graph, &diagnostics);
    assert_eq!(dump_of(&mut writer, snap.id).unwrap(), expected);

    let deleted = std::rc::Rc::new(std::cell::Cell::new(false));
    let mut reader = Store::connect(&db.url).unwrap();
    let read = crate::hooks::with_after_nodes(
        {
            let (deleted, repo) = (deleted.clone(), repo.clone());
            move || {
                assert!(writer.delete_repo(&repo).unwrap());
                deleted.set(true);
            }
        },
        || dump_of(&mut reader, snap.id),
    )
    .unwrap();
    assert!(
        deleted.get(),
        "the hook ran between the node and edge reads"
    );
    assert_eq!(
        read, expected,
        "the concurrent delete is invisible to the read"
    );

    let err = dump_of(&mut reader, snap.id).unwrap_err();
    assert!(
        format!("{err:#}").contains(&format!("snapshot {} does not exist", snap.id)),
        "{err:#}"
    );
}

/// Contract M5 S1: a whole command's reads (the live pointer, then the snapshot) share one
/// transaction, and a read transaction refuses writes.
#[test]
fn a_read_transaction_is_one_snapshot_and_read_only() {
    let Some(db) = db() else { return };
    let repo = repo_name("s1tx");
    let _cleanup = Cleanup {
        url: &db.url,
        repos: vec![repo.clone()],
    };
    let mut writer = Store::connect(&db.url).unwrap();
    let first = writer
        .publish_snapshot(
            &repo,
            &"a".repeat(40),
            &json!({}),
            &sample_graph(3, 0),
            &[],
            &load_dir(),
        )
        .unwrap();
    let mut reader = Store::connect(&db.url).unwrap();
    let (live, nodes) = reader
        .read_tx(|s| {
            let live = s.live_snapshot(&repo)?.unwrap();
            // Another publisher flips the pointer while this read is open.
            let second = writer
                .publish_snapshot(
                    &repo,
                    &"b".repeat(40),
                    &json!({}),
                    &sample_graph(4, 1),
                    &[],
                    &load_dir(),
                )
                .unwrap();
            assert_ne!(second.id, live.id);
            let again = s.live_snapshot(&repo)?.unwrap();
            assert_eq!(
                again.id, live.id,
                "the pointer read twice in one transaction"
            );
            let nodes = s.load_graph(live.id)?.nodes().len();
            Ok((live, nodes))
        })
        .unwrap();
    assert_eq!((live.id, nodes), (first.id, 6));
    let err = reader
        .read_tx(|s| s.delete_repo(&repo).map(|_| ()))
        .unwrap_err();
    assert!(format!("{err:#}").contains("read-only"), "{err:#}");
    // The connection is usable afterwards.
    assert_ne!(reader.live_snapshot(&repo).unwrap().unwrap().id, first.id);
}

/// Contract M5 S2: graph DDL waits for the database-wide lock; a publish and a repository delete
/// block while another session holds it, and finish once it is released.
#[test]
fn graph_ddl_waits_for_the_ddl_lock() {
    let Some(db) = db() else { return };
    let repo = repo_name("s2");
    let _cleanup = Cleanup {
        url: &db.url,
        repos: vec![repo.clone()],
    };
    let mut holder = postgres::Client::connect(&db.url, postgres::NoTls).unwrap();
    let waiters = |c: &mut postgres::Client| -> i64 {
        c.query_one(
            "SELECT count(*) FROM pg_locks WHERE locktype = 'advisory' AND NOT granted
               AND ((classid::bigint << 32) | objid::bigint) = $1",
            &[&crate::RB_AGE_DDL_LOCK],
        )
        .unwrap()
        .get(0)
    };
    for step in ["publish", "delete"] {
        holder
            .execute("SELECT pg_advisory_lock($1)", &[&crate::RB_AGE_DDL_LOCK])
            .unwrap();
        let url = db.url.clone();
        let r = repo.clone();
        let worker = std::thread::spawn(move || {
            let mut store = Store::connect(&url).unwrap();
            match step {
                "publish" => store
                    .publish_snapshot(
                        &r,
                        &"a".repeat(40),
                        &json!({}),
                        &sample_graph(2, 0),
                        &[],
                        &load_dir(),
                    )
                    .map(|_| ()),
                _ => store.delete_repo(&r).map(|_| ()),
            }
        });
        let mut waited = false;
        for _ in 0..600 {
            if waiters(&mut holder) > 0 {
                waited = true;
                break;
            }
            assert!(
                !worker.is_finished(),
                "{step} finished while the lock was held"
            );
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(waited, "{step} never waited for the DDL lock");
        assert!(
            !worker.is_finished(),
            "{step} finished while the lock was held"
        );
        holder
            .execute("SELECT pg_advisory_unlock($1)", &[&crate::RB_AGE_DDL_LOCK])
            .unwrap();
        worker.join().unwrap().unwrap();
    }
}
