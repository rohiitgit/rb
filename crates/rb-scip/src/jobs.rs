//! Which SCIP indexer runs where (design §5.2): every supported build root in a commit becomes
//! one container job with a fetch phase (network to registries) and an index phase (no network).

use serde::{Deserialize, Serialize};

/// The scip-java image: the slim one built locally from `docker/scip-java` (the published
/// `ghcr.io/scip-code/scip-java:0.13.1` is 6.75 GB). A local image has no registry digest, so
/// the cache key uses its image ID, resolved at run time by [`crate::Runner::image_id`]; when the
/// image is absent the job is `unsupported` and nothing builds it implicitly.
pub const SCIP_JAVA_IMAGE: &str = "rb-scip-java:0.13.1";
/// scip-typescript 0.4.0 is published for linux/amd64 only; on arm64 hosts it runs emulated.
pub const SCIP_TYPESCRIPT_IMAGE: &str = "sourcegraph/scip-typescript@sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Indexer {
    ScipJava,
    ScipTypescript,
}

impl Indexer {
    pub fn as_str(self) -> &'static str {
        match self {
            Indexer::ScipJava => "scip-java",
            Indexer::ScipTypescript => "scip-typescript",
        }
    }
}

/// How dependencies are fetched before indexing. A repository's own wrapper (`mvnw`,
/// `gradlew`) is never used: the image's tools are (contract M3b amendment 25).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Fetch {
    /// `mvn dependency:go-offline` and an online build with the image's Maven.
    Maven,
    /// A Gradle dependency resolution (not verified yet: such jobs are `unsupported`).
    Gradle,
    /// `npm ci`, `yarn install --frozen-lockfile` or `pnpm install --frozen-lockfile`.
    Npm { lockfile: NodeLock },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NodeLock {
    PackageLock,
    Yarn,
    Pnpm,
    /// No lockfile: the fetch is `npm install`, which is not reproducible; recorded on the job.
    None,
}

/// One indexer run: a build root, what to fetch, and the projects to index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    /// Repo-relative build root (`""` for the repository root): the container's working
    /// directory and the root the index's paths are relative to.
    pub root: String,
    pub indexer: Indexer,
    pub fetch: Fetch,
    /// scip-typescript: the `tsconfig.json` directories under `root`, relative to it, in order.
    pub projects: Vec<String>,
    /// The fetch step cannot be pinned (no lockfile), so the index is not reproducible.
    pub unpinned: bool,
}

/// The jobs for a commit, from its sorted file list. Maven and Gradle roots are the outermost
/// `pom.xml` / Gradle build files (a reactor or multi-project build runs once). TypeScript roots
/// are package roots holding a lockfile or `package.json`, each indexing every `tsconfig.json`
/// beneath it; a nested package root starts its own job.
pub fn jobs(files: &[&str]) -> Vec<Job> {
    let files: Vec<&str> = files
        .iter()
        .copied()
        .filter(|f| !f.split('/').any(|p| p == "node_modules"))
        .collect();
    let dirs_with = |name: &str| -> Vec<String> {
        files
            .iter()
            .filter(|f| f.rsplit('/').next() == Some(name))
            .map(|f| dir(f).to_owned())
            .collect()
    };
    let has = |path: &str| files.contains(&path);
    let join = |root: &str, name: &str| {
        if root.is_empty() {
            name.to_owned()
        } else {
            format!("{root}/{name}")
        }
    };

    let mut out = Vec::new();
    // Java: outermost build roots only.
    let maven = dirs_with("pom.xml");
    let gradle: Vec<String> = [
        "settings.gradle",
        "settings.gradle.kts",
        "build.gradle",
        "build.gradle.kts",
    ]
    .iter()
    .flat_map(|n| dirs_with(n))
    .collect();
    let mut java_roots: Vec<(String, Fetch)> = maven
        .iter()
        .map(|r| (r.clone(), Fetch::Maven))
        .chain(
            gradle
                .iter()
                .filter(|r| !maven.contains(r))
                .map(|r| (r.clone(), Fetch::Gradle)),
        )
        .collect();
    java_roots.sort();
    java_roots.dedup();
    // A Maven module under a Maven reactor, or a Gradle subproject under a Gradle root, is part of
    // its ancestor's build; a Gradle build under a Maven root is its own.
    let outermost: Vec<(String, Fetch)> = java_roots
        .iter()
        .filter(|(r, f)| {
            !java_roots
                .iter()
                .any(|(o, g)| o != r && is_under(r, o) && f == g)
        })
        .cloned()
        .collect();
    for (root, fetch) in outermost {
        out.push(Job {
            root,
            indexer: Indexer::ScipJava,
            fetch,
            projects: Vec::new(),
            unpinned: false,
        });
    }

    // TypeScript: package roots, nearest one owning each tsconfig.
    let mut package_roots: Vec<String> = dirs_with("package.json");
    package_roots.sort();
    package_roots.dedup();
    let tsconfigs = dirs_with("tsconfig.json");
    for root in &package_roots {
        let mut projects: Vec<String> = tsconfigs
            .iter()
            .filter(|t| {
                (t.as_str() == root || is_under(t, root)) && nearest_root(&package_roots, t) == root
            })
            .map(|t| relative(t, root))
            .collect();
        projects.sort();
        if projects.is_empty() {
            continue;
        }
        let lockfile = if has(&join(root, "package-lock.json")) {
            NodeLock::PackageLock
        } else if has(&join(root, "yarn.lock")) {
            NodeLock::Yarn
        } else if has(&join(root, "pnpm-lock.yaml")) {
            NodeLock::Pnpm
        } else {
            NodeLock::None
        };
        out.push(Job {
            root: root.clone(),
            indexer: Indexer::ScipTypescript,
            fetch: Fetch::Npm { lockfile },
            projects,
            unpinned: lockfile == NodeLock::None,
        });
    }
    out.sort_by(|a, b| (a.root.as_str(), a.indexer).cmp(&(b.root.as_str(), b.indexer)));
    out
}

fn dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// `dir` is strictly below `root` (`""` is the repository root).
fn is_under(dir: &str, root: &str) -> bool {
    dir != root && (root.is_empty() || dir.strip_prefix(root).is_some_and(|r| r.starts_with('/')))
}

/// The deepest package root at or above `dir`.
fn nearest_root<'a>(roots: &'a [String], dir: &str) -> &'a str {
    roots
        .iter()
        .filter(|r| r.as_str() == dir || is_under(dir, r))
        .max_by_key(|r| r.len())
        .map_or("", String::as_str)
}

/// `dir` relative to `root` (`.` for the root itself).
fn relative(dir: &str, root: &str) -> String {
    if dir == root {
        ".".to_owned()
    } else if root.is_empty() {
        dir.to_owned()
    } else {
        dir[root.len() + 1..].to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(files: &[&str]) -> Vec<String> {
        jobs(files)
            .into_iter()
            .map(|j| {
                let fetch = match j.fetch {
                    Fetch::Maven => "maven".to_owned(),
                    Fetch::Gradle => "gradle".to_owned(),
                    Fetch::Npm { lockfile } => format!("npm({lockfile:?})"),
                };
                let mut s = format!("{} {} {fetch}", j.indexer.as_str(), j.root);
                if !j.projects.is_empty() {
                    s.push_str(&format!(" [{}]", j.projects.join(",")));
                }
                if j.unpinned {
                    s.push_str(" unpinned");
                }
                s
            })
            .collect()
    }

    #[test]
    fn java_build_roots_are_the_outermost_ones() {
        let files = [
            "pom.xml",
            "mvnw",
            "core/pom.xml",
            "web/pom.xml",
            "web/src/Main.java",
            "tools/settings.gradle.kts",
            "tools/gradlew",
            "tools/lib/build.gradle.kts",
            "legacy/build.gradle",
        ];
        assert_eq!(
            summary(&files),
            [
                "scip-java  maven",
                "scip-java legacy gradle",
                "scip-java tools gradle",
            ]
        );
    }

    #[test]
    fn typescript_jobs_group_tsconfigs_under_their_nearest_package_root() {
        let files = [
            "package.json",
            "package-lock.json",
            "tsconfig.json",
            "apps/web/package.json",
            "apps/web/tsconfig.json",
            "apps/web/src/index.ts",
            "packages/shared/package.json",
            "packages/shared/tsconfig.json",
            "packages/shared/tsconfig.build.json",
            "services/notification/package.json",
            "services/notification/yarn.lock",
            "services/notification/tsconfig.json",
            "examples/demo/package.json",
            "examples/demo/index.js",
            "node_modules/dep/package.json",
            "node_modules/dep/tsconfig.json",
        ];
        assert_eq!(
            summary(&files),
            [
                // The root package holds a lockfile but the nested packages have no lockfile of
                // their own: they are separate, unpinned jobs (workspace installs are M6).
                "scip-typescript  npm(PackageLock) [.]",
                "scip-typescript apps/web npm(None) [.] unpinned",
                "scip-typescript packages/shared npm(None) [.] unpinned",
                "scip-typescript services/notification npm(Yarn) [.]",
            ]
        );
    }

    #[test]
    fn a_package_without_tsconfig_is_not_a_job_and_mixed_repos_list_both() {
        let files = [
            "services/auth/pom.xml",
            "services/auth/src/A.java",
            "apps/web/package.json",
            "apps/web/tsconfig.json",
            "apps/web/src/a.ts",
            "examples/demo/package.json",
            "examples/demo/index.js",
            "services/billing/go.mod",
        ];
        assert_eq!(
            summary(&files),
            [
                "scip-typescript apps/web npm(None) [.] unpinned",
                "scip-java services/auth maven",
            ]
        );
        assert!(jobs(&["README.md"]).is_empty());
    }
}
