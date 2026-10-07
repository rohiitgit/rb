//! Contract M5 S14 (invariant 2, API ≡ CLI): for each fixture (`--no-scip`) and each endpoint, a
//! table of parameter sets is answered by `rb … --json` and by a running `rb serve`: the bodies
//! must be byte-identical, and each HTTP status must be the exit code's by D5's table (`200` for
//! exit 0). The sets cover the defaults, the maxima, `rta`, `pick`, an ambiguous and a missing
//! target, a value out of bounds, `snapshot=`, and caps lowered by the test-only override (given
//! to the CLI and to a second server alike). Needs `RB_TEST_DATABASE_URL`, like `e2e.rs`.

mod common;

use std::collections::BTreeMap;

use common::*;
use rb_api::{ErrorKind, Settings};
use serde_json::Value;

/// Percent-encodes a query-string value: every byte but the unreserved characters.
fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// A request: the endpoint's path segment and its parameters, as the API names them.
type Set = (&'static str, Vec<(&'static str, String)>);

/// The CLI arguments of a request: the command, its positional parameter, every other parameter
/// as its flag (booleans as bare flags, `columns` as `--cols`), `--repo`, and the JSON switch.
fn cli_args(segment: &str, repo: &str, params: &[(&'static str, String)]) -> Vec<String> {
    let (command, positional) = match segment {
        "status" => ("status", None),
        "snapshots" => ("snapshots", None),
        "find" => ("find", Some("q")),
        "services" => ("service", None),
        "service" => ("service", Some("name")),
        "callers" => ("callers", Some("target")),
        "impact" => ("impact", Some("target")),
        "table-deps" => ("table-deps", Some("table")),
        "path" => ("path", Some("endpoint")),
        "cypher" => ("cypher", Some("query")),
        "diff" => ("diff", None),
        other => panic!("no command for {other}"),
    };
    let mut args = vec![command.to_owned()];
    if let Some(p) = positional {
        let value = params.iter().find(|(k, _)| *k == p).expect("positional");
        args.push(value.1.clone());
    }
    // `rb diff [<from>] [<to>]`: both selectors are positional, `from` first.
    if segment == "diff" {
        for p in ["from", "to"] {
            if let Some((_, v)) = params.iter().find(|(k, _)| *k == p) {
                args.push(v.clone());
            }
        }
    }
    for (k, v) in params {
        match *k {
            k if Some(k) == positional => {}
            "from" | "to" if segment == "diff" => {}
            "include_references" | "full" | "diagnostics" => {
                assert_eq!(v, "true");
                args.push(format!("--{}", k.replace('_', "-")));
            }
            "columns" => args.extend(["--cols".to_owned(), v.clone()]),
            k => args.extend([format!("--{}", k.replace('_', "-")), v.clone()]),
        }
    }
    args.extend(["--repo".to_owned(), repo.to_owned()]);
    if segment == "path" {
        args.extend(["--format".to_owned(), "json".to_owned()]);
    } else {
        args.push("--json".to_owned());
    }
    args
}

/// The same request to the server: a `GET` with a query string, or Cypher's `POST` with its JSON
/// body (`snapshot` stays in the query string). `repo` is the path segment as sent (it may be
/// percent-encoded).
fn api_call(
    server: &Served,
    segment: &str,
    repo: &str,
    params: &[(&'static str, String)],
) -> (u16, String) {
    let path = format!("/v1/repos/{repo}/{segment}");
    if segment == "cypher" {
        let mut body = serde_json::Map::new();
        let mut query = Vec::new();
        for (k, v) in params {
            match *k {
                "query" => {
                    body.insert("query".into(), v.clone().into());
                }
                "columns" => {
                    body.insert("columns".into(), v.split(',').collect::<Vec<_>>().into());
                }
                "timeout_ms" => {
                    body.insert("timeout_ms".into(), v.parse::<u64>().unwrap().into());
                }
                k => query.push(format!("{k}={}", enc(v))),
            }
        }
        let path = if query.is_empty() {
            path
        } else {
            format!("{path}?{}", query.join("&"))
        };
        return server.call("POST", &path, Some(&Value::Object(body).to_string()));
    }
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", enc(v)))
        .collect();
    let path = if query.is_empty() {
        path
    } else {
        format!("{path}?{}", query.join("&"))
    };
    server.call("GET", &path, None)
}

/// Runs one set through both surfaces and checks the identity and the status; the parsed body.
fn same(url: &str, server: &Served, repo: &str, set: &Set, env: &[(&str, &str)]) -> Value {
    same_as(url, server, repo, repo, set, env)
}

/// [`same`], with the repository's path segment sent as `path_repo` (an encoded form of `repo`).
fn same_as(
    url: &str,
    server: &Served,
    repo: &str,
    path_repo: &str,
    (segment, params): &Set,
    env: &[(&str, &str)],
) -> Value {
    let args = cli_args(segment, repo, params);
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let (code, out, err) = rb_exit_env(url, &argv, env);
    assert_eq!(err, "", "rb {argv:?} wrote to stderr");
    let (status, body) = api_call(server, segment, path_repo, params);
    assert_eq!(
        body,
        out,
        "the API body differs from `rb {}`",
        args.join(" ")
    );
    let v: Value = serde_json::from_str(&body).unwrap();
    if code == 0 {
        assert_eq!(status, 200, "rb {argv:?}");
    } else {
        let kind = v["error"]["kind"].as_str().unwrap();
        let k = ErrorKind::ALL
            .into_iter()
            .find(|k| k.as_str() == kind)
            .unwrap();
        assert_eq!(
            (code, status),
            (i32::from(k.exit_code()), k.status()),
            "{kind}: rb {argv:?}"
        );
    }
    v
}

fn p(k: &'static str, v: &str) -> (&'static str, String) {
    (k, v.to_owned())
}

/// The first item key of `rb find <q> --kind <kind>`, when there is one.
fn first_key(url: &str, repo: &str, q: &str, kind: &str) -> Option<String> {
    let v: Value = serde_json::from_str(&rb(
        url,
        &[
            "find", q, "--kind", kind, "--limit", "1000", "--repo", repo, "--json",
        ],
    ))
    .unwrap();
    v["result"]["items"][0]["key"].as_str().map(str::to_owned)
}

/// The parameter table of one fixture, its targets discovered from its own answers: a symbol
/// with callers, a name two symbols share (ambiguous), an endpoint, a table.
fn table(url: &str, repo: &str, target: &str) -> (Vec<Set>, bool) {
    let snapshot = live_id(url, repo).to_string();
    let symbols: Value = serde_json::from_str(&rb(
        url,
        &[
            "find", "/", "--kind", "symbol", "--limit", "1000", "--repo", repo, "--json",
        ],
    ))
    .unwrap();
    let mut names: BTreeMap<String, usize> = BTreeMap::new();
    for s in symbols["result"]["items"].as_array().unwrap() {
        *names
            .entry(s["name"].as_str().unwrap().to_owned())
            .or_default() += 1;
    }
    let shared = names.iter().find(|(_, n)| **n >= 2).map(|(k, _)| k.clone());
    let endpoint = first_key(url, repo, "HTTP", "endpoint").expect("an endpoint");
    let table = first_key(url, repo, ".", "table").expect("a table");
    let mut sets: Vec<Set> = vec![
        ("status", vec![]),
        ("snapshots", vec![]),
        ("snapshots", vec![p("limit", "1")]),
        ("snapshots", vec![p("offset", "1")]),
        ("find", vec![p("q", "a")]),
        ("find", vec![p("q", "a"), p("limit", "1000")]),
        (
            "find",
            vec![
                p("q", "a"),
                p("kind", "symbol"),
                p("limit", "7"),
                p("offset", "3"),
            ],
        ),
        ("find", vec![p("q", "zz_nothing_matches_this")]),
        ("find", vec![p("q", "a"), p("snapshot", &snapshot)]),
        ("find", vec![p("q", "a"), p("limit", "1001")]),
        ("find", vec![p("q", "a"), p("kind", "thing")]),
        ("services", vec![]),
        ("services", vec![p("limit", "1"), p("offset", "1")]),
        ("service", vec![p("name", ".")]),
        ("service", vec![p("name", "no-such-service")]),
        ("callers", vec![p("target", target)]),
        ("callers", vec![p("target", target), p("depth", "25")]),
        (
            "callers",
            vec![
                p("target", target),
                p("precision", "rta"),
                p("include_references", "true"),
                p("min_certainty", "heuristic"),
            ],
        ),
        (
            "callers",
            vec![p("target", target), p("snapshot", &snapshot)],
        ),
        ("callers", vec![p("target", "no_such_symbol_anywhere")]),
        ("callers", vec![p("target", target), p("depth", "26")]),
        (
            "callers",
            vec![p("target", target), p("snapshot", "9000000000000000000")],
        ),
        ("impact", vec![p("target", target)]),
        ("impact", vec![p("target", target), p("precision", "rta")]),
        ("impact", vec![p("target", &table)]),
        ("table-deps", vec![p("table", &table)]),
        ("table-deps", vec![p("table", &table), p("hops", "8")]),
        (
            "table-deps",
            vec![p("table", &table), p("hops", "0"), p("precision", "rta")],
        ),
        ("table-deps", vec![p("table", "no_such_table")]),
        ("table-deps", vec![p("table", &table), p("hops", "9")]),
        ("path", vec![p("endpoint", &endpoint)]),
        (
            "path",
            vec![
                p("endpoint", &endpoint),
                p("depth", "25"),
                p("hops", "8"),
                p("full", "true"),
            ],
        ),
        (
            "path",
            vec![p("endpoint", &endpoint), p("precision", "rta")],
        ),
        ("path", vec![p("endpoint", "/")]),
        ("path", vec![p("endpoint", "/"), p("pick", "2")]),
        ("path", vec![p("endpoint", "no-such-route-anywhere")]),
        (
            "cypher",
            vec![p("query", "MATCH (n) RETURN count(n)"), p("columns", "n")],
        ),
        (
            "cypher",
            vec![
                p("query", "MATCH (n) RETURN count(n)"),
                p("columns", "n"),
                p("timeout_ms", "30000"),
            ],
        ),
        ("cypher", vec![p("query", "MATCH (n RETURN n")]),
        (
            "cypher",
            vec![p("query", "RETURN 1"), p("timeout_ms", "30001")],
        ),
        // Each repository was indexed twice: the default `from` is the live snapshot's parent.
        ("diff", vec![]),
        (
            "diff",
            vec![p("diagnostics", "true"), p("limit", "1"), p("offset", "1")],
        ),
        ("diff", vec![p("from", &snapshot), p("to", &snapshot)]),
        ("diff", vec![p("from", "zz")]),
        ("diff", vec![p("from", "9000000000000000000")]),
        ("diff", vec![p("limit", "1001")]),
    ];
    if let Some(name) = &shared {
        sets.push(("callers", vec![p("target", name)]));
        sets.push(("callers", vec![p("target", name), p("pick", "2")]));
        sets.push(("impact", vec![p("target", name)]));
        sets.push(("impact", vec![p("target", name), p("pick", "1")]));
    }
    (sets, shared.is_some())
}

#[test]
fn api_bodies_equal_cli_json() {
    let Some(url) = db_url() else { return };
    let pid = std::process::id();
    // (fixture, a symbol with callers, from the fixtures' `rb callers` goldens)
    let fixtures = [
        ("ts-express-prisma", "Cache.findUserByEmail"),
        ("java-spring-jpa", "DefaultAuthService.authenticate"),
        ("polyglot-monorepo", "MfaClient.sendCode"),
    ];
    let repos: Vec<String> = fixtures
        .iter()
        .map(|(f, _)| format!("t_api_{}_{pid}", f.replace('-', "_")))
        .collect();
    let _cleanup = Cleanup {
        url: &url,
        repos: repos.clone(),
    };
    let mut held = Vec::new();
    for ((fixture, _), repo) in fixtures.iter().zip(&repos) {
        let dir = fixture_repo(fixture);
        let path = dir.path().to_str().unwrap();
        rb(&url, &["index", path, "--repo", repo, "--no-scip"]);
        // A retired snapshot to page `snapshots` over.
        rb(
            &url,
            &["index", path, "--repo", repo, "--no-scip", "--force"],
        );
        held.push(dir);
    }
    let server = Served::start(&url, &[]);
    let mut statuses: BTreeMap<u16, usize> = BTreeMap::new();
    let mut ambiguous = 0;
    for ((fixture, target), repo) in fixtures.iter().zip(&repos) {
        let (sets, has_shared) = table(&url, repo, target);
        ambiguous += usize::from(has_shared);
        for set in &sets {
            let v = same(&url, &server, repo, set, &[]);
            let status = match v["error"]["kind"].as_str() {
                Some(kind) => ErrorKind::ALL
                    .into_iter()
                    .find(|k| k.as_str() == kind)
                    .unwrap()
                    .status(),
                None => 200,
            };
            *statuses.entry(status).or_default() += 1;
        }
        eprintln!("{fixture}: {} parameter sets identical", sets.len());
    }
    // A snapshot retention deleted (410), on a scratch repository.
    let gone_repo = format!("t_api_gone_{pid}");
    let _cleanup_gone = Cleanup {
        url: &url,
        repos: vec![gone_repo.clone()],
    };
    let small = repo_from(&[("src/a.ts", "export function a(): number { return 1; }\n")]);
    let small_path = small.path().to_str().unwrap();
    rb(
        &url,
        &["index", small_path, "--repo", &gone_repo, "--no-scip"],
    );
    let gone = live_id(&url, &gone_repo).to_string();
    rb(
        &url,
        &[
            "index",
            small_path,
            "--repo",
            &gone_repo,
            "--no-scip",
            "--force",
            "--keep",
            "0",
        ],
    );
    let v = same(
        &url,
        &server,
        &gone_repo,
        &("callers", vec![p("target", "a"), p("snapshot", &gone)]),
        &[],
    );
    assert_eq!(v["error"]["kind"], "snapshot_not_retained");
    *statuses.entry(410).or_default() += 1;

    // A `failed` snapshot is `not_found` (amendment 6 (c); evaluator pass 1, Z5): a publish whose
    // projection cannot be loaded (the CSVs are written where the database cannot see them)
    // records a `failed` row.
    let unseen = tempfile::tempdir().unwrap();
    let (code, _, err) = rb_exit(
        &url,
        &[
            "index",
            small_path,
            "--repo",
            &gone_repo,
            "--no-scip",
            "--force",
            "--age-load-dir",
            unseen.path().to_str().unwrap(),
        ],
    );
    assert_ne!(code, 0, "{err}");
    let listed: Value =
        serde_json::from_str(&rb(&url, &["snapshots", "--repo", &gone_repo, "--json"])).unwrap();
    let failed = listed["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["status"] == "failed")
        .unwrap_or_else(|| panic!("no failed row: {listed}"))["id"]
        .as_i64()
        .unwrap()
        .to_string();
    for set in [
        ("callers", vec![p("target", "a"), p("snapshot", &failed)]),
        ("find", vec![p("q", "a"), p("snapshot", &failed)]),
        (
            "cypher",
            vec![p("query", "RETURN 1"), p("snapshot", &failed)],
        ),
    ] {
        let v = same(&url, &server, &gone_repo, &set, &[]);
        assert_eq!(
            (v["error"]["kind"].as_str(), v["error"]["message"].as_str()),
            (
                Some("not_found"),
                Some(
                    format!("snapshot {failed} of {gone_repo:?} is failed and holds no graph")
                        .as_str()
                )
            ),
            "{set:?}"
        );
    }

    // Every repository's status, and a repository never indexed.
    let (status, body) = server.call("GET", "/v1/repos", None);
    assert_eq!((status, body), (200, rb(&url, &["status", "--json"])));
    let nope = format!("t_api_nope_{pid}");
    for set in [
        ("callers", vec![p("target", "x")]),
        ("status", vec![]),
        ("snapshots", vec![]),
    ] {
        let v = same(&url, &server, &nope, &set, &[]);
        assert_eq!(v["error"]["kind"], "not_found", "{set:?}");
    }

    // Percent-encoded repository segments name the same repository (evaluator pass 1, Z3): `_`
    // as `%5F` or `%5f`, an unreserved letter encoded, and a name that is not ASCII.
    let real = &repos[0];
    let encoded = [
        real.replace('_', "%5F"),
        real.replace('_', "%5f"),
        format!("%{:02X}{}", real.as_bytes()[0], &real[1..]),
    ];
    for path_repo in &encoded {
        for set in [
            ("status", vec![]),
            ("callers", vec![p("target", fixtures[0].1)]),
        ] {
            let v = same_as(&url, &server, real, path_repo, &set, &[]);
            assert!(v["error"].is_null(), "{path_repo} {set:?}: {v}");
        }
    }
    let unicode = format!("t_api_\u{e9}t\u{e9}_{pid}");
    for set in [("status", vec![]), ("callers", vec![p("target", "x")])] {
        let v = same_as(&url, &server, &unicode, &enc(&unicode), &set, &[]);
        assert_eq!(
            v["error"]["message"],
            format!("repository {unicode:?} is not indexed"),
            "{set:?}"
        );
    }
    // The rows every class of answer must have produced: answers, invalid parameters, not
    // found, ambiguous (at least two fixtures share a name), not retained, Cypher refusals.
    eprintln!("statuses: {statuses:?}");
    for status in [200, 400, 404, 409, 410, 422] {
        assert!(
            statuses.get(&status).copied().unwrap_or(0) > 0,
            "no {status}: {statuses:?}"
        );
    }
    assert!(ambiguous >= 2, "{ambiguous}");

    // Invariant 8: a second server answers with the same bytes.
    let again = Served::start(&url, &[]);
    for repo in &repos {
        for path in [
            format!("/v1/repos/{repo}/status"),
            format!("/v1/repos/{repo}/services"),
            format!("/v1/repos/{repo}/find?q=a&limit=1000"),
        ] {
            assert_eq!(
                server.call("GET", &path, None),
                again.call("GET", &path, None),
                "{path}"
            );
        }
    }

    // Truncating caps through the test-only override, given to the CLI and to a server.
    let caps = [(Settings::TEST_CAPS_VAR, "tree=1,nodes=1,work=3")];
    let capped = Served::start_env(&url, &[], &caps);
    for ((fixture, target), repo) in fixtures.iter().zip(&repos) {
        let endpoint = first_key(&url, repo, "HTTP", "endpoint").unwrap();
        let table = first_key(&url, repo, ".", "table").unwrap();
        let mut cut = Vec::new();
        for set in [
            ("callers", vec![p("target", target), p("depth", "25")]),
            ("impact", vec![p("target", target)]),
            ("path", vec![p("endpoint", &endpoint)]),
            ("table-deps", vec![p("table", &table)]),
            ("service", vec![p("name", ".")]),
        ] {
            let v = same(&url, &capped, repo, &set, &caps);
            assert_eq!(v["limits"]["work"], 3, "{set:?}");
            if v["truncated"].as_array().is_some_and(|t| !t.is_empty()) {
                cut.push(set.0);
            }
        }
        assert!(cut.len() >= 3, "{fixture}: only {cut:?} were cut");
    }
}
