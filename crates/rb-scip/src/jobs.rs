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
/// `unsupported` and nothing builds it implicitly.
pub const SCIP_GO_IMAGE: &str = "rb-scip-go:0.2.7";
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
    /// `workspace_glob_unsupported`, `workspace_without_lockfile`,
    /// `workspace_lockfile_unsupported` (only `pnpm-lock.yaml`), `yarn_berry` or
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
            Some(Ok(None)) => continue,
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
            ungrouped("workspace_lockfile_unsupported");
            continue;
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
fn is_yarn_berry(text: &[u8]) -> bool {
    String::from_utf8_lossy(text)
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .is_some_and(|l| l.starts_with("__metadata:"))
}

/// One segment of an allowed workspace pattern (contract M6a D5).
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
#[derive(Default)]
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

#[cfg(test)]
mod tests {
    use super::*;

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
    /// would resolve every member afresh), a Yarn ≥ 2 lockfile and a pnpm lockfile are not
    /// grouped, each with its reason; the jobs are today's.
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
            [" workspace_lockfile_unsupported"]
        );
        assert_eq!(
            ungrouped(
                "yarn.lock",
                "# yarn lockfile v1\n\n\"a@1\":\n  version \"1\"\n"
            ),
            Vec::<String>::new()
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
}
