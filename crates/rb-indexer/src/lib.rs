//! Indexing pipeline (design §3.1): commit → manifest → structure (CODEOWNERS, boundaries) → graph.
//!
//! Everything here is a pure function of the commit's tree, so the same commit always yields the
//! same graph and diagnostics.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

mod modules;
mod orm;
mod packages;
mod projects;
mod resolve;
mod routes;
mod scip;
mod sql;
mod stitch;
mod syntax;

pub use projects::{
    NoScipCache, ScipCache, ScipCacheEntry, ScipMode, ScipOptions, ScipProject, WriteOnlyScipCache,
};
pub use syntax::{FactsCache, FactsKey, NoCache};

use rb_git::Repo;
use rb_git::boundaries::{self, Boundaries, Project};
use rb_git::codeowners::{self, Codeowners};
use rb_model::{
    Attrs, Certainty, Diagnostic, EdgeKind, Evidence, FileEntry, Graph, GraphBuilder, Manifest,
    NewEdge, NodeKey, NodeKind, Provenance, ServiceRole,
};
use serde_json::{Value, json};

/// Identifies the extraction pipeline; bump whenever indexing output changes for the same input,
/// so a snapshot an older binary built is not "up to date" for this one (it had stayed
/// `m1b-syntax-1` through M2b–M3a; contract M3b amendment 42). That includes every change to
/// the SCIP decoder or join (`rb_scip::decode`, `scip::join`): evaluator passes 4 and 5 changed
/// the join's output without a bump (contract M3b amendment 63); `m3b-scip-3` adds the
/// per-document agreement check (amendment 64), `m3b-scip-4` its `encoding_ambiguous` and
/// `structure_disagreement` reasons (amendments 68 and 69), `m3b-scip-5` the two-way definition
/// agreement and the encoding tie (amendments 72 and 73), `m3b-scip-6` kind-aware pairing, a
/// type anchored by its type symbol, the syntax-error rule for TypeScript and compact
/// constructors as definitions (amendment 75), `m3b-scip-7` the `structural_escape` reason and
/// supertypes confirmed by the index (amendments 77 and 78), `m3b-scip-8` an escaped backslash
/// as a structural escape (amendment 80).
/// `m4a-http-1` adds endpoints, `EXPOSES`, `HANDLED_BY` and route diagnostics (contract M4a),
/// `m4a-http-2` `CONSUMES`, the `http_*` site diagnostics and `http_coverage`;
/// `m4a-http-3` suffix matches ranked by the literals of the matched tail (M4a amendment 9),
/// `m4a-http-4` the router's identity in the key of an endpoint whose prefix is unknown (M4a
/// amendment 10, fork F3), `m4a-http-5` evaluator pass 1's fixes (M4a amendment 11: hosts kept
/// past a port hole, suffix evidence and own-service ranking, opaque tails, `config_unread`,
/// `ServeMux` hosts, unknown bases of server-side Java clients), `m4a-http-6` evaluator pass
/// 2's (M4a amendment 12: bases stated at the call or on the instance, partial hosts, a hole
/// after a literal port, more Spring configuration counted, `node-fetch`), `m4a-http-7`
/// evaluator pass 3's (M4a amendment 13: verified instance bases, paths that start with a hole
/// matched by suffix, the config client turned off only by a base document, `cross-fetch` and
/// inline client instances), `m4a-http-8` evaluator pass 4's (M4a amendment 14: a conservative
/// token scan verifies instance bases, a `uriBuilderFactory` overrides `baseUrl`, any
/// `spring.config.activate.*` document is conditional), `m4a-http-9` evaluator pass 5's (M4a
/// amendment 15: every carrier gated, whatever the URL's source; global hooks cap bare module
/// clients), `m4a-http-10` evaluator pass 6's (M4a amendment 16: carriers plain only by
/// allow-listed token roles; a hook caps every site of its client kind, Feign included),
/// `m4a-http-11` evaluator pass 7's (M4a amendment 17: files without token facts scanned as
/// text, string literals naming `fetch` or axios, discovery configuration caps host-by-name
/// `synthetic`).
/// `m4b-di-1` adds manifest dependencies: `Package` versions from lockfiles, `go.mod` and poms,
/// and Service → Service `DEPENDS_ON` `via=package` (contract M4b S15, S16).
pub const PIPELINE: &str = "m4b-di-1";

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
    /// Files without facts scanned as text (contract M4a amendment 17).
    pub scanned: usize,
    /// The SCIP jobs of the commit, in job order, with their decoded indexes dropped.
    pub scip: Vec<ScipProject>,
}

/// Indexes `rev` of the repository at `repo_path`, reusing and filling `cache` and, for SCIP
/// indexes, `scip_cache`. Jobs run only when `scip.mode` is `On` (the caller has checked that
/// Docker answers); otherwise every job is recorded as `skipped` and the whole graph is
/// heuristic.
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
    // Files with no facts are scanned as text for hooks and discovery (amendment 17).
    let (unparsed, scanned) =
        syntax::scan_unparsed(&manifest.files, &plan, &|blob: &str| repo.blob(blob))?;
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
    let docker = rb_scip::DockerRunner::default();
    let mode = match scip.mode.skip_reason() {
        None => projects::Mode::Run(&docker),
        Some(reason) => projects::Mode::Skip(reason.into()),
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
    let config = manifest
        .files
        .iter()
        .filter(|f| is_config_input(&f.path))
        .map(|f| Ok((f.path.as_str(), repo.blob_text(&f.blob_sha)?)))
        .collect::<anyhow::Result<_>>()?;
    let manifests = manifest
        .files
        .iter()
        .filter(|f| packages::is_input(&f.path) && !f.binary)
        .map(|f| Ok((f.path.as_str(), repo.blob_text(&f.blob_sha)?)))
        .collect::<anyhow::Result<_>>()?;
    let content = Content {
        facts: facts_by_path,
        sql: texts("sql")?,
        prisma: texts("prisma")?,
        go_modules,
        scip: scip_projects,
        config,
        unparsed,
        manifests,
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
        scanned,
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
    /// Spring configuration read by path ([`is_config_input`]; contract M4a D5, evaluator
    /// pass 1, P7).
    pub config: BTreeMap<&'a str, String>,
    /// What the raw-text scan of the files without facts found (contract M4a amendment 17).
    pub unparsed: rb_frameworks::clients::RawScan,
    /// `package.json`, npm lockfiles, `go.mod` and `pom.xml` texts (contract M4b S15).
    pub manifests: BTreeMap<&'a str, String>,
}

/// What a configuration input is to the endpoint and stitching stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigFile {
    /// `src/main/resources/application.properties`: read (D5).
    Read,
    /// A profile file (`application-<p>.*`), not read.
    Profile,
    /// `application.yml`/`.yaml`, not read (no YAML parser).
    Yaml,
    /// `bootstrap*.properties|yml|yaml`, not read.
    Bootstrap,
    /// `config/application.properties` (Spring Boot's `classpath:/config/`, which overrides
    /// the read file), not read (amendment 12).
    ConfigDir,
    /// A Maven or Gradle build file, read for a Spring Cloud Config client dependency.
    Build,
}

fn config_file(path: &str) -> Option<ConfigFile> {
    let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
    if matches!(name, "pom.xml" | "build.gradle" | "build.gradle.kts") {
        return Some(ConfigFile::Build);
    }
    // `src/main/resources/` and Spring Boot's `classpath:/config/` location (amendment 12).
    let resources = |d: &str| d == "src/main/resources" || d.ends_with("/src/main/resources");
    let config_dir = dir
        .strip_suffix("/config")
        .is_some_and(|d| resources(d) || d == "src/main/resources");
    if !resources(dir) && !config_dir {
        return None;
    }
    let (stem, ext) = name.rsplit_once('.')?;
    if !matches!(ext, "properties" | "yml" | "yaml") {
        return None;
    }
    let yaml = ext != "properties";
    match stem {
        "application" if yaml => Some(ConfigFile::Yaml),
        "application" if config_dir => Some(ConfigFile::ConfigDir),
        "application" => Some(ConfigFile::Read),
        s if s.starts_with("application-") => Some(ConfigFile::Profile),
        s if s == "bootstrap" || s.starts_with("bootstrap-") => Some(ConfigFile::Bootstrap),
        _ => None,
    }
}

/// A file the indexer reads as configuration: Spring's `application*`/`bootstrap*` files under
/// `src/main/resources/` and its `config/` directory, and build files (contract M4a D5; evaluator pass 1, P7);
/// and every other settings file, scanned only for Feign client keys (amendment 16).
fn is_config_input(path: &str) -> bool {
    config_file(path).is_some() || is_settings_file(path)
}

/// Any `.properties`/`.yml`/`.yaml` file: a config server's shared files or a
/// `@PropertySource` may configure Feign clients (contract M4a amendment 16).
fn is_settings_file(path: &str) -> bool {
    path.rsplit_once('.')
        .is_some_and(|(_, ext)| matches!(ext, "properties" | "yml" | "yaml"))
}

/// Whether any settings file sets a Feign client key
/// ([`rb_frameworks::clients::feign_config_key`]).
fn feign_configured(config: &BTreeMap<&str, String>) -> bool {
    config
        .iter()
        .filter(|(p, _)| is_settings_file(p))
        .any(|(path, text)| {
            let entries = if path.ends_with(".properties") {
                rb_frameworks::fold::parse_properties(text)
            } else {
                rb_frameworks::fold::yaml_entries(text)
            };
            entries
                .keys()
                .any(|k| rb_frameworks::clients::feign_config_key(k))
        })
}

/// Spring configuration per service.
#[derive(Debug, Default)]
struct SpringConfig {
    /// Service key → its `application.properties` (a later file's keys win).
    properties: BTreeMap<String, BTreeMap<String, String>>,
    /// Service key → what configuration M4a does not read may set.
    overrides: BTreeMap<String, stitch::Overrides>,
    /// Services with a `config_unread` diagnostic.
    unread: BTreeSet<String>,
}

/// Build dependencies that make a service a Spring Cloud Config client.
const CONFIG_CLIENT_DEPENDENCIES: &[&str] =
    &["spring-cloud-starter-config", "spring-cloud-config-client"];
/// A key that makes a service a Spring Cloud Config client: `spring.config.import`, or one under
/// `spring.cloud.config.` other than a config server's own `spring.cloud.config.server.*`.
fn is_config_client_key(key: &str) -> bool {
    key.starts_with("spring.config.import")
        || (key.starts_with("spring.cloud.config.")
            && key != "spring.cloud.config.server"
            && !key.starts_with("spring.cloud.config.server."))
}

/// `false` in a base document turns the config client off, whatever else configures it
/// (amendments 12 and 13).
const CONFIG_CLIENT_ENABLED: &str = "spring.cloud.config.enabled";

/// One configuration file as read: path, text, kind, service, and its keys with their values.
type ConfigRead<'a> = (
    &'a str,
    &'a str,
    ConfigFile,
    String,
    BTreeMap<String, String>,
);

/// Reads the services' Spring configuration (evaluator pass 1, P7): `application.properties`
/// is read; profile files, YAML, `bootstrap*` files and a Spring Cloud Config client (by build
/// dependency, or by a `spring.config.import`/`spring.cloud.config` key) are not, so each such
/// service gets one `config_unread` diagnostic (lesson 2026-10-01: a suppressed source is
/// counted) and its overrides: the keys those files set, or every key for a config client. A
/// YAML file mentioning `context-path` keeps its `spring_config_unread` (S4).
fn spring_config(
    config: &BTreeMap<&str, String>,
    service_of: &dyn Fn(&str) -> String,
) -> (SpringConfig, Vec<Diagnostic>) {
    let mut out = SpringConfig::default();
    let mut diagnostics = Vec::new();
    // Each file's kind, service and entries; then the services that turn the client off.
    let mut read: Vec<ConfigRead<'_>> = Vec::new();
    for (path, text) in config {
        let Some(kind) = config_file(path) else {
            continue;
        };
        let entries = match kind {
            ConfigFile::Build => BTreeMap::new(),
            _ if path.ends_with(".properties") => rb_frameworks::fold::parse_properties(text),
            _ => rb_frameworks::fold::yaml_entries(text),
        };
        read.push((path, text, kind, service_of(path), entries));
    }
    // `false` turns the client off only in a base (unconditional) document of
    // `application.properties`/`application.yml`, and only when no file or document sets it
    // back to `true`: a profile's or a cloud platform's switch applies there alone (amendments
    // 13 and 14; evaluator passes 3 and 4, R3 and T5).
    let (mut off, mut on): (BTreeSet<&str>, BTreeSet<&str>) = Default::default();
    for (path, text, kind, service, _) in &read {
        if *kind == ConfigFile::Build {
            continue;
        }
        let documents = if path.ends_with(".properties") {
            rb_frameworks::fold::properties_documents(text)
        } else {
            rb_frameworks::fold::yaml_documents(text)
        };
        let base_file = matches!(
            kind,
            ConfigFile::Read | ConfigFile::ConfigDir | ConfigFile::Yaml
        );
        for doc in &documents {
            match doc.get(CONFIG_CLIENT_ENABLED).map(|v| v.trim()) {
                Some(v) if v.eq_ignore_ascii_case("true") => {
                    on.insert(service);
                }
                Some(v)
                    if v.eq_ignore_ascii_case("false")
                        && base_file
                        && !rb_frameworks::fold::conditional_document(doc) =>
                {
                    off.insert(service);
                }
                _ => {}
            }
        }
    }
    let disabled: BTreeSet<&str> = off.difference(&on).copied().collect();
    // Service → (reasons, files).
    let mut unread: BTreeMap<String, (BTreeSet<&str>, BTreeSet<&str>)> = BTreeMap::new();
    for (path, text, kind, service, entries) in &read {
        let (path, text, kind, service) = (*path, *text, *kind, service.clone());
        let keys: BTreeSet<String> = entries.keys().cloned().collect();
        if kind == ConfigFile::Read {
            out.properties
                .entry(service.clone())
                .or_default()
                .extend(entries.clone());
        }
        if matches!(kind, ConfigFile::Yaml | ConfigFile::Profile)
            && !path.ends_with(".properties")
            && text.contains("context-path")
        {
            diagnostics.push(Diagnostic {
                kind: "spring_config_unread".into(),
                path: Some((*path).to_owned()),
                span: None,
                detail: json!({ "key": "server.servlet.context-path" }),
            });
        }
        let client = !disabled.contains(service.as_str())
            && match kind {
                ConfigFile::Build => CONFIG_CLIENT_DEPENDENCIES.iter().any(|d| text.contains(d)),
                _ => keys.iter().any(|k| is_config_client_key(k)),
            };
        let reason = match kind {
            ConfigFile::Profile => Some("profile"),
            ConfigFile::Yaml => Some("yaml"),
            ConfigFile::Bootstrap => Some("bootstrap"),
            ConfigFile::ConfigDir => Some("config_dir"),
            ConfigFile::Read | ConfigFile::Build => None,
        };
        if reason.is_none() && !client {
            continue;
        }
        let (reasons, files) = unread.entry(service.clone()).or_default();
        reasons.extend(reason);
        if client {
            reasons.insert("config_client");
        }
        files.insert(path);
        let overrides = out.overrides.entry(service).or_default();
        overrides.all |= client;
        if reason.is_some() {
            overrides.keys.extend(keys);
        }
    }
    for (service, (reasons, files)) in unread {
        diagnostics.push(Diagnostic {
            kind: "config_unread".into(),
            path: files.first().map(|p| (*p).to_owned()),
            span: None,
            detail: json!({ "service": service, "reasons": reasons, "files": files }),
        });
        out.unread.insert(service);
    }
    (out, diagnostics)
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
    // index is ok and has a document for it, and heuristic otherwise (contract M3b invariant 1,
    // amendment 15).
    let owner_of =
        |path: &str| projects::owner_root(path, content.scip.iter().map(|p| p.root.as_str()));
    let mut scip_edges = Vec::new();
    let mut exact_files: BTreeSet<&str> = BTreeSet::new();
    // Route facts per file (contract M4a), so an exact index can resolve handler names.
    let file_routes: BTreeMap<&str, rb_frameworks::routes::FileRoutes> = content
        .facts
        .iter()
        .map(|(path, (language, facts))| (*path, rb_frameworks::routes::routes(*language, facts)))
        .collect();
    let mut exact_handlers: BTreeMap<(&str, rb_model::Span), String> = BTreeMap::new();
    // `Service -DEPENDS_ON-> Package` (design §5.5): per service and package, the first site of
    // the service's files that names the package is the one evidence.
    let mut packages: BTreeMap<(NodeKey, String), (rb_scip::Package, Evidence)> = BTreeMap::new();
    for project in &content.scip {
        let files: BTreeMap<&str, (rb_syntax::Language, &rb_syntax::FileFacts)> = content
            .facts
            .iter()
            .filter(|(path, _)| owner_of(path) == Some(project.root.as_str()))
            .map(|(path, (language, facts))| (*path, (*language, *facts)))
            .collect();
        let coverage = match (&project.status, &project.index) {
            (rb_scip::RunStatus::Ok, Some(index)) => {
                let joined = scip::join(
                    index,
                    &project.root,
                    &files,
                    &keys,
                    scip::indexed_languages(project.indexer),
                );
                for (path, routes) in &file_routes {
                    for r in &routes.routes {
                        if let rb_frameworks::routes::Handler::Name { text, span } = &r.handler {
                            let (token, at) = routes::handler_token(text, *span);
                            if let Some(symbol) =
                                scip::definition_at(index, &project.root, &joined, path, at, token)
                            {
                                exact_handlers.insert((path, at), symbol);
                            }
                        }
                    }
                }
                for (path, replaced) in joined.keys {
                    let file_keys = keys
                        .get_mut(path.as_str())
                        .expect("the join only returns files it was given");
                    for (i, key) in replaced {
                        file_keys[i] = key;
                    }
                }
                scip_edges.extend(joined.edges);
                exact_files.extend(
                    joined.documented.iter().filter_map(|path| {
                        files.get_key_value(path.as_str()).map(|(path, _)| *path)
                    }),
                );
                for u in joined.packages {
                    let service = boundaries
                        .project_for(&u.at.path)
                        .map_or_else(|| unassigned.clone(), |p| service_key(&p.dir));
                    packages
                        .entry((service, u.package.key()))
                        .and_modify(|(_, at)| {
                            if u.at < *at {
                                *at = u.at.clone();
                            }
                        })
                        .or_insert((u.package, u.at));
                }
                Some(joined.coverage)
            }
            _ => None,
        };
        diagnostics.push(scip_status(project, coverage.as_ref()));
    }
    for ((service, key), (package, at)) in packages {
        let node = NodeKey::new(NodeKind::Package, key);
        b.add_node(
            node.clone(),
            &package.name,
            object(json!({
                "manager": package.manager,
                "name": package.name,
                "version": package.version,
            })),
        )?;
        b.add_edge(
            NewEdge::new(
                service,
                EdgeKind::DependsOn,
                node,
                Provenance::Scip,
                Certainty::Exact,
            )
            .evidence(at),
        )?;
    }
    // Manifest dependencies (contract M4b S15, S16), next to the SCIP ones.
    let paths: BTreeSet<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
    let (manifest_edges, manifest_diagnostics) = packages::dependencies(&packages::Inputs {
        projects: boundaries.projects(),
        texts: &content.manifests,
        paths: &paths,
    });
    packages::add(&mut b, &manifest_edges)?;
    diagnostics.extend(manifest_diagnostics);
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
    let index = resolve::Index::new(&content.facts, &content.go_modules, &keys);
    diagnostics.extend(resolve::add_edges(&mut b, &index, &exact_files)?);
    // Endpoints (contract M4a S4–S8): Spring's context path from each service's
    // `application.properties`; a YAML file that sets one is counted as unread.
    let service_of = |path: &str| {
        boundaries
            .project_for(path)
            .map_or_else(|| boundaries::ROOT_DIR.to_owned(), |p| p.dir.clone())
    };
    let (spring, config_diagnostics) = spring_config(&content.config, &service_of);
    diagnostics.extend(config_diagnostics);
    let context_paths: BTreeMap<String, String> = spring
        .properties
        .iter()
        .filter_map(|(service, p)| {
            let ctx = p.get("server.servlet.context-path")?.trim();
            (!ctx.is_empty()).then(|| (service.clone(), ctx.to_owned()))
        })
        .collect();
    let (route_diagnostics, endpoints) = routes::add_endpoints(
        &mut b,
        &routes::Inputs {
            files: &content.facts,
            routes: &file_routes,
            keys: &keys,
            index: &index,
            exact_handlers: &exact_handlers,
            service_of: &service_of,
            context_paths: &context_paths,
            config_unread: &spring.unread,
        },
    )?;
    diagnostics.extend(route_diagnostics);
    // Client calls stitched to other services' endpoints (contract M4a S12–S14).
    let sites = stitch::sites(&stitch::Inputs {
        files: &content.facts,
        index: &index,
        keys: &keys,
        service_of: &service_of,
        properties: &spring.properties,
        overrides: &spring.overrides,
        feign_configured: feign_configured(&content.config),
        unparsed: &content.unparsed,
    });
    let services: Vec<stitch::ServiceRef> = boundaries
        .projects()
        .iter()
        .map(|p| stitch::ServiceRef {
            key: p.dir.clone(),
            name: p.name.clone(),
        })
        .collect();
    diagnostics.extend(stitch::add_consumes(&mut b, &sites, &endpoints, &services)?);
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
/// commit and the cache, never of this run or this host: whether the index was served from the
/// cache and how long the container ran are reported by `rb index`, not recorded here (design
/// §3.3 rule 4: wall-clock time never enters the graph), so fresh runs and warm re-indexes dump
/// byte-identically (amendment 32); the `stderr_tail` is recorded for a `failed` or `timeout`
/// job (the build tool's text) and not for an `environment` one, whose text is the docker
/// client's and names the host (its redacted first line is the `reason`; amendment 49).
fn scip_status(p: &ScipProject, coverage: Option<&scip::Coverage>) -> Diagnostic {
    let mut detail = json!({
        "indexer": p.indexer,
        "status": p.status.as_str(),
        "unpinned": p.unpinned,
    });
    match p.status.reason() {
        Some(reason) => detail["reason"] = reason.into(),
        None => detail["image"] = p.image.as_str().into(),
    }
    if let Some(phase) = p.status.phase() {
        detail["phase"] = phase.as_str().into();
    }
    if let rb_scip::RunStatus::Timeout { budget_secs, .. } = &p.status {
        detail["budget_secs"] = (*budget_secs).into();
    }
    if let Some(c) = coverage {
        detail["coverage"] = json!({
            "definitions_matched": c.definitions_matched,
            "definitions_matched_by_name": c.definitions_matched_by_name,
            "definitions_local": c.definitions_local,
            "definitions_total": c.definitions_total,
            "definitions_without_occurrence": c.definitions_without_occurrence,
            "references_matched": c.references_matched,
            "references_external": c.references_external,
            "references_undocumented": c.references_undocumented,
            "references_local": c.references_local,
            "references_unanchored": c.references_unanchored,
            "references_unmatched": c.references_unmatched,
            "files_without_document": c.files_without_document,
            "supertypes_unresolved": c.supertypes_unresolved,
            "supertypes_unconfirmed": c.supertypes_unconfirmed,
            "occurrences_unconverted": c.occurrences_unconverted,
            "relationships_dropped": c.relationships_dropped,
            "relationships_dropped_in_project": c.relationships_dropped_in_project,
            "relationships_anonymous": c.relationships_anonymous,
            "documents_rejected": c.documents_rejected,
            "documents_rejected_reasons": c
                .documents_rejected_reasons
                .iter()
                .map(|(reason, n)| (reason.as_str().to_owned(), Value::from(*n)))
                .collect::<serde_json::Map<String, Value>>(),
        });
    }
    if matches!(
        p.status,
        rb_scip::RunStatus::Failed(_) | rb_scip::RunStatus::Timeout { .. }
    ) && !p.stderr_tail.is_empty()
    {
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
    use rb_frameworks::clients::RawScan;

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

    /// Contract M4a amendment 17: what the raw-text scan found ([`Content::unparsed`]) reaches
    /// the stitcher: a hooked kind is capped (`base: unverified`), discovery caps a
    /// host-by-name match (`discovery: configured`).
    #[test]
    fn the_raw_scan_reaches_the_stitcher() {
        let manifest = Manifest {
            commit_sha: "c".repeat(40),
            files: vec![
                entry("orders/package.json"),
                entry("orders/src/app.ts"),
                entry("web/package.json"),
                entry("web/src/t.ts"),
            ],
            skipped: vec![],
        };
        let boundaries = Boundaries::new(
            vec![
                project("orders", "orders/package.json"),
                project("web", "web/package.json"),
            ],
            vec![],
        );
        let app = rb_syntax::extract(
            rb_syntax::Language::TypeScript,
            b"import express from \"express\";\nconst app = express();\napp.get(\"/orders/:id\", (req, res) => res.json({}));\n",
        )
        .unwrap();
        let t = rb_syntax::extract(
            rb_syntax::Language::TypeScript,
            b"export function f() { return fetch(\"http://orders/orders/1\"); }\n",
        )
        .unwrap();
        let consumes = |unparsed: RawScan| {
            let content = Content {
                facts: BTreeMap::from([
                    ("orders/src/app.ts", (rb_syntax::Language::TypeScript, &app)),
                    ("web/src/t.ts", (rb_syntax::Language::TypeScript, &t)),
                ]),
                unparsed,
                ..Content::default()
            };
            let g = assemble(&manifest, None, &boundaries, &content).unwrap().0;
            g.edges()
                .iter()
                .filter(|e| e.kind == EdgeKind::Consumes)
                .map(|e| {
                    format!(
                        "{} base={} discovery={}",
                        e.certainty.as_str(),
                        e.attrs["base"].as_str().unwrap_or("?"),
                        e.attrs
                            .get("discovery")
                            .and_then(|d| d.as_str())
                            .unwrap_or("-")
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            consumes(RawScan::default()),
            ["synthetic base=literal discovery=-"]
        );
        assert_eq!(
            consumes(RawScan {
                hooks: BTreeSet::from([rb_frameworks::clients::ClientKind::Fetch]),
                discovery: false,
            }),
            ["heuristic base=unverified discovery=-"]
        );
        assert_eq!(
            consumes(RawScan {
                hooks: BTreeSet::new(),
                discovery: true,
            }),
            ["heuristic base=literal discovery=configured"]
        );
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

    /// Design §5.5: a SCIP-ok project's external symbols give `Package` nodes and one
    /// `DEPENDS_ON` edge per service and package, from the service of the referencing file,
    /// with the first site as its only evidence.
    #[test]
    fn external_symbols_give_one_depends_on_edge_per_service_and_package() {
        let src = "import { Client } from 'lib';\nexport const c: Client = 1;\n";
        let facts = rb_syntax::extract(rb_syntax::Language::TypeScript, src.as_bytes()).unwrap();
        let paths = ["svc/src/a.ts", "svc/src/b.ts", "other/src/c.ts"];
        let manifest = Manifest {
            commit_sha: "c".repeat(40),
            files: ["svc/package.json", "other/package.json"]
                .into_iter()
                .chain(paths)
                .map(entry)
                .collect(),
            skipped: vec![],
        };
        let boundaries = Boundaries::new(
            vec![
                project("other", "other/package.json"),
                project("svc", "svc/package.json"),
            ],
            vec![],
        );
        let lib = "scip-typescript npm lib 2.0.0 `index.d.ts`/Client#";
        let index = rb_scip::ScipIndex {
            documents: paths
                .iter()
                .map(|path| rb_scip::Document {
                    path: (*path).to_owned(),
                    occurrences: vec![
                        rb_scip::Occurrence {
                            span: [0, 9, 0, 15],
                            symbol: lib.into(),
                            roles: 2,
                            enclosing: None,
                        },
                        rb_scip::Occurrence {
                            span: [1, 16, 1, 22],
                            symbol: lib.into(),
                            roles: 0,
                            enclosing: None,
                        },
                    ],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let content = Content {
            facts: paths
                .iter()
                .map(|p| (*p, (rb_syntax::Language::TypeScript, &facts)))
                .collect(),
            scip: vec![ScipProject {
                root: String::new(),
                indexer: "scip-typescript",
                image: "img".into(),
                status: rb_scip::RunStatus::Ok,
                index: Some(index),
                elapsed_ms: 0,
                cached: false,
                stderr_tail: String::new(),
                unpinned: false,
            }],
            ..Content::default()
        };
        let g = assemble(&manifest, None, &boundaries, &content).unwrap().0;
        let package = g
            .nodes()
            .iter()
            .find(|n| n.kind == NodeKind::Package)
            .unwrap();
        assert_eq!(package.key, "npm/lib@2.0.0");
        assert_eq!(package.name, "lib");
        assert_eq!(package.attrs["version"], "2.0.0");
        assert_eq!(
            edges_of(&g, EdgeKind::DependsOn),
            [
                ("other".into(), "npm/lib@2.0.0".into()),
                ("svc".into(), "npm/lib@2.0.0".into()),
            ]
        );
        let evidence: Vec<Vec<(String, rb_model::Span)>> = g
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::DependsOn)
            .map(|e| {
                assert_eq!(
                    (e.provenance, e.certainty),
                    (Provenance::Scip, Certainty::Exact)
                );
                e.evidence
                    .iter()
                    .map(|ev| (ev.path.clone(), ev.span))
                    .collect()
            })
            .collect();
        assert_eq!(
            evidence,
            [
                vec![("other/src/c.ts".into(), [0, 9, 0, 15])],
                vec![("svc/src/a.ts".into(), [0, 9, 0, 15])],
            ]
        );
    }

    /// A definition occurrence at every definition name and a reference occurrence at every
    /// call, for one file, as an ok index would carry them.
    fn document_for(
        path: &str,
        facts: &rb_syntax::FileFacts,
        sym: &dyn Fn(&str) -> String,
    ) -> rb_scip::Document {
        let mut occurrences: Vec<rb_scip::Occurrence> = facts
            .definitions
            .iter()
            .map(|d| rb_scip::Occurrence {
                span: d.name_span,
                symbol: sym(&d.name),
                roles: 1,
                enclosing: None,
            })
            .collect();
        occurrences.extend(facts.references.iter().map(|r| rb_scip::Occurrence {
            span: r.span,
            symbol: sym(&r.name),
            roles: 0,
            enclosing: None,
        }));
        rb_scip::Document {
            path: path.into(),
            occurrences,
            ..Default::default()
        }
    }

    fn ok_project(
        root: &str,
        indexer: &'static str,
        documents: Vec<rb_scip::Document>,
    ) -> ScipProject {
        ScipProject {
            root: root.into(),
            indexer,
            image: "img".into(),
            status: rb_scip::RunStatus::Ok,
            index: Some(rb_scip::ScipIndex {
                documents,
                ..Default::default()
            }),
            elapsed_ms: 0,
            cached: false,
            stderr_tail: String::new(),
            unpinned: false,
        }
    }

    /// `(src key, dst key, provenance)` of every `CALLS` edge.
    fn calls(g: &Graph) -> Vec<(String, String, Provenance)> {
        let key = |id: i64| g.nodes().iter().find(|n| n.id == id).unwrap().key.clone();
        g.edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Calls)
            .map(|e| (key(e.src), key(e.dst), e.provenance))
            .collect()
    }

    fn files_without_document(diagnostics: &[Diagnostic]) -> u64 {
        let d = diagnostics
            .iter()
            .find(|d| d.kind == "scip_status")
            .expect("one scip_status diagnostic");
        d.detail["coverage"]["files_without_document"]
            .as_u64()
            .expect("coverage of an ok project")
    }

    /// Contract M3b amendment 15: ownership is by document. A Python file under a SCIP-ok Maven
    /// root has no document, so it keeps its Tier B edges and is not a "file without a
    /// document" (scip-java never indexes Python).
    #[test]
    fn a_file_of_another_language_under_a_scip_ok_root_keeps_tier_b() {
        let java = rb_syntax::extract(
            rb_syntax::Language::Java,
            b"package p;\nclass A { void a() {} }\n",
        )
        .unwrap();
        let python = rb_syntax::extract(
            rb_syntax::Language::Python,
            b"def helper():\n    pass\n\ndef main():\n    helper()\n",
        )
        .unwrap();
        let manifest = Manifest {
            commit_sha: "c".repeat(40),
            files: ["pom.xml", "src/A.java", "scripts/tool.py"]
                .map(entry)
                .to_vec(),
            skipped: vec![],
        };
        let boundaries = Boundaries::new(vec![project(".", "pom.xml")], vec![]);
        let sym = |name: &str| format!("scip-java maven . . p/{name}");
        let content = Content {
            facts: BTreeMap::from([
                ("src/A.java", (rb_syntax::Language::Java, &java)),
                ("scripts/tool.py", (rb_syntax::Language::Python, &python)),
            ]),
            scip: vec![ok_project(
                "",
                "scip-java",
                vec![document_for("src/A.java", &java, &sym)],
            )],
            ..Content::default()
        };
        let (g, diagnostics) = assemble(&manifest, None, &boundaries, &content).unwrap();
        let py_keys = rb_syntax::symbol_keys("scripts/tool.py", &python);
        let main = py_keys[python
            .definitions
            .iter()
            .position(|d| d.name == "main")
            .unwrap()]
        .clone();
        let helper = py_keys[python
            .definitions
            .iter()
            .position(|d| d.name == "helper")
            .unwrap()]
        .clone();
        assert_eq!(calls(&g), [(main, helper, Provenance::Heuristic)]);
        assert_eq!(files_without_document(&diagnostics), 0);
    }

    /// Contract M3b amendment 15: a TypeScript file the index has no document for (excluded by
    /// the tsconfig) keeps Tier B, while the documented file gets only exact edges; the missing
    /// document is counted, since scip-typescript should have covered the file.
    #[test]
    fn a_file_without_a_document_keeps_tier_b_next_to_a_documented_one() {
        let src = rb_syntax::extract(
            rb_syntax::Language::TypeScript,
            b"export function f() {}\nexport function g() { f(); }\n",
        )
        .unwrap();
        let spec = rb_syntax::extract(
            rb_syntax::Language::TypeScript,
            b"function h() {}\nfunction t() { h(); }\n",
        )
        .unwrap();
        let manifest = Manifest {
            commit_sha: "c".repeat(40),
            files: [
                "package.json",
                "tsconfig.json",
                "src/a.ts",
                "test/a.spec.ts",
            ]
            .map(entry)
            .to_vec(),
            skipped: vec![],
        };
        let boundaries = Boundaries::new(vec![project(".", "package.json")], vec![]);
        let sym = |name: &str| format!("scip-typescript npm p 1.0.0 src/`a.ts`/{name}().");
        let content = Content {
            facts: BTreeMap::from([
                ("src/a.ts", (rb_syntax::Language::TypeScript, &src)),
                ("test/a.spec.ts", (rb_syntax::Language::TypeScript, &spec)),
            ]),
            scip: vec![ok_project(
                "",
                "scip-typescript",
                vec![document_for("src/a.ts", &src, &sym)],
            )],
            ..Content::default()
        };
        let (g, diagnostics) = assemble(&manifest, None, &boundaries, &content).unwrap();
        let spec_keys = rb_syntax::symbol_keys("test/a.spec.ts", &spec);
        let t = spec_keys[spec.definitions.iter().position(|d| d.name == "t").unwrap()].clone();
        let h = spec_keys[spec.definitions.iter().position(|d| d.name == "h").unwrap()].clone();
        // In the graph's canonical order (heuristic keys sort before SCIP ones).
        assert_eq!(
            calls(&g),
            [
                (t, h, Provenance::Heuristic),
                (sym("g"), sym("f"), Provenance::Scip),
            ]
        );
        assert_eq!(files_without_document(&diagnostics), 1);
    }

    /// Contract M3b amendments 1 and 38: a file belongs to the deepest build root containing
    /// it. `pkg/src/a.ts` lies under the outer root, whose ok index happens to carry a document
    /// for it, and under the nested `pkg` package, whose own job failed: the nested root owns
    /// it, so it keeps its heuristic call and the outer index's document for it is not used.
    #[test]
    fn a_file_of_a_nested_failed_package_stays_heuristic_under_an_ok_outer_root() {
        let outer = rb_syntax::extract(
            rb_syntax::Language::TypeScript,
            b"export function f() {}\nexport function g() { f(); }\n",
        )
        .unwrap();
        let nested = rb_syntax::extract(
            rb_syntax::Language::TypeScript,
            b"export function h() {}\nexport function t() { h(); }\n",
        )
        .unwrap();
        let manifest = Manifest {
            commit_sha: "c".repeat(40),
            files: [
                "package.json",
                "tsconfig.json",
                "src/a.ts",
                "pkg/package.json",
                "pkg/tsconfig.json",
                "pkg/src/a.ts",
            ]
            .map(entry)
            .to_vec(),
            skipped: vec![],
        };
        let boundaries = Boundaries::new(
            vec![
                project(".", "package.json"),
                project("pkg", "pkg/package.json"),
            ],
            vec![],
        );
        let outer_sym = |name: &str| format!("scip-typescript npm p 1.0.0 src/`a.ts`/{name}().");
        let nested_sym =
            |name: &str| format!("scip-typescript npm p 1.0.0 pkg/src/`a.ts`/{name}().");
        let mut failed = ok_project("pkg", "scip-typescript", vec![]);
        failed.status = rb_scip::RunStatus::Failed(rb_scip::Phase::Fetch);
        failed.index = None;
        let content = Content {
            facts: BTreeMap::from([
                ("src/a.ts", (rb_syntax::Language::TypeScript, &outer)),
                ("pkg/src/a.ts", (rb_syntax::Language::TypeScript, &nested)),
            ]),
            scip: vec![
                ok_project(
                    "",
                    "scip-typescript",
                    vec![
                        document_for("src/a.ts", &outer, &outer_sym),
                        document_for("pkg/src/a.ts", &nested, &nested_sym),
                    ],
                ),
                failed,
            ],
            ..Content::default()
        };
        let (g, _) = assemble(&manifest, None, &boundaries, &content).unwrap();
        let nested_keys = rb_syntax::symbol_keys("pkg/src/a.ts", &nested);
        let t = nested_keys[nested
            .definitions
            .iter()
            .position(|d| d.name == "t")
            .unwrap()]
        .clone();
        let h = nested_keys[nested
            .definitions
            .iter()
            .position(|d| d.name == "h")
            .unwrap()]
        .clone();
        assert_eq!(
            calls(&g),
            [
                (t, h, Provenance::Heuristic),
                (outer_sym("g"), outer_sym("f"), Provenance::Scip),
            ]
        );
        assert!(
            !g.nodes().iter().any(|n| n.key == nested_sym("h")),
            "the outer index's document for the nested file was used"
        );
    }

    /// Contract M3b amendment 49: an `environment` job's diagnostic carries the redacted
    /// reason only; the docker client's whole text (which names the host) stays out of the
    /// dump, while a failed build's stderr tail is recorded.
    #[test]
    fn scip_status_records_no_environment_detail() {
        let mut project = ok_project("", "scip-java", vec![]);
        project.index = None;
        project.status = rb_scip::RunStatus::Environment(
            "Cannot connect to the Docker daemon at unix://<host path>. Is the docker daemon running?".into(),
        );
        project.stderr_tail =
            "Cannot connect to the Docker daemon at unix:///Users/rohit/.colima/default/docker.sock. Is the docker daemon running?".into();
        let d = scip_status(&project, None);
        assert_eq!(d.detail["status"], "environment");
        assert_eq!(
            d.detail["reason"],
            "Cannot connect to the Docker daemon at unix://<host path>. Is the docker daemon running?"
        );
        assert!(d.detail.get("stderr_tail").is_none(), "{}", d.detail);
        assert!(!d.detail.to_string().contains("/Users"), "{}", d.detail);

        project.status = rb_scip::RunStatus::Failed(rb_scip::Phase::Fetch);
        project.stderr_tail = "[ERROR] boom".into();
        let d = scip_status(&project, None);
        assert_eq!(d.detail["stderr_tail"], "[ERROR] boom");
        // The phase that failed is recorded, as for a timeout (contract M3b amendment 56).
        assert_eq!(d.detail["phase"], "fetch");
        assert!(d.detail.get("budget_secs").is_none(), "{}", d.detail);
    }

    /// Contract M4a amendment 16 (evaluator pass 6, V2): a Feign client key in any settings
    /// file (a config server's shared file too) caps every Feign site; other Feign keys and
    /// other files do not.
    #[test]
    fn feign_client_config_in_any_settings_file() {
        let shared = "config/src/main/resources/shared/orders.yml";
        assert!(is_config_input(shared));
        let configured = |files: &[(&str, &str)]| {
            let config: BTreeMap<&str, String> =
                files.iter().map(|(p, t)| (*p, (*t).to_owned())).collect();
            feign_configured(&config)
        };
        assert!(configured(&[(
            shared,
            "feign:\n  client:\n    config:\n      default:\n        requestInterceptors:\n          - p.Rewrite\n"
        )]));
        assert!(configured(&[(
            "a/src/main/resources/application.properties",
            "spring.cloud.openfeign.client.config.orders.url=https://api.stripe.com\n"
        )]));
        assert!(!configured(&[
            (shared, "feign:\n  hystrix:\n    enabled: true\n"),
            ("a/pom.xml", "<x>feign.client.config.default.url=y</x>\n"),
        ]));
    }

    /// Evaluator pass 1, P7: a service with Spring configuration M4a does not read (profile
    /// files, YAML, `bootstrap*`, or a Spring Cloud Config client by dependency or by key) gets
    /// one `config_unread` diagnostic; the keys those files set are overrides, all keys for a
    /// config client; `application.properties` alone is read and stays clean.
    #[test]
    fn spring_configuration_beyond_application_properties_is_counted() {
        let res = "src/main/resources";
        let files: Vec<(String, &str)> = vec![
            (
                format!("a/{res}/application.properties"),
                "users.url=http://users\n",
            ),
            (
                format!("a/{res}/application-prod.properties"),
                "users.url=http://legacy\n",
            ),
            (
                format!("b/{res}/bootstrap.yml"),
                "spring:\n  cloud:\n    config:\n      uri: http://config\n",
            ),
            (
                "c/pom.xml".to_owned(),
                "<dependency><artifactId>spring-cloud-starter-config</artifactId></dependency>",
            ),
            (
                format!("d/{res}/application.properties"),
                "spring.config.import=optional:configserver:\n",
            ),
            (
                format!("e/{res}/application.yml"),
                "users:\n  url: http://y\n",
            ),
            (format!("f/{res}/application.properties"), "x=1\n"),
            (
                "f/pom.xml".to_owned(),
                "<artifactId>spring-boot-starter-web</artifactId>",
            ),
            (
                format!("g/{res}/application-dev.yaml"),
                "server:\n  servlet:\n    context-path: /g\n",
            ),
            // A config server's own keys do not make it a client.
            (
                format!("h/{res}/application.yml"),
                "spring:\n  cloud:\n    config:\n      server:\n        git:\n          uri: x\n",
            ),
            (
                format!("i/{res}/application.properties"),
                "spring.cloud.config.server.native.search-locations=classpath:/shared\n",
            ),
        ];
        let config: BTreeMap<&str, String> = files
            .iter()
            .map(|(p, t)| (p.as_str(), (*t).to_owned()))
            .collect();
        assert!(
            files.iter().all(|(p, _)| is_config_input(p)),
            "every file is read"
        );
        assert!(!is_config_input("a/src/main/resources/db/migration/V1.sql"));
        // Read only for Feign client keys (amendment 16), not as Spring configuration.
        assert!(is_config_input("a/src/main/resources/messages.properties"));
        assert!(config_file("a/src/main/resources/messages.properties").is_none());
        let service_of = |p: &str| p.split('/').next().unwrap().to_owned();
        let (spring, diagnostics) = spring_config(&config, &service_of);
        let unread: Vec<String> = diagnostics
            .iter()
            .filter(|d| d.kind == "config_unread")
            .map(|d| format!("{} {}", d.path.as_deref().unwrap(), d.detail))
            .collect();
        assert_eq!(
            unread,
            [
                "a/src/main/resources/application-prod.properties {\"files\":[\"a/src/main/resources/application-prod.properties\"],\"reasons\":[\"profile\"],\"service\":\"a\"}",
                "b/src/main/resources/bootstrap.yml {\"files\":[\"b/src/main/resources/bootstrap.yml\"],\"reasons\":[\"bootstrap\",\"config_client\"],\"service\":\"b\"}",
                "c/pom.xml {\"files\":[\"c/pom.xml\"],\"reasons\":[\"config_client\"],\"service\":\"c\"}",
                "d/src/main/resources/application.properties {\"files\":[\"d/src/main/resources/application.properties\"],\"reasons\":[\"config_client\"],\"service\":\"d\"}",
                "e/src/main/resources/application.yml {\"files\":[\"e/src/main/resources/application.yml\"],\"reasons\":[\"yaml\"],\"service\":\"e\"}",
                "g/src/main/resources/application-dev.yaml {\"files\":[\"g/src/main/resources/application-dev.yaml\"],\"reasons\":[\"profile\"],\"service\":\"g\"}",
                "h/src/main/resources/application.yml {\"files\":[\"h/src/main/resources/application.yml\"],\"reasons\":[\"yaml\"],\"service\":\"h\"}",
            ]
        );
        // The YAML file setting a context path keeps its own diagnostic (S4).
        assert!(diagnostics.iter().any(|d| d.kind == "spring_config_unread"
            && d.path.as_deref() == Some("g/src/main/resources/application-dev.yaml")));
        assert_eq!(
            spring.properties["a"]["users.url"], "http://users",
            "a profile file is not merged into the read properties"
        );
        assert!(spring.properties["f"].contains_key("x"));
        assert!(spring.overrides["a"].sets("users.url"));
        assert!(!spring.overrides["a"].sets("other"));
        assert!(spring.overrides["e"].sets("users.url"));
        for client in ["b", "c", "d"] {
            assert!(spring.overrides[client].sets("anything"), "{client}");
        }
        assert!(!spring.overrides.contains_key("f"));
        let unread_services: Vec<&str> = spring.unread.iter().map(String::as_str).collect();
        assert_eq!(unread_services, ["a", "b", "c", "d", "e", "g", "h"]);
        assert!(!spring.overrides["h"].all, "a config server is no client");
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q3): Spring Boot's `classpath:/config/`
    /// location counts (its `application.properties` is not read either, so it is counted with
    /// the reason `config_dir`); a flow mapping's keys are overrides; a service that turns the
    /// config client off (`spring.cloud.config.enabled=false` in its base document; amendment
    /// 13) is no client.
    #[test]
    fn config_directories_flow_mappings_and_disabled_clients() {
        let res = "src/main/resources";
        let files: Vec<(String, &str)> = vec![
            (
                format!("a/{res}/application.properties"),
                "stripe.url=http://orders\n",
            ),
            (
                format!("a/{res}/application-prod.yml"),
                "stripe: { url: \"https://api.stripe.com\" }\n",
            ),
            (
                format!("b/{res}/config/application-prod.properties"),
                "users.url=https://legacy-users.example.com\n",
            ),
            (
                format!("c/{res}/config/application.properties"),
                "users.url=https://legacy-users.example.com\n",
            ),
            (
                format!("d/{res}/application.properties"),
                "spring.cloud.config.enabled=false\nspring.cloud.config.uri=http://c\n",
            ),
            (
                "e/pom.xml".to_owned(),
                "<artifactId>spring-cloud-starter-config</artifactId>",
            ),
            (
                format!("e/{res}/application.properties"),
                "spring.cloud.config.enabled=false\n",
            ),
            (
                format!("f/{res}/application.yml"),
                "spring: {cloud: {config: {enabled: \"false\"}}}\n",
            ),
            (
                format!("g/{res}/application.properties"),
                "spring.cloud.config.enabled=true\n",
            ),
        ];
        assert!(files.iter().all(|(p, _)| is_config_input(p)));
        assert!(config_file("a/src/main/resources/config/messages.properties").is_none());
        assert!(config_file("a/src/main/resources/config/x/application.yml").is_none());
        let config: BTreeMap<&str, String> = files
            .iter()
            .map(|(p, t)| (p.as_str(), (*t).to_owned()))
            .collect();
        let service_of = |p: &str| p.split('/').next().unwrap().to_owned();
        let (spring, diagnostics) = spring_config(&config, &service_of);
        let unread: Vec<String> = diagnostics
            .iter()
            .filter(|d| d.kind == "config_unread")
            .map(|d| format!("{} {}", d.detail["service"], d.detail["reasons"]))
            .collect();
        assert_eq!(
            unread,
            [
                "\"a\" [\"profile\"]",
                "\"b\" [\"profile\"]",
                "\"c\" [\"config_dir\"]",
                "\"f\" [\"yaml\"]",
                "\"g\" [\"config_client\"]",
            ]
        );
        assert!(spring.overrides["a"].sets("stripe.url"));
        assert!(spring.overrides["b"].sets("users.url"));
        assert!(spring.overrides["c"].sets("users.url"));
        assert!(!spring.overrides["f"].all);
        assert!(!spring.overrides.contains_key("d") && !spring.overrides.contains_key("e"));
        assert!(spring.overrides["g"].all);
        // `config/application.properties` is not merged into the read properties.
        assert!(
            !spring
                .properties
                .get("c")
                .is_some_and(|p| p.contains_key("users.url"))
        );
    }

    /// Contract M4a amendments 13 and 14 (evaluator passes 3 and 4, R3 and T5):
    /// `spring.cloud.config.enabled=false` turns the config client off only in the base
    /// (unconditional) `application.properties` or `application.yml` document, and only when no
    /// profile file or document sets it back to `true`. A profile file
    /// (`application-local.properties`), a profile `bootstrap` file, or a conditional document
    /// of `application.yml` or of a multi-document `application.properties` (`#---`): any
    /// `spring.config.activate.*` key (`on-profile`, `on-cloud-platform`) or the legacy
    /// `spring.profiles`, never turns it off; `spring.profiles.active` is no condition.
    #[test]
    fn only_the_base_document_turns_the_config_client_off() {
        let res = "src/main/resources";
        let pom = "<artifactId>spring-cloud-starter-config</artifactId>";
        let files: Vec<(String, &str)> = vec![
            ("h/pom.xml".to_owned(), pom),
            (
                format!("h/{res}/application.properties"),
                "users.url=http://users\n",
            ),
            (
                format!("h/{res}/application-local.properties"),
                "spring.cloud.config.enabled=false\n",
            ),
            ("i/pom.xml".to_owned(), pom),
            (
                format!("i/{res}/bootstrap-local.yml"),
                "spring:\n  config:\n    activate:\n      on-profile: local\n  cloud:\n    config:\n      enabled: false\n",
            ),
            ("j/pom.xml".to_owned(), pom),
            (
                format!("j/{res}/application.yml"),
                "users:\n  url: http://users\n---\nspring:\n  config:\n    activate:\n      on-profile: test\n  cloud:\n    config:\n      enabled: false\n",
            ),
            ("k/pom.xml".to_owned(), pom),
            (
                format!("k/{res}/application.properties"),
                "spring.cloud.config.enabled=false\n",
            ),
            (
                format!("k/{res}/application-prod.properties"),
                "spring.cloud.config.enabled=true\n",
            ),
            ("l/pom.xml".to_owned(), pom),
            (
                format!("l/{res}/application.properties"),
                "users.url=http://users\n#---\nspring.config.activate.on-profile=local\nspring.cloud.config.enabled=false\n",
            ),
            ("m/pom.xml".to_owned(), pom),
            (
                format!("m/{res}/application.yml"),
                "users.url: http://users\n---\nspring.profiles: test\nspring.cloud.config.enabled: false\n",
            ),
            // Activated on a cloud platform, not by a profile (evaluator pass 4, T5: `bk7`,
            // `bk6`): still not the base document.
            ("o/pom.xml".to_owned(), pom),
            (
                format!("o/{res}/application.properties"),
                "users.url=http://users\nspring.config.import=optional:configserver:http://cfgsrv:8888\n#---\nspring.config.activate.on-cloud-platform=kubernetes\nspring.cloud.config.enabled=false\n",
            ),
            ("p/pom.xml".to_owned(), pom),
            (
                format!("p/{res}/application.yml"),
                "users:\n  url: http://users\n---\nspring:\n  config:\n    activate:\n      on-cloud-platform: kubernetes\n  cloud:\n    config:\n      enabled: false\n",
            ),
            // `spring.profiles.active` in the base document is no activation condition.
            ("q/pom.xml".to_owned(), pom),
            (
                format!("q/{res}/application.yml"),
                "spring:\n  profiles:\n    active: dev\n  cloud:\n    config:\n      enabled: false\n",
            ),
            // The base document does turn it off.
            ("n/pom.xml".to_owned(), pom),
            (
                format!("n/{res}/application.yml"),
                "spring:\n  cloud:\n    config:\n      enabled: false\n---\nspring:\n  config:\n    activate:\n      on-profile: test\nusers:\n  url: http://users\n",
            ),
        ];
        let config: BTreeMap<&str, String> = files
            .iter()
            .map(|(p, t)| (p.as_str(), (*t).to_owned()))
            .collect();
        let service_of = |p: &str| p.split('/').next().unwrap().to_owned();
        let (spring, diagnostics) = spring_config(&config, &service_of);
        let unread: Vec<String> = diagnostics
            .iter()
            .filter(|d| d.kind == "config_unread")
            .map(|d| format!("{} {}", d.detail["service"], d.detail["reasons"]))
            .collect();
        assert_eq!(
            unread,
            [
                "\"h\" [\"config_client\",\"profile\"]",
                "\"i\" [\"bootstrap\",\"config_client\"]",
                "\"j\" [\"config_client\",\"yaml\"]",
                "\"k\" [\"config_client\",\"profile\"]",
                "\"l\" [\"config_client\"]",
                "\"m\" [\"config_client\",\"yaml\"]",
                "\"n\" [\"yaml\"]",
                "\"o\" [\"config_client\"]",
                "\"p\" [\"config_client\",\"yaml\"]",
                "\"q\" [\"yaml\"]",
            ]
        );
        for service in ["h", "i", "j", "k", "l", "m", "o", "p"] {
            assert!(spring.overrides[service].all, "{service}");
        }
        assert!(!spring.overrides["n"].all);
        assert!(!spring.overrides["q"].all);
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
