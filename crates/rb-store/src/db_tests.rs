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
        // A test that failed half-way may leave a graph no row names any more.
        if let Ok(mut c) = postgres::Client::connect(self.url, postgres::NoTls) {
            for r in &self.repos {
                let pattern = format!("^rb_{r}_[0-9]+$");
                let Ok(rows) = c.query(
                    "SELECT name::text FROM ag_catalog.ag_graph WHERE name::text ~ $1",
                    &[&pattern],
                ) else {
                    continue;
                };
                for row in rows {
                    let name: String = row.get(0);
                    let _ = c.transaction().and_then(|mut tx| {
                        tx.execute(
                            "SELECT pg_advisory_xact_lock($1)",
                            &[&crate::RB_AGE_DDL_LOCK],
                        )?;
                        tx.execute(
                            "SELECT ag_catalog.drop_graph($1::text::name, true)",
                            &[&name],
                        )?;
                        tx.commit()
                    });
                }
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
        .unwrap()
        .snapshot;
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
        .unwrap()
        .snapshot;
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
                .unwrap()
                .snapshot;
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

/// The rows of `repo`'s snapshots by status, the pointer, and its `rb_<repo>_<n>` graphs.
#[derive(Debug, PartialEq)]
struct RepoState {
    live_pointer: Option<i64>,
    live: Vec<i64>,
    retired: Vec<i64>,
    failed: usize,
    building: usize,
    /// Snapshots other than the pointer's that still name a graph.
    stray_age_graphs: Vec<i64>,
    graphs: Vec<String>,
}

fn repo_state(c: &mut postgres::Client, repo: &str) -> RepoState {
    let pointer: Option<i64> = c
        .query_opt(
            "SELECT live_snapshot_id FROM rb.repo WHERE name = $1",
            &[&repo],
        )
        .unwrap()
        .and_then(|r| r.get(0));
    let rows = c
        .query(
            "SELECT s.id, s.status, s.age_graph FROM rb.snapshot s JOIN rb.repo r ON r.id = s.repo_id
             WHERE r.name = $1 ORDER BY s.id",
            &[&repo],
        )
        .unwrap();
    let ids = |status: &str| -> Vec<i64> {
        rows.iter()
            .filter(|r| r.get::<_, String>(1) == status)
            .map(|r| r.get(0))
            .collect()
    };
    let pattern = format!("^rb_{repo}_[0-9]+$");
    let graphs = c
        .query(
            "SELECT name::text FROM ag_catalog.ag_graph WHERE name::text ~ $1 ORDER BY 1",
            &[&pattern],
        )
        .unwrap()
        .iter()
        .map(|r| r.get(0))
        .collect();
    RepoState {
        live_pointer: pointer,
        live: ids("live"),
        retired: ids("retired"),
        failed: ids("failed").len(),
        building: ids("building").len(),
        stray_age_graphs: rows
            .iter()
            .filter(|r| Some(r.get::<_, i64>(0)) != pointer)
            .filter(|r| r.get::<_, Option<String>>(2).is_some())
            .map(|r| r.get(0))
            .collect(),
        graphs,
    }
}

/// A fixed-seed generator (xorshift64*), so a failing sequence is reproducible.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) % n
    }
}

/// Contract M5 S3 (property): random sequences of publishes, failed publishes (an error injected
/// after the COPY) and `--keep` changes over two repositories. After every step, for each
/// repository: the pointer's snapshot exists and is `live`, the retired rows are exactly the
/// `N` newest (by the keep in force at the last publish), `failed` rows exist only since the last
/// publish, no `building` row is visible, and `ag_catalog.ag_graph` holds exactly one
/// `rb_<repo>_*` graph, the live snapshot's (no retired row names a graph).
#[test]
fn retention_holds_over_random_sequences() {
    let Some(db) = db() else { return };
    let repos = [repo_name("pa"), repo_name("pb")];
    let _cleanup = Cleanup {
        url: &db.url,
        repos: repos.to_vec(),
    };
    #[derive(Default)]
    struct Model {
        live: Option<i64>,
        retired: Vec<i64>,
        failed: usize,
    }
    let mut models = [Model::default(), Model::default()];
    let mut store = Store::connect(&db.url).unwrap();
    let mut probe = postgres::Client::connect(&db.url, postgres::NoTls).unwrap();
    let mut rng = Rng(0x5eed_2026_1007);
    let mut keep = crate::KEEP_RETIRED;
    let mut ops = [0usize; 3];
    for step in 0..48u32 {
        let r = rng.below(2) as usize;
        let repo = &repos[r];
        let op = rng.below(4);
        let graph = sample_graph(2 + (step % 3) as usize, step);
        match op {
            0 | 1 => {
                let published = store
                    .publish_snapshot(
                        repo,
                        &format!("{step:040}"),
                        &json!({"step": step}),
                        &graph,
                        &[],
                        &load_dir(),
                    )
                    .unwrap();
                let m = &mut models[r];
                m.retired.extend(m.live);
                m.live = Some(published.snapshot.id);
                let n = m.retired.len().saturating_sub(keep as usize);
                m.retired.drain(..n);
                m.failed = 0;
                ops[0] += 1;
            }
            2 => {
                let err = crate::hooks::with_after_copy(
                    |_pid| anyhow::bail!("injected failure"),
                    || {
                        store.publish_snapshot(
                            repo,
                            &format!("{step:040}"),
                            &json!({"step": step}),
                            &graph,
                            &[],
                            &load_dir(),
                        )
                    },
                )
                .unwrap_err();
                assert!(format!("{err:#}").contains("injected failure"), "{err:#}");
                models[r].failed += 1;
                ops[1] += 1;
            }
            _ => {
                keep = rng.below(4) as i64;
                store.set_keep_retired(keep).unwrap();
                ops[2] += 1;
            }
        }
        for (repo, m) in repos.iter().zip(&models) {
            let state = repo_state(&mut probe, repo);
            let expected = RepoState {
                live_pointer: m.live,
                live: m.live.into_iter().collect(),
                retired: m.retired.clone(),
                failed: m.failed,
                building: 0,
                stray_age_graphs: vec![],
                graphs: m
                    .live
                    .map(|id| crate::graph_name(repo, id))
                    .into_iter()
                    .collect(),
            };
            assert_eq!(
                state, expected,
                "step {step} ({repo}, op {op}, keep {keep})"
            );
        }
    }
    assert!(
        ops.iter().all(|&n| n >= 5),
        "every operation exercised: {ops:?}"
    );
}

/// Contract M5 S3: the snapshot row is written inside the publish transaction, so a publisher
/// whose connection dies between the COPY and the commit leaves no row and no graph (before M5
/// the `building` row was committed first and survived).
#[test]
fn a_publisher_killed_before_commit_leaves_no_row() {
    let Some(db) = db() else { return };
    let repo = repo_name("crash");
    let _cleanup = Cleanup {
        url: &db.url,
        repos: vec![repo.clone()],
    };
    let mut store = Store::connect(&db.url).unwrap();
    let url = db.url.clone();
    let err = crate::hooks::with_after_copy(
        move |pid| {
            let mut killer = postgres::Client::connect(&url, postgres::NoTls)?;
            killer.execute("SELECT pg_terminate_backend($1)", &[&pid])?;
            // Wait until the server has ended the backend (and rolled back its transaction).
            while killer
                .query_opt("SELECT 1 FROM pg_stat_activity WHERE pid = $1", &[&pid])?
                .is_some()
            {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Ok(())
        },
        || {
            store.publish_snapshot(
                &repo,
                &"c".repeat(40),
                &json!({}),
                &sample_graph(3, 0),
                &[],
                &load_dir(),
            )
        },
    )
    .unwrap_err();
    let mut probe = postgres::Client::connect(&db.url, postgres::NoTls).unwrap();
    let state = repo_state(&mut probe, &repo);
    assert_eq!(
        (
            state.live.len(),
            state.retired.len(),
            state.failed,
            state.building
        ),
        (0, 0, 0, 0),
        "{state:?} after {err:#}"
    );
    assert!(state.graphs.is_empty(), "{state:?}");
}

/// Contract M5 S3: a failed publish leaves the live snapshot as it was and records one `failed`
/// row with the attempt's commit and tools, written after the rollback; the next publish's
/// retention removes it.
#[test]
fn a_failed_publish_records_a_failed_row() {
    let Some(db) = db() else { return };
    let repo = repo_name("failed");
    let _cleanup = Cleanup {
        url: &db.url,
        repos: vec![repo.clone()],
    };
    let mut store = Store::connect(&db.url).unwrap();
    let first = store
        .publish_snapshot(
            &repo,
            &"a".repeat(40),
            &json!({"t": 1}),
            &sample_graph(2, 0),
            &[],
            &load_dir(),
        )
        .unwrap()
        .snapshot;
    crate::hooks::with_after_copy(
        |_| anyhow::bail!("injected failure"),
        || {
            store.publish_snapshot(
                &repo,
                &"b".repeat(40),
                &json!({"t": 2}),
                &sample_graph(3, 1),
                &[],
                &load_dir(),
            )
        },
    )
    .unwrap_err();
    let mut probe = postgres::Client::connect(&db.url, postgres::NoTls).unwrap();
    let failed = probe
        .query(
            "SELECT s.commit_sha, s.tool_versions, s.parent_id, s.age_graph FROM rb.snapshot s
             JOIN rb.repo r ON r.id = s.repo_id WHERE r.name = $1 AND s.status = 'failed'",
            &[&repo],
        )
        .unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].get::<_, String>(0), "b".repeat(40));
    assert_eq!(failed[0].get::<_, serde_json::Value>(1), json!({"t": 2}));
    assert_eq!(failed[0].get::<_, Option<i64>>(2), Some(first.id));
    assert_eq!(failed[0].get::<_, Option<String>>(3), None);
    let state = repo_state(&mut probe, &repo);
    assert_eq!(state.live, vec![first.id]);
    assert_eq!(state.graphs, vec![crate::graph_name(&repo, first.id)]);
    store
        .publish_snapshot(
            &repo,
            &"c".repeat(40),
            &json!({"t": 3}),
            &sample_graph(2, 2),
            &[],
            &load_dir(),
        )
        .unwrap();
    assert_eq!(repo_state(&mut probe, &repo).failed, 0);
}

/// Contract M5 S3: the snapshot the repository's pointer names is never selected, whatever its
/// status says: a pointer to a row marked `retired` keeps the row and its graph, and retention
/// reports it.
#[test]
fn retention_never_selects_the_live_snapshot() {
    let Some(db) = db() else { return };
    let repo = repo_name("pointer");
    let _cleanup = Cleanup {
        url: &db.url,
        repos: vec![repo.clone()],
    };
    let mut store = Store::connect(&db.url).unwrap();
    store.set_keep_retired(0).unwrap();
    let mut last = None;
    for i in 0..3u32 {
        last = Some(
            store
                .publish_snapshot(
                    &repo,
                    &format!("{i:040}"),
                    &json!({}),
                    &sample_graph(2, i),
                    &[],
                    &load_dir(),
                )
                .unwrap()
                .snapshot,
        );
    }
    let live = last.unwrap();
    let mut probe = postgres::Client::connect(&db.url, postgres::NoTls).unwrap();
    probe
        .execute(
            "UPDATE rb.snapshot SET status = 'retired' WHERE id = $1",
            &[&live.id],
        )
        .unwrap();
    let report = store.apply_retention(&repo).unwrap();
    assert_eq!(report.live_marked_retired, Some(live.id));
    assert!(report.snapshots_deleted.is_empty() && report.graphs_dropped.is_empty());
    let state = repo_state(&mut probe, &repo);
    assert_eq!(state.live_pointer, Some(live.id));
    assert_eq!(state.retired, vec![live.id]);
    assert_eq!(state.graphs, vec![crate::graph_name(&repo, live.id)]);
    // The row delete has its own guard: with no graph named, the row is still kept.
    probe
        .execute(
            "UPDATE rb.snapshot SET age_graph = NULL WHERE id = $1",
            &[&live.id],
        )
        .unwrap();
    let report = store.apply_retention(&repo).unwrap();
    assert_eq!(report.live_marked_retired, Some(live.id));
    assert!(report.snapshots_deleted.is_empty(), "{report:?}");
    probe
        .execute(
            "UPDATE rb.snapshot SET age_graph = $2 WHERE id = $1",
            &[&live.id, &crate::graph_name(&repo, live.id)],
        )
        .unwrap();
    assert!(store.set_keep_retired(21).is_err() && store.set_keep_retired(-1).is_err());
    store.set_keep_retired(20).unwrap();
}

/// A scratch database (`t_store_db_<pid>_<tag>`, migrated) for operations that act on the whole
/// database, such as `rb cache prune`; dropped, with its connections, when this value is.
struct ScratchDb {
    admin: String,
    name: String,
    url: String,
}

impl ScratchDb {
    fn create(base_url: &str, tag: &str) -> ScratchDb {
        let name = format!("t_store_db_{}_{tag}", std::process::id());
        let mut admin = postgres::Client::connect(base_url, postgres::NoTls).unwrap();
        admin
            .batch_execute(&format!("DROP DATABASE IF EXISTS {name} WITH (FORCE)"))
            .unwrap();
        admin
            .batch_execute(&format!("CREATE DATABASE {name}"))
            .unwrap();
        let (head, _) = base_url.rsplit_once('/').unwrap();
        let url = format!("{head}/{name}");
        Store::connect(&url).unwrap().migrate().unwrap();
        ScratchDb {
            admin: base_url.to_owned(),
            name,
            url,
        }
    }
}

impl Drop for ScratchDb {
    fn drop(&mut self) {
        if let Ok(mut admin) = postgres::Client::connect(&self.admin, postgres::NoTls) {
            let _ = admin.batch_execute(&format!(
                "DROP DATABASE IF EXISTS {} WITH (FORCE)",
                self.name
            ));
        }
    }
}

/// Contract M5 S4: `rb gc` lists, and then removes, a `building` row left by an older binary, an
/// AGE graph no snapshot row names, and a graph a retired snapshot still names (a pre-M5
/// retention); never the live snapshot's graph; one graph per transaction under the DDL lock.
/// `--dry-run` removes nothing; a second run finds nothing. Scoped by repository prefix, as the
/// e2e start-up clean-up uses it.
#[test]
fn gc_removes_stale_rows_and_graphs_but_never_the_live_graph() {
    let Some(db) = db() else { return };
    let prefix = format!("t_store_gc_{}_", std::process::id());
    let repo = format!("{prefix}a");
    let _cleanup = Cleanup {
        url: &db.url,
        repos: vec![repo.clone()],
    };
    let mut store = Store::connect(&db.url).unwrap();
    let first = store
        .publish_snapshot(
            &repo,
            &"a".repeat(40),
            &json!({}),
            &sample_graph(2, 0),
            &[],
            &load_dir(),
        )
        .unwrap()
        .snapshot;
    let live = store
        .publish_snapshot(
            &repo,
            &"b".repeat(40),
            &json!({}),
            &sample_graph(3, 1),
            &[],
            &load_dir(),
        )
        .unwrap()
        .snapshot;
    // Plant what older binaries and interrupted runs leave behind.
    let mut c = postgres::Client::connect(&db.url, postgres::NoTls).unwrap();
    c.batch_execute(r#"SET search_path = ag_catalog, "$user", public"#)
        .unwrap();
    let stale: i64 = c
        .query_one(
            "INSERT INTO rb.snapshot (repo_id, commit_sha, status, tool_versions)
             SELECT id, $2, 'building', '{}' FROM rb.repo WHERE name = $1 RETURNING id",
            &[&repo, &"c".repeat(40)],
        )
        .unwrap()
        .get(0);
    let orphan = format!("rb_{repo}_999999999");
    let retired_graph = crate::graph_name(&repo, first.id);
    for g in [&orphan, &retired_graph] {
        let mut tx = c.transaction().unwrap();
        tx.execute(
            "SELECT pg_advisory_xact_lock($1)",
            &[&crate::RB_AGE_DDL_LOCK],
        )
        .unwrap();
        tx.execute("SELECT ag_catalog.create_graph($1::text::name)", &[g])
            .unwrap();
        tx.commit().unwrap();
    }
    c.execute(
        "UPDATE rb.snapshot SET age_graph = $2 WHERE id = $1",
        &[&first.id, &retired_graph],
    )
    .unwrap();

    let plan = store.gc_plan(&prefix).unwrap();
    let graphs: Vec<(&str, Option<i64>)> = plan
        .graphs
        .iter()
        .map(|g| (g.name.as_str(), g.snapshot))
        .collect();
    assert_eq!(
        graphs,
        [
            (retired_graph.as_str(), Some(first.id)),
            (orphan.as_str(), None)
        ]
    );
    let building: Vec<i64> = plan.building.iter().map(|b| b.id).collect();
    assert_eq!(building, [stale]);
    // A dry run is the plan alone: nothing changed.
    assert_eq!(store.gc_plan(&prefix).unwrap(), plan);

    let removed = store.gc_apply(&plan).unwrap();
    assert_eq!(removed, plan);
    let state = repo_state(&mut c, &repo);
    assert_eq!(state.graphs, vec![crate::graph_name(&repo, live.id)]);
    assert_eq!((state.building, state.stray_age_graphs.len()), (0, 0));
    assert_eq!(state.live, vec![live.id]);
    assert_eq!(store.gc_plan(&prefix).unwrap(), crate::GcPlan::default());
    // `gc_apply` re-checks under its locks: a stale plan naming the live graph, or a row that is
    // no longer `building`, removes nothing.
    let stale_plan = crate::GcPlan {
        graphs: vec![crate::GcGraph {
            name: crate::graph_name(&repo, live.id),
            snapshot: Some(live.id),
            repo: Some(repo.clone()),
            status: Some("retired".into()),
        }],
        building: vec![crate::GcSnapshot {
            id: live.id,
            repo: repo.clone(),
            commit_sha: live.commit_sha.clone(),
        }],
    };
    assert_eq!(
        store.gc_apply(&stale_plan).unwrap(),
        crate::GcPlan::default()
    );
    let state = repo_state(&mut c, &repo);
    assert_eq!(
        (state.live, state.graphs),
        (vec![live.id], vec![crate::graph_name(&repo, live.id)])
    );
    // Out of scope: a prefix that matches nothing lists nothing.
    assert_eq!(
        store.gc_plan(&format!("{prefix}zz")).unwrap(),
        crate::GcPlan::default()
    );
}

/// Contract M5 S4, D13: `rb cache prune` removes facts rows of other extractors and SCIP rows of
/// other images, reports rows and bytes, keeps the current ones, and `--dry-run` removes
/// nothing. In a scratch database: the prune is database-wide.
#[test]
fn cache_prune_keeps_the_current_rows() {
    let Some(db) = db() else { return };
    let scratch = ScratchDb::create(&db.url, "prune");
    let mut store = Store::connect(&scratch.url).unwrap();
    let blob = |n: u8| format!("{n:040}");
    store
        .save_facts(&[
            (blob(1), "cur;v1;go".into(), b"{}".to_vec()),
            (blob(1), "cur;v1;java".into(), b"{\"a\":1}".to_vec()),
            (blob(1), "cur;v0;go".into(), b"{\"old\":1}".to_vec()),
            (blob(2), "cur;v1x;go".into(), b"{\"x\":12}".to_vec()),
        ])
        .unwrap();
    let entry = |bytes: &[u8]| crate::CachedScipIndex {
        status: "ok".into(),
        phase: None,
        index: Some(bytes.to_vec()),
        stderr_tail: String::new(),
        elapsed_ms: 0,
    };
    store
        .scip_index_put("h1", "img-current", &entry(b"abc"))
        .unwrap();
    store
        .scip_index_put("h1", "img-old", &entry(b"abcdef"))
        .unwrap();

    for dry_run in [true, false] {
        let facts = store.prune_facts("cur;v1", dry_run).unwrap();
        assert_eq!((facts.rows, facts.bytes), (2, 17), "dry run {dry_run}");
        let scip = store.prune_scip(&["img-current"], dry_run).unwrap();
        assert_eq!((scip.rows, scip.bytes), (1, 6), "dry run {dry_run}");
    }
    let mut left = |extractor: &str| {
        store
            .load_facts(&[(blob(1), extractor.to_owned())])
            .unwrap()
            .len()
    };
    assert_eq!(
        (left("cur;v1;go"), left("cur;v1;java"), left("cur;v0;go")),
        (1, 1, 0)
    );
    assert!(store.scip_index_get("h1", "img-current").unwrap().is_some());
    assert!(store.scip_index_get("h1", "img-old").unwrap().is_none());
    assert_eq!(store.prune_facts("cur;v1", false).unwrap().rows, 0);
    assert!(
        store.prune_scip(&[], false).is_err(),
        "no current image: refuse"
    );
}

/// Contract M5 S7, D17: a retired snapshot has no AGE projection (D10), so a Cypher query on it
/// is refused as such; the live one answers as the role `rb_reader`.
#[test]
fn cypher_on_a_retired_snapshot_is_refused() {
    let Some(db) = db() else { return };
    let repo = repo_name("cyretired");
    let _cleanup = Cleanup {
        url: &db.url,
        repos: vec![repo.clone()],
    };
    let mut store = Store::connect(&db.url).unwrap();
    let first = store
        .publish_snapshot(
            &repo,
            &"a".repeat(40),
            &json!({}),
            &sample_graph(2, 0),
            &[],
            &load_dir(),
        )
        .unwrap()
        .snapshot;
    let live = store
        .publish_snapshot(
            &repo,
            &"b".repeat(40),
            &json!({}),
            &sample_graph(3, 1),
            &[],
            &load_dir(),
        )
        .unwrap()
        .snapshot;
    let limits = crate::CypherLimits::default();
    let err = store
        .cypher(first.id, "MATCH (n) RETURN count(n)", &["n".into()], limits)
        .unwrap_err();
    assert!(
        matches!(
            err.downcast_ref::<crate::CypherError>(),
            Some(crate::CypherError::NoProjection { snapshot }) if *snapshot == first.id
        ),
        "{err:#}"
    );
    let out = store
        .cypher(live.id, "MATCH (n) RETURN count(n)", &["n".into()], limits)
        .unwrap();
    assert_eq!(
        (out.rows, out.truncated),
        (vec![vec!["6".to_owned()]], vec![])
    );
}

/// Contract M5 S15, D18: `find`'s pages (one `LIMIT/OFFSET` query and one count) concatenate to
/// the whole match list at limits 1, 7 and 1,000, `total` counts every match, and an offset past
/// the end is an empty page; the order is total, so repeated reads give the same pages.
#[test]
fn find_pages_concatenate_to_the_whole_list() {
    let Some(db) = db() else { return };
    let repo = repo_name("findpages");
    let _cleanup = Cleanup {
        url: &db.url,
        repos: vec![repo.clone()],
    };
    let mut store = Store::connect(&db.url).unwrap();
    let snap = store
        .publish_snapshot(
            &repo,
            &"a".repeat(40),
            &json!({}),
            &sample_graph(23, 0),
            &[],
            &load_dir(),
        )
        .unwrap()
        .snapshot;
    // `f` matches the 23 files (`src/fN.ts`) and the 23 symbols (`sym fN`) by key.
    let whole = store.find(snap.id, "f", None, 1_000, 0).unwrap();
    assert_eq!(whole.total, 46);
    assert_eq!(whole.nodes.len(), 46);
    for limit in [1i64, 7, 1_000] {
        let mut joined = Vec::new();
        let mut offset = 0;
        while offset < whole.total {
            let page = store.find(snap.id, "f", None, limit, offset).unwrap();
            assert_eq!(page.total, whole.total);
            assert!(page.nodes.len() as i64 <= limit);
            offset += page.nodes.len() as i64;
            joined.extend(page.nodes);
        }
        assert_eq!(joined, whole.nodes, "limit {limit}");
    }
    let past = store.find(snap.id, "f", None, 7, 46).unwrap();
    assert!(past.nodes.is_empty() && past.total == 46);
    let kinds = store
        .find(snap.id, "f", Some(NodeKind::Symbol), 1_000, 0)
        .unwrap();
    assert_eq!(kinds.total, 23);
}
