//! The Repository Brain's request layer (contract M5 D2, D4, D5, S11, S12): one place that reads a
//! request's parameters, answers it from one snapshot, and renders the JSON envelope or the error,
//! so that `rb … --json` and the REST API cannot drift (invariant 2). No HTTP here: the CLI and
//! the server only turn their input into [`Params`] and print [`Output::body`] or
//! [`ApiError::body`].

pub mod error;
pub mod params;
pub mod render;

use std::collections::BTreeMap;
use std::sync::Arc;

use rb_model::{Certainty, NodeKind};
use rb_store::code::{CallerTree, Impact, Precision, SnapshotIndex};
use rb_store::limits::{self, Caps, Limit};
use rb_store::path::RequestPath;
use rb_store::{
    CypherLimits, CypherRows, DiCoverage, FoundNode, HttpCoverage, ResolutionCoverage, Resolved,
    RtaCoverage, ServiceCard, ServiceSummary, SnapshotInfo, SqlCoverage, Store, TableDeps,
};
use serde::{Serialize, Serializer};
use serde_json::{Value, json};

pub use error::{ApiError, ErrorKind};
pub use params::{Params, parse_certainty, parse_precision};

/// The `api` field of every body.
pub const API_VERSION: u32 = 1;

/// `rb find`'s page size: default and maximum.
pub const FIND_LIMIT_DEFAULT: u64 = 20;
pub const FIND_LIMIT_MAX: u64 = 1_000;

/// What a request asks, as a path segment of `/v1/repos/{repo}/…` and a CLI command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Endpoint {
    Status,
    Snapshots,
    Find,
    Services,
    Service,
    Callers,
    Impact,
    TableDeps,
    Path,
    Cypher,
}

impl Endpoint {
    pub const ALL: [Endpoint; 10] = [
        Endpoint::Status,
        Endpoint::Snapshots,
        Endpoint::Find,
        Endpoint::Services,
        Endpoint::Service,
        Endpoint::Callers,
        Endpoint::Impact,
        Endpoint::TableDeps,
        Endpoint::Path,
        Endpoint::Cypher,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Endpoint::Status => "status",
            Endpoint::Snapshots => "snapshots",
            Endpoint::Find => "find",
            Endpoint::Services => "services",
            Endpoint::Service => "service",
            Endpoint::Callers => "callers",
            Endpoint::Impact => "impact",
            Endpoint::TableDeps => "table-deps",
            Endpoint::Path => "path",
            Endpoint::Cypher => "cypher",
        }
    }

    pub fn from_segment(s: &str) -> Option<Endpoint> {
        Endpoint::ALL.into_iter().find(|e| e.as_str() == s)
    }

    /// The parameters it takes; any other is refused. Every question takes `snapshot` (D17);
    /// `status` and `snapshots` describe the repository, not one snapshot.
    pub fn params(self) -> &'static [&'static str] {
        match self {
            Endpoint::Status | Endpoint::Snapshots => &[],
            Endpoint::Find => &["q", "kind", "limit", "snapshot"],
            Endpoint::Services => &["snapshot"],
            Endpoint::Service => &["name", "snapshot"],
            Endpoint::Callers => &[
                "target",
                "depth",
                "include_references",
                "min_certainty",
                "precision",
                "pick",
                "snapshot",
            ],
            Endpoint::Impact => &["target", "min_certainty", "precision", "pick", "snapshot"],
            Endpoint::TableDeps => &["table", "precision", "hops", "snapshot"],
            Endpoint::Path => &[
                "endpoint",
                "depth",
                "hops",
                "full",
                "precision",
                "pick",
                "snapshot",
            ],
            Endpoint::Cypher => &["query", "columns", "timeout_ms", "snapshot"],
        }
    }
}

/// What a deployment sets once: the caps every question runs under and Cypher's default
/// statement timeout (the server's `--cypher-timeout-ms`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    pub caps: Caps,
    pub cypher_timeout_ms: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            caps: Caps::default(),
            cypher_timeout_ms: limits::CYPHER_TIMEOUT_MS_DEFAULT,
        }
    }
}

impl Settings {
    /// The environment variable holding the test-only cap override (contract M5 S14).
    pub const TEST_CAPS_VAR: &'static str = "RB_TEST_CAPS";

    /// Lowers caps for tests (contract M5 S14: "a truncating cap lowered by a test-only limit
    /// override"): `nodes=N,tree=N,work=N`, each at most D6's value; raising one is refused. The
    /// caps in force are in every envelope's `limits`, so an overridden answer says so.
    pub fn with_test_caps(mut self, spec: &str) -> Result<Self, String> {
        let var = Self::TEST_CAPS_VAR;
        let defaults = Caps::default();
        for part in spec.split(',').filter(|p| !p.is_empty()) {
            let (name, value) = part
                .split_once('=')
                .ok_or_else(|| format!("{var}: expected name=value, got {part:?}"))?;
            let n: u64 = value
                .parse()
                .map_err(|_| format!("{var}: {name} must be a number, got {value:?}"))?;
            let max = match name {
                "nodes" => defaults.nodes as u64,
                "tree" => defaults.tree as u64,
                "work" => defaults.work,
                other => return Err(format!("{var}: unknown cap {other:?} (nodes, tree, work)")),
            };
            if n > max {
                return Err(format!("{var}: {name} may only be lowered (at most {max})"));
            }
            let size = || usize::try_from(n).expect("at most a default");
            match name {
                "nodes" => self.caps.nodes = size(),
                "tree" => self.caps.tree = size(),
                _ => self.caps.work = n,
            }
        }
        Ok(self)
    }
}

/// A validated request.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    /// `None`: the only repository (the CLI without `--repo`); for `status`, every repository.
    pub repo: Option<String>,
    /// A kept snapshot of the repository (D17); `None`: its live snapshot.
    pub snapshot: Option<i64>,
    pub query: Query,
}

/// A request's parameters, typed, with every default filled in.
#[derive(Debug, Clone, PartialEq)]
pub enum Query {
    Status,
    Snapshots,
    Find {
        q: String,
        kind: Option<NodeKind>,
        limit: u64,
    },
    Services,
    Service {
        name: String,
    },
    Callers {
        target: String,
        depth: usize,
        include_references: bool,
        min_certainty: Certainty,
        precision: Precision,
        pick: Option<usize>,
    },
    Impact {
        target: String,
        min_certainty: Certainty,
        precision: Precision,
        pick: Option<usize>,
    },
    TableDeps {
        table: String,
        precision: Precision,
        hops: usize,
    },
    Path {
        endpoint: String,
        depth: usize,
        hops: usize,
        full: bool,
        precision: Precision,
        pick: Option<usize>,
    },
    Cypher {
        query: String,
        columns: Vec<String>,
        timeout_ms: u64,
    },
}

/// The largest `pick` accepted; a pick beyond the candidates lists them again.
const PICK_MAX: u64 = u32::MAX as u64;

/// Reads `params` for `endpoint` (contract M5 S12): unknown parameters, missing required ones and
/// values outside D6's bounds are `invalid_param`.
pub fn parse(
    endpoint: Endpoint,
    repo: Option<&str>,
    p: &Params,
    settings: &Settings,
) -> Result<Request, ApiError> {
    p.only(endpoint.params())?;
    let usize_of = |n: u64| usize::try_from(n).expect("bounded by D6");
    let pick =
        || -> Result<Option<usize>, ApiError> { Ok(p.int_opt("pick", 1, PICK_MAX)?.map(usize_of)) };
    let query = match endpoint {
        Endpoint::Status => Query::Status,
        Endpoint::Snapshots => Query::Snapshots,
        Endpoint::Find => Query::Find {
            q: p.text("q")?,
            kind: p.kind_opt("kind")?,
            limit: p.int("limit", FIND_LIMIT_DEFAULT, 1, FIND_LIMIT_MAX)?,
        },
        Endpoint::Services => Query::Services,
        Endpoint::Service => Query::Service {
            name: p.text("name")?,
        },
        Endpoint::Callers => Query::Callers {
            target: p.text("target")?,
            depth: usize_of(p.int(
                "depth",
                limits::CALLER_DEPTH_DEFAULT as u64,
                1,
                limits::CALLER_DEPTH_MAX as u64,
            )?),
            include_references: p.boolean("include_references")?,
            min_certainty: p.certainty("min_certainty", Certainty::Ambiguous)?,
            precision: p.precision("precision", Precision::Cha)?,
            pick: pick()?,
        },
        Endpoint::Impact => Query::Impact {
            target: p.text("target")?,
            min_certainty: p.certainty("min_certainty", Certainty::Ambiguous)?,
            precision: p.precision("precision", Precision::Cha)?,
            pick: pick()?,
        },
        Endpoint::TableDeps => Query::TableDeps {
            table: p.text("table")?,
            precision: p.precision("precision", Precision::Cha)?,
            hops: usize_of(p.int(
                "hops",
                limits::HOPS_DEFAULT as u64,
                0,
                limits::HOPS_MAX as u64,
            )?),
        },
        Endpoint::Path => Query::Path {
            endpoint: p.text("endpoint")?,
            depth: usize_of(p.int(
                "depth",
                limits::PATH_DEPTH_DEFAULT as u64,
                1,
                limits::PATH_DEPTH_MAX as u64,
            )?),
            hops: usize_of(p.int(
                "hops",
                limits::HOPS_DEFAULT as u64,
                0,
                limits::HOPS_MAX as u64,
            )?),
            full: p.boolean("full")?,
            precision: p.precision("precision", Precision::Cha)?,
            pick: pick()?,
        },
        Endpoint::Cypher => Query::Cypher {
            query: p.text("query")?,
            columns: columns(p)?,
            timeout_ms: p.int(
                "timeout_ms",
                settings.cypher_timeout_ms,
                1,
                limits::CYPHER_TIMEOUT_MS_MAX,
            )?,
        },
    };
    Ok(Request {
        repo: repo.map(str::to_owned),
        snapshot: p
            .int_opt("snapshot", 1, i64::MAX as u64)?
            .map(|id| i64::try_from(id).expect("bounded")),
        query,
    })
}

/// Cypher's result columns: comma-separated names `[a-z_][a-z0-9_]*`, `result` by default.
fn columns(p: &Params) -> Result<Vec<String>, ApiError> {
    let Some(text) = p.get("columns") else {
        return Ok(vec!["result".to_owned()]);
    };
    let names: Vec<String> = text.split(',').map(str::to_owned).collect();
    let ok = |c: &String| {
        let mut chars = c.chars();
        chars
            .next()
            .is_some_and(|f| f.is_ascii_lowercase() || f == '_')
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    if names.iter().all(ok) {
        Ok(names)
    } else {
        Err(ApiError::invalid(
            "columns",
            format_args!("must be comma-separated names [a-z_][a-z0-9_]* (got {text:?})"),
        ))
    }
}

impl Query {
    pub fn endpoint(&self) -> Endpoint {
        match self {
            Query::Status => Endpoint::Status,
            Query::Snapshots => Endpoint::Snapshots,
            Query::Find { .. } => Endpoint::Find,
            Query::Services => Endpoint::Services,
            Query::Service { .. } => Endpoint::Service,
            Query::Callers { .. } => Endpoint::Callers,
            Query::Impact { .. } => Endpoint::Impact,
            Query::TableDeps { .. } => Endpoint::TableDeps,
            Query::Path { .. } => Endpoint::Path,
            Query::Cypher { .. } => Endpoint::Cypher,
        }
    }

    /// The envelope's `query`: every parameter, defaults filled in, typed (D4).
    pub fn normalised(&self) -> Value {
        match self {
            Query::Status | Query::Snapshots => json!({}),
            Query::Find { q, kind, limit } => {
                json!({"q": q, "kind": kind.map(NodeKind::as_str), "limit": limit})
            }
            Query::Services => json!({}),
            Query::Service { name } => json!({ "name": name }),
            Query::Callers {
                target,
                depth,
                include_references,
                min_certainty,
                precision,
                pick,
            } => json!({
                "target": target,
                "depth": depth,
                "include_references": include_references,
                "min_certainty": min_certainty.as_str(),
                "precision": precision.as_str(),
                "pick": pick,
            }),
            Query::Impact {
                target,
                min_certainty,
                precision,
                pick,
            } => json!({
                "target": target,
                "min_certainty": min_certainty.as_str(),
                "precision": precision.as_str(),
                "pick": pick,
            }),
            Query::TableDeps {
                table,
                precision,
                hops,
            } => json!({"table": table, "precision": precision.as_str(), "hops": hops}),
            Query::Path {
                endpoint,
                depth,
                hops,
                full,
                precision,
                pick,
            } => json!({
                "endpoint": endpoint,
                "depth": depth,
                "hops": hops,
                "full": full,
                "precision": precision.as_str(),
                "pick": pick,
            }),
            Query::Cypher {
                query,
                columns,
                timeout_ms,
            } => json!({"query": query, "columns": columns, "timeout_ms": timeout_ms}),
        }
    }
}

/// The snapshot an answer was read from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SnapshotRef {
    pub id: i64,
    pub commit_sha: String,
}

/// What a request computed, before rendering: the CLI renders it as text, and both surfaces
/// serialise it as the envelope's `result`.
pub enum Answer {
    /// One repository's status, or every repository's (`all`).
    Status {
        all: bool,
        reports: Vec<StatusReport>,
    },
    Snapshots(Vec<SnapshotInfo>),
    Find(Vec<FoundNode>),
    Services(Vec<ServiceSummary>),
    Card(Box<ServiceCard>),
    Callers {
        ix: Arc<SnapshotIndex>,
        tree: CallerTree,
    },
    Impact {
        ix: Arc<SnapshotIndex>,
        impact: Box<Impact>,
    },
    TableDeps(Box<TableDeps>),
    Path {
        ix: Arc<SnapshotIndex>,
        path: Box<RequestPath>,
    },
    Cypher(CypherRows),
}

impl Serialize for Answer {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Answer::Status { all: true, reports } => json!({ "repos": reports }).serialize(s),
            Answer::Status {
                all: false,
                reports,
            } => reports
                .first()
                .expect("one report per repository")
                .serialize(s),
            Answer::Snapshots(snapshots) => snapshots
                .iter()
                .map(|x| {
                    json!({
                        "id": x.id,
                        "commit_sha": x.commit_sha,
                        "status": x.status,
                        "parent_id": x.parent_id,
                        "age_graph": x.age_graph,
                        "tool_versions": x.tool_versions,
                    })
                })
                .collect::<Vec<_>>()
                .serialize(s),
            Answer::Find(found) => found.serialize(s),
            Answer::Services(services) => services.serialize(s),
            Answer::Card(card) => card.serialize(s),
            Answer::Callers { ix, tree } => render::callers_json(ix, tree).serialize(s),
            Answer::Impact { ix, impact } => render::impact_json(ix, impact).serialize(s),
            Answer::TableDeps(deps) => deps.serialize(s),
            Answer::Path { ix, path } => render::path_json(ix, path).serialize(s),
            Answer::Cypher(rows) => {
                json!({"columns": rows.columns, "rows": rows.rows}).serialize(s)
            }
        }
    }
}

/// A computed answer with the envelope's fields (D4).
pub struct Output {
    pub repo: Option<String>,
    pub snapshot: Option<SnapshotRef>,
    /// The normalised parameters.
    pub query: Value,
    /// The caps in force that can cut this answer, by limit name.
    pub limits: BTreeMap<&'static str, u64>,
    /// The limits that cut it, sorted.
    pub truncated: Vec<String>,
    /// Certainty → rows, when the answer's rows carry one.
    pub certainty: Option<BTreeMap<&'static str, usize>>,
    pub answer: Answer,
}

impl Output {
    /// The response body and `rb … --json`'s output (D4): the envelope, pretty-printed, with a
    /// final newline. Deterministic: no time, host path or request id (invariant 8).
    pub fn body(&self) -> String {
        #[derive(Serialize)]
        struct Envelope<'a> {
            api: u32,
            repo: Option<&'a str>,
            snapshot: Option<&'a SnapshotRef>,
            query: &'a Value,
            limits: &'a BTreeMap<&'static str, u64>,
            truncated: &'a [String],
            #[serde(skip_serializing_if = "Option::is_none")]
            certainty: Option<&'a BTreeMap<&'static str, usize>>,
            result: &'a Answer,
        }
        pretty(&Envelope {
            api: API_VERSION,
            repo: self.repo.as_deref(),
            snapshot: self.snapshot.as_ref(),
            query: &self.query,
            limits: &self.limits,
            truncated: &self.truncated,
            certainty: self.certainty.as_ref(),
            result: &self.answer,
        })
    }
}

fn pretty(v: &impl Serialize) -> String {
    let mut out = serde_json::to_string_pretty(v).expect("an answer serialises");
    out.push('\n');
    out
}

/// Where a request's snapshot index comes from: the CLI builds one per invocation; the server
/// keeps one per live snapshot (D3).
pub trait Indexes {
    /// The index of `snapshot`, read inside the request's read transaction when it is not at
    /// hand; `live` says whether it is its repository's live snapshot.
    fn index(
        &mut self,
        store: &mut Store,
        snapshot: &SnapshotInfo,
        live: bool,
    ) -> anyhow::Result<Arc<SnapshotIndex>>;
}

/// Loads the graph and builds the index for every request (the CLI's embedded mode).
pub struct LoadEach;

impl Indexes for LoadEach {
    fn index(
        &mut self,
        store: &mut Store,
        snapshot: &SnapshotInfo,
        _live: bool,
    ) -> anyhow::Result<Arc<SnapshotIndex>> {
        Ok(Arc::new(SnapshotIndex::new(store.load_graph(snapshot.id)?)))
    }
}

/// Answers `req`: the repository's snapshot and every read of it in one `REPEATABLE READ READ
/// ONLY` transaction (contract M5 S1); Cypher in its own read-only transaction after the
/// snapshot is resolved.
pub fn run(
    store: &mut Store,
    indexes: &mut dyn Indexes,
    req: &Request,
    settings: &Settings,
) -> Result<Output, ApiError> {
    if let Query::Cypher {
        query,
        columns,
        timeout_ms,
    } = &req.query
    {
        let (snap, _) = store.read_tx(|s| {
            let repo = resolve_repo(s, req.repo.as_deref())?;
            Ok(resolve_snapshot(s, &repo, req.snapshot)?)
        })?;
        let rows = store.cypher(
            snap.id,
            query,
            columns,
            CypherLimits {
                timeout_ms: *timeout_ms,
                ..Default::default()
            },
        )?;
        return Ok(Output {
            repo: Some(snap.repo.clone()),
            snapshot: Some(snapshot_ref(&snap)),
            query: req.query.normalised(),
            limits: BTreeMap::from([(Limit::Rows.as_str(), limits::ROWS as u64)]),
            truncated: rows.truncated.iter().map(|l| (*l).to_owned()).collect(),
            certainty: None,
            answer: Answer::Cypher(rows),
        });
    }
    Ok(store.read_tx(|s| match &req.query {
        Query::Status => Ok(status(s, req.repo.as_deref())?),
        Query::Snapshots => {
            let repo = resolve_repo(s, req.repo.as_deref())?;
            let live = live_snapshot(s, &repo).ok();
            if live.is_none() && !s.repo_names()?.contains(&repo) {
                return Err(not_indexed(&repo).into());
            }
            Ok(Output {
                repo: Some(repo.clone()),
                snapshot: live.as_ref().map(snapshot_ref),
                query: req.query.normalised(),
                limits: BTreeMap::new(),
                truncated: Vec::new(),
                certainty: None,
                answer: Answer::Snapshots(s.snapshots(&repo)?),
            })
        }
        _ => {
            let repo = resolve_repo(s, req.repo.as_deref())?;
            let (snap, live) = resolve_snapshot(s, &repo, req.snapshot)?;
            s.read_snapshot(snap.id, |s| {
                Ok(answer(s, indexes, &snap, live, &req.query, settings)?)
            })
        }
    })?)
}

/// One repository's status (`rb status`, `GET /v1/repos/{repo}/status`): its live snapshot,
/// counts, and the coverage lines as data.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusReport {
    pub name: String,
    pub live: Option<LiveRef>,
    pub nodes: i64,
    pub edges: i64,
    pub diagnostics: i64,
    /// Snapshot rows kept (live, retired, failed).
    pub snapshots: i64,
    /// The live snapshot's coverage; absent without one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<Coverage>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveRef {
    pub id: i64,
    pub commit_sha: String,
    pub age_graph: Option<String>,
}

/// `rb status`'s coverage lines (design §12.4) as data.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Coverage {
    pub sql: SqlCoverage,
    pub code: ResolutionCoverage,
    pub http: HttpCoverage,
    pub di: DiCoverage,
    pub rta: RtaCoverage,
    /// The `scip_status` diagnostics, one per project: `{path, detail}`.
    #[serde(serialize_with = "scip_jobs")]
    pub scip: Vec<rb_model::Diagnostic>,
}

fn scip_jobs<S: Serializer>(jobs: &[rb_model::Diagnostic], s: S) -> Result<S::Ok, S::Error> {
    jobs.iter()
        .map(|d| json!({"path": d.path, "detail": d.detail}))
        .collect::<Vec<_>>()
        .serialize(s)
}

/// The status of `repo`, or of every repository (inside the read transaction).
fn status(store: &mut Store, repo: Option<&str>) -> Result<Output, ApiError> {
    let names = match repo {
        Some(r) => vec![r.to_owned()],
        None => store.repo_names()?,
    };
    let mut reports = Vec::new();
    for name in names {
        let st = store.status(&name)?;
        let coverage = match &st.live {
            Some(live) => Some(Coverage {
                sql: store.sql_coverage(live.id)?,
                code: store.resolution_coverage(live.id)?,
                http: store.http_coverage(live.id)?,
                di: store.di_coverage(live.id)?,
                rta: store.rta_coverage(live.id)?,
                scip: store
                    .load_diagnostics(live.id)?
                    .into_iter()
                    .filter(|d| d.kind == "scip_status")
                    .collect(),
            }),
            None => None,
        };
        reports.push(StatusReport {
            name: st.name,
            live: st.live.as_ref().map(|l| LiveRef {
                id: l.id,
                commit_sha: l.commit_sha.clone(),
                age_graph: l.age_graph.clone(),
            }),
            nodes: st.nodes,
            edges: st.edges,
            diagnostics: st.diagnostics,
            snapshots: st.snapshots,
            coverage,
        });
    }
    let single = repo.and_then(|_| reports.first());
    Ok(Output {
        repo: repo.map(str::to_owned),
        snapshot: single.and_then(|r| r.live.as_ref()).map(|l| SnapshotRef {
            id: l.id,
            commit_sha: l.commit_sha.clone(),
        }),
        query: Query::Status.normalised(),
        limits: BTreeMap::new(),
        truncated: Vec::new(),
        certainty: None,
        answer: Answer::Status {
            all: repo.is_none(),
            reports,
        },
    })
}

/// The snapshot a question reads (D17): the live one, or `requested`, which must be a kept
/// (`live` or `retired`) snapshot of `repo`; a deleted one is `snapshot_not_retained`. Returns it
/// and whether it is the live one.
fn resolve_snapshot(
    store: &mut Store,
    repo: &str,
    requested: Option<i64>,
) -> Result<(SnapshotInfo, bool), ApiError> {
    let Some(id) = requested else {
        return Ok((live_snapshot(store, repo)?, true));
    };
    if !store.repo_names()?.iter().any(|r| r == repo) {
        return Err(not_indexed(repo));
    }
    match store.snapshot_opt(id)? {
        Some(snap) if snap.repo != repo => Err(ApiError::not_found(format!(
            "snapshot {id} is not a snapshot of {repo:?}"
        ))),
        Some(snap) if snap.status == "live" || snap.status == "retired" => {
            let live = store.live_snapshot(repo)?.is_some_and(|l| l.id == id);
            Ok((snap, live))
        }
        Some(snap) => Err(ApiError::not_found(format!(
            "snapshot {id} of {repo:?} is {} and holds no graph",
            snap.status
        ))),
        None if store.snapshot_issued(id)? => Err(ApiError::new(
            ErrorKind::SnapshotNotRetained,
            format!("snapshot {id} is no longer kept (`rb snapshots` lists the kept ones)"),
        )),
        None => Err(ApiError::not_found(format!("snapshot {id} does not exist"))),
    }
}

fn not_indexed(repo: &str) -> ApiError {
    ApiError::not_found(format!("repository {repo:?} is not indexed"))
}

fn snapshot_ref(snap: &SnapshotInfo) -> SnapshotRef {
    SnapshotRef {
        id: snap.id,
        commit_sha: snap.commit_sha.clone(),
    }
}

/// The repository a request names, or the only one indexed.
fn resolve_repo(store: &mut Store, repo: Option<&str>) -> Result<String, ApiError> {
    if let Some(r) = repo {
        return Ok(r.to_owned());
    }
    let names = store.repo_names()?;
    match names.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(ApiError::not_found("no repositories indexed yet")),
        many => Err(ApiError {
            param: Some("repo".into()),
            ..ApiError::new(
                ErrorKind::InvalidParam,
                format!(
                    "several repositories indexed; pass --repo (one of: {})",
                    many.join(", ")
                ),
            )
        }),
    }
}

/// The repository's live snapshot: `not_found` for a repository never indexed,
/// `no_live_snapshot` for one without a live snapshot.
fn live_snapshot(store: &mut Store, repo: &str) -> Result<SnapshotInfo, ApiError> {
    if let Some(snap) = store.live_snapshot(repo)? {
        return Ok(snap);
    }
    if store.repo_names()?.iter().any(|r| r == repo) {
        Err(ApiError::new(
            ErrorKind::NoLiveSnapshot,
            format!("repository {repo:?} has no live snapshot"),
        ))
    } else {
        Err(not_indexed(repo))
    }
}

fn limits_of(names: &[(Limit, u64)]) -> BTreeMap<&'static str, u64> {
    names.iter().map(|(l, v)| (l.as_str(), *v)).collect()
}

fn histogram(certainties: impl Iterator<Item = Certainty>) -> BTreeMap<&'static str, usize> {
    let mut out = BTreeMap::new();
    for c in certainties {
        *out.entry(c.as_str()).or_default() += 1;
    }
    out
}

fn names(limits: &std::collections::BTreeSet<Limit>) -> Vec<String> {
    limits.iter().map(|l| l.as_str().to_owned()).collect()
}

/// Computes `query` on snapshot `snap` (inside its read transaction).
fn answer(
    store: &mut Store,
    indexes: &mut dyn Indexes,
    snap: &SnapshotInfo,
    live: bool,
    query: &Query,
    settings: &Settings,
) -> Result<Output, ApiError> {
    let caps = settings.caps;
    let mut out = Output {
        repo: Some(snap.repo.clone()),
        snapshot: Some(snapshot_ref(snap)),
        query: query.normalised(),
        limits: BTreeMap::new(),
        truncated: Vec::new(),
        certainty: None,
        answer: Answer::Services(Vec::new()),
    };
    out.answer = match query {
        Query::Find { q, kind, limit } => {
            Answer::Find(store.find(snap.id, q, *kind, i64::try_from(*limit).expect("bounded"))?)
        }
        Query::Services => Answer::Services(store.services(snap.id)?),
        Query::Service { name } => {
            let found = match store.resolve_service(snap.id, name)? {
                Resolved::One(n) => n,
                Resolved::None => {
                    return Err(ApiError::not_found(format!(
                        "no service matches {name:?} (try `rb service` to list them)"
                    )));
                }
                Resolved::Ambiguous(candidates) => {
                    let list: Vec<String> = candidates
                        .iter()
                        .map(|c| format!("  {}  ({})", c.key, c.name))
                        .collect();
                    return Err(ApiError::ambiguous(
                        format!(
                            "{name:?} matches several services; pass a directory:\n{}",
                            list.join("\n")
                        ),
                        candidates.iter().map(|c| json!(c)).collect(),
                    ));
                }
            };
            let ix = indexes.index(store, snap, live)?;
            // The card's derived sections read adjacency: one budget for the card (amendment 4).
            let (card, spent) =
                rb_store::work::metered(caps.work, || store.service_card(snap.id, &ix, &found.key));
            out.limits = limits_of(&[(Limit::Work, caps.work)]);
            if spent.refused {
                out.truncated = vec![Limit::Work.as_str().to_owned()];
            }
            Answer::Card(Box::new(card?))
        }
        Query::Callers {
            target,
            depth,
            include_references,
            min_certainty,
            precision,
            pick,
        } => {
            let ix = indexes.index(store, snap, live)?;
            let t = render::pick_target(&ix, target, *pick)?;
            let tree = rb_store::code::callers(
                &ix,
                t,
                rb_store::code::CallerOptions {
                    depth: *depth,
                    include_references: *include_references,
                    min_certainty: *min_certainty,
                    precision: *precision,
                    caps,
                },
            );
            out.limits = limits_of(&[(Limit::Tree, caps.tree as u64), (Limit::Work, caps.work)]);
            out.truncated = names(&tree.truncated);
            out.certainty = Some(histogram(caller_certainties(&tree.callers)));
            Answer::Callers { ix, tree }
        }
        Query::Impact {
            target,
            min_certainty,
            precision,
            pick,
        } => {
            let ix = indexes.index(store, snap, live)?;
            let t = render::pick_target(&ix, target, *pick)?;
            let impact = rb_store::code::impact_capped(&ix, t, *min_certainty, *precision, caps);
            out.limits = limits_of(&[(Limit::Nodes, caps.nodes as u64), (Limit::Work, caps.work)]);
            out.truncated = names(&impact.truncated);
            out.certainty = Some(histogram(
                impact
                    .breaks
                    .iter()
                    .chain(&impact.affects)
                    .map(|x| x.certainty),
            ));
            Answer::Impact {
                ix,
                impact: Box::new(impact),
            }
        }
        Query::TableDeps {
            table,
            precision,
            hops,
        } => {
            let found = match store.resolve_table(snap.id, table)? {
                Resolved::One(n) => n,
                Resolved::None => {
                    return Err(ApiError::not_found(format!(
                        "no table matches {table:?} (try `rb find {table} --kind table`)"
                    )));
                }
                Resolved::Ambiguous(candidates) => {
                    let list: Vec<String> =
                        candidates.iter().map(|c| format!("  {}", c.key)).collect();
                    return Err(ApiError::ambiguous(
                        format!(
                            "{table:?} matches several tables; pass schema.table:\n{}",
                            list.join("\n")
                        ),
                        candidates.iter().map(|c| json!(c)).collect(),
                    ));
                }
            };
            let ix = indexes.index(store, snap, live)?;
            let deps = store.table_deps(
                snap.id,
                &ix,
                &found.key,
                rb_store::rollup::RollupOptions {
                    precision: *precision,
                    hops: *hops,
                    caps,
                },
            )?;
            out.limits = limits_of(&[
                (Limit::Nodes, caps.nodes as u64),
                (Limit::Tree, caps.tree as u64),
                (Limit::Work, caps.work),
            ]);
            out.truncated = deps.rollup.truncated.clone();
            out.certainty = Some(histogram(
                deps.migrations
                    .iter()
                    .chain(&deps.mappings)
                    .chain(&deps.accesses)
                    .filter_map(|a| parse_certainty(&a.certainty).ok()),
            ));
            Answer::TableDeps(Box::new(deps))
        }
        Query::Path {
            endpoint,
            depth,
            hops,
            full,
            precision,
            pick,
        } => {
            let ix = indexes.index(store, snap, live)?;
            let e = render::pick_endpoint(&ix, endpoint, *pick)?;
            let path = rb_store::path::request_path(
                &ix,
                e,
                rb_store::path::PathOptions {
                    depth: *depth,
                    hops: *hops,
                    full: *full,
                    precision: *precision,
                    caps,
                },
            );
            out.limits = limits_of(&[(Limit::Tree, caps.tree as u64), (Limit::Work, caps.work)]);
            out.truncated = names(&path.truncated);
            out.certainty = Some(
                path.histogram
                    .iter()
                    .map(|(c, n)| (c.as_str(), *n))
                    .collect(),
            );
            Answer::Path {
                ix,
                path: Box::new(path),
            }
        }
        Query::Status | Query::Snapshots | Query::Cypher { .. } => {
            unreachable!("answered by `run` itself")
        }
    };
    Ok(out)
}

/// Every caller entry's certainty, depth first.
fn caller_certainties(callers: &[rb_store::code::Caller]) -> impl Iterator<Item = Certainty> {
    let mut all = Vec::new();
    fn walk(cs: &[rb_store::code::Caller], all: &mut Vec<Certainty>) {
        for c in cs {
            all.push(c.certainty);
            walk(&c.callers, all);
        }
    }
    walk(callers, &mut all);
    all.into_iter()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_pairs(endpoint: Endpoint, pairs: &[(&str, &str)]) -> Result<Request, ApiError> {
        parse(
            endpoint,
            Some("r"),
            &Params::from_pairs(pairs.iter().copied()).unwrap(),
            &Settings::default(),
        )
    }

    /// Contract M5 S12, D6: per bounded parameter, its maximum is accepted and one above is
    /// `invalid_param`, never clamped.
    #[test]
    fn each_bound_accepts_its_maximum_and_refuses_one_above() {
        type Case<'a> = (Endpoint, &'a [(&'a str, &'a str)], &'a str, u64);
        let cases: &[Case] = &[
            (Endpoint::Callers, &[("target", "x")], "depth", 25),
            (Endpoint::Path, &[("endpoint", "x")], "depth", 25),
            (Endpoint::Path, &[("endpoint", "x")], "hops", 8),
            (Endpoint::TableDeps, &[("table", "x")], "hops", 8),
            (Endpoint::Find, &[("q", "x")], "limit", 1_000),
            (
                Endpoint::Cypher,
                &[("query", "RETURN 1")],
                "timeout_ms",
                30_000,
            ),
            (
                Endpoint::Callers,
                &[("target", "x")],
                "pick",
                u64::from(u32::MAX),
            ),
        ];
        for (endpoint, base, param, max) in cases {
            let at = max.to_string();
            let mut pairs = base.to_vec();
            pairs.push((param, &at));
            assert!(
                parse_pairs(*endpoint, &pairs).is_ok(),
                "{endpoint:?} {param}={max}"
            );
            let above = (max + 1).to_string();
            let mut pairs = base.to_vec();
            pairs.push((param, &above));
            let e = parse_pairs(*endpoint, &pairs).unwrap_err();
            assert_eq!(
                (e.kind, e.param.as_deref()),
                (ErrorKind::InvalidParam, Some(*param)),
                "{endpoint:?} {param}={above}"
            );
        }
        // Lower bounds: depth and limit start at 1, hops at 0.
        assert!(parse_pairs(Endpoint::Callers, &[("target", "x"), ("depth", "0")]).is_err());
        assert!(parse_pairs(Endpoint::Path, &[("endpoint", "x"), ("hops", "0")]).is_ok());
        assert!(parse_pairs(Endpoint::Find, &[("q", "x"), ("limit", "0")]).is_err());
        assert!(parse_pairs(Endpoint::Callers, &[("target", "x"), ("pick", "0")]).is_err());
    }

    #[test]
    fn requests_need_their_parameters_and_refuse_others() {
        for endpoint in Endpoint::ALL {
            let e = parse_pairs(endpoint, &[("bogus", "1")]).unwrap_err();
            assert_eq!(e.param.as_deref(), Some("bogus"), "{endpoint:?}");
            assert_eq!(Endpoint::from_segment(endpoint.as_str()), Some(endpoint));
        }
        assert_eq!(
            parse_pairs(Endpoint::Callers, &[]).unwrap_err().message,
            "target is required"
        );
        let e = parse_pairs(
            Endpoint::Cypher,
            &[("query", "RETURN 1"), ("columns", "a,B")],
        )
        .unwrap_err();
        assert_eq!(e.param.as_deref(), Some("columns"));
    }

    /// Contract M5 S14: the test-only override lowers caps and never raises them.
    #[test]
    fn test_caps_only_lower() {
        let s = Settings::default()
            .with_test_caps("tree=3,work=10")
            .unwrap();
        assert_eq!(
            (s.caps.tree, s.caps.work, s.caps.nodes),
            (3, 10, limits::NODES)
        );
        assert_eq!(
            Settings::default().with_test_caps("").unwrap(),
            Settings::default()
        );
        for bad in [
            "work=20000001",
            "nodes=200001",
            "tree=5001",
            "depth=1",
            "work",
            "work=x",
        ] {
            assert!(Settings::default().with_test_caps(bad).is_err(), "{bad}");
        }
    }

    /// D4: the normalised query fills every default, so two spellings of one request have one
    /// envelope.
    #[test]
    fn the_normalised_query_fills_defaults() {
        let r = parse_pairs(Endpoint::Callers, &[("target", "x")]).unwrap();
        assert_eq!(
            r.query.normalised(),
            json!({"target": "x", "depth": 1, "include_references": false,
                   "min_certainty": "ambiguous", "precision": "cha", "pick": null})
        );
        let explicit = parse_pairs(
            Endpoint::Callers,
            &[
                ("target", "x"),
                ("depth", "01"),
                ("include_references", "false"),
                ("precision", "cha"),
            ],
        )
        .unwrap();
        assert_eq!(explicit, r);
        let p = parse_pairs(Endpoint::Path, &[("endpoint", "GET /")]).unwrap();
        assert_eq!(
            p.query.normalised(),
            json!({"endpoint": "GET /", "depth": 12, "hops": 4, "full": false,
                   "precision": "cha", "pick": null})
        );
        let c = parse(
            Endpoint::Cypher,
            None,
            &Params::from_pairs([("query", "RETURN 1")]).unwrap(),
            &Settings {
                cypher_timeout_ms: 500,
                ..Settings::default()
            },
        )
        .unwrap();
        assert_eq!(
            c.query.normalised(),
            json!({"query": "RETURN 1", "columns": ["result"], "timeout_ms": 500})
        );
    }
}
