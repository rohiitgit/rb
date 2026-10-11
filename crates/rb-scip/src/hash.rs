//! The cache key of a project's index (design §5.2): a digest of the blobs the indexer can see,
//! so an unchanged project is never re-indexed and a changed one never reuses a stale index.

use sha2::{Digest, Sha256};

/// SHA-256 (hex) over the sorted `(path, blob)` pairs of exactly the files the job exports (the
/// caller's selection: its build root's files, and for a scip-typescript job the files of its
/// in-repository link targets too, contract M6a D7; the root's lockfiles and manifests are
/// among them), the job's fetch recipe (`recipe`, [`crate::orchestrate::recipe`]; contract
/// M3b amendment 47, per indexer since contract M6a D8), so an index an older recipe produced
/// is a miss, and the fetch phase's allow-list (contract M5 S21), so a fetch that failed on a
/// host outside it is retried once the host is allowed. Files outside the selection do not
/// affect the key: the indexer never sees them.
pub fn project_input_hash<'a>(
    recipe: &str,
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
    egress: &crate::Egress,
) -> String {
    let recipe = format!("{recipe}\nallow {}", egress.hosts.join(" "));
    input_hash(files, &recipe)
}

fn input_hash<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>, recipe: &str) -> String {
    let mut entries: Vec<(&str, &str)> = files.into_iter().collect();
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

    fn key<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> String {
        project_input_hash(crate::RECIPE_JAVA, files, &crate::Egress::default())
    }

    /// Every exported file counts, by path and blob; order and repetition do not. (Which files
    /// a job exports is the caller's selection: `rb-indexer`'s `projects::job_files`.)
    #[test]
    fn every_exported_file_counts_and_order_does_not() {
        let a = key([("svc/pom.xml", "1"), ("svc/src/A.java", "2")]);
        let b = key([
            ("svc/src/A.java", "2"),
            ("svc/pom.xml", "1"),
            ("svc/pom.xml", "1"),
        ]);
        assert_eq!(a, b);
        let changed = key([("svc/pom.xml", "1"), ("svc/src/A.java", "3")]);
        assert_ne!(a, changed);
        let added = key([
            ("svc/pom.xml", "1"),
            ("svc/src/A.java", "2"),
            ("svc/B.java", "4"),
        ]);
        assert_ne!(a, added);
        let moved = key([("svc/pom.xml", "1"), ("svc/src/B.java", "2")]);
        assert_ne!(a, moved);
        assert_eq!(a.len(), 64);
    }

    /// Contract M3b amendment 47: the fetch recipe is part of the key, so a row an older
    /// binary's recipe wrote is a miss.
    #[test]
    fn the_recipe_is_part_of_the_key() {
        let files = [("svc/pom.xml", "1"), ("svc/src/A.java", "2")];
        assert_eq!(
            key(files),
            input_hash(
                files,
                &format!(
                    "{}\nallow files.pythonhosted.org plugins.gradle.org proxy.golang.org pypi.org registry.npmjs.org registry.yarnpkg.com repo.maven.apache.org repo1.maven.org storage.googleapis.com",
                    crate::RECIPE_JAVA
                )
            )
        );
        assert_ne!(input_hash(files, "m3b-1"), input_hash(files, "m3b-2"));
    }

    /// Contract M5 S21: the fetch phase's allow-list is part of the key.
    #[test]
    fn the_allow_list_is_part_of_the_key() {
        let files = [("svc/pom.xml", "1"), ("svc/src/A.java", "2")];
        let wider = crate::Egress::with(&["repo.spring.io".to_owned()]).unwrap();
        assert_ne!(
            key(files),
            project_input_hash(crate::RECIPE_JAVA, files, &wider)
        );
        assert_eq!(
            project_input_hash(crate::RECIPE_JAVA, files, &wider),
            project_input_hash(crate::RECIPE_JAVA, files, &wider)
        );
    }

    /// Contract M6a D8, invariant 7: splitting the recipe per indexer, and moving the root's
    /// selection to the caller (D7), leave every scip-java key as it was, so cached Java
    /// indexes stay valid. The value is what `cfa8ab9`'s `project_input_hash("svc", …)`
    /// computed for these files plus one outside `svc` (printed by that code before the split);
    /// scip-typescript's recipe gives another key for the same files.
    #[test]
    fn the_java_key_is_pinned_across_the_recipe_split() {
        let files = [("svc/pom.xml", "1"), ("svc/src/A.java", "2")];
        assert_eq!(crate::RECIPE_JAVA, "m5-egress-1");
        assert_eq!(
            key(files),
            "52fda8b2d590773546f2e15802148527ef6710270cba3a75bcadce88b954d340"
        );
        assert_ne!(
            project_input_hash(crate::RECIPE_TS, files, &crate::Egress::default()),
            key(files)
        );
    }
}
