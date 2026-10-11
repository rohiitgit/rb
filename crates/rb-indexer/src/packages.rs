//! Manifest dependencies (contract M4b S15, S16, D7): direct dependencies read from
//! `package.json` (versions from `package-lock.json` v2/v3), `go.mod` and `pom.xml`, as
//! `Service -DEPENDS_ON-> Package` and, for a dependency on another service's package,
//! `Service -DEPENDS_ON{via: package}-> Service`, both with provenance `manifest` and evidence at
//! the dependency's entry. Only direct dependencies are read; a version is exact only from a
//! lockfile entry or a pom literal.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rb_git::boundaries::{self, Project};
use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance, Span,
};
use serde_json::{Value, json};

/// A manifest file this stage reads, by file name.
pub(crate) fn is_input(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "package.json"
            | "package-lock.json"
            | "npm-shrinkwrap.json"
            | "go.mod"
            | "go.work"
            | "pom.xml"
            // `yarn.lock`/`pnpm-lock.yaml` pin `npm/` versions (contract M6e
            // S5); `npm-shrinkwrap.json` stays npm-read.
            | "yarn.lock"
            | "pnpm-lock.yaml"
            // `build.gradle(.kts)` dependencies and `libs.versions.toml`
            // catalogs feed `maven/` package edges (contract M6e S3).
            | "build.gradle"
            | "build.gradle.kts"
            | "libs.versions.toml"
            // `pyproject.toml` direct dependencies feed `py/` package edges;
            // `poetry.lock`/`uv.lock` pin their versions (contract M6d S6).
            // `requirements.txt`/`Pipfile` files are noticed, never read.
            | "pyproject.toml"
            | "poetry.lock"
            | "uv.lock"
            // `lombok.config` texts feed qualifier-copying resolution (contract
            // M6c S8); the packages stage ignores unknown basenames.
            | "lombok.config"
            // `tsconfig.json` texts feed exact alias resolution (contract M6c
            // S4); the packages stage ignores unknown basenames.
            | "tsconfig.json"
    )
}

/// What this stage reads: the projects, the texts of [`is_input`] files, and every path of the
/// commit (a `yarn.lock` or `pnpm-lock.yaml` is noticed, not read).
pub(crate) struct Inputs<'a> {
    pub projects: &'a [Project],
    pub texts: &'a BTreeMap<&'a str, String>,
    pub paths: &'a BTreeSet<&'a str>,
}

/// A package as keyed by M3b (`<manager>/<name>@<version>`, `?` when unresolved).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Package {
    pub manager: &'static str,
    pub name: String,
    /// Empty when unresolved.
    pub version: String,
}

impl Package {
    pub fn key(&self) -> String {
        let version = if self.version.is_empty() {
            "?"
        } else {
            &self.version
        };
        format!("{}/{}@{version}", self.manager, self.name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Target {
    Package(Package),
    /// Another service's directory (`via: package`, S16).
    Service(String),
}

/// One `DEPENDS_ON` edge from a service's manifest.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ManifestEdge {
    /// The service's directory.
    pub service: String,
    pub target: Target,
    pub certainty: Certainty,
    pub attrs: Attrs,
    pub evidence: Evidence,
}

/// One declared dependency, as written.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Dep {
    name: String,
    declared: String,
    scope: &'static str,
    span: Span,
}

fn dir_prefix(dir: &str) -> String {
    if dir == boundaries::ROOT_DIR || dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    }
}

fn parent_dir(dir: &str) -> Option<&str> {
    if dir.is_empty() {
        None
    } else {
        Some(dir.rsplit_once('/').map_or("", |(p, _)| p))
    }
}

/// `a/b` joined with a relative path (`../c`, `./d`), normalised; `None` when it leaves the
/// repository.
fn join(dir: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = dir.split('/').filter(|p| !p.is_empty()).collect();
    for seg in rel.split('/') {
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

/// The span of the byte range `start..end` of `text`: rows and byte columns.
fn span_of(text: &str, start: usize, end: usize) -> Span {
    let point = |at: usize| {
        let before = &text[..at.min(text.len())];
        let row = before.matches('\n').count();
        let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1);
        (
            i32::try_from(row).unwrap_or(i32::MAX),
            i32::try_from(col).unwrap_or(i32::MAX),
        )
    };
    let (a, b) = (point(start), point(end));
    [a.0, a.1, b.0, b.1]
}

/// The byte range of the JSON key `"key"` (followed by `:`) at or after `from`.
fn json_key(text: &str, key: &str, from: usize) -> Option<(usize, usize)> {
    let quoted = format!("\"{key}\"");
    let mut at = from;
    while let Some(i) = text.get(at..)?.find(&quoted) {
        let start = at + i;
        let end = start + quoted.len();
        if text[end..].trim_start().starts_with(':') {
            return Some((start, end));
        }
        at = end;
    }
    None
}

/// The `package.json` fields read, with each one's scope (D7).
const NPM_FIELDS: &[(&str, &str)] = &[
    ("dependencies", "runtime"),
    ("devDependencies", "dev"),
    ("peerDependencies", "peer"),
    ("optionalDependencies", "optional"),
];

/// A `package.json`'s `name` and direct dependencies, in field order then name order; `None`
/// when it is no JSON object.
fn npm_manifest(text: &str) -> Option<(Option<String>, Vec<Dep>)> {
    let json: Value = serde_json::from_str(text).ok()?;
    let object = json.as_object()?;
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let mut deps = Vec::new();
    for (field, scope) in NPM_FIELDS {
        let Some(entries) = object.get(*field).and_then(Value::as_object) else {
            continue;
        };
        let field_at = json_key(text, field, 0).map_or(0, |(_, e)| e);
        for (dep, range) in entries {
            let span =
                json_key(text, dep, field_at).map_or([0, 0, 0, 0], |(s, e)| span_of(text, s, e));
            deps.push(Dep {
                name: dep.clone(),
                declared: range.as_str().unwrap_or_default().to_owned(),
                scope,
                span,
            });
        }
    }
    Some((name, deps))
}

/// A parsed npm lockfile: `packages` by path, or why it cannot be read.
fn npm_lock(text: &str) -> Result<serde_json::Map<String, Value>, String> {
    let json: Value = serde_json::from_str(text).map_err(|_| "invalid JSON".to_owned())?;
    match json.get("lockfileVersion").and_then(Value::as_u64) {
        Some(2 | 3) => {}
        Some(v) => return Err(format!("lockfileVersion {v}")),
        None => return Err("no lockfileVersion".to_owned()),
    }
    json.get("packages")
        .and_then(Value::as_object)
        .cloned()
        .ok_or_else(|| "no packages".to_owned())
}

/// Node's resolution of `name` from directory `rel` (relative to the lockfile's directory):
/// `<rel>/node_modules/<name>`, then each parent's, up to `node_modules/<name>`. A link entry
/// (a workspace) takes the version of the folder it resolves to.
fn lock_version(
    packages: &serde_json::Map<String, Value>,
    rel: &str,
    name: &str,
) -> Option<String> {
    let mut dir = Some(rel);
    while let Some(d) = dir {
        let key = if d.is_empty() {
            format!("node_modules/{name}")
        } else {
            format!("{d}/node_modules/{name}")
        };
        if let Some(entry) = packages.get(&key) {
            let entry = if entry.get("link").and_then(Value::as_bool) == Some(true) {
                packages.get(entry.get("resolved").and_then(Value::as_str)?)?
            } else {
                entry
            };
            return entry
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_owned);
        }
        dir = parent_dir(d);
    }
    None
}

/// A yarn blockade: spec names to pinned versions (`npm:` alias specs key
/// by their alias; the real name resolves through [`npm_alias`]).
struct YarnLock {
    versions: BTreeMap<String, Vec<String>>,
}

/// Whether yarn.lock text is Berry (`__metadata:` first) or v1.
fn is_yarn_berry_text(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim();
        !(l.is_empty() || l.starts_with('#')) && l == "__metadata:"
    })
}

/// Split a yarn spec: `alias@npm:real@range` → (`alias`, its real package);
/// anything else `name@range` → (`name`, none). `None` when malformed.
fn yarn_spec(spec: &str) -> Option<(String, Option<String>)> {
    if let Some((name, rest)) = spec.split_once("@npm:") {
        if name.is_empty() || name.contains('@') {
            return None;
        }
        let real = npm_alias(&format!("npm:{rest}")).map(str::to_owned)?;
        return Some((name.to_owned(), Some(real)));
    }
    let (name, range) = if let Some(rest) = spec.strip_prefix('@') {
        let at = rest.rfind('@')?;
        (format!("@{}", &rest[..at]), Some(&rest[at + 1..]))
    } else {
        let (name, range) = spec.split_once('@')?;
        (name.to_owned(), Some(range))
    };
    if name.is_empty() || range.is_some_and(str::is_empty) {
        return None;
    }
    Some((name, None))
}

/// Record one pin, deduped.
fn yarn_pin(versions: &mut BTreeMap<String, Vec<String>>, spec: &str, version: &str) {
    let versions = versions.entry(spec.to_owned()).or_default();
    if !versions.iter().any(|v| v == version) {
        versions.push(version.to_owned());
    }
}

/// A yarn.lock text (v1 blockade or Berry resolutions) to spec names and
/// versions. Never fails: garbage yields no blocks, and every dep then
/// reports a missing entry (safe `?`).
fn yarn_lock(text: &str) -> YarnLock {
    let mut versions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if is_yarn_berry_text(text) {
        // `"name@npm:^1":` entries pin through `resolution: "name@npm:1.2.3"`
        // (or the `version:` field when no resolution is stated); anything
        // but `@npm:` descriptors (workspace/file/link) stays out.
        let mut key: Option<String> = None;
        for line in text.lines() {
            let body = line.trim();
            if body.starts_with('"') && body.ends_with("\":") {
                key = Some(body[1..body.len() - 2].to_owned());
                continue;
            }
            let Some(k) = key.as_deref() else {
                continue;
            };
            if body.is_empty() {
                key = None;
                continue;
            }
            if let Some(rest) = body.strip_prefix("resolution:") {
                if let Some(r) = unquote(rest.trim())
                    && let Some((real, v)) = r.split_once("@npm:")
                    && !real.is_empty()
                    && !v.is_empty()
                    && let Some((spec, _)) = yarn_spec(k)
                {
                    yarn_pin(&mut versions, &spec, v);
                }
                continue;
            }
            if let Some(rest) = body.strip_prefix("version:") {
                // Only `@npm:` descriptors pin (workspace/file/link keys
                // never reach versions, even when they state one).
                if !k.contains("@npm:") {
                    continue;
                }
                if let Some(v) = unquote(rest.trim()) {
                    let Some((spec, _)) = yarn_spec(k) else {
                        continue;
                    };
                    yarn_pin(&mut versions, &spec, &v);
                }
            }
        }
        return YarnLock { versions };
    }
    // v1: unindented `"spec":` headers (comma-separated aliases share the
    // block) with an indented `version "x"` line each.
    let mut specs: Vec<String> = Vec::new();
    let mut version: Option<String> = None;
    let mut flush = |specs: &mut Vec<String>, version: &mut Option<String>| {
        if let Some(v) = version.take() {
            for spec in specs.drain(..) {
                let Some((name, _)) = yarn_spec(&spec) else {
                    continue;
                };
                yarn_pin(&mut versions, &name, &v);
            }
        } else {
            specs.clear();
        }
    };
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            // v1 `version "x"` and Berry `version: x` (colon optional); first
            // wins, so a nested dependency literally named `version` cannot
            // override the block's pin.
            if version.is_none()
                && let Some(v) = line
                    .trim()
                    .strip_prefix("version")
                    .and_then(|v| unquote(v.trim().trim_start_matches(':').trim()))
            {
                version = Some(v);
            }
            continue;
        }
        let body = line.trim();
        if body.is_empty() || body.starts_with('#') {
            flush(&mut specs, &mut version);
            continue;
        }
        if body.ends_with(':') {
            // A new header closes the open block.
            flush(&mut specs, &mut version);
            let header = body.strip_suffix(':').unwrap_or_default().trim();
            for part in header.split(',') {
                specs.push(part.trim().trim_matches('"').to_owned());
            }
            continue;
        }
        flush(&mut specs, &mut version);
    }
    flush(&mut specs, &mut version);
    YarnLock { versions }
}

/// A pnpm `packages:` map: package name to pinned versions. Only lockfile
/// major 9 parses (pnpm 9/10); anything else, a missing `packages:` map or
/// malformed YAML is unreadable, never inferred (contract M6e S5).
fn pnpm_lock(text: &str) -> Result<BTreeMap<String, Vec<String>>, String> {
    let mut lines = text.lines().peekable();
    // `lockfileVersion: '9.0'` (major 9, quoted or bare).
    let mut major = None;
    for line in lines.by_ref() {
        let body = line.trim();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        let Some(rest) = body.strip_prefix("lockfileVersion:") else {
            return Err("no lockfileVersion".to_owned());
        };
        let version = rest.trim().trim_matches('\'').trim_matches('"').to_owned();
        major = Some(version.split('.').next().unwrap_or_default().to_owned());
        break;
    }
    if major.as_deref() != Some("9") {
        return Err(format!(
            "unsupported lockfileVersion {}",
            major.unwrap_or_default()
        ));
    }
    // The `packages:` map: two-space `name@version:` keys (peer suffixes in
    // parens stripped, scoped names kept). Indented content lines ignored.
    let mut versions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut in_packages = false;
    let mut saw_packages = false;
    for line in lines {
        if line.starts_with([' ', '\t']) {
            if !in_packages {
                continue;
            }
            if line.starts_with("  ") && !line.starts_with("   ") {
                let key = line.trim().strip_suffix(':').unwrap_or_default();
                let key = key.trim_matches('\'').trim_matches('"');
                let key = key.split('(').next().unwrap_or_default();
                let Some((name, v)) = key.rsplit_once('@') else {
                    continue;
                };
                if name.is_empty() || !v.starts_with(|c: char| c.is_ascii_digit()) {
                    continue;
                }
                let versions = versions.entry(name.to_owned()).or_default();
                if !versions.iter().any(|e| e == v) {
                    versions.push(v.to_owned());
                }
            }
            continue;
        }
        let body = line.trim();
        if body.is_empty() || body.starts_with('#') {
            continue;
        }
        if body == "packages:" {
            in_packages = true;
            saw_packages = true;
            continue;
        }
        // Any other top-level key ends the map.
        if in_packages {
            break;
        }
        // Lines before `packages:` that are not the version are ignored.
    }
    if !saw_packages {
        return Err("no packages".to_owned());
    }
    Ok(versions)
}
/// A governing lockfile's kind: npm JSON, a yarn blockade, or a pnpm map.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LockKind {
    Npm,
    Yarn,
    Pnpm,
}

/// A governing lockfile, parsed.
enum Lock {
    Npm {
        path: String,
        packages: serde_json::Map<String, Value>,
        rel: String,
    },
    Yarn {
        path: String,
        lock: YarnLock,
    },
    Pnpm {
        path: String,
        versions: BTreeMap<String, Vec<String>>,
    },
}

/// Resolve one dependency against pinned reads: the single read pins
/// `Exact`, several distinct reads clear to `@?` (`Heuristic`, Maven
/// duplicate rule), none reports a missing entry (`unresolved`).
fn pin_reads(
    reads: &[String],
    from: &'static str,
    m: &str,
    dep: &Dep,
    service: &str,
    path: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> (String, &'static str, Certainty, Vec<String>) {
    let mut distinct: Vec<String> = Vec::new();
    for v in reads {
        if !distinct.contains(v) {
            distinct.push(v.clone());
        }
    }
    match distinct.as_slice() {
        [] => {
            diagnostics.push(diagnostic(
                "lockfile_missing_entry",
                m,
                Some(dep.span),
                json!({ "service": service, "name": dep.name, "lockfile": path }),
            ));
            (String::new(), "unresolved", Certainty::Exact, Vec::new())
        }
        [v] => (v.clone(), from, Certainty::Exact, Vec::new()),
        many => (String::new(), from, Certainty::Heuristic, many.to_vec()),
    }
}

/// The lockfile governing a service directory: the nearest directory holding one, npm-first
/// per directory (`package-lock.json` over `yarn.lock` over `pnpm-lock.yaml`, mirroring
/// `node_lock`), then up toward the root.
fn lockfile_for(service_dir: &str, paths: &BTreeSet<&str>) -> Option<(LockKind, String)> {
    let mut dir = Some(service_dir);
    while let Some(d) = dir {
        let prefix = dir_prefix(d);
        for (kind, name) in [
            (LockKind::Npm, "npm-shrinkwrap.json"),
            (LockKind::Npm, "package-lock.json"),
            (LockKind::Yarn, "yarn.lock"),
            (LockKind::Pnpm, "pnpm-lock.yaml"),
        ] {
            let path = format!("{prefix}{name}");
            if paths.contains(path.as_str()) {
                return Some((kind, path));
            }
        }
        dir = parent_dir(d);
    }
    None
}

/// The package an npm alias installs: `npm:bar@^1` → `bar`, `npm:@s/p@2` → `@s/p`.
/// The version catalog governing a Gradle build file: the nearest
/// `gradle/libs.versions.toml` at or above the build file's directory
/// (contract M6e S3; mirrors `lockfile_for`).
fn catalog_for(build_path: &str, paths: &BTreeSet<&str>) -> Option<String> {
    let mut dir = Some(build_path.rsplit_once('/').map_or("", |(d, _)| d));
    while let Some(d) = dir {
        let path = format!("{}gradle/libs.versions.toml", dir_prefix(d));
        if paths.contains(path.as_str()) {
            return Some(path);
        }
        dir = parent_dir(d);
    }
    None
}

fn npm_alias(declared: &str) -> Option<&str> {
    let spec = declared.trim().strip_prefix("npm:")?;
    let name = match spec.rfind('@') {
        Some(at) if at > 0 => &spec[..at],
        _ => spec,
    };
    (!name.is_empty()).then_some(name)
}

/// An npm range under which a workspace dependency always resolves to the workspace (S16):
/// `*`, empty, `workspace:…`, or `file:`/`link:` to `target`'s directory.
fn npm_range_is_local(range: &str, service_dir: &str, target_dir: &str) -> bool {
    let range = range.trim();
    if range.is_empty() || range == "*" || range.starts_with("workspace:") {
        return true;
    }
    range
        .strip_prefix("file:")
        .or_else(|| range.strip_prefix("link:"))
        .and_then(|rel| join(service_dir, rel))
        .is_some_and(|d| d == target_dir)
}

/// `go.mod`'s direct requirements (no `// indirect`), its `replace` directives
/// and its `exclude` directives.
#[derive(Debug, Default)]
struct GoMod {
    requires: Vec<(Dep, String)>,
    replaces: Vec<Replace>,
    excludes: Vec<(String, String)>,
}

/// `replace <module> [<version>] => <target> [<version>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Replace {
    module: String,
    /// Only this required version is replaced; `None`: every version.
    from_version: Option<String>,
    /// A module path, or a directory (`./x`, `../x`, `/x`) relative to the `go.mod`'s.
    target: String,
    to_version: Option<String>,
}

impl Replace {
    fn local(&self) -> bool {
        self.target.starts_with("./")
            || self.target.starts_with("../")
            || self.target.starts_with('/')
    }

    /// As written after the arrow (`attrs.replaced`).
    fn written(&self) -> String {
        match &self.to_version {
            Some(v) => format!("{} {v}", self.target),
            None => self.target.clone(),
        }
    }
}

impl GoMod {
    /// The `replace` that applies to `module` at `version`: one naming that version, else one
    /// naming none (Go's rule).
    fn replace_for(&self, module: &str, version: &str) -> Option<&Replace> {
        let of = |r: &&Replace| r.module == module;
        self.replaces
            .iter()
            .filter(of)
            .find(|r| r.from_version.as_deref() == Some(version))
            .or_else(|| {
                self.replaces
                    .iter()
                    .filter(of)
                    .find(|r| r.from_version.is_none())
            })
    }

    fn excluded(&self, module: &str, version: &str) -> bool {
        self.excludes
            .iter()
            .any(|(m, v)| m == module && v == version)
    }
}

fn go_mod(text: &str) -> GoMod {
    let mut out = GoMod::default();
    let mut block: Option<&str> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let raw = line.trim_end_matches(['\n', '\r']);
        let indirect = raw.contains("// indirect");
        let code = raw.split("//").next().unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let (directive, rest) = match block {
            Some(_) if code == ")" => {
                block = None;
                continue;
            }
            Some(d) => (d, code),
            None => {
                let (d, rest) = code.split_once(char::is_whitespace).unwrap_or((code, ""));
                let rest = rest.trim();
                if rest == "(" {
                    block = Some(match d {
                        "require" => "require",
                        "replace" => "replace",
                        _ => "other",
                    });
                    continue;
                }
                (d, rest)
            }
        };
        let unquote = |s: &str| s.trim_matches('"').to_owned();
        match directive {
            "require" if !indirect => {
                let mut words = rest.split_whitespace();
                if let (Some(module), Some(version)) = (words.next(), words.next()) {
                    let at = start + raw.find(module).unwrap_or(0);
                    out.requires.push((
                        Dep {
                            name: unquote(module),
                            declared: version.to_owned(),
                            scope: "runtime",
                            span: span_of(text, at, at + module.len()),
                        },
                        version.to_owned(),
                    ));
                }
            }
            "replace" => {
                if let Some((from, to)) = rest.split_once("=>") {
                    let mut from = from.split_whitespace().map(unquote);
                    let mut to = to.split_whitespace().map(unquote);
                    if let (Some(module), Some(target)) = (from.next(), to.next()) {
                        out.replaces.push(Replace {
                            module,
                            from_version: from.next(),
                            target,
                            to_version: to.next(),
                        });
                    }
                }
            }
            "exclude" => {
                let mut words = rest.split_whitespace().map(unquote);
                if let (Some(module), Some(version)) = (words.next(), words.next()) {
                    out.excludes.push((module, version));
                }
            }
            _ => {}
        }
    }
    out
}

/// A `go.work` workspace: `use` member directories, workspace `replace`
/// directives (which win over a member module's own) and `exclude` directives
/// (contract M6e S8). Only the allow-listed directives are read; `go`,
/// `toolchain` and anything else are ignored.
#[derive(Debug, Default)]
struct GoWork {
    uses: Vec<String>,
    replaces: Vec<Replace>,
    excludes: Vec<(String, String)>,
}

impl GoWork {
    /// The `replace` that applies to `module` at `version`: one naming that version, else one
    /// naming none (Go's rule; mirrors [`GoMod::replace_for`]).
    fn replace_for(&self, module: &str, version: &str) -> Option<&Replace> {
        let of = |r: &&Replace| r.module == module;
        self.replaces
            .iter()
            .filter(of)
            .find(|r| r.from_version.as_deref() == Some(version))
            .or_else(|| {
                self.replaces
                    .iter()
                    .filter(of)
                    .find(|r| r.from_version.is_none())
            })
    }

    fn excluded(&self, module: &str, version: &str) -> bool {
        self.excludes
            .iter()
            .any(|(m, v)| m == module && v == version)
    }
}

fn go_work(text: &str) -> GoWork {
    let mut out = GoWork::default();
    let mut block: Option<&str> = None;
    for line in text.split_inclusive('\n') {
        let raw = line.trim_end_matches(['\n', '\r']);
        let code = raw.split("//").next().unwrap_or("").trim();
        if code.is_empty() {
            continue;
        }
        let (directive, rest) = match block {
            Some(_) if code == ")" => {
                block = None;
                continue;
            }
            Some(d) => (d, code),
            None => {
                let (d, rest) = code.split_once(char::is_whitespace).unwrap_or((code, ""));
                let rest = rest.trim();
                if rest == "(" {
                    block = Some(match d {
                        "use" => "use",
                        "replace" => "replace",
                        "exclude" => "exclude",
                        _ => "other",
                    });
                    continue;
                }
                (d, rest)
            }
        };
        let unquote = |s: &str| s.trim_matches('"').to_owned();
        match directive {
            "use" => {
                if let Some(dir) = rest.split_whitespace().next() {
                    out.uses.push(unquote(dir));
                }
            }
            "replace" => {
                if let Some((from, to)) = rest.split_once("=>") {
                    let mut from = from.split_whitespace().map(unquote);
                    let mut to = to.split_whitespace().map(unquote);
                    if let (Some(module), Some(target)) = (from.next(), to.next()) {
                        out.replaces.push(Replace {
                            module,
                            from_version: from.next(),
                            target,
                            to_version: to.next(),
                        });
                    }
                }
            }
            "exclude" => {
                let mut words = rest.split_whitespace().map(unquote);
                if let (Some(module), Some(version)) = (words.next(), words.next()) {
                    out.excludes.push((module, version));
                }
            }
            _ => {}
        }
    }
    out
}

/// The `go.work` governing a service directory: the nearest `go.work` at or
/// above it (contract M6e S8; mirrors `catalog_for`).
fn go_work_for(service_dir: &str, paths: &BTreeSet<&str>) -> Option<String> {
    let mut dir = Some(service_dir);
    while let Some(d) = dir {
        let path = format!("{}go.work", dir_prefix(d));
        if paths.contains(path.as_str()) {
            return Some(path);
        }
        dir = parent_dir(d);
    }
    None
}

/// A PEP 503 canonical project name: lowercased with every run of `-_.`
/// folded to one `-`.
fn pep503(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut in_sep = false;
    for c in name.to_lowercase().chars() {
        if c == '-' || c == '_' || c == '.' {
            if !in_sep {
                out.push('-');
                in_sep = true;
            }
        } else {
            out.push(c);
            in_sep = false;
        }
    }
    out
}

/// Whether `name` is shaped like a dependency name (leading alphanumerics,
/// then alphanumerics and `-_.`).
fn valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// A PEP 508 requirement string: (canonical name, declared spec as written,
/// raw name length for evidence). Extras stay out of the name, markers after
/// `;` and URL targets after `@` never become versions; a parenthesised spec
/// is tolerated. `None` for anything without a leading name.
fn pep508(req: &str) -> Option<(String, String, usize)> {
    let head = req.split(';').next()?.trim();
    if head.is_empty() {
        return None;
    }
    // A direct reference (`name[extras]@url`): the version is never pinned.
    if let Some((name, _)) = head.split_once('@') {
        let name = name.split('[').next()?.trim();
        if !valid_name(name) {
            return None;
        }
        return Some((pep503(name), String::new(), name.len()));
    }
    let len = head
        .char_indices()
        .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '-' || *c == '_' || *c == '.')
        .map(|(i, c)| i + c.len_utf8())
        .last()?;
    let name = &head[..len];
    if !valid_name(name) {
        return None;
    }
    let mut rest = head[len..].trim_start();
    if let Some(after) = rest.strip_prefix('[') {
        rest = after.find(']').map(|end| &after[end + 1..])?.trim_start();
    }
    let rest = rest.trim();
    let declared = rest
        .strip_prefix('(')
        .and_then(|r| r.strip_suffix(')'))
        .unwrap_or(rest)
        .trim()
        .to_owned();
    Some((pep503(name), declared, len))
}

/// One direct runtime dependency from a `pyproject.toml`, with its evidence span.
struct PyDep {
    name: String,
    declared: String,
    span: Span,
}

/// The span of TOML key `key` at/after `from` (bare, single- or
/// double-quoted); the key's own characters. Deterministic for any text.
fn key_span(text: &str, key: &str, from: usize) -> Span {
    let spellings = [key.to_string(), format!("\"{key}\""), format!("'{key}'")];
    let mut found: Option<(usize, usize)> = None;
    for spelling in &spellings {
        let mut at = from;
        while let Some(i) = text.get(at..).and_then(|rest| rest.find(spelling.as_str())) {
            let start = at + i;
            at = start + 1;
            let after = &text[start + spelling.len()..];
            let is_bare = !spelling.starts_with(['"', '\'']);
            let line_start = text[..start].rfind('\n').map_or(0, |n| n + 1);
            // A key is followed by `=`; a bare key also starts its line.
            if !after.trim_start_matches([' ', '\t']).starts_with('=') {
                continue;
            }
            if is_bare && start != 0 && !text[line_start..start].trim().is_empty() {
                continue;
            }
            let inner = if is_bare {
                (start, start + key.len())
            } else {
                (start + 1, start + 1 + key.len())
            };
            if found.is_none_or(|(s, _)| inner.0 < s) {
                found = Some(inner);
            }
            break;
        }
    }
    match found {
        Some((start, end)) => span_of(text, start, end),
        None => span_of(text, from, from),
    }
}

/// Direct runtime dependencies: `[project].dependencies` (PEP 508 strings)
/// and `[tool.poetry.dependencies]` (constraints or inline tables; `python`
/// is the runtime). Poetry groups, extras and optional dependencies are out
/// of scope (contract M6d S6). Unparseable entries name no dependency.
fn pyproject_deps(text: &str) -> Vec<PyDep> {
    let table: toml::Table = match toml::from_str(text) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    if let Some(deps) = table
        .get("project")
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_array())
    {
        let from = text.find("[project]").unwrap_or(0);
        for item in deps {
            let Some(req) = item.as_str() else {
                continue;
            };
            let Some((name, declared, len)) = pep508(req) else {
                continue;
            };
            let start = text[from..].find(req).map(|i| from + i).unwrap_or(from);
            out.push(PyDep {
                name,
                declared,
                span: span_of(text, start, start + len),
            });
        }
    }
    if let Some(deps) = table
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|p| p.get("dependencies"))
        .and_then(|d| d.as_table())
    {
        let from = text.find("[tool.poetry.dependencies]").unwrap_or(0);
        for (key, value) in deps {
            if key == "python" {
                continue;
            }
            // A constraint string, an inline table's `version`, or the first
            // of several: anything else (a bare git/path/url table, an empty
            // list) declares no pin.
            let declared = match value {
                toml::Value::String(s) => s.clone(),
                toml::Value::Table(t) => t
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                toml::Value::Array(items) => items
                    .iter()
                    .find_map(|item| match item {
                        toml::Value::String(s) => Some(s.clone()),
                        toml::Value::Table(t) => {
                            t.get("version").and_then(|v| v.as_str()).map(str::to_owned)
                        }
                        _ => None,
                    })
                    .unwrap_or_default(),
                _ => continue,
            };
            out.push(PyDep {
                name: pep503(key),
                declared,
                span: key_span(text, key, from),
            });
        }
    }
    out
}

/// name → version for registry-source `[[package]]` entries; `Err` when the
/// TOML does not parse or names no packages. A git/directory/editable/url
/// source never pins (contract M6d S6: lockfiles pin, never infer).
fn poetry_lock(text: &str) -> Result<BTreeMap<String, String>, String> {
    let table: toml::Table = toml::from_str(text).map_err(|_| "invalid TOML".to_owned())?;
    let mut out = BTreeMap::new();
    let pkgs = table
        .get("package")
        .and_then(|p| p.as_array())
        .ok_or_else(|| "no packages".to_owned())?;
    for entry in pkgs {
        let (Some(name), Some(version)) = (
            entry.get("name").and_then(|n| n.as_str()),
            entry.get("version").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let pinnable = match entry.get("source") {
            None => true,
            Some(source) => source
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t == "legacy"),
        };
        if pinnable {
            out.insert(pep503(name), version.to_owned());
        }
    }
    Ok(out)
}

/// name → version for registry-source `[[package]]` entries of a `uv.lock`
/// (same pinnability rule as [`poetry_lock`]: a `registry` source, or none).
fn uv_lock(text: &str) -> Result<BTreeMap<String, String>, String> {
    let table: toml::Table = toml::from_str(text).map_err(|_| "invalid TOML".to_owned())?;
    let mut out = BTreeMap::new();
    let pkgs = table
        .get("package")
        .and_then(|p| p.as_array())
        .ok_or_else(|| "no packages".to_owned())?;
    for entry in pkgs {
        let (Some(name), Some(version)) = (
            entry.get("name").and_then(|n| n.as_str()),
            entry.get("version").and_then(|v| v.as_str()),
        ) else {
            continue;
        };
        let pinnable = match entry.get("source") {
            None => true,
            Some(source) => source
                .as_table()
                .is_some_and(|s| s.contains_key("registry")),
        };
        if pinnable {
            out.insert(pep503(name), version.to_owned());
        }
    }
    Ok(out)
}

/// Which lockfile's versions pin a service's Python dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LockSource {
    Poetry,
    Uv,
}

/// A lockfile relevant to a service: one that pins, or one noticed but never
/// read (contract M4b: `Pipfile`/`requirements` pinning is residue).
#[derive(Debug, Clone, PartialEq, Eq)]
enum PyLock {
    Read(String, LockSource),
    Unread(String),
}

/// The `[tool.uv.workspace] members` of a `pyproject.toml`, if any.
fn uv_workspace_members(text: &str) -> Vec<String> {
    toml::from_str::<toml::Table>(text)
        .ok()
        .and_then(|t| {
            t.get("tool")?
                .get("uv")?
                .get("workspace")?
                .get("members")?
                .as_array()
                .cloned()
        })
        .unwrap_or_default()
        .iter()
        .filter_map(|m| m.as_str().map(str::to_owned))
        .collect()
}

/// Whether workspace member pattern `member` (`backend`, `packages/*`)
/// selects `rel` (the service directory relative to the workspace root).
fn workspace_member(member: &str, rel: &str) -> bool {
    if let Some(prefix) = member.strip_suffix("/*") {
        rel.strip_prefix(prefix)
            .is_some_and(|r| !r.is_empty() && !r.trim_start_matches('/').contains('/'))
    } else {
        member.trim_end_matches('/') == rel
    }
}

/// The lockfile governing `service_dir` (the `here` form: `""` for the root):
/// a same-directory `poetry.lock` (winning ties), then `uv.lock`, then an
/// unread shape; then the nearest ancestor `uv.lock` whose workspace lists
/// the service, then ancestors' unread shapes. A `poetry.lock` above never
/// governs below (poetry has no workspaces).
fn py_lockfile_for(
    service_dir: &str,
    paths: &BTreeSet<&str>,
    texts: &BTreeMap<&str, String>,
) -> Option<PyLock> {
    let at = |dir: &str, name: &str| {
        let prefix = dir_prefix(dir);
        let path = format!("{prefix}{name}");
        paths.contains(path.as_str()).then_some(path)
    };
    const UNREAD: [&str; 3] = ["requirements.txt", "Pipfile.lock", "Pipfile"];
    if let Some(path) = at(service_dir, "poetry.lock") {
        return Some(PyLock::Read(path, LockSource::Poetry));
    }
    if let Some(path) = at(service_dir, "uv.lock") {
        return Some(PyLock::Read(path, LockSource::Uv));
    }
    for name in UNREAD {
        if let Some(path) = at(service_dir, name) {
            return Some(PyLock::Unread(path));
        }
    }
    let mut dir = parent_dir(service_dir);
    while let Some(d) = dir {
        if let Some(path) = at(d, "uv.lock") {
            let manifest = if d.is_empty() {
                "pyproject.toml".to_owned()
            } else {
                format!("{d}/pyproject.toml")
            };
            let rel = if d.is_empty() {
                service_dir.to_owned()
            } else {
                service_dir
                    .strip_prefix(&format!("{d}/"))
                    .unwrap_or(service_dir)
                    .to_owned()
            };
            let member = texts
                .get(manifest.as_str())
                .map(|t| {
                    uv_workspace_members(t)
                        .iter()
                        .any(|m| workspace_member(m, &rel))
                })
                .unwrap_or(false);
            if member {
                return Some(PyLock::Read(path, LockSource::Uv));
            }
        }
        for name in UNREAD {
            if let Some(path) = at(d, name) {
                return Some(PyLock::Unread(path));
            }
        }
        dir = parent_dir(d);
    }
    None
}
#[derive(Debug, Default)]
/// A minimal XML element: name, direct text, children, and where its start tag begins.
struct Element {
    name: String,
    text: String,
    children: Vec<Element>,
    start: usize,
}

impl Element {
    fn child(&self, name: &str) -> Option<&Element> {
        self.children.iter().find(|c| c.name == name)
    }

    fn text_of(&self, name: &str) -> Option<&str> {
        self.child(name).map(|c| c.text.trim())
    }

    /// The distinct texts of every child named `name`, in order.
    fn texts_of(&self, name: &str) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for c in self.children.iter().filter(|c| c.name == name) {
            let t = c.text.trim();
            if !out.contains(&t) {
                out.push(t);
            }
        }
        out
    }
}

/// Parses an XML document into its root element; comments, processing instructions and
/// declarations are skipped, CDATA is text, attributes are ignored. `None` when the tags do not
/// balance.
fn parse_xml(text: &str) -> Option<Element> {
    let mut stack: Vec<Element> = vec![Element::default()];
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < text.len() {
        if bytes[i] != b'<' {
            let next = text[i..].find('<').map_or(text.len(), |n| i + n);
            stack.last_mut()?.text.push_str(&decode(&text[i..next]));
            i = next;
            continue;
        }
        let rest = &text[i..];
        if let Some(body) = rest.strip_prefix("<!--") {
            i += 4 + body.find("-->")? + 3;
        } else if let Some(body) = rest.strip_prefix("<![CDATA[") {
            let end = body.find("]]>")?;
            stack.last_mut()?.text.push_str(&body[..end]);
            i += 9 + end + 3;
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            i += rest.find('>')? + 1;
        } else if let Some(body) = rest.strip_prefix("</") {
            let end = body.find('>')?;
            let name = body[..end].trim();
            let done = stack.pop()?;
            if done.name != name || stack.is_empty() {
                return None;
            }
            stack.last_mut()?.children.push(done);
            i += 2 + end + 1;
        } else {
            let end = rest.find('>')?;
            let tag = &rest[1..end];
            let closed = tag.ends_with('/');
            let name = tag
                .trim_end_matches('/')
                .split(|c: char| c.is_whitespace())
                .next()?
                .to_owned();
            let element = Element {
                name,
                start: i,
                ..Element::default()
            };
            if closed {
                stack.last_mut()?.children.push(element);
            } else {
                stack.push(element);
            }
            i += end + 1;
        }
    }
    let mut document = stack.pop()?;
    if !stack.is_empty() {
        return None;
    }
    document.children.pop()
}

fn decode(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// A pom's coordinates and direct dependencies with their versions read (D7).
#[derive(Debug, Default)]
struct Pom {
    group: String,
    artifact: String,
    version: String,
    /// Each dependency (named `<groupId>/<artifactId>`) with its version, empty when
    /// unresolved, the version's source (`literal`, `property` or `unresolved`), and the
    /// conflicting versions when the artifact is declared twice, or its property defined twice,
    /// with different values (evaluator pass 1, Y8, Y11).
    deps: Vec<(Dep, String, &'static str, Vec<String>)>,
}

/// A `${…}` reference in a pom, read.
enum Prop<'a> {
    Value(&'a str),
    /// Defined more than once with different values.
    Conflict(Vec<&'a str>),
    Unknown,
}

/// The value of `${name}` in this pom: a `<properties>` literal, or `project.version`,
/// `project.groupId`, `project.artifactId`. A property defined twice with different values is
/// a conflict (Y11).
fn property<'a>(project: &'a Element, coords: [(&str, &'a str); 3], text: &str) -> Prop<'a> {
    let Some(name) = text.strip_prefix("${").and_then(|t| t.strip_suffix('}')) else {
        return Prop::Unknown;
    };
    let usable = |v: &str| !v.is_empty() && !v.contains("${");
    if let Some((_, v)) = coords.iter().find(|(k, _)| *k == name) {
        return if usable(v) {
            Prop::Value(v)
        } else {
            Prop::Unknown
        };
    }
    let values = project
        .child("properties")
        .map(|p| p.texts_of(name))
        .unwrap_or_default();
    match values.as_slice() {
        [] => Prop::Unknown,
        [v] if usable(v) => Prop::Value(v),
        [_] => Prop::Unknown,
        _ => Prop::Conflict(values),
    }
}

fn pom(text: &str) -> Option<Pom> {
    let project = parse_xml(text)?;
    if project.name != "project" {
        return None;
    }
    let parent = project.child("parent");
    let inherited = |name: &str| {
        project
            .text_of(name)
            .or_else(|| parent.and_then(|p| p.text_of(name)))
            .unwrap_or_default()
            .to_owned()
    };
    let (group, artifact, version) = (
        inherited("groupId"),
        project.text_of("artifactId").unwrap_or_default().to_owned(),
        inherited("version"),
    );
    let coords = [
        ("project.version", version.as_str()),
        ("project.groupId", group.as_str()),
        ("project.artifactId", artifact.as_str()),
    ];
    let mut deps = Vec::new();
    for d in project
        .child("dependencies")
        .into_iter()
        .flat_map(|ds| ds.children.iter().filter(|c| c.name == "dependency"))
    {
        let resolve = |raw: &str| match property(&project, coords, raw) {
            Prop::Value(v) => v.to_owned(),
            Prop::Conflict(_) | Prop::Unknown => raw.to_owned(),
        };
        let group = resolve(d.text_of("groupId").unwrap_or_default());
        let artifact = resolve(d.text_of("artifactId").unwrap_or_default());
        if group.is_empty() || artifact.is_empty() {
            continue;
        }
        let declared = d.text_of("version").unwrap_or_default();
        let mut conflict = Vec::new();
        let (version, from) = if declared.is_empty() {
            (String::new(), "unresolved")
        } else if declared.starts_with("${") {
            match property(&project, coords, declared) {
                Prop::Value(v) => (v.to_owned(), "property"),
                Prop::Conflict(vs) => {
                    conflict = vs.into_iter().map(str::to_owned).collect();
                    (String::new(), "unresolved")
                }
                Prop::Unknown => (String::new(), "unresolved"),
            }
        } else if declared.contains("${") || declared.starts_with(['[', '(']) {
            (String::new(), "unresolved")
        } else {
            (declared.to_owned(), "literal")
        };
        let scope = match d.text_of("scope").unwrap_or_default() {
            "test" => "test",
            "provided" | "system" => "provided",
            _ if d.text_of("optional") == Some("true") => "optional",
            _ => "runtime",
        };
        let at = d.child("artifactId").map_or(d.start, |a| a.start);
        let end = text[at..].find('\n').map_or(text.len(), |n| at + n);
        deps.push((
            Dep {
                name: format!("{group}/{artifact}"),
                declared: declared.to_owned(),
                scope,
                span: span_of(text, at, end),
            },
            version,
            from,
            conflict,
        ));
    }
    // An artifact declared twice: Maven keeps one declaration (and warns), so with different
    // versions none is the stated one (Y8). The first declaration stays, unresolved, listing
    // every version read.
    let mut versions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (dep, version, _, conflict) in &deps {
        let read = if conflict.is_empty() {
            vec![if !version.is_empty() {
                version.clone()
            } else if !dep.declared.is_empty() {
                dep.declared.clone()
            } else {
                "?".to_owned()
            }]
        } else {
            conflict.clone()
        };
        let seen = versions.entry(dep.name.clone()).or_default();
        for v in read {
            if !seen.contains(&v) {
                seen.push(v);
            }
        }
    }
    let mut kept = BTreeSet::new();
    deps.retain(|(dep, _, _, _)| kept.insert(dep.name.clone()));
    for (dep, version, from, conflict) in &mut deps {
        let read = &versions[&dep.name];
        if read.len() > 1 {
            version.clear();
            *from = "unresolved";
            conflict.clone_from(read);
        }
    }
    Some(Pom {
        group: group.clone(),
        artifact: artifact.clone(),
        version: version.clone(),
        deps,
    })
}

fn attrs(pairs: &[(&str, &str)]) -> Attrs {
    pairs
        .iter()
        .filter(|(_, v)| !v.is_empty())
        .map(|(k, v)| ((*k).to_owned(), Value::from(*v)))
        .collect()
}

/// A Gradle version catalog (`gradle/libs.versions.toml`): alias → (module
/// `group:artifact`, version if the catalog states one). Versions resolve
/// through `[versions]`; a dangling `version.ref` names the module with no
/// version (safe `?` at use, never an error). Anything outside the
/// allow-listed shapes makes the whole catalog unreadable (contract M6e S3:
/// deny-default on structure, lenient on forward-compatible additions —
/// unknown root sections like `[plugins]`/`[bundles]` are ignored, while a
/// malformed `[libraries]` entry fails the read).
fn gradle_catalog(text: &str) -> Result<BTreeMap<String, (String, Option<String>)>, String> {
    let table: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut versions: BTreeMap<String, String> = BTreeMap::new();
    if let Some(v) = table.get("versions") {
        let v = v
            .as_table()
            .ok_or_else(|| "versions is not a table".to_owned())?;
        // Rich versions (`lang = { strictly = … }`) are deliberately not
        // resolved: the entry is skipped, so references to it dangle to `?`
        // instead of taking the whole catalog down with them.
        for (k, val) in v {
            if let Some(s) = val.as_str() {
                versions.insert(k.clone(), s.to_owned());
            }
        }
    }
    fn libraries(
        table: &toml::Table,
        prefix: &str,
        versions: &BTreeMap<String, String>,
        out: &mut BTreeMap<String, (String, Option<String>)>,
    ) -> Result<(), String> {
        for (k, val) in table {
            let alias = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match val {
                toml::Value::String(s) => {
                    let mut parts = s.split(':');
                    match (parts.next(), parts.next(), parts.next(), parts.next()) {
                        (Some(g), Some(a), v, None) => {
                            out.insert(alias, (format!("{g}:{a}"), v.map(str::to_owned)));
                        }
                        _ => {
                            return Err(format!(
                                "library {alias:?} is not group:artifact[:version]"
                            ));
                        }
                    }
                }
                toml::Value::Table(t) if t.contains_key("module") => {
                    let module = t
                        .get("module")
                        .and_then(toml::Value::as_str)
                        .ok_or_else(|| format!("library {alias:?} has no string module"))?;
                    if module.split(':').count() != 2 {
                        return Err(format!("library {alias:?} module is not group:artifact"));
                    }
                    // `version = "x"` wins; a dotted `version.ref` parses as a
                    // nested table; anything else dangles to `?`.
                    let version = match t.get("version") {
                        Some(toml::Value::String(v)) => Some(v.clone()),
                        Some(toml::Value::Table(t)) => match t.get("ref") {
                            Some(toml::Value::String(r)) => versions.get(r).cloned(),
                            _ => None,
                        },
                        _ => match t.get("version.ref") {
                            Some(toml::Value::String(r)) => versions.get(r).cloned(),
                            _ => None,
                        },
                    };
                    out.insert(alias, (module.to_owned(), version));
                }
                toml::Value::Table(t) => libraries(t, &alias, versions, out)?,
                _ => {
                    return Err(format!("library {alias:?} is not a string or table"));
                }
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    if let Some(l) = table.get("libraries") {
        libraries(
            l.as_table()
                .ok_or_else(|| "libraries is not a table".to_owned())?,
            "",
            &versions,
            &mut out,
        )?;
    }
    Ok(out)
}

/// One Gradle dependency with its resolved version, mirroring `Pom.deps`.
struct GradleDep {
    dep: Dep,
    version: String,
    from: &'static str,
    conflict: Vec<String>,
}

/// Dependency configurations read, with the scope each reports (contract
/// M6e S3: allow-listed; custom configurations stay out).
const GRADLE_CONFIGS: &[(&str, &str)] = &[
    ("testImplementation", "test"),
    ("testRuntimeOnly", "test"),
    ("implementation", "runtime"),
    ("compileOnly", "compile"),
    ("runtimeOnly", "runtime"),
    ("api", "runtime"),
];

/// `line` without a `//` comment (a `//` inside quotes is kept). Byte-safe:
/// only ASCII bytes decide, and a multibyte character never contains one.
fn strip_line_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if escaped {
            escaped = false;
        } else if quote.is_none() && b == b'\\' {
            escaped = true;
        } else if quote.is_none() && (b == b'"' || b == b'\'') {
            quote = Some(b);
        } else if Some(b) == quote {
            quote = None;
        } else if quote.is_none() && b == b'/' && bytes.get(i + 1) == Some(&b'/') {
            return line.get(..i).unwrap_or(line);
        }
        i += 1;
    }
    line
}

/// The content of the first balanced parenthesis group of `s` (which starts
/// just after the opening paren), skipping quoted spans; `None` when
/// unbalanced. Byte-safe: never splits a character, returns `None` instead.
fn paren_group(s: &str) -> Option<&str> {
    let mut depth = 1u32;
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    for (i, b) in s.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, b) {
            (None, b'\\') => escaped = true,
            (None, b'(') => depth += 1,
            (None, b')') => {
                depth -= 1;
                if depth == 0 {
                    return s.get(..i);
                }
            }
            (None, b'"') | (None, b'\'') => quote = Some(b),
            (Some(q), b) if b == q => quote = None,
            _ => {}
        }
    }
    None
}

/// A quoted string's content (`"…"`, `'…`), or the bare token itself.
fn unquote(token: &str) -> Option<String> {
    let token = token.trim();
    if token.len() >= 2
        && ((token.starts_with('"') && token.ends_with('"'))
            || (token.starts_with('\'') && token.ends_with('\'')))
    {
        Some(token[1..token.len() - 1].to_owned())
    } else if token
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_' | b'$'))
    {
        Some(token.to_owned())
    } else {
        None
    }
}

/// A `group:artifact[:version]` coordinate, or `None` outside the shape.
/// An interpolated (`$`) version resolves to nothing (safe `?`).
fn gav(text: &str) -> Option<(String, String, Option<String>)> {
    let mut parts = text.split(':');
    match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(g), Some(a), v, None)
            if !g.is_empty() && !a.is_empty() && !g.contains('$') && !a.contains('$') =>
        {
            let version = match v {
                Some(v) if !v.is_empty() && !v.contains('$') => Some(v.to_owned()),
                _ => None,
            };
            Some((
                format!("{g}/{a}"),
                v.unwrap_or_default().to_owned(),
                version,
            ))
        }
        _ => None,
    }
}

/// Split on top-level commas (commas inside quotes do not split); `None`
/// when quotes are unbalanced.
fn split_top_level(text: &str) -> Option<Vec<&str>> {
    let mut parts = Vec::new();
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    let mut start = 0;
    for (i, b) in text.bytes().enumerate() {
        if escaped {
            escaped = false;
        } else if quote.is_none() && b == b'\\' {
            escaped = true;
        } else if quote.is_none() && (b == b'"' || b == b'\'') {
            quote = Some(b);
        } else if Some(b) == quote {
            quote = None;
        } else if quote.is_none() && b == b',' {
            parts.push(text.get(start..i)?);
            start = i + 1;
        }
    }
    if quote.is_some() {
        return None;
    }
    parts.push(text.get(start..)?);
    Some(parts)
}

/// Whether `text` holds a top-level `:` or `=` (outside quotes): a map or
/// named-argument form rather than a single coordinate.
fn has_top_level_pair(text: &str) -> bool {
    let mut quote: Option<u8> = None;
    let mut escaped = false;
    for b in text.bytes() {
        if escaped {
            escaped = false;
        } else if quote.is_none() && b == b'\\' {
            escaped = true;
        } else if quote.is_none() && (b == b'"' || b == b'\'') {
            quote = Some(b);
        } else if Some(b) == quote {
            quote = None;
        } else if quote.is_none() && (b == b':' || b == b'=') {
            return true;
        }
    }
    false
}

/// One dependency spec: (name `group/artifact`, declared version expression,
/// resolved version, source). `None` outside the allow-listed shapes
/// (contract M6e S3).
fn gradle_spec(
    spec: &str,
    catalog: &BTreeMap<String, (String, Option<String>)>,
) -> Option<(String, String, Option<String>, &'static str)> {
    let spec = spec.trim();
    // One `platform()`/`enforcedPlatform()` layer unwraps to the platform
    // artifact itself; deeper nesting and `project()` references stay out.
    let mut inner = spec;
    for prefix in ["platform", "enforcedPlatform"] {
        if let Some(rest) = inner.strip_prefix(prefix) {
            let rest = rest.trim_start();
            inner = paren_group(rest.strip_prefix('(')?)?;
            let nested = inner.trim_start();
            if nested.starts_with("platform(") || nested.starts_with("enforcedPlatform(") {
                return None;
            }
            break;
        }
    }
    if inner.trim_start().starts_with("project(") {
        return None;
    }
    // A `libs.` catalog reference (bare or parenthesised; the paren group
    // already isolated it).
    let token = inner.trim();
    if let Some(alias) = token.strip_prefix("libs.") {
        if !alias
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_')
            || alias.is_empty()
        {
            return None;
        }
        return match catalog.get(alias) {
            Some((module, version)) => {
                let mut parts = module.split(':');
                match (parts.next(), parts.next(), parts.next()) {
                    (Some(g), Some(a), None) => Some((
                        format!("{g}/{a}"),
                        token.to_owned(),
                        version.clone(),
                        if version.is_some() {
                            "catalog"
                        } else {
                            "unresolved"
                        },
                    )),
                    _ => None,
                }
            }
            None => None,
        };
    }
    // Map / named-argument form (`group: 'g', name: 'a', version: 'v'`).
    if has_top_level_pair(token) {
        let parts = split_top_level(token)?;
        let mut group = None;
        let mut name = None;
        let mut module = None;
        let mut version = None;
        for part in parts {
            let mut kv = part.splitn(2, [':', '=']);
            let (k, v) = (kv.next()?.trim(), kv.next()?.trim());
            let k = unquote(k)?;
            let v = unquote(v)?;
            match k.as_str() {
                "group" => group = Some(v),
                "name" => name = Some(v),
                "module" => module = Some(v),
                "version" => version = Some(v),
                _ => return None,
            }
        }
        let (g, a) = match (group, name, module) {
            (Some(g), Some(a), _) if !g.contains('$') && !a.contains('$') => (g, a),
            (None, None, Some(m)) => match m.split_once(':') {
                Some((g, a)) if !g.contains('$') && !a.contains('$') => {
                    (g.to_owned(), a.to_owned())
                }
                _ => return None,
            },
            _ => return None,
        };
        return match version {
            Some(v) if !v.is_empty() && !v.contains('$') => {
                Some((format!("{g}/{a}"), v.clone(), Some(v), "literal"))
            }
            _ => Some((
                format!("{g}/{a}"),
                version.unwrap_or_default(),
                None,
                "unresolved",
            )),
        };
    }
    // Triple form (`"g", "a", "v"`) or a single coordinate.
    if token.contains(',') {
        let parts = split_top_level(token)?;
        if parts.len() != 2 && parts.len() != 3 {
            return None;
        }
        let gav: Vec<String> = parts
            .iter()
            .map(|p| unquote(p.trim()).unwrap_or_default())
            .collect();
        let (g, a) = (gav[0].clone(), gav[1].clone());
        if g.is_empty() || a.is_empty() || g.contains('$') || a.contains('$') {
            return None;
        }
        return match gav.get(2).map(String::as_str) {
            None | Some("") => Some((format!("{g}/{a}"), String::new(), None, "unresolved")),
            Some(v) if v.contains('$') => {
                Some((format!("{g}/{a}"), v.to_owned(), None, "unresolved"))
            }
            Some(v) => Some((
                format!("{g}/{a}"),
                v.to_owned(),
                Some(v.to_owned()),
                "literal",
            )),
        };
    }
    // A single `group:artifact[:version]` string.
    let token = unquote(token)?;
    let (name, declared, version) = gav(&token)?;
    let from = if version.is_some() {
        "literal"
    } else {
        "unresolved"
    };
    Some((name, declared, version, from))
}

/// The dependencies a Gradle build file declares (both DSLs): literals,
/// `libs.` catalog references and `platform()`-wrapped literals.
/// `project()` references, multi-line blocks and anything outside the
/// allow-listed shapes stay out (contract M6e S3). Duplicates mirror Maven:
/// the first declaration stays, cleared to `@?` when versions differ.
fn gradle_deps(text: &str, catalog: &BTreeMap<String, (String, Option<String>)>) -> Vec<GradleDep> {
    let mut deps: Vec<GradleDep> = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let stripped = strip_line_comment(line);
        let body = stripped.trim_start();
        if body.is_empty() {
            continue;
        }
        // Longest-first: `testImplementation` wins over `implementation`.
        // No trailing-boundary check: followers are strict (a parenthesis
        // or an exact spec), so a longer identifier can never parse — proven
        // when its mutant survived (contract M6e amendment 4), and the
        // `implementationX` row below pins the behavior.
        let Some((config, scope)) = GRADLE_CONFIGS.iter().find(|&(c, _)| body.starts_with(c))
        else {
            continue;
        };
        let kw_start = start + (stripped.len() - body.len());
        let rest = body[config.len()..].trim_start();
        let rest_start = kw_start + config.len() + (body[config.len()..].len() - rest.len());
        // Parenthesised form: the first balanced group; bare form: the rest
        // of the line must be exactly one spec.
        let (spec, spec_end) = if let Some(after) = rest.strip_prefix('(') {
            let Some(group) = paren_group(after) else {
                continue;
            };
            let end = rest_start + 1 + group.len() + 1;
            (group, end)
        } else {
            (rest, rest_start + rest.len())
        };
        let Some((name, declared, version, from)) = gradle_spec(spec, catalog) else {
            continue;
        };
        deps.push(GradleDep {
            dep: Dep {
                name,
                declared,
                scope,
                span: span_of(text, kw_start, spec_end),
            },
            version: version.unwrap_or_default(),
            from,
            conflict: Vec::new(),
        });
    }
    // An artifact declared twice: Maven keeps one declaration (and warns), so with
    // different versions none is the stated one. First stays, listing every version read.
    let mut versions: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for d in &deps {
        let read = if d.version.is_empty() {
            if d.dep.declared.is_empty() {
                "?".to_owned()
            } else {
                d.dep.declared.clone()
            }
        } else {
            d.version.clone()
        };
        let seen = versions.entry(d.dep.name.clone()).or_default();
        if !seen.contains(&read) {
            seen.push(read);
        }
    }
    let mut kept = BTreeSet::new();
    deps.retain(|d| kept.insert(d.dep.name.clone()));
    for d in &mut deps {
        if versions[&d.dep.name].len() > 1 {
            d.version.clear();
            d.from = "unresolved";
            d.conflict.clone_from(&versions[&d.dep.name]);
        }
    }
    deps
}

fn diagnostic(kind: &str, path: &str, span: Option<Span>, detail: Value) -> Diagnostic {
    Diagnostic {
        kind: kind.to_owned(),
        path: Some(path.to_owned()),
        span,
        detail,
    }
}

/// Every manifest edge and `lockfile_*` diagnostic of the commit (S15, S16).
pub(crate) fn dependencies(inputs: &Inputs<'_>) -> (Vec<ManifestEdge>, Vec<Diagnostic>) {
    let mut edges = Vec::new();
    let mut diagnostics = Vec::new();
    // Service identities: npm names, Maven coordinates, Go module paths.
    let mut npm_names: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut maven: BTreeMap<(String, String), Vec<(&str, String)>> = BTreeMap::new();
    let mut go_modules: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    let mut poms: BTreeMap<&str, Pom> = BTreeMap::new();
    for p in inputs.projects {
        for m in &p.manifests {
            let Some(text) = inputs.texts.get(m.as_str()) else {
                continue;
            };
            match m.rsplit('/').next().unwrap_or(m) {
                "package.json" => {
                    if let Some((Some(name), _)) = npm_manifest(text) {
                        npm_names.entry(name).or_default().push(&p.dir);
                    }
                }
                "pom.xml" => {
                    if let Some(pom) = pom(text) {
                        maven
                            .entry((pom.group.clone(), pom.artifact.clone()))
                            .or_default()
                            .push((&p.dir, pom.version.clone()));
                        poms.insert(m.as_str(), pom);
                    }
                }
                "go.mod" => {
                    if let Some(module) = boundaries::go_module_path(text) {
                        go_modules.entry(module).or_default().push(&p.dir);
                    }
                }
                _ => {}
            }
        }
    }
    let rel_dir = |dir: &str| dir_prefix(dir).trim_end_matches('/').to_owned();
    // `go.work` workspaces (contract M6e S8): every workspace by path, and
    // per workspace the module path each `use` member directory provides, so
    // `use` directories join services even where they are not projects of
    // their own. A use directory that leaves the repository is ignored.
    let mut works: BTreeMap<String, GoWork> = BTreeMap::new();
    let mut work_members: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    for (path, text) in inputs.texts.iter() {
        if path.rsplit('/').next().unwrap_or(path) != "go.work" {
            continue;
        }
        let work = go_work(text);
        let work_dir = path.rsplit_once('/').map_or("", |(d, _)| d);
        let mut members: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for u in &work.uses {
            let Some(dir) = join(work_dir, u) else {
                continue;
            };
            let mod_path = if dir.is_empty() {
                "go.mod".to_owned()
            } else {
                format!("{dir}/go.mod")
            };
            if let Some(module) = inputs
                .texts
                .get(mod_path.as_str())
                .and_then(|t| boundaries::go_module_path(t))
            {
                let dirs = members.entry(module).or_default();
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
        }
        work_members.insert((*path).to_owned(), members);
        works.insert((*path).to_owned(), work);
    }
    // Catalogs already reported unreadable (one diagnostic per catalog, not
    // per build file using it).
    let mut catalog_unread: BTreeSet<String> = BTreeSet::new();
    for p in inputs.projects {
        let service = p.dir.as_str();
        let here = rel_dir(service);
        for m in &p.manifests {
            let Some(text) = inputs.texts.get(m.as_str()) else {
                continue;
            };
            let evidence = |span: Span| Evidence {
                path: m.clone(),
                span,
            };
            match m.rsplit('/').next().unwrap_or(m) {
                "package.json" => {
                    let Some((_, deps)) = npm_manifest(text) else {
                        continue;
                    };
                    // The lockfile, read once per service.
                    let lock = match lockfile_for(&here, inputs.paths) {
                        Some((LockKind::Npm, path)) => {
                            let lock_dir = path.rsplit_once('/').map_or("", |(d, _)| d);
                            let rel = here
                                .strip_prefix(lock_dir)
                                .unwrap_or(&here)
                                .trim_start_matches('/');
                            match inputs.texts.get(path.as_str()).map(|t| npm_lock(t)) {
                                Some(Ok(packages)) => {
                                    // A lockfile above the service governs it only as a
                                    // workspace it lists.
                                    if rel.is_empty() || packages.contains_key(rel) {
                                        Some(Lock::Npm {
                                            path: path.clone(),
                                            packages,
                                            rel: rel.to_owned(),
                                        })
                                    } else {
                                        None
                                    }
                                }
                                Some(Err(reason)) => {
                                    diagnostics.push(diagnostic(
                                        "lockfile_unreadable",
                                        &path,
                                        None,
                                        json!({ "service": service, "reason": reason }),
                                    ));
                                    None
                                }
                                None => None,
                            }
                        }
                        Some((LockKind::Yarn, path)) => {
                            inputs.texts.get(path.as_str()).map(|text| Lock::Yarn {
                                path: path.clone(),
                                lock: yarn_lock(text),
                            })
                        }
                        Some((LockKind::Pnpm, path)) => {
                            match inputs.texts.get(path.as_str()).map(|t| pnpm_lock(t)) {
                                Some(Ok(versions)) => Some(Lock::Pnpm {
                                    path: path.clone(),
                                    versions,
                                }),
                                Some(Err(reason)) => {
                                    diagnostics.push(diagnostic(
                                        "lockfile_unreadable",
                                        &path,
                                        None,
                                        json!({ "service": service, "reason": reason }),
                                    ));
                                    None
                                }
                                None => None,
                            }
                        }
                        None => None,
                    };
                    for dep in deps {
                        // An alias (`"foo": "npm:bar@^1"`) installs `bar` under `foo` (Y9).
                        let aliased = npm_alias(&dep.declared);
                        if let Some(targets) = npm_names.get(aliased.unwrap_or(&dep.name))
                            && !targets.contains(&service)
                        {
                            let certainty = match targets.as_slice() {
                                [one]
                                    if npm_range_is_local(&dep.declared, &here, &rel_dir(one)) =>
                                {
                                    Certainty::Synthetic
                                }
                                [_] => Certainty::Heuristic,
                                _ => Certainty::Ambiguous,
                            };
                            for t in targets {
                                edges.push(ManifestEdge {
                                    service: service.to_owned(),
                                    target: Target::Service((*t).to_owned()),
                                    certainty,
                                    attrs: attrs(&[
                                        ("via", "package"),
                                        ("declared", &dep.declared),
                                        ("scope", dep.scope),
                                    ]),
                                    evidence: evidence(dep.span),
                                });
                            }
                            continue;
                        }
                        let real = aliased.map(str::to_owned);
                        let (version, from, certainty, conflict) = match &lock {
                            Some(Lock::Npm {
                                path,
                                packages,
                                rel,
                            }) => match lock_version(packages, rel, &dep.name) {
                                Some(v) => (v, "lockfile", Certainty::Exact, Vec::new()),
                                None => {
                                    diagnostics.push(diagnostic(
                                            "lockfile_missing_entry",
                                            m,
                                            Some(dep.span),
                                            json!({ "service": service, "name": dep.name, "lockfile": path }),
                                        ));
                                    (String::new(), "unresolved", Certainty::Exact, Vec::new())
                                }
                            },
                            Some(Lock::Yarn { path, lock }) => pin_reads(
                                lock.versions.get(&dep.name).map_or(&[], Vec::as_slice),
                                "yarn_lock",
                                m,
                                &dep,
                                service,
                                path,
                                &mut diagnostics,
                            ),
                            Some(Lock::Pnpm { path, versions }) => {
                                let key = real.as_deref().unwrap_or(&dep.name);
                                pin_reads(
                                    versions.get(key).map_or(&[], Vec::as_slice),
                                    "pnpm_lock",
                                    m,
                                    &dep,
                                    service,
                                    path,
                                    &mut diagnostics,
                                )
                            }
                            None => (String::new(), "unresolved", Certainty::Exact, Vec::new()),
                        };
                        let alias = if real.is_some() {
                            dep.name.as_str()
                        } else {
                            ""
                        };
                        let mut attrs = attrs(&[
                            ("alias", alias),
                            ("declared", &dep.declared),
                            ("scope", dep.scope),
                            ("version_from", from),
                        ]);
                        if !conflict.is_empty() {
                            attrs.insert("versions".into(), json!(conflict));
                        }
                        edges.push(ManifestEdge {
                            service: service.to_owned(),
                            target: Target::Package(Package {
                                manager: "npm",
                                name: real.unwrap_or_else(|| dep.name.clone()),
                                version,
                            }),
                            certainty,
                            attrs,
                            evidence: evidence(dep.span),
                        });
                    }
                }
                "go.mod" => {
                    let parsed = go_mod(text);
                    // The governing workspace, if any: its replaces win over
                    // the module's own, joined at the workspace root, and its
                    // `use` members join services by module path (S8).
                    let work_path = go_work_for(&here, inputs.paths);
                    let work = work_path.as_deref().and_then(|p| works.get(p));
                    let work_dir = work_path
                        .as_deref()
                        .map(|p| p.rsplit_once('/').map_or("", |(d, _)| d))
                        .unwrap_or("");
                    for (dep, version) in &parsed.requires {
                        // An excluded required version resolves to `?` with a
                        // diagnostic: MVS would pick an unknowable next
                        // version, so the reader never guesses (S8).
                        let excluded = parsed.excluded(&dep.name, version)
                            || work.is_some_and(|w| w.excluded(&dep.name, version));
                        if excluded {
                            diagnostics.push(diagnostic(
                                "go_excluded",
                                m,
                                Some(dep.span),
                                json!({ "service": service, "name": dep.name }),
                            ));
                            edges.push(ManifestEdge {
                                service: service.to_owned(),
                                target: Target::Package(Package {
                                    manager: "go",
                                    name: dep.name.clone(),
                                    version: String::new(),
                                }),
                                certainty: Certainty::Exact,
                                attrs: attrs(&[
                                    ("declared", &dep.declared),
                                    ("scope", dep.scope),
                                    ("version_from", "unresolved"),
                                ]),
                                evidence: evidence(dep.span),
                            });
                            continue;
                        }
                        let (replace, base) =
                            match work.and_then(|w| w.replace_for(&dep.name, version)) {
                                Some(r) => (Some(r), work_dir),
                                None => (parsed.replace_for(&dep.name, version), here.as_str()),
                            };
                        // Only a relative directory joins a service: an
                        // absolute path (`/b`) is a filesystem path, never a
                        // repo-relative directory (evaluator pass 1, E2-1).
                        // It stays a local replace for the version logic below.
                        let local = replace
                            .filter(|r| r.target.starts_with("./") || r.target.starts_with("../"))
                            .and_then(|r| join(base, &r.target));
                        let by_dir: Vec<&str> = local
                            .as_deref()
                            .map(|d| {
                                inputs
                                    .projects
                                    .iter()
                                    .filter(|p| rel_dir(&p.dir) == d)
                                    .map(|p| p.dir.as_str())
                                    .collect()
                            })
                            .unwrap_or_default();
                        let (targets, certainty): (Vec<&str>, Certainty) = if by_dir.len() == 1 {
                            (by_dir, Certainty::Synthetic)
                        } else if replace.is_some() {
                            // Replaced by a directory that is no single service, or by another
                            // module: not the module path's own package (Y10).
                            (Vec::new(), Certainty::Heuristic)
                        } else {
                            // Module-path candidates: the workspace's `use`
                            // members first, then every known module (deduped;
                            // members that are also projects appear once).
                            let mut names: Vec<&str> = Vec::new();
                            if let Some(members) = work_path
                                .as_deref()
                                .and_then(|p| work_members.get(p))
                                .and_then(|m| m.get(&dep.name))
                            {
                                names.extend(members.iter().map(String::as_str));
                            }
                            if let Some(known) = go_modules.get(&dep.name) {
                                names.extend(known.iter().copied());
                            }
                            names.sort();
                            names.dedup();
                            let certainty = if names.len() <= 1 {
                                Certainty::Heuristic
                            } else {
                                Certainty::Ambiguous
                            };
                            (names, certainty)
                        };
                        let targets: Vec<&str> =
                            targets.into_iter().filter(|t| *t != service).collect();
                        if !targets.is_empty() {
                            for t in targets {
                                edges.push(ManifestEdge {
                                    service: service.to_owned(),
                                    target: Target::Service(t.to_owned()),
                                    certainty,
                                    attrs: attrs(&[
                                        ("via", "package"),
                                        ("declared", &dep.declared),
                                        ("scope", dep.scope),
                                    ]),
                                    evidence: evidence(dep.span),
                                });
                            }
                            continue;
                        }
                        // A replace by another version of the same module sets the version; by a
                        // directory or another module, the version is unresolved (Y10).
                        let (version, from, certainty) = match replace {
                            None => (version.clone(), "go_mod", Certainty::Synthetic),
                            Some(r) if !r.local() && r.target == dep.name => match &r.to_version {
                                Some(v) => (v.clone(), "go_mod", Certainty::Synthetic),
                                None => (String::new(), "unresolved", Certainty::Exact),
                            },
                            Some(_) => (String::new(), "unresolved", Certainty::Exact),
                        };
                        let replaced = replace.map(Replace::written).unwrap_or_default();
                        edges.push(ManifestEdge {
                            service: service.to_owned(),
                            target: Target::Package(Package {
                                manager: "go",
                                name: dep.name.clone(),
                                version,
                            }),
                            certainty,
                            attrs: attrs(&[
                                ("declared", &dep.declared),
                                ("replaced", &replaced),
                                ("scope", dep.scope),
                                ("version_from", from),
                            ]),
                            evidence: evidence(dep.span),
                        });
                    }
                }
                "pyproject.toml" => {
                    let deps = pyproject_deps(text);
                    if deps.is_empty() {
                        continue;
                    }
                    // The governing lockfile, read once per service.
                    let lock = match py_lockfile_for(&here, inputs.paths, inputs.texts) {
                        Some(PyLock::Read(path, source)) => {
                            let reader = match source {
                                LockSource::Poetry => poetry_lock,
                                LockSource::Uv => uv_lock,
                            };
                            let from = match source {
                                LockSource::Poetry => "poetry_lock",
                                LockSource::Uv => "uv_lock",
                            };
                            match inputs.texts.get(path.as_str()).map(|t| reader(t)) {
                                Some(Ok(packages)) => Some((path, packages, from)),
                                Some(Err(reason)) => {
                                    diagnostics.push(diagnostic(
                                        "lockfile_unreadable",
                                        &path,
                                        None,
                                        json!({ "service": service, "reason": reason }),
                                    ));
                                    None
                                }
                                None => None,
                            }
                        }
                        Some(PyLock::Unread(path)) => {
                            diagnostics.push(diagnostic(
                                "lockfile_unread",
                                &path,
                                None,
                                json!({ "service": service }),
                            ));
                            None
                        }
                        None => None,
                    };
                    for dep in deps {
                        let (version, from) = match &lock {
                            Some((path, packages, from)) => match packages.get(&dep.name) {
                                Some(v) => (v.clone(), *from),
                                None => {
                                    diagnostics.push(diagnostic(
                                        "lockfile_missing_entry",
                                        m,
                                        Some(dep.span),
                                        json!({ "service": service, "name": dep.name, "lockfile": path }),
                                    ));
                                    (String::new(), "unresolved")
                                }
                            },
                            None => (String::new(), "unresolved"),
                        };
                        edges.push(ManifestEdge {
                            service: service.to_owned(),
                            target: Target::Package(Package {
                                manager: "py",
                                name: dep.name.clone(),
                                version,
                            }),
                            certainty: Certainty::Exact,
                            attrs: attrs(&[
                                ("declared", &dep.declared),
                                ("scope", "runtime"),
                                ("version_from", from),
                            ]),
                            evidence: evidence(dep.span),
                        });
                    }
                }
                "pom.xml" => {
                    let Some(pom) = poms.get(m.as_str()) else {
                        continue;
                    };
                    for (dep, version, from, conflict) in &pom.deps {
                        let (group, artifact) =
                            dep.name.split_once('/').unwrap_or((dep.name.as_str(), ""));
                        if let Some(modules) = maven.get(&(group.to_owned(), artifact.to_owned())) {
                            let modules: Vec<&(&str, String)> =
                                modules.iter().filter(|(d, _)| *d != service).collect();
                            if !modules.is_empty() {
                                let certainty = match modules.as_slice() {
                                    [(_, v)] if *from == "literal" && v == version => {
                                        Certainty::Synthetic
                                    }
                                    [_] => Certainty::Heuristic,
                                    _ => Certainty::Ambiguous,
                                };
                                for (t, _) in modules {
                                    edges.push(ManifestEdge {
                                        service: service.to_owned(),
                                        target: Target::Service((*t).to_owned()),
                                        certainty,
                                        attrs: attrs(&[
                                            ("via", "package"),
                                            ("declared", &dep.declared),
                                            ("scope", dep.scope),
                                        ]),
                                        evidence: evidence(dep.span),
                                    });
                                }
                                continue;
                            }
                        }
                        let certainty = if !conflict.is_empty() {
                            Certainty::Heuristic
                        } else if *from == "property" {
                            Certainty::Synthetic
                        } else {
                            Certainty::Exact
                        };
                        let mut attrs = attrs(&[
                            ("declared", &dep.declared),
                            ("scope", dep.scope),
                            ("version_from", from),
                        ]);
                        if !conflict.is_empty() {
                            attrs.insert("versions".into(), json!(conflict));
                        }
                        edges.push(ManifestEdge {
                            service: service.to_owned(),
                            target: Target::Package(Package {
                                manager: "maven",
                                name: dep.name.clone(),
                                version: version.clone(),
                            }),
                            certainty,
                            attrs,
                            evidence: evidence(dep.span),
                        });
                    }
                }
                "build.gradle" | "build.gradle.kts" => {
                    // The governing catalog (nearest `gradle/libs.versions.toml`
                    // above the build file); an unreadable one is one
                    // diagnostic, and its `libs.` references stay unresolved.
                    let mut catalog = BTreeMap::new();
                    if let Some(path) = catalog_for(m, inputs.paths)
                        && let Some(text) = inputs.texts.get(path.as_str())
                    {
                        match gradle_catalog(text) {
                            Ok(read) => catalog = read,
                            Err(reason) => {
                                if catalog_unread.insert(path.clone()) {
                                    diagnostics.push(diagnostic(
                                        "catalog_unreadable",
                                        &path,
                                        None,
                                        json!({ "service": service, "reason": reason }),
                                    ));
                                }
                            }
                        }
                    }
                    for d in gradle_deps(text, &catalog) {
                        let certainty = if !d.conflict.is_empty() {
                            Certainty::Heuristic
                        } else {
                            Certainty::Exact
                        };
                        let mut attrs = attrs(&[
                            ("declared", &d.dep.declared),
                            ("scope", d.dep.scope),
                            ("version_from", d.from),
                        ]);
                        if !d.conflict.is_empty() {
                            attrs.insert("versions".into(), json!(d.conflict));
                        }
                        edges.push(ManifestEdge {
                            service: service.to_owned(),
                            target: Target::Package(Package {
                                manager: "maven",
                                name: d.dep.name.clone(),
                                version: d.version.clone(),
                            }),
                            certainty,
                            attrs,
                            evidence: evidence(d.dep.span),
                        });
                    }
                }
                _ => {}
            }
        }
    }
    // One edge per (service, target): the first declaration wins (field order, then name).
    let mut seen = BTreeSet::new();
    edges.retain(|e| seen.insert((e.service.clone(), e.target.clone())));
    (edges, diagnostics)
}

/// Adds the manifest edges and their `Package` nodes (attributes as M3b's, so a key shared
/// with a `scip` edge names one node).
pub(crate) fn add(b: &mut GraphBuilder, edges: &[ManifestEdge]) -> anyhow::Result<()> {
    for e in edges {
        let dst = match &e.target {
            Target::Package(p) => {
                let key = NodeKey::new(NodeKind::Package, p.key());
                b.add_node(
                    key.clone(),
                    &p.name,
                    crate::object(json!({
                        "manager": p.manager,
                        "name": p.name,
                        "version": p.version,
                    })),
                )?;
                key
            }
            Target::Service(dir) => NodeKey::new(NodeKind::Service, dir.clone()),
        };
        b.add_edge(
            NewEdge::new(
                NodeKey::new(NodeKind::Service, e.service.clone()),
                EdgeKind::DependsOn,
                dst,
                Provenance::Manifest,
                e.certainty,
            )
            .attrs(e.attrs.clone())
            .evidence(e.evidence.clone()),
        )?;
    }
    Ok(())
}

/// classpath(S) (D8): S and every service S reaches by `via=package` edges, transitively; one
/// BFS with a visited set, so a cycle terminates.
pub(crate) fn classpath(
    service: &str,
    depends: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut seen = BTreeSet::from([service.to_owned()]);
    let mut queue = VecDeque::from([service.to_owned()]);
    while let Some(s) = queue.pop_front() {
        for next in depends.get(&s).into_iter().flatten() {
            if seen.insert(next.clone()) {
                queue.push_back(next.clone());
            }
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(dir: &str, manifests: &[&str]) -> Project {
        Project {
            dir: dir.to_owned(),
            name: dir.to_owned(),
            detected_by: Vec::new(),
            manifests: manifests.iter().map(|m| (*m).to_owned()).collect(),
            deployable: true,
        }
    }

    /// `service -> target [certainty] {attrs}` per edge, and `kind path` per diagnostic.
    fn run(projects: &[Project], files: &[(&str, &str)]) -> (Vec<String>, Vec<String>) {
        let texts: BTreeMap<&str, String> = files
            .iter()
            .filter(|(p, _)| is_input(p))
            .map(|(p, t)| (*p, (*t).to_owned()))
            .collect();
        let paths: BTreeSet<&str> = files.iter().map(|(p, _)| *p).collect();
        let (edges, diagnostics) = dependencies(&Inputs {
            projects,
            texts: &texts,
            paths: &paths,
        });
        let edges = edges
            .iter()
            .map(|e| {
                let target = match &e.target {
                    Target::Package(p) => p.key(),
                    Target::Service(s) => format!("service {s}"),
                };
                format!(
                    "{} -> {target} [{}] {}",
                    e.service,
                    e.certainty.as_str(),
                    Value::Object(e.attrs.clone())
                )
            })
            .collect();
        let diagnostics = diagnostics
            .iter()
            .map(|d| format!("{} {}", d.kind, d.path.as_deref().unwrap_or("-")))
            .collect();
        (edges, diagnostics)
    }

    /// Contract M6c S4: `tsconfig.json` texts reach the manifest readers that
    /// feed exact alias resolution.
    #[test]
    fn tsconfig_json_is_a_manifest_input() {
        assert!(is_input("svc/tsconfig.json"));
        assert!(is_input("tsconfig.json"));
        assert!(!is_input("svc/tsconfig.json.bak"));
    }

    /// Contract M6c S8: `lombok.config` texts reach the manifest readers that
    /// feed qualifier-copying resolution.
    #[test]
    fn lombok_config_is_a_manifest_input() {
        assert!(is_input("svc/lombok.config"));
        assert!(is_input("lombok.config"));
        assert!(!is_input("svc/lombok.config.bak"));
    }

    /// Contract M6e S5: `yarn.lock` and `pnpm-lock.yaml` reach the manifest
    /// readers that feed `npm/` package edges.
    #[test]
    fn yarn_and_pnpm_lockfiles_are_manifest_inputs() {
        assert!(is_input("svc/yarn.lock"));
        assert!(is_input("svc/pnpm-lock.yaml"));
        assert!(is_input("yarn.lock"));
        assert!(!is_input("svc/yarn.lock.bak"));
    }

    /// Contract M6e S5: yarn v1 blockade — multi-header blocks share one
    /// version, quoted scoped headers parse, `npm:` alias specs resolve to
    /// the real package, version-less blocks are skipped, comments ignored.
    #[test]
    fn yarn_v1_blocks_versions_and_aliases() {
        let lock = yarn_lock(
            "# yarn lockfile v1\n\n\"@scope/name@^1.0.0\", \"@scope/name@~1.0.0\":\n  version \"1.2.3\"\n  resolved \"https://registry.yarnpkg.com/@scope/name/-/name-1.2.3.tgz\"\n\nbare@^2:\n  version \"2.0.0\"\n\n\"alias@npm:real@^1\":\n  version \"1.0.1\"\n\nnoversion@^1:\n  resolved \"https://example.com/x.tgz\"\n",
        );
        assert_eq!(lock.versions.get("bare"), Some(&vec!["2.0.0".to_owned()]));
        assert_eq!(lock.versions.get("alias"), Some(&vec!["1.0.1".to_owned()]));
        assert!(!lock.versions.contains_key("noversion"));
        // Two majors for one name both survive for the duplicate rule.
        let lock = yarn_lock("a@^1:\n  version \"1.0.0\"\n\na@^2:\n  version \"2.0.0\"\n");
        assert_eq!(
            lock.versions.get("a"),
            Some(&vec!["1.0.0".to_owned(), "2.0.0".to_owned()])
        );
    }

    /// Contract M6e S5: Berry lockfiles gate on `__metadata` and pin through
    /// `resolution:`; non-`npm:` descriptors (workspace/file/link) stay out.
    #[test]
    fn yarn_berry_entries_pin_through_resolutions() {
        let lock = yarn_lock(
            "__metadata:\n  version: 8\n  cacheKey: 10c0\n\n\"name@npm:^1.0.0\":\n  version: 1.2.3\n  resolution: \"name@npm:1.2.3\"\n  checksum: abc\n\n\"alias@npm:real@^1\":\n  version: 1.0.1\n  resolution: \"real@npm:1.0.1\"\n\n\"local@workspace:.\":\n  version: 0.0.0-use.local\n  resolution: \"local@workspace:.\"\n",
        );
        assert_eq!(lock.versions.get("name"), Some(&vec!["1.2.3".to_owned()]));
        assert_eq!(lock.versions.get("alias"), Some(&vec!["1.0.1".to_owned()]));
        assert!(!lock.versions.contains_key("local"));
    }

    /// Contract M6e S5: pnpm v9 `packages:` keys pin versions (peer suffixes
    /// stripped, scoped names kept); other majors and missing `packages:`
    /// are unreadable, never inferred.
    #[test]
    fn pnpm_packages_peers_and_major_gate() {
        let versions = pnpm_lock(
            "lockfileVersion: '9.0'\npackages:\n  foo@1.0.0:\n    resolution: {integrity: sha512-x}\n  bar@2.0.0(bar@3.0.0):\n    resolution: {integrity: sha512-y}\n  '@scope/baz@4.1.0':\n    resolution: {integrity: sha512-z}\n",
        )
        .unwrap();
        assert_eq!(versions.get("foo"), Some(&vec!["1.0.0".to_owned()]));
        assert_eq!(versions.get("bar"), Some(&vec!["2.0.0".to_owned()]));
        assert_eq!(versions.get("@scope/baz"), Some(&vec!["4.1.0".to_owned()]));
        assert!(pnpm_lock("lockfileVersion: '6.0'\npackages:\n  foo@1.0.0:\n").is_err());
        assert!(pnpm_lock("lockfileVersion: '9.0'\n").is_err());
        assert!(pnpm_lock("not: [yaml").is_err());
    }

    /// Contract M6e S5, edge level: yarn/pnpm locks govern by nearness with
    /// npm-first ties; pins are `Exact` with per-format `version_from`;
    /// missing entries and multi-major duplicates resolve safe.
    #[test]
    fn yarn_and_pnpm_govern_pins_and_duplicates() {
        let projects = [
            project("y", &["y/package.json"]),
            project("p", &["p/package.json"]),
            project("n", &["n/package.json"]),
        ];
        let manifest = r#"{ "dependencies": { "a": "^1", "b": "^2", "c": "^1", "d": "^1", "e": "npm:real@^1" } }"#;
        let (edges, diagnostics) = run(
            &projects,
            &[
                ("y/package.json", manifest),
                (
                    "y/yarn.lock",
                    "a@^1:\n  version \"1.0.0\"\n\nb@^2:\n  version \"2.0.0\"\n\nb@^3:\n  version \"3.0.0\"\n\n\"e@npm:real@^1\":\n  version \"5.0.0\"\n",
                ),
                ("p/package.json", manifest),
                (
                    "p/pnpm-lock.yaml",
                    "lockfileVersion: '9.0'\npackages:\n  a@1.0.0:\n  c@3.0.0:\n  real@5.0.0:\n",
                ),
                ("n/package.json", manifest),
                (
                    "n/package-lock.json",
                    r#"{ "lockfileVersion": 3, "packages": {} }"#,
                ),
                ("n/yarn.lock", "a@^1:\n  version \"9.9.9\"\n"),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"y -> npm/a@1.0.0 [exact] {"declared":"^1","scope":"runtime","version_from":"yarn_lock"}"#,
                r#"y -> npm/b@? [heuristic] {"declared":"^2","scope":"runtime","version_from":"yarn_lock","versions":["2.0.0","3.0.0"]}"#,
                r#"y -> npm/c@? [exact] {"declared":"^1","scope":"runtime","version_from":"unresolved"}"#,
                r#"y -> npm/d@? [exact] {"declared":"^1","scope":"runtime","version_from":"unresolved"}"#,
                r#"y -> npm/real@5.0.0 [exact] {"alias":"e","declared":"npm:real@^1","scope":"runtime","version_from":"yarn_lock"}"#,
                r#"p -> npm/a@1.0.0 [exact] {"declared":"^1","scope":"runtime","version_from":"pnpm_lock"}"#,
                r#"p -> npm/b@? [exact] {"declared":"^2","scope":"runtime","version_from":"unresolved"}"#,
                r#"p -> npm/c@3.0.0 [exact] {"declared":"^1","scope":"runtime","version_from":"pnpm_lock"}"#,
                r#"p -> npm/d@? [exact] {"declared":"^1","scope":"runtime","version_from":"unresolved"}"#,
                r#"p -> npm/real@5.0.0 [exact] {"alias":"e","declared":"npm:real@^1","scope":"runtime","version_from":"pnpm_lock"}"#,
                r#"n -> npm/a@? [exact] {"declared":"^1","scope":"runtime","version_from":"unresolved"}"#,
                r#"n -> npm/b@? [exact] {"declared":"^2","scope":"runtime","version_from":"unresolved"}"#,
                r#"n -> npm/c@? [exact] {"declared":"^1","scope":"runtime","version_from":"unresolved"}"#,
                r#"n -> npm/d@? [exact] {"declared":"^1","scope":"runtime","version_from":"unresolved"}"#,
                r#"n -> npm/real@? [exact] {"alias":"e","declared":"npm:real@^1","scope":"runtime","version_from":"unresolved"}"#,
            ]
        );
        let mut diagnostics = diagnostics;
        diagnostics.sort();
        assert_eq!(
            diagnostics,
            [
                "lockfile_missing_entry n/package.json",
                "lockfile_missing_entry n/package.json",
                "lockfile_missing_entry n/package.json",
                "lockfile_missing_entry n/package.json",
                "lockfile_missing_entry n/package.json",
                "lockfile_missing_entry p/package.json",
                "lockfile_missing_entry p/package.json",
                "lockfile_missing_entry y/package.json",
                "lockfile_missing_entry y/package.json",
            ]
        );
    }

    /// Contract M6e S8: `go.work` workspaces — `use` members join services,
    /// workspace replaces win over module replaces, and excluded required
    /// versions resolve to `?` with a diagnostic.
    #[test]
    fn go_work_use_replace_and_exclude() {
        let projects = [
            project("app", &["app/go.mod"]),
            project("lib", &["lib/go.mod"]),
        ];
        let (edges, diagnostics) = run(
            &projects,
            &[
                (
                    "go.work",
                    "go 1.23\n\nuse ./app\nuse (\n\t./lib\n)\n\nreplace example.com/lib => ./lib\n",
                ),
                (
                    "app/go.mod",
                    "module example.com/app\n\ngo 1.23\n\nrequire (\n\texample.com/lib v0.0.0\n\tgithub.com/gin-gonic/gin v1.10.0\n\texample.com/stale v1.0.0\n)\n\nreplace example.com/lib => example.com/fork v1.0.0\n\nexclude example.com/stale v1.0.0\n",
                ),
                ("lib/go.mod", "module example.com/lib\n"),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"app -> service lib [synthetic] {"declared":"v0.0.0","scope":"runtime","via":"package"}"#,
                r#"app -> go/github.com/gin-gonic/gin@v1.10.0 [synthetic] {"declared":"v1.10.0","scope":"runtime","version_from":"go_mod"}"#,
                r#"app -> go/example.com/stale@? [exact] {"declared":"v1.0.0","scope":"runtime","version_from":"unresolved"}"#,
            ]
        );
        let mut diagnostics = diagnostics;
        diagnostics.sort();
        assert_eq!(diagnostics, ["go_excluded app/go.mod"]);
        // Two members sharing one module path stay ambiguous.
        let projects = [
            project("app", &["app/go.mod"]),
            project("lib", &["lib/go.mod"]),
            project("lib2", &["lib2/go.mod"]),
        ];
        let (edges, _) = run(
            &projects,
            &[
                ("go.work", "go 1.23\nuse ./app\nuse ./lib\nuse ./lib2\n"),
                (
                    "app/go.mod",
                    "module example.com/app\n\nrequire example.com/lib v0.0.0\n",
                ),
                ("lib/go.mod", "module example.com/lib\n"),
                ("lib2/go.mod", "module example.com/lib\n"),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"app -> service lib [ambiguous] {"declared":"v0.0.0","scope":"runtime","via":"package"}"#,
                r#"app -> service lib2 [ambiguous] {"declared":"v0.0.0","scope":"runtime","via":"package"}"#,
            ]
        );
    }

    /// Contract M6e S8: `go.work` files reach the manifest readers that feed
    /// `go/` package edges.
    #[test]
    fn go_work_files_are_manifest_inputs() {
        assert!(is_input("go.work"));
        assert!(is_input("svc/go.work"));
        assert!(!is_input("svc/go.work.bak"));
    }

    /// Contract M6e S8: `go.work` shapes — single and block `use`, `replace`
    /// directives, `go`/`toolchain` lines and garbage ignored.
    #[test]
    fn go_work_use_and_replace_shapes() {
        let w = go_work(
            "go 1.23\n\ntoolchain go1.24.0\nuse ./a\nuse (\n\t./b\n\t./c\n)\n\nreplace example.com/x => ./x\nreplace example.com/y v1.0.0 => example.com/z v2.0.0\ngarbage )(\n",
        );
        assert_eq!(
            w.uses,
            vec!["./a".to_owned(), "./b".to_owned(), "./c".to_owned()]
        );
        assert_eq!(w.replaces.len(), 2);
        assert_eq!(w.replaces[0].target, "./x");
        let w = go_work("go 1.23\n");
        assert!(w.uses.is_empty() && w.replaces.is_empty());
    }

    /// Contract M6e S3: version catalogs resolve `module` + `version` /
    /// `version.ref` / short `group:artifact:version` entries; a dangling
    /// ref resolves to nothing; bad TOML is unreadable, never inferred.
    #[test]
    fn gradle_catalogs_resolve_libraries_versions_and_refs() {
        let catalog = gradle_catalog(
            "[versions]\nlang = \"3.20.0\"\ngson = \"2.13.1\"\n[libraries]\nlang3 = { module = \"org.apache.commons:commons-lang3\", version = \"3.20.0\" }\ngson = { module = \"com.google.code.gson:gson\", version.ref = \"gson\" }\nshort = \"org.example:short:1.2.3\"\ndangling = { module = \"org.example:dangling\", version.ref = \"missing\" }\n",
        )
        .unwrap();
        assert_eq!(
            catalog.get("lang3"),
            Some(&(
                "org.apache.commons:commons-lang3".to_owned(),
                Some("3.20.0".to_owned())
            ))
        );
        assert_eq!(
            catalog.get("gson"),
            Some(&(
                "com.google.code.gson:gson".to_owned(),
                Some("2.13.1".to_owned())
            ))
        );
        assert_eq!(
            catalog.get("short"),
            Some(&("org.example:short".to_owned(), Some("1.2.3".to_owned())))
        );
        assert_eq!(
            catalog.get("dangling"),
            Some(&("org.example:dangling".to_owned(), None))
        );
        assert!(gradle_catalog("[[[nope").is_err());
        assert!(gradle_catalog("[libraries]\nlang3 = 42\n").is_err());
    }

    /// Contract M6e S3: Kotlin-DSL literals across the allow-listed
    /// configurations — parenthesised, triple-argument, named-argument and
    /// `libs.` forms; `platform()` unwraps; `$`-interpolated versions stay
    /// unresolved; commented lines give nothing.
    #[test]
    fn gradle_kotlin_literals_and_catalog_refs() {
        let catalog = gradle_catalog(
            "[libraries]\nlang3 = { module = \"org.apache.commons:commons-lang3\", version = \"3.20.0\" }\nodd = { module = \"org.example:odd\", version = \"3.0.1\" }\n",
        )
        .unwrap();
        let deps = gradle_deps(
            "plugins { id(\"java\") }\ndependencies {\n    implementation(\"org.apache.commons:commons-lang3:3.20.0\")\n    api(\"com.google.code.gson\", \"gson\", \"2.13.1\")\n    testImplementation(libs.odd)\n    compileOnly(group = \"org.example\", name = \"bare\", version = \"1.0\")\n    runtimeOnly(\"org.example:interp:${v}\")\n    implementation(platform(\"org.example:bom:2.0\"))\n    // implementation(\"org.example:commented:9.9\")\n}\n",
            &catalog,
        );
        let row =
            |d: &GradleDep| format!("{}|{}|{}|{}", d.dep.name, d.version, d.from, d.dep.scope);
        let rows: Vec<String> = deps.iter().map(row).collect();
        assert_eq!(
            rows,
            [
                "org.apache.commons/commons-lang3|3.20.0|literal|runtime",
                "com.google.code.gson/gson|2.13.1|literal|runtime",
                "org.example/odd|3.0.1|catalog|test",
                "org.example/bare|1.0|literal|compile",
                "org.example/interp||unresolved|runtime",
                "org.example/bom|2.0|literal|runtime",
            ]
        );
    }

    /// Contract M6e S3: Groovy-DSL literals — single quotes, bare (no
    /// parens), map notation and `libs.` forms; `project()` references and
    /// multi-line blocks stay out.
    #[test]
    fn gradle_groovy_literals_maps_and_bare_forms() {
        let catalog = gradle_catalog(
            "[libraries]\nodd = { module = \"org.example:is-odd\", version = \"3.0.1\" }\n",
        )
        .unwrap();
        let deps = gradle_deps(
            "dependencies {\n    implementation 'org.apache.commons:commons-lang3:3.20.0'\n    implementation \"org.example:bare:1.1\"\n    implementation group: 'org.example', name: 'mapped', version: '2.0'\n    testImplementation libs.odd\n    implementation project(':lib')\n    implementation(\"org.example:multi\",\n        \"1.2\")\n    implementation 'org.example:trail:1.0' // trailing comment\n    implementationX(\"org.example:nope:1.0\")\n}\n",
            &catalog,
        );
        let rows: Vec<String> = deps
            .iter()
            .map(|d| format!("{}|{}|{}|{}", d.dep.name, d.version, d.from, d.dep.scope))
            .collect();
        assert_eq!(
            rows,
            [
                "org.apache.commons/commons-lang3|3.20.0|literal|runtime",
                "org.example/bare|1.1|literal|runtime",
                "org.example/mapped|2.0|literal|runtime",
                "org.example/is-odd|3.0.1|catalog|test",
                "org.example/trail|1.0|literal|runtime",
            ]
        );
    }

    /// Contract M6e S3: a repeated artifact at two versions keeps the first
    /// and clears it to `@?` with `versions` (the Maven duplicate rule).
    #[test]
    fn gradle_duplicate_artifacts_keep_the_first_as_unknown() {
        let projects = [project("a", &["a/build.gradle"])];
        let (edges, _) = run(
            &projects,
            &[(
                "a/build.gradle",
                "dependencies {\n    implementation(\"org.example:dup:1.0\")\n    implementation(\"org.example:dup:2.0\")\n}\n",
            )],
        );
        assert_eq!(
            edges,
            [
                "a -> maven/org.example/dup@? [heuristic] {\"declared\":\"1.0\",\"scope\":\"runtime\",\"version_from\":\"unresolved\",\"versions\":[\"1.0\",\"2.0\"]}"
            ]
        );
    }

    /// Contract M6e S3: the governing catalog is the nearest
    /// `gradle/libs.versions.toml` at or above the build file.
    #[test]
    fn gradle_catalogs_govern_from_the_nearest_ancestor() {
        let projects = [project("a", &["a/sub/build.gradle"])];
        let (edges, _) = run(
            &projects,
            &[
                (
                    "a/sub/build.gradle",
                    "dependencies {\n    implementation(libs.odd)\n}\n",
                ),
                (
                    "a/gradle/libs.versions.toml",
                    "[libraries]\nodd = { module = \"org.example:odd\", version = \"3.0.1\" }\n",
                ),
            ],
        );
        assert_eq!(
            edges,
            [
                "a -> maven/org.example/odd@3.0.1 [exact] {\"declared\":\"libs.odd\",\"scope\":\"runtime\",\"version_from\":\"catalog\"}"
            ]
        );
    }

    /// Contract M6e S3, edge level: literals are `Exact`, catalog pins are
    /// `Exact` with `version_from: catalog`, unresolvable versions are
    /// `Exact` with `version_from: unresolved`, and an unreadable catalog is
    /// a diagnostic (its `libs.` refs stay unresolved).
    #[test]
    fn gradle_edges_versions_and_catalog_diagnostics() {
        let projects = [
            project("k", &["k/build.gradle.kts", "k/gradle/libs.versions.toml"]),
            project("g", &["g/build.gradle", "g/gradle/libs.versions.toml"]),
        ];
        let (edges, diagnostics) = run(
            &projects,
            &[
                (
                    "k/build.gradle.kts",
                    "dependencies {\n    implementation(\"org.apache.commons:commons-lang3:3.20.0\")\n    testImplementation(libs.odd)\n    implementation(\"org.example:interp:${v}\")\n    implementation(libs.ghost)\n}\n",
                ),
                (
                    "k/gradle/libs.versions.toml",
                    "[libraries]\nlang3 = { module = \"org.apache.commons:commons-lang3\", version = \"3.20.0\" }\nodd = { module = \"org.example:odd\", version = \"3.0.1\" }\nghost = { module = \"org.example:ghost\" }\n",
                ),
                (
                    "g/build.gradle",
                    "dependencies {\n    implementation 'org.example:mapped:4.0'\n    implementation(\"org.example:bare\")\n}\n",
                ),
                ("g/gradle/libs.versions.toml", "[[[nope"),
            ],
        );
        assert_eq!(
            edges,
            [
                "k -> maven/org.apache.commons/commons-lang3@3.20.0 [exact] {\"declared\":\"3.20.0\",\"scope\":\"runtime\",\"version_from\":\"literal\"}",
                "k -> maven/org.example/odd@3.0.1 [exact] {\"declared\":\"libs.odd\",\"scope\":\"test\",\"version_from\":\"catalog\"}",
                "k -> maven/org.example/interp@? [exact] {\"declared\":\"${v}\",\"scope\":\"runtime\",\"version_from\":\"unresolved\"}",
                "k -> maven/org.example/ghost@? [exact] {\"declared\":\"libs.ghost\",\"scope\":\"runtime\",\"version_from\":\"unresolved\"}",
                "g -> maven/org.example/mapped@4.0 [exact] {\"declared\":\"4.0\",\"scope\":\"runtime\",\"version_from\":\"literal\"}",
                "g -> maven/org.example/bare@? [exact] {\"scope\":\"runtime\",\"version_from\":\"unresolved\"}",
            ]
        );
        assert_eq!(
            diagnostics,
            ["catalog_unreadable g/gradle/libs.versions.toml"]
        );
    }

    const LOCK: &str = r#"{
  "name": "root", "lockfileVersion": 3,
  "packages": {
    "": { "workspaces": ["apps/web", "packages/shared"] },
    "apps/web": { "name": "@acme/web" },
    "apps/web/node_modules/axios": { "version": "1.6.0" },
    "node_modules/axios": { "version": "1.7.7" },
    "node_modules/@prisma/client": { "version": "5.22.0" },
    "node_modules/@acme/shared": { "resolved": "packages/shared", "link": true },
    "packages/shared": { "name": "@acme/shared", "version": "0.1.0" }
  }
}"#;

    #[test]
    fn npm_versions_from_the_lockfile_walk() {
        let projects = [
            project(".", &["package.json"]),
            project("apps/web", &["apps/web/package.json"]),
            project("packages/shared", &["packages/shared/package.json"]),
        ];
        let (edges, diagnostics) = run(
            &projects,
            &[
                (
                    "package.json",
                    r#"{ "name": "root", "devDependencies": { "axios": "^1.7.0", "left-pad": "1.0.0" } }"#,
                ),
                ("package-lock.json", LOCK),
                (
                    "apps/web/package.json",
                    "{ \"name\": \"@acme/web\",\n  \"dependencies\": {\n    \"axios\": \"^1.6.0\",\n    \"@prisma/client\": \"^5\",\n    \"@acme/shared\": \"*\"\n  }\n}",
                ),
                (
                    "packages/shared/package.json",
                    r#"{ "name": "@acme/shared", "version": "0.1.0" }"#,
                ),
            ],
        );
        assert_eq!(
            edges,
            [
                r#". -> npm/axios@1.7.7 [exact] {"declared":"^1.7.0","scope":"dev","version_from":"lockfile"}"#,
                r#". -> npm/left-pad@? [exact] {"declared":"1.0.0","scope":"dev","version_from":"unresolved"}"#,
                // A workspace link; a scoped package; nested (not hoisted) wins for the workspace.
                r#"apps/web -> service packages/shared [synthetic] {"declared":"*","scope":"runtime","via":"package"}"#,
                r#"apps/web -> npm/@prisma/client@5.22.0 [exact] {"declared":"^5","scope":"runtime","version_from":"lockfile"}"#,
                r#"apps/web -> npm/axios@1.6.0 [exact] {"declared":"^1.6.0","scope":"runtime","version_from":"lockfile"}"#,
            ]
        );
        assert_eq!(diagnostics, ["lockfile_missing_entry package.json"]);
    }

    #[test]
    fn unreadable_unread_and_absent_lockfiles() {
        let projects = [
            project("a", &["a/package.json"]),
            project("b", &["b/package.json"]),
        ];
        let manifest = r#"{ "dependencies": { "express": "^4" } }"#;
        let (edges, diagnostics) = run(
            &projects,
            &[
                ("a/package.json", manifest),
                (
                    "a/package-lock.json",
                    r#"{ "lockfileVersion": 1, "dependencies": {} }"#,
                ),
                ("b/package.json", manifest),
                ("b/yarn.lock", "express@^4:\n  version \"4.19.2\"\n"),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"a -> npm/express@? [exact] {"declared":"^4","scope":"runtime","version_from":"unresolved"}"#,
                r#"b -> npm/express@4.19.2 [exact] {"declared":"^4","scope":"runtime","version_from":"yarn_lock"}"#,
            ]
        );
        assert_eq!(diagnostics, ["lockfile_unreadable a/package-lock.json"]);
        let (_, diagnostics) = run(
            &projects[..1],
            &[
                ("a/package.json", manifest),
                ("a/package-lock.json", "{ nope"),
            ],
        );
        assert_eq!(diagnostics, ["lockfile_unreadable a/package-lock.json"]);
    }

    #[test]
    fn workspace_ranges_and_shared_names() {
        let projects = [
            project("web", &["web/package.json"]),
            project("lib", &["lib/package.json"]),
            project("x1", &["x1/package.json"]),
            project("x2", &["x2/package.json"]),
        ];
        let (edges, _) = run(
            &projects,
            &[
                (
                    "web/package.json",
                    r#"{ "name": "web", "dependencies": { "lib": "^1.0.0", "dup": "*" }, "devDependencies": { "lib": "file:../lib" } }"#,
                ),
                (
                    "lib/package.json",
                    r#"{ "name": "lib", "dependencies": { "web": "workspace:*" } }"#,
                ),
                ("x1/package.json", r#"{ "name": "dup" }"#),
                ("x2/package.json", r#"{ "name": "dup" }"#),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"web -> service x1 [ambiguous] {"declared":"*","scope":"runtime","via":"package"}"#,
                r#"web -> service x2 [ambiguous] {"declared":"*","scope":"runtime","via":"package"}"#,
                // A registry range may resolve to the registry: heuristic (the first field wins).
                r#"web -> service lib [heuristic] {"declared":"^1.0.0","scope":"runtime","via":"package"}"#,
                r#"lib -> service web [synthetic] {"declared":"workspace:*","scope":"runtime","via":"package"}"#,
            ]
        );
        assert!(npm_range_is_local("file:../lib", "web", "lib"));
        assert!(npm_range_is_local("link:./sub", "web", "web/sub"));
        assert!(!npm_range_is_local("file:../other", "web", "lib"));
    }

    #[test]
    fn go_requires_replaces_and_module_services() {
        let projects = [
            project("svc", &["svc/go.mod"]),
            project("lib", &["lib/go.mod"]),
            project("far", &["far/go.mod"]),
        ];
        let (edges, _) = run(
            &projects,
            &[
                (
                    "svc/go.mod",
                    "module example.com/svc\n\ngo 1.23\n\nrequire github.com/gin-gonic/gin v1.10.0\nrequire (\n\tgolang.org/x/text v0.14.0 // indirect\n\t\"example.com/lib\" v0.0.0\n\texample.com/far v1.2.0\n)\nreplace example.com/lib => ../lib\n",
                ),
                ("lib/go.mod", "module example.com/lib\n"),
                ("far/go.mod", "module example.com/far\n"),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"svc -> go/github.com/gin-gonic/gin@v1.10.0 [synthetic] {"declared":"v1.10.0","scope":"runtime","version_from":"go_mod"}"#,
                r#"svc -> service lib [synthetic] {"declared":"v0.0.0","scope":"runtime","via":"package"}"#,
                r#"svc -> service far [heuristic] {"declared":"v1.2.0","scope":"runtime","via":"package"}"#,
            ]
        );
        let parsed = go_mod("require a v1\n");
        assert_eq!(parsed.requires[0].0.span, [0, 8, 0, 9]);
    }

    #[test]
    fn pom_literals_properties_and_managed_versions() {
        let pom_text = r#"<?xml version="1.0"?>
<!-- a <dependency> in a comment is not one -->
<project>
  <parent><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter-parent</artifactId><version>3.3.4</version></parent>
  <groupId>com.acme</groupId>
  <artifactId>auth</artifactId>
  <version>1.4.0</version>
  <properties><jjwt.version>0.12.6</jjwt.version></properties>
  <dependencyManagement><dependencies>
    <dependency><groupId>x</groupId><artifactId>bom</artifactId><version>1</version><scope>import</scope></dependency>
  </dependencies></dependencyManagement>
  <dependencies>
    <dependency><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter-web</artifactId></dependency>
    <dependency><groupId>io.jsonwebtoken</groupId><artifactId>jjwt-api</artifactId><version>${jjwt.version}</version></dependency>
    <dependency><groupId>junit</groupId><artifactId>junit</artifactId><version>4.13.2</version><scope>test</scope></dependency>
    <dependency><groupId>a</groupId><artifactId>b</artifactId><version>${unknown}</version></dependency>
    <dependency><groupId>com.acme</groupId><artifactId>shared</artifactId><version>${project.version}</version></dependency>
  </dependencies>
  <build><plugins><plugin><dependencies><dependency><groupId>p</groupId><artifactId>q</artifactId><version>1</version></dependency></dependencies></plugin></plugins></build>
</project>
"#;
        let projects = [
            project("auth", &["auth/pom.xml"]),
            project("shared", &["shared/pom.xml"]),
        ];
        let (edges, _) = run(
            &projects,
            &[
                ("auth/pom.xml", pom_text),
                (
                    "shared/pom.xml",
                    "<project><groupId>com.acme</groupId><artifactId>shared</artifactId><version>1.4.0</version></project>",
                ),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"auth -> maven/org.springframework.boot/spring-boot-starter-web@? [exact] {"scope":"runtime","version_from":"unresolved"}"#,
                r#"auth -> maven/io.jsonwebtoken/jjwt-api@0.12.6 [synthetic] {"declared":"${jjwt.version}","scope":"runtime","version_from":"property"}"#,
                r#"auth -> maven/junit/junit@4.13.2 [exact] {"declared":"4.13.2","scope":"test","version_from":"literal"}"#,
                r#"auth -> maven/a/b@? [exact] {"declared":"${unknown}","scope":"runtime","version_from":"unresolved"}"#,
                // A property is not a literal: heuristic, though the versions agree.
                r#"auth -> service shared [heuristic] {"declared":"${project.version}","scope":"runtime","via":"package"}"#,
            ]
        );
        // The evidence is the dependency's `artifactId` line.
        let pom = pom(pom_text).unwrap();
        let line = pom_text
            .lines()
            .position(|l| l.contains("jjwt-api"))
            .unwrap();
        assert_eq!(pom.deps[1].0.span[0], i32::try_from(line).unwrap());
        // A literal equal to the module's version is synthetic.
        let (edges, _) = run(
            &projects,
            &[
                (
                    "auth/pom.xml",
                    "<project><groupId>g</groupId><artifactId>auth</artifactId><dependencies><dependency><groupId>com.acme</groupId><artifactId>shared</artifactId><version>1.4.0</version></dependency></dependencies></project>",
                ),
                (
                    "shared/pom.xml",
                    "<project><groupId>com.acme</groupId><artifactId>shared</artifactId><version>1.4.0</version></project>",
                ),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"auth -> service shared [synthetic] {"declared":"1.4.0","scope":"runtime","via":"package"}"#
            ]
        );
        assert!(parse_xml("<project><a></project>").is_none());
    }

    /// Evaluator pass 1, Y8 and Y11: an artifact declared twice with different versions, or
    /// a property defined twice with different values, has no exact version: one `heuristic`
    /// edge to the `@?` key listing the versions. The same version twice stays exact.
    #[test]
    fn duplicate_declarations_and_properties_are_unresolved() {
        let projects = [project("svc", &["svc/pom.xml"])];
        let pom = |body: &str| -> String {
            format!(
                "<project><groupId>g</groupId><artifactId>svc</artifactId><version>1</version>{body}</project>"
            )
        };
        let dep = |v: &str| {
            format!(
                "<dependency><groupId>org.example</groupId><artifactId>dup</artifactId><version>{v}</version></dependency>"
            )
        };
        let twice = pom(&format!(
            "<dependencies>{}{}</dependencies>",
            dep("1.0"),
            dep("2.0")
        ));
        let (edges, _) = run(&projects, &[("svc/pom.xml", &twice)]);
        assert_eq!(
            edges,
            [
                r#"svc -> maven/org.example/dup@? [heuristic] {"declared":"1.0","scope":"runtime","version_from":"unresolved","versions":["1.0","2.0"]}"#
            ]
        );
        let same = pom(&format!(
            "<dependencies>{}{}</dependencies>",
            dep("1.0"),
            dep("1.0")
        ));
        let (edges, _) = run(&projects, &[("svc/pom.xml", &same)]);
        assert_eq!(
            edges,
            [
                r#"svc -> maven/org.example/dup@1.0 [exact] {"declared":"1.0","scope":"runtime","version_from":"literal"}"#
            ]
        );
        let props = pom(&format!(
            "<properties><lib.version>1.0</lib.version><lib.version>2.0</lib.version></properties><dependencies>{}</dependencies>",
            dep("${lib.version}")
        ));
        let (edges, _) = run(&projects, &[("svc/pom.xml", &props)]);
        assert_eq!(
            edges,
            [
                r#"svc -> maven/org.example/dup@? [heuristic] {"declared":"${lib.version}","scope":"runtime","version_from":"unresolved","versions":["1.0","2.0"]}"#
            ]
        );
    }

    /// Evaluator pass 1, Y9: an npm alias (`"foo": "npm:bar@^1"`) installs `bar`: the key is
    /// the real package with the lockfile's version, and the alias is an attribute.
    #[test]
    fn npm_aliases_key_the_real_package() {
        let projects = [project("web", &["web/package.json"])];
        let lock = r#"{ "lockfileVersion": 3, "packages": {
            "": {},
            "node_modules/foo": { "name": "bar", "version": "1.2.3" },
            "node_modules/sc": { "name": "@s/pkg", "version": "2.0.0" }
        } }"#;
        let manifest = r#"{ "name": "web", "dependencies": { "foo": "npm:bar@^1.0.0", "sc": "npm:@s/pkg@^2", "nolock": "npm:real@1" } }"#;
        let (edges, _) = run(
            &projects,
            &[
                ("web/package.json", manifest),
                ("web/package-lock.json", lock),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"web -> npm/bar@1.2.3 [exact] {"alias":"foo","declared":"npm:bar@^1.0.0","scope":"runtime","version_from":"lockfile"}"#,
                r#"web -> npm/real@? [exact] {"alias":"nolock","declared":"npm:real@1","scope":"runtime","version_from":"unresolved"}"#,
                r#"web -> npm/@s/pkg@2.0.0 [exact] {"alias":"sc","declared":"npm:@s/pkg@^2","scope":"runtime","version_from":"lockfile"}"#,
            ]
        );
    }

    /// Evaluator pass 1, E2-1: an absolute-path `replace` target joins no
    /// service: `/b` is a filesystem path, never the repo-relative `a/b`
    /// below the service. The version stays unresolved (Y10), as for any
    /// directory replace.
    #[test]
    fn absolute_replace_paths_never_join_a_service() {
        let projects = [project("a", &["a/go.mod"]), project("a/b", &["a/b/go.mod"])];
        let (edges, _) = run(
            &projects,
            &[
                (
                    "a/go.mod",
                    "module example.com/a\n\nrequire example.com/dep v1.0.0\nreplace example.com/dep => /b\n",
                ),
                ("a/b/go.mod", "module example.com/dep\n"),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"a -> go/example.com/dep@? [exact] {"declared":"v1.0.0","replaced":"/b","scope":"runtime","version_from":"unresolved"}"#,
            ]
        );
    }

    /// Evaluator pass 1, Y10: a `replace` to another version of the same module sets the
    /// version; one to a local directory that is no service, or to another module, leaves it
    /// unresolved; a replace pinned to another required version does not apply.
    #[test]
    fn go_replace_directives() {
        let projects = [project("svc", &["svc/go.mod"])];
        let go = "module example.com/svc\n\nrequire (\n\tgithub.com/gin-gonic/gin v1.10.0\n\texample.com/a v1.0.0\n\texample.com/b v1.0.0\n\texample.com/c v1.0.0\n)\nreplace github.com/gin-gonic/gin => github.com/gin-gonic/gin v1.9.1\nreplace example.com/a => ./vendor/a\nreplace (\n\texample.com/b v1.0.0 => example.com/fork v1.1.0\n\texample.com/c v0.9.0 => example.com/c v0.9.1\n)\n";
        let (edges, _) = run(&projects, &[("svc/go.mod", go)]);
        assert_eq!(
            edges,
            [
                r#"svc -> go/github.com/gin-gonic/gin@v1.9.1 [synthetic] {"declared":"v1.10.0","replaced":"github.com/gin-gonic/gin v1.9.1","scope":"runtime","version_from":"go_mod"}"#,
                r#"svc -> go/example.com/a@? [exact] {"declared":"v1.0.0","replaced":"./vendor/a","scope":"runtime","version_from":"unresolved"}"#,
                r#"svc -> go/example.com/b@? [exact] {"declared":"v1.0.0","replaced":"example.com/fork v1.1.0","scope":"runtime","version_from":"unresolved"}"#,
                r#"svc -> go/example.com/c@v1.0.0 [synthetic] {"declared":"v1.0.0","scope":"runtime","version_from":"go_mod"}"#,
            ]
        );
    }

    /// D8: the classpath closure terminates on a cycle and follows edges transitively.
    #[test]
    fn classpath_closure_with_a_cycle() {
        let depends: BTreeMap<String, BTreeSet<String>> =
            [("a", vec!["b"]), ("b", vec!["c"]), ("c", vec!["a", "d"])]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.into_iter().map(str::to_owned).collect()))
                .collect();
        let got: Vec<String> = classpath("a", &depends).into_iter().collect();
        assert_eq!(got, ["a", "b", "c", "d"]);
        let got: Vec<String> = classpath("d", &depends).into_iter().collect();
        assert_eq!(got, ["d"]);
    }

    /// Contract M6d S6: PEP 508 requirement strings split into a canonical
    /// name and the declared spec as written; extras, markers and URL targets
    /// never become versions; anything else names no dependency.
    #[test]
    fn pep508_requirements_split_names_and_specs() {
        let req = |s: &str| pep508(s).map(|(n, d, _)| (n, d));
        assert_eq!(
            req("fastapi[standard]>=0.141.1,<1.0.0"),
            Some(("fastapi".into(), ">=0.141.1,<1.0.0".into()))
        );
        assert_eq!(req("Requests>=2"), Some(("requests".into(), ">=2".into())));
        assert_eq!(req("foo_bar.baz"), Some(("foo-bar-baz".into(), "".into())));
        assert_eq!(
            req("pwdlib[argon2,bcrypt]>=0.3.1"),
            Some(("pwdlib".into(), ">=0.3.1".into()))
        );
        assert_eq!(
            req("pyjwt>=2.15.0,<3.0.0; python_version >= '3.9'"),
            Some(("pyjwt".into(), ">=2.15.0,<3.0.0".into()))
        );
        assert_eq!(
            req("name @ https://example.com/name-1.0.tar.gz"),
            Some(("name".into(), "".into()))
        );
        assert_eq!(
            req("name[extra]@ https://example.com/n.tgz"),
            Some(("name".into(), "".into()))
        );
        // A parenthesised spec is tolerated; anything without a name is not a dep.
        assert_eq!(req("legacy (>=2)"), Some(("legacy".into(), ">=2".into())));
        assert_eq!(req(""), None);
        assert_eq!(req(">=2"), None);
        assert_eq!(req("; python_version >= '3'"), None);
        assert_eq!(req("[extra]>=2"), None);
        assert_eq!(req("foo[bar>=2"), None);
    }

    /// Contract M6d S6: names canonicalise per PEP 503.
    #[test]
    fn pep503_folds_case_and_separators() {
        assert_eq!(pep503("Requests"), "requests");
        assert_eq!(pep503("foo_bar.baz-qux"), "foo-bar-baz-qux");
        assert_eq!(pep503("a__b..c--d"), "a-b-c-d");
    }

    /// Contract M6d S6: `pyproject.toml` and both lockfiles are manifest inputs.
    #[test]
    fn python_manifests_are_inputs() {
        for f in [
            "pyproject.toml",
            "poetry.lock",
            "uv.lock",
            "svc/pyproject.toml",
        ] {
            assert!(is_input(f), "{f}");
        }
        assert!(!is_input("svc/requirements.txt"));
        assert!(!is_input("svc/Pipfile.lock"));
    }

    const POETRY_PYPROJECT: &str = r#"[tool.poetry]
name = "svc"
version = "0.1.0"

[tool.poetry.dependencies]
python = ">=3.10,<4"
requests = "^2.32.0"
shaped = { version = "^1.0", extras = ["fast"] }
gitted = { git = "https://example.com/gitted.git", rev = "abc" }
pathed = { path = "../pathed" }
multi = [{ version = "^2.0", python = ">=3.10" }, { version = "^1.0", python = "<3.10" }]
"#;

    const POETRY_LOCK: &str = r#"[[package]]
name = "requests"
version = "2.32.3"

[[package]]
name = "shaped"
version = "1.2.0"

[[package]]
name = "gitted"
version = "0.4.0"

[package.source]
type = "git"
url = "https://example.com/gitted.git"

[[package]]
name = "transitive"
version = "9.9.9"
"#;

    /// Contract M6d S6: a `poetry.lock` pins exact registry versions; a git
    /// source never pins; transitive entries and `[tool.poetry]` extras stay
    /// out; `python` is the runtime, not a dependency.
    #[test]
    fn poetry_lock_pins_exact_registry_versions() {
        let projects = [project("svc", &["svc/pyproject.toml"])];
        let (edges, diagnostics) = run(
            &projects,
            &[
                ("svc/pyproject.toml", POETRY_PYPROJECT),
                ("svc/poetry.lock", POETRY_LOCK),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"svc -> py/gitted@? [exact] {"scope":"runtime","version_from":"unresolved"}"#,
                r#"svc -> py/multi@? [exact] {"declared":"^2.0","scope":"runtime","version_from":"unresolved"}"#,
                r#"svc -> py/pathed@? [exact] {"scope":"runtime","version_from":"unresolved"}"#,
                r#"svc -> py/requests@2.32.3 [exact] {"declared":"^2.32.0","scope":"runtime","version_from":"poetry_lock"}"#,
                r#"svc -> py/shaped@1.2.0 [exact] {"declared":"^1.0","scope":"runtime","version_from":"poetry_lock"}"#,
            ]
        );
        assert_eq!(
            diagnostics,
            [
                "lockfile_missing_entry svc/pyproject.toml",
                "lockfile_missing_entry svc/pyproject.toml",
                "lockfile_missing_entry svc/pyproject.toml"
            ]
        );
    }

    const PEP621_PYPROJECT: &str = r#"[project]
name = "app"
version = "0.1.0"
dependencies = [
    "fastapi[standard]>=0.141.1,<1.0.0",
    "httpx<1.0.0,>=0.25.1",
    "bare",
    "missing>=1.0",
]
"#;

    const UV_LOCK: &str = r#"version = 1

[[package]]
name = "fastapi"
version = "0.143.0"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "httpx"
version = "0.28.1"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "editable-root"
version = "0.1.0"
source = { editable = "." }
"#;

    /// Contract M6d S6: PEP 621 dependencies pin from the governing `uv.lock`;
    /// a dep the lock does not list stays `?` with a counted diagnostic.
    #[test]
    fn uv_lock_pins_pep621_dependencies() {
        let projects = [project("svc", &["svc/pyproject.toml"])];
        let (edges, diagnostics) = run(
            &projects,
            &[
                ("svc/pyproject.toml", PEP621_PYPROJECT),
                ("svc/uv.lock", UV_LOCK),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"svc -> py/fastapi@0.143.0 [exact] {"declared":">=0.141.1,<1.0.0","scope":"runtime","version_from":"uv_lock"}"#,
                r#"svc -> py/httpx@0.28.1 [exact] {"declared":"<1.0.0,>=0.25.1","scope":"runtime","version_from":"uv_lock"}"#,
                r#"svc -> py/bare@? [exact] {"scope":"runtime","version_from":"unresolved"}"#,
                r#"svc -> py/missing@? [exact] {"declared":">=1.0","scope":"runtime","version_from":"unresolved"}"#,
            ]
        );
        assert_eq!(
            diagnostics,
            [
                "lockfile_missing_entry svc/pyproject.toml",
                "lockfile_missing_entry svc/pyproject.toml"
            ]
        );
    }

    const UV_ROOT_PYPROJECT: &str = r#"[tool.uv.workspace]
members = ["backend", "packages/*"]
"#;

    /// Contract M6d S6: a `uv.lock` governs its own directory and the
    /// workspace members the root lists (exact or one-level `*`); an
    /// unrelated nested project keeps `?`. A `poetry.lock` above never
    /// governs below (poetry has no workspaces). Both locks in one directory:
    /// poetry wins.
    #[test]
    fn uv_lock_governs_members_and_poetry_stays_local() {
        let projects = [
            project(".", &["pyproject.toml"]),
            project("backend", &["backend/pyproject.toml"]),
            project("packages/a", &["packages/a/pyproject.toml"]),
            project("other", &["other/pyproject.toml"]),
            project("nested", &["nested/pyproject.toml"]),
            project("both", &["both/pyproject.toml"]),
        ];
        let backend = "[project]\nname = \"app\"\ndependencies = [\"httpx>=0.25\"]\n";
        let (edges, _) = run(
            &projects,
            &[
                ("pyproject.toml", UV_ROOT_PYPROJECT),
                ("uv.lock", UV_LOCK),
                ("backend/pyproject.toml", backend),
                ("packages/a/pyproject.toml", backend),
                ("other/pyproject.toml", backend),
                ("nested/pyproject.toml", backend),
                ("poetry.lock", POETRY_LOCK),
                (
                    "nested/poetry.lock",
                    "[[package]]\nname = \"httpx\"\nversion = \"0.1.0\"\n",
                ),
                ("both/pyproject.toml", backend),
                (
                    "both/poetry.lock",
                    "[[package]]\nname = \"httpx\"\nversion = \"0.2.0\"\n",
                ),
                ("both/uv.lock", UV_LOCK),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"backend -> py/httpx@0.28.1 [exact] {"declared":">=0.25","scope":"runtime","version_from":"uv_lock"}"#,
                r#"packages/a -> py/httpx@0.28.1 [exact] {"declared":">=0.25","scope":"runtime","version_from":"uv_lock"}"#,
                r#"other -> py/httpx@? [exact] {"declared":">=0.25","scope":"runtime","version_from":"unresolved"}"#,
                r#"nested -> py/httpx@0.1.0 [exact] {"declared":">=0.25","scope":"runtime","version_from":"poetry_lock"}"#,
                r#"both -> py/httpx@0.2.0 [exact] {"declared":">=0.25","scope":"runtime","version_from":"poetry_lock"}"#,
            ]
        );
    }

    /// Contract M6d S6: an unreadable lockfile is a counted diagnostic, and a
    /// `requirements.txt`/`Pipfile` is noticed but never read (residue).
    #[test]
    fn unreadable_and_unread_python_lockfiles() {
        let projects = [
            project("a", &["a/pyproject.toml"]),
            project("b", &["b/pyproject.toml"]),
        ];
        let manifest = "[project]\nname = \"x\"\ndependencies = [\"httpx>=0.25\"]\n";
        let (edges, diagnostics) = run(
            &projects,
            &[
                ("a/pyproject.toml", manifest),
                ("a/poetry.lock", "[[package]"),
                ("b/pyproject.toml", manifest),
                ("b/requirements.txt", "httpx==0.28.1\n"),
            ],
        );
        assert_eq!(
            edges,
            [
                r#"a -> py/httpx@? [exact] {"declared":">=0.25","scope":"runtime","version_from":"unresolved"}"#,
                r#"b -> py/httpx@? [exact] {"declared":">=0.25","scope":"runtime","version_from":"unresolved"}"#,
            ]
        );
        assert_eq!(
            diagnostics,
            [
                "lockfile_unreadable a/poetry.lock",
                "lockfile_unread b/requirements.txt"
            ]
        );
    }

    /// Contract M6d S6: only registry (or absent) sources pin; git,
    /// editable, directory and url sources never do.
    #[test]
    fn uv_lock_skips_non_registry_sources() {
        let lock = uv_lock(
            r#"version = 1
[[package]]
name = "reg"
version = "1.0"
source = { registry = "https://pypi.org/simple" }
[[package]]
name = "bare"
version = "1.0"
[[package]]
name = "gitted"
version = "1.0"
source = { git = "https://example.com/g.git" }
[[package]]
name = "ed"
version = "1.0"
source = { editable = "." }
[[package]]
name = "dir"
version = "1.0"
source = { directory = "../dir" }
"#,
        )
        .unwrap();
        assert_eq!(
            lock.into_iter().collect::<Vec<_>>(),
            [
                ("bare".to_owned(), "1.0".to_owned()),
                ("reg".to_owned(), "1.0".to_owned())
            ]
        );
        assert!(uv_lock("version = 1\n").is_err());
        assert!(poetry_lock("[[package]\n").is_err());
    }

    /// Contract M6d S6: evidence points at the dependency's own entry.
    #[test]
    fn python_dependency_spans_point_at_the_entry() {
        let text = PEP621_PYPROJECT;
        let deps = pyproject_deps(text);
        assert_eq!(deps.len(), 4);
        let at = |name: &str| {
            let start = text.find(name).unwrap();
            span_of(text, start, start + name.len())
        };
        assert_eq!(deps[0].span, at("fastapi"));
        assert_eq!(deps[2].span, at("\"bare\"\n".trim_matches(['"', '\n'])));
        let poetry = pyproject_deps(POETRY_PYPROJECT);
        let key = |name: &str| {
            let line = POETRY_PYPROJECT
                .lines()
                .find(|l| l.starts_with(name))
                .unwrap();
            let start = POETRY_PYPROJECT.find(line).unwrap();
            span_of(POETRY_PYPROJECT, start, start + name.len())
        };
        assert_eq!(
            poetry.iter().find(|d| d.name == "requests").unwrap().span,
            key("requests")
        );
    }
}
