//! Indexing pipeline (design §3.1): commit → manifest → structure (CODEOWNERS, boundaries) → graph.
//!
//! Everything here is a pure function of the commit's tree, so the same commit always yields the
//! same graph and diagnostics.

use std::collections::BTreeMap;
use std::path::Path;

use rb_git::Repo;
use rb_git::boundaries::{self, Boundaries, Project};
use rb_git::codeowners::{self, Codeowners};
use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, FileEntry, Graph, GraphBuilder, Manifest,
    NewEdge, NodeKey, NodeKind, Provenance, ServiceRole,
};
use serde_json::{Value, json};

/// Identifies the extraction pipeline; bump whenever indexing output changes for the same input.
pub const PIPELINE: &str = "m1a-structure-1";

/// Name of the repo-root service when no project lives at the root. Files outside every project
/// belong to it (design §4.3). Its key is the root directory `.`, which Git forbids as a path
/// component, so no real directory can collide with it.
pub const UNASSIGNED_NAME: &str = "(unassigned)";

pub struct Indexed {
    pub commit_sha: String,
    pub graph: Graph,
    /// In canonical order.
    pub diagnostics: Vec<Diagnostic>,
}

/// Indexes `rev` of the repository at `repo_path`.
pub fn index_commit(repo_path: &Path, rev: &str) -> anyhow::Result<Indexed> {
    let repo = Repo::open(repo_path)?;
    let manifest = repo.manifest(rev)?;
    let read = |f: &FileEntry| repo.blob_text(&f.blob_sha);

    let mut diagnostics: Vec<Diagnostic> = manifest
        .skipped
        .iter()
        .map(|s| Diagnostic {
            kind: format!("skipped_{}", s.reason),
            path: Some(s.path.clone()),
            span: None,
            detail: json!({}),
        })
        .collect();

    let codeowners = load_codeowners(&manifest, &read, &mut diagnostics)?;
    let boundaries = boundaries::detect(&manifest.files, &read)?;
    diagnostics.extend(boundaries.diagnostics.iter().cloned());

    let graph = assemble(&manifest, codeowners.as_ref(), &boundaries)?;
    rb_model::sort_diagnostics(&mut diagnostics);
    Ok(Indexed {
        commit_sha: manifest.commit_sha,
        graph,
        diagnostics,
    })
}

/// Parses the CODEOWNERS file GitHub would use; reports invalid lines and ignored extra files.
fn load_codeowners(
    manifest: &Manifest,
    read: &dyn Fn(&FileEntry) -> anyhow::Result<String>,
    diagnostics: &mut Vec<Diagnostic>,
) -> anyhow::Result<Option<Codeowners>> {
    let present: Vec<&FileEntry> = codeowners::LOCATIONS
        .iter()
        .filter_map(|loc| manifest.files.iter().find(|f| f.path == *loc))
        .collect();
    let Some((used, ignored)) = present.split_first() else {
        return Ok(None);
    };
    for f in ignored {
        diagnostics.push(Diagnostic {
            kind: "codeowners_ignored".into(),
            path: Some(f.path.clone()),
            span: None,
            detail: json!({ "used": used.path }),
        });
    }
    let co = Codeowners::parse(&used.path, &read(used)?);
    for e in &co.errors {
        diagnostics.push(Diagnostic {
            kind: "codeowners_invalid_line".into(),
            path: Some(co.path.clone()),
            span: Some(line_span(e.line, e.line_len)),
            detail: json!({ "error": e.message }),
        });
    }
    Ok(Some(co))
}

/// Builds the snapshot graph from the structural facts.
pub fn assemble(
    manifest: &Manifest,
    codeowners: Option<&Codeowners>,
    boundaries: &Boundaries,
) -> anyhow::Result<Graph> {
    let mut b = GraphBuilder::new();

    // Services: one per project, plus the root as `unassigned` if files lie outside all projects.
    for p in boundaries.projects() {
        b.add_node(service_key(&p.dir), &p.name, service_attrs(p))?;
    }
    let unassigned = service_key(boundaries::ROOT_DIR);

    let mut files_by_project: BTreeMap<&str, Vec<&FileEntry>> = BTreeMap::new();
    for f in &manifest.files {
        let file = NodeKey::new(NodeKind::File, f.path.clone());
        let name = f.path.rsplit('/').next().unwrap_or(&f.path);
        b.add_node(file.clone(), name, file_attrs(f))?;

        let service = match boundaries.project_for(&f.path) {
            Some(p) => {
                files_by_project.entry(p.dir.as_str()).or_default().push(f);
                service_key(&p.dir)
            }
            None => {
                if !b.has_node(&unassigned) {
                    b.add_node(
                        unassigned.clone(),
                        UNASSIGNED_NAME,
                        object(json!({"role": ServiceRole::Unassigned.as_str()})),
                    )?;
                }
                unassigned.clone()
            }
        };
        b.add_edge(NewEdge::new(
            file.clone(),
            EdgeKind::InService,
            service,
            Provenance::Manifest,
            Certainty::Exact,
        ))?;

        if let Some(co) = codeowners {
            add_owners(&mut b, co, &f.path, file, Certainty::Exact, None)?;
        }
    }

    // A service's owners are those of its anchor file: the first manifest, else its first file.
    if let Some(co) = codeowners {
        for p in boundaries.projects() {
            let anchor = p.manifests.first().map(String::as_str).or_else(|| {
                files_by_project
                    .get(p.dir.as_str())
                    .and_then(|fs| fs.first())
                    .map(|f| f.path.as_str())
            });
            if let Some(anchor) = anchor {
                add_owners(
                    &mut b,
                    co,
                    anchor,
                    service_key(&p.dir),
                    Certainty::Synthetic,
                    Some(anchor),
                )?;
            }
        }
    }
    b.finish()
}

/// Adds `OWNED_BY` edges from `src` for the CODEOWNERS rule deciding `path`, with the rule's line
/// as evidence.
fn add_owners(
    b: &mut GraphBuilder,
    co: &Codeowners,
    path: &str,
    src: NodeKey,
    certainty: Certainty,
    via: Option<&str>,
) -> anyhow::Result<()> {
    let Some(rule) = co.rule_for(path) else {
        return Ok(());
    };
    for owner in &rule.owners {
        let key = NodeKey::new(NodeKind::Owner, owner.clone());
        b.add_node(key.clone(), owner, Attrs::new())?;
        let mut attrs = object(json!({"pattern": rule.pattern.as_str()}));
        if let Some(via) = via {
            attrs.insert("via".into(), via.into());
        }
        b.add_edge(
            NewEdge::new(
                src.clone(),
                EdgeKind::OwnedBy,
                key,
                Provenance::Codeowners,
                certainty,
            )
            .attrs(attrs)
            .evidence(Evidence {
                path: co.path.clone(),
                span: line_span(rule.line, rule.line_len),
            }),
        )?;
    }
    Ok(())
}

fn service_key(dir: &str) -> NodeKey {
    NodeKey::new(NodeKind::Service, dir)
}

fn service_attrs(p: &Project) -> Attrs {
    let role = if p.deployable {
        ServiceRole::Service
    } else {
        ServiceRole::Library
    };
    object(json!({
        "role": role.as_str(),
        "detected_by": p.detected_by,
        "manifests": p.manifests,
    }))
}

/// File attributes; false flags are omitted to keep dumps compact.
fn file_attrs(f: &FileEntry) -> Attrs {
    let mut a = object(json!({"blob": f.blob_sha, "size": f.size}));
    if let Some(lang) = f.lang {
        a.insert("lang".into(), lang.into());
    }
    for (flag, set) in [
        ("binary", f.binary),
        ("executable", f.executable),
        ("generated", f.generated),
    ] {
        if set {
            a.insert(flag.into(), true.into());
        }
    }
    a
}

fn line_span(line: usize, len: usize) -> rb_model::Span {
    let line = i32::try_from(line).unwrap_or(i32::MAX);
    [line, 0, line, i32::try_from(len).unwrap_or(i32::MAX)]
}

fn object(v: Value) -> Attrs {
    match v {
        Value::Object(m) => m,
        _ => unreachable!("object() is only called with json!({{..}}) literals"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> FileEntry {
        FileEntry {
            path: path.into(),
            blob_sha: "b".repeat(40),
            size: 1,
            executable: false,
            lang: rb_git::language_for_path(path),
            binary: false,
            generated: false,
        }
    }

    fn project(dir: &str, manifest: &str) -> Project {
        Project {
            dir: dir.into(),
            name: dir.into(),
            detected_by: vec!["package.json".into()],
            manifests: vec![manifest.into()],
            deployable: true,
        }
    }

    fn edges_of(g: &Graph, kind: EdgeKind) -> Vec<(String, String)> {
        let key = |id: i64| g.nodes().iter().find(|n| n.id == id).unwrap().key.clone();
        g.edges()
            .iter()
            .filter(|e| e.kind == kind)
            .map(|e| (key(e.src), key(e.dst)))
            .collect()
    }

    #[test]
    fn assembles_services_owners_and_evidence() {
        let manifest = Manifest {
            commit_sha: "c".repeat(40),
            files: vec![
                entry("CODEOWNERS"),
                entry("README.md"),
                entry("api/package.json"),
                entry("api/src/app.ts"),
            ],
            skipped: vec![],
        };
        let co = Codeowners::parse(
            "CODEOWNERS",
            "* @acme/platform\n/api/src/ @alice docs@acme.io\n",
        );
        let boundaries = Boundaries::new(vec![project("api", "api/package.json")], vec![]);
        let g = assemble(&manifest, Some(&co), &boundaries).unwrap();

        assert_eq!(
            edges_of(&g, EdgeKind::InService),
            [
                ("CODEOWNERS".into(), ".".into()),
                ("README.md".into(), ".".into()),
                ("api/package.json".into(), "api".into()),
                ("api/src/app.ts".into(), "api".into()),
            ]
        );
        let owned: Vec<(String, String)> = edges_of(&g, EdgeKind::OwnedBy);
        assert!(owned.contains(&("api/src/app.ts".into(), "@alice".into())));
        assert!(owned.contains(&("api/src/app.ts".into(), "docs@acme.io".into())));
        assert!(!owned.contains(&("api/src/app.ts".into(), "@acme/platform".into())));
        // The service is owned via its manifest, which only the `*` rule covers.
        assert!(owned.contains(&("api".into(), "@acme/platform".into())));

        let service = |key: &str| {
            g.nodes()
                .iter()
                .find(|n| n.kind == NodeKind::Service && n.key == key)
                .unwrap()
        };
        assert_eq!(service("api").attrs["role"], "service");
        assert_eq!(service(".").attrs["role"], "unassigned");
        // The key is the directory; nothing derivable from it is stored again.
        for n in g.nodes() {
            assert!(!n.attrs.contains_key("path"), "{} stores path", n.key);
            assert!(!n.attrs.contains_key("type"), "{} stores type", n.key);
            assert!(
                !n.attrs.contains_key("deployable"),
                "{} stores deployable",
                n.key
            );
        }

        let alice = g.nodes().iter().find(|n| n.key == "@alice").unwrap();
        let e = g.edges().iter().find(|e| e.dst == alice.id).unwrap();
        assert_eq!(e.evidence[0].path, "CODEOWNERS");
        assert_eq!(e.evidence[0].span, [1, 0, 1, 29]);
        assert_eq!(e.attrs["pattern"], "/api/src/");
    }

    #[test]
    fn no_synthetic_service_when_every_file_is_in_a_project() {
        let manifest = Manifest {
            commit_sha: "c".repeat(40),
            files: vec![entry("package.json"), entry("index.js")],
            skipped: vec![],
        };
        let mut library = project(".", "package.json");
        library.deployable = false;
        let boundaries = Boundaries::new(vec![library], vec![]);
        let g = assemble(&manifest, None, &boundaries).unwrap();
        assert!(
            g.nodes()
                .iter()
                .all(|n| n.attrs.get("role") != Some(&"unassigned".into()))
        );
        let root = g.nodes().iter().find(|n| n.key == ".").unwrap();
        assert_eq!(root.attrs["role"], "library");
        assert!(edges_of(&g, EdgeKind::OwnedBy).is_empty());
    }

    #[test]
    fn a_directory_named_like_a_label_cannot_capture_unassigned_files() {
        let manifest = Manifest {
            commit_sha: "c".repeat(40),
            files: vec![entry("(root)/package.json"), entry("README.md")],
            skipped: vec![],
        };
        let boundaries = Boundaries::new(vec![project("(root)", "(root)/package.json")], vec![]);
        let g = assemble(&manifest, None, &boundaries).unwrap();
        assert_eq!(
            edges_of(&g, EdgeKind::InService),
            [
                ("(root)/package.json".into(), "(root)".into()),
                ("README.md".into(), ".".into()),
            ]
        );
        let root = g.nodes().iter().find(|n| n.key == ".").unwrap();
        assert_eq!(root.attrs["role"], "unassigned");
        assert_eq!(root.name, "(unassigned)");
    }
}
