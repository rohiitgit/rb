//! Service / project boundary detection (design §4.3).
//!
//! Every directory holding a recognised build manifest is a **project**; each project becomes a
//! `Service` node with `deployable` telling services (runnable) apart from libraries. Files belong
//! to the innermost project containing them. `rb.toml` at the repo root can exclude manifests from
//! detection and add or override projects.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, bail};
use rb_model::{Diagnostic, FileEntry};
use serde::Deserialize;
use serde_json::json;

use crate::glob::Pattern;

/// Directory key of the repository root.
pub const ROOT_DIR: &str = ".";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// Repo-relative directory, [`ROOT_DIR`] for the root.
    pub dir: String,
    pub name: String,
    /// Detectors that recognised this directory, sorted (e.g. `["dockerfile", "package.json"]`).
    pub detected_by: Vec<String>,
    /// Manifest files that define the project, sorted.
    pub manifests: Vec<String>,
    pub deployable: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Boundaries {
    /// Sorted by `dir` (enforced by [`Boundaries::new`]); `project_for` relies on it.
    projects: Vec<Project>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Boundaries {
    pub fn new(mut projects: Vec<Project>, diagnostics: Vec<Diagnostic>) -> Self {
        projects.sort_by(|a, b| a.dir.cmp(&b.dir));
        Self {
            projects,
            diagnostics,
        }
    }

    /// All projects, sorted by directory.
    pub fn projects(&self) -> &[Project] {
        &self.projects
    }

    /// The innermost project whose directory contains `file`.
    pub fn project_for(&self, file: &str) -> Option<&Project> {
        let mut dir = parent_dir(file);
        loop {
            // `projects` is sorted by `dir`.
            if let Ok(i) = self.projects.binary_search_by(|p| p.dir.as_str().cmp(dir)) {
                return Some(&self.projects[i]);
            }
            if dir == ROOT_DIR {
                return None;
            }
            dir = parent_dir(dir);
        }
    }
}

/// `rb.toml` (design §4.3 "explicit overrides, which always win").
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct RbToml {
    /// Patterns (CODEOWNERS dialect) of manifest files to ignore during detection.
    #[serde(default)]
    exclude: Vec<String>,
    #[serde(default)]
    service: Vec<ServiceOverride>,
    /// Host mappings (contract M6g S4): read by the indexer, ignored here beyond parsing.
    #[serde(default)]
    hosts: Vec<toml::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceOverride {
    path: String,
    name: Option<String>,
    deployable: Option<bool>,
}

/// Accumulates detector results per directory.
#[derive(Default)]
struct Found {
    /// Names reported by detectors, keyed by detector priority (lower wins).
    names: BTreeMap<u8, String>,
    /// From `rb.toml`; beats every detector.
    override_name: Option<String>,
    detected_by: BTreeSet<String>,
    manifests: BTreeSet<String>,
    deployable: bool,
}

/// Detects projects among `files`. `read` returns a file's text (used only for manifests).
pub fn detect(
    files: &[FileEntry],
    read: &dyn Fn(&FileEntry) -> anyhow::Result<String>,
) -> anyhow::Result<Boundaries> {
    // Manifests are parsed, never spanned, so a byte order mark can simply be dropped.
    let read = |f: &FileEntry| read(f).map(|t| crate::strip_bom(&t).to_owned());
    let by_path: BTreeMap<&str, &FileEntry> = files.iter().map(|f| (f.path.as_str(), f)).collect();
    let config = match by_path.get("rb.toml") {
        Some(f) => toml::from_str::<RbToml>(&read(f)?).context("parsing rb.toml")?,
        None => RbToml::default(),
    };
    let excludes = config
        .exclude
        .iter()
        .map(|p| Pattern::parse(p).map_err(|e| anyhow::anyhow!("rb.toml exclude {p:?}: {e}")))
        .collect::<anyhow::Result<Vec<_>>>()?;

    let mut found: BTreeMap<String, Found> = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for f in files {
        if f.binary || excludes.iter().any(|p| p.matches(&f.path)) {
            continue;
        }
        let dir = parent_dir(&f.path).to_owned();
        let base = f.path.rsplit('/').next().unwrap_or(&f.path);
        let detected = match base {
            "package.json" => Some(package_json(&read(f)?)),
            "pom.xml" => Some(pom_xml(&read(f)?)),
            "build.gradle" | "build.gradle.kts" => Some(Ok(gradle(&read(f)?))),
            "go.mod" => Some(Ok(go_mod(&read(f)?))),
            "Cargo.toml" => cargo_toml(&read(f)?, &dir, &by_path).transpose(),
            "pyproject.toml" => Some(pyproject(&read(f)?)),
            "setup.py" => Some(Ok(Detected::new(3, "setup.py", None, false))),
            _ if base == "Dockerfile" || base.starts_with("Dockerfile.") => {
                Some(Ok(Detected::new(0, "dockerfile", None, true)))
            }
            _ if base.ends_with(".go") && !base.ends_with("_test.go") => {
                is_go_main(&read(f)?).then(|| Ok(Detected::new(1, "go-main", None, true)))
            }
            _ => None,
        };
        let detected = match detected {
            None => continue,
            Some(Ok(d)) => d,
            Some(Err(err)) => {
                diagnostics.push(Diagnostic {
                    kind: "manifest_parse_error".into(),
                    path: Some(f.path.clone()),
                    span: None,
                    detail: json!({ "error": format!("{err:#}") }),
                });
                // Still a project boundary: the manifest exists, we just could not read its fields.
                Detected::new(2, manifest_detector(base), None, false)
            }
        };
        let entry = found.entry(dir).or_default();
        if let Some(name) = detected.name {
            entry.names.entry(detected.priority).or_insert(name);
        }
        entry.detected_by.insert(detected.detector.into());
        if detected.detector != "dockerfile" && detected.detector != "go-main" {
            entry.manifests.insert(f.path.clone());
        }
        entry.deployable |= detected.deployable;
    }

    for o in &config.service {
        let dir = normalise_dir(&o.path);
        let prefix = format!("{dir}/");
        let exists = dir == ROOT_DIR || by_path.keys().any(|p| p.starts_with(&prefix));
        if !exists {
            bail!(
                "rb.toml service path {:?} is not a directory in this commit",
                o.path
            );
        }
        let entry = found.entry(dir).or_insert_with(|| Found {
            deployable: true,
            ..Found::default()
        });
        entry.detected_by.insert("rb.toml".into());
        entry.override_name.clone_from(&o.name);
        if let Some(d) = o.deployable {
            entry.deployable = d;
        }
    }

    let projects = found
        .into_iter()
        .map(|(dir, f)| Project {
            name: f
                .override_name
                .or_else(|| f.names.into_values().next())
                .unwrap_or_else(|| default_name(&dir)),
            detected_by: f.detected_by.into_iter().collect(),
            manifests: f.manifests.into_iter().collect(),
            deployable: f.deployable,
            dir,
        })
        .collect();
    Ok(Boundaries::new(projects, diagnostics))
}

struct Detected {
    /// Lower wins when several detectors name the same directory.
    priority: u8,
    detector: &'static str,
    name: Option<String>,
    deployable: bool,
}

impl Detected {
    fn new(priority: u8, detector: &'static str, name: Option<String>, deployable: bool) -> Self {
        Self {
            priority,
            detector,
            name,
            deployable,
        }
    }
}

fn manifest_detector(base: &str) -> &'static str {
    match base {
        "package.json" => "package.json",
        "pom.xml" => "maven",
        "Cargo.toml" => "cargo",
        "pyproject.toml" => "pyproject",
        _ => "manifest",
    }
}

fn package_json(text: &str) -> anyhow::Result<Detected> {
    let v: serde_json::Value = serde_json::from_str(text)?;
    let name = v["name"].as_str().map(str::to_owned);
    let deployable = v["scripts"]["start"].is_string();
    Ok(Detected::new(2, "package.json", name, deployable))
}

fn xml_child<'a, 'i>(node: roxmltree::Node<'a, 'i>, tag: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == tag)
}

fn xml_text(node: Option<roxmltree::Node<'_, '_>>) -> Option<String> {
    node.and_then(|n| n.text()).map(|t| t.trim().to_owned())
}

fn pom_xml(text: &str) -> anyhow::Result<Detected> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: true,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(text, options)?;
    let project = doc.root_element();
    let name = xml_text(xml_child(project, "artifactId"));
    let packaging = xml_text(xml_child(project, "packaging")).unwrap_or_else(|| "jar".into());
    let boot_parent =
        xml_text(xml_child(project, "parent").and_then(|p| xml_child(p, "artifactId")))
            .is_some_and(|a| a == "spring-boot-starter-parent");
    let boot_plugin = project.descendants().any(|n| {
        n.tag_name().name() == "artifactId"
            && n.text().map(str::trim) == Some("spring-boot-maven-plugin")
    });
    let deployable = packaging != "pom" && (boot_parent || boot_plugin);
    Ok(Detected::new(2, "maven", name, deployable))
}

fn gradle(text: &str) -> Detected {
    let deployable = text.contains("org.springframework.boot")
        || [
            "id(\"application\")",
            "id 'application'",
            "id \"application\"",
            "plugin: 'application'",
            "plugin: \"application\"",
        ]
        .iter()
        .any(|p| text.contains(p));
    Detected::new(3, "gradle", None, deployable)
}

fn go_mod(text: &str) -> Detected {
    Detected::new(2, "go.mod", go_module_path(text), false)
}

/// The module path declared by a `go.mod` file.
pub fn go_module_path(text: &str) -> Option<String> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("module "))
        .map(|m| m.trim().trim_matches('"').to_owned())
}

/// True if the first `package` clause (outside comments) is `package main`.
fn is_go_main(text: &str) -> bool {
    let mut rest = text;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix("//") {
            rest = after.split_once('\n').map_or("", |(_, r)| r);
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, r)| r);
        } else {
            return rest
                .strip_prefix("package")
                .filter(|r| r.starts_with(char::is_whitespace))
                .and_then(|r| r.split_whitespace().next())
                == Some("main");
        }
    }
}

fn cargo_toml(
    text: &str,
    dir: &str,
    by_path: &BTreeMap<&str, &FileEntry>,
) -> anyhow::Result<Option<Detected>> {
    let v: toml::Table = toml::from_str(text)?;
    let Some(package) = v.get("package").and_then(|p| p.as_table()) else {
        return Ok(None); // a virtual workspace manifest is not a project
    };
    let name = package
        .get("name")
        .and_then(|n| n.as_str())
        .map(str::to_owned);
    let main_rs = join(dir, "src/main.rs");
    let deployable = v.contains_key("bin") || by_path.contains_key(main_rs.as_str());
    Ok(Some(Detected::new(2, "cargo", name, deployable)))
}

fn pyproject(text: &str) -> anyhow::Result<Detected> {
    let v: toml::Table = toml::from_str(text)?;
    let get = |path: &[&str]| {
        path.iter()
            .try_fold(&toml::Value::Table(v.clone()), |acc, k| acc.get(k))
            .cloned()
    };
    let name = get(&["project", "name"])
        .or_else(|| get(&["tool", "poetry", "name"]))
        .and_then(|n| n.as_str().map(str::to_owned));
    let deployable =
        get(&["project", "scripts"]).is_some() || get(&["tool", "poetry", "scripts"]).is_some();
    Ok(Detected::new(2, "pyproject", name, deployable))
}

/// Parent directory of a repo-relative path (`.` for top-level entries).
pub fn parent_dir(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[..i],
        None => ROOT_DIR,
    }
}

fn join(dir: &str, rel: &str) -> String {
    if dir == ROOT_DIR {
        rel.into()
    } else {
        format!("{dir}/{rel}")
    }
}

fn normalise_dir(path: &str) -> String {
    let p = path.trim().trim_start_matches("./").trim_matches('/');
    if p.is_empty() || p == "." {
        ROOT_DIR.into()
    } else {
        p.into()
    }
}

fn default_name(dir: &str) -> String {
    if dir == ROOT_DIR {
        "(repo root)".into()
    } else {
        dir.rsplit('/').next().unwrap_or(dir).into()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn detect_in(tree: &[(&str, &str)]) -> anyhow::Result<Boundaries> {
        let contents: HashMap<String, String> = tree
            .iter()
            .map(|(p, c)| ((*p).to_owned(), (*c).to_owned()))
            .collect();
        let mut files: Vec<FileEntry> = tree
            .iter()
            .map(|(p, c)| FileEntry {
                path: (*p).into(),
                blob_sha: String::new(),
                size: c.len() as u64,
                executable: false,
                lang: None,
                binary: false,
                generated: false,
            })
            .collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        detect(&files, &|f| Ok(contents[&f.path].clone()))
    }

    fn summary(b: &Boundaries) -> Vec<(String, String, bool)> {
        b.projects
            .iter()
            .map(|p| (p.dir.clone(), p.name.clone(), p.deployable))
            .collect()
    }

    fn s(dir: &str, name: &str, deployable: bool) -> (String, String, bool) {
        (dir.into(), name.into(), deployable)
    }

    const BOOT_POM: &str = r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
  <parent><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter-parent</artifactId></parent>
  <artifactId>auth-service</artifactId>
</project>"#;

    #[test]
    fn detects_each_ecosystem() {
        let b = detect_in(&[
            (
                "package.json",
                r#"{"name":"mono","private":true,"workspaces":["apps/*"]}"#,
            ),
            (
                "apps/web/package.json",
                r#"{"name":"@acme/web","scripts":{"start":"vite"}}"#,
            ),
            ("apps/web/src/main.ts", ""),
            (
                "libs/shared/package.json",
                r#"{"name":"@acme/shared","main":"index.js"}"#,
            ),
            ("services/auth/pom.xml", BOOT_POM),
            ("services/auth/src/main/java/A.java", ""),
            (
                "services/agg/pom.xml",
                "<project><artifactId>agg</artifactId><packaging>pom</packaging></project>",
            ),
            (
                "services/notify/go.mod",
                "module github.com/acme/notify\n\ngo 1.23\n",
            ),
            ("services/notify/internal/mail.go", "package mail\n"),
            (
                "services/notify/cmd/notify/main.go",
                "// Command notify.\n/* block\n */\npackage main\n",
            ),
            ("tools/gen/Cargo.toml", "[package]\nname = \"gen\"\n"),
            ("tools/gen/src/main.rs", "fn main() {}"),
            ("tools/Cargo.toml", "[workspace]\nmembers = [\"gen\"]\n"),
            (
                "ml/pyproject.toml",
                "[project]\nname = \"ml\"\n[project.scripts]\ntrain = \"ml:main\"\n",
            ),
            ("legacy/Dockerfile", "FROM scratch\n"),
            ("legacy/app.sh", ""),
            (
                "gradle-svc/build.gradle.kts",
                "plugins { id(\"org.springframework.boot\") }\n",
            ),
        ])
        .unwrap();
        assert_eq!(
            summary(&b),
            [
                s(".", "mono", false),
                s("apps/web", "@acme/web", true),
                s("gradle-svc", "gradle-svc", true),
                s("legacy", "legacy", true),
                s("libs/shared", "@acme/shared", false),
                s("ml", "ml", true),
                s("services/agg", "agg", false),
                s("services/auth", "auth-service", true),
                s("services/notify", "github.com/acme/notify", false),
                s("services/notify/cmd/notify", "notify", true),
                s("tools/gen", "gen", true),
            ]
        );
        assert!(b.diagnostics.is_empty());
        assert_eq!(
            b.project_for("apps/web/src/main.ts").unwrap().dir,
            "apps/web"
        );
        assert_eq!(
            b.project_for("services/notify/internal/mail.go")
                .unwrap()
                .dir,
            "services/notify"
        );
        assert_eq!(b.project_for("README.md").unwrap().dir, ".");
        let auth = &b.projects[7];
        assert_eq!(auth.manifests, ["services/auth/pom.xml"]);
        assert_eq!(auth.detected_by, ["maven"]);
    }

    #[test]
    fn dockerfile_makes_a_library_deployable() {
        let b = detect_in(&[
            ("api/package.json", r#"{"name":"api"}"#),
            ("api/Dockerfile", "FROM node\n"),
        ])
        .unwrap();
        assert_eq!(summary(&b), [s("api", "api", true)]);
        assert_eq!(b.projects[0].detected_by, ["dockerfile", "package.json"]);
        assert_eq!(b.projects[0].manifests, ["api/package.json"]);
    }

    #[test]
    fn files_outside_projects_have_none() {
        let b = detect_in(&[("svc/package.json", "{}"), ("README.md", "")]).unwrap();
        assert!(b.project_for("README.md").is_none());
        assert_eq!(b.projects[0].name, "svc");
    }

    #[test]
    fn broken_manifest_is_a_boundary_with_a_diagnostic() {
        let b = detect_in(&[("svc/package.json", "{ nope")]).unwrap();
        assert_eq!(summary(&b), [s("svc", "svc", false)]);
        assert_eq!(b.diagnostics.len(), 1);
        assert_eq!(b.diagnostics[0].kind, "manifest_parse_error");
        assert_eq!(b.diagnostics[0].path.as_deref(), Some("svc/package.json"));
    }

    #[test]
    fn rb_toml_excludes_and_overrides() {
        let b = detect_in(&[
            (
                "rb.toml",
                "exclude = [\"examples/**\"]\n\n[[service]]\npath = \"./backend/\"\nname = \"api\"\n\n[[service]]\npath = \"svc\"\ndeployable = false\n",
            ),
            ("examples/demo/package.json", r#"{"name":"demo","scripts":{"start":"x"}}"#),
            ("backend/server.js", ""),
            ("svc/package.json", r#"{"name":"svc","scripts":{"start":"x"}}"#),
        ])
        .unwrap();
        assert_eq!(
            summary(&b),
            [s("backend", "api", true), s("svc", "svc", false)]
        );
        assert_eq!(b.projects[0].detected_by, ["rb.toml"]);
    }

    #[test]
    fn rb_toml_errors_fail_loudly() {
        assert!(detect_in(&[("rb.toml", "bogus = 1\n")]).is_err());
        assert!(detect_in(&[("rb.toml", "exclude = [\"!x\"]\n")]).is_err());
        assert!(detect_in(&[("rb.toml", "[[service]]\npath = \"nope\"\n")]).is_err());
    }

    /// Contract M6g S4: `[[hosts]]` entries parse (the indexer reads them; detection
    /// ignores them beyond parsing).
    #[test]
    fn rb_toml_hosts_entries_parse() {
        let b = detect_in(&[
            (
                "rb.toml",
                "[[hosts]]\nservice = \"services/auth\"\nnames = [\"auth\", \"login\"]\n",
            ),
            ("services/auth/package.json", "{}"),
        ])
        .unwrap();
        assert_eq!(b.projects.len(), 1);
    }

    #[test]
    fn innermost_project_uses_whole_directory_segments() {
        let b = detect_in(&[
            ("services/auth/package.json", "{}"),
            ("services/auth2/x.ts", ""),
            ("services/auth/deep/er/y.ts", ""),
        ])
        .unwrap();
        assert!(b.project_for("services/auth2/x.ts").is_none());
        assert_eq!(
            b.project_for("services/auth/deep/er/y.ts").unwrap().dir,
            "services/auth"
        );
        assert_eq!(
            b.project_for("services/auth/package.json").unwrap().dir,
            "services/auth"
        );
    }

    #[test]
    fn byte_order_marks_and_doctypes_are_accepted() {
        let b = detect_in(&[
            ("web/package.json", "\u{feff}{\"name\":\"web\",\"scripts\":{\"start\":\"x\"}}"),
            (
                "svc/pom.xml",
                "\u{feff}<?xml version=\"1.0\"?>\n<!DOCTYPE project>\n<project><artifactId>svc</artifactId></project>",
            ),
        ])
        .unwrap();
        assert!(b.diagnostics.is_empty(), "{:?}", b.diagnostics);
        assert_eq!(summary(&b), [s("svc", "svc", false), s("web", "web", true)]);
    }

    #[test]
    fn go_main_detection() {
        assert!(is_go_main("package main\n"));
        assert!(is_go_main("// +build tools\n\npackage main // x\n"));
        assert!(is_go_main("/*\nlicense\n*/\npackage main\n"));
        assert!(!is_go_main("package mainly\n"));
        assert!(is_go_main("/* Copyright */ package main\n"));
        assert!(is_go_main("/* a */ /* b\n c */ // d\npackage main\n"));
        assert!(!is_go_main("/* package main */ package lib\n"));
        assert!(!is_go_main("// package main\npackage lib\n"));
    }
}
