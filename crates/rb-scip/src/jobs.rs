//! Which SCIP indexer runs where (design §5.2): every supported build root in a commit becomes
//! one container job with a fetch phase (network to registries) and an index phase (no network).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The scip-java image: the slim one built locally from `docker/scip-java` (the published
/// `ghcr.io/scip-code/scip-java:0.13.1` is 6.75 GB). A local image has no registry digest, so
/// the cache key uses its image ID, resolved at run time by [`crate::Runner::image_id`]; when the
/// image is absent the job is `unsupported` and nothing builds it implicitly.
pub const SCIP_JAVA_IMAGE: &str = "rb-scip-java:0.13.1";
/// The scip-java image on JDK 25 (contract M9d D12): built locally from
/// `docker/scip-java/Dockerfile.jdk25`, with the same Maven, Gradle and scip-java files as
/// [`SCIP_JAVA_IMAGE`]. A Maven job whose poms declare a release above 21 runs on it.
pub const SCIP_JAVA25_IMAGE: &str = "rb-scip-java:0.13.1-jdk25";

/// The JDK of the scip-java image a job runs on (contract M9d D12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Jdk {
    Jdk21,
    Jdk25,
}

impl Jdk {
    /// The image's reference.
    pub fn image(self) -> &'static str {
        match self {
            Jdk::Jdk21 => SCIP_JAVA_IMAGE,
            Jdk::Jdk25 => SCIP_JAVA25_IMAGE,
        }
    }
}

/// The JDK a scip-java job needs (contract M9d D12): JDK 25 for a Maven job whose release is
/// above 21, JDK 21 otherwise (a Gradle job has no release). `Err` with the release when it is
/// above 25, which neither image compiles.
pub fn jdk(job: &Job) -> Result<Jdk, u32> {
    match job.java_release {
        Some(release) if release > 25 => Err(release),
        Some(release) if release > 21 => Ok(Jdk::Jdk25),
        _ => Ok(Jdk::Jdk21),
    }
}
/// The scip-go image: built locally from `docker/scip-go` (no published image
/// is suitable). Like scip-java, a local image has no registry digest, so the
/// cache key uses its image ID, resolved at run time; when absent the job is
/// `unsupported` and nothing builds it implicitly. scip-go v0.2.7 with
/// golang.org/x/tools raised to v0.50.0, the first that reads the export data
/// Go 1.27.2 writes (version 5); `rb-scip-go:0.2.7` loaded no imported package
/// (contract M9c amendment 2).
pub const SCIP_GO_IMAGE: &str = "rb-scip-go:0.2.7-xtools0.50.0";
/// scip-python v0.6.6 is published for linux/amd64 only; on arm64 hosts it
/// runs emulated (like scip-typescript). Pinned by digest after measuring
/// against the disk budget (contract M6d S2).
pub const SCIP_PYTHON_IMAGE: &str = "sourcegraph/scip-python@sha256:1847528ce710f58d13200e8c0237f8028342eee3f8a073a9f6109107f930be23";
/// scip-typescript 0.4.0 is published for linux/amd64 only; on arm64 hosts it runs emulated.
pub const SCIP_TYPESCRIPT_IMAGE: &str = "sourcegraph/scip-typescript@sha256:028c00429913fde8ed684ae9e9f175f0e66484398c7c9f6de51bf06d32223845";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Indexer {
    ScipJava,
    ScipTypescript,
    ScipGo,
    ScipPython,
}

impl Indexer {
    pub fn as_str(self) -> &'static str {
        match self {
            Indexer::ScipJava => "scip-java",
            Indexer::ScipTypescript => "scip-typescript",
            Indexer::ScipGo => "scip-go",
            Indexer::ScipPython => "scip-python",
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
    /// `go mod download`: reproducible only with a `go.sum` alongside (else
    /// the job is `unpinned`, as with an unlockfiled npm install).
    Go,
    /// Python environment resolution (contract M6d S4): lockfile-driven
    /// installs only — the locked tree for a `poetry.lock`, pins without
    /// transitive dependencies for a `requirements.txt`, the project itself
    /// without dependencies otherwise (a `uv.lock` has no installer in the
    /// image, so it takes the project-only path; S6 reads its versions).
    /// Reproducible only with a `poetry.lock` alongside.
    Python { lockfile: PythonLock },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NodeLock {
    PackageLock,
    Yarn,
    Pnpm,
    /// No lockfile: the fetch is `npm install`, which is not reproducible; recorded on the job.
    None,
}

/// Which lockfile a Python job's fetch installs from (contract M6d S4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum PythonLock {
    /// A `poetry.lock`: `poetry install` of the locked tree, reproducible.
    Poetry,
    /// A `requirements.txt`: `pip install --no-deps`, pins only.
    Requirements,
    /// Anything else (including a `uv.lock`, whose installer is not in the
    /// image): `pip install --no-deps -e .`, the project only.
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
    /// scip-go (contract M9d D5): the directories under `root` (relative to it, sorted) of the
    /// other `go.mod` files whose module paths the root's `go.mod` requires without a
    /// `replace`. Non-empty: rb writes a `go.work` that uses `.` and each of them, and both
    /// phases run in workspace mode. Empty for every other job.
    pub workspace: Vec<String>,
    /// scip-go (contract M9d D6): the root's `go.mod` `replace` targets that are local
    /// directories outside `root`, as written (`../../server/public`), sorted and distinct.
    /// `projects::job_files` resolves them against the root: one inside the repository is
    /// exported, one leaving it makes the job `unsupported`. Non-empty: the export is
    /// repository-shaped and the container starts in the root. Empty for every other job.
    pub replace_targets: Vec<String>,
    /// scip-java Maven (contract M9d D12): the highest Java release the job's poms declare,
    /// which picks its image. `None` for a Maven job that declares none, and for every other
    /// job (a Gradle job always runs on the JDK 21 image).
    pub java_release: Option<u32>,
}

/// A workspace root whose members are not grouped into its job (contract M6a D5), and why:
/// its packages keep the jobs they had before M6a.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ungrouped {
    pub root: String,
    /// `workspace_glob_unsupported`, `workspace_without_lockfile`, `yarn_berry` or
    /// `manifest_unreadable` (a `package.json` that is not JSON, or a `workspaces` value of
    /// another shape).
    pub reason: &'static str,
}

/// A commit's jobs, the workspace members grouped into them and the workspace roots left
/// ungrouped.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    pub jobs: Vec<Job>,
    /// Per grouped scip-typescript job root: its workspace members (contract M6a D5), package
    /// roots relative to it, sorted.
    pub members: BTreeMap<String, Vec<String>>,
    pub ungrouped: Vec<Ungrouped>,
}

/// The jobs for a commit, from its sorted file list; `read` gives a file's bytes (`None` when
/// it cannot be read) and is asked only for `package.json` files, workspace roots'
/// `yarn.lock`, Go roots' `go.mod` (contract M9d D7) and the `pom.xml` files under Maven roots
/// (D12). Maven and Gradle roots are the outermost `pom.xml` / Gradle build files (a
/// reactor or multi-project build runs once). TypeScript roots are package roots (a directory
/// with `package.json`), each indexing every `tsconfig.json` directory whose nearest package
/// root it is; a nested package root starts its own job, except a workspace member (contract
/// M6a D5): a package root whose `package.json` names it in `workspaces` through the allowed
/// glob forms, under a root holding `package-lock.json` or a Yarn v1 `yarn.lock`, belongs to
/// that root's job, which installs and indexes the whole workspace.
pub fn jobs(files: &[&str], read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Plan {
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
        // Contract M9d D12: a Maven job's release picks its image; a Gradle job has none.
        let java_release = if fetch == Fetch::Maven {
            maven_release(&root, &files, read)
        } else {
            None
        };
        out.push(Job {
            root,
            indexer: Indexer::ScipJava,
            fetch,
            projects: Vec::new(),
            unpinned: false,
            workspace: Vec::new(),
            replace_targets: Vec::new(),
            java_release,
        });
    }

    // TypeScript: package roots, workspaces grouped (D5), the nearest root owning each tsconfig.
    let mut package_roots: Vec<String> = dirs_with("package.json");
    package_roots.sort();
    package_roots.dedup();
    let Workspaces {
        groups,
        member_of,
        ungrouped,
    } = workspaces(&package_roots, &has, read);
    let roots: BTreeSet<&str> = package_roots.iter().map(String::as_str).collect();
    let mut projects: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for t in dirs_with("tsconfig.json") {
        let Some(nearest) = nearest_root(&roots, &t) else {
            continue;
        };
        let owner = member_of.get(nearest).copied().unwrap_or(nearest);
        projects.entry(owner).or_default().push(relative(&t, owner));
    }
    let mut members = BTreeMap::new();
    for (root, mut projects) in projects {
        projects.sort();
        let lockfile = match groups.get(root) {
            Some((lockfile, grouped)) => {
                members.insert(root.to_owned(), grouped.clone());
                *lockfile
            }
            None => node_lock(root, &has),
        };
        out.push(Job {
            root: root.to_owned(),
            indexer: Indexer::ScipTypescript,
            fetch: Fetch::Npm { lockfile },
            projects,
            unpinned: lockfile == NodeLock::None,
            workspace: Vec::new(),
            replace_targets: Vec::new(),
            java_release: None,
        });
    }
    // Go: every go.mod directory is a job. Contract M9d D7: each root's go.mod is read, so its
    // workspace (D5) and its replace targets outside the root (D6) are decided here, and the
    // input hash covers them through the exported go.mod files. A root holding its own
    // go.work is left as it is (out of scope); an unreadable go.mod plans as before.
    let mut go_roots = dirs_with("go.mod");
    go_roots.sort();
    go_roots.dedup();
    let go_mods: BTreeMap<&str, GoMod> = go_roots
        .iter()
        .filter_map(|root| {
            let text = read(&join(root, "go.mod"))?;
            Some((root.as_str(), go_mod(&String::from_utf8_lossy(&text))))
        })
        .collect();
    for root in &go_roots {
        let (workspace, replace_targets) = match go_mods.get(root.as_str()) {
            Some(module) if has(&join(root, "go.work")) => {
                (Vec::new(), go_replace_targets(root, module))
            }
            Some(module) => (
                go_workspace(root, module, &go_mods),
                go_replace_targets(root, module),
            ),
            None => (Vec::new(), Vec::new()),
        };
        out.push(Job {
            root: root.clone(),
            indexer: Indexer::ScipGo,
            fetch: Fetch::Go,
            projects: Vec::new(),
            unpinned: !has(&join(root, "go.sum")),
            workspace,
            replace_targets,
            java_release: None,
        });
    }
    // Python: every pyproject.toml/setup.py directory is a job (contract M6d
    // S2); the fetch installs from its lockfile (S4). So is a directory with a
    // requirements.txt and neither (contract M9d D2), which installs its pins.
    let requirements_only: BTreeSet<String> = dirs_with("requirements.txt")
        .into_iter()
        .filter(|r| !has(&join(r, "pyproject.toml")) && !has(&join(r, "setup.py")))
        .collect();
    let mut python_roots: Vec<String> = dirs_with("pyproject.toml")
        .into_iter()
        .chain(dirs_with("setup.py"))
        .chain(requirements_only.iter().cloned())
        .collect();
    python_roots.sort();
    python_roots.dedup();
    for root in python_roots {
        // The strongest pin wins: a poetry.lock pins the whole tree, a
        // requirements.txt its pins only, anything else the project alone.
        // Poetry needs a pyproject.toml, so a requirements-only root installs
        // its pins whatever else it holds.
        let lockfile = if requirements_only.contains(&root) {
            PythonLock::Requirements
        } else if has(&join(&root, "poetry.lock")) {
            PythonLock::Poetry
        } else if has(&join(&root, "requirements.txt")) {
            PythonLock::Requirements
        } else {
            PythonLock::None
        };
        out.push(Job {
            root: root.clone(),
            indexer: Indexer::ScipPython,
            fetch: Fetch::Python { lockfile },
            projects: Vec::new(),
            unpinned: lockfile != PythonLock::Poetry,
            workspace: Vec::new(),
            replace_targets: Vec::new(),
            java_release: None,
        });
    }
    out.sort_by(|a, b| (a.root.as_str(), a.indexer).cmp(&(b.root.as_str(), b.indexer)));
    Plan {
        jobs: out,
        members,
        ungrouped,
    }
}

/// The lockfile a package root's own install uses.
fn node_lock(root: &str, has: &dyn Fn(&str) -> bool) -> NodeLock {
    if has(&join(root, "package-lock.json")) {
        NodeLock::PackageLock
    } else if has(&join(root, "yarn.lock")) {
        NodeLock::Yarn
    } else if has(&join(root, "pnpm-lock.yaml")) {
        NodeLock::Pnpm
    } else {
        NodeLock::None
    }
}

fn join(root: &str, name: &str) -> String {
    if root.is_empty() {
        name.to_owned()
    } else {
        format!("{root}/{name}")
    }
}

/// The workspace grouping of a commit's package roots (contract M6a D5).
struct Workspaces<'a> {
    /// Per grouped workspace root: its lockfile and its members relative to it, sorted.
    groups: BTreeMap<&'a str, (NodeLock, Vec<String>)>,
    /// Each member's workspace root.
    member_of: BTreeMap<&'a str, &'a str>,
    ungrouped: Vec<Ungrouped>,
}

/// Decides membership from the root manifests alone. Roots are visited parents first (sorted
/// paths), so an outer workspace claims a nested package root before the nested root is read:
/// a member's own `workspaces` are not read (npm does not nest workspaces), and a nested root
/// the outer patterns do not match is a root of its own.
fn workspaces<'a>(
    package_roots: &'a [String],
    has: &dyn Fn(&str) -> bool,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Workspaces<'a> {
    let mut out = Workspaces {
        groups: BTreeMap::new(),
        member_of: BTreeMap::new(),
        ungrouped: Vec::new(),
    };
    for (k, root) in package_roots.iter().enumerate() {
        if out.member_of.contains_key(root.as_str()) {
            continue;
        }
        let mut ungrouped = |reason: &'static str| {
            out.ungrouped.push(Ungrouped {
                root: root.clone(),
                reason,
            });
        };
        let patterns = match read(&join(root, "package.json"))
            .as_deref()
            .map(workspace_patterns)
        {
            Some(Ok(Some(patterns))) => patterns,
            // pnpm workspaces declare members in `pnpm-workspace.yaml`
            // (contract M6e S6); a root with neither is a plain root.
            Some(Ok(None)) => match read(&join(root, "pnpm-workspace.yaml"))
                .as_deref()
                .map(pnpm_workspace_patterns)
            {
                Some(Some(patterns)) => patterns,
                _ => continue,
            },
            Some(Err(())) | None => {
                ungrouped("manifest_unreadable");
                continue;
            }
        };
        let lockfile = if has(&join(root, "package-lock.json")) {
            NodeLock::PackageLock
        } else if has(&join(root, "yarn.lock")) {
            let berry = read(&join(root, "yarn.lock")).is_none_or(|text| is_yarn_berry(&text));
            if berry {
                ungrouped("yarn_berry");
                continue;
            }
            NodeLock::Yarn
        } else if has(&join(root, "pnpm-lock.yaml")) {
            NodeLock::Pnpm
        } else {
            ungrouped("workspace_without_lockfile");
            continue;
        };
        let Some(trie) = patterns.iter().try_fold(Trie::default(), |mut trie, p| {
            trie.insert(&parse_pattern(p)?);
            Some(trie)
        }) else {
            ungrouped("workspace_glob_unsupported");
            continue;
        };
        // The package roots strictly under `root` share its string prefix (`""` for the
        // top), so they form one contiguous run in sorted order; but siblings such as
        // `mods-old` sort between the root and its members (`-` is below `/`), so the
        // scan continues while the string prefix holds and `is_under` decides per
        // candidate (a `take_while(is_under)` stops at the first such sibling and
        // shadows every later member: evaluator pass 1, Z1). Each candidate is still
        // examined once per enclosing workspace root, so the S19 bound holds.
        let mut members = Vec::new();
        for candidate in package_roots[k + 1..]
            .iter()
            .take_while(|c| root.is_empty() || c.starts_with(root.as_str()))
            .filter(|c| is_under(c, root))
        {
            tick(1);
            if out.member_of.contains_key(candidate.as_str()) {
                continue;
            }
            let rel = relative(candidate, root);
            if trie.matches(&rel.split('/').collect::<Vec<_>>()) {
                out.member_of.insert(candidate.as_str(), root.as_str());
                members.push(rel);
            }
        }
        out.groups.insert(root.as_str(), (lockfile, members));
    }
    out
}

/// `package.json`'s workspace patterns: `Ok(None)` when it declares none, `Err` when it is not
/// JSON or `workspaces` is neither an array of strings nor `{ "packages": [strings] }`.
fn workspace_patterns(bytes: &[u8]) -> Result<Option<Vec<String>>, ()> {
    let manifest: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| ())?;
    let Some(workspaces) = manifest.get("workspaces") else {
        return Ok(None);
    };
    let list = match workspaces {
        serde_json::Value::Array(list) => list,
        serde_json::Value::Object(o) => match o.get("packages") {
            Some(serde_json::Value::Array(list)) => list,
            _ => return Err(()),
        },
        _ => return Err(()),
    };
    list.iter()
        .map(|p| p.as_str().map(str::to_owned).ok_or(()))
        .collect::<Result<_, _>>()
        .map(Some)
}

/// A Yarn ≥ 2 lockfile: its first line that is neither blank nor a `#` comment starts
/// `__metadata:` (Yarn v1's lockfiles start with `# yarn lockfile v1` and entries).
/// `pub(crate)` for the fetch phase's Berry refusal (contract M6e S6).
pub(crate) fn is_yarn_berry(text: &[u8]) -> bool {
    String::from_utf8_lossy(text)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .is_some_and(|l| l.starts_with("__metadata:"))
}

/// `pnpm-workspace.yaml`'s `packages:` globs, hand-read (contract M6e S6: no YAML
/// parser; the glob allow-list below validates). `None` when the file declares
/// none (a scalar or missing `packages:` is not a workspace).
fn pnpm_workspace_patterns(bytes: &[u8]) -> Option<Vec<String>> {
    let text = String::from_utf8_lossy(bytes);
    let mut out: Option<Vec<String>> = None;
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(patterns) = out.as_mut()
                && let Some(item) = line.trim().strip_prefix("- ")
            {
                let item = item.trim().trim_matches('\'').trim_matches('"');
                if !item.is_empty() {
                    patterns.push(item.to_owned());
                }
            }
            continue;
        }
        let body = line.trim();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        if out.is_some() {
            // Any second top-level key ends the list.
            break;
        }
        if body == "packages:" {
            out = Some(Vec::new());
        }
    }
    out
}
enum Segment {
    Literal(String),
    /// `*`: exactly one directory.
    Star,
    /// `**` as the final segment: one directory or more.
    Rest,
}

/// The allowed forms only: `/`-separated literal segments of `[A-Za-z0-9._@+-]` (not `.` or
/// `..`), `*` as a whole segment, `**` as the whole final segment; a trailing `/` is ignored.
/// `None` for anything else (`!`, braces, brackets, `?`, a partial `*`, an empty segment, an
/// absolute path, an empty pattern).
fn parse_pattern(pattern: &str) -> Option<Vec<Segment>> {
    let pattern = pattern.strip_suffix('/').unwrap_or(pattern);
    let parts: Vec<&str> = pattern.split('/').collect();
    parts
        .iter()
        .enumerate()
        .map(|(i, part)| match *part {
            "*" => Some(Segment::Star),
            "**" if i + 1 == parts.len() => Some(Segment::Rest),
            "" | "." | ".." => None,
            literal
                if literal
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._@+-".contains(&b)) =>
            {
                Some(Segment::Literal(literal.to_owned()))
            }
            _ => None,
        })
        .collect()
}

/// A workspace's patterns as a trie of segments: a path is matched by walking it once,
/// following its literal child and the `*` child at each level, each node at most once (no
/// backtracking beyond the final `**`, invariant 9).
#[derive(Default, Clone)]
struct Trie {
    literal: BTreeMap<String, Trie>,
    star: Option<Box<Trie>>,
    /// A pattern ends here.
    end: bool,
    /// A pattern's final `**` starts here.
    rest: bool,
}

impl Trie {
    fn insert(&mut self, segments: &[Segment]) {
        match segments.split_first() {
            None => self.end = true,
            Some((Segment::Rest, _)) => self.rest = true,
            Some((Segment::Star, tail)) => self.star.get_or_insert_with(Box::default).insert(tail),
            Some((Segment::Literal(l), tail)) => {
                self.literal.entry(l.clone()).or_default().insert(tail);
            }
        }
    }

    fn matches(&self, path: &[&str]) -> bool {
        let mut stack = vec![(self, 0)];
        while let Some((node, i)) = stack.pop() {
            tick(1);
            if i == path.len() {
                if node.end {
                    return true;
                }
                continue;
            }
            if node.rest {
                return true;
            }
            if let Some(next) = node.literal.get(path[i]) {
                stack.push((next, i + 1));
            }
            if let Some(next) = &node.star {
                stack.push((next, i + 1));
            }
        }
        false
    }
}

/// Counts steps of the measured loops in tests (contract M6a S19); nothing outside tests.
#[cfg(test)]
pub(crate) mod work {
    thread_local! {
        static COUNT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    }

    pub fn tick(n: usize) {
        COUNT.with(|c| c.set(c.get() + n as u64));
    }

    /// The count since the last call, on this thread.
    pub fn take() -> u64 {
        COUNT.with(|c| c.replace(0))
    }
}

#[inline(always)]
fn tick(n: usize) {
    #[cfg(test)]
    work::tick(n);
    let _ = n;
}

fn dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// `dir` is strictly below `root` (`""` is the repository root).
fn is_under(dir: &str, root: &str) -> bool {
    dir != root && (root.is_empty() || dir.strip_prefix(root).is_some_and(|r| r.starts_with('/')))
}

/// The deepest package root at or above `dir`, found by walking up its directories.
fn nearest_root<'a>(roots: &BTreeSet<&'a str>, dir: &str) -> Option<&'a str> {
    let mut at = dir;
    loop {
        tick(1);
        if let Some(root) = roots.get(at) {
            return Some(root);
        }
        if at.is_empty() {
            return None;
        }
        at = at.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
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

/// What the Go planner (contract M9d D5–D7) and the `go.work` writer read from a `go.mod`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct GoMod {
    /// The `module` path.
    pub(crate) module: Option<String>,
    /// The `go` line's version.
    pub(crate) go: Option<String>,
    /// Every `require`d module path, direct or `// indirect`.
    pub(crate) requires: BTreeSet<String>,
    /// Every `replace`: the replaced module path, and the target as written (a module path,
    /// or a local directory).
    pub(crate) replaces: Vec<(String, String)>,
}

/// Reads a `go.mod`'s `module`, `go`, `require` and `replace` directives, single-line and in
/// blocks; comments and every other directive are ignored, quotes around paths dropped.
pub(crate) fn go_mod(text: &str) -> GoMod {
    let mut out = GoMod::default();
    let mut block: Option<&str> = None;
    for line in text.lines() {
        let code = line.split("//").next().unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let (directive, rest) = match block {
            Some(_) if code == ")" => {
                block = None;
                continue;
            }
            Some(directive) => (directive, code),
            None => {
                let (directive, rest) = code.split_once(char::is_whitespace).unwrap_or((code, ""));
                let rest = rest.trim();
                if rest == "(" {
                    block = Some(directive);
                    continue;
                }
                (directive, rest)
            }
        };
        let unquote = |s: &str| s.trim_matches('"').to_owned();
        let first = rest.split_whitespace().next().map(unquote);
        match directive {
            "module" => out.module = first,
            "go" => out.go = first,
            "require" => out.requires.extend(first),
            "replace" => {
                if let Some((from, to)) = rest.split_once("=>")
                    && let (Some(module), Some(target)) =
                        (from.split_whitespace().next(), to.split_whitespace().next())
                {
                    out.replaces.push((unquote(module), unquote(target)));
                }
            }
            _ => {}
        }
    }
    out
}

/// A `go` line's version as a key in Go's order: numerically by release (`1.26.10` above
/// `1.26.9`, `1.21` above `1.9`), a language version (`1.21`) below its pre-releases
/// (`1.21rc1`), which sort below its first release (`1.21.0`).
pub(crate) fn go_version_key(version: &str) -> (Vec<u64>, bool, &str) {
    let split = version
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(version.len());
    let (release, pre) = version.split_at(split);
    let numbers = release
        .split('.')
        .filter(|n| !n.is_empty())
        .map(|n| n.parse().unwrap_or(0))
        .collect();
    (numbers, !pre.is_empty(), pre)
}

/// Contract M9d D5: the directories under `root` (relative to it, sorted) of the other
/// `go.mod` files whose module path `module` requires with no `replace` of that path. A
/// directory inside `vendor` holds a vendored copy, and one Go ignores (`testdata`, a name
/// starting `.` or `_`) is no module of the build: neither is a workspace module.
fn go_workspace(root: &str, module: &GoMod, modules: &BTreeMap<&str, GoMod>) -> Vec<String> {
    let ignored = |rel: &str| {
        rel.split('/').any(|segment| {
            matches!(segment, "vendor" | "testdata")
                || segment.starts_with('.')
                || segment.starts_with('_')
        })
    };
    modules
        .iter()
        .filter(|(dir, _)| is_under(dir, root))
        .filter(|(dir, _)| !ignored(&relative(dir, root)))
        .filter(|(_, other)| {
            other.module.as_ref().is_some_and(|path| {
                let required = module.requires.contains(path);
                let replaced = module.replaces.iter().any(|(m, _)| m == path);
                required && !replaced
            })
        })
        .map(|(dir, _)| relative(dir, root))
        .collect()
}

/// Contract M9d D6: `module`'s `replace` targets that are local directories (Go's rule: `.`,
/// `..`, or a path starting `./`, `../` or `/`) and do not resolve, lexically, to `root` or
/// below it; as written, sorted and distinct. A target leaving the repository is kept, for
/// `projects::job_files` to refuse.
fn go_replace_targets(root: &str, module: &GoMod) -> Vec<String> {
    let local = |t: &str| {
        t == "." || t == ".." || t.starts_with("./") || t.starts_with("../") || t.starts_with('/')
    };
    let targets: BTreeSet<&String> = module
        .replaces
        .iter()
        .map(|(_, target)| target)
        .filter(|target| local(target) && !resolves_under(root, target))
        .collect();
    targets.into_iter().cloned().collect()
}

/// Whether the relative directory `target`, read from `root`, resolves lexically (`.` and
/// `..` folded, nothing on disk followed) to `root` or below it. An absolute path, or one
/// that leaves the repository, does not.
fn resolves_under(root: &str, target: &str) -> bool {
    if target.starts_with('/') {
        return false;
    }
    let mut at: Vec<&str> = if root.is_empty() {
        Vec::new()
    } else {
        root.split('/').collect()
    };
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if at.pop().is_none() {
                    return false;
                }
            }
            name => at.push(name),
        }
    }
    let dir = at.join("/");
    dir == root || is_under(&dir, root)
}

/// The highest Java release the `pom.xml` files under the Maven root `root` declare (contract
/// M9d D12), each read by [`pom_release`]; `None` when none declares one. A parent pom outside
/// the job (a `spring-boot-starter-parent`) is not visible: a release it derives from a
/// module's `java.version` is not seen (amendment 1, risk 4).
fn maven_release(
    root: &str,
    files: &[&str],
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Option<u32> {
    let poms: BTreeMap<&str, Pom> = files
        .iter()
        .copied()
        .filter(|f| f.rsplit('/').next() == Some("pom.xml"))
        .filter(|f| dir(f) == root || is_under(dir(f), root))
        .filter_map(|f| Some((f, Pom::read(&String::from_utf8_lossy(&read(f)?)))))
        .collect();
    poms.keys()
        .filter_map(|path| pom_release(path, &poms))
        .max()
}

/// What the release reader takes from one `pom.xml` (contract M9d D12).
#[derive(Debug, Default)]
struct Pom {
    /// The project's own `artifactId`.
    artifact_id: Option<String>,
    /// The `<parent>` element's `artifactId`, and its `relativePath`: `../pom.xml` when absent,
    /// `None` when empty (Maven then looks the parent up in the repositories only).
    parent: Option<(Option<String>, Option<String>)>,
    /// The project's `<properties>` (not a profile's), in document order.
    properties: Vec<(String, String)>,
    /// The `release`, `source` and `target` values of the compiler plugin's configurations
    /// (the plugin's own and its executions').
    compiler: Vec<String>,
}

impl Pom {
    /// Reads the elements [`Pom`] holds from `xml`, outside comments; anything else is skipped.
    fn read(xml: &str) -> Pom {
        let mut pom = Pom::default();
        let mut stack: Vec<&str> = Vec::new();
        let (mut parent, mut parent_artifact, mut relative) = (false, None, None);
        // Per open `<plugin>`: its artifactId and its configuration values.
        let mut plugins: Vec<(Option<String>, Vec<String>)> = Vec::new();
        let mut rest = xml;
        while let Some(open) = rest.find('<') {
            let text = rest[..open].trim();
            if !text.is_empty() {
                let text = text.to_owned();
                match stack.as_slice() {
                    ["project", "artifactId"] => pom.artifact_id = Some(text),
                    ["project", "parent", "artifactId"] => parent_artifact = Some(text),
                    ["project", "parent", "relativePath"] => relative = Some(Some(text)),
                    ["project", "properties", name] => {
                        pom.properties.push(((*name).to_owned(), text))
                    }
                    [.., "plugin", "artifactId"] => {
                        if let Some(plugin) = plugins.last_mut() {
                            plugin.0 = Some(text);
                        }
                    }
                    [.., "configuration", "release" | "source" | "target"] => {
                        if let Some(plugin) = plugins.last_mut() {
                            plugin.1.push(text);
                        }
                    }
                    _ => {}
                }
            }
            rest = &rest[open..];
            if let Some(comment) = rest.strip_prefix("<!--") {
                rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
                continue;
            }
            let Some(close) = rest.find('>') else {
                break;
            };
            let tag = &rest[1..close];
            rest = &rest[close + 1..];
            if tag.starts_with('?') || tag.starts_with('!') {
                continue;
            }
            if let Some(name) = tag.strip_prefix('/') {
                let name = name.trim();
                if name == "plugin"
                    && let Some((Some(artifact), values)) = plugins.pop()
                    && artifact == "maven-compiler-plugin"
                {
                    pom.compiler.extend(values);
                }
                if let Some(at) = stack.iter().rposition(|open| *open == name) {
                    stack.truncate(at);
                }
                continue;
            }
            let name = tag
                .trim_end_matches('/')
                .split_whitespace()
                .next()
                .unwrap_or("");
            let in_parent = stack.as_slice() == ["project", "parent"];
            if in_parent && name == "relativePath" {
                relative = Some(None);
            }
            if tag.ends_with('/') {
                continue;
            }
            parent |= stack.as_slice() == ["project"] && name == "parent";
            if name == "plugin" {
                plugins.push((None, Vec::new()));
            }
            stack.push(name);
        }
        if parent {
            let relative = relative.unwrap_or_else(|| Some("../pom.xml".to_owned()));
            pom.parent = Some((parent_artifact, relative));
        }
        pom
    }
}

/// The pom at `path` that the pom's `<parent>` names inside `poms`: its `relativePath` (a
/// directory gets `pom.xml`) resolved from the pom's directory, if that pom's `artifactId` is
/// the one `<parent>` names, as Maven checks before it looks in the repositories.
fn parent_pom(path: &str, pom: &Pom, poms: &BTreeMap<&str, Pom>) -> Option<String> {
    let (artifact, relative) = pom.parent.as_ref()?;
    let mut at: Vec<&str> = dir(path).split('/').filter(|s| !s.is_empty()).collect();
    for segment in relative.as_deref()?.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                at.pop()?;
            }
            name => at.push(name),
        }
    }
    let mut target = at.join("/");
    if !target.ends_with(".xml") {
        target = join(&target, "pom.xml");
    }
    let candidate = poms.get(target.as_str())?;
    (artifact.is_none() || *artifact == candidate.artifact_id).then_some(target)
}

/// One pom's release (contract M9d D12): the highest of `maven.compiler.release`, `source` and
/// `target` and the compiler plugin's `release`, `source` and `target`, read over the pom and
/// its parent chain inside the job as Maven inherits them (a child's property over its
/// ancestors', every ancestor's plugin configuration), with a whole `${name}` value resolved
/// through those properties. `java.version` counts only through them. `1.8` is release 8;
/// an unresolved or non-numeric value counts as none.
fn pom_release(path: &str, poms: &BTreeMap<&str, Pom>) -> Option<u32> {
    // The pom and its ancestors, nearest first; a cycle ends the chain.
    let mut chain: Vec<&Pom> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut at = Some(path.to_owned());
    while let Some(p) = at.take() {
        let Some(pom) = poms.get(p.as_str()) else {
            break;
        };
        if !seen.insert(p.clone()) {
            break;
        }
        chain.push(pom);
        at = parent_pom(&p, pom, poms);
    }
    let mut properties: BTreeMap<&str, &str> = BTreeMap::new();
    for pom in chain.iter().rev() {
        for (name, value) in &pom.properties {
            properties.insert(name, value);
        }
    }
    let resolve = |value: &str| -> String {
        let mut value = value.to_owned();
        // A bounded chain of `${a}` → `${b}` → value.
        for _ in 0..8 {
            let Some(name) = value.strip_prefix("${").and_then(|v| v.strip_suffix('}')) else {
                break;
            };
            match properties.get(name) {
                Some(v) => value = (*v).to_owned(),
                None => break,
            }
        }
        value
    };
    let number = |value: String| -> Option<u32> {
        let value = value.trim();
        value.strip_prefix("1.").unwrap_or(value).parse().ok()
    };
    [
        "maven.compiler.release",
        "maven.compiler.source",
        "maven.compiler.target",
    ]
    .iter()
    .filter_map(|name| properties.get(name).copied())
    .chain(
        chain
            .iter()
            .flat_map(|pom| pom.compiler.iter().map(String::as_str)),
    )
    .filter_map(|value| number(resolve(value)))
    .max()
}

/// The source encoding a Maven build declares when it is not UTF-8 (contract M3b amendment
/// 64), read from every `pom.xml` of the build: the first value of a
/// `project.build.sourceEncoding`, `maven.compiler.encoding` or `encoding` element (the last
/// also configures other plugins; reading it as the sources' encoding errs towards rejecting)
/// that is not UTF-8 after resolving `${property}` through the poms' own simple elements; an
/// unresolved value counts as not UTF-8. `None` when every declaration is UTF-8 or there is
/// none: javac then reads UTF-8 (measured with `rb-scip-java:0.13.1`). A declaration in a
/// parent pom outside the repository is not visible here (open item).
pub fn maven_source_encoding(poms: &[&[u8]]) -> Option<String> {
    let elements: Vec<(String, String)> = poms
        .iter()
        .flat_map(|pom| simple_elements(&String::from_utf8_lossy(pom)))
        .collect();
    let resolve = |value: &str| -> String {
        let mut value = value.to_owned();
        // A bounded chain of `${a}` → `${b}` → value.
        for _ in 0..8 {
            let Some(name) = value.strip_prefix("${").and_then(|v| v.strip_suffix('}')) else {
                break;
            };
            match elements.iter().find(|(n, _)| n == name) {
                Some((_, v)) => value.clone_from(v),
                None => break,
            }
        }
        value
    };
    elements
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "project.build.sourceEncoding" | "maven.compiler.encoding" | "encoding"
            )
        })
        .map(|(_, value)| resolve(value))
        .find(|value| !value.eq_ignore_ascii_case("UTF-8") && !value.eq_ignore_ascii_case("UTF8"))
}

/// The inputs to [`effective_encoding`]: a job's poms (repository path, bytes), its
/// `.mvn/maven.config` bytes when exported, and the explicit `-D` flags of the Maven
/// invocation (highest precedence; the pipeline passes no encoding `-D` flag — `java_runs`
/// passes only `-DskipTests`, Develocity and logger flags — so the slot is structurally empty
/// and unit-tested only).
pub struct EncodingInputs<'a> {
    pub poms: Vec<(&'a str, &'a [u8])>,
    pub maven_config: Option<&'a [u8]>,
    pub cli: &'a [String],
}

/// Whether `value` names UTF-8 (any spelling the readers accept).
fn is_utf8(value: &str) -> bool {
    value.eq_ignore_ascii_case("UTF-8") || value.eq_ignore_ascii_case("UTF8")
}

/// The `-encoding` value a pom's `<compilerArg>` entries declare, if any: the value after the
/// last `-encoding` entry in document order (javac's own last-wins; a dangling `-encoding`
/// declares nothing).
fn compiler_arg_encoding(pom: &str) -> Option<String> {
    let args: Vec<String> = simple_elements(pom)
        .into_iter()
        .filter(|(name, _)| name == "compilerArg")
        .map(|(_, value)| value)
        .collect();
    args.iter()
        .rposition(|a| a == "-encoding")
        .and_then(|i| args.get(i + 1).cloned())
}

/// A `-Dname=value` flag naming one of the encoding properties, if `token` is one: the value.
/// Both `-Dname=value` and `-D name=value` (two tokens) forms; anything else names nothing.
fn d_flag<'a>(token: &'a str, next: Option<&'a str>) -> Option<String> {
    const NAMES: [&str; 3] = [
        "encoding",
        "project.build.sourceEncoding",
        "maven.compiler.encoding",
    ];
    let assignment = token
        .strip_prefix("-D")
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            if token == "-D" {
                next.map(str::to_owned)
            } else {
                None
            }
        })?;
    let (name, value) = assignment.split_once('=')?;
    NAMES.contains(&name).then(|| value.to_owned())
}

/// The encoding the last `-D` flag of `tokens` (whitespace-split) declares, if any.
fn d_flag_encoding(tokens: &[&str]) -> Option<String> {
    let mut out = None;
    for (i, token) in tokens.iter().enumerate() {
        if let Some(value) = d_flag(token, tokens.get(i + 1).copied()) {
            out = Some(value);
        }
    }
    out
}

/// The encoding elements one pom declares, `${property}` resolved through its own simple
/// elements: the first declaration in document order, UTF-8 included (`None` only when the
/// pom declares nothing). The resolver is `maven_source_encoding`'s, scoped to this pom.
fn pom_declaration(pom: &[u8]) -> Option<String> {
    let elements: Vec<(String, String)> = simple_elements(&String::from_utf8_lossy(pom));
    let resolve = |value: &str| -> String {
        let mut value = value.to_owned();
        // A bounded chain of `${a}` → `${b}` → value.
        for _ in 0..8 {
            let Some(name) = value.strip_prefix("${").and_then(|v| v.strip_suffix('}')) else {
                break;
            };
            match elements.iter().find(|(n, _)| n == name) {
                Some((_, v)) => value.clone_from(v),
                None => break,
            }
        }
        value
    };
    elements
        .iter()
        .filter(|(name, _)| {
            matches!(
                name.as_str(),
                "project.build.sourceEncoding" | "maven.compiler.encoding" | "encoding"
            )
        })
        .map(|(_, value)| resolve(value))
        .next()
}

/// The javac source encoding one document reads (contract M6e S11; D9's compilerArgs
/// position corrected by measurement): the highest precedence slot that declares one wins —
/// the document's own module first (the nearest pom at or above it; a module's declaration
/// over its ancestors', as Maven resolves), then the build-wide `.mvn/maven.config` `-D`,
/// then explicit `-D` flags; `compilerArgs -encoding` reads lowest. Measured on
/// `rb-scip-java:0.13.1` with a 0xE9 source (an unmappable-character error names the
/// effective encoding): the pom literal wins both directions over `compilerArgs`, while
/// `.mvn/maven.config` beats the pom and explicit `-D` beats the config file. `None` when no
/// slot declares, or the winning declaration is UTF-8: javac then reads UTF-8 (measured).
/// Anything unreadable (a dangling `-encoding`, garbage config lines, a flag without `=`, a
/// non-UTF-8 config file) declares nothing and keeps the higher slot's reading. Per-document
/// so a mixed build (an ISO root with a UTF-8 module) rejects exactly the ISO-read files
/// instead of trusting either side for all.
pub fn effective_encoding(doc: &str, inputs: &EncodingInputs) -> Option<String> {
    let dir = doc.rsplit_once('/').map_or("", |(d, _)| d);
    // The pom chain above the document, nearest first.
    let mut chain: Vec<(&str, &[u8])> = inputs
        .poms
        .iter()
        .filter(|(path, _)| {
            let pom_dir = path.rsplit_once('/').map_or("", |(d, _)| d);
            pom_dir.is_empty() || dir == pom_dir || dir.starts_with(&format!("{pom_dir}/"))
        })
        .map(|(path, bytes)| (*path, *bytes))
        .collect();
    chain.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
    let pom = chain.iter().find_map(|(_, bytes)| pom_declaration(bytes));
    let arg = chain
        .iter()
        .find_map(|(_, bytes)| compiler_arg_encoding(&String::from_utf8_lossy(bytes)));
    let config = inputs
        .maven_config
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|text| d_flag_encoding(&text.split_whitespace().collect::<Vec<_>>()));
    let cli = d_flag_encoding(&inputs.cli.iter().map(String::as_str).collect::<Vec<_>>());
    cli.or(config).or(pom).or(arg).filter(|v| !is_utf8(v))
}

/// `<name>text</name>` elements whose content holds no markup, outside comments, with the
/// content trimmed.
fn simple_elements(xml: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        let Some(close) = rest.find('>') else {
            break;
        };
        let name = &rest[1..close];
        rest = &rest[close + 1..];
        if name.is_empty() || name.contains(|c: char| c.is_whitespace() || "/?!".contains(c)) {
            continue;
        }
        let content = &rest[..rest.find('<').unwrap_or(rest.len())];
        if rest[content.len()..].starts_with(&format!("</{name}>")) {
            out.push((name.to_owned(), content.trim().to_owned()));
        }
    }
    out
}

/// A tsconfig scoping key: absent, known (compiled), or present but unusable (contract M6e
/// S10). An unknown key makes membership undecidable (fail-open), never empty.
#[derive(Debug, Clone)]
enum Field<T> {
    Absent,
    Known(T),
    Unknown,
}

/// One resolved `tsconfig*.json` scope: its directory and its `include`/`exclude` glob tries
/// and `files` set. `extends` is already resolved (the child's keys replace the base's).
struct TsScope {
    dir: String,
    include: Field<Trie>,
    exclude: Field<Trie>,
    files: Field<BTreeSet<String>>,
}

/// The `tsconfig*.json` scopes of a commit (contract M6e S10, D8): `include`/`exclude`/`files`
/// (+ `extends`, bounded by depth 8 with a cycle guard) read through the workspace glob
/// allow-list ([`parse_pattern`]); anything beyond it stays unread and counted in [`unread`].
/// Membership only explains `files_without_document` — the indexer owns inclusion, so no edge
/// may move because of this reader.
pub struct TsScopes {
    scopes: BTreeMap<String, TsScope>,
    /// Unread patterns, entries, files and links per tsconfig path.
    unread_by_scope: BTreeMap<String, usize>,
    /// Patterns, entries, files and links beyond the allow-list: each counted once.
    pub unread: usize,
}

/// `text` with its `//` and `/* */` comments removed (string-aware; JSON itself has only
/// `"` strings, which is all this strips around).
pub fn strip_json_comments(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut start = 0;
    let mut quote = false;
    let mut escaped = false;
    while i < bytes.len() {
        let b = bytes[i];
        if escaped {
            escaped = false;
            i += 1;
            continue;
        }
        if quote {
            if b == b'\\' {
                escaped = true;
            } else if b == b'"' {
                quote = false;
            }
            i += 1;
            continue;
        }
        match b {
            b'"' => {
                quote = true;
                i += 1;
            }
            b'/' if bytes.get(i + 1) == Some(&b'/') => {
                out.push_str(&text[start..i]);
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                start = i;
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                out.push_str(&text[start..i]);
                i += 2;
                while i < bytes.len() && !(bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
                start = i;
            }
            _ => {
                i += 1;
            }
        }
    }
    out.push_str(&text[start..]);
    out
}

/// `text` read as JSONC, as `tsc` reads a `tsconfig.json` (contract M9c D1): a leading UTF-8
/// byte order mark dropped (amendment 3, E4), [`strip_json_comments`], then each `,` outside
/// strings followed only by whitespace before `}` or `]` is dropped, then strict JSON. `None`
/// when that is not JSON.
pub fn parse_jsonc(text: &str) -> Option<serde_json::Value> {
    let text = strip_json_comments(text.strip_prefix('\u{feff}').unwrap_or(text));
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut start = 0;
    let mut quote = false;
    let mut escaped = false;
    for (i, &b) in bytes.iter().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if quote {
            match b {
                b'\\' => escaped = true,
                b'"' => quote = false,
                _ => {}
            }
            continue;
        }
        match b {
            b'"' => quote = true,
            b',' if bytes[i + 1..]
                .iter()
                .find(|c| !c.is_ascii_whitespace())
                .is_some_and(|&c| c == b'}' || c == b']') =>
            {
                out.push_str(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push_str(&text[start..]);
    serde_json::from_str(&out).ok()
}

/// A `files` entry as a path relative to its tsconfig directory: one leading `./` ignored;
/// `None` for absolute paths, backslashes, empty segments and escapes above the directory.
fn normalise_rel(entry: &str) -> Option<String> {
    let entry = entry.strip_prefix("./").unwrap_or(entry);
    if entry.is_empty() || entry.starts_with('/') || entry.contains('\\') {
        return None;
    }
    let mut parts = Vec::new();
    for seg in entry.split('/') {
        match seg {
            "" | "." => return None,
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// An `extends` target joined to its tsconfig's directory, lexically (`None` for absolute
/// paths, backslashes and escapes above the repository root).
fn join_tsconfig(dir: &str, target: &str) -> Option<String> {
    if target.is_empty() || target.starts_with('/') || target.contains('\\') {
        return None;
    }
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

impl TsScopes {
    /// Builds the scopes from `(path, text)` pairs (`tsconfig*.json` entries only; everything
    /// else ignored). Never fails: an unreadable file contributes no scope.
    pub fn build<'a>(files: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        let texts: BTreeMap<&str, &str> = files.into_iter().collect();
        let mut out = Self {
            scopes: BTreeMap::new(),
            unread_by_scope: BTreeMap::new(),
            unread: 0,
        };
        let mut paths: Vec<&str> = texts
            .keys()
            .filter(|p| {
                p.rsplit('/')
                    .next()
                    .is_some_and(|n| n.starts_with("tsconfig."))
            })
            .copied()
            .collect();
        paths.sort();
        for path in paths {
            let mut visited = Vec::new();
            match Self::resolve(path, &texts, &mut visited, 0) {
                Some((scope, unread)) => {
                    out.unread += unread;
                    out.unread_by_scope.insert(path.to_owned(), unread);
                    out.scopes.insert(path.to_owned(), scope);
                }
                None => {
                    out.unread += 1;
                    out.unread_by_scope.insert(path.to_owned(), 1);
                }
            }
        }
        out
    }

    /// The unread count of the scopes at or below `root` (`""`: every scope): the project
    /// detail for `files_without_document` partitioning (contract M6e S10).
    pub fn unread_under(&self, root: &str) -> usize {
        self.unread_by_scope
            .iter()
            .filter(|(path, _)| {
                let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
                root.is_empty() || dir == root || dir.starts_with(&format!("{root}/"))
            })
            .map(|(_, n)| n)
            .sum()
    }

    /// The scope at `path` with its `extends` chain resolved (child keys replace the base's),
    /// plus the unread count. `None` when the file itself does not parse.
    fn resolve(
        path: &str,
        texts: &BTreeMap<&str, &str>,
        visited: &mut Vec<String>,
        depth: usize,
    ) -> Option<(TsScope, usize)> {
        if depth > 8 || visited.iter().any(|v| v == path) {
            return None;
        }
        let text = texts.get(path)?;
        let json: serde_json::Value = serde_json::from_str(&strip_json_comments(text)).ok()?;
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d).to_owned();
        let mut unread = 0;
        let mut base: Option<TsScope> = None;
        let mut broken = false;
        if let Some(extends) = json.get("extends") {
            match extends.as_str() {
                Some(target) => {
                    let base_path = join_tsconfig(&dir, target);
                    match base_path.filter(|p| texts.contains_key(p.as_str())) {
                        Some(found) => {
                            visited.push(path.to_owned());
                            match Self::resolve(&found, texts, visited, depth + 1) {
                                Some((scope, u)) => {
                                    unread += u;
                                    base = Some(scope);
                                }
                                None => {
                                    unread += 1;
                                    broken = true;
                                }
                            }
                            visited.pop();
                        }
                        None => {
                            unread += 1;
                            broken = true;
                        }
                    }
                }
                // A present non-string `extends` is ignored (the scope stands alone), counted.
                None => unread += 1,
            }
        }
        let include = Self::glob_field(
            json.get("include"),
            base.as_ref().map(|b| &b.include),
            broken,
            &mut unread,
        );
        let exclude = Self::glob_field(
            json.get("exclude"),
            base.as_ref().map(|b| &b.exclude),
            broken,
            &mut unread,
        );
        let files = Self::files_field(
            json.get("files"),
            base.as_ref().map(|b| &b.files),
            broken,
            &mut unread,
        );
        Some((
            TsScope {
                dir,
                include,
                exclude,
                files,
            },
            unread,
        ))
    }

    /// One glob key (`include`/`exclude`): the child's array when present (non-string entries
    /// skipped and counted; present but with no usable entry left means unknown), else the
    /// base's, else absent — or unknown when the chain broke before naming one.
    fn glob_field(
        own: Option<&serde_json::Value>,
        base: Option<&Field<Trie>>,
        broken: bool,
        unread: &mut usize,
    ) -> Field<Trie> {
        match own {
            None => match (base, broken) {
                (Some(field), _) => field.clone(),
                (None, true) => Field::Unknown,
                (None, false) => Field::Absent,
            },
            Some(value) => {
                let Some(items) = value.as_array() else {
                    *unread += 1;
                    return Field::Unknown;
                };
                let mut trie = Trie::default();
                let mut usable = 0;
                for item in items {
                    match item
                        .as_str()
                        .map(|s| s.strip_prefix("./").unwrap_or(s))
                        .and_then(parse_pattern)
                    {
                        Some(mut segments) => {
                            // A wildcard-free pattern names a directory: it covers the
                            // whole subtree (`"src"` ≡ `"src/**/*"`, TypeScript's
                            // rule) as well as the path itself (an exact file).
                            // Inserted in both forms here, so the workspace globs
                            // sharing the trie keep their own meaning.
                            if segments.iter().all(|s| matches!(s, Segment::Literal(_))) {
                                trie.insert(&segments);
                                segments.push(Segment::Rest);
                            }
                            trie.insert(&segments);
                            usable += 1;
                        }
                        None => *unread += 1,
                    }
                }
                if usable == 0 && !items.is_empty() {
                    Field::Unknown
                } else {
                    Field::Known(trie)
                }
            }
        }
    }

    /// One `files` key, same shape with exact relative paths instead of globs.
    fn files_field(
        own: Option<&serde_json::Value>,
        base: Option<&Field<BTreeSet<String>>>,
        broken: bool,
        unread: &mut usize,
    ) -> Field<BTreeSet<String>> {
        match own {
            None => match (base, broken) {
                (Some(field), _) => field.clone(),
                (None, true) => Field::Unknown,
                (None, false) => Field::Absent,
            },
            Some(value) => {
                let Some(items) = value.as_array() else {
                    *unread += 1;
                    return Field::Unknown;
                };
                let mut set = BTreeSet::new();
                let mut usable = 0;
                for item in items {
                    match item.as_str().and_then(normalise_rel) {
                        Some(path) => {
                            set.insert(path);
                            usable += 1;
                        }
                        None => *unread += 1,
                    }
                }
                if usable == 0 && !items.is_empty() {
                    Field::Unknown
                } else {
                    Field::Known(set)
                }
            }
        }
    }

    /// Whether `file` (a repository path) is positively outside its governing tsconfig's
    /// scope: false when no tsconfig governs it or its membership is unreadable (fail-open:
    /// the file stays in the unexplained bucket). The governing scope is the nearest
    /// directory at or above the file holding a tsconfig (sorted-first path on ties).
    pub fn out_of_scope(&self, file: &str) -> bool {
        let mut best: Option<(&str, &TsScope)> = None;
        for (path, scope) in &self.scopes {
            tick(1);
            let dir = scope.dir.as_str();
            if !(dir.is_empty() || file.strip_prefix(dir).is_some_and(|r| r.starts_with('/'))) {
                continue;
            }
            let nearer = match &best {
                None => true,
                Some((p, s)) => {
                    dir.len() > s.dir.len() || (dir.len() == s.dir.len() && path.as_str() < *p)
                }
            };
            if nearer {
                best = Some((path.as_str(), scope));
            }
        }
        let Some((_, scope)) = best else {
            return false;
        };
        let rel = if scope.dir.is_empty() {
            file
        } else {
            &file[scope.dir.len() + 1..]
        };
        let segments: Vec<&str> = rel.split('/').collect();
        match &scope.files {
            Field::Known(set) => !set.contains(rel),
            Field::Unknown => false,
            Field::Absent => {
                let matched = match &scope.include {
                    Field::Known(trie) => trie.matches(&segments),
                    Field::Unknown => return false,
                    Field::Absent => true,
                };
                if !matched {
                    return true;
                }
                match &scope.exclude {
                    Field::Known(trie) => trie.matches(&segments),
                    Field::Unknown => false,
                    Field::Absent => false,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contract M9c amendment 2 (PI): the scip-go tag names what its Dockerfile pins, scip-go's
    /// version and the raised x/tools (the first that reads Go 1.27.2's export data), so a
    /// Dockerfile change that keeps the old tag fails here. The build-time self-test stays in
    /// the Dockerfile, and both READMEs build the tag `rb index` runs.
    #[test]
    fn scip_go_image_tag_names_the_dockerfile_pins() {
        let dockerfile = include_str!("../../../docker/scip-go/Dockerfile");
        let arg = |name: &str| {
            let prefix = format!("ARG {name}=");
            dockerfile
                .lines()
                .find_map(|l| l.strip_prefix(prefix.as_str()))
                .unwrap_or_else(|| panic!("no {prefix} in the Dockerfile"))
        };
        let xtools = arg("XTOOLS_VERSION");
        assert_eq!(xtools, "v0.50.0");
        let tag = format!(
            "rb-scip-go:{}-xtools{}",
            arg("SCIP_GO_VERSION"),
            xtools.trim_start_matches('v')
        );
        assert_eq!(SCIP_GO_IMAGE, tag);
        assert!(
            dockerfile.contains("grep -aq ' os/Getenv()[.]' index.scip"),
            "the build-time self-test is gone"
        );
        let build = format!("docker build -t {SCIP_GO_IMAGE} docker/scip-go");
        for readme in [
            include_str!("../../../docker/scip-go/README.md"),
            include_str!("../../../README.md"),
        ] {
            assert!(readme.contains(&build), "{build}");
        }
    }

    fn summary(files: &[&str]) -> Vec<String> {
        workspace_summary(files, &[]).0
    }

    /// The jobs of `files` (as [`summary`] prints them, plus `{members}` for a grouped root)
    /// and the ungrouped roots, with `contents` as the files' text; a `package.json` not given
    /// reads `{}`, any other file not given cannot be read.
    fn workspace_summary(files: &[&str], contents: &[(&str, &str)]) -> (Vec<String>, Vec<String>) {
        let read = |path: &str| -> Option<Vec<u8>> {
            contents
                .iter()
                .find(|(p, _)| *p == path)
                .map(|(_, text)| text.as_bytes().to_vec())
                .or_else(|| path.ends_with("package.json").then(|| b"{}".to_vec()))
        };
        let plan = jobs(files, &read);
        let members = plan.members.clone();
        let ungrouped = plan
            .ungrouped
            .iter()
            .map(|u| format!("{} {}", u.root, u.reason))
            .collect();
        let jobs = plan
            .jobs
            .into_iter()
            .map(|j| {
                let members = members
                    .get(&j.root)
                    .map(|m| m.join(","))
                    .unwrap_or_default();
                let fetch = match j.fetch {
                    Fetch::Maven => "maven".to_owned(),
                    Fetch::Gradle => "gradle".to_owned(),
                    Fetch::Go => "go".to_owned(),
                    Fetch::Python { lockfile } => format!("python({lockfile:?})"),
                    Fetch::Npm { lockfile } => format!("npm({lockfile:?})"),
                };
                let mut s = format!("{} {} {fetch}", j.indexer.as_str(), j.root);
                if !j.projects.is_empty() {
                    s.push_str(&format!(" [{}]", j.projects.join(",")));
                }
                if j.unpinned {
                    s.push_str(" unpinned");
                }
                if !members.is_empty() {
                    s.push_str(&format!(" {{{members}}}"));
                }
                if !j.workspace.is_empty() {
                    s.push_str(&format!(" work[{}]", j.workspace.join(",")));
                }
                if !j.replace_targets.is_empty() {
                    s.push_str(&format!(" replace[{}]", j.replace_targets.join(",")));
                }
                if let Some(release) = j.java_release {
                    s.push_str(&format!(" release[{release}]"));
                }
                s
            })
            .collect();
        (jobs, ungrouped)
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

    /// Contract M9d D12 (C11): a Maven job's release is the highest its poms declare, read from
    /// `maven.compiler.release`, `source` or `target` and the compiler plugin's configuration,
    /// `${…}` resolved through the pom's properties and its parent chain inside the job;
    /// `java.version` counts only through them. Above 21 the job takes the JDK 25 image; above
    /// 25, or with that image absent, it is unsupported. A Gradle job stays on JDK 21.
    #[test]
    fn maven_release_above_21_takes_the_jdk25_image() {
        let pom =
            |body: &str| format!("<project><modelVersion>4.0.0</modelVersion>{body}</project>");
        let app = |properties: &str| {
            pom(&format!(
                "<artifactId>app</artifactId><properties>{properties}</properties>"
            ))
        };
        let plan = |contents: &[(&str, &str)]| -> Vec<String> {
            let files: Vec<&str> = contents.iter().map(|(path, _)| *path).collect();
            workspace_summary(&files, contents).0
        };
        let release = |n: &str| format!("<maven.compiler.release>{n}</maven.compiler.release>");
        // A literal; `${java.version}` through the pom's own properties; `java.version` alone.
        assert_eq!(
            plan(&[("pom.xml", &app(&release("25")))]),
            ["scip-java  maven release[25]"]
        );
        assert_eq!(
            plan(&[(
                "pom.xml",
                &app(&format!(
                    "<java.version>25</java.version>{}",
                    release("${java.version}")
                ))
            )]),
            ["scip-java  maven release[25]"]
        );
        assert_eq!(
            plan(&[("pom.xml", &app("<java.version>25</java.version>"))]),
            ["scip-java  maven"]
        );
        // `source` and `target`, `1.8` being release 8: the highest counts.
        assert_eq!(
            plan(&[(
                "pom.xml",
                &app("<maven.compiler.source>1.8</maven.compiler.source>\
                     <maven.compiler.target>11</maven.compiler.target>")
            )]),
            ["scip-java  maven release[11]"]
        );
        // The compiler plugin's `<release>`, resolved; another plugin's is not the compiler's.
        let plugins = pom(
            "<artifactId>app</artifactId><properties><jdk>22</jdk></properties><build><plugins>\
             <plugin><artifactId>maven-compiler-plugin</artifactId>\
             <configuration><release>${jdk}</release></configuration></plugin>\
             <plugin><artifactId>maven-enforcer-plugin</artifactId>\
             <configuration><release>99</release></configuration></plugin>\
             </plugins></build>",
        );
        assert_eq!(
            plan(&[("pom.xml", &plugins)]),
            ["scip-java  maven release[22]"]
        );
        // The parent chain inside the job: `api` inherits the root's `${java.version}` release
        // with its own `java.version`. `web`'s parent is outside the job (`<relativePath/>`),
        // and `svc`'s `../pom.xml` is not its parent (another artifactId): neither inherits.
        let root = pom(&format!(
            "<artifactId>parent</artifactId><properties><java.version>17</java.version>{}\
             </properties>",
            release("${java.version}")
        ));
        let module = |parent: &str, relative: &str| {
            pom(&format!(
                "<parent><artifactId>{parent}</artifactId>{relative}</parent>\
                 <artifactId>m</artifactId><properties><java.version>24</java.version></properties>"
            ))
        };
        assert_eq!(
            plan(&[("api/pom.xml", &module("parent", "")), ("pom.xml", &root),]),
            ["scip-java  maven release[24]"]
        );
        assert_eq!(
            plan(&[
                ("pom.xml", &root),
                ("svc/pom.xml", &module("spring-boot-starter-parent", "")),
                ("web/pom.xml", &module("parent", "<relativePath/>")),
            ]),
            ["scip-java  maven release[17]"]
        );
        // A Gradle build never takes a release, even over a Maven build inside it.
        assert_eq!(
            plan(&[
                ("tools/settings.gradle", ""),
                ("tools/sub/pom.xml", &app(&release("25"))),
            ]),
            [
                "scip-java tools gradle",
                "scip-java tools/sub maven release[25]"
            ]
        );

        // The image follows the release.
        use crate::orchestrate::{Images, image_for};
        let maven = |java_release: Option<u32>| Job {
            root: String::new(),
            indexer: Indexer::ScipJava,
            fetch: Fetch::Maven,
            projects: Vec::new(),
            unpinned: false,
            workspace: Vec::new(),
            replace_targets: Vec::new(),
            java_release,
        };
        let gradle = Job {
            fetch: Fetch::Gradle,
            ..maven(None)
        };
        for (job, image) in [
            (maven(Some(25)), SCIP_JAVA25_IMAGE),
            (maven(Some(22)), SCIP_JAVA25_IMAGE),
            (maven(Some(21)), SCIP_JAVA_IMAGE),
            (maven(None), SCIP_JAVA_IMAGE),
            (gradle.clone(), SCIP_JAVA_IMAGE),
        ] {
            assert_eq!(image_for(&job), image, "{job:?}");
        }
        let images = Images {
            java: Ok("sha256:jdk21".into()),
            java25: Ok("sha256:jdk25".into()),
            go: Err("absent".into()),
            proxy: Ok("sha256:proxy".into()),
        };
        assert_eq!(
            images.for_job(&maven(Some(25))).as_deref(),
            Ok("sha256:jdk25")
        );
        assert_eq!(
            images.for_job(&maven(Some(21))).as_deref(),
            Ok("sha256:jdk21")
        );
        assert_eq!(images.for_job(&gradle).as_deref(), Ok("sha256:jdk21"));
        let above = images.for_job(&maven(Some(26))).unwrap_err();
        assert!(above.contains("release 26"), "{above}");
        let absent = Images {
            java25: Err(format!(
                "scip-java: image {SCIP_JAVA25_IMAGE} is not available"
            )),
            ..images.clone()
        };
        let reason = absent.for_job(&maven(Some(25))).unwrap_err();
        assert!(reason.contains(SCIP_JAVA25_IMAGE), "{reason}");
        assert_eq!(
            absent.for_job(&maven(Some(21))).as_deref(),
            Ok("sha256:jdk21")
        );
    }

    #[test]
    fn go_modules_are_their_own_jobs() {
        // Each go.mod directory builds independently (workspaces are M6e);
        // without a go.sum alongside, the fetch is not reproducible.
        let files = ["go.mod", "go.sum", "main.go", "tools/go.mod", "lib/go.mod"];
        assert_eq!(
            summary(&files),
            [
                "scip-go  go",
                "scip-go lib go unpinned",
                "scip-go tools go unpinned"
            ]
        );
    }

    /// Contract M9d D5, D6, D7 (C5): the planner reads each Go root's `go.mod`. A root whose
    /// `go.mod` requires, with no `replace`, the module of another `go.mod` under it gets a
    /// workspace of those directories, sorted (mattermost `server`: `public`). A required
    /// in-repo module that is also replaced (by a directory or by another module), a module
    /// outside the root, and a root with its own `go.work` get none. A `replace` onto a local
    /// directory outside the root is a replace target, as written, whether or not it stays in
    /// the repository (`projects::job_files` decides that); one inside the root, or onto
    /// another module, is not. A `go.mod` that cannot be read plans as before. A vendored
    /// module's `go.mod` (go_gin_example commits `vendor/` with six) and one in a directory Go
    /// ignores (`testdata`, a name starting `.` or `_`) are not workspace modules.
    #[test]
    fn go_jobs_requiring_in_repo_modules_get_a_workspace() {
        let files = [
            "server/go.mod",
            "server/go.sum",
            "server/public/go.mod",
            "server/public/go.sum",
            "multi/go.mod",
            "multi/z/go.mod",
            "multi/a/go.mod",
            "svc/go.mod",
            "svc/lib/go.mod",
            "fork/go.mod",
            "fork/lib/go.mod",
            "tools/x/go.mod",
            "tools/sct/go.mod",
            "tools/esc/go.mod",
            "ws/go.mod",
            "ws/go.work",
            "ws/sub/go.mod",
            "unread/go.mod",
            "unread/sub/go.mod",
            "vend/go.mod",
            "vend/vendor/example.com/x/go.mod",
            "vend/testdata/mod/go.mod",
            "vend/_tools/go.mod",
            "vend/.cache/go.mod",
            "vend/ok/go.mod",
        ];
        let contents = [
            (
                "server/go.mod",
                "module github.com/m/m/server/v8\n\ngo 1.26.7\n\nrequire (\n\tgithub.com/m/m/server/public v0.4.3\n\tgithub.com/other/x v1.0.0 // indirect\n)\n\nreplace github.com/vmihailenco/msgpack/v5 => github.com/m/msgpack/v5 v5.0.0\n",
            ),
            (
                "server/public/go.mod",
                "module github.com/m/m/server/public\n\ngo 1.26.7\n",
            ),
            (
                "multi/go.mod",
                "module example.com/multi\n\nrequire example.com/multi/z v0.1.0\nrequire \"example.com/multi/a\" v0.1.0\n",
            ),
            ("multi/z/go.mod", "module example.com/multi/z\n"),
            ("multi/a/go.mod", "module \"example.com/multi/a\" // a\n"),
            (
                "svc/go.mod",
                "module example.com/svc\n\nrequire example.com/svc/lib v0.1.0\n\nreplace example.com/svc/lib => ./lib\n",
            ),
            ("svc/lib/go.mod", "module example.com/svc/lib\n"),
            (
                "fork/go.mod",
                "module example.com/fork\n\nrequire example.com/fork/lib v0.1.0\n\nreplace (\n\texample.com/fork/lib v0.1.0 => example.com/other/lib v0.2.0\n)\n",
            ),
            ("fork/lib/go.mod", "module example.com/fork/lib\n"),
            (
                "tools/x/go.mod",
                "module example.com/x\n\nrequire github.com/m/m/server/public v0.4.0\n",
            ),
            (
                "tools/sct/go.mod",
                "module github.com/m/m/tools/sct\n\ngo 1.26.7\n\nrequire github.com/m/m/server/public v0.4.0\n\nreplace github.com/m/m/server/public => ../../server/public\n",
            ),
            (
                "tools/esc/go.mod",
                "module example.com/esc\n\nreplace (\n\texample.com/a => ../../../../a\n\texample.com/b v1.0.0 => /opt/b\n\texample.com/c => ./c\n\texample.com/a => ../../../../a\n)\n",
            ),
            (
                "ws/go.mod",
                "module example.com/ws\n\nrequire example.com/ws/sub v0.1.0\n",
            ),
            ("ws/sub/go.mod", "module example.com/ws/sub\n"),
            ("unread/sub/go.mod", "module example.com/unread/sub\n"),
            (
                "vend/go.mod",
                "module example.com/vend\n\nrequire (\n\texample.com/x v1.0.0\n\texample.com/vend/fixture v0.0.0\n\texample.com/vend/tools v0.0.0\n\texample.com/vend/cache v0.0.0\n\texample.com/vend/ok v0.0.0\n)\n",
            ),
            ("vend/vendor/example.com/x/go.mod", "module example.com/x\n"),
            (
                "vend/testdata/mod/go.mod",
                "module example.com/vend/fixture\n",
            ),
            ("vend/_tools/go.mod", "module example.com/vend/tools\n"),
            ("vend/.cache/go.mod", "module example.com/vend/cache\n"),
            ("vend/ok/go.mod", "module example.com/vend/ok\n"),
        ];
        assert_eq!(
            workspace_summary(&files, &contents).0,
            [
                "scip-go fork go unpinned",
                "scip-go fork/lib go unpinned",
                "scip-go multi go unpinned work[a,z]",
                "scip-go multi/a go unpinned",
                "scip-go multi/z go unpinned",
                "scip-go server go work[public]",
                "scip-go server/public go",
                "scip-go svc go unpinned",
                "scip-go svc/lib go unpinned",
                "scip-go tools/esc go unpinned replace[../../../../a,/opt/b]",
                "scip-go tools/sct go unpinned replace[../../server/public]",
                "scip-go tools/x go unpinned",
                "scip-go unread go unpinned",
                "scip-go unread/sub go unpinned",
                "scip-go vend go unpinned work[ok]",
                "scip-go vend/.cache go unpinned",
                "scip-go vend/_tools go unpinned",
                "scip-go vend/ok go unpinned",
                "scip-go vend/testdata/mod go unpinned",
                "scip-go vend/vendor/example.com/x go unpinned",
                "scip-go ws go unpinned",
                "scip-go ws/sub go unpinned",
            ]
        );
    }

    #[test]
    fn python_roots_are_their_own_jobs() {
        // Contract M6d S2: every pyproject.toml/setup.py directory is a job.
        // Contract M6d S4: without a lockfile the fetch installs the project
        // only (unpinned).
        let files = [
            "pyproject.toml",
            "pkg/main.py",
            "libs/pyproject.toml",
            "tools/setup.py",
        ];
        assert_eq!(
            summary(&files),
            [
                "scip-python  python(None) unpinned",
                "scip-python libs python(None) unpinned",
                "scip-python tools python(None) unpinned",
            ]
        );
    }

    #[test]
    fn python_fetches_are_lockfile_driven() {
        // Contract M6d S4: a poetry.lock installs the locked tree
        // (reproducible); a requirements.txt installs its pins without
        // transitive dependencies; anything else installs the project only.
        // A uv.lock has no installer in the image, so it takes the
        // project-only path (S6 reads its versions).
        let files = [
            "pyproject.toml",
            "poetry.lock",
            "libs/pyproject.toml",
            "libs/requirements.txt",
            "tools/setup.py",
            "tools/uv.lock",
            "tools/pyproject.toml",
            "bare/pyproject.toml",
        ];
        assert_eq!(
            summary(&files),
            [
                "scip-python  python(Poetry)",
                "scip-python bare python(None) unpinned",
                "scip-python libs python(Requirements) unpinned",
                "scip-python tools python(None) unpinned",
            ]
        );
    }

    /// Contract M9d D2 (C3): a directory with a `requirements.txt` and neither a
    /// `pyproject.toml` nor a `setup.py` is a Python root installing its pins (microblog's
    /// root, microservices_demo's `src/*service`), at the repository root too. A
    /// `requirements.txt` beside a `pyproject.toml` or a `setup.py` is that root's lockfile,
    /// not a second job. A requirements-only root stays `Requirements` beside a `poetry.lock`
    /// (Poetry needs a `pyproject.toml`).
    #[test]
    fn requirements_only_directories_are_python_roots() {
        let files = [
            "requirements.txt",
            "app.py",
            "src/emailservice/requirements.txt",
            "src/emailservice/email_server.py",
            "libs/pyproject.toml",
            "libs/requirements.txt",
            "tools/setup.py",
            "tools/requirements.txt",
            "legacy/requirements.txt",
            "legacy/poetry.lock",
            "docs/conf.py",
        ];
        assert_eq!(
            summary(&files),
            [
                "scip-python  python(Requirements) unpinned",
                "scip-python legacy python(Requirements) unpinned",
                "scip-python libs python(Requirements) unpinned",
                "scip-python src/emailservice python(Requirements) unpinned",
                "scip-python tools python(Requirements) unpinned",
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
                // The root package holds a lockfile but declares no workspaces, and the nested
                // packages have no lockfile of their own: they are separate, unpinned jobs
                // (contract M6a D5 groups only a root that declares them).
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
                // Contract M6d S1: go.mod directories are ScipGo jobs.
                "scip-go services/billing go unpinned",
            ]
        );
        assert!(jobs(&["README.md"], &|_| None).jobs.is_empty());
    }

    /// Contract M3b amendment 64 (`source_encoding`): measured with `rb-scip-java:0.13.1`
    /// (`var/fix-pass7/scratch`), javac reads the sources in the declared
    /// `project.build.sourceEncoding`, and as UTF-8 when nothing is declared; an ISO-8859-1
    /// build shifts every later column of a non-ASCII line.
    #[test]
    fn a_maven_build_declaring_another_source_encoding_is_found() {
        let pom = |body: &str| format!("<project><properties>{body}</properties></project>");
        let found = |poms: &[String]| {
            maven_source_encoding(&poms.iter().map(|p| p.as_bytes()).collect::<Vec<_>>())
        };
        // Undeclared, UTF-8 in any spelling, or a property resolving to UTF-8.
        assert_eq!(found(&[pom("")]), None);
        assert_eq!(
            found(&[pom(
                "<project.build.sourceEncoding> utf8 </project.build.sourceEncoding>"
            )]),
            None
        );
        assert_eq!(
            found(&[
                pom("<enc.x>UTF-8</enc.x>"),
                pom("<maven.compiler.encoding>${enc.x}</maven.compiler.encoding>"),
            ]),
            None
        );
        // petclinic's `<encoding.source>` is another element; a comment declares nothing.
        assert_eq!(
            found(&[pom(
                "<encoding.source>ISO-8859-1</encoding.source><!-- <encoding>Cp1252</encoding> -->"
            )]),
            None
        );
        // Anything else, in any module's pom, or a property that does not resolve.
        assert_eq!(
            found(&[pom(
                "<project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding>"
            )])
            .as_deref(),
            Some("ISO-8859-1")
        );
        assert_eq!(
            found(&[
                pom("<project.build.sourceEncoding>UTF-8</project.build.sourceEncoding>"),
                "<project><build><plugins><plugin><configuration><encoding>Cp1252</encoding></configuration></plugin></plugins></build></project>".to_owned(),
            ])
            .as_deref(),
            Some("Cp1252")
        );
        assert_eq!(
            found(&[pom(
                "<maven.compiler.encoding>${missing}</maven.compiler.encoding>"
            )])
            .as_deref(),
            Some("${missing}")
        );
    }

    /// Contract M6e S11: the effective javac encoding resolves per document from all five
    /// configuration sources in documented precedence (lowest first): `compilerArgs
    /// -encoding`, pom literals (the nearest pom's declaration over its ancestors'),
    /// `.mvn/maven.config` `-D`, explicit `-D` flags (the pipeline passes none). Anything
    /// unreadable keeps the higher slot's reading (or UTF-8).
    #[test]
    fn effective_encoding_follows_the_five_source_precedence() {
        let case = |doc: &str, paths: &[(&str, &str)], config: Option<&str>, cli: &[&str]| {
            let owned: Vec<(String, Vec<u8>)> = paths
                .iter()
                .map(|(p, t)| ((*p).to_owned(), t.as_bytes().to_vec()))
                .collect();
            let config = config.map(str::to_owned);
            let cli: Vec<String> = cli.iter().map(|s| (*s).to_owned()).collect();
            let poms: Vec<(&str, &[u8])> = owned
                .iter()
                .map(|(p, t)| (p.as_str(), t.as_slice()))
                .collect();
            let borrowed = EncodingInputs {
                poms,
                maven_config: config.as_deref().map(str::as_bytes),
                cli: &cli,
            };
            effective_encoding(doc, &borrowed).clone()
        };
        let iso = "<project><properties><project.build.sourceEncoding>ISO-8859-1</project.build.sourceEncoding></properties></project>";
        let utf8 = "<project><properties><project.build.sourceEncoding>UTF-8</project.build.sourceEncoding></properties></project>";
        let arg = |v: &str| {
            format!(
                "<project><build><plugins><plugin><configuration><compilerArgs><compilerArg>-encoding</compilerArg><compilerArg>{v}</compilerArg></compilerArgs></configuration></plugin></plugins></build></project>"
            )
        };
        // Nothing anywhere: today's UTF-8 reading.
        assert_eq!(case("A.java", &[], None, &[]), None);
        assert_eq!(case("A.java", &[("pom.xml", utf8)], None, &[]), None);
        // The nearest pom governs each document: a module ISO build rejects its own files
        // while the UTF-8 root's files stay trusted, and vice versa (no per-job unsafe
        // corner).
        assert_eq!(
            case(
                "mod/B.java",
                &[("pom.xml", utf8), ("mod/pom.xml", iso)],
                None,
                &[]
            )
            .as_deref(),
            Some("ISO-8859-1")
        );
        assert_eq!(
            case(
                "A.java",
                &[("pom.xml", utf8), ("mod/pom.xml", iso)],
                None,
                &[]
            ),
            None
        );
        assert_eq!(
            case(
                "A.java",
                &[("pom.xml", iso), ("mod/pom.xml", utf8)],
                None,
                &[]
            )
            .as_deref(),
            Some("ISO-8859-1")
        );
        assert_eq!(
            case(
                "mod/B.java",
                &[("pom.xml", iso), ("mod/pom.xml", utf8)],
                None,
                &[]
            ),
            None
        );
        // A pom governs its subtree only: the module's ISO declaration does not touch the
        // root's files, and a nested file answers to the nearest pom.
        assert_eq!(case("A.java", &[("mod/pom.xml", iso)], None, &[]), None);
        assert_eq!(
            case(
                "mod/sub/C.java",
                &[("pom.xml", utf8), ("mod/pom.xml", iso)],
                None,
                &[]
            )
            .as_deref(),
            Some("ISO-8859-1")
        );
        // `compilerArgs -encoding` reads lowest (measured): a pom literal beats it, but it
        // still declares when the poms are silent, nearest likewise.
        assert_eq!(
            case("A.java", &[("pom.xml", &arg("ISO-8859-1"))], None, &[]).as_deref(),
            Some("ISO-8859-1")
        );
        assert_eq!(
            case(
                "mod/B.java",
                &[("pom.xml", iso), ("mod/pom.xml", &arg("UTF-8"))],
                None,
                &[]
            )
            .as_deref(),
            Some("ISO-8859-1")
        );
        assert_eq!(
            case(
                "mod/B.java",
                &[
                    ("pom.xml", &arg("ISO-8859-1")),
                    ("mod/pom.xml", &arg("UTF-8"))
                ],
                None,
                &[]
            ),
            None
        );
        // `.mvn/maven.config -D` beats `compilerArgs`.
        assert_eq!(
            case(
                "A.java",
                &[("pom.xml", &arg("ISO-8859-1"))],
                Some("-Dproject.build.sourceEncoding=UTF-8"),
                &[]
            ),
            None
        );
        assert_eq!(
            case(
                "A.java",
                &[("pom.xml", utf8)],
                Some("-Dencoding=ISO-8859-1"),
                &[]
            )
            .as_deref(),
            Some("ISO-8859-1")
        );
        // Explicit `-D` flags beat the config file.
        assert_eq!(
            case(
                "A.java",
                &[("pom.xml", utf8)],
                Some("-Dproject.build.sourceEncoding=ISO-8859-1"),
                &["-Dproject.build.sourceEncoding=UTF-8"]
            ),
            None
        );
        // Unreadable anywhere keeps the higher slot's reading: a dangling `-encoding`, garbage
        // config lines, and a `-D` without `=` name nothing. An unresolvable `${…}` value is a
        // declaration, like the pom reader's `${missing}`: fail-safe away from UTF-8.
        assert_eq!(
            case(
                "A.java",
                &[("pom.xml", iso)],
                Some("not an option\n-Dencoding"),
                &["-Dencoding"]
            )
            .as_deref(),
            Some("ISO-8859-1")
        );
        assert_eq!(
            case("A.java", &[("pom.xml", &arg("${enc}"))], None, &[]).as_deref(),
            Some("${enc}")
        );
    }

    // Contract M6a D5 (S8): workspace membership from the root manifests.

    /// The files of a workspace: the root's manifest and lockfile, members `packages/a` and
    /// `packages/b` with a `tsconfig.json` each, and `legacy` (not matched) with its own.
    const WORKSPACE: [&str; 11] = [
        "package.json",
        "package-lock.json",
        "packages/a/package.json",
        "packages/a/tsconfig.json",
        "packages/a/src/a.ts",
        "packages/b/package.json",
        "packages/b/tsconfig.json",
        "packages/b/test/tsconfig.json",
        "legacy/package.json",
        "legacy/tsconfig.json",
        "README.md",
    ];

    fn with_workspaces(workspaces: &str) -> (Vec<String>, Vec<String>) {
        let manifest = format!("{{\"name\": \"root\", \"workspaces\": {workspaces}}}");
        workspace_summary(&WORKSPACE, &[("package.json", manifest.as_str())])
    }

    /// The npm array and Yarn's `{ "packages": [...] }` forms, with literal, `*` and `**`
    /// segments and a trailing `/`: the members' `tsconfig.json` directories, sorted, are the
    /// root job's projects; `legacy`, not matched, keeps its own unpinned job.
    #[test]
    fn a_workspace_root_with_a_lockfile_groups_its_members() {
        let grouped = (
            vec![
                "scip-typescript  npm(PackageLock) [packages/a,packages/b,packages/b/test] {packages/a,packages/b}"
                    .to_owned(),
                "scip-typescript legacy npm(None) [.] unpinned".to_owned(),
            ],
            Vec::<String>::new(),
        );
        for form in [
            r#"["packages/*"]"#,
            r#"["packages/*/"]"#,
            r#"["packages/a", "packages/b"]"#,
            r#"["packages/**"]"#,
            r#"{"packages": ["packages/*"]}"#,
            r#"["packages/*", "packages/a", "tools/*"]"#,
        ] {
            assert_eq!(with_workspaces(form), grouped, "{form}");
        }
        // `**` matches at any depth below its prefix, at least one level.
        let deep = [
            "package.json",
            "yarn.lock",
            "libs/x/package.json",
            "libs/x/tsconfig.json",
            "libs/y/z/package.json",
            "libs/y/z/tsconfig.json",
            "libs/package.json",
            "libs/tsconfig.json",
        ];
        assert_eq!(
            workspace_summary(
                &deep,
                &[
                    ("package.json", r#"{"workspaces": ["libs/**"]}"#),
                    (
                        "yarn.lock",
                        "# THIS IS AN AUTOGENERATED FILE.\n# yarn lockfile v1\n\n\"a@1\":\n"
                    ),
                ]
            ),
            (
                vec![
                    "scip-typescript  npm(Yarn) [libs/x,libs/y/z] {libs/x,libs/y/z}".to_owned(),
                    "scip-typescript libs npm(None) [.] unpinned".to_owned(),
                ],
                vec![]
            )
        );
    }

    /// A sibling whose name sorts between a workspace root and its members
    /// (`mods-old` between `mods` and `mods/app`: `-` is below `/`) must not shadow
    /// the members: the scan continues while the string prefix holds and `is_under`
    /// decides per candidate (evaluator pass 1, Z1).
    #[test]
    fn a_sibling_sorting_between_a_root_and_its_members_does_not_shadow_them() {
        let files = [
            "mods/package.json",
            "mods/package-lock.json",
            "mods/app/package.json",
            "mods/app/tsconfig.json",
            "mods-old/package.json",
            "mods-old/tsconfig.json",
        ];
        assert_eq!(
            workspace_summary(
                &files,
                &[("mods/package.json", r#"{"workspaces": ["app"]}"#)]
            ),
            (
                vec![
                    "scip-typescript mods npm(PackageLock) [app] {app}".to_owned(),
                    "scip-typescript mods-old npm(None) [.] unpinned".to_owned(),
                ],
                Vec::<String>::new(),
            )
        );
    }

    /// Every pattern outside D5's allow-list leaves the whole root ungrouped (today's jobs),
    /// counted with its reason.
    #[test]
    fn an_unsupported_pattern_leaves_the_workspace_ungrouped() {
        let ungrouped_jobs = vec![
            "scip-typescript legacy npm(None) [.] unpinned".to_owned(),
            "scip-typescript packages/a npm(None) [.] unpinned".to_owned(),
            "scip-typescript packages/b npm(None) [.,test] unpinned".to_owned(),
        ];
        for pattern in [
            "!packages/a",
            "{packages/a,packages/b}",
            "packages/[ab]",
            "packages/?",
            "packages/pkg-*",
            "packages/*b",
            "../x",
            "/abs",
            "",
            "./packages/*",
            "packages//a",
            "**/a",
            "packages/**/a",
            "packages/a b",
        ] {
            let form = format!("[\"packages/*\", {}]", serde_json::Value::from(pattern));
            assert_eq!(
                with_workspaces(&form),
                (
                    ungrouped_jobs.clone(),
                    vec![" workspace_glob_unsupported".to_owned()]
                ),
                "{pattern:?}"
            );
        }
        // A `workspaces` value of another shape, or a manifest that is not JSON.
        for manifest in [
            r#"{"workspaces": "packages/*"}"#,
            r#"{"workspaces": [1]}"#,
            r#"{"workspaces": {"nohoist": ["x"]}}"#,
            r#"{"workspaces": ["packages/*"],"#,
        ] {
            assert_eq!(
                workspace_summary(&WORKSPACE, &[("package.json", manifest)]),
                (
                    ungrouped_jobs.clone(),
                    vec![" manifest_unreadable".to_owned()]
                ),
                "{manifest}"
            );
        }
    }

    /// A root without `package-lock.json` or `yarn.lock` (`polyglot-monorepo`'s shape: npm
    /// would resolve every member afresh) and a Yarn ≥ 2 lockfile are not
    /// grouped, each with its reason; pnpm roots group through `pnpm-workspace.yaml`
    /// (contract M6e S6); the jobs are today's.
    #[test]
    fn a_workspace_without_a_supported_lockfile_is_ungrouped() {
        let polyglot = [
            "package.json",
            "apps/web/package.json",
            "apps/web/tsconfig.json",
            "packages/shared/package.json",
            "packages/shared/tsconfig.json",
            "services/notification/package.json",
            "services/notification/tsconfig.json",
        ];
        assert_eq!(
            workspace_summary(
                &polyglot,
                &[(
                    "package.json",
                    r#"{"workspaces": ["apps/*", "packages/*", "services/notification"]}"#
                )]
            ),
            (
                vec![
                    "scip-typescript apps/web npm(None) [.] unpinned".to_owned(),
                    "scip-typescript packages/shared npm(None) [.] unpinned".to_owned(),
                    "scip-typescript services/notification npm(None) [.] unpinned".to_owned(),
                ],
                vec![" workspace_without_lockfile".to_owned()]
            )
        );
        let ungrouped = |lockfile: &str, text: &str| {
            let mut files: Vec<&str> = WORKSPACE.to_vec();
            files.retain(|f| *f != "package-lock.json");
            files.push(lockfile);
            workspace_summary(
                &files,
                &[
                    ("package.json", r#"{"workspaces": ["packages/*"]}"#),
                    (lockfile, text),
                ],
            )
            .1
        };
        assert_eq!(
            ungrouped(
                "yarn.lock",
                "# This file is generated by running \"yarn install\"\n\n__metadata:\n  version: 6\n"
            ),
            [" yarn_berry"]
        );
        assert_eq!(
            ungrouped("pnpm-lock.yaml", "lockfileVersion: '9.0'\n"),
            Vec::<String>::new()
        );
        assert_eq!(
            ungrouped(
                "yarn.lock",
                "# yarn lockfile v1\n\n\"a@1\":\n  version \"1\"\n"
            ),
            Vec::<String>::new()
        );
    }

    /// Contract M6e S6: pnpm workspaces group like npm ones — members come from
    /// `pnpm-workspace.yaml` `packages:` globs through the same allow-list engine, and the
    /// root job fetches with pnpm from its lockfile. A root with a pnpm lockfile but no
    /// workspace file is a plain single-package job (no reason recorded).
    #[test]
    fn pnpm_workspace_file_patterns() {
        assert_eq!(
            pnpm_workspace_patterns(b"packages:\n  - 'a/*'\n  - b/c\n"),
            Some(vec!["a/*".to_owned(), "b/c".to_owned()])
        );
        assert_eq!(
            pnpm_workspace_patterns(b"# comment\npackages:\n"),
            Some(vec![])
        );
        assert_eq!(pnpm_workspace_patterns(b"packages: ['a']\n"), None);
        assert_eq!(pnpm_workspace_patterns(b"other: 1\n"), None);
    }

    #[test]
    fn pnpm_workspaces_group_through_the_workspace_file() {
        let files = [
            "package.json",
            "pnpm-workspace.yaml",
            "pnpm-lock.yaml",
            "packages/a/package.json",
            "packages/a/tsconfig.json",
            "packages/b/package.json",
            "packages/b/tsconfig.json",
            "legacy/package.json",
            "legacy/tsconfig.json",
        ];
        let (jobs, ungrouped) = workspace_summary(
            &files,
            &[
                ("package.json", r#"{"name": "root"}"#),
                ("pnpm-workspace.yaml", "packages:\n  - 'packages/*'\n"),
            ],
        );
        assert_eq!(
            jobs,
            vec![
                "scip-typescript  npm(Pnpm) [packages/a,packages/b] {packages/a,packages/b}"
                    .to_owned(),
                "scip-typescript legacy npm(None) [.] unpinned".to_owned(),
            ]
        );
        assert!(ungrouped.is_empty(), "{ungrouped:?}");
        // No workspace file: plain roots keep today's jobs (the root lock
        // alone groups nothing and records no reason).
        let (jobs, ungrouped) =
            workspace_summary(&files, &[("package.json", r#"{"name": "root"}"#)]);
        assert!(ungrouped.is_empty(), "{ungrouped:?}");
        assert_eq!(
            jobs,
            vec![
                "scip-typescript legacy npm(None) [.] unpinned".to_owned(),
                "scip-typescript packages/a npm(None) [.] unpinned".to_owned(),
                "scip-typescript packages/b npm(None) [.] unpinned".to_owned(),
            ]
        );
    }

    /// A member's own lockfile is ignored (npm and Yarn install from the root's); a nested
    /// workspace root the outer patterns match is a member, one they do not match is its own
    /// root (npm does not nest workspaces); a package root outside every pattern keeps its job.
    #[test]
    fn members_lockfiles_and_nested_roots() {
        let files = [
            "package.json",
            "package-lock.json",
            "packages/a/package.json",
            "packages/a/package-lock.json",
            "packages/a/tsconfig.json",
            "packages/inner/package.json",
            "packages/inner/tsconfig.json",
            "packages/inner/sub/package.json",
            "packages/inner/sub/tsconfig.json",
            "tools/ws/package.json",
            "tools/ws/yarn.lock",
            "tools/ws/tsconfig.json",
            "tools/ws/mods/m/package.json",
            "tools/ws/mods/m/tsconfig.json",
        ];
        let contents = [
            ("package.json", r#"{"workspaces": ["packages/*"]}"#),
            ("packages/inner/package.json", r#"{"workspaces": ["sub"]}"#),
            ("tools/ws/package.json", r#"{"workspaces": ["mods/*"]}"#),
            ("tools/ws/yarn.lock", "# yarn lockfile v1\n"),
        ];
        assert_eq!(
            workspace_summary(&files, &contents),
            (
                vec![
                    "scip-typescript  npm(PackageLock) [packages/a,packages/inner] {packages/a,packages/inner}".to_owned(),
                    "scip-typescript packages/inner/sub npm(None) [.] unpinned".to_owned(),
                    "scip-typescript tools/ws npm(Yarn) [.,mods/m] {mods/m}".to_owned(),
                ],
                vec![]
            )
        );
    }

    /// Contract M6a S19, invariants 9 and 10: membership is one trie walk per package root
    /// (literal and `*` children, a final `**`), and each `tsconfig.json` finds its package
    /// root by walking up its directories: 4× the package roots and patterns → at most 5× the
    /// counted steps.
    fn membership_work_for(n: usize) -> u64 {
        let mut files = vec!["package.json".to_owned(), "package-lock.json".to_owned()];
        let mut patterns = vec!["\"deep/**\"".to_owned(), "\"wild/*\"".to_owned()];
        for i in 0..n {
            for dir in [
                format!("pkgs/p{i}"),
                format!("deep/d{i}/x"),
                format!("wild/w{i}"),
            ] {
                files.push(format!("{dir}/package.json"));
                files.push(format!("{dir}/tsconfig.json"));
            }
            patterns.push(format!("\"pkgs/p{i}\""));
        }
        let manifest = format!("{{\"workspaces\": [{}]}}", patterns.join(", "));
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        let read = |path: &str| (path == "package.json").then(|| manifest.as_bytes().to_vec());
        work::take();
        let plan = jobs(&files, &read);
        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(plan.members[""].len(), 3 * n);
        work::take()
    }

    #[test]
    fn workspace_membership_grows_linearly() {
        let (small, large) = (membership_work_for(100), membership_work_for(400));
        assert!(large <= small * 5, "{small} -> {large}");
    }

    /// S19 with shadowing siblings (evaluator pass 1, Z1): each workspace root has
    /// a non-member sibling sorting between it and its members (`rt-old{i}` between
    /// `rt` and `rt/pkgs/…`), which the scan must walk past; 4× the roots → at most
    /// 5× the counted steps.
    fn shadowed_membership_work_for(n: usize) -> u64 {
        let mut files = vec![
            "rt/package.json".to_owned(),
            "rt/package-lock.json".to_owned(),
        ];
        let mut patterns = vec!["\"deep/**\"".to_owned(), "\"wild/*\"".to_owned()];
        for i in 0..n {
            for dir in [
                format!("rt/pkgs/p{i}"),
                format!("rt/deep/d{i}/x"),
                format!("rt/wild/w{i}"),
                format!("rt-old{i}"),
            ] {
                files.push(format!("{dir}/package.json"));
                files.push(format!("{dir}/tsconfig.json"));
            }
            patterns.push(format!("\"pkgs/p{i}\""));
        }
        let manifest = format!("{{\"workspaces\": [{}]}}", patterns.join(", "));
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        let read = |path: &str| (path == "rt/package.json").then(|| manifest.as_bytes().to_vec());
        work::take();
        let plan = jobs(&files, &read);
        assert_eq!(plan.members["rt"].len(), 3 * n);
        work::take()
    }

    #[test]
    fn workspace_membership_with_shadowing_siblings_grows_linearly() {
        let (small, large) = (
            shadowed_membership_work_for(100),
            shadowed_membership_work_for(400),
        );
        assert!(large <= small * 5, "{small} -> {large}");
    }

    /// Contract M6e S10 (tsconfig scoping): `include`/`exclude`/`files` decide per-file
    /// membership under the governing (nearest) tsconfig; anything unreadable fails open (the
    /// file stays unexplained, never wrongly out of scope).
    #[test]
    fn tsconfig_scopes_explain_out_of_scope_files() {
        let scopes = |files: &[(&str, &str)]| TsScopes::build(files.iter().copied());
        let scoped = scopes(&[(
            "tsconfig.json",
            r#"{"include": ["src"], "exclude": ["src/gen"]}"#,
        )]);
        assert!(!scoped.out_of_scope("src/a.ts"));
        assert!(scoped.out_of_scope("src/gen/b.ts"));
        assert!(scoped.out_of_scope("test/c.ts"));
        assert_eq!(scoped.unread, 0);
        // `files` lists exact paths; comments do not break the read.
        let listed = scopes(&[(
            "app/tsconfig.json",
            "// keep\n{\"files\": [\"main.ts\", \"./lib/x.ts\"]}",
        )]);
        assert!(!listed.out_of_scope("app/main.ts"));
        assert!(!listed.out_of_scope("app/lib/x.ts"));
        assert!(listed.out_of_scope("app/other.ts"));
        // No include/files: everything not excluded is in scope.
        let open = scopes(&[("tsconfig.json", r#"{"exclude": ["dist"]}"#)]);
        assert!(!open.out_of_scope("src/a.ts"));
        assert!(open.out_of_scope("dist/a.ts"));
        // No governing tsconfig, or an unreadable one, fails open.
        assert!(!scopes(&[]).out_of_scope("src/a.ts"));
        assert!(!scopes(&[("tsconfig.json", "{broken")]).out_of_scope("src/a.ts"));
        assert_eq!(scopes(&[("tsconfig.json", "{broken")]).unread, 1);
        // Globs beyond the allow-list (`{a,b}`, `?`, `!`) stay unread and fail open.
        let wild = scopes(&[("tsconfig.json", r#"{"include": ["src/{a,b}", "?.ts"]}"#)]);
        assert!(!wild.out_of_scope("src/a.ts"));
        assert_eq!(wild.unread, 2);
        // Non-string entries are skipped and counted; a key left with no usable entry is
        // unknown, not empty.
        let mixed = scopes(&[(
            "tsconfig.json",
            r#"{"include": [42], "exclude": ["dist", null]}"#,
        )]);
        assert!(!mixed.out_of_scope("src/a.ts"));
        assert!(!mixed.out_of_scope("dist/a.ts"));
        assert_eq!(mixed.unread, 2);
    }

    /// Contract M6e S10: `extends` resolves relatively with the child's keys winning, bounded
    /// by depth 8 with a cycle guard; an unresolvable chain leaves membership unknown.
    #[test]
    fn tsconfig_extends_resolve_bounded() {
        let scopes = |files: &[(&str, &str)]| TsScopes::build(files.iter().copied());
        // The child's `exclude` replaces the base's; the base's `include` fills in.
        let s = scopes(&[
            (
                "base.json",
                r#"{"include": ["src"], "exclude": ["src/a.ts"]}"#,
            ),
            (
                "app/tsconfig.json",
                r#"{"extends": "../base.json", "exclude": ["src/b.ts"]}"#,
            ),
        ]);
        assert!(!s.out_of_scope("app/src/x.ts"));
        assert!(!s.out_of_scope("app/src/a.ts"));
        assert!(s.out_of_scope("app/src/b.ts"));
        assert_eq!(s.unread, 0);
        // Deviation, fail-open: an extension-less file pattern (`"src/c"`) does not take on
        // TypeScript's inferred extensions, so `src/c.ts` stays unexplained rather than
        // wrongly out of scope. Directory patterns cover their subtree regardless.
        let ext = scopes(&[("tsconfig.json", r#"{"exclude": ["src/c", "src/gen"]}"#)]);
        assert!(!ext.out_of_scope("src/c.ts"));
        assert!(ext.out_of_scope("src/gen/c.ts"));
        // A cycle and a missing base leave membership unknown, counted once each.
        let looped = scopes(&[
            ("a/tsconfig.json", r#"{"extends": "../b/tsconfig.json"}"#),
            ("b/tsconfig.json", r#"{"extends": "../a/tsconfig.json"}"#),
        ]);
        assert!(!looped.out_of_scope("a/x.ts"));
        assert_eq!(looped.unread, 2);
        let missing = scopes(&[("app/tsconfig.json", r#"{"extends": "./nope.json"}"#)]);
        assert!(!missing.out_of_scope("app/x.ts"));
        assert_eq!(missing.unread, 1);
        // Nine chained extends exceed the depth cap.
        let owned: Vec<(String, String)> = (0..10)
            .map(|i| {
                (
                    format!("d{i}/tsconfig.json"),
                    format!("{{\"extends\": \"../d{}/tsconfig.json\"}}", i + 1),
                )
            })
            .collect();
        let refs: Vec<(&str, &str)> = owned
            .iter()
            .map(|(p, t)| (p.as_str(), t.as_str()))
            .collect();
        let deep = scopes(&refs);
        assert!(!deep.out_of_scope("d0/x.ts"));
        assert!(deep.unread >= 1);
    }

    /// Contract M6e S10, invariant 6: scoping 4× the files costs at most 5× the operations.
    #[test]
    fn tsconfig_scope_membership_grows_linearly() {
        let work = |n: usize| {
            let text = r#"{"include": ["src", "lib/*"], "exclude": ["src/gen", "lib/internal"]}"#;
            let scopes = TsScopes::build([("tsconfig.json", text)]);
            let files: Vec<String> = (0..n)
                .flat_map(|i| {
                    [
                        format!("src/a{i}.ts"),
                        format!("src/gen/b{i}.ts"),
                        format!("lib/x{i}/c.ts"),
                        format!("test/d{i}.ts"),
                    ]
                })
                .collect();
            work::take();
            for f in &files {
                let _ = scopes.out_of_scope(f);
            }
            work::take()
        };
        let (small, large) = (work(100), work(400));
        assert!(large <= small * 5, "{small} -> {large}");
    }

    /// Contract M9c C2 (D1): JSONC as `tsc` reads it, bookcars `admin/tsconfig.json`'s shape.
    /// Comments and trailing commas go; a `,}` or `,]` inside a string stays; strict JSON
    /// parses as `serde_json` parses it; broken text is `None`.
    #[test]
    fn tsconfig_jsonc_resolves_aliases() {
        let admin = r#"{
  "compilerOptions": {
    "target": "ES2020",
    "lib": ["ES2020", "DOM",],

    /* Bundler mode */
    "moduleResolution": "bundler", // trailing comment
    "description": "keeps ,} and ,] and // and /* inside strings",
    "paths": {
      "@/*": ["./src/*"],
      ":bookcars-types": ["../packages/bookcars-types"],
    },
  },
  "include": ["src"],
}
"#;
        let json = parse_jsonc(admin).expect("JSONC parses");
        let options = &json["compilerOptions"];
        assert_eq!(options["paths"]["@/*"], serde_json::json!(["./src/*"]));
        assert_eq!(
            options["paths"][":bookcars-types"],
            serde_json::json!(["../packages/bookcars-types"])
        );
        assert_eq!(options["lib"], serde_json::json!(["ES2020", "DOM"]));
        assert_eq!(
            options["description"],
            "keeps ,} and ,] and // and /* inside strings"
        );
        assert_eq!(json["include"], serde_json::json!(["src"]));
        // An escaped quote does not end the string, so the comma before `}` inside it stays.
        assert_eq!(
            parse_jsonc(r#"{"a": "x\",}", "b": [1, 2 ,  ] ,}"#),
            Some(serde_json::json!({"a": "x\",}", "b": [1, 2]}))
        );
        let strict = r#"{"compilerOptions": {"paths": {"@/*": ["./src/*"]}}, "n": [1, 2]}"#;
        assert_eq!(
            parse_jsonc(strict),
            serde_json::from_str::<serde_json::Value>(strict).ok()
        );
        // A leading UTF-8 byte order mark is not text, as `tsc` reads it (amendment 3, E4).
        assert_eq!(
            parse_jsonc(&format!("\u{feff}{strict}")),
            serde_json::from_str::<serde_json::Value>(strict).ok()
        );
        for broken in [
            r#"{"compilerOptions": {"paths": {"@/*": ["./src/*"]}}"#,
            r#"{"a": 1,, }"#,
            "",
        ] {
            assert_eq!(parse_jsonc(broken), None, "{broken}");
        }
    }

    /// Evaluator pass 1, M6e-12 (S10): a pattern escaping the tsconfig directory stays
    /// unread and fails open.
    #[test]
    fn tsconfig_patterns_escaping_their_directory_stay_unread() {
        let scopes =
            TsScopes::build([("tsconfig.json", r#"{"include": ["../o", "/abs", "src"]}"#)]);
        assert!(!scopes.out_of_scope("src/a.ts"));
        assert!(scopes.out_of_scope("o/a.ts"));
        assert_eq!(scopes.unread, 2);
    }
}
