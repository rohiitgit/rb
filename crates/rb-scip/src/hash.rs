//! The cache key of a project's index (design §5.2): a digest of the blobs the indexer can see,
//! so an unchanged project is never re-indexed and a changed one never reuses a stale index.

use sha2::{Digest, Sha256};

/// SHA-256 (hex) over the sorted `(path, blob)` pairs of every file at or below `root`, plus the
/// build root's own lockfiles and manifests, which are already among them, the fetch recipe
/// ([`crate::RECIPE`]; contract M3b amendment 47), so an index an older recipe produced is a
/// miss, and the fetch phase's allow-list (contract M5 S21), so a fetch that failed on a host
/// outside it is retried once the host is allowed. Files outside the root do not affect the
/// key: an indexer only reads its build root.
pub fn project_input_hash<'a>(
    root: &str,
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
    egress: &crate::Egress,
) -> String {
    let recipe = format!("{}\nallow {}", crate::RECIPE, egress.hosts.join(" "));
    input_hash(root, files, &recipe)
}

fn input_hash<'a>(
    root: &str,
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
    recipe: &str,
) -> String {
    let mut entries: Vec<(&str, &str)> = files
        .into_iter()
        .filter(|(path, _)| {
            root.is_empty() || path.strip_prefix(root).is_some_and(|r| r.starts_with('/'))
        })
        .collect();
    entries.sort_unstable();
    entries.dedup();
    let mut h = Sha256::new();
    h.update(b"rb-scip-input-v1\n");
    h.update(recipe.as_bytes());
    h.update(b"\n");
    for (path, blob) in entries {
        h.update(path.as_bytes());
        h.update(b"\0");
        h.update(blob.as_bytes());
        h.update(b"\n");
    }
    format!("{:x}", h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key<'a>(root: &str, files: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
        project_input_hash(root, files, &crate::Egress::default())
    }

    #[test]
    fn only_files_under_the_root_count_and_order_does_not() {
        let a = key(
            "svc",
            [
                ("svc/pom.xml", "1"),
                ("svc/src/A.java", "2"),
                ("other/x", "9"),
            ],
        );
        let b = key(
            "svc",
            [
                ("svc/src/A.java", "2"),
                ("svc/pom.xml", "1"),
                ("other/y", "8"),
            ],
        );
        assert_eq!(a, b);
        let changed = key("svc", [("svc/pom.xml", "1"), ("svc/src/A.java", "3")]);
        assert_ne!(a, changed);
        let added = key(
            "svc",
            [
                ("svc/pom.xml", "1"),
                ("svc/src/A.java", "2"),
                ("svc/B.java", "4"),
            ],
        );
        assert_ne!(a, added);
        // `svc2/…` is not under `svc`.
        let sibling = key(
            "svc",
            [
                ("svc/pom.xml", "1"),
                ("svc/src/A.java", "2"),
                ("svc2/x", "5"),
            ],
        );
        assert_eq!(a, sibling);
        // The repository root sees everything.
        assert_ne!(key("", [("a", "1")]), key("", [("a", "1"), ("b", "1")]));
        assert_eq!(a.len(), 64);
    }

    /// Contract M3b amendment 47: the fetch recipe is part of the key, so a row an older
    /// binary's recipe wrote is a miss.
    #[test]
    fn the_recipe_is_part_of_the_key() {
        let files = [("svc/pom.xml", "1"), ("svc/src/A.java", "2")];
        assert_eq!(
            key("svc", files),
            input_hash(
                "svc",
                files,
                &format!(
                    "{}\nallow registry.npmjs.org registry.yarnpkg.com repo.maven.apache.org repo1.maven.org",
                    crate::RECIPE
                )
            )
        );
        assert_ne!(
            input_hash("svc", files, "m3b-1"),
            input_hash("svc", files, "m3b-2")
        );
    }

    /// Contract M5 S21: the fetch phase's allow-list is part of the key.
    #[test]
    fn the_allow_list_is_part_of_the_key() {
        let files = [("svc/pom.xml", "1"), ("svc/src/A.java", "2")];
        let wider = crate::Egress::with(&["repo.spring.io".to_owned()]).unwrap();
        assert_ne!(key("svc", files), project_input_hash("svc", files, &wider));
        assert_eq!(
            project_input_hash("svc", files, &wider),
            project_input_hash("svc", files, &wider)
        );
    }
}
