//! Which SCIP indexer runs where (design §5.2): every supported build root in a commit becomes
//! one container job with a fetch phase (network to registries) and an index phase (no network).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// The scip-java image: the slim one built locally from `docker/scip-java` (the published
/// `ghcr.io/scip-code/scip-java:0.13.1` is 6.75 GB). A local image has no registry digest, so
/// the cache key uses its image ID, resolved at run time by [`crate::Runner::image_id`]; when the
/// image is absent the job is `unsupported` and nothing builds it implicitly.
pub const SCIP_JAVA_IMAGE: &str = "rb-scip-java:0.13.1";
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
/// it cannot be read) and is asked only for `package.json` files and workspace roots'
/// `yarn.lock`. Maven and Gradle roots are the outermost `pom.xml` / Gradle build files (a
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
        out.push(Job {
            root,
            indexer: Indexer::ScipJava,
            fetch,
            projects: Vec::new(),
            unpinned: false,
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
        });
    }
    // Go: every go.mod directory builds independently (workspaces are M6e).
    let mut go_roots = dirs_with("go.mod");
    go_roots.sort();
    go_roots.dedup();
    for root in go_roots {
        out.push(Job {
            root: root.clone(),
            indexer: Indexer::ScipGo,
            fetch: Fetch::Go,
            projects: Vec::new(),
            unpinned: !has(&join(&root, "go.sum")),
        });
    }
    // Python: every pyproject.toml/setup.py directory is a job (contract M6d
    // S2); the fetch installs from its lockfile (S4).
    let mut python_roots: Vec<String> = dirs_with("pyproject.toml")
        .into_iter()
        .chain(dirs_with("setup.py"))
        .collect();
    python_roots.sort();
    python_roots.dedup();
    for root in python_roots {
        // The strongest pin wins: a poetry.lock pins the whole tree, a
        // requirements.txt its pins only, anything else the project alone.
        let lockfile = if has(&join(&root, "poetry.lock")) {
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
