//! microSecEnD (contract M8b, T2): a hand-made dataflow diagram of a Java microservice
//! application, with every item traced to file:line, against rb's `--no-scip` dump of the
//! application at its pin (`bench/pin_microsecend.py`).
//!
//! A model service is mapped to an rb service through its evidence file (the service rb assigns
//! that file to), never by name. The dataset labels each service `internal` or `infrastructural`
//! (gateways, registries, config servers, monitors, brokers); only flows between two internal
//! services, and the endpoints of internal services, are scored. Databases, external entities,
//! infrastructural services and message flows are counted `out_of_scope`.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

use crate::dump::Dump;
use crate::identity::{Class, EdgeId, Found, Loc, Locator, Side};
use crate::score::{Cell, Scope, score};
use crate::structural::{Services, endpoints, normalize_path, service_flows};

pub mod reason {
    /// A flow touching an external entity (a user, a third-party service).
    pub const EXTERNAL: &str = "external";
    /// A flow touching a database.
    pub const DATABASE: &str = "database";
    /// A flow touching an infrastructural service (gateway, registry, config, monitoring, broker).
    pub const NOT_MODELLED: &str = "not_modelled";
    /// A flow over a message broker (Kafka, RabbitMQ): rb models no messaging flow.
    pub const MESSAGING: &str = "messaging";
    /// rb found it between services the model does not have as internal ones.
    pub const NOT_IN_MODEL: &str = "not_in_model";
    /// Two ends in one rb service: no service flow.
    pub const SAME_SERVICE: &str = "same_service";
    /// A model service whose evidence names no file that maps to an rb service.
    pub const NO_EVIDENCE_FILE: &str = "no_evidence_file";
    /// rb's endpoint of an internal service the model lists no endpoints for: a missing list
    /// says nothing about the service's endpoints.
    pub const NO_ENDPOINT_LIST: &str = "no_endpoint_list";
}

pub const CLASSES: [Class; 2] = [Class::ServiceFlows, Class::Endpoints];

#[derive(Deserialize)]
struct Model {
    services: Vec<Node>,
    #[serde(default)]
    external_entities: Vec<Node>,
    information_flows: Vec<Flow>,
}

#[derive(Deserialize)]
struct Node {
    name: String,
    #[serde(default)]
    stereotypes: Vec<String>,
    #[serde(default)]
    tagged_values: BTreeMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct Flow {
    sender: String,
    receiver: String,
    #[serde(default)]
    stereotypes: Vec<String>,
}

#[derive(Deserialize)]
struct Trace {
    nodes: BTreeMap<String, serde_json::Value>,
}

/// Removes commas before `}` or `]` outside strings: two of the dataset's files have them.
/// Returns the text and whether anything was removed.
pub fn repair(text: &str) -> (String, bool) {
    let chars: Vec<char> = text.chars().collect();
    let (mut out, mut in_string, mut escaped, mut removed) = (String::new(), false, false, false);
    for (i, &c) in chars.iter().enumerate() {
        if in_string {
            in_string = !(c == '"' && !escaped);
            escaped = c == '\\' && !escaped;
        } else if c == '"' {
            in_string = true;
        } else if c == ',' {
            let next = chars[i + 1..].iter().find(|c| !c.is_whitespace());
            if matches!(next, Some('}' | ']')) {
                removed = true;
                continue;
            }
        }
        out.push(c);
    }
    (out, removed)
}

/// The repository path an evidence URL names, without the doubled `<ref>/` prefix the dataset
/// sometimes writes.
fn evidence_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let (_, after) = rest.split_once("/blob/")?;
    let (git_ref, path) = after.split_once('/')?;
    let path = path.split('#').next().unwrap_or(path);
    let path = path.strip_prefix(&format!("{git_ref}/")).unwrap_or(path);
    Some(path.to_owned())
}

/// Every evidence URL of a traceability item, its own first, then its sub-items'.
fn urls(item: &serde_json::Value) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(f) = item.get("file").and_then(|f| f.as_str()) {
        out.push(f.to_owned());
    }
    if let Some(subs) = item.get("sub_items").and_then(|s| s.as_object()) {
        for sub in subs.values() {
            out.extend(urls(sub));
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Internal,
    Infrastructural,
    Database,
    External,
}

/// The endpoints both sides state, in the key's terms: (rb service directory, normalised path).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Review {
    /// The model's endpoints, after overrides.
    pub gold: BTreeSet<(String, String)>,
    /// rb's endpoints of services the model lists endpoints for.
    pub found: BTreeSet<(String, String)>,
}

/// The statement an adjudicator judges for one endpoint; the same text whichever side states it.
pub fn endpoint_statement((svc, path): &(String, String)) -> String {
    format!("ENDPOINT ANY {path} @{svc}")
}

/// Applies an app's override file (`bench/keys/microsecend/<target>.md`): append-only
/// `- ADD `ENDPOINT ANY <path> @<service>`` and `- REMOVE `…`` entries from a blind adjudication
/// of the model's endpoints against the source. A REMOVE of an endpoint the key lacks is an error.
pub fn apply_overrides(gold: &mut BTreeSet<(String, String)>, text: &str) -> Result<(), String> {
    for (n, l) in text.lines().enumerate() {
        let Some(rest) = l.strip_prefix("- ") else {
            continue;
        };
        let (op, rest) = match rest.split_once(' ') {
            Some((op @ ("ADD" | "REMOVE"), rest)) => (op, rest),
            _ => continue,
        };
        let entry = rest
            .strip_prefix('`')
            .and_then(|r| r.split_once('`'))
            .map(|(e, _)| e)
            .ok_or_else(|| format!("override line {}: no `ENDPOINT …`", n + 1))?;
        let fact = entry
            .strip_prefix("ENDPOINT ANY ")
            .and_then(|r| r.rsplit_once(" @"))
            .map(|(path, svc)| (svc.to_owned(), normalize_path(path)))
            .ok_or_else(|| {
                format!(
                    "override line {}: not `ENDPOINT ANY <path> @<service>`",
                    n + 1
                )
            })?;
        if op == "ADD" {
            gold.insert(fact);
        } else if !gold.remove(&fact) {
            return Err(format!(
                "override line {}: REMOVE of an endpoint the key lacks",
                n + 1
            ));
        }
    }
    Ok(())
}

/// The two cells of one application, and notes on what was mapped.
pub fn score_app(
    model: &str,
    trace: &str,
    overrides: &str,
    dump: &Dump,
) -> Result<(BTreeMap<Class, Cell>, Vec<String>), String> {
    analyse(model, trace, overrides, dump).map(|(cells, notes, _)| (cells, notes))
}

/// What an adjudicator judges for one application: every endpoint either side states (all of
/// them, so nothing says which side a statement came from), and, apart, the key's own list.
pub fn endpoint_review(model: &str, trace: &str, dump: &Dump) -> Result<Review, String> {
    analyse(model, trace, "", dump).map(|(_, _, review)| review)
}

/// One application's cells, notes and endpoint review.
type Analysis = (BTreeMap<Class, Cell>, Vec<String>, Review);

fn analyse(model: &str, trace: &str, overrides: &str, dump: &Dump) -> Result<Analysis, String> {
    let (model, repaired_model) = repair(model);
    let (trace, repaired_trace) = repair(trace);
    let model: Model =
        serde_json::from_str(&model).map_err(|e| format!("not a microSecEnD model: {e}"))?;
    let trace: Trace =
        serde_json::from_str(&trace).map_err(|e| format!("not a microSecEnD trace: {e}"))?;
    let services = Services::new(dump);

    let mut kind: BTreeMap<&str, Kind> = BTreeMap::new();
    for s in &model.services {
        let has = |st: &str| s.stereotypes.iter().any(|x| x == st);
        let k = if has("database") {
            Kind::Database
        } else if has("internal") {
            Kind::Internal
        } else {
            Kind::Infrastructural
        };
        kind.insert(&s.name, k);
    }
    for e in &model.external_entities {
        kind.insert(&e.name, Kind::External);
    }
    // Internal model services to rb services, through the first evidence file that maps. The
    // model writes `account_service` where its own traceability file keys `account-service`.
    let same = |n: &str| n.to_lowercase().replace('-', "_");
    let items: BTreeMap<String, &serde_json::Value> =
        trace.nodes.iter().map(|(k, v)| (same(k), v)).collect();
    let mut mapped: BTreeMap<&str, Result<String, &'static str>> = BTreeMap::new();
    for s in model
        .services
        .iter()
        .filter(|s| kind[s.name.as_str()] == Kind::Internal)
    {
        let rb = items
            .get(&same(&s.name))
            .map(|v| urls(v))
            .unwrap_or_default()
            .iter()
            .filter_map(|u| evidence_path(u))
            .find_map(|p| services.of_file(&p).map(str::to_owned))
            .ok_or(reason::NO_EVIDENCE_FILE);
        mapped.insert(&s.name, rb);
    }
    let internal_rb: BTreeSet<&str> = mapped.values().flatten().map(String::as_str).collect();
    // The rb services of internal model services that list their endpoints.
    let listed: BTreeSet<&str> = model
        .services
        .iter()
        .filter(|s| {
            s.tagged_values
                .get("Endpoints")
                .and_then(|v| v.as_array())
                .is_some_and(|a| !a.is_empty())
        })
        .filter_map(|s| mapped.get(s.name.as_str())?.as_ref().ok())
        .map(String::as_str)
        .collect();

    // An out-of-scope flow keeps its model pair, so distinct flows stay distinct.
    let marker = |reason: &str, f: &Flow| Loc {
        path: format!("<{reason}>"),
        line: 0,
        name: format!("{} -> {}", f.sender, f.receiver),
    };
    let marked = |l: &Loc| {
        [
            reason::EXTERNAL,
            reason::DATABASE,
            reason::NOT_MODELLED,
            reason::MESSAGING,
            reason::SAME_SERVICE,
        ]
        .into_iter()
        .find(|r| l.path == format!("<{r}>"))
    };

    // Gold flows. Model flows over one pair (REST and Feign, say) are one service link, as rb's.
    let mut flows: Side<()> = Side::default();
    for f in &model.information_flows {
        let ends = [kind.get(f.sender.as_str()), kind.get(f.receiver.as_str())];
        let out = if f.stereotypes.iter().any(|s| s.starts_with("message_")) {
            Some(reason::MESSAGING)
        } else if ends.contains(&Some(&Kind::External)) || ends.contains(&None) {
            Some(reason::EXTERNAL)
        } else if ends.contains(&Some(&Kind::Database)) {
            Some(reason::DATABASE)
        } else if ends.contains(&Some(&Kind::Infrastructural)) {
            Some(reason::NOT_MODELLED)
        } else {
            None
        };
        let id = match out {
            Some(r) => Ok(EdgeId {
                src: marker(r, f),
                class: Class::ServiceFlows,
                dst: Loc::service(&f.receiver),
            }),
            None => match (&mapped[f.sender.as_str()], &mapped[f.receiver.as_str()]) {
                (Ok(a), Ok(b)) if a == b => Ok(EdgeId {
                    src: marker(reason::SAME_SERVICE, f),
                    class: Class::ServiceFlows,
                    dst: Loc::service(&f.receiver),
                }),
                (Ok(a), Ok(b)) => Ok(EdgeId {
                    src: Loc::service(a),
                    class: Class::ServiceFlows,
                    dst: Loc::service(b),
                }),
                (Err(r), _) | (_, Err(r)) => Err(*r),
            },
        };
        match id {
            Ok(id) => {
                flows.located.insert(id, ());
            }
            Err(r) => *flows.unmatchable.entry(r).or_default() += 1,
        }
    }

    // Gold endpoints: each internal service's `Endpoints`, method unknown, then the overrides.
    let mut eps: Side<()> = Side::default();
    for s in model
        .services
        .iter()
        .filter(|s| kind[s.name.as_str()] == Kind::Internal)
    {
        let paths: Vec<String> = s
            .tagged_values
            .get("Endpoints")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.as_str())
                    .map(normalize_path)
                    .collect()
            })
            .unwrap_or_default();
        for p in paths {
            match &mapped[s.name.as_str()] {
                Ok(svc) => {
                    let id = EdgeId {
                        src: Loc::service(svc),
                        class: Class::Endpoints,
                        dst: Loc::endpoint(svc, "ANY", &p),
                    };
                    eps.located.insert(id, ());
                }
                Err(r) => *eps.unmatchable.entry(r).or_default() += 1,
            }
        }
    }

    // What rb found, in the same terms: flows as they are, endpoints with the method dropped.
    let locator = Locator::new(dump);
    let rb_flows = service_flows(dump, &locator, &services);
    let raw_eps = endpoints(dump);
    let mut rb_eps: Side<Found> = Side {
        located: BTreeMap::new(),
        unmatchable: raw_eps.unmatchable,
    };
    for (id, f) in raw_eps.located {
        let (service, rest) = id.dst.name.split_once(' ').unwrap_or_default();
        let path = rest.split_once(' ').map_or(rest, |(_, p)| p);
        let id = EdgeId {
            src: id.src,
            class: Class::Endpoints,
            dst: Loc::endpoint(service, "ANY", path),
        };
        rb_eps.insert(id, f);
    }

    let scope = |id: &EdgeId| {
        if let Some(r) = marked(&id.src) {
            return Scope::Out(r);
        }
        let in_model = |l: &Loc| internal_rb.contains(l.path.as_str());
        match id.class {
            Class::ServiceFlows if id.src == id.dst => Scope::Out(reason::SAME_SERVICE),
            Class::ServiceFlows if !(in_model(&id.src) && in_model(&id.dst)) => {
                Scope::Out(reason::NOT_IN_MODEL)
            }
            Class::Endpoints if !in_model(&id.src) => Scope::Out(reason::NOT_IN_MODEL),
            Class::Endpoints if !listed.contains(id.src.path.as_str()) => {
                Scope::Out(reason::NO_ENDPOINT_LIST)
            }
            _ => Scope::In,
        }
    };
    let mut gold_eps: BTreeSet<(String, String)> = eps
        .located
        .keys()
        .map(|id| (id.src.path.clone(), endpoint_path(&id.dst)))
        .collect();
    apply_overrides(&mut gold_eps, overrides)?;
    eps.located = gold_eps
        .iter()
        .map(|(svc, path)| {
            let id = EdgeId {
                src: Loc::service(svc),
                class: Class::Endpoints,
                dst: Loc::endpoint(svc, "ANY", path),
            };
            (id, ())
        })
        .collect();
    let review = Review {
        gold: gold_eps,
        found: rb_eps
            .located
            .keys()
            .filter(|id| scope(id) == Scope::In)
            .map(|id| (id.src.path.clone(), endpoint_path(&id.dst)))
            .collect(),
    };
    let cells = BTreeMap::from([
        (Class::ServiceFlows, score(&flows, &rb_flows, scope)),
        (Class::Endpoints, score(&eps, &rb_eps, scope)),
    ]);

    let internal = mapped.len();
    let ok = mapped.values().filter(|m| m.is_ok()).count();
    let mut notes = vec![format!(
        "{ok} of {internal} internal services mapped to {} rb services",
        internal_rb.len()
    )];
    if repaired_model || repaired_trace {
        notes.push("JSON repaired (trailing commas)".to_owned());
    }
    Ok((cells, notes, review))
}

/// The path of an endpoint location (`<service> <METHOD> <path>`).
fn endpoint_path(ep: &Loc) -> String {
    ep.name.splitn(3, ' ').nth(2).unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dump::parse;

    #[test]
    fn trailing_commas_are_removed_outside_strings_only() {
        let (t, r) = repair(r#"{"a": [1, 2,], "b": "x,}", "c": {"d": 1,},}"#);
        assert!(r);
        assert_eq!(t, r#"{"a": [1, 2], "b": "x,}", "c": {"d": 1}}"#);
        assert_eq!(
            repair(r#"{"a": "\",]"}"#),
            (r#"{"a": "\",]"}"#.to_owned(), false)
        );
    }

    #[test]
    fn evidence_urls_become_paths() {
        assert_eq!(
            evidence_path("https://github.com/o/r/blob/master/a/b.yml#L4").as_deref(),
            Some("a/b.yml")
        );
        assert_eq!(
            evidence_path("https://github.com/o/r/blob/master/master/a/b.yml#L4").as_deref(),
            Some("a/b.yml")
        );
        assert_eq!(evidence_path("heuristic"), None);
    }

    /// A trimmed real model shape (`ewolff_microservice`): a gateway (infrastructural), two
    /// internal services, a registry, a user; one internal flow rb finds, one it misses.
    const MODEL: &str = r#"{
      "services": [
        {"name": "zuul", "stereotypes": ["gateway", "infrastructural"], "tagged_values": {}},
        {"name": "eureka", "stereotypes": ["service_discovery", "infrastructural"], "tagged_values": {}},
        {"name": "order", "stereotypes": ["internal", "local_logging"], "tagged_values": {"Endpoints": ["/order", "/order/{id}"]}},
        {"name": "catalog", "stereotypes": ["internal"], "tagged_values": {"Endpoints": ["/catalog"]}},
        {"name": "customer", "stereotypes": ["internal"], "tagged_values": {"Endpoints": ["/customer"]}},
        {"name": "billing", "stereotypes": ["internal"], "tagged_values": {}},
        {"name": "db", "stereotypes": ["database"], "tagged_values": {}}
      ],
      "external_entities": [{"name": "user", "stereotypes": ["user_stereotype"], "tagged_values": {}}],
      "information_flows": [
        {"sender": "user", "receiver": "zuul", "stereotypes": ["restful_http"], "tagged_values": {}},
        {"sender": "zuul", "receiver": "order", "stereotypes": ["restful_http"], "tagged_values": {}},
        {"sender": "order", "receiver": "eureka", "stereotypes": ["restful_http"], "tagged_values": {}},
        {"sender": "order", "receiver": "catalog", "stereotypes": ["restful_http"], "tagged_values": {}},
        {"sender": "order", "receiver": "customer", "stereotypes": ["restful_http"], "tagged_values": {}},
        {"sender": "catalog", "receiver": "db", "stereotypes": ["jdbc"], "tagged_values": {}},
        {"sender": "order", "receiver": "catalog", "stereotypes": ["message_producer_kafka"], "tagged_values": {}}
      ]
    }"#;

    const TRACE: &str = r#"{"nodes": {
      "Order": {"file": "https://github.com/o/r/blob/master/order/src/main/resources/application.properties#L1", "line": 1, "span": "(0, 5)"},
      "catalog": {"file": "heuristic", "line": "heuristic", "span": "heuristic",
                  "sub_items": {"Port": {"file": "https://github.com/o/r/blob/master/master/catalog/pom.xml#L3", "line": 3, "span": "(0, 2)"}}},
      "customer": {"file": "implicit", "line": "implicit", "span": "implicit"},
      "billing": {"file": "https://github.com/o/r/blob/master/billing/pom.xml#L1", "line": 1, "span": "(0, 1)"}
    }, "edges": {}}"#;

    fn dump() -> Dump {
        let rows = [
            r#"["N","service","order","order",{}]"#,
            r#"["N","service","catalog","catalog",{}]"#,
            r#"["N","service","admin","admin",{}]"#,
            r#"["N","service","billing","billing",{}]"#,
            r#"["E","service","billing","EXPOSES","endpoint","HTTP GET /bills @billing","treesitter",5,{},[["order/src/main/java/O.java",[2,0,2,1]]]]"#,
            r#"["N","file","order/src/main/java/O.java","O.java",{}]"#,
            r#"["N","symbol","rb-heuristic . . . `order/src/main/java/O.java`/O#call().","call",{}]"#,
            r#"["E","file","order/src/main/java/O.java","IN_SERVICE","service","order","manifest",5,{},[]]"#,
            r#"["E","symbol","rb-heuristic . . . `order/src/main/java/O.java`/O#call().","DEFINED_IN","file","order/src/main/java/O.java","treesitter",5,{},[["order/src/main/java/O.java",[3,0,3,1]]]]"#,
            r#"["E","symbol","rb-heuristic . . . `order/src/main/java/O.java`/O#call().","CONSUMES","endpoint","HTTP GET /catalog @catalog","http_stitch",3,{},[["order/src/main/java/O.java",[4,0,4,1]]]]"#,
            r#"["E","symbol","rb-heuristic . . . `order/src/main/java/O.java`/O#call().","CONSUMES","endpoint","HTTP GET /x @admin","http_stitch",2,{},[["order/src/main/java/O.java",[5,0,5,1]]]]"#,
            r#"["E","symbol","rb-heuristic . . . `order/src/main/java/O.java`/O#call().","CONSUMES","endpoint","HTTP GET /order/{} @order","http_stitch",2,{},[["order/src/main/java/O.java",[6,0,6,1]]]]"#,
            r#"["E","service","order","EXPOSES","endpoint","HTTP GET /order/{} @order","treesitter",5,{},[["order/src/main/java/O.java",[2,0,2,1]]]]"#,
            r#"["E","service","order","EXPOSES","endpoint","HTTP POST /order/{} @order","treesitter",5,{},[["order/src/main/java/O.java",[2,0,2,1]]]]"#,
            r#"["E","service","catalog","EXPOSES","endpoint","HTTP GET /catalog @catalog","treesitter",5,{},[["order/src/main/java/O.java",[2,0,2,1]]]]"#,
            r#"["E","service","admin","EXPOSES","endpoint","HTTP GET /x @admin","treesitter",5,{},[["order/src/main/java/O.java",[2,0,2,1]]]]"#,
        ];
        parse(&format!("[\"rb-dump\",2,\"x\"]\n{}\n", rows.join("\n"))).unwrap()
    }

    #[test]
    fn a_model_scores_flows_and_endpoints() {
        let (cells, notes) = score_app(MODEL, TRACE, "", &dump()).unwrap();
        let flows = &cells[&Class::ServiceFlows];
        // order→catalog found; order→customer: customer has no evidence file.
        assert_eq!((flows.tp_total(), flows.fn_), (1, 0));
        assert_eq!(flows.unmatchable.get(reason::NO_EVIDENCE_FILE), Some(&1));
        let oos: BTreeMap<&str, u64> = flows
            .out_of_scope
            .iter()
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        assert_eq!(
            oos,
            BTreeMap::from([
                ("database", 1),
                ("external", 1),
                ("messaging", 1),
                ("not_modelled", 2)
            ])
        );
        assert_eq!(
            flows.found_out_of_scope.get(reason::NOT_IN_MODEL),
            Some(&1),
            "order→admin"
        );
        assert_eq!(
            flows.found_out_of_scope.get(reason::SAME_SERVICE),
            Some(&1),
            "order→order"
        );
        let eps = &cells[&Class::Endpoints];
        // /order/{} (rb's GET and POST merge) and /catalog found; /order missed.
        assert_eq!((eps.tp_total(), eps.fn_), (2, 1));
        assert_eq!(
            eps.unmatchable.get(reason::NO_EVIDENCE_FILE),
            Some(&1),
            "customer's /customer"
        );
        assert_eq!(
            eps.found_out_of_scope.get(reason::NOT_IN_MODEL),
            Some(&1),
            "admin's /x"
        );
        assert_eq!(
            eps.found_out_of_scope.get(reason::NO_ENDPOINT_LIST),
            Some(&1),
            "billing lists no endpoints, so its /bills is no false positive"
        );
        assert_eq!(notes[0], "3 of 4 internal services mapped to 3 rb services");
    }

    #[test]
    fn overrides_add_and_remove_by_entry_only() {
        let mut gold = BTreeSet::from([("svc".to_owned(), "/a".to_owned())]);
        let text = "# notes\n- REMOVE `ENDPOINT ANY /a @svc` — no such mapping\n- ADD `ENDPOINT ANY /p/{id} @svc` — prefix from config\nprose\n";
        apply_overrides(&mut gold, text).unwrap();
        assert_eq!(
            gold,
            BTreeSet::from([("svc".to_owned(), "/p/{}".to_owned())])
        );
        let err = apply_overrides(&mut gold, "- REMOVE `ENDPOINT ANY /zz @svc` — x\n").unwrap_err();
        assert!(err.contains("lacks"), "{err}");
    }

    #[test]
    fn the_review_lists_both_sides_and_overrides_change_the_score() {
        let review = endpoint_review(MODEL, TRACE, &dump()).unwrap();
        let has = |set: &BTreeSet<(String, String)>, svc: &str, p: &str| {
            set.contains(&(svc.to_owned(), p.to_owned()))
        };
        assert!(has(&review.gold, "order", "/order") && has(&review.gold, "catalog", "/catalog"));
        assert!(has(&review.found, "order", "/order/{}"), "{review:?}");
        assert!(
            !review
                .found
                .iter()
                .any(|(s, _)| s == "billing" || s == "admin"),
            "only listed services"
        );
        assert_eq!(
            endpoint_statement(&("order".to_owned(), "/order".to_owned())),
            "ENDPOINT ANY /order @order"
        );
        let fix = "- REMOVE `ENDPOINT ANY /order @order` — a class prefix, not an endpoint\n";
        let (cells, _) = score_app(MODEL, TRACE, fix, &dump()).unwrap();
        assert_eq!(
            cells[&Class::Endpoints].fn_,
            0,
            "the key's error no longer counts as a miss"
        );
    }
}
