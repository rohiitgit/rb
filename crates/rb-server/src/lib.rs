//! `rb serve`: the REST API (design §11.2; contract M5 S13, D1, D3, D7, D8).
//!
//! Every answer comes from the request layer (`rb-api`), the same code `rb … --json` runs, so the
//! two bodies cannot differ (invariant 2); this crate only adds HTTP and the server's guard rails:
//! - **local only (D7):** it binds loopback addresses only, answers only requests whose `Host` is
//!   its own loopback address and port (`421` otherwise: DNS rebinding), and sends no CORS header;
//!   `OPTIONS` and every method an endpoint does not take are `405`, and the one `POST` (Cypher)
//!   needs `Content-Type: application/json` (`415`), which a cross-origin page cannot send without
//!   a preflight that gets no approval;
//! - **read-only (invariant 7):** every pooled connection is a read-only session, and Cypher runs
//!   as `rb_reader` (D9, in `rb-store`);
//! - **bounded (D8, invariant 9):** `--workers` requests run at once, at most `--queue` wait, and
//!   one more gets `503 busy` at once; a request not answered within `--deadline-ms` gets
//!   `503 deadline_exceeded` and no result (the abandoned work keeps its slot until its work cap
//!   ends it); every question also runs under the work cap;
//! - **deterministic bodies (invariant 8):** timings travel only in `Server-Timing`.
//!
//! The postgres client is synchronous, so each request's work runs in `spawn_blocking`, holding
//! its slot and a pooled connection. One index per live snapshot is shared by the workers, plus
//! one other snapshot's (D3).

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::Query;
use axum::http::{HeaderValue, Method, Request, Response, header};
use rb_api::{ApiError, Endpoint, ErrorKind, Indexes, Params, Settings};
use rb_store::code::SnapshotIndex;
use rb_store::{SnapshotInfo, Store};
use tokio::sync::Semaphore;

/// The default listening address.
pub const DEFAULT_BIND: &str = "127.0.0.1:55480";

/// The largest `POST` body read (a Cypher query).
const BODY_LIMIT: usize = 1 << 20;

/// How the server is run.
#[derive(Debug, Clone)]
pub struct Config {
    pub bind: SocketAddr,
    pub database_url: String,
    /// Requests answered at once (and pooled connections).
    pub workers: usize,
    /// Requests waiting for a worker at most; one more is `busy`.
    pub queue: usize,
    /// Wall-clock deadline of a request, from its arrival.
    pub deadline: Duration,
    /// Caps and Cypher's default timeout.
    pub settings: Settings,
    /// Test hook (`RB_TEST_HOLD_MS`): each request's work waits this long before it starts.
    pub hold: Duration,
}

/// Refuses a listening address that is not loopback (D7: no remote binding in M5).
pub fn check_bind(addr: &SocketAddr) -> anyhow::Result<()> {
    anyhow::ensure!(
        addr.ip().is_loopback(),
        "refusing to bind {addr}: the server answers on a loopback address only \
         (127.0.0.0/8 or ::1), since it has no authentication"
    );
    Ok(())
}

/// When the server stops: on SIGINT or SIGTERM, or when the channel fires (tests).
pub enum Shutdown {
    Signal,
    Channel(tokio::sync::oneshot::Receiver<()>),
}

/// Binds `config.bind`, calls `ready` with the bound address, and serves until `shutdown`;
/// requests in flight finish first.
pub fn serve(
    config: Config,
    ready: impl FnOnce(SocketAddr),
    shutdown: Shutdown,
) -> anyhow::Result<()> {
    check_bind(&config.bind)?;
    anyhow::ensure!(config.workers >= 1, "--workers must be at least 1");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let server = runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(config.bind).await?;
        let addr = listener.local_addr()?;
        let server = Arc::new(Server::new(config, addr));
        ready(addr);
        let app_server = server.clone();
        let app = axum::Router::new().fallback(move |req: Request<Body>| {
            let server = app_server.clone();
            async move { server.handle(req).await }
        });
        axum::serve(listener, app)
            .with_graceful_shutdown(stopped(shutdown))
            .await?;
        anyhow::Ok(server)
    })?;
    // Dropping the runtime waits for work a deadline abandoned (bounded by the work cap); the
    // pooled connections are dropped after it, outside any runtime, since the synchronous
    // postgres client runs one of its own.
    drop(runtime);
    drop(server);
    Ok(())
}

async fn stopped(shutdown: Shutdown) {
    match shutdown {
        Shutdown::Channel(rx) => {
            let _ = rx.await;
        }
        Shutdown::Signal => {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("a SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        }
    }
}

struct Server {
    config: Config,
    /// `Host` values answered: the bound address and `localhost` with its port (D7).
    hosts: Vec<String>,
    slots: Arc<Semaphore>,
    waiting: AtomicUsize,
    pool: Mutex<Vec<Store>>,
    indexes: Mutex<IndexCache>,
}

/// A refused request: its error and, for `405`, the method the endpoint takes (`Allow`).
struct Refusal {
    error: ApiError,
    allow: Option<Method>,
}

impl From<ApiError> for Refusal {
    fn from(error: ApiError) -> Self {
        Refusal { error, allow: None }
    }
}

/// What a path names.
enum Route {
    Health,
    Api {
        endpoint: Endpoint,
        repo: Option<String>,
    },
}

/// Where a request's time went, for `Server-Timing` only.
struct Timing {
    started: Instant,
    queued: Option<Duration>,
    ran: Option<Duration>,
}

impl Timing {
    fn header(&self) -> String {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let mut parts = Vec::new();
        if let Some(q) = self.queued {
            parts.push(format!("queue;dur={:.1}", ms(q)));
        }
        if let Some(r) = self.ran {
            parts.push(format!("run;dur={:.1}", ms(r)));
        }
        parts.push(format!("total;dur={:.1}", ms(self.started.elapsed())));
        parts.join(", ")
    }
}

/// Decrements the queue count when a waiting request leaves it (served or abandoned).
struct Waiting<'a>(&'a AtomicUsize);

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl Server {
    fn new(config: Config, addr: SocketAddr) -> Server {
        let port = addr.port();
        let own = match addr.ip() {
            IpAddr::V4(ip) => format!("{ip}:{port}"),
            IpAddr::V6(ip) => format!("[{ip}]:{port}"),
        };
        let mut hosts = vec![
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
        ];
        if !hosts.contains(&own) {
            hosts.push(own);
        }
        Server {
            slots: Arc::new(Semaphore::new(config.workers)),
            waiting: AtomicUsize::new(0),
            pool: Mutex::new(Vec::new()),
            indexes: Mutex::new(IndexCache::default()),
            hosts,
            config,
        }
    }

    async fn handle(self: Arc<Self>, req: Request<Body>) -> Response<Body> {
        let mut timing = Timing {
            started: Instant::now(),
            queued: None,
            ran: None,
        };
        match self.clone().answer(req, &mut timing).await {
            Ok(body) => respond(200, body, None, &timing),
            Err(Refusal { error, allow }) => {
                respond(error.kind.status(), error.body(), allow, &timing)
            }
        }
    }

    /// The body of a successful answer, or the refusal. The checks that need no database come
    /// first, in order: `Host`, path, method, media type, parameters.
    async fn answer(
        self: Arc<Self>,
        req: Request<Body>,
        timing: &mut Timing,
    ) -> Result<String, Refusal> {
        let (parts, body) = req.into_parts();
        let host = parts
            .headers
            .get(header::HOST)
            .and_then(|h| h.to_str().ok())
            .map(str::to_ascii_lowercase);
        if !host.as_ref().is_some_and(|h| self.hosts.contains(h)) {
            return Err(ApiError::new(
                ErrorKind::MisdirectedRequest,
                format!(
                    "this server answers only Host {} (got {})",
                    self.hosts.join(", "),
                    host.as_deref()
                        .map_or("none".to_owned(), |h| format!("{h:?}"))
                ),
            )
            .into());
        }
        let route = route(parts.uri.path())?;
        let wanted = match &route {
            Route::Api {
                endpoint: Endpoint::Cypher,
                ..
            } => Method::POST,
            _ => Method::GET,
        };
        if parts.method != wanted {
            return Err(Refusal {
                error: ApiError::new(
                    ErrorKind::MethodNotAllowed,
                    format!("{} {} takes {wanted} only", parts.method, parts.uri.path()),
                ),
                allow: Some(wanted),
            });
        }
        let (endpoint, repo) = match route {
            Route::Health => return Ok("{\n  \"api\": 1,\n  \"status\": \"ok\"\n}\n".to_owned()),
            Route::Api { endpoint, repo } => (endpoint, repo),
        };
        let mut pairs = Query::<Vec<(String, String)>>::try_from_uri(&parts.uri)
            .map_err(|e| ApiError::invalid("query", format_args!("string is malformed: {e}")))?
            .0;
        if endpoint == Endpoint::Cypher {
            let json = parts
                .headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.split(';').next())
                .is_some_and(|m| m.trim().eq_ignore_ascii_case("application/json"));
            if !json {
                return Err(ApiError::new(
                    ErrorKind::UnsupportedMediaType,
                    "POST …/cypher takes Content-Type: application/json",
                )
                .into());
            }
            let bytes = axum::body::to_bytes(body, BODY_LIMIT)
                .await
                .map_err(|e| ApiError::invalid("body", format_args!("could not be read: {e}")))?;
            pairs.extend(cypher_body(&bytes)?);
        }
        let params = Params::from_pairs(pairs)?;
        let request = rb_api::parse(endpoint, repo.as_deref(), &params, &self.config.settings)?;
        Ok(self.run(request, timing).await?)
    }

    /// Waits for a slot (or refuses at once when the queue is full), then answers in a blocking
    /// task that holds the slot and a pooled connection, within the request's deadline.
    async fn run(
        self: Arc<Self>,
        request: rb_api::Request,
        timing: &mut Timing,
    ) -> Result<String, ApiError> {
        let deadline = self.config.deadline;
        let left = deadline.saturating_sub(timing.started.elapsed());
        let server = self.clone();
        let queued_at = Instant::now();
        let ran_at: Arc<Mutex<Option<Instant>>> = Arc::default();
        let ran = ran_at.clone();
        let work = async move {
            let permit = match server.slots.clone().try_acquire_owned() {
                Ok(p) => p,
                Err(_) => {
                    if server.waiting.fetch_add(1, Ordering::SeqCst) >= server.config.queue {
                        server.waiting.fetch_sub(1, Ordering::SeqCst);
                        return Err(ApiError::new(
                            ErrorKind::Busy,
                            format!(
                                "every worker is busy and the queue is full \
                                 (--workers {}, --queue {})",
                                server.config.workers, server.config.queue
                            ),
                        ));
                    }
                    let _waiting = Waiting(&server.waiting);
                    server
                        .slots
                        .clone()
                        .acquire_owned()
                        .await
                        .expect("the semaphore is never closed")
                }
            };
            *ran.lock().expect("not poisoned") = Some(Instant::now());
            let worker = server.clone();
            tokio::task::spawn_blocking(move || {
                // The slot is held until the work ends, even past the deadline (D8).
                let _permit = permit;
                if !worker.config.hold.is_zero() {
                    std::thread::sleep(worker.config.hold);
                }
                worker.work(&request)
            })
            .await
            .map_err(|_| ApiError::new(ErrorKind::Internal, "the request's worker panicked"))?
        };
        let outcome = tokio::time::timeout(left, work).await;
        let started_run = *ran_at.lock().expect("not poisoned");
        timing.queued = Some(started_run.unwrap_or_else(Instant::now) - queued_at);
        timing.ran = started_run.map(|t| t.elapsed());
        match outcome {
            Ok(answer) => answer,
            Err(_) => Err(ApiError::new(
                ErrorKind::DeadlineExceeded,
                format!(
                    "the request did not finish within its deadline of {} ms (--deadline-ms)",
                    deadline.as_millis()
                ),
            )),
        }
    }

    /// Answers on a pooled read-only connection (a fresh one when none is free or the last one
    /// failed).
    fn work(&self, request: &rb_api::Request) -> Result<String, ApiError> {
        let pooled = self.pool.lock().expect("not poisoned").pop();
        let mut store = match pooled {
            Some(s) => s,
            None => Store::connect_read_only(&self.config.database_url)?,
        };
        let mut indexes = SharedIndexes(&self.indexes);
        let out = rb_api::run(&mut store, &mut indexes, request, &self.config.settings);
        // A connection that failed is not reused: the next request connects afresh.
        if !matches!(&out, Err(e) if e.kind == ErrorKind::Internal) {
            self.pool.lock().expect("not poisoned").push(store);
        }
        out.map(|o| o.body())
    }
}

/// The endpoint a path names (`/v1/…`).
fn route(path: &str) -> Result<Route, ApiError> {
    let parts: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    let api = |endpoint, repo: Option<&str>| {
        Ok(Route::Api {
            endpoint,
            repo: repo.map(str::to_owned),
        })
    };
    match parts.as_slice() {
        ["", "v1", "health"] => Ok(Route::Health),
        ["", "v1", "repos"] => api(Endpoint::Status, None),
        ["", "v1", "repos", repo, segment] if !repo.is_empty() => {
            match Endpoint::from_segment(segment) {
                Some(e) => api(e, Some(repo)),
                None => Err(no_such(path)),
            }
        }
        _ => Err(no_such(path)),
    }
}

fn no_such(path: &str) -> ApiError {
    ApiError::not_found(format!(
        "no endpoint {path:?} (see design §11.2: /v1/repos/{{repo}}/…)"
    ))
}

/// A Cypher body `{"query", "columns"?, "timeout_ms"?}` as parameters; any other key, or a value
/// of another type, is `invalid_param`.
fn cypher_body(bytes: &[u8]) -> Result<Vec<(String, String)>, ApiError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|e| ApiError::invalid("body", format_args!("is not JSON: {e}")))?;
    let Some(object) = value.as_object() else {
        return Err(ApiError::invalid("body", "must be a JSON object"));
    };
    let mut pairs = Vec::new();
    for (key, v) in object {
        let text = match (key.as_str(), v) {
            ("query", serde_json::Value::String(q)) => q.clone(),
            ("columns", serde_json::Value::Array(cols)) => cols
                .iter()
                .map(|c| c.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| ApiError::invalid("columns", "must be an array of strings"))?
                .join(","),
            ("timeout_ms", serde_json::Value::Number(n)) => n.to_string(),
            ("query" | "columns" | "timeout_ms", _) => {
                return Err(ApiError::invalid(key, "has the wrong JSON type"));
            }
            (other, _) => {
                return Err(ApiError::invalid(
                    other,
                    "is not a field of a Cypher request (query, columns, timeout_ms)",
                ));
            }
        };
        pairs.push((key.clone(), text));
    }
    Ok(pairs)
}

fn respond(status: u16, body: String, allow: Option<Method>, timing: &Timing) -> Response<Body> {
    let mut response = Response::new(Body::from(body));
    *response.status_mut() =
        axum::http::StatusCode::from_u16(status).expect("a status from D5's table");
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    if let Ok(v) = HeaderValue::from_str(&timing.header()) {
        headers.insert("server-timing", v);
    }
    if let Some(v) = allow.and_then(|m| HeaderValue::from_str(m.as_str()).ok()) {
        headers.insert(header::ALLOW, v);
    }
    response
}

/// The indexes the workers share (D3): each repository's live snapshot's, and one other.
#[derive(Default)]
struct IndexCache {
    live: HashMap<String, (i64, Arc<SnapshotIndex>)>,
    other: Option<(i64, Arc<SnapshotIndex>)>,
}

struct SharedIndexes<'a>(&'a Mutex<IndexCache>);

impl Indexes for SharedIndexes<'_> {
    /// A cached index of `snapshot`, or one built from its rows in the request's transaction;
    /// when the live pointer has moved, the new live snapshot's replaces the old one (requests
    /// still running keep their `Arc` of it).
    fn index(
        &mut self,
        store: &mut Store,
        snapshot: &SnapshotInfo,
        live: bool,
    ) -> anyhow::Result<Arc<SnapshotIndex>> {
        {
            let cache = self.0.lock().expect("not poisoned");
            let hit = cache
                .live
                .get(&snapshot.repo)
                .into_iter()
                .chain(cache.other.as_ref())
                .find(|(id, _)| *id == snapshot.id);
            if let Some((_, ix)) = hit {
                return Ok(ix.clone());
            }
        }
        let ix = Arc::new(SnapshotIndex::new(store.load_graph(snapshot.id)?));
        let mut cache = self.0.lock().expect("not poisoned");
        if live {
            cache
                .live
                .insert(snapshot.repo.clone(), (snapshot.id, ix.clone()));
        } else {
            cache.other = Some((snapshot.id, ix.clone()));
        }
        Ok(ix)
    }
}

#[cfg(test)]
mod tests;
