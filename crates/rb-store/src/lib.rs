//! Postgres storage for the Repository Brain (design §8).
//!
//! The relational schema `rb.*` is the source of truth; each live snapshot also gets an AGE graph
//! projection. A snapshot is published in a single transaction (rows, AGE graph, live-pointer flip),
//! so readers never see a partial graph and a failed load leaves nothing behind but a `failed` row.

mod age;
pub mod card;
pub mod code;
#[cfg(test)]
mod db_tests;
pub mod diff;
pub mod path;
mod query;
pub mod rollup;

/// Contract M5 D6: every limit of a question, in one table. Parameters above their maximum are
/// refused by the caller (`invalid_param`), never clamped; caps that cut an answer name
/// themselves in its `truncated` list ([`limits::Limit`]).
pub mod limits {
    /// Q1 caller levels: default and maximum.
    pub const CALLER_DEPTH_DEFAULT: usize = 1;
    pub const CALLER_DEPTH_MAX: usize = 25;
    /// Q4 call levels per hop: default and maximum.
    pub const PATH_DEPTH_DEFAULT: usize = 12;
    pub const PATH_DEPTH_MAX: usize = 25;
    /// Service hops through `CONSUMES` (Q4, Q3): default and maximum.
    pub const HOPS_DEFAULT: usize = 4;
    pub const HOPS_MAX: usize = 8;
    /// Nodes a reachability walk visits (Q2, Q3).
    pub const NODES: usize = 200_000;
    /// Entries in a rendered tree (Q1 callers, Q4 path nodes).
    pub const TREE: usize = 5_000;
    /// Counted operations (adjacency elements examined) per request, every question: a
    /// deterministic budget, so the same request truncates at the same element on any machine.
    /// Calibrated in phase 6b (contract M5 amendment 4): the largest default-parameter question
    /// on the fixtures costs under 1/10,000 of it; re-calibrated on the benchmark (S25).
    pub const WORK: u64 = 20_000_000;
    /// Cypher rows returned.
    pub const ROWS: usize = 10_000;
    /// Cypher statement timeout: default and maximum, in milliseconds.
    pub const CYPHER_TIMEOUT_MS_DEFAULT: u64 = 2_000;
    pub const CYPHER_TIMEOUT_MS_MAX: u64 = 30_000;

    /// A limit that cut an answer, as named in its `truncated` list.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub enum Limit {
        Depth,
        Hops,
        Nodes,
        Tree,
        Work,
        Rows,
    }

    impl Limit {
        pub fn as_str(self) -> &'static str {
            match self {
                Limit::Depth => "depth",
                Limit::Hops => "hops",
                Limit::Nodes => "nodes",
                Limit::Tree => "tree",
                Limit::Work => "work",
                Limit::Rows => "rows",
            }
        }
    }

    /// The caps a question runs under; the defaults are the table's. Tests (and the API's
    /// test-only override, S14) lower them.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Caps {
        pub nodes: usize,
        pub tree: usize,
        pub work: u64,
    }

    impl Default for Caps {
        fn default() -> Self {
            Caps {
                nodes: NODES,
                tree: TREE,
                work: WORK,
            }
        }
    }
}

/// Counted operations (contract M5 S10, D6; M4a invariant 11): every adjacency element a
/// question examines is spent from its request's budget ([`work::metered`]), in every build;
/// once the budget is spent, further elements are refused and the request is truncated by
/// `work`. The thread's running count ([`work::tick`]) also serves the growth tests, which
/// compare counts for an input and one four times as large.
pub mod work {
    use std::cell::Cell;

    #[derive(Debug, Clone, Copy)]
    struct Meter {
        spent: u64,
        cap: u64,
        refused: bool,
    }

    thread_local! {
        static COUNT: Cell<u64> = const { Cell::new(0) };
        static METER: Cell<Option<Meter>> = const { Cell::new(None) };
    }

    /// Counts `n` elements examined (an index build, a scan outside any request budget).
    #[inline]
    pub(crate) fn tick(n: usize) {
        COUNT.with(|c| c.set(c.get() + n as u64));
    }

    /// Spends `n` elements of the running request's budget; `false` (and the request marked
    /// truncated by `work`) when they do not fit, the elements then not to be examined. Outside
    /// a request, always `true`.
    #[inline]
    pub(crate) fn spend(n: usize) -> bool {
        tick(n);
        METER.with(|m| match m.get() {
            Some(mut meter) => {
                let n = n as u64;
                let fits = meter.spent + n <= meter.cap;
                if fits {
                    meter.spent += n;
                } else {
                    meter.refused = true;
                }
                m.set(Some(meter));
                fits
            }
            None => true,
        })
    }

    /// What a request spent, and whether its budget refused an element.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Spent {
        pub work: u64,
        pub refused: bool,
    }

    /// Runs `f` as one request with a budget of `cap` elements; a call inside a running request
    /// joins it (one budget for the whole answer). Returns what the request has spent so far.
    pub fn metered<T>(cap: u64, f: impl FnOnce() -> T) -> (T, Spent) {
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                METER.with(|m| m.set(None));
            }
        }
        let outer = METER.with(Cell::get).is_some();
        let _reset = (!outer).then(|| {
            METER.with(|m| {
                m.set(Some(Meter {
                    spent: 0,
                    cap,
                    refused: false,
                }))
            });
            Reset
        });
        let out = f();
        let meter = METER.with(Cell::get).expect("a request is running");
        (
            out,
            Spent {
                work: meter.spent,
                refused: meter.refused,
            },
        )
    }

    /// The count since the last call, on this thread.
    #[cfg(test)]
    pub(crate) fn take() -> u64 {
        COUNT.with(|c| c.replace(0))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// A budget of `n` admits exactly `n` elements; a nested request joins its caller's.
        #[test]
        fn a_budget_admits_exactly_its_cap() {
            let ((), spent) = metered(3, || {
                assert!(spend(1) && spend(2));
                assert!(!spend(1), "the fourth element is refused");
                let ((), inner) = metered(100, || assert!(!spend(1)));
                assert!(inner.refused, "the nested call shares the budget");
            });
            assert_eq!(
                spent,
                Spent {
                    work: 3,
                    refused: true
                }
            );
            let ((), spent) = metered(3, || assert!(spend(3)));
            assert_eq!(
                spent,
                Spent {
                    work: 3,
                    refused: false
                }
            );
            assert!(spend(1_000_000), "outside a request nothing is refused");
        }
    }
}

/// Test-only interleaving points (contract M5 S1): a hook runs once, on this thread, between the
/// statements of a read.
#[cfg(test)]
pub(crate) mod hooks {
    use std::cell::RefCell;

    thread_local! {
        static AFTER_NODES: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
    }

    /// Runs `body` with `hook` armed to run after `load_graph` has read a snapshot's nodes.
    pub(crate) fn with_after_nodes<T>(hook: impl FnMut() + 'static, body: impl FnOnce() -> T) -> T {
        AFTER_NODES.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
        let out = body();
        AFTER_NODES.with(|h| *h.borrow_mut() = None);
        out
    }

    pub(crate) fn after_nodes() {
        let hook = AFTER_NODES.with(|h| h.borrow_mut().take());
        if let Some(mut hook) = hook {
            hook();
        }
    }

    type AfterCopy = Box<dyn FnMut(i32) -> anyhow::Result<()>>;

    thread_local! {
        static AFTER_COPY: RefCell<Option<AfterCopy>> = RefCell::new(None);
    }

    /// Runs `body` with `hook` armed to run inside the publish transaction after its COPYs, with
    /// the publishing backend's pid; an error from it fails the publish (contract M5 S3).
    pub(crate) fn with_after_copy<T>(
        hook: impl FnMut(i32) -> anyhow::Result<()> + 'static,
        body: impl FnOnce() -> T,
    ) -> T {
        AFTER_COPY.with(|h| *h.borrow_mut() = Some(Box::new(hook)));
        let out = body();
        AFTER_COPY.with(|h| *h.borrow_mut() = None);
        out
    }

    pub(crate) fn after_copy(pid: i32) -> anyhow::Result<()> {
        let hook = AFTER_COPY.with(|h| h.borrow_mut().take());
        match hook {
            Some(mut hook) => hook(pid),
            None => Ok(()),
        }
    }
}

use anyhow::{Context, bail};
use postgres::binary_copy::BinaryCopyInWriter;
use postgres::types::Type;
use postgres::{Client, NoTls};
use std::collections::HashMap;

use rb_model::{
    Certainty, Diagnostic, Edge, EdgeKind, Evidence, Graph, Node, NodeKind, Provenance,
};
use serde_json::Value;

pub use age::{LoadDir, LoadedFile, ProjectionProfile, graph_name};
pub use query::{
    Api, DiCoverage, FoundNode, FoundPage, Handler, HttpCoverage, HttpPeer, HttpUse, OwnerShare,
    ResolutionCoverage, Resolved, RtaCoverage, ServiceCard, ServiceSummary, SqlCoverage,
    TableAccess, TableDeps,
};

/// A recorded SCIP indexer run (design §5.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedScipIndex {
    /// `ok`, `failed` or `timeout`.
    pub status: String,
    /// `fetch` or `index` for a `failed` run (contract M3b amendment 56); `None` otherwise and
    /// on rows written before the column existed.
    pub phase: Option<String>,
    /// `index.scip` bytes when `ok`.
    pub index: Option<Vec<u8>>,
    pub stderr_tail: String,
    pub elapsed_ms: i32,
}

/// Embedded migrations, applied in order and recorded in `public.rb_schema_migrations`.
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_init",
        include_str!("../../../migrations/0001_init.sql"),
    ),
    (
        "0002_evidence_paths",
        include_str!("../../../migrations/0002_evidence_paths.sql"),
    ),
    (
        "0003_trgm_schema",
        include_str!("../../../migrations/0003_trgm_schema.sql"),
    ),
    (
        "0004_scip_index",
        include_str!("../../../migrations/0004_scip_index.sql"),
    ),
    (
        "0005_scip_index_phase",
        include_str!("../../../migrations/0005_scip_index_phase.sql"),
    ),
    (
        "0006_reader_role",
        include_str!("../../../migrations/0006_reader_role.sql"),
    ),
];

/// Name of the newest embedded migration; recorded in each snapshot's `tool_versions`.
pub fn schema_version() -> &'static str {
    MIGRATIONS[MIGRATIONS.len() - 1].0
}

/// Retired snapshots kept per repo (plus the live one) before being dropped (design §8.3;
/// contract M5 D10: `--keep`, `RB_KEEP_RETIRED`).
pub const KEEP_RETIRED: i64 = 2;

/// The largest `--keep` (contract M5 S3).
pub const MAX_KEEP_RETIRED: i64 = 20;

/// Arbitrary constant for `pg_advisory_xact_lock` so concurrent `rb migrate` runs serialise.
const MIGRATION_LOCK: i64 = 0x7262_6d69_6772;

/// The database-wide advisory lock every AGE graph DDL statement (`create_graph`, `drop_graph`)
/// runs under, transaction-scoped (contract M5 D10, S2; design §14: concurrent graph DDL from
/// several processes has killed backends with `failed to re-find shared lock object`). Taken
/// after any repository row lock, never before.
pub const RB_AGE_DDL_LOCK: i64 = 0x7262_6167_6464;

#[derive(Debug, Clone, PartialEq)]
pub struct SnapshotInfo {
    pub id: i64,
    pub repo: String,
    pub commit_sha: String,
    /// The repository's live snapshot when this one was published (contract M5 D11's default
    /// `from` of `rb diff`); `NULL` once that row is deleted.
    pub parent_id: Option<i64>,
    pub status: String,
    pub tool_versions: Value,
    pub age_graph: Option<String>,
}

/// A published snapshot and what the retention run after it removed.
#[derive(Debug, Clone, PartialEq)]
pub struct Published {
    pub snapshot: SnapshotInfo,
    pub retention: RetentionReport,
    /// What the AGE projection loaded, with timings (`rb index --profile`).
    pub projection: ProjectionProfile,
}

/// What one retention run did (contract M5 D10, S3).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RetentionReport {
    /// AGE graphs dropped, one transaction each: every snapshot's but the live one's.
    pub graphs_dropped: Vec<String>,
    /// `failed` rows and `retired` rows beyond the newest `--keep`.
    pub snapshots_deleted: Vec<i64>,
    /// The live pointer names a row whose status is not `live`; the row was kept.
    pub live_marked_retired: Option<i64>,
}

/// What `rb gc` removes (contract M5 S4): AGE graphs named `rb_*` that are not a live
/// snapshot's, and `building` rows (left by binaries before M5, which committed the row before
/// loading). Every list is in bytewise name or id order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GcPlan {
    pub graphs: Vec<GcGraph>,
    pub building: Vec<GcSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GcGraph {
    pub name: String,
    /// The snapshot that still names the graph (not its repository's live one), if any.
    pub snapshot: Option<i64>,
    pub repo: Option<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GcSnapshot {
    pub id: i64,
    pub repo: String,
    pub commit_sha: String,
}

/// Rows a cache prune removes (or would remove) and their payload bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PruneReport {
    pub rows: i64,
    pub bytes: i64,
}

/// Default Cypher statement timeout (contract M5 D6: 2 s; `--timeout-ms`, `RB_CYPHER_TIMEOUT_MS`).
pub const CYPHER_TIMEOUT_MS_DEFAULT: u64 = limits::CYPHER_TIMEOUT_MS_DEFAULT;
/// The largest Cypher statement timeout a request may set.
pub const CYPHER_TIMEOUT_MS_MAX: u64 = limits::CYPHER_TIMEOUT_MS_MAX;
/// Cypher rows returned at most; more are cut with `truncated: ["rows"]` (D6).
pub const CYPHER_ROWS_MAX: usize = limits::ROWS;

/// The guard rails of one Cypher request (contract M5 S7).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CypherLimits {
    pub timeout_ms: u64,
    pub rows: usize,
}

impl Default for CypherLimits {
    fn default() -> Self {
        Self {
            timeout_ms: CYPHER_TIMEOUT_MS_DEFAULT,
            rows: CYPHER_ROWS_MAX,
        }
    }
}

/// A Cypher answer: the requested columns, the rows (agtype text), and the limits that cut it.
#[derive(Debug, Clone, PartialEq)]
pub struct CypherRows {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
    pub truncated: Vec<&'static str>,
}

/// Why a Cypher query was refused (contract M5 D5: `cypher`, `cypher_timeout`; a retired
/// snapshot is a parameter error, D17).
#[derive(Debug, Clone, PartialEq)]
pub enum CypherError {
    /// Only the live snapshot has an AGE graph (D10).
    NoProjection { snapshot: i64 },
    /// The statement timeout cancelled the query.
    Timeout { limit_ms: u64 },
    /// The database refused the query: syntax, a write in the read-only transaction, a
    /// privilege `rb_reader` lacks.
    Refused(String),
}

impl std::fmt::Display for CypherError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CypherError::NoProjection { snapshot } => write!(
                f,
                "snapshot {snapshot} has no AGE projection (only the live snapshot has one)"
            ),
            CypherError::Timeout { limit_ms } => write!(
                f,
                "cypher query cancelled by its statement timeout of {limit_ms} ms \
                 (limit `timeout_ms`: --timeout-ms, at most {CYPHER_TIMEOUT_MS_MAX})"
            ),
            CypherError::Refused(message) => write!(f, "cypher: {message}"),
        }
    }
}

impl std::error::Error for CypherError {}

/// Classifies a database error from a Cypher query; anything that is not the server's answer
/// (a broken connection) stays as it is.
fn cypher_error(e: postgres::Error, limits: CypherLimits) -> anyhow::Error {
    match e.as_db_error() {
        Some(db) if *db.code() == postgres::error::SqlState::QUERY_CANCELED => {
            CypherError::Timeout {
                limit_ms: limits.timeout_ms,
            }
            .into()
        }
        Some(db) => CypherError::Refused(db.message().to_owned()).into(),
        None => e.into(),
    }
}

/// What a publish loads, in one transaction.
struct Load<'a> {
    repo_id: i64,
    repo: &'a str,
    commit_sha: &'a str,
    tool_versions: &'a Value,
    graph: &'a Graph,
    diagnostics: &'a [Diagnostic],
    load_dir: &'a LoadDir,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RepoStatus {
    pub name: String,
    pub live: Option<SnapshotInfo>,
    pub nodes: i64,
    pub edges: i64,
    pub diagnostics: i64,
    pub snapshots: i64,
}

pub struct Store {
    client: Client,
    /// Inside [`Store::read_tx`]: statements belong to its transaction, and nothing may open
    /// another one.
    in_read: bool,
    /// Retired snapshots each publish keeps per repository.
    keep_retired: i64,
}

impl Store {
    pub fn connect(url: &str) -> anyhow::Result<Self> {
        let mut client = Client::connect(url, NoTls)
            .with_context(|| format!("connecting to {}", redact(url)))?;
        // AGE's cypher() and agtype live in ag_catalog; rb.* is always schema-qualified.
        client.batch_execute(r#"SET search_path = ag_catalog, "$user", public"#)?;
        Ok(Self {
            client,
            in_read: false,
            keep_retired: KEEP_RETIRED,
        })
    }

    /// A connection whose every transaction is read-only (contract M5 D8, S13, invariant 7: the
    /// server's pool): `SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY`, so the database
    /// refuses a write whatever statement issues it.
    pub fn connect_read_only(url: &str) -> anyhow::Result<Self> {
        let mut store = Self::connect(url)?;
        store
            .client
            .batch_execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY")?;
        Ok(store)
    }

    /// This session's `default_transaction_read_only`.
    pub fn session_read_only(&mut self) -> anyhow::Result<bool> {
        let value: String = self
            .client
            .query_one("SHOW default_transaction_read_only", &[])?
            .get(0);
        Ok(value == "on")
    }

    /// Sets how many retired snapshots retention keeps per repository (0 to
    /// [`MAX_KEEP_RETIRED`]; default [`KEEP_RETIRED`]).
    pub fn set_keep_retired(&mut self, keep: i64) -> anyhow::Result<()> {
        anyhow::ensure!(
            (0..=MAX_KEEP_RETIRED).contains(&keep),
            "--keep must be between 0 and {MAX_KEEP_RETIRED}"
        );
        self.keep_retired = keep;
        Ok(())
    }

    /// A read-write transaction; refused inside [`Store::read_tx`], whose transaction it would
    /// otherwise end.
    fn begin(&mut self) -> anyhow::Result<postgres::Transaction<'_>> {
        anyhow::ensure!(
            !self.in_read,
            "cannot write inside a read-only snapshot transaction"
        );
        Ok(self.client.transaction()?)
    }

    /// Applies pending migrations; returns the names applied by this call.
    pub fn migrate(&mut self) -> anyhow::Result<Vec<&'static str>> {
        let mut tx = self.begin()?;
        tx.execute("SELECT pg_advisory_xact_lock($1)", &[&MIGRATION_LOCK])?;
        // Objects created by migrations must land in `public`, not in AGE's ag_catalog.
        tx.batch_execute("SET LOCAL search_path = public")?;
        tx.batch_execute(
            "CREATE TABLE IF NOT EXISTS public.rb_schema_migrations (
               name text PRIMARY KEY, applied_at timestamptz NOT NULL DEFAULT now())",
        )?;
        let mut applied = Vec::new();
        for (name, sql) in MIGRATIONS {
            let done = tx
                .query_opt(
                    "SELECT 1 FROM public.rb_schema_migrations WHERE name = $1",
                    &[name],
                )?
                .is_some();
            if !done {
                tx.batch_execute(sql)
                    .with_context(|| format!("applying migration {name}"))?;
                tx.execute(
                    "INSERT INTO public.rb_schema_migrations (name) VALUES ($1)",
                    &[name],
                )?;
                applied.push(*name);
            }
        }
        tx.commit()?;
        Ok(applied)
    }

    pub fn ensure_repo(&mut self, name: &str, origin_url: Option<&str>) -> anyhow::Result<i64> {
        let row = self.client.query_one(
            "INSERT INTO rb.repo (name, origin_url) VALUES ($1, $2)
             ON CONFLICT (name) DO UPDATE SET origin_url = COALESCE(EXCLUDED.origin_url, rb.repo.origin_url)
             RETURNING id",
            &[&name, &origin_url],
        )?;
        Ok(row.get(0))
    }

    pub fn repo_names(&mut self) -> anyhow::Result<Vec<String>> {
        let rows = self
            .client
            .query("SELECT name FROM rb.repo ORDER BY name COLLATE \"C\"", &[])?;
        Ok(rows.iter().map(|r| r.get(0)).collect())
    }

    /// A cached SCIP index for `(input_hash, indexer_digest)`, if one was recorded.
    pub fn scip_index_get(
        &mut self,
        input_hash: &str,
        indexer_digest: &str,
    ) -> anyhow::Result<Option<CachedScipIndex>> {
        let row = self.client.query_opt(
            "SELECT status, index, stderr_tail, elapsed_ms, phase FROM rb.scip_index
             WHERE input_hash = $1 AND indexer_digest = $2",
            &[&input_hash, &indexer_digest],
        )?;
        Ok(row.map(|r| CachedScipIndex {
            status: r.get(0),
            index: r.get(1),
            stderr_tail: r.get(2),
            elapsed_ms: r.get(3),
            phase: r.get(4),
        }))
    }

    /// Records an index run; a later run with the same key replaces it (a retry after a fix).
    pub fn scip_index_put(
        &mut self,
        input_hash: &str,
        indexer_digest: &str,
        entry: &CachedScipIndex,
    ) -> anyhow::Result<()> {
        self.client.execute(
            "INSERT INTO rb.scip_index (input_hash, indexer_digest, status, index, stderr_tail, elapsed_ms, phase)
             VALUES ($1, $2, $3, $4, $5, $6, $7)
             ON CONFLICT (input_hash, indexer_digest) DO UPDATE
             SET status = EXCLUDED.status, index = EXCLUDED.index,
                 stderr_tail = EXCLUDED.stderr_tail, elapsed_ms = EXCLUDED.elapsed_ms,
                 phase = EXCLUDED.phase, created_at = now()",
            &[
                &input_hash,
                &indexer_digest,
                &entry.status,
                &entry.index,
                &entry.stderr_tail,
                &entry.elapsed_ms,
                &entry.phase,
            ],
        )?;
        Ok(())
    }

    /// Deletes a repository, all its snapshots and their AGE graphs, one graph per transaction
    /// (contract M5 D10). Returns false if it did not exist.
    pub fn delete_repo(&mut self, name: &str) -> anyhow::Result<bool> {
        let Some(repo_id) = self
            .client
            .query_opt("SELECT id FROM rb.repo WHERE name = $1", &[&name])?
            .map(|r| r.get::<_, i64>(0))
        else {
            return Ok(false);
        };
        while self.drop_one_graph(repo_id, true)?.is_some() {}
        let mut tx = self.begin()?;
        // Clear the live pointer first so snapshot rows can cascade away with the repo.
        tx.execute(
            "UPDATE rb.repo SET live_snapshot_id = NULL WHERE id = $1",
            &[&repo_id],
        )?;
        let deleted = tx.execute("DELETE FROM rb.repo WHERE id = $1", &[&repo_id])? > 0;
        tx.commit()?;
        Ok(deleted)
    }

    /// Drops one AGE graph of the repository's snapshots in a transaction of its own, under the
    /// repository row lock and then the DDL lock, and clears the snapshot's `age_graph`; the live
    /// snapshot's only with `include_live`. Returns the graph's name, or `None` when none is left.
    fn drop_one_graph(
        &mut self,
        repo_id: i64,
        include_live: bool,
    ) -> anyhow::Result<Option<String>> {
        let mut tx = self.begin()?;
        tx.execute(
            "SELECT 1 FROM rb.repo WHERE id = $1 FOR UPDATE",
            &[&repo_id],
        )?;
        let Some(row) = tx.query_opt(
            "SELECT s.id, s.age_graph FROM rb.snapshot s JOIN rb.repo r ON r.id = s.repo_id
             WHERE s.repo_id = $1 AND s.age_graph IS NOT NULL
               AND ($2 OR (s.id IS DISTINCT FROM r.live_snapshot_id AND s.status <> 'live'))
             ORDER BY s.id LIMIT 1",
            &[&repo_id, &include_live],
        )?
        else {
            return Ok(None);
        };
        let (id, graph): (i64, String) = (row.get(0), row.get(1));
        age::drop_graph_if_exists(&mut tx, &graph)?;
        tx.execute(
            "UPDATE rb.snapshot SET age_graph = NULL WHERE id = $1",
            &[&id],
        )?;
        tx.commit()?;
        Ok(Some(graph))
    }

    pub fn live_snapshot(&mut self, repo: &str) -> anyhow::Result<Option<SnapshotInfo>> {
        let row = self.client.query_opt(
            &format!("{SNAPSHOT_SELECT} WHERE r.name = $1 AND s.id = r.live_snapshot_id"),
            &[&repo],
        )?;
        Ok(row.as_ref().map(snapshot_from_row))
    }

    pub fn snapshot(&mut self, id: i64) -> anyhow::Result<SnapshotInfo> {
        self.snapshot_opt(id)?
            .with_context(|| format!("snapshot {id} does not exist"))
    }

    /// The snapshot with id `id`, if its row exists.
    pub fn snapshot_opt(&mut self, id: i64) -> anyhow::Result<Option<SnapshotInfo>> {
        let row = self
            .client
            .query_opt(&format!("{SNAPSHOT_SELECT} WHERE s.id = $1"), &[&id])?;
        Ok(row.as_ref().map(snapshot_from_row))
    }

    /// The repository's snapshot rows (live, retired, failed), newest first.
    pub fn snapshots(&mut self, repo: &str) -> anyhow::Result<Vec<SnapshotInfo>> {
        let rows = self.client.query(
            &format!("{SNAPSHOT_SELECT} WHERE r.name = $1 ORDER BY s.id DESC"),
            &[&repo],
        )?;
        Ok(rows.iter().map(snapshot_from_row).collect())
    }

    /// Whether snapshot id `id` was ever issued (it is at most the id sequence's last value):
    /// a missing row with such an id was deleted, by retention or with its repository
    /// (contract M5 D17: `snapshot_not_retained` rather than `not_found`).
    pub fn snapshot_issued(&mut self, id: i64) -> anyhow::Result<bool> {
        let row = self.client.query_one(
            "SELECT coalesce(pg_sequence_last_value('rb.snapshot_id_seq'), 0) >= $1",
            &[&id],
        )?;
        Ok(row.get(0))
    }

    /// Publishes `graph` as the repo's new live snapshot: the snapshot row, its rows, the AGE
    /// projection and the live flip in one transaction (contract M5 D10, S3), then retention
    /// (the previous live snapshot's graph is dropped in a transaction of its own). A failed
    /// publish leaves nothing of the attempt but a separate `failed` row written after the
    /// rollback; a publisher that dies before its commit leaves no row at all.
    pub fn publish_snapshot(
        &mut self,
        repo: &str,
        commit_sha: &str,
        tool_versions: &Value,
        graph: &Graph,
        diagnostics: &[Diagnostic],
        load_dir: &LoadDir,
    ) -> anyhow::Result<Published> {
        let repo_id = self.ensure_repo(repo, None)?;
        let load = Load {
            repo_id,
            repo,
            commit_sha,
            tool_versions,
            graph,
            diagnostics,
            load_dir,
        };
        let (snapshot_id, projection) = match self.load_and_flip(&load) {
            Ok(loaded) => loaded,
            Err(err) => {
                let recorded = self.client.query_one(
                    "INSERT INTO rb.snapshot (repo_id, commit_sha, parent_id, status, tool_versions)
                     SELECT id, $2, live_snapshot_id, 'failed', $3 FROM rb.repo WHERE id = $1
                     RETURNING id",
                    &[&repo_id, &commit_sha, tool_versions],
                );
                let what = match recorded {
                    Ok(row) => format!("recorded as failed snapshot {}", row.get::<_, i64>(0)),
                    Err(e) => format!("the failure could not be recorded: {e}"),
                };
                return Err(err.context(format!("publishing a snapshot of {repo} ({what})")));
            }
        };
        let retention = self
            .retain(repo_id)
            .with_context(|| format!("snapshot {snapshot_id} is live; retention failed"))?;
        Ok(Published {
            snapshot: self.snapshot(snapshot_id)?,
            retention,
            projection,
        })
    }

    /// Writes the snapshot row and its rows, projects it and flips the pointer, all in one
    /// transaction; returns the new snapshot's id and what the projection loaded.
    fn load_and_flip(&mut self, load: &Load<'_>) -> anyhow::Result<(i64, ProjectionProfile)> {
        let Load {
            repo_id,
            repo,
            commit_sha,
            tool_versions,
            graph,
            diagnostics,
            load_dir,
        } = *load;
        let mut tx = self.begin()?;
        // Serialise publishers of the same repo; the parent is read under the lock.
        let parent: Option<i64> = tx
            .query_one(
                "SELECT live_snapshot_id FROM rb.repo WHERE id = $1 FOR UPDATE",
                &[&repo_id],
            )?
            .get(0);
        let snapshot_id: i64 = tx
            .query_one(
                "INSERT INTO rb.snapshot (repo_id, commit_sha, parent_id, status, tool_versions)
                 VALUES ($1, $2, $3, 'building', $4) RETURNING id",
                &[&repo_id, &commit_sha, &parent, tool_versions],
            )?
            .get(0);
        let graph_name = graph_name(repo, snapshot_id);

        let sink = tx.copy_in(
            "COPY rb.node (snapshot_id, id, kind, key, name, attrs) FROM STDIN (FORMAT binary)",
        )?;
        let mut w = BinaryCopyInWriter::new(
            sink,
            &[
                Type::INT8,
                Type::INT8,
                Type::TEXT,
                Type::TEXT,
                Type::TEXT,
                Type::JSONB,
            ],
        );
        for n in graph.nodes() {
            let attrs = Value::Object(n.attrs.clone());
            w.write(&[
                &snapshot_id,
                &n.id,
                &n.kind.as_str(),
                &n.key,
                &n.name,
                &attrs,
            ])?;
        }
        w.finish().context("copying nodes")?;

        let sink = tx.copy_in(
            "COPY rb.edge (snapshot_id, src, dst, kind, provenance, certainty, attrs) FROM STDIN (FORMAT binary)",
        )?;
        let mut w = BinaryCopyInWriter::new(
            sink,
            &[
                Type::INT8,
                Type::INT8,
                Type::INT8,
                Type::TEXT,
                Type::TEXT,
                Type::INT2,
                Type::JSONB,
            ],
        );
        for e in graph.edges() {
            let attrs = Value::Object(e.attrs.clone());
            w.write(&[
                &snapshot_id,
                &e.src,
                &e.dst,
                &e.kind.as_str(),
                &e.provenance.as_str(),
                &e.certainty.rank(),
                &attrs,
            ])?;
        }
        w.finish().context("copying edges")?;

        let file_ids: HashMap<&str, i64> = graph
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::File)
            .map(|n| (n.key.as_str(), n.id))
            .collect();

        let sink = tx.copy_in(
            "COPY rb.edge_evidence (snapshot_id, src, kind, dst, provenance, file_id, span) FROM STDIN (FORMAT binary)",
        )?;
        let mut w = BinaryCopyInWriter::new(
            sink,
            &[
                Type::INT8,
                Type::INT8,
                Type::TEXT,
                Type::INT8,
                Type::TEXT,
                Type::INT8,
                Type::INT4_ARRAY,
            ],
        );
        for e in graph.edges() {
            for ev in &e.evidence {
                let file_id = file_ids
                    .get(ev.path.as_str())
                    .with_context(|| format!("evidence file {:?} is not a node", ev.path))?;
                w.write(&[
                    &snapshot_id,
                    &e.src,
                    &e.kind.as_str(),
                    &e.dst,
                    &e.provenance.as_str(),
                    file_id,
                    &ev.span.to_vec(),
                ])?;
            }
        }
        w.finish().context("copying edge evidence")?;

        let sink = tx.copy_in(
            "COPY rb.diagnostic (snapshot_id, kind, path, file_id, span, detail) FROM STDIN (FORMAT binary)",
        )?;
        let mut w = BinaryCopyInWriter::new(
            sink,
            &[
                Type::INT8,
                Type::TEXT,
                Type::TEXT,
                Type::INT8,
                Type::INT4_ARRAY,
                Type::JSONB,
            ],
        );
        for d in diagnostics {
            let file_id = d.path.as_deref().and_then(|p| file_ids.get(p));
            let span = d.span.map(|s| s.to_vec());
            w.write(&[&snapshot_id, &d.kind, &d.path, &file_id, &span, &d.detail])?;
        }
        w.finish().context("copying diagnostics")?;
        #[cfg(test)]
        {
            let pid: i32 = tx.query_one("SELECT pg_backend_pid()", &[])?.get(0);
            hooks::after_copy(pid)?;
        }

        let projection =
            age::project(&mut tx, graph, &graph_name, load_dir).with_context(|| {
                format!(
                    "projecting snapshot into AGE (the server must see {} as /tmp/age/{})",
                    load_dir.host.display(),
                    load_dir.server_rel
                )
            })?;

        tx.execute(
            "UPDATE rb.snapshot SET status = 'retired' WHERE repo_id = $1 AND status = 'live'",
            &[&repo_id],
        )?;
        tx.execute(
            "UPDATE rb.snapshot SET status = 'live', age_graph = $2 WHERE id = $1",
            &[&snapshot_id, &graph_name],
        )?;
        tx.execute(
            "UPDATE rb.repo SET live_snapshot_id = $2 WHERE id = $1",
            &[&repo_id, &snapshot_id],
        )?;
        // The planner's statistics must cover the new snapshot when it becomes visible: with
        // statistics from before the COPY, `snapshot_id = <new>` is estimated at one row and a
        // snapshot read joins as a nested loop over the whole snapshot (contract M5 amendment
        // 14). ANALYZE counts this transaction's own rows and its statistics commit with the flip.
        tx.batch_execute("ANALYZE rb.node, rb.edge, rb.edge_evidence, rb.diagnostic")
            .context("analyzing the snapshot tables")?;
        tx.commit()?;
        Ok((snapshot_id, projection))
    }

    /// Retention for one repository (contract M5 D10, S3): see [`Store::retain`].
    pub fn apply_retention(&mut self, repo: &str) -> anyhow::Result<RetentionReport> {
        let repo_id: i64 = self
            .client
            .query_opt("SELECT id FROM rb.repo WHERE name = $1", &[&repo])?
            .with_context(|| format!("repository {repo:?} does not exist"))?
            .get(0);
        self.retain(repo_id)
    }

    /// Drops the AGE graph of every snapshot but the live one, one per transaction; then, in one
    /// transaction, deletes the `failed` rows and the `retired` rows beyond the newest
    /// `keep_retired`. The snapshot the repository's pointer names is never selected, whatever
    /// its status (one marked otherwise is reported); a row whose graph could not be dropped is
    /// left for the next run; `building` rows are left to `rb gc`.
    fn retain(&mut self, repo_id: i64) -> anyhow::Result<RetentionReport> {
        let mut report = RetentionReport::default();
        while let Some(graph) = self.drop_one_graph(repo_id, false)? {
            report.graphs_dropped.push(graph);
        }
        let keep = self.keep_retired;
        let mut tx = self.begin()?;
        let pointer: Option<i64> = tx
            .query_one(
                "SELECT live_snapshot_id FROM rb.repo WHERE id = $1 FOR UPDATE",
                &[&repo_id],
            )?
            .get(0);
        if let Some(id) = pointer {
            let status: String = tx
                .query_one("SELECT status FROM rb.snapshot WHERE id = $1", &[&id])?
                .get(0);
            if status != "live" {
                report.live_marked_retired = Some(id);
            }
        }
        report.snapshots_deleted = tx
            .query(
                "DELETE FROM rb.snapshot WHERE id IN (
                   SELECT id FROM rb.snapshot
                   WHERE repo_id = $1 AND id IS DISTINCT FROM $2 AND age_graph IS NULL AND (
                     status = 'failed'
                     OR (status = 'retired' AND id NOT IN (
                           SELECT id FROM rb.snapshot
                           WHERE repo_id = $1 AND status = 'retired' AND id IS DISTINCT FROM $2
                           ORDER BY id DESC LIMIT $3))))
                 RETURNING id",
                &[&repo_id, &pointer, &keep],
            )?
            .iter()
            .map(|r| r.get(0))
            .collect();
        report.snapshots_deleted.sort_unstable();
        tx.commit()?;
        Ok(report)
    }

    /// What `rb gc` would remove, for repositories whose names start with `prefix` (graphs named
    /// `rb_<prefix>…`; `""` for all): every `rb_*` graph that no snapshot names, or that a snapshot
    /// other than its repository's live one names, and every `building` row.
    pub fn gc_plan(&mut self, prefix: &str) -> anyhow::Result<GcPlan> {
        let graph_prefix = format!("rb_{prefix}");
        let graphs = self
            .client
            .query(
                "SELECT g.name::text, s.id, r.name, s.status
                 FROM ag_catalog.ag_graph g
                 LEFT JOIN rb.snapshot s ON s.age_graph = g.name::text
                 LEFT JOIN rb.repo r ON r.id = s.repo_id
                 WHERE left(g.name::text, length($1)) = $1
                   AND (s.id IS NULL OR s.id IS DISTINCT FROM r.live_snapshot_id)
                 ORDER BY g.name::text COLLATE \"C\", s.id",
                &[&graph_prefix],
            )?
            .iter()
            .map(|r| GcGraph {
                name: r.get(0),
                snapshot: r.get(1),
                repo: r.get(2),
                status: r.get(3),
            })
            .collect();
        let building = self
            .client
            .query(
                "SELECT s.id, r.name, s.commit_sha FROM rb.snapshot s JOIN rb.repo r ON r.id = s.repo_id
                 WHERE s.status = 'building' AND left(r.name, length($1)) = $1
                 ORDER BY s.id",
                &[&prefix],
            )?
            .iter()
            .map(|r| GcSnapshot {
                id: r.get(0),
                repo: r.get(1),
                commit_sha: r.get(2),
            })
            .collect();
        Ok(GcPlan { graphs, building })
    }

    /// Removes what `plan` lists, re-checking each item under its locks: one graph per
    /// transaction (the repository row, then [`RB_AGE_DDL_LOCK`]; a graph that a live snapshot
    /// names by then is skipped), then each `building` row still `building`. Returns what was
    /// removed.
    pub fn gc_apply(&mut self, plan: &GcPlan) -> anyhow::Result<GcPlan> {
        let mut removed = GcPlan::default();
        for g in &plan.graphs {
            let mut tx = self.begin()?;
            if let Some(id) = g.snapshot {
                tx.execute(
                    "SELECT 1 FROM rb.repo r JOIN rb.snapshot s ON s.repo_id = r.id
                     WHERE s.id = $1 FOR UPDATE OF r",
                    &[&id],
                )?;
            }
            age::lock_ddl(&mut tx)?;
            // Under the DDL lock no publisher is between creating a graph and committing it.
            let named_by_live = tx
                .query_opt(
                    "SELECT 1 FROM rb.snapshot s JOIN rb.repo r ON r.live_snapshot_id = s.id
                     WHERE s.age_graph = $1",
                    &[&g.name],
                )?
                .is_some();
            if named_by_live {
                continue;
            }
            age::drop_graph_if_exists(&mut tx, &g.name)?;
            tx.execute(
                "UPDATE rb.snapshot SET age_graph = NULL WHERE age_graph = $1",
                &[&g.name],
            )?;
            tx.commit()?;
            removed.graphs.push(g.clone());
        }
        for b in &plan.building {
            let mut tx = self.begin()?;
            tx.execute(
                "SELECT 1 FROM rb.repo r JOIN rb.snapshot s ON s.repo_id = r.id
                 WHERE s.id = $1 FOR UPDATE OF r",
                &[&b.id],
            )?;
            let deleted = tx.execute(
                "DELETE FROM rb.snapshot WHERE id = $1 AND status = 'building'",
                &[&b.id],
            )?;
            tx.commit()?;
            if deleted > 0 {
                removed.building.push(b.clone());
            }
        }
        Ok(removed)
    }

    /// Facts-cache rows whose extractor is not `current_extractor` (rows are keyed
    /// `<extractor>;<language>`), removed unless `dry_run` (contract M5 D13).
    pub fn prune_facts(
        &mut self,
        current_extractor: &str,
        dry_run: bool,
    ) -> anyhow::Result<PruneReport> {
        let current = format!("{current_extractor};");
        let filter = "WHERE left(extractor, length($1)) <> $1";
        let sql = if dry_run {
            format!(
                "SELECT count(*), COALESCE(sum(octet_length(facts)), 0)::bigint
                 FROM rb.file_facts_cache {filter}"
            )
        } else {
            format!(
                "WITH gone AS (DELETE FROM rb.file_facts_cache {filter} RETURNING facts)
                 SELECT count(*), COALESCE(sum(octet_length(facts)), 0)::bigint FROM gone"
            )
        };
        let row = self.client.query_one(sql.as_str(), &[&current])?;
        Ok(PruneReport {
            rows: row.get(0),
            bytes: row.get(1),
        })
    }

    /// SCIP index rows whose image is none of `current_images`, removed unless `dry_run`
    /// (contract M5 D13). An empty list is refused: it would remove every row.
    pub fn prune_scip(
        &mut self,
        current_images: &[&str],
        dry_run: bool,
    ) -> anyhow::Result<PruneReport> {
        anyhow::ensure!(
            !current_images.is_empty(),
            "no current SCIP image is known; nothing pruned"
        );
        let filter = "WHERE indexer_digest <> ALL($1)";
        let sql = if dry_run {
            format!(
                "SELECT count(*), COALESCE(sum(octet_length(index)), 0)::bigint
                 FROM rb.scip_index {filter}"
            )
        } else {
            format!(
                "WITH gone AS (DELETE FROM rb.scip_index {filter} RETURNING index)
                 SELECT count(*), COALESCE(sum(octet_length(index)), 0)::bigint FROM gone"
            )
        };
        let row = self.client.query_one(sql.as_str(), &[&current_images])?;
        Ok(PruneReport {
            rows: row.get(0),
            bytes: row.get(1),
        })
    }

    /// Cached syntax facts for `(blob, extractor)` rows; missing rows are simply absent.
    pub fn load_facts(
        &mut self,
        rows: &[(String, String)],
    ) -> anyhow::Result<HashMap<(String, String), Vec<u8>>> {
        let (blobs, extractors): (Vec<&str>, Vec<&str>) =
            rows.iter().map(|(b, e)| (b.as_str(), e.as_str())).unzip();
        Ok(self
            .client
            .query(
                "SELECT c.blob_sha, c.extractor, c.facts
                 FROM rb.file_facts_cache c
                 JOIN unnest($1::text[], $2::text[]) AS k(blob_sha, extractor)
                   USING (blob_sha, extractor)",
                &[&blobs, &extractors],
            )?
            .iter()
            .map(|r| ((r.get(0), r.get(1)), r.get(2)))
            .collect())
    }

    /// Stores syntax facts. Rows are content-addressed, so an existing row is already correct.
    pub fn save_facts(&mut self, rows: &[(String, String, Vec<u8>)]) -> anyhow::Result<()> {
        let blobs: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
        let extractors: Vec<&str> = rows.iter().map(|r| r.1.as_str()).collect();
        let facts: Vec<&[u8]> = rows.iter().map(|r| r.2.as_slice()).collect();
        self.client.execute(
            "INSERT INTO rb.file_facts_cache (blob_sha, extractor, facts)
             SELECT * FROM unnest($1::text[], $2::text[], $3::bytea[])
             ON CONFLICT (blob_sha, extractor) DO NOTHING",
            &[&blobs, &extractors, &facts],
        )?;
        Ok(())
    }

    /// Reads a snapshot back into a [`Graph`] (used by `rb dump` and the determinism tests).
    pub fn load_graph(&mut self, snapshot_id: i64) -> anyhow::Result<Graph> {
        self.read_snapshot(snapshot_id, |s| s.graph_rows(snapshot_id))
    }

    /// Runs `f` with every statement on this connection inside one `REPEATABLE READ READ ONLY`
    /// transaction (contract M5 S1, invariant 4), so a command's reads see one state of the
    /// database whatever other sessions commit meanwhile. A call inside another is part of it.
    pub fn read_tx<T>(
        &mut self,
        f: impl FnOnce(&mut Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        if self.in_read {
            return f(self);
        }
        self.client
            .batch_execute("BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY")?;
        self.in_read = true;
        let result = f(self);
        self.in_read = false;
        // A failed read rolls back; its own error is the one reported.
        let end = self
            .client
            .batch_execute(if result.is_ok() { "COMMIT" } else { "ROLLBACK" });
        let value = result?;
        end?;
        Ok(value)
    }

    /// [`Store::read_tx`] over one snapshot: `snapshot {id} does not exist` when it was deleted
    /// before the transaction started; read whole when it is deleted afterwards.
    pub fn read_snapshot<T>(
        &mut self,
        snapshot_id: i64,
        f: impl FnOnce(&mut Self) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        self.read_tx(|s| {
            s.client
                .query_opt("SELECT 1 FROM rb.snapshot WHERE id = $1", &[&snapshot_id])?
                .with_context(|| format!("snapshot {snapshot_id} does not exist"))?;
            f(s)
        })
    }

    fn graph_rows(&mut self, snapshot_id: i64) -> anyhow::Result<Graph> {
        let nodes = self
            .client
            .query(
                "SELECT id, kind, key, name, attrs FROM rb.node WHERE snapshot_id = $1",
                &[&snapshot_id],
            )?
            .iter()
            .map(|r| {
                Ok(Node {
                    id: r.get(0),
                    kind: r.get::<_, String>(1).parse()?,
                    key: r.get(2),
                    name: r.get(3),
                    attrs: into_object(r.get(4))?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        #[cfg(test)]
        hooks::after_nodes();
        let edges = self
            .client
            .query(
                "SELECT src, dst, kind, provenance, certainty, attrs FROM rb.edge WHERE snapshot_id = $1",
                &[&snapshot_id],
            )?
            .iter()
            .map(|r| {
                Ok(Edge {
                    src: r.get(0),
                    dst: r.get(1),
                    kind: r.get::<_, String>(2).parse()?,
                    provenance: r.get::<_, String>(3).parse()?,
                    certainty: Certainty::from_rank(r.get(4))?,
                    attrs: into_object(r.get(5))?,
                    evidence: Vec::new(),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;

        let mut evidence: HashMap<(i64, EdgeKind, i64, Provenance), Vec<Evidence>> = HashMap::new();
        for r in self.client.query(
            "SELECT ev.src, ev.kind, ev.dst, ev.provenance, f.key, ev.span
             FROM rb.edge_evidence ev
             JOIN rb.node f ON f.snapshot_id = ev.snapshot_id AND f.id = ev.file_id
             WHERE ev.snapshot_id = $1",
            &[&snapshot_id],
        )? {
            let key = (
                r.get(0),
                r.get::<_, String>(1).parse()?,
                r.get(2),
                r.get::<_, String>(3).parse()?,
            );
            evidence.entry(key).or_default().push(Evidence {
                path: r.get(4),
                span: span_from_sql(r.get(5))?,
            });
        }
        let mut edges = edges;
        for e in &mut edges {
            if let Some(ev) = evidence.remove(&(e.src, e.kind, e.dst, e.provenance)) {
                e.evidence = ev;
            }
        }
        anyhow::ensure!(evidence.is_empty(), "evidence rows without a matching edge");
        Graph::from_parts(nodes, edges)
    }

    /// Reads a snapshot's diagnostics, in canonical order.
    pub fn load_diagnostics(&mut self, snapshot_id: i64) -> anyhow::Result<Vec<Diagnostic>> {
        self.read_snapshot(snapshot_id, |s| s.diagnostic_rows(snapshot_id))
    }

    fn diagnostic_rows(&mut self, snapshot_id: i64) -> anyhow::Result<Vec<Diagnostic>> {
        let mut out = self
            .client
            .query(
                "SELECT kind, path, span, detail FROM rb.diagnostic WHERE snapshot_id = $1",
                &[&snapshot_id],
            )?
            .iter()
            .map(|r| {
                Ok(Diagnostic {
                    kind: r.get(0),
                    path: r.get(1),
                    span: r
                        .get::<_, Option<Vec<i32>>>(2)
                        .map(span_from_sql)
                        .transpose()?,
                    detail: r.get(3),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        rb_model::sort_diagnostics(&mut out);
        Ok(out)
    }

    /// Runs a read-only Cypher query against the snapshot's AGE graph (contract M5 S7, D9): in one
    /// `REPEATABLE READ READ ONLY` transaction, under `limits.timeout_ms` as its statement
    /// timeout, as the role `rb_reader`, fetching at most `limits.rows + 1` rows; more than
    /// `limits.rows` returns the first `limits.rows` and `truncated: ["rows"]`. Each value is
    /// rendered as agtype text. Refusals are [`CypherError`]s.
    pub fn cypher(
        &mut self,
        snapshot_id: i64,
        query: &str,
        columns: &[String],
        limits: CypherLimits,
    ) -> anyhow::Result<CypherRows> {
        anyhow::ensure!(
            !self.in_read,
            "cypher runs in its own read-only transaction"
        );
        anyhow::ensure!(
            (1..=CYPHER_TIMEOUT_MS_MAX).contains(&limits.timeout_ms),
            "--timeout-ms must be between 1 and {CYPHER_TIMEOUT_MS_MAX}"
        );
        anyhow::ensure!(
            (1..=CYPHER_ROWS_MAX).contains(&limits.rows),
            "the Cypher row cap must be between 1 and {CYPHER_ROWS_MAX}"
        );
        let mut tx = self
            .client
            .build_transaction()
            .isolation_level(postgres::IsolationLevel::RepeatableRead)
            .read_only(true)
            .start()?;
        let row = tx
            .query_opt(
                "SELECT age_graph FROM rb.snapshot WHERE id = $1",
                &[&snapshot_id],
            )?
            .with_context(|| format!("snapshot {snapshot_id} does not exist"))?;
        let Some(graph) = row.get::<_, Option<String>>(0) else {
            return Err(CypherError::NoProjection {
                snapshot: snapshot_id,
            }
            .into());
        };
        let sql = format!(
            "{} LIMIT {}",
            age::cypher_sql(&graph, query, columns)?,
            limits.rows + 1
        );
        tx.batch_execute(&format!(
            "SET LOCAL statement_timeout = {}; SET LOCAL ROLE rb_reader",
            limits.timeout_ms
        ))?;
        let mut rows = tx
            .query(sql.as_str(), &[])
            .map_err(|e| cypher_error(e, limits))?;
        tx.commit()?;
        let truncated = if rows.len() > limits.rows {
            rows.truncate(limits.rows);
            vec!["rows"]
        } else {
            vec![]
        };
        let rows = rows
            .iter()
            .map(|r| {
                (0..columns.len())
                    .map(|i| {
                        r.get::<_, Option<String>>(i)
                            .unwrap_or_else(|| "null".into())
                    })
                    .collect()
            })
            .collect();
        Ok(CypherRows {
            columns: columns.to_vec(),
            rows,
            truncated,
        })
    }

    /// The live snapshot and its counts, read in one transaction (contract M5 S1).
    pub fn status(&mut self, repo: &str) -> anyhow::Result<RepoStatus> {
        self.read_tx(|s| s.status_rows(repo))
    }

    fn status_rows(&mut self, repo: &str) -> anyhow::Result<RepoStatus> {
        let live = self.live_snapshot(repo)?;
        let snapshots: i64 = self
            .client
            .query_one(
                "SELECT count(*) FROM rb.snapshot s JOIN rb.repo r ON r.id = s.repo_id WHERE r.name = $1",
                &[&repo],
            )?
            .get(0);
        let (mut nodes, mut edges, mut diagnostics) = (0, 0, 0);
        if let Some(s) = &live {
            let row = self.client.query_one(
                "SELECT (SELECT count(*) FROM rb.node WHERE snapshot_id = $1),
                        (SELECT count(*) FROM rb.edge WHERE snapshot_id = $1),
                        (SELECT count(*) FROM rb.diagnostic WHERE snapshot_id = $1)",
                &[&s.id],
            )?;
            (nodes, edges, diagnostics) = (row.get(0), row.get(1), row.get(2));
        }
        Ok(RepoStatus {
            name: repo.to_owned(),
            live,
            nodes,
            edges,
            diagnostics,
            snapshots,
        })
    }
}

const SNAPSHOT_SELECT: &str =
    "SELECT s.id, r.name, s.commit_sha, s.status, s.tool_versions, s.age_graph, s.parent_id
                               FROM rb.snapshot s JOIN rb.repo r ON r.id = s.repo_id";

fn snapshot_from_row(r: &postgres::Row) -> SnapshotInfo {
    SnapshotInfo {
        id: r.get(0),
        repo: r.get(1),
        commit_sha: r.get(2),
        status: r.get(3),
        tool_versions: r.get(4),
        age_graph: r.get(5),
        parent_id: r.get(6),
    }
}

fn span_from_sql(v: Vec<i32>) -> anyhow::Result<rb_model::Span> {
    v.try_into()
        .map_err(|v: Vec<i32>| anyhow::anyhow!("span must have 4 elements, got {}", v.len()))
}

fn into_object(v: Value) -> anyhow::Result<rb_model::Attrs> {
    match v {
        Value::Object(m) => Ok(m),
        other => bail!("attrs must be a JSON object, got {other}"),
    }
}

/// Hides the password in a `postgresql://user:pass@host/db` URL for error messages.
fn redact(url: &str) -> String {
    match (url.find("://"), url.rfind('@')) {
        (Some(s), Some(at)) if at > s => {
            let creds = &url[s + 3..at];
            match creds.split_once(':') {
                Some((user, _)) => format!("{}{user}:***{}", &url[..s + 3], &url[at..]),
                None => url.to_owned(),
            }
        }
        _ => url.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_passwords() {
        assert_eq!(
            redact("postgresql://rb:secret@h:1/db"),
            "postgresql://rb:***@h:1/db"
        );
        assert_eq!(redact("postgresql://rb@h/db"), "postgresql://rb@h/db");
        assert_eq!(redact("host=h user=rb"), "host=h user=rb");
    }
}
