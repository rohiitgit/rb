//! Contract M5 S13: each server rule with its test. The database URL refuses connections (port
//! 1), so these exercise everything a request meets before the database: the bind, `Host`,
//! methods, media type, parameters, the slots, the queue and the deadline (the test hook holds the
//! work first). The read-only sessions and a moved live pointer are tested against the database
//! (`rb-store`'s `read_only_sessions_refuse_writes`, the e2e suite).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::*;

const NO_DATABASE: &str = "postgresql://rb:rb@127.0.0.1:1/rb";

struct Running {
    addr: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<JoinHandle<anyhow::Result<()>>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(t) = self.thread.take() {
            t.join().unwrap().unwrap();
        }
    }
}

fn start(adjust: impl FnOnce(&mut Config)) -> Running {
    let mut config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url: NO_DATABASE.to_owned(),
        workers: 4,
        queue: 16,
        deadline: Duration::from_secs(10),
        settings: Settings::default(),
        hold: Duration::ZERO,
    };
    adjust(&mut config);
    let (tx, rx) = tokio::sync::oneshot::channel();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        serve(
            config,
            move |addr| ready_tx.send(addr).unwrap(),
            Shutdown::Channel(rx),
        )
    });
    let addr = ready_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    Running {
        addr,
        stop: Some(tx),
        thread: Some(thread),
    }
}

#[derive(Debug)]
struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    fn kind(&self) -> String {
        let v: serde_json::Value = serde_json::from_str(&self.body).unwrap();
        v["error"]["kind"].as_str().unwrap_or_default().to_owned()
    }
}

/// Sends one raw HTTP/1.1 request and reads the reply to the end. Every reply is checked for the
/// absence of CORS headers (D7) and for a `Server-Timing` header with no timing in the body (D4).
fn raw(addr: SocketAddr, request: &str) -> Reply {
    let mut s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    s.write_all(request.as_bytes()).unwrap();
    let mut bytes = Vec::new();
    s.read_to_end(&mut bytes).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let mut lines = head.lines();
    let status = lines
        .next()
        .unwrap()
        .split(' ')
        .nth(1)
        .unwrap()
        .parse()
        .unwrap();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let reply = Reply {
        status,
        headers,
        body: body.to_owned(),
    };
    assert!(
        reply
            .headers
            .iter()
            .all(|(n, _)| !n.starts_with("access-control-")),
        "{reply:?}"
    );
    assert!(reply.header("server-timing").is_some(), "{reply:?}");
    assert!(!reply.body.contains("dur="), "{reply:?}");
    reply
}

fn request(r: &Running, method: &str, path: &str, extra: &str, body: &str) -> Reply {
    raw(
        r.addr,
        &format!(
            "{method} {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n{extra}Content-Length: {}\r\n\r\n{body}",
            r.addr,
            body.len()
        ),
    )
}

fn get(r: &Running, path: &str) -> Reply {
    request(r, "GET", path, "", "")
}

#[test]
fn a_non_loopback_bind_is_refused() {
    for bad in ["0.0.0.0:0", "192.168.1.10:55480", "[::]:0", "10.0.0.1:1"] {
        let addr: SocketAddr = bad.parse().unwrap();
        let err = check_bind(&addr).unwrap_err().to_string();
        assert!(err.contains("loopback"), "{bad}: {err}");
    }
    for ok in ["127.0.0.1:0", "127.0.0.2:55480", "[::1]:0"] {
        check_bind(&ok.parse().unwrap()).unwrap();
    }
    // `serve` refuses before binding anything.
    let config = Config {
        bind: "0.0.0.0:0".parse().unwrap(),
        database_url: NO_DATABASE.to_owned(),
        workers: 1,
        queue: 0,
        deadline: Duration::from_secs(1),
        settings: Settings::default(),
        hold: Duration::ZERO,
    };
    let mut bound = false;
    let (_tx, rx) = tokio::sync::oneshot::channel();
    assert!(serve(config, |_| bound = true, Shutdown::Channel(rx)).is_err());
    assert!(!bound);
}

/// D7, DNS rebinding: only the server's own loopback address and port, or `localhost` with its
/// port, are answered; a foreign name, another port or no `Host` at all is `421`.
#[test]
fn a_foreign_host_is_misdirected() {
    let r = start(|_| {});
    let port = r.addr.port();
    let with_host = |host: Option<&str>| {
        let host = host.map_or(String::new(), |h| format!("Host: {h}\r\n"));
        raw(
            r.addr,
            &format!("GET /v1/health HTTP/1.1\r\n{host}Connection: close\r\n\r\n"),
        )
    };
    for ok in [
        format!("127.0.0.1:{port}"),
        format!("localhost:{port}"),
        format!("LOCALHOST:{port}"),
    ] {
        let reply = with_host(Some(&ok));
        assert_eq!(reply.status, 200, "{ok}");
        assert_eq!(reply.body, "{\n  \"api\": 1,\n  \"status\": \"ok\"\n}\n");
    }
    for bad in [
        Some("evil.example".to_owned()),
        Some(format!("evil.example:{port}")),
        Some(format!("127.0.0.1:{}", port.wrapping_add(1))),
        Some("127.0.0.1".to_owned()),
        Some(format!("localhost.:{port}")),
        None,
    ] {
        let reply = with_host(bad.as_deref());
        assert_eq!(
            (reply.status, reply.kind().as_str()),
            (421, "misdirected_request"),
            "{bad:?}"
        );
    }
}

/// D7: no CORS approval: `OPTIONS` (a preflight) is `405` with no `Access-Control-*` header;
/// every method an endpoint does not take is `405` with `Allow`; Cypher's `POST` needs
/// `application/json` (`415`).
#[test]
fn methods_and_media_types_are_allow_listed() {
    let r = start(|_| {});
    let preflight = request(
        &r,
        "OPTIONS",
        "/v1/repos/x/callers?target=a",
        "Origin: https://evil.example\r\nAccess-Control-Request-Method: GET\r\n",
        "",
    );
    assert_eq!(
        (preflight.status, preflight.header("allow")),
        (405, Some("GET"))
    );
    for (method, path, allow) in [
        ("POST", "/v1/repos/x/callers?target=a", "GET"),
        ("PUT", "/v1/health", "GET"),
        ("DELETE", "/v1/repos/x/find?q=a", "GET"),
        ("HEAD", "/v1/repos", "GET"),
        ("GET", "/v1/repos/x/cypher", "POST"),
    ] {
        let reply = request(&r, method, path, "", "");
        assert_eq!(
            (reply.status, reply.header("allow")),
            (405, Some(allow)),
            "{method} {path}"
        );
        if method != "HEAD" {
            assert_eq!(reply.kind(), "method_not_allowed");
        }
    }
    let body = r#"{"query": "RETURN 1"}"#;
    for ct in [
        "",
        "Content-Type: text/plain\r\n",
        "Content-Type: application/x-www-form-urlencoded\r\n",
    ] {
        let reply = request(&r, "POST", "/v1/repos/x/cypher", ct, body);
        assert_eq!(
            (reply.status, reply.kind().as_str()),
            (415, "unsupported_media_type"),
            "{ct:?}"
        );
    }
    // The media type is checked, then the body: an unknown field is `invalid_param`, before
    // any database work.
    let reply = request(
        &r,
        "POST",
        "/v1/repos/x/cypher",
        "Content-Type: application/json; charset=utf-8\r\n",
        r#"{"query": "RETURN 1", "role": "rb"}"#,
    );
    assert_eq!(
        (reply.status, reply.kind().as_str()),
        (400, "invalid_param")
    );
}

/// S13: an unknown or repeated parameter is `invalid_param` (no silent ignore); an unknown path
/// is `not_found`; both before any database work.
#[test]
fn parameters_and_paths_are_checked_first() {
    let r = start(|_| {});
    for path in [
        "/v1/repos/x/callers?target=a&dpeth=2",
        "/v1/repos/x/callers?target=a&target=b",
        "/v1/repos/x/callers?target=a&depth=26",
        "/v1/repos/x/service?name=a&limit=2",
    ] {
        let reply = get(&r, path);
        assert_eq!(
            (reply.status, reply.kind().as_str()),
            (400, "invalid_param"),
            "{path}"
        );
    }
    for path in ["/v1/repos/x/nope", "/v1/repos//callers", "/v2/health", "/"] {
        let reply = get(&r, path);
        assert_eq!(
            (reply.status, reply.kind().as_str()),
            (404, "not_found"),
            "{path}"
        );
    }
}

/// D8: with every worker busy and the queue full, one more request is `503 busy` at once; a
/// request that fits the queue waits for its slot.
#[test]
fn a_full_queue_is_busy_at_once() {
    let r = start(|c| {
        c.workers = 1;
        c.queue = 1;
        c.hold = Duration::from_millis(1_500);
    });
    let addr = r.addr;
    let send = move || {
        raw(
            addr,
            &format!(
                "GET /v1/repos/x/callers?target=a HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
            ),
        )
    };
    let running = std::thread::spawn(send);
    std::thread::sleep(Duration::from_millis(300));
    let queued = std::thread::spawn(send);
    std::thread::sleep(Duration::from_millis(300));
    let started = Instant::now();
    let busy = send();
    assert!(
        started.elapsed() < Duration::from_millis(1_000),
        "{:?}",
        started.elapsed()
    );
    assert_eq!((busy.status, busy.kind().as_str()), (503, "busy"));
    // The first two were answered, after the hold, by the (absent) database.
    for t in [running, queued] {
        let reply = t.join().unwrap();
        assert_eq!(
            (reply.status, reply.kind().as_str()),
            (500, "internal"),
            "{reply:?}"
        );
    }
}

/// D6, D8: a request not answered within the deadline is `503 deadline_exceeded`, without any
/// result, as soon as the deadline passes (the test hook holds its work for 3 s).
#[test]
fn a_deadline_is_an_error_never_a_partial_body() {
    let r = start(|c| {
        c.deadline = Duration::from_millis(300);
        c.hold = Duration::from_secs(3);
    });
    let started = Instant::now();
    let reply = get(&r, "/v1/repos/x/callers?target=a");
    assert!(
        started.elapsed() < Duration::from_millis(2_500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(
        (reply.status, reply.kind().as_str()),
        (503, "deadline_exceeded")
    );
    assert!(!reply.body.contains("\"result\""), "{}", reply.body);
    let timing = reply.header("server-timing").unwrap();
    assert!(timing.contains("total;dur="), "{timing}");
}

#[test]
fn cypher_bodies_become_parameters() {
    assert_eq!(
        cypher_body(br#"{"query": "RETURN 1", "columns": ["a", "b"], "timeout_ms": 500}"#).unwrap(),
        [
            ("columns".to_owned(), "a,b".to_owned()),
            ("query".to_owned(), "RETURN 1".to_owned()),
            ("timeout_ms".to_owned(), "500".to_owned())
        ]
    );
    for bad in [
        &br#"[]"#[..],
        br#"{"query": 1}"#,
        br#"{"query": "x", "columns": [1]}"#,
        br#"{"query": "x", "sql": "DROP"}"#,
        b"not json",
    ] {
        assert_eq!(
            cypher_body(bad).unwrap_err().kind,
            ErrorKind::InvalidParam,
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

/// Invariant 7: every connection the server pools is a read-only session (the database refuses a
/// write whatever statement issues it). Needs `RB_TEST_DATABASE_URL`.
#[test]
fn pooled_connections_are_read_only_sessions() {
    let Ok(url) = std::env::var("RB_TEST_DATABASE_URL") else {
        eprintln!("skipping: set RB_TEST_DATABASE_URL to run database tests");
        return;
    };
    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        database_url: url,
        workers: 1,
        queue: 0,
        deadline: Duration::from_secs(10),
        settings: Settings::default(),
        hold: Duration::ZERO,
    };
    let server = Server::new(config, "127.0.0.1:1".parse().unwrap());
    let request =
        rb_api::parse(Endpoint::Status, None, &Params::new(), &Settings::default()).unwrap();
    server.work(&request).unwrap();
    let mut pooled = server
        .pool
        .lock()
        .unwrap()
        .pop()
        .expect("a pooled connection");
    assert!(pooled.session_read_only().unwrap());
    assert!(pooled.ensure_repo("t_server_never", None).is_err());
}
