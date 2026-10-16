//! Indexing pipeline (design §3.1): commit → manifest → structure (CODEOWNERS, boundaries) → graph.
//!
//! Everything here is a pure function of the commit's tree, so the same commit always yields the
//! same graph and diagnostics.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::Path;

mod constants;
mod di;
mod exclude;
mod liveness;
mod modules;
mod orm;
mod packages;
mod projects;
mod resolve;
mod routes;
mod rta;
mod scip;
mod sql;
mod stitch;
mod syntax;
mod topics;
mod xml_beans;

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
/// and Service → Service `DEPENDS_ON` `via=package` (contract M4b S15, S16); `m4b-di-2` Spring
/// DI's `INJECTS` edges, `di_*` diagnostics and `di_coverage` (S4–S8); `m4b-di-3` RTA's
/// `impl_instantiated: false` on `IMPLEMENTS` and `rta_coverage` (S9); `m4b-di-4` the `Symbol`
/// attributes `entry` and `orphan_eligible` and `orphan_coverage` (S11, S12); `m4b-di-5`
/// Express mounts of default imports (S19) and catch-all endpoints only for named hosts (S20);
/// `m4b-di-6` injected Spring clients' bases from the `@Bean` method DI resolves (S21);
/// `m4b-di-7` an external injected type matches a `@Bean` return type only when the names as
/// written agree (amendment 19); `m4b-di-8` evaluator pass 1's (amendment 21: RTA reads binary
/// names, every text file and module objects, a supertype of a non-definition keeps its base;
/// orphan rule (e) walks the supertype chain; DI caps by every assignable registration, never
/// narrows a generic or annotated point by `@Primary`, reads `@Named`; duplicate Maven
/// versions, npm aliases and `go.mod` replaces).
/// `m5-1`: a `CONSUMES` edge matched by suffix because no host resolved says `base: unresolved`
/// (contract M5 S18, M4a open item 6); `m5-2` the carrier review's fixes (S19 (a), (b), (f)):
/// discovery words and JSON key paths in every file, parsed ones included; XML comments read;
/// a contested base with an external candidate is external; `m5-3` the orphan and DI fixes
/// (S19 (d), (e), (g)): Spring Data fragment implementations are no orphans, a supertype head
/// resolved by name is external unless unique and imported from nowhere outside, and a class a
/// class literal registers is a DI candidate (`ambiguous` beside the scanned beans).
/// `m6a-1`: scip-java documents are judged without their Lombok-generated occurrences
/// (contract M6a D1); `m6a-2` a scip-typescript interface method's `IMPLEMENTS` is confirmed by
/// its declared clauses alone (D4); `m6a-3` npm and Yarn workspaces are indexed by their root's
/// job (D5, D6).
/// `m6b-1` gin groups passed as parameters compose from their call sites (S2).
/// `m6b-2` Express construction chains and anonymous default-exported routers (S3).
/// `m6b-3` in-process and empty-URL Go requests are `unmatched`, never edges (S4).
/// `m6b-4` axios and certainty folding: cross-file spreads, by-name configs, ordered Spring
/// statements (S5).
/// `m6b-5` constructor-assigned members fold from their single write (S6).
/// `m6b-6` userinfo holes match within the literal host (S7).
/// `m6b-7` discovery narrowing: parsed-file words cap only through
/// import-resolved known types or single-literal keys (S8).
/// `m6b-8` client breadth: got/ky/superagent/undici fetch-class sites,
/// HttpClient and axios member forms capped (S9).
/// `m6c-1` module objects: tsconfig `paths` + package `exports`/`main`
/// exact resolution for Tier B and RTA (M6c S4).
/// `m6c-2` non-bean consumers: `@Bean`-returned objects resolve from each
/// returning method, `ConstraintValidator`s from their class, always at
/// `heuristic` (M6c S6).
/// `m6c-3` inherited Spring Data calls: allow-listed member names on
/// repository-typed receivers link to the interface node with `inherited`
/// (M6c S7).
/// `m6c-4` Lombok qualifiers: a field qualifier a `lombok.config` chain lists
/// rides the generated parameter and narrows, counted (M6c S8).
/// `m6c-5` exact type-arg agreement: the single bean in simple-name agreement
/// picks at `heuristic`, capped by the generic gate, counted (M6c S9a).
/// `m7a-1` `exact`-label correctness: Go/TS gRPC clients typed by the constructor
/// link (S1), FastAPI through the shared router composer (S2), gRPC endpoints by
/// server registration with `grpc_implementer_unknown` counted (S3).
/// `m7c-1` HTTP recall: Express paths written as names fold across files (object
/// members, default imports) or are counted dynamic (S1); default-imported axios
/// instances (S2); `require('express')()` (S3); TS gRPC stubs need a generated import (S4).
/// `m7d-1` DI and SQL roles: .NET implicit/global usings import DI (S1), implementation-
/// registered owners (S2), framework-provided points are `external` (S4), `.sql` files are
/// migrations only by evidence, else `QUERIES` (S5).
/// `m7e-1` input hygiene: vendored and built files (rb.toml `ignore`, Go `vendor/`,
/// `node_modules`, `.gitattributes` linguist marks, tsconfig `outDir` outputs) keep
/// their `File` node with `excluded` and are not analysed; counted per reason.
/// `m7g-1` honesty: php/scala/swift/dart/elixir files named; code without a grammar
/// counted (`language_unsupported`); frameworks without a pack named
/// (`framework_unsupported`).
/// `m7i-1` Express routes match case-insensitively unless their router opts in
/// (`CONSUMES` `case: folded`); JSON spec operations carry their path key's span.
/// `m7j-1` `case: folded` decided per candidate (it was copied from the first).
/// `m9a-1` `exact` precision: test-file routes are not endpoints (B7), spec-only
/// endpoints are `synthetic` (B8), literal `"{id}"` paths (B12a), Go method sources take
/// no interface bypass (B13), native config server context paths (B14).
/// `m9a-2` an empty remote context path clears a lower one (M9a evaluator pass 1, E3).
/// `m9b-1` Go methods declared outside their receiver type's file are its members and no
/// package-level names (B9); `fmt.Sprintf` SQL folds package constants (B10); Spring Data REST
/// repositories expose `synthetic` endpoints (B12b).
pub const PIPELINE: &str = "m9b-2";

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
    /// Workspace roots whose members were not grouped, and why (contract M6a D5): printed by
    /// `rb index`, never stored, so a dump does not change through them.
    pub scip_ungrouped: Vec<rb_scip::jobs::Ungrouped>,
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

    // Contract M7e: one exclusion decision, from the full tree. Every analysis stage below sees
    // only `analysed`; `File` nodes and ownership keep the whole manifest.
    let excluded = exclude::decide(&manifest.files, &read)?;
    let mut by_reason: BTreeMap<&str, usize> = BTreeMap::new();
    for reason in excluded.values() {
        *by_reason.entry(reason).or_default() += 1;
    }
    diagnostics.extend(by_reason.into_iter().map(|(reason, files)| Diagnostic {
        kind: "excluded".into(),
        path: None,
        span: None,
        detail: json!({ "reason": reason, "files": files }),
    }));
    let analysed: Vec<FileEntry> = manifest
        .files
        .iter()
        .filter(|f| !excluded.contains_key(&f.path))
        .cloned()
        .collect();

    let codeowners = load_codeowners(&manifest, &read, &mut diagnostics)?;
    let boundaries = boundaries::detect(&analysed, &read)?;
    diagnostics.extend(boundaries.diagnostics.iter().cloned());

    let (plan, skipped) = syntax::parse_plan(&analysed);
    diagnostics.extend(skipped);
    // Files with no facts are scanned as text for hooks and discovery (amendment 17).
    let (unparsed, scanned) =
        syntax::scan_unparsed(&analysed, &plan, &|blob: &str| repo.blob(blob))?;
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
    // The whole-word scan of every non-binary file for the definitions' names (contract M4b
    // S12 (f)); it depends on the whole commit, so it is not cached.
    let names: HashSet<&str> = facts_by_path
        .values()
        .flat_map(|(_, f)| f.definitions.iter().map(|d| d.name.as_str()))
        .collect();
    let mut words = liveness::Words::scanned();
    for f in analysed.iter().filter(|f| !f.binary) {
        let bytes = (f.size <= liveness::WORD_SCAN_BYTES)
            .then(|| repo.blob(&f.blob_sha))
            .transpose()?;
        words.scan(&f.path, bytes.as_deref(), &names);
    }

    let mut texts = |lang: &str| -> anyhow::Result<BTreeMap<&str, String>> {
        let (files, skipped) = sql::text_plan(&analysed, lang);
        diagnostics.extend(skipped);
        files
            .into_iter()
            .map(|f| Ok((f.path.as_str(), repo.blob_text(&f.blob_sha)?)))
            .collect()
    };
    let go_modules = analysed
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
    let (scip_projects, scip_ungrouped) = projects::run_projects(
        &analysed,
        &|blob: &str| repo.blob(blob),
        &mode,
        scip,
        scip_cache,
    )?;
    let config = analysed
        .iter()
        .filter(|f| is_config_input(&f.path))
        .map(|f| Ok((f.path.as_str(), repo.blob_text(&f.blob_sha)?)))
        .collect::<anyhow::Result<_>>()?;
    let manifests = analysed
        .iter()
        .filter(|f| packages::is_input(&f.path) && !f.binary)
        .map(|f| Ok((f.path.as_str(), repo.blob_text(&f.blob_sha)?)))
        .collect::<anyhow::Result<_>>()?;
    let specs = analysed
        .iter()
        .filter(|f| {
            (rb_frameworks::spec::is_spec_input(&f.path)
                || rb_frameworks::spec::is_proto_input(&f.path))
                && !f.binary
        })
        .map(|f| Ok((f.path.as_str(), repo.blob_text(&f.blob_sha)?)))
        .collect::<anyhow::Result<_>>()?;
    let mappers = analysed
        .iter()
        .filter(|f| f.path.ends_with(".xml") && !f.binary)
        .map(|f| Ok((f.path.as_str(), repo.blob_text(&f.blob_sha)?)))
        .collect::<anyhow::Result<_>>()?;
    let gateways = analysed
        .iter()
        .filter(|f| rb_frameworks::gateway::is_gateway_input(&f.path) && !f.binary)
        .map(|f| Ok((f.path.as_str(), repo.blob_text(&f.blob_sha)?)))
        .collect::<anyhow::Result<_>>()?;
    let host_files = analysed
        .iter()
        .filter(|f| rb_frameworks::hosts::is_hosts_input(&f.path) && !f.binary)
        .map(|f| Ok((f.path.as_str(), repo.blob_text(&f.blob_sha)?)))
        .collect::<anyhow::Result<BTreeMap<&str, String>>>()?;
    let rb_hosts = analysed
        .iter()
        .find(|f| f.path == "rb.toml" && !f.binary)
        .map(|f| {
            repo.blob_text(&f.blob_sha)
                .map(|t| rb_frameworks::hosts::rb_toml_hosts(&t))
        })
        .transpose()?
        .unwrap_or_default();
    let content = Content {
        excluded,
        facts: facts_by_path,
        sql: texts("sql")?,
        prisma: texts("prisma")?,
        go_modules,
        scip: scip_projects,
        config,
        unparsed,
        manifests,
        specs,
        mappers,
        gateways,
        host_files,
        rb_hosts,
        words,
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
        scip_ungrouped,
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
    /// Files excluded from analysis, with their reason (contract M7e): `File` nodes only.
    pub excluded: BTreeMap<String, &'static str>,
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
    /// OpenAPI/Swagger and `.proto` texts by path (contract M6f S3).
    pub specs: BTreeMap<&'a str, String>,
    /// `*.xml` texts by path; MyBatis mappers read by root tag, everything else skipped
    /// (contract M6f S5).
    pub mappers: BTreeMap<&'a str, String>,
    /// Gateway texts by path (`nginx.conf`, Spring Gateway `application*`/`bootstrap*` YAML;
    /// contract M6g S3).
    pub gateways: BTreeMap<&'a str, String>,
    /// Host-name texts by path (Compose files, Kubernetes YAML; contract M6g S4).
    pub host_files: BTreeMap<&'a str, String>,
    /// The root `rb.toml`'s `[[hosts]]` entries: `(service directory, names; contract M6g S4).
    pub rb_hosts: Vec<(String, Vec<String>)>,
    /// Where the definitions' names occur in every non-binary file (contract M4b S12 (f)); the
    /// default is "not scanned", under which no symbol is orphan-eligible.
    pub words: liveness::Words,
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
    /// Spring Cloud Config clients (the `config_client` reason).
    clients: BTreeSet<String>,
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

/// The key selecting active profiles (contract M6g S5). The environment and the command line
/// are never read (host-dependent), so only file-declared profiles activate.
const ACTIVE_PROFILES_KEY: &str = "spring.profiles.active";

/// A profile list value as names: comma/whitespace separated, quotes and brackets dropped.
fn split_profiles(value: &str) -> Vec<String> {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split([',', ' ', '\t', '\n'])
        .filter_map(|p| {
            let p = p.trim().trim_matches('"').trim_matches('\'').trim();
            (!p.is_empty()).then(|| p.to_owned())
        })
        .collect()
}

/// The profiles a configuration document selects: `spring.config.activate.on-profile` and the
/// legacy `spring.profiles` scalar (contract M6g S5). Cloud-platform conditions select none.
fn document_profiles(doc: &BTreeMap<String, String>) -> Vec<String> {
    let mut out = Vec::new();
    for key in ["spring.config.activate.on-profile", "spring.profiles"] {
        if let Some(value) = doc.get(key) {
            out.extend(split_profiles(value));
        }
    }
    out
}

/// The active profiles per service (contract M6g S5): the highest-precedence file that sets
/// `spring.profiles.active` wins (bootstrap < YAML < properties, per Spring's file order;
/// profile files cannot select themselves). Profile-specific and build files never select.
fn active_profiles(read: &[ConfigRead<'_>]) -> BTreeMap<String, BTreeSet<String>> {
    let rank = |kind: ConfigFile| match kind {
        ConfigFile::Bootstrap => 0,
        ConfigFile::Yaml => 1,
        ConfigFile::Read => 2,
        ConfigFile::Profile | ConfigFile::ConfigDir | ConfigFile::Build => 3,
    };
    let mut per_service: BTreeMap<String, Vec<(usize, BTreeSet<String>)>> = BTreeMap::new();
    for (path, text, kind, service, _) in read {
        rb_frameworks::work::tick(1);
        // Only unconditional documents select profiles: a profile-gated document cannot
        // select itself.
        let base: BTreeMap<String, String> = match kind {
            ConfigFile::Read => rb_frameworks::fold::properties_documents(text)
                .into_iter()
                .filter(|d| !rb_frameworks::fold::conditional_document(d))
                .flatten()
                .collect(),
            ConfigFile::Yaml => select_yaml_documents(text, &BTreeSet::new()).0,
            ConfigFile::Bootstrap if !path.ends_with(".properties") => {
                select_yaml_documents(text, &BTreeSet::new()).0
            }
            ConfigFile::Bootstrap => rb_frameworks::fold::parse_properties(text),
            ConfigFile::Profile | ConfigFile::ConfigDir | ConfigFile::Build => continue,
        };
        if let Some(value) = base.get(ACTIVE_PROFILES_KEY) {
            per_service
                .entry(service.clone())
                .or_default()
                .push((rank(*kind), split_profiles(value).into_iter().collect()));
        }
    }
    per_service
        .into_iter()
        .map(|(service, mut ranked)| {
            ranked.sort_by_key(|(r, _)| *r);
            (
                service,
                ranked.last().map(|(_, p)| p.clone()).unwrap_or_default(),
            )
        })
        .collect()
}

/// A YAML file's entries split into base and selected-profile documents (contract M6g S5):
/// unconditional documents always apply; conditional ones apply when they select an active
/// profile. A tabbed file is unreadable: it contributes nothing (today's reading is kept).
fn select_yaml_documents(
    text: &str,
    active: &BTreeSet<String>,
) -> (BTreeMap<String, String>, BTreeMap<String, String>) {
    let (mut base, mut profile) = (BTreeMap::new(), BTreeMap::new());
    if text.contains('\t') {
        return (base, profile);
    }
    for doc in rb_frameworks::fold::yaml_documents(text) {
        rb_frameworks::work::tick(1);
        if !rb_frameworks::fold::conditional_document(&doc) {
            base.extend(doc);
        } else if document_profiles(&doc).iter().any(|p| active.contains(p)) {
            profile.extend(doc);
        }
    }
    (base, profile)
}

/// The profile of an `application-<profile>.*` file name, or `None` for anything else.
fn profile_of(path: &str) -> Option<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let stem = name.rsplit_once('.')?.0;
    stem.strip_prefix("application-").map(str::to_owned)
}

/// One configuration file as read: path, text, kind, service, and its keys with their values.
type ConfigRead<'a> = (
    &'a str,
    &'a str,
    ConfigFile,
    String,
    BTreeMap<String, String>,
);

/// Reads the services' Spring configuration (evaluator pass 1, P7; contract M6g S5):
/// `application.properties` is read; `application.yml`/`.yaml` (unconditional documents plus
/// the active profiles' documents) and active `application-<profile>.*` files are read above
/// it; `bootstrap*` files are read lowest. Precedence, lowest first: bootstrap, YAML base,
/// properties, YAML profile documents, profile YAML, profile properties (profile files beat
/// base files, and `.properties` beats `.yml` in the same tier, per Spring's file order).
/// Profile files, YAML, `bootstrap*` files and a Spring Cloud Config client keep their
/// `config_unread` diagnostic and overrides (profiles can also come from the environment or
/// the command line, which are never read, so the caps stay conservative); a YAML file that
/// still contributes nothing and mentions `context-path` keeps `spring_config_unread`.
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
    let active = active_profiles(&read);
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
    // Selected values per service in precedence order (rank, path, entries): merged lowest
    // first after the loop, so profile files beat base files.
    let mut selected: Vec<(String, usize, &str, BTreeMap<String, String>)> = Vec::new();
    for (path, text, kind, service, entries) in &read {
        rb_frameworks::work::tick(1);
        let (path, text, kind, service) = (*path, *text, *kind, service.clone());
        let keys: BTreeSet<String> = entries.keys().cloned().collect();
        let active_here: BTreeSet<String> = active.get(&service).cloned().unwrap_or_default();
        // The values this file contributes (contract M6g S5); `None` reads as nothing.
        let mut values: Vec<(usize, BTreeMap<String, String>)> = Vec::new();
        match kind {
            ConfigFile::Read => values.push((2, entries.clone())),
            ConfigFile::Bootstrap if path.ends_with(".properties") => {
                values.push((0, entries.clone()));
            }
            ConfigFile::Bootstrap => {
                let (base, profile) = select_yaml_documents(text, &active_here);
                values.push((0, base));
                values.push((0, profile));
            }
            ConfigFile::Yaml => {
                let (base, profile) = select_yaml_documents(text, &active_here);
                values.push((1, base));
                values.push((3, profile));
            }
            ConfigFile::Profile => {
                let gated = profile_of(path).is_some_and(|p| active_here.contains(&p));
                if gated {
                    if path.ends_with(".properties") {
                        values.push((5, entries.clone()));
                    } else {
                        let (base, profile) = select_yaml_documents(text, &active_here);
                        values.push((4, base));
                        values.push((4, profile));
                    }
                }
            }
            ConfigFile::ConfigDir | ConfigFile::Build => {}
        }
        let contributed = values.iter().any(|(_, m)| !m.is_empty());
        for (rank, map) in values {
            selected.push((service.clone(), rank, path, map));
        }
        if matches!(kind, ConfigFile::Yaml | ConfigFile::Profile)
            && !path.ends_with(".properties")
            && !contributed
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
            out.clients.insert(service.clone());
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
    // Effective values, lowest precedence first (a later file's keys win).
    selected.sort_by(|a, b| a.1.cmp(&b.1).then(a.2.cmp(b.2)));
    for (service, _, _, map) in selected {
        out.properties.entry(service).or_default().extend(map);
    }
    (out, diagnostics)
}

const CONTEXT_PATH_KEY: &str = "server.servlet.context-path";

/// Build dependencies that make a service serve Spring Data REST (contract M9b D7).
const DATA_REST_DEPENDENCIES: &[&str] =
    &["spring-boot-starter-data-rest", "spring-data-rest-webmvc"];
/// The keys of Spring Data REST's base path, in Spring's relaxed binding forms.
const DATA_REST_BASE_PATH_KEYS: &[&str] =
    &["spring.data.rest.base-path", "spring.data.rest.basePath"];

/// Services whose build file names Spring Data REST (contract M9b D7), each with the base path
/// its read configuration sets (without slashes; `None` when unset or blank).
fn data_rest_services(
    config: &BTreeMap<&str, String>,
    spring: &SpringConfig,
    service_of: &dyn Fn(&str) -> String,
) -> BTreeMap<String, Option<String>> {
    config
        .iter()
        .filter(|(path, text)| {
            config_file(path) == Some(ConfigFile::Build)
                && DATA_REST_DEPENDENCIES.iter().any(|d| text.contains(d))
        })
        .map(|(path, _)| {
            let service = service_of(path);
            let base = spring.properties.get(&service).and_then(|p| {
                DATA_REST_BASE_PATH_KEYS
                    .iter()
                    .find_map(|k| p.get(*k))
                    .map(|v| v.trim().trim_matches('/').to_owned())
                    .filter(|v| !v.is_empty())
            });
            (service, base)
        })
        .collect()
}

/// Context paths a native Spring Cloud Config server sets for its clients (contract M9a D4).
/// The server is the one service whose effective configuration activates the `native` profile
/// and lists `search-locations`; each `classpath:` location is the server's
/// `src/main/resources/<dir>/` (`file:` and placeholder locations are not read). A config
/// client takes `server.servlet.context-path` from the unconditional documents of
/// `application.*` and then `<spring.application.name>.*` there, in location order
/// (`.properties` above YAML in one location), all above its local files. The highest value
/// found wins, and an empty one clears the context path (Spring's root context). Each
/// application is counted; several such servers apply nothing.
fn config_server_context_paths(
    config: &BTreeMap<&str, String>,
    spring: &SpringConfig,
    context_paths: &mut BTreeMap<String, String>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let servers: Vec<(&String, &String)> = spring
        .properties
        .iter()
        .filter_map(|(service, p)| {
            let native = p
                .get(ACTIVE_PROFILES_KEY)
                .is_some_and(|v| split_profiles(v).iter().any(|profile| profile == "native"));
            let locations = ["search-locations", "searchLocations"]
                .iter()
                .find_map(|k| p.get(&format!("spring.cloud.config.server.native.{k}")))?;
            native.then_some((service, locations))
        })
        .collect();
    if servers.len() > 1 {
        diagnostics.push(Diagnostic {
            kind: "config_server_ambiguous".into(),
            path: None,
            span: None,
            detail: json!({ "servers": servers.iter().map(|(s, _)| s).collect::<Vec<_>>() }),
        });
        return diagnostics;
    }
    let Some(&(server, locations)) = servers.first() else {
        return diagnostics;
    };
    let resources = if server == boundaries::ROOT_DIR {
        "src/main/resources".to_owned()
    } else {
        format!("{server}/src/main/resources")
    };
    let dirs: Vec<String> = locations
        .split(',')
        .map(str::trim)
        .filter(|l| !l.contains('{'))
        .filter_map(|l| l.strip_prefix("classpath:"))
        .map(|d| match d.trim_matches('/') {
            "" => format!("{resources}/"),
            d => format!("{resources}/{d}/"),
        })
        .collect();
    for client in &spring.clients {
        rb_frameworks::work::tick(1);
        let name = spring
            .properties
            .get(client)
            .and_then(|p| p.get("spring.application.name"));
        let Some(name) = name.filter(|_| client != server) else {
            continue;
        };
        // Lowest first, so the last value found wins.
        let mut found: Option<(&str, String)> = None;
        for stem in ["application", name.trim()] {
            for dir in &dirs {
                for ext in ["yml", "yaml", "properties"] {
                    let Some((&file, text)) =
                        config.get_key_value(format!("{dir}{stem}.{ext}").as_str())
                    else {
                        continue;
                    };
                    let entries: BTreeMap<String, String> = if ext == "properties" {
                        rb_frameworks::fold::properties_documents(text)
                            .into_iter()
                            .filter(|d| !rb_frameworks::fold::conditional_document(d))
                            .flatten()
                            .collect()
                    } else {
                        select_yaml_documents(text, &BTreeSet::new()).0
                    };
                    if let Some(value) = entries.get(CONTEXT_PATH_KEY) {
                        found = Some((file, value.trim().to_owned()));
                    }
                }
            }
        }
        let Some((file, value)) = found else {
            continue;
        };
        diagnostics.push(Diagnostic {
            kind: "config_server_context_path".into(),
            path: Some(file.to_owned()),
            span: None,
            detail: json!({ "service": client, "file": file, "context_path": value }),
        });
        // The highest value wins even when empty: Spring serves an empty context path at
        // the root, so it clears a lower one, as for local files (evaluator pass 1, E3).
        if value.is_empty() {
            context_paths.remove(client);
        } else {
            context_paths.insert(client.clone(), value);
        }
    }
    diagnostics
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
    let app_services = app_services(boundaries, &content.facts);
    let deployable = deployable_services(boundaries, &app_services);
    for p in boundaries.projects() {
        b.add_node(service_key(&p.dir), &p.name, service_attrs(p, &deployable))?;
    }
    let unassigned = service_key(boundaries::ROOT_DIR);

    let mut files_by_project: BTreeMap<&str, Vec<&FileEntry>> = BTreeMap::new();
    for f in &manifest.files {
        let file = NodeKey::new(NodeKind::File, f.path.clone());
        let name = f.path.rsplit('/').next().unwrap_or(&f.path);
        let mut attrs = file_attrs(f);
        if let Some(reason) = content.excluded.get(&f.path) {
            attrs.insert("excluded".into(), (*reason).into());
        }
        b.add_node(file.clone(), name, attrs)?;

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
    // `tsconfig*.json` scopes for partitioning `files_without_document` (contract M6e S10).
    let ts_scopes =
        rb_scip::TsScopes::build(content.manifests.iter().map(|(p, t)| (*p, t.as_str())));
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
                    &ts_scopes,
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
    diagnostics.extend(packages::unsupported_frameworks(&manifest_edges));
    // ORM mappings from every file first, so accesses can resolve entities across files.
    // Go package constants for `fmt.Sprintf` SQL (contract M9b D6), before any file's queries.
    let go_consts = sql::GoConsts::new(&content.facts);
    for (path, (_, facts)) in &content.facts {
        syntax::add_symbols(&mut b, path, facts, &keys[path])?;
        diagnostics.extend(sql::add_queries(
            &mut b,
            path,
            facts,
            &keys[path],
            &go_consts,
        )?);
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
    // Exact manifest resolution for bare specifiers (contract M6c S4), shared
    // by Tier B and RTA (F4).
    let aliases = rta::AliasMaps::build(&content.manifests);
    let index = index.with_aliases(aliases.clone());
    diagnostics.extend(resolve::add_edges(&mut b, &index, &exact_files)?);
    // Endpoints (contract M4a S4–S8): Spring's context path from each service's
    // configuration, then from a native config server's shared files (contract M9a D4).
    let service_of = |path: &str| {
        boundaries
            .project_for(path)
            .map_or_else(|| boundaries::ROOT_DIR.to_owned(), |p| p.dir.clone())
    };
    let (spring, config_diagnostics) = spring_config(&content.config, &service_of);
    diagnostics.extend(config_diagnostics);
    let mut context_paths: BTreeMap<String, String> = spring
        .properties
        .iter()
        .filter_map(|(service, p)| {
            let ctx = p.get(CONTEXT_PATH_KEY)?.trim();
            (!ctx.is_empty()).then(|| (service.clone(), ctx.to_owned()))
        })
        .collect();
    diagnostics.extend(config_server_context_paths(
        &content.config,
        &spring,
        &mut context_paths,
    ));
    let has_project = |path: &str| boundaries.project_for(path).is_some();
    // Spring DI (contract M4b S4–S8): `INJECTS` from injection points to beans.
    // (`deployable` computed above: manifest-detected plus `@SpringBootApplication` modules.)
    let depends = di::package_dependencies(&manifest_edges);
    let mut resolution = di::resolve(&di::Inputs {
        files: &content.facts,
        keys: &keys,
        index: &index,
        service_of: &service_of,
        deployable: &deployable,
        depends: &depends,
        unparsed: &content.unparsed,
        manifests: &content.manifests,
        xml: &content.mappers,
    });
    let fragments = resolution.fragments.clone();
    let points = std::mem::take(&mut resolution.points);
    let spring_data = resolution.spring_data.clone();
    diagnostics.extend(di::add_injects(&mut b, resolution)?);
    // Inherited Spring Data calls (contract M6c S7): allow-listed member
    // names on repository-typed receivers link to the interface node.
    diagnostics.extend(di::add_inherited_calls(
        &mut b,
        &index,
        &content.facts,
        &keys,
        &spring_data,
        &exact_files,
    )?);
    // RTA (contract M4b S9, D4): after every `IMPLEMENTS` producer (SCIP, Tier B).
    let considered =
        rta::implementing_types(&b.edge_sources(EdgeKind::Implements), &content.facts, &keys);
    let proof = rta::prove(
        &rta::Inputs {
            files: &content.facts,
            service_of: &service_of,
            deployable: &deployable,
            unparsed: &content.unparsed,
            words: &content.words,
            fragments: &fragments,
            aliases: &aliases,
        },
        &considered,
    );
    diagnostics.push(rta::apply(&mut b, &proof, &content.facts, &keys));
    let data_rest = data_rest_services(&content.config, &spring, &service_of);
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
            data_rest: &data_rest,
            specs: &content.specs,
            has_project: &has_project,
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
        points: &points,
    });
    let services: Vec<stitch::ServiceRef> = boundaries
        .projects()
        .iter()
        .map(|p| stitch::ServiceRef {
            key: p.dir.clone(),
            name: p.name.clone(),
        })
        .collect();
    // Gateway rewrites (contract M6g S3): nginx + Spring Gateway rules become prefix strips
    // scoped to the target service. Unread blocks are counted per file, never edges.
    let mut gateway_rules: Vec<(String, String)> = Vec::new();
    for (path, text) in &content.gateways {
        let file = if path.ends_with("nginx.conf") {
            rb_frameworks::gateway::nginx_gateway(text)
        } else {
            rb_frameworks::gateway::spring_gateway(text)
        };
        gateway_rules.extend(file.rules.into_iter().map(|r| (r.prefix, r.target)));
        if file.unread > 0 {
            diagnostics.push(Diagnostic {
                kind: "gateway_unread".into(),
                path: Some((*path).to_owned()),
                span: None,
                detail: json!({ "unread": file.unread }),
            });
        }
    }
    let rewrites = stitch::Stitcher::resolve_rewrites(&gateway_rules, &services);
    // Host names (contract M6g S4): Compose/k8s names owned by path alias their service;
    // root-level names match services by key basename or name; `rb.toml` maps explicitly.
    // Names matching nothing are counted, never edges; duplicates stay ambiguous.
    let mut host_aliases: Vec<(String, String)> = Vec::new();
    for (dir, names) in &content.rb_hosts {
        match boundaries.projects().iter().find(|p| &p.dir == dir) {
            Some(p) => host_aliases.extend(names.iter().map(|n| (n.clone(), p.dir.clone()))),
            None => diagnostics.push(Diagnostic {
                kind: "hosts_unmatched".into(),
                path: Some("rb.toml".to_owned()),
                span: None,
                detail: json!({ "service": dir, "names": names }),
            }),
        }
    }
    for (path, text) in &content.host_files {
        let mut names = Vec::new();
        if rb_frameworks::hosts::is_compose_input(path) {
            names.extend(rb_frameworks::hosts::compose_names(text));
        }
        names.extend(rb_frameworks::hosts::k8s_service_names(text));
        if names.is_empty() {
            continue;
        }
        match boundaries.project_for(path) {
            Some(p) => {
                // Root-matches win over ownership (a manifest for another service is never
                // stolen); otherwise-novel names alias the owning service.
                for name in &names {
                    let mut matched = false;
                    for service in &services {
                        let base = service.key.rsplit('/').next().unwrap_or(&service.key);
                        if base.eq_ignore_ascii_case(name)
                            || service.name.eq_ignore_ascii_case(name)
                        {
                            host_aliases.push((name.clone(), service.key.clone()));
                            matched = true;
                        }
                    }
                    if !matched {
                        host_aliases.push((name.clone(), p.dir.clone()));
                    }
                }
            }
            None => {
                let mut matched = false;
                for service in &services {
                    let base = service.key.rsplit('/').next().unwrap_or(&service.key);
                    for name in &names {
                        if base.eq_ignore_ascii_case(name)
                            || service.name.eq_ignore_ascii_case(name)
                        {
                            host_aliases.push((name.clone(), service.key.clone()));
                            matched = true;
                        }
                    }
                }
                if !matched {
                    diagnostics.push(Diagnostic {
                        kind: "hosts_unmatched".into(),
                        path: Some((*path).to_owned()),
                        span: None,
                        detail: json!({ "names": names }),
                    });
                }
            }
        }
    }
    if rewrites.is_empty() && host_aliases.is_empty() {
        diagnostics.extend(stitch::add_consumes(&mut b, &sites, &endpoints, &services)?);
    } else {
        diagnostics.extend(stitch::add_consumes_with(
            &mut b,
            &sites,
            &endpoints,
            &services,
            &rewrites,
            &host_aliases,
        )?);
    }
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
        diagnostics.extend(sql::add_sql_file(&mut b, path, text)?);
    }
    for (path, text) in &content.mappers {
        diagnostics.extend(sql::add_mapper_queries(&mut b, path, text)?);
    }
    // Message topics (contract M6g S2): publish/subscribe edges from topic literals.
    diagnostics.extend(topics::add_topics(
        &mut b,
        &topics::Inputs {
            files: &content.facts,
            service_of: &service_of,
            properties: &spring.properties,
        },
    )?);
    // Entry kinds and orphan eligibility (contract M4b S11, S12), after every edge producer.
    diagnostics.push(liveness::apply(
        &mut b,
        &liveness::Inputs {
            files: &content.facts,
            keys: &keys,
            index: &index,
            service_of: &service_of,
            deployable: &deployable,
            words: &content.words,
            fragments: &fragments,
        },
    ));
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
    // Contract M6a D3, D5, D7: only for a grouped workspace root or a job with link targets,
    // so other jobs dump as before.
    if p.workspace_members > 0 {
        detail["workspace_members"] = p.workspace_members.into();
    }
    if p.link_targets > 0 {
        detail["link_targets"] = p.link_targets.into();
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
        // Contract M6a D3: written only when non-zero, so a project without Lombok dumps as
        // before M6a. Contract M6e S10 likewise: a project with no out-of-scope file and no
        // unread tsconfig pattern dumps as before M6e.
        for (key, n) in [
            ("lombok_removed", c.lombok_removed),
            ("documents_lombok", c.documents_lombok),
            ("documents_outside_root", c.documents_outside_root),
            (
                "files_without_document_out_of_scope",
                c.files_without_document_out_of_scope,
            ),
            ("tsconfig_unread", c.tsconfig_unread),
        ] {
            if n > 0 {
                detail["coverage"][key] = n.into();
            }
        }
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

/// Services holding a main-source `@SpringBootApplication` class: deployable
/// even when their manifests say otherwise (contract M6c S3) — a Maven module
/// inheriting its build from a parent pom, Gradle without the plugin text.
/// Test sources never count: a test application class runs nothing in the
/// deployed context. The carrier must be runnable itself (concrete and
/// top-level or static, as for bean classes): an abstract or
/// non-static-nested carrier never flips (evaluator pass 1, C5).
fn app_services(
    boundaries: &Boundaries,
    facts: &BTreeMap<&str, (rb_syntax::Language, &rb_syntax::FileFacts)>,
) -> BTreeSet<String> {
    facts
        .iter()
        .filter(|(path, (language, facts))| {
            rb_frameworks::work::tick(1);
            *language == rb_syntax::Language::Java
                && !rb_model::is_test_path(path)
                && facts.definitions.iter().any(|d| {
                    d.kind == rb_syntax::DefKind::Class
                        && !d.modifiers.iter().any(|m| m == "abstract")
                        && d.parent.is_none_or(|p| {
                            facts.definitions[p].kind == rb_syntax::DefKind::Interface
                                || d.modifiers.iter().any(|m| m == "static")
                        })
                        && d.decorations.iter().any(|deco| {
                            rb_frameworks::routes::is_annotation(
                                facts,
                                deco,
                                "org.springframework.boot.autoconfigure",
                                "SpringBootApplication",
                            )
                        })
                })
        })
        .filter_map(|(path, _)| {
            rb_frameworks::work::tick(1);
            boundaries.project_for(path).map(|p| p.dir.clone())
        })
        .collect()
}

/// Services runnable in production: manifest-detected deployables plus modules
/// holding a main-source `@SpringBootApplication` class (contract M6c S3).
/// One predicate feeds service roles, the DI classpath and RTA alike, so a
/// module is never `role=service` without being deployable, or vice versa.
fn deployable_services(
    boundaries: &Boundaries,
    app_services: &BTreeSet<String>,
) -> BTreeSet<String> {
    boundaries
        .projects()
        .iter()
        .filter(|p| p.deployable || app_services.contains(&p.dir))
        .map(|p| p.dir.clone())
        .collect()
}

fn service_key(dir: &str) -> NodeKey {
    NodeKey::new(NodeKind::Service, dir)
}

fn service_attrs(p: &Project, deployable: &BTreeSet<String>) -> Attrs {
    let role = if deployable.contains(&p.dir) {
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

    /// Contract M7e S2: excluded files keep their `File` node (marked with the reason) and
    /// nothing else — no symbols, no project, no parse — and each reason is counted once.
    #[test]
    fn excluded_files_keep_file_nodes_and_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let files = [
            ("go.mod", "module example.com/app\n\ngo 1.21\n"),
            (
                "main.go",
                "package main\n\nfunc main() { helper() }\n\nfunc helper() {}\n",
            ),
            ("vendor/modules.txt", "# github.com/x/lib v1.0.0\n"),
            (
                "vendor/github.com/x/lib/go.mod",
                "module github.com/x/lib\n",
            ),
            (
                "vendor/github.com/x/lib/lib.go",
                "package lib\n\nfunc Vendored() {}\n",
            ),
            (
                "web/node_modules/pkg/index.js",
                "function nm() {}\nmodule.exports = nm;\n",
            ),
        ];
        for (path, text) in files {
            let p = dir.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "rb")
                .env("GIT_AUTHOR_EMAIL", "rb@example.invalid")
                .env("GIT_COMMITTER_NAME", "rb")
                .env("GIT_COMMITTER_EMAIL", "rb@example.invalid")
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["add", "-A"]);
        git(&["commit", "-q", "-m", "init"]);
        let indexed = index_commit(
            dir,
            "HEAD",
            &mut NoCache,
            &ScipOptions::disabled(),
            &mut NoScipCache,
        )
        .unwrap();
        let g = &indexed.graph;
        let excluded: Vec<(String, String)> = g
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::File)
            .filter_map(|n| {
                n.attrs
                    .get("excluded")
                    .map(|r| (n.key.clone(), r.as_str().unwrap().to_owned()))
            })
            .collect();
        assert_eq!(
            excluded,
            [
                (
                    "vendor/github.com/x/lib/go.mod".to_owned(),
                    "go_vendor".to_owned()
                ),
                (
                    "vendor/github.com/x/lib/lib.go".to_owned(),
                    "go_vendor".to_owned()
                ),
                ("vendor/modules.txt".to_owned(), "go_vendor".to_owned()),
                (
                    "web/node_modules/pkg/index.js".to_owned(),
                    "node_modules".to_owned()
                ),
            ]
        );
        assert_eq!(
            g.nodes()
                .iter()
                .filter(|n| n.kind == NodeKind::File)
                .count(),
            6
        );
        let symbols: Vec<&str> = g
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::Symbol)
            .map(|n| n.name.as_str())
            .collect();
        assert!(symbols.contains(&"helper"), "{symbols:?}");
        assert!(
            !symbols.contains(&"Vendored") && !symbols.contains(&"nm"),
            "{symbols:?}"
        );
        let services: Vec<&str> = g
            .nodes()
            .iter()
            .filter(|n| n.kind == NodeKind::Service)
            .map(|n| n.key.as_str())
            .collect();
        assert_eq!(services, ["."], "{services:?}");
        // One per reason; their order is `sort_diagnostics`' canonical one.
        let counted: BTreeSet<String> = indexed
            .diagnostics
            .iter()
            .filter(|d| d.kind == "excluded")
            .map(|d| d.detail.to_string())
            .collect();
        assert_eq!(
            counted,
            BTreeSet::from([
                r#"{"files":3,"reason":"go_vendor"}"#.to_owned(),
                r#"{"files":1,"reason":"node_modules"}"#.to_owned(),
            ])
        );
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
                ..RawScan::default()
            }),
            ["heuristic base=unverified discovery=-"]
        );
        assert_eq!(
            consumes(RawScan {
                hooks: BTreeSet::new(),
                discovery: true,
                ..RawScan::default()
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
                workspace_members: 0,
                link_targets: 0,
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
            workspace_members: 0,
            link_targets: 0,
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

    /// Contract M6c S3: a module holding a main-source `@SpringBootApplication`
    /// class is deployable even when its manifests say otherwise (a Maven
    /// module inheriting its build, Gradle without the plugin text). A module
    /// without one, and an app class in test sources, stay library.
    #[test]
    fn spring_boot_application_class_makes_a_library_deployable() {
        let app = rb_syntax::extract(
            rb_syntax::Language::Java,
            b"package com.acme;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\n@SpringBootApplication public class App {}\n",
        )
        .unwrap();
        let plain = rb_syntax::extract(
            rb_syntax::Language::Java,
            b"package com.acme;\npublic class Plain {}\n",
        )
        .unwrap();
        let test_app = rb_syntax::extract(
            rb_syntax::Language::Java,
            b"package com.acme;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\n@SpringBootApplication public class TestApp {}\n",
        )
        .unwrap();
        let ts_app = rb_syntax::extract(
            rb_syntax::Language::TypeScript,
            b"import { SpringBootApplication } from 'org.springframework.boot.autoconfigure';\n@SpringBootApplication class App {}\n",
        )
        .unwrap();
        let meth_app = rb_syntax::extract(
            rb_syntax::Language::Java,
            b"package com.acme;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\nclass Holder {\n@SpringBootApplication void m() {}\n}\n",
        )
        .unwrap();
        // Evaluator pass 1, C5: an abstract or non-static-nested carrier can
        // never be the application — no flip.
        let abstract_app = rb_syntax::extract(
            rb_syntax::Language::Java,
            b"package com.acme;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\n@SpringBootApplication public abstract class AbsApp {}\n",
        )
        .unwrap();
        // An interface carrier is not a class either (discriminates the kind
        // gate from the parent rule: top-level and concrete, only the kind
        // blocks it).
        let iface_app = rb_syntax::extract(
            rb_syntax::Language::Java,
            b"package com.acme;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\n@SpringBootApplication public interface AppIfc {}\n",
        )
        .unwrap();
        let nested_app = rb_syntax::extract(
            rb_syntax::Language::Java,
            b"package com.acme;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\nclass Outer {\n@SpringBootApplication class Inner {}\n}\n",
        )
        .unwrap();
        let manifest = Manifest {
            commit_sha: "c".repeat(40),
            files: [
                "svc/pom.xml",
                "svc/src/main/java/com/acme/App.java",
                "other/pom.xml",
                "other/src/main/java/com/acme/Plain.java",
                "testonly/pom.xml",
                "testonly/src/test/java/com/acme/TestApp.java",
                "tslib/package.json",
                "tslib/src/app.ts",
                "meth/pom.xml",
                "meth/src/main/java/com/acme/Holder.java",
                "abs/pom.xml",
                "abs/src/main/java/com/acme/AbsApp.java",
                "nest/pom.xml",
                "nest/src/main/java/com/acme/Outer.java",
                "iface/pom.xml",
                "iface/src/main/java/com/acme/AppIfc.java",
            ]
            .into_iter()
            .map(entry)
            .collect(),
            skipped: vec![],
        };
        let library = |dir: &str, manifest: &str| {
            let mut p = project(dir, manifest);
            p.deployable = false;
            p
        };
        let boundaries = Boundaries::new(
            vec![
                library("svc", "svc/pom.xml"),
                library("other", "other/pom.xml"),
                library("testonly", "testonly/pom.xml"),
                library("tslib", "tslib/package.json"),
                library("meth", "meth/pom.xml"),
                library("abs", "abs/pom.xml"),
                library("nest", "nest/pom.xml"),
                library("iface", "iface/pom.xml"),
            ],
            vec![],
        );
        let content = Content {
            facts: BTreeMap::from([
                (
                    "svc/src/main/java/com/acme/App.java",
                    (rb_syntax::Language::Java, &app),
                ),
                (
                    "other/src/main/java/com/acme/Plain.java",
                    (rb_syntax::Language::Java, &plain),
                ),
                (
                    "testonly/src/test/java/com/acme/TestApp.java",
                    (rb_syntax::Language::Java, &test_app),
                ),
                (
                    "tslib/src/app.ts",
                    (rb_syntax::Language::TypeScript, &ts_app),
                ),
                (
                    "meth/src/main/java/com/acme/Holder.java",
                    (rb_syntax::Language::Java, &meth_app),
                ),
                (
                    "abs/src/main/java/com/acme/AbsApp.java",
                    (rb_syntax::Language::Java, &abstract_app),
                ),
                (
                    "nest/src/main/java/com/acme/Outer.java",
                    (rb_syntax::Language::Java, &nested_app),
                ),
                (
                    "iface/src/main/java/com/acme/AppIfc.java",
                    (rb_syntax::Language::Java, &iface_app),
                ),
            ]),
            ..Content::default()
        };
        let (g, _) = assemble(&manifest, None, &boundaries, &content).unwrap();
        let role = |key: &str| {
            g.nodes().iter().find(|n| n.key == key).unwrap().attrs["role"]
                .as_str()
                .unwrap()
                .to_owned()
        };
        assert_eq!(role("svc"), "service");
        assert_eq!(role("other"), "library");
        assert_eq!(role("testonly"), "library");
        assert_eq!(role("tslib"), "library");
        assert_eq!(role("meth"), "library");
        assert_eq!(role("abs"), "library");
        assert_eq!(role("nest"), "library");
        assert_eq!(role("iface"), "library");
    }

    /// Contract M6c S3, invariant 9: one app class per module costs linear
    /// work — 4× the modules makes at most 5× the counted steps. Counted, not
    /// timed, so machine load cannot fail it.
    #[test]
    fn app_services_grows_linearly() {
        fn work(n: usize) -> u64 {
            let srcs: Vec<String> = (0..n)
                .map(|i| format!("package m{i};\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\n@SpringBootApplication public class App{i} {{}}\n"))
                .collect();
            let facts: Vec<rb_syntax::FileFacts> = srcs
                .iter()
                .map(|s| rb_syntax::extract(rb_syntax::Language::Java, s.as_bytes()).unwrap())
                .collect();
            let paths: Vec<String> = (0..n)
                .map(|i| format!("m{i}/src/main/java/m{i}/App{i}.java"))
                .collect();
            let map: BTreeMap<&str, (rb_syntax::Language, &rb_syntax::FileFacts)> = paths
                .iter()
                .zip(&facts)
                .map(|(p, f)| (p.as_str(), (rb_syntax::Language::Java, f)))
                .collect();
            let boundaries = Boundaries::new(
                (0..n)
                    .map(|i| {
                        let mut p = project(&format!("m{i}"), &format!("m{i}/pom.xml"));
                        p.deployable = false;
                        p
                    })
                    .collect(),
                vec![],
            );
            rb_frameworks::work::take();
            let found = app_services(&boundaries, &map);
            assert_eq!(found.len(), n);
            rb_frameworks::work::take()
        }
        let (small, large) = (work(100), work(400));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
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

    /// Contract M6a D3, D7: a job's exported link targets (top level, any status) and the
    /// documents it ignored outside its root (coverage) are written only when non-zero.
    #[test]
    fn link_targets_and_documents_outside_the_root_are_written_only_when_non_zero() {
        let mut project = ok_project("tools/e2e", "scip-typescript", vec![]);
        let plain = scip_status(&project, Some(&scip::Coverage::default()));
        assert!(
            plain.detail.get("link_targets").is_none(),
            "{}",
            plain.detail
        );
        assert!(
            plain.detail["coverage"]
                .get("documents_outside_root")
                .is_none()
        );
        project.link_targets = 2;
        let outside = scip::Coverage {
            documents_outside_root: 3,
            ..Default::default()
        };
        let d = scip_status(&project, Some(&outside));
        assert_eq!(d.detail["link_targets"], 2);
        assert_eq!(d.detail["coverage"]["documents_outside_root"], 3);
    }

    /// Contract M6a D3, D5: a grouped workspace root's member count is written to its
    /// `scip_status` (whatever its status, so a failed fetch shows it too) only when non-zero.
    #[test]
    fn workspace_members_are_written_only_for_a_grouped_root() {
        let mut project = ok_project("", "scip-typescript", vec![]);
        let plain = scip_status(&project, None);
        assert!(
            plain.detail.get("workspace_members").is_none(),
            "{}",
            plain.detail
        );
        project.workspace_members = 2;
        project.status = rb_scip::RunStatus::Failed(rb_scip::Phase::Fetch);
        let grouped = scip_status(&project, None);
        assert_eq!(grouped.detail["workspace_members"], 2);
    }

    /// Contract M6a D3, S3: the Lombok counters are written to the coverage only when
    /// non-zero, so a project without Lombok dumps as before; the other keys are unchanged.
    #[test]
    fn lombok_counters_are_written_only_when_non_zero() {
        let project = ok_project("", "scip-java", vec![]);
        let plain = scip::Coverage::default();
        let d = scip_status(&project, Some(&plain));
        let keys: Vec<&String> = d.detail["coverage"].as_object().unwrap().keys().collect();
        assert!(!keys.iter().any(|k| k.contains("lombok")), "{keys:?}");
        let lombok = scip::Coverage {
            lombok_removed: 27,
            documents_lombok: 4,
            ..Default::default()
        };
        let d = scip_status(&project, Some(&lombok));
        assert_eq!(d.detail["coverage"]["lombok_removed"], 27);
        assert_eq!(d.detail["coverage"]["documents_lombok"], 4);
        let mut without = d.detail["coverage"].as_object().unwrap().clone();
        without.remove("lombok_removed");
        without.remove("documents_lombok");
        assert_eq!(
            serde_json::Value::Object(without),
            scip_status(&project, Some(&plain)).detail["coverage"]
        );
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

    /// Contract M6g S5: `application.yml` multi-document profiles are evaluated. Base
    /// documents always apply; `on-profile` (and legacy `spring.profiles`) documents apply
    /// when their profile is active. `bootstrap*` is lowest, `.properties` beats `.yml` in
    /// the same tier, profile files beat inline documents, and an unreadable (tabbed) YAML
    /// file keeps today's `.properties` reading.
    #[test]
    fn spring_yaml_profiles_evaluate_in_documented_precedence() {
        let res = "src/main/resources";
        let files: Vec<(String, &str)> = vec![
            // Active profile `prod`, set by the properties file (beats the YAML's `dev`).
            (
                format!("a/{res}/application.properties"),
                "spring.profiles.active=prod\nusers.url=http://base\nshared=props\n",
            ),
            (
                format!("a/{res}/application.yml"),
                "spring:\n  profiles:\n    active: dev\nusers:\n  url: http://yaml-base\nshared: yaml\n---\nspring:\n  config:\n    activate:\n      on-profile: prod\nusers:\n  url: http://yaml-prod\n---\nspring:\n  config:\n    activate:\n      on-profile: staging\nusers:\n  url: http://yaml-staging\n",
            ),
            // A legacy `spring.profiles` document for the active profile.
            (
                format!("b/{res}/application.properties"),
                "spring.profiles.active=prod\n",
            ),
            (
                format!("b/{res}/application.yml"),
                "users:\n  url: http://b-base\n---\nspring:\n  profiles: prod\nusers:\n  url: http://b-prod\n",
            ),
            (
                format!("b/{res}/application-prod.properties"),
                "users.url=http://b-file\n",
            ),
            // Bootstrap is lowest: loses to `application.properties`.
            (
                format!("c/{res}/bootstrap.properties"),
                "users.url=http://boot-props\n",
            ),
            (
                format!("c/{res}/application.properties"),
                "users.url=http://c-app\n",
            ),
            // A tabbed YAML file is unreadable: today's `.properties` reading is kept.
            (
                format!("d/{res}/application.properties"),
                "spring.profiles.active=prod\nusers.url=http://d-base\n",
            ),
            (
                format!("d/{res}/application.yml"),
                "users:\n\turl: http://d-yaml\n---\nspring:\n  config:\n    activate:\n      on-profile: prod\nusers:\n  url: http://d-prod\n",
            ),
            // An inactive profile file contributes nothing.
            (
                format!("e/{res}/application.properties"),
                "users.url=http://e-base\n",
            ),
            (
                format!("e/{res}/application-staging.properties"),
                "users.url=http://e-staging\n",
            ),
            // A context path from the active YAML document flows into the properties.
            (
                format!("f/{res}/application.properties"),
                "spring.profiles.active=prod\n",
            ),
            (
                format!("f/{res}/application.yml"),
                "server:\n  servlet:\n    context-path: /base\n---\nspring:\n  config:\n    activate:\n      on-profile: prod\nserver:\n  servlet:\n    context-path: /prod\n",
            ),
        ];
        let config: BTreeMap<&str, String> = files
            .iter()
            .map(|(p, t)| (p.as_str(), (*t).to_owned()))
            .collect();
        let service_of = |p: &str| p.split('/').next().unwrap().to_owned();
        let (spring, _) = spring_config(&config, &service_of);
        // `prod` (properties) beats `dev` (YAML); the staging document never applies.
        assert_eq!(spring.properties["a"]["users.url"], "http://yaml-prod");
        assert_eq!(spring.properties["a"]["shared"], "props");
        // The profile file beats the inline profile document.
        assert_eq!(spring.properties["b"]["users.url"], "http://b-file");
        // Bootstrap loses.
        assert_eq!(spring.properties["c"]["users.url"], "http://c-app");
        // Tabbed YAML contributes nothing.
        assert_eq!(spring.properties["d"]["users.url"], "http://d-base");
        // Inactive profile files contribute nothing.
        assert_eq!(spring.properties["e"]["users.url"], "http://e-base");
        // The active profile's context path flows through.
        assert_eq!(
            spring.properties["f"]["server.servlet.context-path"],
            "/prod"
        );
    }

    /// Contract M6g S5: with no active profile only base documents apply, and a read YAML
    /// file setting a context path loses its `spring_config_unread` diagnostic.
    #[test]
    fn spring_yaml_without_an_active_profile_reads_base_documents_only() {
        let res = "src/main/resources";
        let files: Vec<(String, &str)> = vec![
            (
                format!("a/{res}/application.yml"),
                "server:\n  servlet:\n    context-path: /base\nusers:\n  url: http://base\n---\nspring:\n  config:\n    activate:\n      on-profile: prod\nusers:\n  url: http://prod\nserver:\n  servlet:\n    context-path: /prod\n",
            ),
            (
                format!("a/{res}/application-prod.properties"),
                "users.url=http://file\n",
            ),
        ];
        let config: BTreeMap<&str, String> = files
            .iter()
            .map(|(p, t)| (p.as_str(), (*t).to_owned()))
            .collect();
        let service_of = |p: &str| p.split('/').next().unwrap().to_owned();
        let (spring, diagnostics) = spring_config(&config, &service_of);
        assert_eq!(spring.properties["a"]["users.url"], "http://base");
        assert_eq!(
            spring.properties["a"]["server.servlet.context-path"],
            "/base"
        );
        assert!(
            diagnostics.iter().all(|d| d.kind != "spring_config_unread"),
            "a read file's context path is applied, not counted unread"
        );
    }

    /// Contract M6g S9, invariants 5–6: selecting 4× the configuration files costs at most
    /// 5× the operations.
    #[test]
    fn spring_config_selection_grows_linearly() {
        fn work(n: usize) -> u64 {
            let res = "src/main/resources";
            let mut files: Vec<(String, String)> = Vec::new();
            for i in 0..n {
                files.push((
                    format!("s{i}/{res}/application.properties"),
                    "spring.profiles.active=prod\n".to_owned(),
                ));
                files.push((
                    format!("s{i}/{res}/application.yml"),
                    "k: base\n---\nspring:\n  config:\n    activate:\n      on-profile: prod\nk: prod\n"
                        .to_owned(),
                ));
                files.push((
                    format!("s{i}/{res}/bootstrap.properties"),
                    "k: boot\n".to_owned(),
                ));
            }
            let config: BTreeMap<&str, String> =
                files.iter().map(|(p, t)| (p.as_str(), t.clone())).collect();
            let service_of = |p: &str| p.split('/').next().unwrap().to_owned();
            rb_frameworks::work::take();
            let (spring, _) = spring_config(&config, &service_of);
            assert_eq!(spring.properties.len(), n);
            rb_frameworks::work::take()
        }
        let (small, large) = (work(25), work(100));
        assert!(small > 0 && large <= small * 5, "{small} -> {large}");
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

    /// Contract M9b D7 (M9b pass 1, E1): a service serves Spring Data REST only when its Maven
    /// or Gradle build names `spring-boot-starter-data-rest` or `spring-data-rest-webmvc`, and
    /// its base path is read from `application.properties` or `application.yml` in either
    /// relaxed form (`base-path`, `basePath`), without slashes; a blank one is none.
    #[test]
    fn data_rest_services_need_the_dependency_and_read_the_base_path() {
        let starter = "<dependency><groupId>org.springframework.boot</groupId><artifactId>spring-boot-starter-data-rest</artifactId></dependency>";
        let config: BTreeMap<&str, String> = [
            ("prop/pom.xml", format!("<project>{starter}</project>")),
            (
                "prop/src/main/resources/application.properties",
                "spring.data.rest.basePath=/rest/\n".to_owned(),
            ),
            (
                "yml/build.gradle",
                "dependencies {\n    implementation 'org.springframework.boot:spring-boot-starter-data-rest'\n}\n".to_owned(),
            ),
            (
                "yml/src/main/resources/application.yml",
                "spring:\n  data:\n    rest:\n      base-path: /api/\n".to_owned(),
            ),
            (
                "kts/build.gradle.kts",
                "dependencies {\n    implementation(\"org.springframework.data:spring-data-rest-webmvc\")\n}\n".to_owned(),
            ),
            (
                "blank/pom.xml",
                format!("<project>{starter}</project>"),
            ),
            (
                "blank/src/main/resources/application.properties",
                "spring.data.rest.base-path=  /  \n".to_owned(),
            ),
            // A base path without the dependency serves nothing.
            (
                "web/pom.xml",
                "<project><artifactId>spring-boot-starter-web</artifactId></project>".to_owned(),
            ),
            (
                "web/src/main/resources/application.properties",
                "spring.data.rest.base-path=/api\n".to_owned(),
            ),
        ]
        .into();
        let service_of = |p: &str| p.split('/').next().unwrap().to_owned();
        let (spring, _) = spring_config(&config, &service_of);
        assert_eq!(
            data_rest_services(&config, &spring, &service_of),
            BTreeMap::from([
                ("blank".to_owned(), None),
                ("kts".to_owned(), None),
                ("prop".to_owned(), Some("rest".to_owned())),
                ("yml".to_owned(), Some("api".to_owned())),
            ])
        );
    }

    /// Contract M9b N1 (M9a pass 2): a config server's context-path value is trimmed, so a
    /// padded quoted value is its path and a whitespace-only one is empty and clears a local
    /// context path, as for local files.
    #[test]
    fn config_server_context_path_values_are_trimmed() {
        let shared = "config/src/main/resources/shared";
        let config: BTreeMap<&str, String> = [
            (
                "config/src/main/resources/shared/acct-service.yml",
                "server:\n  servlet:\n    context-path: \" /acct \"\n".to_owned(),
            ),
            (
                "config/src/main/resources/shared/blank-service.yml",
                "server:\n  servlet:\n    context-path: \"   \"\n".to_owned(),
            ),
        ]
        .into();
        let props = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect()
        };
        let spring = SpringConfig {
            properties: [
                (
                    "config".to_owned(),
                    props(&[
                        (ACTIVE_PROFILES_KEY, "native"),
                        (
                            "spring.cloud.config.server.native.search-locations",
                            "classpath:/shared/",
                        ),
                    ]),
                ),
                (
                    "acct".to_owned(),
                    props(&[("spring.application.name", "acct-service")]),
                ),
                (
                    "blank".to_owned(),
                    props(&[("spring.application.name", "blank-service")]),
                ),
            ]
            .into(),
            clients: ["acct".to_owned(), "blank".to_owned()].into(),
            ..SpringConfig::default()
        };
        assert!(config.keys().all(|k| k.starts_with(shared)));
        let mut context_paths: BTreeMap<String, String> =
            [("blank".to_owned(), "/local".to_owned())].into();
        config_server_context_paths(&config, &spring, &mut context_paths);
        assert_eq!(
            context_paths,
            BTreeMap::from([("acct".to_owned(), "/acct".to_owned())])
        );
    }

    /// Contract M9a D4 (bench B14): a native config server's shared files set its clients'
    /// context paths, matched by `spring.application.name`, above local files and with
    /// `<name>.*` above `application.*`; the prefixed path is `synthetic` and counted.
    #[test]
    fn config_server_shared_files_set_client_context_paths() {
        let controller = "package p;\nimport org.springframework.web.bind.annotation.*;\n@RestController\nclass C { @GetMapping(\"/x\") String x() { return \"\"; } }\n";
        let facts = rb_syntax::extract(rb_syntax::Language::Java, controller.as_bytes()).unwrap();
        // Each top directory is a service with a `pom.xml` and the controller (files under
        // `src/` belong to a project at the repository root); returns its `EXPOSES` targets
        // with their certainty and the `config_server_*` diagnostics.
        let run = |config: &[(&str, &str)]| -> (Vec<String>, Vec<String>) {
            let dirs: BTreeSet<&str> = config
                .iter()
                .map(|(p, _)| match p.split('/').next().unwrap() {
                    "src" => boundaries::ROOT_DIR,
                    d => d,
                })
                .collect();
            let under = |d: &str, rest: &str| {
                if d == boundaries::ROOT_DIR {
                    rest.to_owned()
                } else {
                    format!("{d}/{rest}")
                }
            };
            let poms: Vec<String> = dirs.iter().map(|d| under(d, "pom.xml")).collect();
            let java: Vec<String> = dirs
                .iter()
                .map(|d| under(d, "src/main/java/p/C.java"))
                .collect();
            let paths = config.iter().map(|(p, _)| *p);
            let manifest = Manifest {
                commit_sha: "c".repeat(40),
                files: paths
                    .clone()
                    .chain(poms.iter().map(String::as_str))
                    .chain(java.iter().map(String::as_str))
                    .map(entry)
                    .collect(),
                skipped: vec![],
            };
            let projects = dirs
                .iter()
                .zip(&poms)
                .map(|(d, pom)| project(d, pom))
                .collect();
            let boundaries = Boundaries::new(projects, vec![]);
            let content = Content {
                facts: java
                    .iter()
                    .map(|p| (p.as_str(), (rb_syntax::Language::Java, &facts)))
                    .collect(),
                config: config
                    .iter()
                    .map(|(p, t)| (*p, (*t).to_owned()))
                    .chain(poms.iter().map(|p| (p.as_str(), "<project/>\n".to_owned())))
                    .collect(),
                ..Content::default()
            };
            let (g, diagnostics) = assemble(&manifest, None, &boundaries, &content).unwrap();
            let key = |id: i64| g.nodes().iter().find(|n| n.id == id).unwrap().key.clone();
            let exposes = g
                .edges()
                .iter()
                .filter(|e| e.kind == EdgeKind::Exposes)
                .map(|e| format!("{} [{}]", key(e.dst), e.certainty.as_str()))
                .collect();
            let counted = diagnostics
                .iter()
                .filter(|d| d.kind.starts_with("config_server_"))
                .map(|d| format!("{} {}", d.kind, d.detail))
                .collect();
            (exposes, counted)
        };
        let res = "src/main/resources";
        let client = |name: &str| {
            format!(
                "spring:\n  application:\n    name: {name}\n  cloud:\n    config:\n      uri: http://config:8888\n"
            )
        };
        // The server is itself a named config client (`fail-fast`), so only the "other than
        // the server" rule keeps the shared `application.yml` off its route. Its locations
        // carry a trailing `/`, padding, and a second `classpath:` location last.
        let native = "spring:\n  application:\n    name: config-service\n  cloud:\n    config:\n      fail-fast: true\n      server:\n        native:\n          search-locations: classpath:/shared/ , file:/extra,  classpath:/{application}, classpath:common\n  profiles:\n     active: native\n";
        let names = [
            "acct", "alpha", "beta", "eps", "gamma", "iota", "notes", "stats", "theta", "zeta",
        ];
        let clients: Vec<String> = names
            .iter()
            .map(|n| client(&format!("{n}-service")))
            .collect();
        let path = |text: &str| format!("server:\n  servlet:\n    context-path: {text}\n");
        let (app, accounts, beta_yml, eps, gamma_shared, gamma_common, zeta) = (
            path("/app"),
            path("/accounts"),
            path("/beta-yml"),
            path("\"\""),
            path("/gamma-shared"),
            path("/gamma-common"),
            path(""),
        );
        let shared = [
            (format!("config/{res}/shared/application.yml"), app.as_str()),
            (
                format!("config/{res}/shared/acct-service.yml"),
                accounts.as_str(),
            ),
            // Not read: a `file:` location and a placeholder location.
            (
                format!("config/{res}/extra/notes-service.yml"),
                "server:\n  servlet:\n    context-path: /from-file\n",
            ),
            (
                format!("config/{res}/{{application}}/notes-service.yml"),
                "server:\n  servlet:\n    context-path: /from-placeholder\n",
            ),
            // Only the unconditional documents are read, in YAML and in `.properties`.
            (
                format!("config/{res}/shared/alpha-service.yml"),
                "server:\n  servlet:\n    context-path: /alpha\n---\nspring:\n  profiles: docker\nserver:\n  servlet:\n    context-path: /docker-alpha\n",
            ),
            (
                format!("config/{res}/shared/iota-service.properties"),
                "server.servlet.context-path=/iota\n#---\nspring.config.activate.on-profile=docker\nserver.servlet.context-path=/docker-iota\n",
            ),
            // In one location `.properties` beats YAML.
            (
                format!("config/{res}/shared/beta-service.yml"),
                beta_yml.as_str(),
            ),
            (
                format!("config/{res}/shared/beta-service.properties"),
                "server.servlet.context-path=/beta-props\n",
            ),
            // A later location beats an earlier one.
            (
                format!("config/{res}/shared/gamma-service.yml"),
                gamma_shared.as_str(),
            ),
            (
                format!("config/{res}/common/gamma-service.yml"),
                gamma_common.as_str(),
            ),
            // An empty remote value is the root context, over a local path (evaluator pass 1,
            // E3) and over the shared `application.yml`; an absent key overrides nothing.
            (format!("config/{res}/shared/eps-service.yml"), eps.as_str()),
            (
                format!("config/{res}/shared/zeta-service.yml"),
                zeta.as_str(),
            ),
            (
                format!("config/{res}/shared/theta-service.yml"),
                "management:\n  port: 9000\n",
            ),
        ];
        let mut files: Vec<(String, &str)> = vec![
            (format!("config/{res}/application.yml"), native),
            // The client's directory is not its name; its local context path ranks lowest.
            (
                format!("account/{res}/application.properties"),
                "server.servlet.context-path=/local\n",
            ),
            (
                format!("eps/{res}/application.properties"),
                "server.servlet.context-path=/local-eps\n",
            ),
            // The same name without a config client.
            (
                format!("other/{res}/application.properties"),
                "spring.application.name=acct-service\n",
            ),
        ];
        for (n, text) in names.iter().zip(&clients) {
            let dir = if *n == "acct" { "account" } else { n };
            files.push((format!("{dir}/{res}/bootstrap.yml"), text));
        }
        files.extend(shared.iter().map(|(p, t)| (p.clone(), *t)));
        let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), *t)).collect();
        let applied = |service: &str, file: &str, context_path: &str| {
            format!(
                r#"config_server_context_path {{"context_path":"{context_path}","file":"config/src/main/resources/{file}","service":"{service}"}}"#
            )
        };
        assert_eq!(
            run(&borrowed),
            (
                vec![
                    "HTTP GET /accounts/x @account [synthetic]".to_owned(),
                    "HTTP GET /alpha/x @alpha [synthetic]".to_owned(),
                    "HTTP GET /beta-props/x @beta [synthetic]".to_owned(),
                    "HTTP GET /x @config [exact]".to_owned(),
                    "HTTP GET /x @eps [exact]".to_owned(),
                    "HTTP GET /gamma-common/x @gamma [synthetic]".to_owned(),
                    "HTTP GET /iota/x @iota [synthetic]".to_owned(),
                    "HTTP GET /app/x @notes [synthetic]".to_owned(),
                    "HTTP GET /x @other [exact]".to_owned(),
                    "HTTP GET /app/x @stats [synthetic]".to_owned(),
                    "HTTP GET /app/x @theta [synthetic]".to_owned(),
                    "HTTP GET /x @zeta [exact]".to_owned(),
                ],
                vec![
                    applied("account", "shared/acct-service.yml", "/accounts"),
                    applied("alpha", "shared/alpha-service.yml", "/alpha"),
                    applied("beta", "shared/beta-service.properties", "/beta-props"),
                    applied("eps", "shared/eps-service.yml", ""),
                    applied("gamma", "common/gamma-service.yml", "/gamma-common"),
                    applied("iota", "shared/iota-service.properties", "/iota"),
                    applied("notes", "shared/application.yml", "/app"),
                    applied("stats", "shared/application.yml", "/app"),
                    applied("theta", "shared/application.yml", "/app"),
                    applied("zeta", "shared/zeta-service.yml", ""),
                ],
            )
        );
        let acct = clients[0].as_str();
        // `searchLocations` (camelCase) in `.properties`, and a server at the repository root.
        let camel = [
            (
                format!("config/{res}/application.properties"),
                "spring.profiles.active=native\nspring.cloud.config.server.native.searchLocations=classpath:/shared\n",
            ),
            (format!("account/{res}/bootstrap.yml"), acct),
            (shared[1].0.clone(), shared[1].1),
        ];
        let root = [
            (
                format!("{res}/application.yml"),
                "spring:\n  profiles:\n    active: native\n  cloud:\n    config:\n      server:\n        native:\n          search-locations: classpath:/shared\n",
            ),
            (format!("account/{res}/bootstrap.yml"), acct),
            (format!("{res}/shared/acct-service.yml"), shared[1].1),
        ];
        for (files, server, file) in [
            (&camel, "@config", "config/src/main/resources/shared"),
            (&root, "@.", "src/main/resources/shared"),
        ] {
            let borrowed: Vec<(&str, &str)> = files.iter().map(|(p, t)| (p.as_str(), *t)).collect();
            let mut exposes = vec![
                "HTTP GET /accounts/x @account [synthetic]".to_owned(),
                format!("HTTP GET /x {server} [exact]"),
            ];
            exposes.sort();
            let (mut got, counted) = run(&borrowed);
            got.sort();
            assert_eq!(
                (got, counted),
                (
                    exposes,
                    vec![format!(
                        r#"config_server_context_path {{"context_path":"/accounts","file":"{file}/acct-service.yml","service":"account"}}"#
                    )],
                ),
                "{server}"
            );
        }
        // Local files follow the same rule: Spring reads YAML null and `key=` as the empty
        // string, the root context, so a higher-ranked empty value clears a lower path.
        let (boot, quoted, kept) = (path("/boot"), path("\"\""), path("/kept"));
        let local = [
            (format!("lbase/{res}/application.yml"), kept.as_str()),
            (format!("lyml/{res}/bootstrap.yml"), boot.as_str()),
            (format!("lyml/{res}/application.yml"), quoted.as_str()),
            (
                format!("lprof/{res}/application.properties"),
                "spring.profiles.active=dev\nserver.servlet.context-path=/base\n",
            ),
            (
                format!("lprof/{res}/application-dev.properties"),
                "server.servlet.context-path=\n",
            ),
            (
                format!("lnull/{res}/application.properties"),
                "spring.profiles.active=dev\nserver.servlet.context-path=/base\n",
            ),
            (format!("lnull/{res}/application-dev.yml"), zeta.as_str()),
        ];
        let borrowed: Vec<(&str, &str)> = local.iter().map(|(p, t)| (p.as_str(), *t)).collect();
        assert_eq!(
            run(&borrowed),
            (
                vec![
                    "HTTP GET /kept/x @lbase [synthetic]".to_owned(),
                    "HTTP GET /x @lnull [exact]".to_owned(),
                    "HTTP GET /x @lprof [exact]".to_owned(),
                    "HTTP GET /x @lyml [exact]".to_owned(),
                ],
                vec![],
            )
        );
        // A git backend serves nothing rb reads, even with native locations configured.
        let git = "spring:\n  cloud:\n    config:\n      server:\n        git:\n          uri: https://example.com/cfg\n        native:\n          search-locations: classpath:/shared\n  profiles:\n     active: git\n";
        let git_files = [
            (format!("config/{res}/application.yml"), git),
            (format!("account/{res}/bootstrap.yml"), acct),
            (shared[1].0.clone(), shared[1].1),
        ];
        let borrowed: Vec<(&str, &str)> = git_files.iter().map(|(p, t)| (p.as_str(), *t)).collect();
        assert_eq!(
            run(&borrowed),
            (
                vec![
                    "HTTP GET /x @account [exact]".to_owned(),
                    "HTTP GET /x @config [exact]".to_owned(),
                ],
                vec![],
            )
        );
        // Two native servers: nothing applies, and they are counted.
        let two = [
            (format!("config/{res}/application.yml"), native),
            (format!("config2/{res}/application.yml"), native),
            (format!("account/{res}/bootstrap.yml"), acct),
            (shared[1].0.clone(), shared[1].1),
            (
                format!("config2/{res}/shared/acct-service.yml"),
                shared[1].1,
            ),
        ];
        let borrowed: Vec<(&str, &str)> = two.iter().map(|(p, t)| (p.as_str(), *t)).collect();
        assert_eq!(
            run(&borrowed),
            (
                vec![
                    "HTTP GET /x @account [exact]".to_owned(),
                    "HTTP GET /x @config [exact]".to_owned(),
                    "HTTP GET /x @config2 [exact]".to_owned(),
                ],
                vec![r#"config_server_ambiguous {"servers":["config","config2"]}"#.to_owned()],
            )
        );
    }
}
