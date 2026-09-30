//! Indexing pipeline (design §3.1): commit → manifest → structure (CODEOWNERS, boundaries) → graph.
//!
//! Everything here is a pure function of the commit's tree, so the same commit always yields the
//! same graph and diagnostics.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

mod modules;
mod orm;
mod projects;
mod resolve;
mod scip;
mod sql;
mod syntax;

pub use projects::{NoScipCache, ScipCache, ScipCacheEntry, ScipOptions, ScipProject};
pub use syntax::{FactsCache, FactsKey, NoCache};

use rb_git::Repo;
use rb_git::boundaries::{self, Boundaries, Project};
use rb_git::codeowners::{self, Codeowners};
use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, FileEntry, Graph, GraphBuilder, Manifest,
    NewEdge, NodeKey, NodeKind, Provenance, ServiceRole,
};
use serde_json::{Value, json};

/// Identifies the extraction pipeline; bump whenever indexing output changes for the same input.
pub const PIPELINE: &str = "m1b-syntax-1";

/// Name of the repo-root service when no project lives at the root. Files outside every project
/// belong to it (design §4.3). Its key is the root directory `.`, which Git forbids as a path
/// component, so no real directory can collide with it.
pub const UNASSIGNED_NAME: &str = "(unassigned)";

pub struct Indexed {
    pub commit_sha: String,
    pub graph: Graph,
    /// In canonical order.
    pub diagnostics: Vec<Diagnostic>,
    /// Blobs parsed in this run (cache misses); 0 on a fully warm cache.
    pub parsed: usize,
    /// The SCIP jobs of the commit, in job order, with their decoded indexes dropped.
    pub scip: Vec<ScipProject>,
}

/// Indexes `rev` of the repository at `repo_path`, reusing and filling `cache` and, for SCIP
/// indexes, `scip_cache`. Jobs run only when `scip.enabled` and Docker answers; otherwise every
/// job is recorded as `skipped` and the whole graph is heuristic.
pub fn index_commit(
    repo_path: &Path,
    rev: &str,
    cache: &mut dyn FactsCache,
    scip: &ScipOptions,
    scip_cache: &mut dyn ScipCache,
) -> anyhow::Result<Indexed> {
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

    let (plan, skipped) = syntax::parse_plan(&manifest.files);
    diagnostics.extend(skipped);
    let threads = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let (facts, parsed) =
        syntax::load_or_parse(&|blob: &str| repo.blob(blob), &plan, cache, threads)?;
    let facts_by_path: BTreeMap<&str, (rb_syntax::Language, &rb_syntax::FileFacts)> = plan
        .iter()
        .map(|(f, language)| {
            let key = FactsKey {
                blob: f.blob_sha.clone(),
                language: *language,
            };
            (f.path.as_str(), (*language, &facts[&key]))
        })
        .collect();
    diagnostics.extend(syntax::syntax_diagnostics(&facts_by_path));

    let mut texts = |lang: &str| -> anyhow::Result<BTreeMap<&str, String>> {
        let (files, skipped) = sql::text_plan(&manifest.files, lang);
        diagnostics.extend(skipped);
        files
            .into_iter()
            .map(|f| Ok((f.path.as_str(), repo.blob_text(&f.blob_sha)?)))
            .collect()
    };
    let go_modules = manifest
        .files
        .iter()
        .filter(|f| f.path == "go.mod" || f.path.ends_with("/go.mod"))
        .map(|f| {
            let dir = f.path.strip_suffix("go.mod").unwrap_or_default();
            let module = boundaries::go_module_path(&repo.blob_text(&f.blob_sha)?);
            Ok(module.map(|m| (dir.trim_end_matches('/').to_owned(), m)))
        })
        .filter_map(Result::transpose)
        .collect::<anyhow::Result<_>>()?;
    let docker = rb_scip::DockerRunner;
    let mode = if !scip.enabled {
        projects::Mode::Skip("SCIP disabled (--no-scip)".into())
    } else {
        match rb_scip::docker_available() {
            Ok(()) => projects::Mode::Run(&docker),
            Err(reason) => projects::Mode::Skip(reason),
        }
    };
    if let projects::Mode::Run(_) = mode {
        std::fs::create_dir_all(&scip.work_dir)?;
    }
    let scip_projects = projects::run_projects(
        &manifest.files,
        &|blob: &str| repo.blob(blob),
        &mode,
        scip,
        scip_cache,
    )?;
    let content = Content {
        facts: facts_by_path,
        sql: texts("sql")?,
        prisma: texts("prisma")?,
        go_modules,
        scip: scip_projects,
    };

    let (graph, assembly_diagnostics) =
        assemble(&manifest, codeowners.as_ref(), &boundaries, &content)?;
    diagnostics.extend(assembly_diagnostics);
    rb_model::sort_diagnostics(&mut diagnostics);
    let mut scip = content.scip;
    for p in &mut scip {
        p.index = None;
    }
    Ok(Indexed {
        commit_sha: manifest.commit_sha,
        graph,
        diagnostics,
        parsed,
        scip,
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

/// Everything read from a commit besides its structure, by path.
#[derive(Default)]
pub struct Content<'a> {
    /// Parsed source files.
    pub facts: BTreeMap<&'a str, (rb_syntax::Language, &'a rb_syntax::FileFacts)>,
    /// `.sql` files.
    pub sql: BTreeMap<&'a str, String>,
    /// `.prisma` schema files.
    pub prisma: BTreeMap<&'a str, String>,
    /// Directory of each `go.mod` (`""` for the root) → its module path.
    pub go_modules: BTreeMap<String, String>,
    /// The commit's SCIP jobs and their outcomes.
    pub scip: Vec<ScipProject>,
}

/// Builds the snapshot graph from the structural, syntax, SQL and ORM facts; returns the
/// diagnostics found while assembling (unparseable SQL, unresolved ORM entities).
pub fn assemble(
    manifest: &Manifest,
    codeowners: Option<&Codeowners>,
    boundaries: &Boundaries,
    content: &Content<'_>,
) -> anyhow::Result<(Graph, Vec<Diagnostic>)> {
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
    let mut diagnostics = Vec::new();
    // Symbol keys: heuristic per file, replaced by SCIP symbols where a project's index matched
    // (design §5.3). Every later stage names symbols through this one map.
    let mut keys: BTreeMap<&str, Vec<String>> = content
        .facts
        .iter()
        .map(|(path, (_, facts))| (*path, rb_syntax::symbol_keys(path, facts)))
        .collect();
    // A file belongs to the deepest build root containing it; it is exact when that root's
    // index is ok, and heuristic otherwise (contract M3b invariant 1).
    let owner_of =
        |path: &str| projects::owner_root(path, content.scip.iter().map(|p| p.root.as_str()));
    let mut scip_edges = Vec::new();
    let mut exact_files: BTreeSet<&str> = BTreeSet::new();
    for project in &content.scip {
        let files: BTreeMap<&str, &rb_syntax::FileFacts> = content
            .facts
            .iter()
            .filter(|(path, _)| owner_of(path) == Some(project.root.as_str()))
            .map(|(path, (_, facts))| (*path, *facts))
            .collect();
        let coverage = match (&project.status, &project.index) {
            (rb_scip::RunStatus::Ok, Some(index)) => {
                let joined = scip::join(index, &project.root, &files, &keys);
                for (path, replaced) in joined.keys {
                    let file_keys = keys
                        .get_mut(path.as_str())
                        .expect("the join only returns files it was given");
                    for (i, key) in replaced {
                        file_keys[i] = key;
                    }
                }
                scip_edges.extend(joined.edges);
                exact_files.extend(files.keys());
                Some(joined.coverage)
            }
            _ => None,
        };
        diagnostics.push(scip_status(project, coverage.as_ref()));
    }
    // ORM mappings from every file first, so accesses can resolve entities across files.
    for (path, (_, facts)) in &content.facts {
        syntax::add_symbols(&mut b, path, facts, &keys[path])?;
        diagnostics.extend(sql::add_queries(&mut b, path, facts, &keys[path])?);
    }
    for e in scip_edges {
        let src = match e.src {
            scip::Source::Symbol(key) => NodeKey::new(NodeKind::Symbol, key),
            scip::Source::File => NodeKey::new(NodeKind::File, e.at.path.clone()),
        };
        let kind = match e.kind {
            scip::EdgeKind::Calls => EdgeKind::Calls,
            scip::EdgeKind::References => EdgeKind::References,
            scip::EdgeKind::Implements => EdgeKind::Implements,
        };
        b.add_edge(
            NewEdge::new(
                src,
                kind,
                NodeKey::new(NodeKind::Symbol, e.dst),
                Provenance::Scip,
                Certainty::Exact,
            )
            .evidence(e.at),
        )?;
    }
    let files: Vec<(&str, rb_syntax::Language, &rb_syntax::FileFacts)> = content
        .facts
        .iter()
        .map(|(path, (language, facts))| (*path, *language, *facts))
        .collect();
    let prisma: Vec<(&str, &str)> = content
        .prisma
        .iter()
        .map(|(path, text)| (*path, text.as_str()))
        .collect();
    diagnostics.extend(resolve::add_edges(
        &mut b,
        &content.facts,
        &content.go_modules,
        &keys,
        &exact_files,
    )?);
    let (orm_index, orm_diagnostics) = orm::add_mappings(&mut b, &files, &prisma, &keys)?;
    diagnostics.extend(orm_diagnostics);
    for (path, (language, facts)) in &content.facts {
        diagnostics.extend(orm::add_accesses(
            &mut b,
            path,
            *language,
            facts,
            &keys[path],
            &orm_index,
        )?);
    }
    for (path, text) in &content.sql {
        diagnostics.extend(sql::add_migrations(&mut b, path, text)?);
    }
    Ok((b.finish()?, diagnostics))
}

/// One `scip_status` diagnostic per job (contract M3b S9). Its detail is a function of the
/// commit and the cache, never of this run: whether the index was served from the cache is
/// reported by `rb index`, not recorded here, so a warm re-index dumps byte-identically.
fn scip_status(p: &ScipProject, coverage: Option<&scip::Coverage>) -> Diagnostic {
    let mut detail = json!({
        "indexer": p.indexer,
        "status": p.status.as_str(),
        "elapsed_ms": p.elapsed_ms,
        "unpinned": p.unpinned,
    });
    match p.status.reason() {
        Some(reason) => detail["reason"] = reason.into(),
        None => detail["image"] = p.image.as_str().into(),
    }
    if let Some(c) = coverage {
        detail["coverage"] = json!({
            "definitions_matched": c.definitions_matched,
            "definitions_matched_by_name": c.definitions_matched_by_name,
            "definitions_total": c.definitions_total,
            "references_matched": c.references_matched,
            "references_external": c.references_external,
            "references_local": c.references_local,
            "references_unanchored": c.references_unanchored,
            "references_unmatched": c.references_unmatched,
        });
    }
    if p.status != rb_scip::RunStatus::Ok && !p.stderr_tail.is_empty() {
        detail["stderr_tail"] = p.stderr_tail.as_str().into();
    }
    Diagnostic {
        kind: "scip_status".into(),
        path: Some(if p.root.is_empty() {
            boundaries::ROOT_DIR.to_owned()
        } else {
            p.root.clone()
        }),
        span: None,
        detail,
    }
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
        let g = assemble(&manifest, Some(&co), &boundaries, &Content::default())
            .unwrap()
            .0;

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
        let g = assemble(&manifest, None, &boundaries, &Content::default())
            .unwrap()
            .0;
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
        let g = assemble(&manifest, None, &boundaries, &Content::default())
            .unwrap()
            .0;
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
