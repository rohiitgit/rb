//! The structural classes (contract M8b, Identity beyond symbols): endpoints, their handlers,
//! table access and service-to-service flows, each read from its own dump rows and reduced to
//! `Loc`s that answer keys can name without rb's keys. An HTTP endpoint is its service, method and
//! normalised path; a table its name (the schema only when the source writes one); a service its
//! directory.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rb_model::diff::EdgeRow;
use rb_model::{EdgeKind, NodeKind};

use crate::dump::Dump;
use crate::identity::{Class, EdgeId, Found, Loc, Locator, Side, Site};

/// A path as every key compares it: a query string or fragment goes, `{x}`, `:x`, `*x` (a
/// catch-all) and `<x>` (and `<int:x>`) become `{}`, `//` collapses, a trailing `/` goes (the root stays `/`), and a leading
/// `/` is added.
pub fn normalize_path(path: &str) -> String {
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let segments: Vec<String> = path
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let param = (s.starts_with('{') && s.ends_with('}'))
                || s.starts_with(':')
                || s.starts_with('*')
                || (s.starts_with('<') && s.ends_with('>'));
            if param { "{}".to_owned() } else { s.to_owned() }
        })
        .collect();
    format!("/{}", segments.join("/"))
}

/// `HTTP <METHOD> <path>[ ?router=…] @<service>` as (method, normalised path, service); `None` for
/// any other endpoint (gRPC). A path whose router prefix rb could not find keeps what rb knows.
pub fn parse_http_endpoint(key: &str) -> Option<(&str, String, &str)> {
    let rest = key.strip_prefix("HTTP ")?;
    let (left, service) = rest.rsplit_once(" @")?;
    let (method, path) = left.split_once(' ')?;
    let path = path.split(" ?").next().unwrap_or(path);
    Some((method, normalize_path(path), service))
}

/// A table name as keys compare it: lower case, and without the schema rb assumed.
fn table_name(e: &EdgeRow) -> String {
    let assumed = e.attrs.get("schema_assumed").and_then(|v| v.as_bool()) == Some(true);
    let name = if assumed {
        e.dst_key
            .split_once('.')
            .map_or(e.dst_key.as_str(), |(_, t)| t)
    } else {
        e.dst_key.as_str()
    };
    name.to_lowercase()
}

fn found_of(e: &EdgeRow) -> Found {
    Found {
        certainty: e.certainty,
        provenances: BTreeSet::from([e.provenance]),
        sites: e
            .evidence
            .iter()
            .map(|ev| Site {
                path: ev.path.clone(),
                span: ev.span,
            })
            .collect(),
    }
}

/// Which service each file belongs to: its `IN_SERVICE` edge, else the service whose directory is
/// the longest prefix of its path (rb's own rule, for files rb did not index, such as config files
/// an answer key cites), else the root service when it is the only one: in a repository of several
/// services, a root-level file names no service.
pub struct Services {
    of_file: HashMap<String, String>,
    dirs: Vec<String>,
}

impl Services {
    pub fn new(dump: &Dump) -> Services {
        let of_file = dump
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::InService && e.src_kind == NodeKind::File)
            .map(|e| (e.src_key.clone(), e.dst_key.clone()))
            .collect();
        let dirs = dump
            .nodes
            .iter()
            .filter(|n| n.kind == NodeKind::Service)
            .map(|n| n.key.clone())
            .collect();
        Services { of_file, dirs }
    }

    pub fn of_file(&self, path: &str) -> Option<&str> {
        if let Some(s) = self.of_file.get(path) {
            return Some(s);
        }
        self.dirs
            .iter()
            .filter(|d| d.as_str() != "." && path.starts_with(&format!("{d}/")))
            .max_by_key(|d| d.len())
            .or_else(|| {
                (self.dirs.len() == 1).then(|| self.dirs.iter().find(|d| d.as_str() == "."))?
            })
            .map(String::as_str)
    }
}

/// Builds one side, counting rows whose endpoint has no location once per (source, target) key.
struct Builder {
    side: Side<Found>,
    unlocated: BTreeMap<(String, String), &'static str>,
}

impl Builder {
    fn new() -> Builder {
        Builder {
            side: Side::default(),
            unlocated: BTreeMap::new(),
        }
    }

    fn add(&mut self, e: &EdgeRow, id: Result<EdgeId, &'static str>) {
        match id {
            Ok(id) => self.side.insert(id, found_of(e)),
            Err(r) => {
                self.unlocated
                    .insert((e.src_key.clone(), e.dst_key.clone()), r);
            }
        }
    }

    fn finish(mut self) -> Side<Found> {
        for r in self.unlocated.into_values() {
            *self.side.unmatchable.entry(r).or_default() += 1;
        }
        self.side
    }
}

/// `EXPOSES` of HTTP endpoints.
pub fn endpoints(dump: &Dump) -> Side<Found> {
    let mut b = Builder::new();
    for e in dump.edges.iter().filter(|e| e.kind == EdgeKind::Exposes) {
        if let Some((method, path, service)) = parse_http_endpoint(&e.dst_key) {
            let id = EdgeId {
                src: Loc::service(service),
                class: Class::Endpoints,
                dst: Loc::endpoint(service, method, &path),
            };
            b.add(e, Ok(id));
        }
    }
    b.finish()
}

/// `HANDLED_BY` of HTTP endpoints: a named handler is its symbol's location, an anonymous one
/// (rb's handler is the file) is its file at the line the handler starts.
pub fn handler_bindings(dump: &Dump, locator: &Locator) -> Side<Found> {
    let mut b = Builder::new();
    for e in dump.edges.iter().filter(|e| e.kind == EdgeKind::HandledBy) {
        let Some((method, path, service)) = parse_http_endpoint(&e.src_key) else {
            continue;
        };
        let handler = match e.dst_kind {
            NodeKind::File => {
                let start = e
                    .attrs
                    .get("handler_spans")
                    .and_then(|v| v.get(0)?.get(0)?.as_u64())
                    .or_else(|| e.evidence.first().map(|ev| ev.span[0].max(0) as u64));
                Ok(Loc {
                    path: e.dst_key.clone(),
                    line: start.map_or(0, |l| l as u32 + 1),
                    name: "<anonymous>".to_owned(),
                })
            }
            kind => locator.loc(kind, &e.dst_key),
        };
        let id = handler.map(|dst| EdgeId {
            src: Loc::endpoint(service, method, &path),
            class: Class::HandlerBinding,
            dst,
        });
        b.add(e, id);
    }
    b.finish()
}

/// `QUERIES` per (source, table), and per (source, table, op) for the ops scored apart.
pub fn table_queries(dump: &Dump, locator: &Locator) -> (Side<Found>, Side<Found>) {
    let (mut queries, mut ops) = (Builder::new(), Builder::new());
    for e in dump.edges.iter().filter(|e| e.kind == EdgeKind::Queries) {
        let src = locator.loc(e.src_kind, &e.src_key);
        let table = table_name(e);
        queries.add(
            e,
            src.clone().map(|src| EdgeId {
                src,
                class: Class::TableQueries,
                dst: Loc::table(&table),
            }),
        );
        let row_ops: Vec<&str> = e
            .attrs
            .get("ops")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|o| o.as_str()).collect())
            .unwrap_or_default();
        for op in row_ops {
            ops.add(
                e,
                src.clone().map(|src| EdgeId {
                    src,
                    class: Class::TableOps,
                    dst: Loc::table(&format!("{table} {op}")),
                }),
            );
        }
    }
    (queries.finish(), ops.finish())
}

/// `MAPS_TO`: an entity class to its table.
pub fn table_maps(dump: &Dump, locator: &Locator) -> Side<Found> {
    let mut b = Builder::new();
    for e in dump.edges.iter().filter(|e| e.kind == EdgeKind::MapsTo) {
        let id = locator.loc(e.src_kind, &e.src_key).map(|src| EdgeId {
            src,
            class: Class::TableMaps,
            dst: Loc::table(&table_name(e)),
        });
        b.add(e, id);
    }
    b.finish()
}

/// Service to service over HTTP: a `CONSUMES` source's file gives the calling service, the
/// endpoint's `@service` the called one. A call within one service is kept; scopes decide.
pub fn service_flows(dump: &Dump, locator: &Locator, services: &Services) -> Side<Found> {
    let mut b = Builder::new();
    for e in dump.edges.iter().filter(|e| e.kind == EdgeKind::Consumes) {
        let Some((_, _, to)) = parse_http_endpoint(&e.dst_key) else {
            continue;
        };
        let file = match e.src_kind {
            NodeKind::File => Ok(e.src_key.clone()),
            kind => locator.loc(kind, &e.src_key).map(|l| l.path),
        };
        let id = file.and_then(|f| {
            let from = services.of_file(&f).ok_or("no_service")?;
            Ok(EdgeId {
                src: Loc::service(from),
                class: Class::ServiceFlows,
                dst: Loc::service(to),
            })
        });
        b.add(e, id);
    }
    b.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dump::parse;
    use rb_model::Certainty;

    fn polyglot() -> Dump {
        parse(include_str!(
            "../../rb-cli/tests/golden/polyglot-monorepo/dump.jsonl"
        ))
        .unwrap()
    }

    #[test]
    fn paths_normalise() {
        assert_eq!(normalize_path("/users/{id}/"), "/users/{}");
        assert_eq!(normalize_path("users//:id"), "/users/{}");
        assert_eq!(normalize_path("/a/<int:x>/b"), "/a/{}/b");
        assert_eq!(normalize_path("/"), "/");
        assert_eq!(normalize_path(""), "/");
        assert_eq!(
            normalize_path("/items/search/find?code={c}"),
            "/items/search/find"
        );
        assert_eq!(normalize_path("/swagger/*any"), "/swagger/{}");
    }

    #[test]
    fn endpoint_keys_parse() {
        assert_eq!(
            parse_http_endpoint("HTTP POST /billing/invoices/{}/pay @services/billing/cmd/billing"),
            Some((
                "POST",
                "/billing/invoices/{}/pay".to_owned(),
                "services/billing/cmd/billing"
            ))
        );
        assert_eq!(
            parse_http_endpoint("HTTP GET /x ?router=a.ts#r @svc"),
            Some(("GET", "/x".to_owned(), "svc"))
        );
        assert_eq!(parse_http_endpoint("GRPC a.B/C @svc"), None);
    }

    #[test]
    fn endpoints_and_handlers_of_a_real_dump() {
        let d = polyglot();
        let eps = endpoints(&d);
        let login = EdgeId {
            src: Loc::service("services/auth"),
            class: Class::Endpoints,
            dst: Loc::endpoint("services/auth", "GET", "/api/v1/login"),
        };
        assert_eq!(eps.located[&login].certainty, Certainty::Exact);
        let exposes = d
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Exposes && e.dst_key.starts_with("HTTP "))
            .count();
        assert_eq!(
            eps.len() as usize,
            exposes,
            "one identity per HTTP EXPOSES row"
        );

        let hb = handler_bindings(&d, &Locator::new(&d));
        let anonymous = hb
            .located
            .keys()
            .find(|k| {
                k.src == Loc::endpoint("services/notification", "POST", "/internal/mfa/email")
            })
            .unwrap();
        assert_eq!(
            anonymous.dst,
            Loc {
                path: "services/notification/src/server.ts".to_owned(),
                line: 7,
                name: "<anonymous>".to_owned()
            }
        );
        let named = hb
            .located
            .keys()
            .find(|k| k.src == Loc::endpoint("services/auth", "GET", "/api/v1/login"))
            .unwrap();
        assert_eq!(named.dst.name, "loginPage");
    }

    #[test]
    fn table_access_of_a_real_dump() {
        let d = polyglot();
        let (q, ops) = table_queries(&d, &Locator::new(&d));
        let names: BTreeSet<&str> = q.located.keys().map(|k| k.dst.name.as_str()).collect();
        assert!(names.contains("users"), "{names:?}");
        assert!(
            names.contains("audit.users"),
            "a written schema stays: {names:?}"
        );
        assert!(!names.iter().any(|n| n.starts_with("public.")), "{names:?}");
        let touch_ops: BTreeSet<&str> = ops
            .located
            .keys()
            .filter(|k| k.src.name == "touch")
            .map(|k| k.dst.name.as_str())
            .collect();
        assert_eq!(
            touch_ops,
            BTreeSet::from(["audit.users insert", "users update"])
        );
    }

    #[test]
    fn service_flows_of_a_real_dump() {
        let d = polyglot();
        let flows = service_flows(&d, &Locator::new(&d), &Services::new(&d));
        let web_to_auth = EdgeId {
            src: Loc::service("apps/web"),
            class: Class::ServiceFlows,
            dst: Loc::service("services/auth"),
        };
        assert!(
            flows.located.contains_key(&web_to_auth),
            "{:?}",
            flows.located.keys().collect::<Vec<_>>()
        );
        assert_eq!(flows.unmatchable.values().sum::<u64>(), 0);
    }

    #[test]
    fn files_without_in_service_take_the_longest_service_directory() {
        let d = polyglot();
        let s = Services::new(&d);
        assert_eq!(
            s.of_file("services/auth/src/main/resources/application.yml"),
            Some("services/auth")
        );
        assert_eq!(
            s.of_file("services/billing/cmd/billing/x.yml"),
            Some("services/billing/cmd/billing")
        );
        assert_eq!(
            s.of_file("docs/readme.md"),
            None,
            "several services: no root fallback"
        );
        let single =
            parse("[\"rb-dump\",2,\"x\"]\n[\"N\",\"service\",\".\",\"app\",{}]\n").unwrap();
        assert_eq!(Services::new(&single).of_file("config/app.yml"), Some("."));
    }
}
