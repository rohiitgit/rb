//! Topic graph (design §6.5, contract M6g S2): `Topic` nodes keyed by literal with
//! `Service -PUBLISHES-> Topic` and `Service -SUBSCRIBES-> Topic` edges (both originate at
//! services), never above `heuristic`. Placeholders resolve through the owning service's Spring
//! properties (single lookup; nested or missing stays dynamic); computed topics are dynamics.

use std::collections::{BTreeMap, BTreeSet};

use rb_frameworks::topics;
use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance,
};
use rb_syntax::{FileFacts, Language};
use serde_json::json;

/// What the topics stage reads.
pub(crate) struct Inputs<'a> {
    pub files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
    /// The key of the service owning a path.
    pub service_of: &'a dyn Fn(&str) -> String,
    /// Service key → its Spring properties.
    pub properties: &'a BTreeMap<String, BTreeMap<String, String>>,
}

/// Resolves a placeholder through the service's properties: the value, unless it is missing or
/// itself templated (both stay dynamic).
fn resolve_placeholder(properties: &BTreeMap<String, String>, name: &str) -> Option<String> {
    let key = name.strip_prefix("${")?.strip_suffix('}')?;
    let value = properties.get(key)?;
    (!value.contains("${")).then(|| value.clone())
}

/// Adds topic nodes and edges for every file's topic sites; dynamics (computed topics and
/// unresolvable placeholders) become counted `topic_dynamic` diagnostics, never edges.
pub(crate) fn add_topics(
    b: &mut GraphBuilder,
    inputs: &Inputs<'_>,
) -> anyhow::Result<Vec<Diagnostic>> {
    let mut diagnostics = Vec::new();
    let mut topics: BTreeSet<String> = BTreeSet::new();
    let mut paths: Vec<&str> = inputs.files.keys().copied().collect();
    paths.sort();
    for path in paths {
        let language = inputs.files[path].0;
        let facts = inputs.files[path].1;
        let service = (inputs.service_of)(path);
        let service_key = NodeKey::new(NodeKind::Service, service.clone());
        let found = topics::topics(language, facts);
        for site in &found.sites {
            rb_frameworks::work::tick(1);
            let name = match &site.topic {
                topics::TopicName::Literal(name) => {
                    if name.is_empty() {
                        continue;
                    }
                    name.clone()
                }
                topics::TopicName::Placeholder(name) => {
                    let properties = inputs.properties.get(&service);
                    match properties.and_then(|p| resolve_placeholder(p, name)) {
                        Some(value) if !value.is_empty() => value,
                        _ => {
                            diagnostics.push(dynamic(path, site.span, "unresolved"));
                            continue;
                        }
                    }
                }
            };
            let key = format!("topic/{name}");
            if topics.insert(key.clone()) {
                b.add_node(
                    NodeKey::new(NodeKind::Topic, key.clone()),
                    &name,
                    Attrs::new(),
                )?;
            }
            let mut edge = NewEdge::new(
                service_key.clone(),
                if site.publish {
                    EdgeKind::Publishes
                } else {
                    EdgeKind::Subscribes
                },
                NodeKey::new(NodeKind::Topic, key),
                Provenance::Heuristic,
                Certainty::Heuristic,
            );
            edge.evidence = vec![Evidence {
                path: path.to_owned(),
                span: site.span,
            }];
            b.add_edge(edge)?;
        }
        for site in &found.dynamic {
            rb_frameworks::work::tick(1);
            diagnostics.push(dynamic(path, site.span, "computed"));
        }
    }
    Ok(diagnostics)
}

fn dynamic(path: &str, span: rb_model::Span, reason: &str) -> Diagnostic {
    Diagnostic {
        kind: "topic_dynamic".into(),
        path: Some(path.to_owned()),
        span: Some(span),
        detail: json!({ "reason": reason }),
    }
}

#[cfg(test)]
mod tests {
    use rb_model::Graph;
    use rb_syntax::Language;

    use super::*;

    fn built(
        files: &[(&str, Language, &str)],
        properties: &[(&str, &str)],
    ) -> (Graph, Vec<Diagnostic>) {
        let facts: Vec<(String, Language, rb_syntax::FileFacts)> = files
            .iter()
            .map(|(p, l, src)| {
                (
                    (*p).to_owned(),
                    *l,
                    rb_syntax::extract(*l, src.as_bytes()).unwrap(),
                )
            })
            .collect();
        let by_path: BTreeMap<&str, (Language, &rb_syntax::FileFacts)> = facts
            .iter()
            .map(|(p, l, f)| (p.as_str(), (*l, f)))
            .collect();
        let service_of = |p: &str| p.split('/').next().unwrap_or(p).to_owned();
        let mut props: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
        for (service, kv) in properties {
            let (k, v) = kv.split_once('=').unwrap();
            props
                .entry((*service).to_owned())
                .or_default()
                .insert(k.to_owned(), v.to_owned());
        }
        let mut b = GraphBuilder::new();
        for service in ["svc", "other"] {
            b.add_node(
                NodeKey::new(NodeKind::Service, service),
                service,
                Attrs::new(),
            )
            .unwrap();
        }
        for (path, _, _) in files {
            b.add_node(NodeKey::new(NodeKind::File, *path), *path, Attrs::new())
                .unwrap();
        }
        let inputs = Inputs {
            files: &by_path,
            service_of: &service_of,
            properties: &props,
        };
        let diagnostics = add_topics(&mut b, &inputs).unwrap();
        (b.finish().unwrap(), diagnostics)
    }

    /// `srcKind -KIND-> dst [certainty]` per topic edge.
    fn edges(g: &Graph) -> Vec<String> {
        let key = |id: i64| g.nodes().iter().find(|n| n.id == id).unwrap().key.clone();
        let mut out: Vec<String> = g
            .edges()
            .iter()
            .filter(|e| matches!(e.kind, EdgeKind::Publishes | EdgeKind::Subscribes))
            .map(|e| {
                format!(
                    "{} -{}-> {} [{}]",
                    key(e.src),
                    e.kind.as_str(),
                    key(e.dst),
                    e.certainty.as_str()
                )
            })
            .collect();
        out.sort();
        out
    }

    /// Contract M6g S2: publishes and subscribes land on `topic/<name>` nodes, never above
    /// `heuristic`; placeholders resolve through the owning service's properties.
    #[test]
    fn topic_edges_publish_and_subscribe_by_literal() {
        let src = "package p;\nimport org.springframework.kafka.annotation.KafkaListener;\nimport org.springframework.kafka.core.KafkaTemplate;\nclass C {\n  KafkaTemplate<String, String> t;\n  @KafkaListener(topics = \"events\")\n  void consume(String m) {}\n  void produce() {\n    t.send(\"events\", \"v\");\n    t.send(\"${app.topic}\", \"v\");\n  }\n}\n";
        let (g, diagnostics) = built(
            &[("svc/C.java", Language::Java, src)],
            &[("svc", "app.topic=orders")],
        );
        assert_eq!(
            edges(&g),
            [
                "svc -PUBLISHES-> topic/events [heuristic]",
                "svc -PUBLISHES-> topic/orders [heuristic]",
                "svc -SUBSCRIBES-> topic/events [heuristic]",
            ]
        );
        assert!(diagnostics.is_empty());
        let kinds: Vec<String> = g
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::Topic)
            .map(|n| n.key.clone())
            .collect();
        assert_eq!(kinds, ["topic/events", "topic/orders"]);
    }

    /// Contract M6e... M6g S2: computed topics and unresolvable placeholders are counted
    /// diagnostics, never edges; empty literals are skipped silently.
    #[test]
    fn dynamic_and_unresolved_topics_are_counted() {
        let src = "package p;\nimport org.springframework.kafka.annotation.KafkaListener;\nimport org.springframework.kafka.core.KafkaTemplate;\nclass C {\n  KafkaTemplate<String, String> t;\n  @KafkaListener(topics = PREFIX + \"x\")\n  void a(String m) {}\n  @KafkaListener(topics = \"${missing}\")\n  void b(String m) {}\n  void c() {\n    t.send(name(), \"v\");\n  }\n}\n";
        let (g, diagnostics) = built(&[("svc/C.java", Language::Java, src)], &[]);
        assert!(edges(&g).is_empty());
        let kinds: Vec<&str> = diagnostics.iter().map(|d| d.kind.as_str()).collect();
        assert_eq!(kinds, ["topic_dynamic", "topic_dynamic", "topic_dynamic"]);
        // Sites resolve first (the unresolved placeholder), then the pack's dynamics.
        let reasons: Vec<&str> = diagnostics
            .iter()
            .map(|d| d.detail["reason"].as_str().unwrap())
            .collect();
        assert_eq!(reasons, ["unresolved", "computed", "computed"]);
    }
}
