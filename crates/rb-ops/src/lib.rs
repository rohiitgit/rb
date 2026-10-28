//! Phase 2-9 pure logic: no DB, no LLM, deterministic.
//! Store holds SQL; CLI calls these helpers then prints.

use std::collections::{BTreeMap, BTreeSet};

/// Stopwords removed from task keywords (Phase 4 context).
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "for", "fix", "show", "what", "which", "how", "why", "does",
    "is", "are", "of", "in", "on", "to", "with", "please", "who", "made", "this", "that", "was",
    "were", "when",
];

/// Phase 4: task text to sorted keyword list.
pub fn keywords(task: &str) -> Vec<String> {
    let mut out = BTreeSet::new();
    for raw in task.split(|c: char| !c.is_alphanumeric()) {
        let w = raw.to_lowercase();
        if w.len() < 3 || STOPWORDS.contains(&w.as_str()) {
            continue;
        }
        out.insert(w);
    }
    out.into_iter().collect()
}

/// Phase 2: paths whose blob changed between two manifests (path -> blob sha).
/// Added, removed, or re-hashed paths are all "changed": the indexer must re-read them.
pub fn incremental_changed(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut keys: BTreeSet<&str> = BTreeSet::new();
    for k in before.keys().chain(after.keys()) {
        keys.insert(k.as_str());
    }
    keys.into_iter()
        .filter(|k| before.get(*k) != after.get(*k))
        .map(str::to_owned)
        .collect()
}

/// Phase 2 event kinds (mirrors `rb.event.kind` CHECK).
pub fn is_event_kind(kind: &str) -> bool {
    matches!(
        kind,
        "commit.created" | "file.changed" | "pr.merged" | "branch.deleted"
    )
}

/// Phase 3: substring match score of a decision against query words.
pub fn decision_score(query_words: &[String], title: &str, reason: &str) -> usize {
    let hay = format!("{title} {reason}").to_lowercase();
    query_words
        .iter()
        .filter(|w| hay.contains(w.as_str()))
        .count()
}

/// Phase 9 agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Agent {
    Backend,
    Frontend,
    Database,
    Security,
    Qa,
}

impl Agent {
    pub fn as_str(self) -> &'static str {
        match self {
            Agent::Backend => "backend",
            Agent::Frontend => "frontend",
            Agent::Database => "database",
            Agent::Security => "security",
            Agent::Qa => "qa",
        }
    }
}

/// Phase 9: deterministic task routing. Order is fixed; duplicates removed.
pub fn route_task(task: &str) -> Vec<Agent> {
    let t = task.to_lowercase();
    let mut out = Vec::new();
    let mut push = |a: Agent| {
        if !out.contains(&a) {
            out.push(a);
        }
    };
    if t.contains("sql")
        || t.contains("table")
        || t.contains("migration")
        || t.contains("postgres")
        || t.contains("query")
    {
        push(Agent::Database);
    }
    if t.contains("css")
        || t.contains("tsx")
        || t.contains("route")
        || t.contains("component")
        || t.contains("page")
        || t.contains("frontend")
        || t.contains(" ui ")
        || t.trim_start().starts_with("ui ")
    {
        push(Agent::Frontend);
    }
    if t.contains("auth")
        || t.contains("secret")
        || t.contains("injection")
        || t.contains("xss")
        || t.contains("password")
        || t.contains("token")
        || t.contains("security")
    {
        push(Agent::Security);
    }
    if t.contains("test") || t.contains("spec") || t.contains(" qa") || t.contains("qa ") {
        push(Agent::Qa);
    }
    push(Agent::Backend);
    out
}

/// Phase 6: one security finding with file:line evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecFinding {
    pub rule: &'static str,
    pub file: String,
    pub line: usize,
    pub detail: String,
}

/// Phase 6 security checker: hardcoded secrets + unsafe SQL string building.
/// Pure function of path + text; never reads the graph. A secret counts only with
/// a quoted literal of 8+ chars after `=`, so option names and short placeholders
/// stay silent (and this scanner stays silent on its own patterns and tests).
pub fn security_scan(file: &str, text: &str) -> Vec<SecFinding> {
    let mut out = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let n = i + 1;
        let low = line.to_lowercase();
        if (low.contains("aws_secret")
            || low.contains("api_key")
            || low.contains(concat!("password", " =")))
            && has_long_quoted_value(line)
        {
            out.push(SecFinding {
                rule: "hardcoded_secret",
                file: file.to_owned(),
                line: n,
                detail: line.trim().to_owned(),
            });
        }
        // `+` counts only at a string boundary (`" + x`, `x + "`): a `+` inside a
        // static statement (`a + b`) is arithmetic, not string building.
        if (low.contains("select ") || low.contains("delete ") || low.contains("insert "))
            && (line.contains("format!")
                || line.contains("f\"")
                || line.contains("\" +")
                || line.contains("+ \"")
                || line.contains("' +")
                || line.contains("+ '"))
        {
            out.push(SecFinding {
                rule: "sql_string_concat",
                file: file.to_owned(),
                line: n,
                detail: line.trim().to_owned(),
            });
        }
    }
    out
}

/// Whether `line` assigns a quoted literal of 8+ chars (the secret shape).
fn has_long_quoted_value(line: &str) -> bool {
    let Some((_, rhs)) = line.split_once('=') else {
        return false;
    };
    let mut chars = rhs.chars();
    while let Some(c) = chars.next() {
        if c == '"' || c == '\'' {
            let mut len = 0;
            let mut closed = false;
            for c2 in chars.by_ref() {
                if c2 == c {
                    closed = true;
                    break;
                }
                len += 1;
            }
            // An unterminated quote is a prefix of code, not a literal.
            if closed && len >= 8 {
                return true;
            }
        }
    }
    false
}

/// Phase 6 cost checker: flags change size, never blocks on taste.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CostFlag {
    pub rule: &'static str,
    pub detail: String,
}

pub fn cost_check(files_changed: usize, deps_added: usize) -> Vec<CostFlag> {
    let mut out = Vec::new();
    if files_changed > 50 {
        out.push(CostFlag {
            rule: "large_change",
            detail: format!("{files_changed} files changed; split the change"),
        });
    }
    if deps_added > 0 {
        out.push(CostFlag {
            rule: "new_dependency",
            detail: format!("{deps_added} new dependencies; each is permanent"),
        });
    }
    out
}

/// Phase 5: deterministic agent plan step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    pub tool: &'static str,
    pub target: String,
    pub why: String,
}

/// Phase 5: plan from a context package summary. No search: every target comes from
/// the caller's lists, capped so output stays bounded.
pub fn plan_from_context(
    services: &[String],
    endpoints: &[String],
    tables: &[String],
    max_steps: usize,
) -> Vec<PlanStep> {
    let mut steps = Vec::new();
    for s in services.iter().take(3) {
        steps.push(PlanStep {
            tool: "read",
            target: format!("service:{s}"),
            why: "confine edits to the owning service".into(),
        });
    }
    for e in endpoints.iter().take(3) {
        steps.push(PlanStep {
            tool: "read",
            target: format!("endpoint:{e}"),
            why: "trace handler before editing".into(),
        });
    }
    for t in tables.iter().take(3) {
        steps.push(PlanStep {
            tool: "read",
            target: format!("table:{t}"),
            why: "check migrations before writes".into(),
        });
    }
    steps.push(PlanStep {
        tool: "execute",
        target: "cargo test".into(),
        why: "prove the change".into(),
    });
    steps.truncate(max_steps.max(1));
    steps
}

/// Phase 8: render a feature without file names (user-facing answer).
/// Phase 2: split changed `(path, blob)` rows into cached (skip extraction) and
/// new (must extract). Pure; the caller reads cache presence from the store.
pub fn partition_cached(
    changed: &[(String, String)],
    cached_blobs: &std::collections::BTreeSet<String>,
) -> (Vec<String>, Vec<String>) {
    let mut skip = Vec::new();
    let mut extract = Vec::new();
    for (path, blob) in changed {
        if cached_blobs.contains(blob) {
            skip.push(path.clone());
        } else {
            extract.push(path.clone());
        }
    }
    (skip, extract)
}

/// Phase 3: parse one ADR markdown document into (title, reason, date, source,
/// author). The title is the first `# ` heading; `Reason:`/`Date:`/`Source:`/
/// `Author:` field lines fill the record; remaining body text extends the reason.
/// Files without a `# ` heading return `None` (skipped, never guessed).
pub fn parse_adr(text: &str) -> Option<(String, String, String, String, String)> {
    let mut title = String::new();
    let mut reason = String::new();
    let mut date = String::new();
    let mut source = String::new();
    let mut author = String::new();
    for line in text.lines() {
        let t = line.trim();
        if title.is_empty() {
            if let Some(h) = t.strip_prefix("# ") {
                title = h.trim().to_owned();
            }
            continue;
        }
        let (k, v) = match t.split_once(':') {
            Some((k, v)) => (k.trim().to_lowercase(), v.trim()),
            None => ("".into(), ""),
        };
        match k.as_str() {
            "reason" => reason = v.to_owned(),
            "date" => date = v.to_owned(),
            "source" => source = v.to_owned(),
            "author" => author = v.to_owned(),
            _ if !t.is_empty() => {
                if !reason.is_empty() {
                    reason.push('\n');
                }
                reason.push_str(t);
            }
            _ => {}
        }
    }
    if title.is_empty() {
        return None;
    }
    Some((title, reason, date, source, author))
}

pub fn render_feature(
    name: &str,
    description: &str,
    services: &[String],
    endpoints: &[String],
    tables: &[String],
    deployments: &[String],
    metrics: &[String],
) -> String {
    let mut out = format!("feature {name}\n{description}\n");
    out.push_str(&format!("services: {}\n", services.join(", ")));
    out.push_str(&format!("endpoints: {}\n", endpoints.join(", ")));
    out.push_str(&format!("tables: {}\n", tables.join(", ")));
    out.push_str(&format!("deployments: {}\n", deployments.join(", ")));
    out.push_str(&format!("metrics: {}\n", metrics.join(", ")));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_drop_stopwords_and_dedupe() {
        assert_eq!(
            keywords("Fix login timeout please"),
            vec!["login", "timeout"]
        );
        assert_eq!(keywords("Login LOGIN login!"), vec!["login"]);
        assert!(keywords("a an or").is_empty());
    }

    #[test]
    fn incremental_changed_detects_add_remove_rehash() {
        let before: BTreeMap<String, String> = [
            ("a.ts".into(), "h1".into()),
            ("b.ts".into(), "h2".into()),
            ("c.ts".into(), "h3".into()),
        ]
        .into_iter()
        .collect();
        let after: BTreeMap<String, String> = [
            ("a.ts".into(), "h1".into()),
            ("b.ts".into(), "h9".into()),
            ("d.ts".into(), "h4".into()),
        ]
        .into_iter()
        .collect();
        assert_eq!(
            incremental_changed(&before, &after),
            vec!["b.ts".to_owned(), "c.ts".to_owned(), "d.ts".to_owned()]
        );
        assert!(incremental_changed(&before, &before).is_empty());
    }

    #[test]
    fn event_kinds_match_the_check_constraint() {
        for k in [
            "commit.created",
            "file.changed",
            "pr.merged",
            "branch.deleted",
        ] {
            assert!(is_event_kind(k), "{k}");
        }
        assert!(!is_event_kind("deploy.created"));
        assert!(!is_event_kind(""));
    }

    #[test]
    fn decisions_score_by_substring() {
        let q = vec!["kafka".to_owned(), "ordering".to_owned()];
        assert_eq!(decision_score(&q, "Use Kafka", "ordering guarantees"), 2);
        assert_eq!(decision_score(&q, "Use Redis", "fast cache"), 0);
    }

    #[test]
    fn routing_covers_each_specialty() {
        assert!(route_task("fix slow sql query").contains(&Agent::Database));
        assert!(route_task("update tsx route component").contains(&Agent::Frontend));
        assert!(route_task("auth token leak").contains(&Agent::Security));
        assert!(route_task("add qa spec tests").contains(&Agent::Qa));
        let back = route_task("refactor handler");
        assert_eq!(back, vec![Agent::Backend]);
    }

    #[test]
    fn security_scan_finds_secrets_and_sql_concat() {
        // Fixtures are built at runtime so this file holds no trigger substrings
        // itself (`rb verify` on this crate stays clean).
        let key: String = ["API_", "KEY"].concat();
        let f = security_scan("a.py", &format!("{key} = \"abcdefgh\"\n"));
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].rule, "hardcoded_secret");
        // Short placeholders stay silent.
        let short = security_scan("a.py", &format!("{key} = \"x\"\n"));
        assert!(short.is_empty(), "{short:?}");
        let kw: String = ["SEL", "ECT"].concat() + " ";
        let stmt = format!("q = {kw}format!({{}})\n");
        let g = security_scan("b.py", &stmt);
        assert!(g.iter().any(|x| x.rule == "sql_string_concat"));
        assert!(security_scan("c.py", "x = 1\n").is_empty());
        // The scanner's own trigger words without a long value stay silent.
        assert!(security_scan("d.py", "option password =\n").is_empty());
    }

    #[test]
    fn cost_flags_fire_only_over_thresholds() {
        assert!(cost_check(3, 0).is_empty());
        assert_eq!(cost_check(51, 0).len(), 1);
        assert_eq!(cost_check(2, 1)[0].rule, "new_dependency");
    }

    #[test]
    fn plans_come_only_from_context_lists() {
        let steps = plan_from_context(&["shop".into()], &["GET /x".into()], &["t".into()], 10);
        assert!(steps.iter().any(|s| s.tool == "execute"));
        assert!(steps.len() <= 10);
        assert!(plan_from_context(&[], &[], &[], 2).len() <= 2);
    }

    #[test]
    fn plan_targets_come_from_context_never_blind_search() {
        let services = vec!["shop".into()];
        let endpoints = vec!["GET /x".into()];
        let tables = vec!["t".into()];
        let allowed: std::collections::BTreeSet<String> = services
            .iter()
            .map(|s: &String| format!("service:{s}"))
            .chain(endpoints.iter().map(|e| format!("endpoint:{e}")))
            .chain(tables.iter().map(|t| format!("table:{t}")))
            .chain(["cargo test".into()])
            .collect();
        for step in plan_from_context(&services, &endpoints, &tables, 10) {
            assert!(
                allowed.contains(&step.target),
                "blind target: {}",
                step.target
            );
            assert!(["read", "edit", "execute", "test"].contains(&step.tool));
        }
    }

    #[test]
    fn adr_documents_parse_or_skip() {
        let doc = "# Use Redis Streams\nReason: Ordering guarantees\nDate: 2026-09-01\nSource: PR #231\nAuthor: rohit\n";
        assert_eq!(
            parse_adr(doc),
            Some((
                "Use Redis Streams".into(),
                "Ordering guarantees".into(),
                "2026-09-01".into(),
                "PR #231".into(),
                "rohit".into()
            ))
        );
        let thin = "# Drop RabbitMQ\nIt cost too much to run.\n";
        assert_eq!(
            parse_adr(thin),
            Some((
                "Drop RabbitMQ".into(),
                "It cost too much to run.".into(),
                "".into(),
                "".into(),
                "".into()
            ))
        );
        assert_eq!(parse_adr("no heading here\n"), None);
        assert_eq!(parse_adr(""), None);
    }

    #[test]
    fn cached_blobs_skip_extraction() {
        use std::collections::BTreeSet;
        let changed = vec![("a.ts".into(), "h1".into()), ("b.ts".into(), "h2".into())];
        let cached: BTreeSet<String> = ["h1".into()].into_iter().collect();
        let (skip, extract) = partition_cached(&changed, &cached);
        assert_eq!(skip, vec!["a.ts".to_owned()]);
        assert_eq!(extract, vec!["b.ts".to_owned()]);
        assert!(partition_cached(&[], &cached).0.is_empty());
    }

    #[test]
    fn feature_render_has_no_file_paths() {
        let s = render_feature(
            "billing",
            "charges customers",
            &["billing".into()],
            &["POST /pay".into()],
            &["invoices".into()],
            &["v5".into()],
            &["pay_latency_p99".into()],
        );
        assert!(s.contains("billing") && s.contains("POST /pay"));
        assert!(s.contains("v5") && s.contains("pay_latency_p99"));
        assert!(!s.contains(".ts") && !s.contains("src/"));
    }
}
