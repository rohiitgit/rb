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
        "package.json" | "package-lock.json" | "npm-shrinkwrap.json" | "go.mod" | "pom.xml"
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

/// The lockfile governing a service directory: the nearest directory, from the service's up to
/// the root, holding `npm-shrinkwrap.json` or `package-lock.json` (`Ok`), or else `yarn.lock` or
/// `pnpm-lock.yaml` (`Err`, not read).
fn lockfile_for(service_dir: &str, paths: &BTreeSet<&str>) -> Option<Result<String, String>> {
    let mut dir = Some(service_dir);
    while let Some(d) = dir {
        let prefix = dir_prefix(d);
        for name in ["npm-shrinkwrap.json", "package-lock.json"] {
            let path = format!("{prefix}{name}");
            if paths.contains(path.as_str()) {
                return Some(Ok(path));
            }
        }
        for name in ["yarn.lock", "pnpm-lock.yaml"] {
            let path = format!("{prefix}{name}");
            if paths.contains(path.as_str()) {
                return Some(Err(path));
            }
        }
        dir = parent_dir(d);
    }
    None
}

/// The package an npm alias installs: `npm:bar@^1` → `bar`, `npm:@s/p@2` → `@s/p`.
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

/// `go.mod`'s direct requirements (no `// indirect`) and its `replace` directives.
#[derive(Debug, Default)]
struct GoMod {
    requires: Vec<(Dep, String)>,
    replaces: Vec<Replace>,
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
            _ => {}
        }
    }
    out
}

/// A minimal XML element: name, direct text, children, and where its start tag begins.
#[derive(Debug, Default)]
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
                        Some(Ok(path)) => {
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
                                        Some((path.clone(), packages, rel.to_owned()))
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
                        Some(Err(path)) => {
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
                        let (version, from) = match &lock {
                            Some((path, packages, rel)) => {
                                match lock_version(packages, rel, &dep.name) {
                                    Some(v) => (v, "lockfile"),
                                    None => {
                                        diagnostics.push(diagnostic(
                                            "lockfile_missing_entry",
                                            m,
                                            Some(dep.span),
                                            json!({ "service": service, "name": dep.name, "lockfile": path }),
                                        ));
                                        (String::new(), "unresolved")
                                    }
                                }
                            }
                            None => (String::new(), "unresolved"),
                        };
                        let alias = if real.is_some() {
                            dep.name.as_str()
                        } else {
                            ""
                        };
                        edges.push(ManifestEdge {
                            service: service.to_owned(),
                            target: Target::Package(Package {
                                manager: "npm",
                                name: real.unwrap_or_else(|| dep.name.clone()),
                                version,
                            }),
                            certainty: Certainty::Exact,
                            attrs: attrs(&[
                                ("alias", alias),
                                ("declared", &dep.declared),
                                ("scope", dep.scope),
                                ("version_from", from),
                            ]),
                            evidence: evidence(dep.span),
                        });
                    }
                }
                "go.mod" => {
                    let parsed = go_mod(text);
                    for (dep, version) in &parsed.requires {
                        let replace = parsed.replace_for(&dep.name, version);
                        let local = replace
                            .filter(|r| r.local())
                            .and_then(|r| join(&here, &r.target));
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
                            match go_modules.get(&dep.name).map(Vec::as_slice) {
                                Some([one]) => (vec![*one], Certainty::Heuristic),
                                Some(many) => (many.to_vec(), Certainty::Ambiguous),
                                None => (Vec::new(), Certainty::Heuristic),
                            }
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
                r#"b -> npm/express@? [exact] {"declared":"^4","scope":"runtime","version_from":"unresolved"}"#,
            ]
        );
        assert_eq!(
            diagnostics,
            [
                "lockfile_unreadable a/package-lock.json",
                "lockfile_unread b/yarn.lock"
            ]
        );
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
}
