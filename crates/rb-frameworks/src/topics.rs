//! Topic packs (design §6.5, contract M6g S1): Kafka and RabbitMQ topic literals from Java
//! code — listener annotations (subscribe) and template send calls (publish). Literals only;
//! anything computed is a counted dynamic site, never an edge. The indexer (phase 2) resolves
//! `${…}` placeholders through Spring properties and turns sites into edges.

use crate::work::tick;
use rb_syntax::{Decoration, FileFacts, Language, Span};

/// A topic name as written: a literal, or a `${…}` placeholder the indexer resolves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicName {
    Literal(String),
    Placeholder(String),
}

/// One topic site: what names it, where, and in which definition (for attribution).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopicSite {
    /// `true` for sends (publish), `false` for listeners (subscribe).
    pub publish: bool,
    pub topic: TopicName,
    pub span: Span,
    /// Index into `FileFacts::definitions` of the enclosing definition, if any.
    pub enclosing: Option<usize>,
}

/// A dynamic (non-literal) topic use: counted, never an edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DynamicSite {
    pub publish: bool,
    pub span: Span,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileTopics {
    pub sites: Vec<TopicSite>,
    pub dynamic: Vec<DynamicSite>,
}

/// The topic sites of one Java file, in source order.
pub fn topics(language: Language, facts: &FileFacts) -> FileTopics {
    let mut out = FileTopics::default();
    if language != Language::Java {
        return out;
    }
    for (i, def) in facts.definitions.iter().enumerate() {
        tick(1);
        for decoration in &def.decorations {
            listener(decoration, Some(i), &mut out);
        }
    }
    for chain in &facts.chains {
        tick(1);
        send(language, facts, chain, &mut out);
    }
    out
}

/// The simple name of a possibly qualified annotation (`a.b.C` → `C`).
fn simple(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// A listener annotation's topics: `topics` (string or array) or, for Rabbit, `queues`.
/// Returns the literal/placeholder values plus whether any value was dynamic. A listener with
/// no readable topic value (a pattern, partitions, bindings, or bare keys) is dynamic, never
/// silent (evaluator pass 1, P18).
fn annotation_topics(decoration: &Decoration) -> Option<(Vec<TopicName>, bool)> {
    let name = simple(&decoration.name);
    let key = match name {
        "KafkaListener" => "topics",
        "RabbitListener" => "queues",
        _ => return None,
    };
    // Other topic-carrying keys (patterns, partitions, bindings): their values never resolve
    // to a literal here, so any of them marks the listener dynamic.
    let mut dynamic = decoration.args.iter().any(|a| {
        matches!(
            a.key.as_deref(),
            Some("topicPattern" | "topicPartitions" | "partitions" | "bindings")
        )
    });
    let Some(arg) = decoration
        .args
        .iter()
        .find(|a| a.key.as_deref() == Some(key))
    else {
        return Some((Vec::new(), true));
    };
    // A `${…}` value is a property reference (the extractor marks it literal): always a
    // placeholder, never a literal topic. Anything SpEL (`#{…}`) or partially templated is
    // computed: dynamic, never an edge (evaluator pass 1, P32).
    let classify = |value: &str| {
        if is_placeholder(value) {
            Some(TopicName::Placeholder(value.to_owned()))
        } else if value.contains("#{") || value.contains("${") {
            None
        } else {
            Some(TopicName::Literal(value.to_owned()))
        }
    };
    let mut topics = Vec::new();
    if arg.items.is_empty() {
        if arg.literal || is_placeholder(&arg.value) {
            match classify(&arg.value) {
                Some(topic) => topics.push(topic),
                None => dynamic = true,
            }
        } else {
            dynamic = true;
        }
    } else {
        for item in &arg.items {
            if item.literal || is_placeholder(&item.value) {
                match classify(&item.value) {
                    Some(topic) => topics.push(topic),
                    None => dynamic = true,
                }
            } else {
                dynamic = true;
            }
        }
    }
    Some((topics, dynamic))
}

fn is_placeholder(value: &str) -> bool {
    value.starts_with("${") && value.ends_with('}')
}

/// Records a listener annotation's topics (subscribe sites) or dynamic marks.
fn listener(decoration: &Decoration, enclosing: Option<usize>, out: &mut FileTopics) {
    let Some((topics, dynamic)) = annotation_topics(decoration) else {
        return;
    };
    for topic in topics {
        out.sites.push(TopicSite {
            publish: false,
            topic,
            span: decoration.span,
            enclosing,
        });
    }
    if dynamic {
        out.dynamic.push(DynamicSite {
            publish: false,
            span: decoration.span,
        });
    }
}

/// The last `.`-segment of a dotted path.
fn last_segment(path: &str) -> &str {
    path.rsplit('.').next().unwrap_or(path)
}

/// A send call's topic: the first string-literal (or placeholder) argument.
fn send(language: Language, facts: &FileFacts, chain: &rb_syntax::CallChain, out: &mut FileTopics) {
    let (Some(root), [first]) = (chain.root.as_deref(), chain.links.as_slice()) else {
        return;
    };
    // The receiver as written (`KafkaTemplate.send`, `new KafkaTemplate().send`), else the
    // receiver binding's declared type (`t.send` with `KafkaTemplate t`) — the rest_template
    // precedent.
    let simple_root = last_segment(root.split('(').next().unwrap_or(root));
    let receiver =
        if simple_root.ends_with("KafkaTemplate") || simple_root.ends_with("RabbitTemplate") {
            simple_root.to_owned()
        } else {
            match crate::fold::local_binding(
                &crate::routes::FileScope::new(facts),
                language,
                root,
                chain.span,
            ) {
                Some(bi) => {
                    let owned = facts.bindings[bi].type_name.clone();
                    last_segment(owned.rsplit('(').next().unwrap_or(&owned)).to_owned()
                }
                None => return,
            }
        };
    let is_template = receiver.ends_with("KafkaTemplate") && first.name == "send"
        || receiver.ends_with("RabbitTemplate")
            && (first.name == "convertAndSend" || first.name == "send");
    if !is_template {
        // `sendDefault` publishes to the configured default topic: unresolvable here, so
        // counted dynamic, never silent (evaluator pass 1, P18).
        if receiver.ends_with("KafkaTemplate") && first.name == "sendDefault" {
            return dynamic_send(chain, out);
        }
        return;
    }
    let arg = first.args.first();
    // Rabbit's exchange-first arities route on a later argument (evaluator pass 1, P30): only
    // arities whose first argument is the routing target read it.
    let kafka = receiver.ends_with("KafkaTemplate");
    if !kafka {
        let arity = first.args.len();
        let routing_first =
            first.name == "convertAndSend" && arity == 2 || first.name == "send" && arity <= 2;
        if !routing_first {
            return dynamic_send(chain, out);
        }
    }
    let text = match arg {
        Some(rb_syntax::Arg::Template { template: t }) => {
            let mut lit = String::new();
            let mut ok = true;
            for part in &t.parts {
                match part {
                    rb_syntax::TemplatePart::Lit { text } => lit.push_str(text),
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if !ok {
                return dynamic_send(chain, out);
            }
            lit
        }
        _ => return dynamic_send(chain, out),
    };
    if text.is_empty() {
        return dynamic_send(chain, out);
    }
    // SpEL and partial templates are computed (evaluator pass 1, P32): dynamic, never an edge.
    if text.contains("#{") || text.contains("${") && !is_placeholder(&text) {
        return dynamic_send(chain, out);
    }
    let topic = if is_placeholder(&text) {
        TopicName::Placeholder(text)
    } else {
        TopicName::Literal(text)
    };
    out.sites.push(TopicSite {
        publish: true,
        topic,
        span: first.span,
        enclosing: chain.enclosing,
    });
}

/// A send whose topic is not a static string: counted, never an edge.
fn dynamic_send(chain: &rb_syntax::CallChain, out: &mut FileTopics) {
    if let Some(first) = chain.links.first() {
        out.dynamic.push(DynamicSite {
            publish: true,
            span: first.span,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work;
    use rb_syntax::extract;

    fn show(topics: &FileTopics) -> (Vec<String>, Vec<String>) {
        let sites = topics
            .sites
            .iter()
            .map(|s| {
                format!(
                    "{}:{}",
                    if s.publish { "pub" } else { "sub" },
                    match &s.topic {
                        TopicName::Literal(t) => t.clone(),
                        TopicName::Placeholder(t) => t.clone(),
                    }
                )
            })
            .collect();
        let dynamic = topics
            .dynamic
            .iter()
            .map(|d| (if d.publish { "pub" } else { "sub" }).to_owned())
            .collect();
        (sites, dynamic)
    }

    /// Contract M6g S1: listener annotations pin their topics (arrays, placeholders); dynamic
    /// values are counted, never edges; other annotations are ignored.
    #[test]
    fn kafka_and_rabbit_listeners_pin_their_topics() {
        let src = "package p;\nimport org.springframework.kafka.annotation.KafkaListener;\nclass C {\n  @KafkaListener(topics = \"events\")\n  void a(String m) {}\n  @KafkaListener(topics = {\"a\", \"b\"})\n  void b(String m) {}\n  @KafkaListener(topics = \"${app.topic}\")\n  void c(String m) {}\n  @KafkaListener(topics = PREFIX + \"x\")\n  void d(String m) {}\n  @GetMapping(\"/x\")\n  String e() { return \"\"; }\n}\n";
        let (sites, dynamic) = show(&topics(
            Language::Java,
            &extract(Language::Java, src.as_bytes()).unwrap(),
        ));
        assert_eq!(sites, ["sub:events", "sub:a", "sub:b", "sub:${app.topic}",]);
        assert_eq!(dynamic, ["sub"]);
        let rabbit = "package p;\nimport org.springframework.amqp.rabbit.annotation.RabbitListener;\nclass C {\n  @RabbitListener(queues = \"jobs\")\n  void a(String m) {}\n}\n";
        let (sites, dynamic) = show(&topics(
            Language::Java,
            &extract(Language::Java, rabbit.as_bytes()).unwrap(),
        ));
        assert_eq!(sites, ["sub:jobs"]);
        assert!(dynamic.is_empty());
        assert!(
            topics(
                Language::TypeScript,
                &extract(Language::TypeScript, b"x = 1\n".as_slice()).unwrap()
            )
            .sites
            .is_empty()
        );
    }

    /// Contract M6g S1: template sends pin their first-argument topic; anything computed is
    /// counted; other receivers and methods are ignored.
    #[test]
    fn template_sends_pin_their_first_argument_topic() {
        let src = "package p;\nimport org.springframework.kafka.core.KafkaTemplate;\nclass C {\n  KafkaTemplate<String, String> t;\n  MailTemplate m;\n  void a() {\n    t.send(\"events\", \"v\");\n    t.send(\"${app.topic}\", \"k\", \"v\");\n    t.send(name(), \"v\");\n    t.flush();\n    other.send(\"x\", \"v\");\n    m.send(\"x\", \"v\");\n    mailTemplate.send(\"x\", \"v\");\n  }\n}\n";
        let (sites, dynamic) = show(&topics(
            Language::Java,
            &extract(Language::Java, src.as_bytes()).unwrap(),
        ));
        assert_eq!(sites, ["pub:events", "pub:${app.topic}"]);
        assert_eq!(dynamic, ["pub"]);
        let rabbit = "package p;\nimport org.springframework.amqp.rabbit.core.RabbitTemplate;\nclass C {\n  RabbitTemplate t;\n  void a() {\n    t.convertAndSend(\"jobs\", \"v\");\n  }\n}\n";
        let (sites, _) = show(&topics(
            Language::Java,
            &extract(Language::Java, rabbit.as_bytes()).unwrap(),
        ));
        assert_eq!(sites, ["pub:jobs"]);
    }

    /// Contract M6g S1 (evaluator pass 1, P18): pattern/partitions/bindings listeners and
    /// `sendDefault` publish to unresolvable topics: counted dynamic, never edges or silence.
    #[test]
    fn unresolvable_topic_forms_are_counted_dynamic() {
        let src = "package p;\nimport org.springframework.kafka.annotation.KafkaListener;\nclass C {\n  @KafkaListener(topicPattern = \"events.*\")\n  void a(String m) {}\n  @KafkaListener(topicPartitions = @TopicPartition(topic = \"t\"))\n  void b(String m) {}\n  @KafkaListener(groupId = \"g\")\n  void c(String m) {}\n}\n";
        let (sites, dynamic) = show(&topics(
            Language::Java,
            &extract(Language::Java, src.as_bytes()).unwrap(),
        ));
        assert!(sites.is_empty());
        assert_eq!(dynamic, ["sub", "sub", "sub"]);
        let src = "package p;\nimport org.springframework.kafka.core.KafkaTemplate;\nclass C {\n  KafkaTemplate<String, String> t;\n  void a(String v) {\n    t.sendDefault(\"v\");\n  }\n}\n";
        let (sites, dynamic) = show(&topics(
            Language::Java,
            &extract(Language::Java, src.as_bytes()).unwrap(),
        ));
        assert!(sites.is_empty());
        assert_eq!(dynamic, ["pub"]);
        // SpEL and partial templates are computed, never literal edges.
        let src = "package p;\nimport org.springframework.kafka.annotation.KafkaListener;\nimport org.springframework.kafka.core.KafkaTemplate;\nclass C {\n  KafkaTemplate<String, String> t;\n  @KafkaListener(topics = \"#{__x.topic}\")\n  void a(String m) {}\n  @KafkaListener(topics = \"pre-${x}\")\n  void b(String m) {}\n  void c(String v) {\n    t.send(\"#{x}\", \"v\");\n    t.send(\"pre-${x}\", \"v\");\n  }\n}\n";
        let (sites, dynamic) = show(&topics(
            Language::Java,
            &extract(Language::Java, src.as_bytes()).unwrap(),
        ));
        assert!(sites.is_empty());
        assert_eq!(dynamic, ["sub", "sub", "pub", "pub"]);
    }

    /// Contract M6g S1 (evaluator pass 1, P30): Rabbit's 3-argument sends take the exchange
    /// first, not the topic: only arities whose first argument routes are read, the rest are
    /// counted dynamic. Kafka's topic is always first, at any arity.
    #[test]
    fn rabbit_sends_read_only_routing_first_arities() {
        let src = "package p;\nimport org.springframework.amqp.rabbit.core.RabbitTemplate;\nclass C {\n  RabbitTemplate t;\n  void a(Object v) {\n    t.convertAndSend(\"jobs\", \"v\");\n    t.convertAndSend(\"ex\", \"jobs\", \"v\");\n    t.send(\"jobs\", \"v\");\n    t.send(\"ex\", \"jobs\", \"v\");\n  }\n}\n";
        let (sites, dynamic) = show(&topics(
            Language::Java,
            &extract(Language::Java, src.as_bytes()).unwrap(),
        ));
        assert_eq!(sites, ["pub:jobs", "pub:jobs"]);
        assert_eq!(dynamic, ["pub", "pub"]);
    }

    /// Contract M6g S1, invariants 5–6: scanning 4× the sites costs at most 5× the operations.
    #[test]
    fn topic_scanning_grows_linearly() {
        let work = |n: usize| {
            let mut src = String::from(
                "package p;\nimport org.springframework.kafka.annotation.KafkaListener;\nclass C {\n",
            );
            for i in 0..n {
                src.push_str(&format!(
                    "  @KafkaListener(topics = \"t{i}\")\n  void m{i}(String m) {{}}\n"
                ));
            }
            src.push_str("}\n");
            let facts = extract(Language::Java, src.as_bytes()).unwrap();
            work::take();
            let out = topics(Language::Java, &facts);
            assert_eq!(out.sites.len(), n);
            work::take()
        };
        let (small, large) = (work(50), work(200));
        assert!(large <= small * 5, "{small} -> {large}");
    }
}
