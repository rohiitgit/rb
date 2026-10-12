//! Host names (design §6.5, contract M6g S4): Compose service names, Kubernetes `Service`
//! names and `rb.toml` host mappings extend the D6 host rule. Literals only; duplicates across
//! services stay ambiguous (never merged); names matching no service are counted, never edges.
//! The stitcher applies them; this module never touches the graph.

use crate::work::tick;

/// A Compose file, by file name: `compose.yaml`/`compose.yml` or
/// `docker-compose*.yaml`/`docker-compose*.yml`.
pub fn is_compose_input(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    (name == "compose.yaml"
        || name == "compose.yml"
        || (name.starts_with("docker-compose")
            && (name.ends_with(".yaml") || name.ends_with(".yml"))))
        && !name.contains('\t')
}

/// A host-name input: a Compose file or any YAML document (Kubernetes `Service` manifests are
/// found by their `kind:`, never by file name).
pub fn is_hosts_input(path: &str) -> bool {
    is_compose_input(path) || path.ends_with(".yaml") || path.ends_with(".yml")
}

/// Reads Compose `services:` names: top-level mapping keys one indent deeper than `services:`.
/// Tabs fail the file; anything unshaped reads as nothing.
pub fn compose_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    if text.contains('\t') {
        return out;
    }
    let mut services_at: Option<usize> = None;
    for line in text.split('\n') {
        tick(1);
        let indent = line.len() - line.trim_start_matches(' ').len();
        let content = line.trim();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        match services_at {
            None => {
                if content == "services:" && indent == 0 {
                    services_at = Some(indent);
                }
            }
            Some(top) => {
                if indent <= top {
                    break;
                }
                // A nested key (deeper than the first name level) ends nothing; only the
                // names' own level reads.
                if content.ends_with(':') {
                    let name = content.trim_end_matches(':').trim();
                    if indent == top + 2 && !name.is_empty() && !name.contains(' ') {
                        out.push(name.to_owned());
                    }
                }
            }
        }
    }
    out
}

/// One YAML document's `kind:` value (`Service`, `Deployment`, …), case-sensitive.
fn doc_kind(doc: &str) -> Option<&str> {
    for line in doc.split('\n') {
        tick(1);
        let content = line.trim();
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        if let Some(rest) = content.strip_prefix("kind:") {
            return Some(rest.trim());
        }
    }
    None
}

/// The `metadata.name` of a YAML document, or `None` when it states none.
fn doc_metadata_name(doc: &str) -> Option<String> {
    let mut in_metadata = false;
    let mut metadata_at = 0usize;
    for line in doc.split('\n') {
        tick(1);
        if line.contains('\t') {
            return None;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        let content = line.trim();
        if content.is_empty() || content.starts_with('#') || content == "---" {
            continue;
        }
        if content == "metadata:" {
            in_metadata = true;
            metadata_at = indent;
            continue;
        }
        if in_metadata {
            if indent <= metadata_at {
                in_metadata = false;
            } else if let Some(rest) = content.strip_prefix("name:") {
                let name = rest.trim().trim_matches('"').trim_matches('\'').trim();
                if !name.is_empty() && !name.contains([' ', '\t', '$', '{', '}']) {
                    return Some(name.to_owned());
                }
                return None;
            }
        }
    }
    None
}

/// Reads Kubernetes `Service` names: the `metadata.name` of every `---`-separated document
/// whose `kind:` is exactly `Service`. Deployments, ConfigMaps and unshaped documents read as
/// nothing.
pub fn k8s_service_names(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for doc in text.split("\n---") {
        tick(1);
        if doc_kind(doc) != Some("Service") {
            continue;
        }
        if let Some(name) = doc_metadata_name(doc) {
            out.push(name);
        }
    }
    out
}

/// One quoted string of a `names = [...]` list, or `None` for anything unshaped.
fn quoted_name(item: &str) -> Option<String> {
    let item = item.trim().trim_end_matches(',').trim();
    let inner = item
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| item.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))?;
    let name = inner.trim();
    (!name.is_empty() && !name.contains([' ', '\t', '$', '{', '}', ',', '[', ']']))
        .then(|| name.to_owned())
}

/// A `key = "value"` string field of a `[[hosts]]` section.
fn string_field(line: &str, key: &str) -> Option<String> {
    let rest = line.trim().strip_prefix(key)?.trim();
    let rest = rest.strip_prefix('=')?.trim();
    quoted_name(rest)
}

/// The root `rb.toml`'s `[[hosts]]` entries: `(service directory, names)`. Hand-rolled line
/// reader (no new dependency, per the M6f D1 precedent): a `[[hosts]]` section's `service =`
/// string and `names = [...]` list (single- or multi-line); anything unshaped reads as
/// nothing. Other sections (including `[x]` tables) end the section.
pub fn rb_toml_hosts(text: &str) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    let mut service: Option<String> = None;
    let mut names: Vec<String> = Vec::new();
    let mut in_names = false;
    let mut in_hosts = false;
    let flush = |service: &mut Option<String>,
                 names: &mut Vec<String>,
                 out: &mut Vec<(String, Vec<String>)>| {
        if let Some(dir) = service.take()
            && !dir.is_empty()
            && !names.is_empty()
        {
            out.push((dir, std::mem::take(names)));
        }
        names.clear();
    };
    for line in text.split('\n') {
        tick(1);
        let content = line.split('#').next().unwrap_or("").trim();
        if content.is_empty() {
            continue;
        }
        if content.starts_with('[') {
            flush(&mut service, &mut names, &mut out);
            in_names = false;
            in_hosts = content == "[[hosts]]";
            continue;
        }
        if !in_hosts {
            continue;
        }
        if in_names {
            for item in content.split(',') {
                if let Some(name) = quoted_name(item.trim_end_matches(']')) {
                    names.push(name);
                }
            }
            if content.contains(']') {
                in_names = false;
            }
            continue;
        }
        if let Some(dir) = string_field(content, "service") {
            service = Some(dir);
        } else if content.starts_with("names") {
            let rest = content.strip_prefix("names").unwrap_or("").trim();
            let rest = rest.strip_prefix('=').unwrap_or("").trim();
            let rest = rest.strip_prefix('[').unwrap_or(rest);
            if !content.contains('[') {
                continue;
            }
            for item in rest.split(',') {
                if let Some(name) = quoted_name(item.trim_end_matches(']')) {
                    names.push(name);
                }
            }
            in_names = !content.contains(']');
        }
    }
    flush(&mut service, &mut names, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work;

    /// Contract M6g S4: Compose service names pin the top-level mapping keys; tabs fail.
    #[test]
    fn compose_services_pin_the_top_level_names() {
        let text = "services:\n  auth:\n    image: auth:1\n  billing:\n    build: ./billing\nvolumes:\n  data:\n";
        assert_eq!(
            compose_names(text),
            vec!["auth".to_owned(), "billing".to_owned()]
        );
        assert!(compose_names("services:\n\t- x\n").is_empty());
        assert!(compose_names("volumes:\n  data:\n").is_empty());
        assert!(is_compose_input("deploy/compose.yaml"));
        assert!(is_compose_input("docker-compose.override.yml"));
        assert!(!is_compose_input("svc/application.yaml"));
        assert!(is_hosts_input("k8s/svc.yaml"));
    }

    /// Contract M6g S4: only `kind: Service` documents name a host; anything else is nothing.
    #[test]
    fn k8s_service_documents_pin_metadata_names() {
        let text = "apiVersion: v1\nkind: Service\nmetadata:\n  name: auth\n---\napiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: auth-deploy\n---\nkind: Service\nmetadata:\n  name: ${DYNAMIC}\n";
        assert_eq!(k8s_service_names(text), vec!["auth".to_owned()]);
    }

    /// Contract M6g S4: `rb.toml` host mappings pin service directories to names.
    #[test]
    fn rb_toml_hosts_pin_service_dirs_to_names() {
        let text = "[[hosts]]\nservice = \"services/auth\"\nnames = [\"auth\", \"login\"]\n";
        assert_eq!(
            rb_toml_hosts(text),
            vec![(
                "services/auth".to_owned(),
                vec!["auth".to_owned(), "login".to_owned()]
            )]
        );
        assert!(rb_toml_hosts("bogus = [\n").is_empty());
        assert!(rb_toml_hosts("exclude = [\"x\"]\n").is_empty());
    }

    /// Contract M6g S4, invariants 5–6: reading 4× the documents costs at most 5× the ops.
    #[test]
    fn host_reading_grows_linearly() {
        let work = |n: usize| {
            let mut text = String::from("services:\n");
            for i in 0..n {
                text.push_str(&format!("  svc{i}:\n    image: x:{i}\n"));
            }
            work::take();
            let names = compose_names(&text);
            assert_eq!(names.len(), n);
            work::take()
        };
        let (small, large) = (work(50), work(200));
        assert!(large <= small * 5, "{small} -> {large}");
    }
}
