//! Running a commit's SCIP jobs (design §5.2): each build root is hashed, looked up in the index
//! cache, and only on a miss exported and indexed in a container. The indexer never talks to the
//! database; the caller plugs in the cache, as for syntax facts.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use rb_model::FileEntry;
use rb_scip::{Budget, Images, RunStatus, Runner, ScipIndex};

/// A recorded indexer run, content-addressed by `(project_input_hash, image)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipCacheEntry {
    /// `ok` or `failed` (contract M3b amendments 30 and 31); a row with any other value, written
    /// by an older binary, is a miss.
    pub status: String,
    /// The phase a `failed` run stopped in (`fetch` or `index`; contract M3b amendment 56); a
    /// `failed` row without one, written by an older binary, is a miss.
    pub phase: Option<String>,
    /// `index.scip` bytes when `ok`.
    pub index: Option<Vec<u8>>,
    pub stderr_tail: String,
    pub elapsed_ms: u64,
}

/// Where earlier index runs live.
pub trait ScipCache {
    fn get(&mut self, input_hash: &str, image: &str) -> anyhow::Result<Option<ScipCacheEntry>>;
    fn put(&mut self, input_hash: &str, image: &str, entry: &ScipCacheEntry) -> anyhow::Result<()>;
}

pub struct NoScipCache;

impl ScipCache for NoScipCache {
    fn get(&mut self, _: &str, _: &str) -> anyhow::Result<Option<ScipCacheEntry>> {
        Ok(None)
    }

    fn put(&mut self, _: &str, _: &str, _: &ScipCacheEntry) -> anyhow::Result<()> {
        Ok(())
    }
}

/// A cache that is never read, only written (`rb index --no-cache`, contract M3b amendment
/// 20): every job runs again and its outcome replaces the row, so a cached `failed` can be
/// retried.
pub struct WriteOnlyScipCache<'a>(pub &'a mut dyn ScipCache);

impl ScipCache for WriteOnlyScipCache<'_> {
    fn get(&mut self, _: &str, _: &str) -> anyhow::Result<Option<ScipCacheEntry>> {
        Ok(None)
    }

    fn put(&mut self, input_hash: &str, image: &str, entry: &ScipCacheEntry) -> anyhow::Result<()> {
        self.0.put(input_hash, image, entry)
    }
}

/// Whether SCIP jobs run, decided by the caller (contract M3b amendment 17). It is part of a
/// snapshot's tool configuration: a graph indexed without Docker is heuristic, so a later run
/// with Docker must not take it for "up to date".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScipMode {
    /// `--no-scip`: every job is `skipped`.
    Off,
    /// Docker does not answer (the reason is user-facing): every job is `skipped`.
    Unavailable(String),
    /// Jobs run in containers.
    On,
}

impl ScipMode {
    /// The value recorded in `tool_versions.scip`.
    pub fn as_str(&self) -> &'static str {
        match self {
            ScipMode::Off => "off",
            ScipMode::Unavailable(_) => "unavailable",
            ScipMode::On => "on",
        }
    }

    /// The `skipped` reason every job records when no job runs, `None` when they run. Fixed
    /// text: the detailed Docker error names the host's socket path and is printed by `rb
    /// index` only, never stored (contract M3b amendment 49).
    pub fn skip_reason(&self) -> Option<&'static str> {
        match self {
            ScipMode::Off => Some("SCIP disabled (--no-scip)"),
            ScipMode::Unavailable(_) => Some("docker is not available"),
            ScipMode::On => None,
        }
    }
}

/// How SCIP jobs run for one `index_commit`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScipOptions {
    pub mode: ScipMode,
    /// The indexer images on this host, resolved once by the caller before its up-to-date check
    /// (contract M3b amendment 29): what each job runs in and caches under.
    pub images: Images,
    /// Holds one export directory per job run (`<input_hash[..16]>-<pid>-<nanos>`, so runs of
    /// the same commit at once share nothing; contract M3b amendment 22), removed after the
    /// run. Must be a path the container runtime can bind-mount.
    pub work_dir: PathBuf,
    /// The repository's package-manager caches (contract M5 S22: the caller keys the directory
    /// by repository, so one repository's fetch cannot poison another's).
    pub cache_dir: PathBuf,
    pub budget: Budget,
    /// The fetch phase's allow-list (contract M5 S21), part of every job's cache key.
    pub egress: rb_scip::Egress,
}

impl ScipOptions {
    /// No jobs run; every project stays heuristic.
    pub fn disabled() -> Self {
        ScipOptions {
            mode: ScipMode::Off,
            images: Images::unresolved(),
            work_dir: PathBuf::new(),
            cache_dir: PathBuf::new(),
            budget: Budget::default(),
            egress: rb_scip::Egress::default(),
        }
    }
}

/// One build root's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct ScipProject {
    /// Repo-relative build root (`""` for the repository root).
    pub root: String,
    pub indexer: &'static str,
    /// The image the job runs in and its index is cached under: a registry digest reference, or
    /// the image ID of the locally built scip-java image (`rb_scip::Images::for_job`).
    pub image: String,
    pub status: RunStatus,
    /// The decoded index when `status` is `Ok`; dropped once the graph is assembled.
    pub index: Option<ScipIndex>,
    /// The container run's time (the cached run's when served from the cache), 0 when not run.
    pub elapsed_ms: u64,
    /// Served from the cache: no container ran.
    pub cached: bool,
    pub stderr_tail: String,
    /// The fetch step had no lockfile, so the index is not reproducible.
    pub unpinned: bool,
    /// Workspace members grouped into this job (contract M6a D5); 0 for any other job.
    pub workspace_members: usize,
    /// Link-target directories outside the root whose files the job exported (contract M6a
    /// D7).
    pub link_targets: usize,
}

/// Whether jobs run at all.
pub(crate) enum Mode<'a> {
    /// Every job is `skipped` with this reason.
    Skip(String),
    Run(&'a dyn Runner),
}

/// Runs (or serves from the cache) every job of `files`, in job order; also returns the
/// workspace roots left ungrouped (contract M6a D5), which `rb index` prints and nothing stores.
pub(crate) fn run_projects(
    files: &[FileEntry],
    read_blob: &dyn Fn(&str) -> anyhow::Result<Vec<u8>>,
    mode: &Mode<'_>,
    options: &ScipOptions,
    cache: &mut dyn ScipCache,
) -> anyhow::Result<(Vec<ScipProject>, Vec<rb_scip::jobs::Ungrouped>)> {
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let blobs: BTreeMap<&str, &str> = files
        .iter()
        .map(|f| (f.path.as_str(), f.blob_sha.as_str()))
        .collect();
    // The manifest reader of D5: `package.json` files and workspace roots' `yarn.lock`.
    let read_path = |path: &str| blobs.get(path).and_then(|blob| read_blob(blob).ok());
    let plan = rb_scip::jobs(&paths, &read_path);
    let mut out = Vec::new();
    for job in plan.jobs {
        let mut project = ScipProject {
            root: job.root.clone(),
            indexer: job.indexer.as_str(),
            image: rb_scip::image_for(&job).to_owned(),
            status: RunStatus::Skipped(String::new()),
            index: None,
            elapsed_ms: 0,
            cached: false,
            stderr_tail: String::new(),
            unpinned: job.unpinned,
            workspace_members: plan.members.get(&job.root).map_or(0, Vec::len),
            link_targets: 0,
        };
        let runner = match mode {
            Mode::Skip(reason) => {
                project.status = RunStatus::Skipped(reason.clone());
                out.push(project);
                continue;
            }
            Mode::Run(runner) => *runner,
        };
        // The image was resolved before the cache lookup: it is the key's second half.
        match options.images.for_job(&job) {
            Ok(image) => project.image = image,
            Err(reason) => {
                project.status = RunStatus::Unsupported(reason);
                out.push(project);
                continue;
            }
        }
        let under_root: Vec<(&str, &str)> = blobs
            .iter()
            .filter(|(path, _)| in_root(path, &job.root))
            .map(|(p, b)| (*p, *b))
            .collect();
        // What the job exports and its key covers (contract M6a D7); a link target outside
        // the repository leaves the job unsupported before anything is written.
        let exported = match job_files(&job, &blobs, &read_path) {
            Ok(selected) => {
                project.link_targets = selected.link_targets;
                selected.files
            }
            Err(reason) => {
                project.status = RunStatus::Unsupported(reason);
                out.push(project);
                continue;
            }
        };
        let input_hash = rb_scip::project_input_hash(
            &rb_scip::recipe(&job, &options.budget),
            exported.iter().copied(),
            &options.egress,
        );
        // A row whose status no current run would write (a `timeout` from an older binary) is
        // a miss (contract M3b amendment 31).
        let cached = cache.get(&input_hash, &project.image)?.and_then(|entry| {
            Some((
                RunStatus::from_cached(&entry.status, entry.phase.as_deref())?,
                entry,
            ))
        });
        let (status, entry) = match cached {
            Some(hit) => {
                project.cached = true;
                hit
            }
            None => {
                // scip-java's export is its root (root-relative paths); scip-typescript's is
                // repository-shaped (D7).
                let exported = exported
                    .iter()
                    .map(|(path, blob)| {
                        // Contract M6d S1/S2: scip-go exports like scip-java
                        // (root-relative); scip-python is repository-shaped
                        // like scip-typescript; both revisited with evidence in S5.
                        let path = match job.indexer {
                            rb_scip::Indexer::ScipJava | rb_scip::Indexer::ScipGo => {
                                relative(path, &job.root)
                            }
                            rb_scip::Indexer::ScipTypescript | rb_scip::Indexer::ScipPython => {
                                (*path).to_owned()
                            }
                        };
                        Ok((path, read_blob(blob)?))
                    })
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let run = rb_scip::run_job(
                    &job,
                    &project.image,
                    &exported,
                    &options.work_dir.join(&input_hash[..16]),
                    &options.cache_dir,
                    &options.budget,
                    &options.egress,
                    runner,
                )?;
                let entry = ScipCacheEntry {
                    status: run.status.as_str().to_owned(),
                    phase: run.status.phase().map(|p| p.as_str().to_owned()),
                    index: run.index,
                    stderr_tail: run.stderr_tail,
                    elapsed_ms: u64::try_from(run.elapsed.as_millis()).unwrap_or(u64::MAX),
                };
                // Only the project's own outcome is cached; a missing image, a runtime failure
                // or a budget overrun is decided again next time (amendments 3, 30 and 31).
                if run.status.is_cacheable() {
                    cache.put(&input_hash, &project.image, &entry)?;
                }
                (run.status, entry)
            }
        };
        project.status = status;
        project.elapsed_ms = entry.elapsed_ms;
        project.stderr_tail = entry.stderr_tail;
        if let (RunStatus::Ok, Some(bytes)) = (&project.status, &entry.index) {
            let source_of = |doc: &str| {
                let path = join_root(&job.root, doc);
                blobs.get(path.as_str()).and_then(|b| read_blob(b).ok())
            };
            match rb_scip::decode(bytes, &source_of) {
                Ok(mut index) => {
                    if job.fetch == rb_scip::jobs::Fetch::Maven {
                        reject_for_source_encoding(&mut index, &under_root, read_blob, &source_of);
                    }
                    project.index = Some(index);
                }
                Err(e) => {
                    project.status = RunStatus::Failed(rb_scip::Phase::Index);
                    project.stderr_tail = format!("decoding index.scip: {e:#}");
                }
            }
        }
        out.push(project);
    }
    Ok((out, plan.ungrouped))
}

/// The files a job exports and its cache key covers (contract M6a D7), with repository paths.
pub(crate) struct JobFiles<'b> {
    pub files: Vec<(&'b str, &'b str)>,
    /// Link-target directories with at least one exported file.
    pub link_targets: usize,
}

/// The reason a job whose lockfile links outside the repository is `unsupported`; it names no
/// path of the host or the repository (contract M6a D7).
const LINK_OUTSIDE: &str = "link target outside the repository";

/// The files of `job`'s root, and for a scip-typescript job whose root holds a
/// `package-lock.json` the tracked files (outside any `node_modules`) of each directory the
/// lockfile links to inside the repository and outside the root ([`link_targets`]). A target
/// outside the repository is `Err`. Each link target's files are one range of the sorted path
/// map, so the selection is linear in what it exports.
pub(crate) fn job_files<'b>(
    job: &rb_scip::Job,
    blobs: &BTreeMap<&'b str, &'b str>,
    read_path: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<JobFiles<'b>, String> {
    // The files under `dir`: one range of the sorted paths, each path examined counted.
    let under = |dir: &str| -> Vec<(&'b str, &'b str)> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        blobs
            .range::<str, _>((
                std::ops::Bound::Included(prefix.as_str()),
                std::ops::Bound::Unbounded,
            ))
            .inspect(|_| rb_frameworks::work::tick(1))
            .take_while(|(p, _)| p.starts_with(&prefix))
            .map(|(p, b)| (*p, *b))
            .collect()
    };
    let mut files = under(&job.root);
    let lockfile = matches!(
        job.fetch,
        rb_scip::jobs::Fetch::Npm {
            lockfile: rb_scip::jobs::NodeLock::PackageLock
        }
    )
    .then(|| read_path(&join_root(&job.root, "package-lock.json")))
    .flatten();
    let Some(lockfile) = lockfile else {
        return Ok(JobFiles {
            files,
            link_targets: 0,
        });
    };
    let mut exported_targets = 0;
    for target in link_targets(&job.root, &lockfile)? {
        let target_files: Vec<(&str, &str)> = under(&target)
            .into_iter()
            .filter(|(p, _)| !p.split('/').any(|segment| segment == "node_modules"))
            .collect();
        exported_targets += usize::from(!target_files.is_empty());
        files.extend(target_files);
    }
    files.sort_unstable();
    files.dedup();
    Ok(JobFiles {
        files,
        link_targets: exported_targets,
    })
}

/// The directories a `package-lock.json` (v2 or v3) at `root` links to inside the repository
/// and outside `root`, sorted and distinct (contract M6a D7): every `packages` entry with
/// `"link": true` whose `resolved` is a relative path, and every `packages` key starting with
/// `../`, resolved lexically against `root` (`.` and `..` folded, nothing on disk followed). A
/// target that leaves the repository is `Err` (its path is not reported). A v1 lockfile (no
/// `packages`) or one that is not JSON names no target. The lockfile is read once, so links
/// that name each other cannot loop.
pub(crate) fn link_targets(root: &str, lockfile: &[u8]) -> Result<Vec<String>, String> {
    let Ok(lock) = serde_json::from_slice::<serde_json::Value>(lockfile) else {
        return Ok(Vec::new());
    };
    let Some(packages) = lock.get("packages").and_then(serde_json::Value::as_object) else {
        return Ok(Vec::new());
    };
    let mut targets = BTreeSet::new();
    for (key, entry) in packages {
        rb_frameworks::work::tick(1);
        let resolved = entry
            .get("link")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
            .then(|| entry.get("resolved").and_then(serde_json::Value::as_str))
            .flatten()
            .filter(|r| !r.starts_with('/') && !r.contains(':'));
        let candidates = [key.starts_with("../").then_some(key.as_str()), resolved];
        for candidate in candidates.into_iter().flatten() {
            let mut at: Vec<&str> = if root.is_empty() {
                Vec::new()
            } else {
                root.split('/').collect()
            };
            for segment in candidate.split('/') {
                match segment {
                    "" | "." => {}
                    ".." => {
                        at.pop().ok_or_else(|| LINK_OUTSIDE.to_owned())?;
                    }
                    name => at.push(name),
                }
            }
            let dir = at.join("/");
            if !(dir == root || in_root(&dir, root)) {
                targets.insert(dir);
            }
        }
    }
    Ok(targets.into_iter().collect())
}

/// Contract M3b amendment 64 (`source_encoding`): when a Maven build declares a source encoding
/// other than UTF-8 in any of its poms, javac reads every non-ASCII file differently from
/// Tree-sitter, so each such document is rejected. The decoder's reasons found earlier stay,
/// except `encoding_ambiguous` (amendment 69): the pom's declaration is the more specific
/// signal and is recorded instead.
fn reject_for_source_encoding(
    index: &mut ScipIndex,
    under_root: &[(&str, &str)],
    read_blob: &dyn Fn(&str) -> anyhow::Result<Vec<u8>>,
    source_of: &dyn Fn(&str) -> Option<Vec<u8>>,
) {
    let poms: Vec<Vec<u8>> = under_root
        .iter()
        .filter(|(path, _)| path.rsplit('/').next() == Some("pom.xml"))
        .filter_map(|(_, blob)| read_blob(blob).ok())
        .collect();
    let poms: Vec<&[u8]> = poms.iter().map(Vec::as_slice).collect();
    if rb_scip::maven_source_encoding(&poms).is_none() {
        return;
    }
    for doc in &mut index.documents {
        if matches!(
            doc.rejected,
            None | Some(rb_scip::Rejection::EncodingAmbiguous)
        ) && source_of(&doc.path).is_some_and(|text| !text.is_ascii())
        {
            doc.rejected = Some(rb_scip::Rejection::SourceEncoding);
        }
    }
}

/// `path` is at or below `root` (`""` is the repository root).
pub(crate) fn in_root(path: &str, root: &str) -> bool {
    root.is_empty() || path.strip_prefix(root).is_some_and(|r| r.starts_with('/'))
}

/// The deepest of `roots` containing `path`, if any.
pub(crate) fn owner_root<'r>(path: &str, roots: impl Iterator<Item = &'r str>) -> Option<&'r str> {
    roots.filter(|r| in_root(path, r)).max_by_key(|r| r.len())
}

fn relative(path: &str, root: &str) -> String {
    if root.is_empty() {
        path.to_owned()
    } else {
        path[root.len() + 1..].to_owned()
    }
}

fn join_root(root: &str, path: &str) -> String {
    if root.is_empty() {
        path.to_owned()
    } else {
        format!("{root}/{path}")
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::HashMap;

    use rb_scip::{ContainerRun, Outcome};

    use super::*;

    const JAVA_ID: &str = "sha256:66a0b395166fc65c49cc099b0b3e4d2cb61e16c177dbc3c022160bb0b4cd1cf2";

    struct Fake {
        runs: RefCell<usize>,
        index: Vec<u8>,
        fail: bool,
        /// The scip-java image is present locally.
        java_image: bool,
        /// Replaces every run's outcome when set (a runtime failure, a timeout).
        outcome: Option<Outcome>,
    }

    impl Fake {
        fn new(index: Vec<u8>, fail: bool, java_image: bool) -> Fake {
            Fake {
                runs: RefCell::new(0),
                index,
                fail,
                java_image,
                outcome: None,
            }
        }
    }

    impl Runner for Fake {
        fn run(&self, run: &ContainerRun) -> anyhow::Result<Outcome> {
            *self.runs.borrow_mut() += 1;
            if let Some(outcome) = &self.outcome {
                return Ok(outcome.clone());
            }
            if run.network == rb_scip::Network::None && !self.fail {
                std::fs::write(run.host_cwd().join("index.scip"), &self.index)?;
            }
            Ok(Outcome {
                exit_code: Some(i32::from(self.fail)),
                timed_out: false,
                stderr_tail: if self.fail {
                    "boom".into()
                } else {
                    String::new()
                },
                elapsed: std::time::Duration::from_millis(7),
            })
        }

        fn image_id(&self, image: &str) -> Option<String> {
            if image == rb_scip::egress::PROXY_IMAGE {
                return Some("sha256:proxy".to_owned());
            }
            (self.java_image && image == rb_scip::jobs::SCIP_JAVA_IMAGE).then(|| JAVA_ID.to_owned())
        }

        fn start_proxy(&self, _: &rb_scip::ProxySpec) -> anyhow::Result<String> {
            Ok("10.9.8.7".to_owned())
        }

        fn stop_proxy(&self, _: &str) -> String {
            String::new()
        }
    }

    #[derive(Default)]
    struct MemCache {
        rows: HashMap<(String, String), ScipCacheEntry>,
        puts: usize,
    }

    impl ScipCache for MemCache {
        fn get(&mut self, h: &str, i: &str) -> anyhow::Result<Option<ScipCacheEntry>> {
            Ok(self.rows.get(&(h.to_owned(), i.to_owned())).cloned())
        }

        fn put(&mut self, h: &str, i: &str, e: &ScipCacheEntry) -> anyhow::Result<()> {
            self.puts += 1;
            self.rows.insert((h.to_owned(), i.to_owned()), e.clone());
            Ok(())
        }
    }

    fn entry(path: &str, blob: &str) -> FileEntry {
        FileEntry {
            path: path.into(),
            blob_sha: blob.repeat(40),
            size: 1,
            executable: false,
            lang: rb_git::language_for_path(path),
            binary: false,
            generated: false,
        }
    }

    /// A protobuf length-delimited field (`field` < 16, body shorter than 128 bytes).
    fn field(number: u8, body: &[u8]) -> Vec<u8> {
        assert!(number < 16 && body.len() < 128);
        let mut out = vec![(number << 3) | 2, body.len() as u8];
        out.extend_from_slice(body);
        out
    }

    /// A minimal `index.scip`, encoded by hand (scip.proto: `Index.documents` = 2,
    /// `Document.relative_path` = 1, `Document.occurrences` = 2, `Occurrence.range` = 1 (packed),
    /// `Occurrence.symbol` = 2, `Occurrence.symbol_roles` = 3): one definition of `f` in
    /// `src/a.ts` at line 0, columns 16–17.
    fn index_bytes() -> Vec<u8> {
        let mut occurrence = field(1, &[0, 16, 17]);
        occurrence.extend(field(2, b"scip-typescript npm p 1.0.0 src/`a.ts`/f()."));
        occurrence.extend([3 << 3, 1]);
        let mut document = field(1, b"src/a.ts");
        document.extend(field(2, &occurrence));
        field(2, &document)
    }

    fn options(tmp: &std::path::Path, runner: &dyn Runner) -> ScipOptions {
        ScipOptions {
            mode: ScipMode::On,
            images: Images::resolve(runner),
            work_dir: tmp.join("work"),
            cache_dir: tmp.join("cache"),
            budget: Budget::default(),
            egress: rb_scip::Egress::default(),
        }
    }

    #[test]
    fn a_miss_runs_the_job_and_fills_the_cache_and_a_hit_runs_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let files = vec![
            entry("web/package.json", "1"),
            entry("web/tsconfig.json", "2"),
            entry("web/src/a.ts", "3"),
            entry("README.md", "4"),
        ];
        let sources: HashMap<String, Vec<u8>> = files
            .iter()
            .map(|f| (f.blob_sha.clone(), b"export function f() {}\n".to_vec()))
            .collect();
        let read = |blob: &str| Ok(sources[blob].clone());
        let fake = Fake::new(index_bytes(), false, true);
        let mut cache = MemCache::default();
        let opts = options(tmp.path(), &fake);
        let cold = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache)
            .unwrap()
            .0;
        assert_eq!(cold.len(), 1);
        let p = &cold[0];
        assert_eq!((p.root.as_str(), p.indexer), ("web", "scip-typescript"));
        assert_eq!(p.status, RunStatus::Ok);
        assert!(!p.cached);
        assert!(p.unpinned, "no lockfile");
        let index = p.index.as_ref().unwrap();
        assert_eq!(index.documents[0].path, "src/a.ts");
        assert_eq!(index.documents[0].occurrences[0].span, [0, 16, 0, 17]);
        assert_eq!(*fake.runs.borrow(), 2, "fetch and index");
        assert_eq!(cache.puts, 1);
        assert!(!tmp.path().join("work").read_dir().unwrap().any(|_| true));

        let warm = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache)
            .unwrap()
            .0;
        assert_eq!(*fake.runs.borrow(), 2, "no container ran");
        assert!(warm[0].cached);
        assert_eq!(warm[0].index, cold[0].index);
        assert_eq!(
            warm[0].elapsed_ms, cold[0].elapsed_ms,
            "the cached run's time is reported"
        );

        // A changed blob under the root is a different key; a change outside it is not.
        let mut changed = files.clone();
        changed[2].blob_sha = "9".repeat(40);
        let sources2: HashMap<String, Vec<u8>> = changed
            .iter()
            .map(|f| (f.blob_sha.clone(), b"export function f() {}\n".to_vec()))
            .collect();
        let read2 = |blob: &str| Ok(sources2[blob].clone());
        run_projects(&changed, &read2, &Mode::Run(&fake), &opts, &mut cache).unwrap();
        assert_eq!(*fake.runs.borrow(), 4);
        let mut outside = files.clone();
        outside[3].blob_sha = "8".repeat(40);
        run_projects(&outside, &read, &Mode::Run(&fake), &opts, &mut cache).unwrap();
        assert_eq!(*fake.runs.borrow(), 4);
    }

    #[test]
    fn skipped_and_unsupported_jobs_touch_neither_runner_nor_cache() {
        let tmp = tempfile::tempdir().unwrap();
        let files = vec![
            entry("svc/pom.xml", "1"),
            entry("svc/src/A.java", "2"),
            entry("web/package.json", "3"),
            entry("web/tsconfig.json", "4"),
        ];
        let read = |_: &str| Ok(Vec::new());
        let fake = Fake::new(Vec::new(), true, false);
        let mut cache = MemCache::default();
        let opts = options(tmp.path(), &fake);
        let skipped = run_projects(
            &files,
            &read,
            &Mode::Skip("--no-scip".into()),
            &opts,
            &mut cache,
        )
        .unwrap()
        .0;
        let statuses: Vec<(&str, &str, Option<String>)> = skipped
            .iter()
            .map(|p| (p.root.as_str(), p.status.as_str(), p.status.reason()))
            .collect();
        assert_eq!(
            statuses,
            [
                ("svc", "skipped", Some("--no-scip".to_owned())),
                ("web", "skipped", Some("--no-scip".to_owned())),
            ]
        );
        assert_eq!(*fake.runs.borrow(), 0);

        let ran = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache)
            .unwrap()
            .0;
        // The scip-java image is not built locally: unsupported, with the reason pointing at
        // the image's README.
        assert_eq!(ran[0].status.as_str(), "unsupported");
        let reason = ran[0].status.reason().unwrap();
        assert!(
            reason.contains("rb-scip-java:0.13.1") && reason.contains("docker/scip-java/README.md"),
            "{reason}"
        );
        // The TypeScript job's fetch failed: recorded, cached as such, and served from the cache
        // next time without another attempt.
        // The row records the phase (contract M3b amendment 56).
        assert_eq!(ran[1].status, RunStatus::Failed(rb_scip::Phase::Fetch));
        assert!(ran[1].index.is_none());
        assert_eq!(ran[1].stderr_tail, "boom");
        assert_eq!(cache.puts, 1, "unsupported jobs are not cached");
        assert_eq!(*fake.runs.borrow(), 1);
        let row = cache.rows.values().next().unwrap();
        assert_eq!(
            (row.status.as_str(), row.phase.as_deref()),
            ("failed", Some("fetch"))
        );
        let again = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache)
            .unwrap()
            .0;
        assert!(again[1].cached && again[1].status == RunStatus::Failed(rb_scip::Phase::Fetch));
        assert_eq!(*fake.runs.borrow(), 1);
        // A `failed` row without a phase (an older binary's) is a miss: the job runs again.
        for row in cache.rows.values_mut() {
            row.phase = None;
        }
        let rerun = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache)
            .unwrap()
            .0;
        assert!(!rerun[1].cached && rerun[1].status == RunStatus::Failed(rb_scip::Phase::Fetch));
        assert_eq!(*fake.runs.borrow(), 2);

        // `--no-cache` (contract M3b amendment 20): the cached failure is not read, the job
        // runs again, and its new outcome replaces the row.
        let retry = Fake::new(index_bytes(), false, false);
        let retried = run_projects(
            &files,
            &read,
            &Mode::Run(&retry),
            &opts,
            &mut WriteOnlyScipCache(&mut cache),
        )
        .unwrap()
        .0;
        assert!(!retried[1].cached && retried[1].status == RunStatus::Ok);
        assert_eq!(*retry.runs.borrow(), 2, "fetch and index ran again");
        assert_eq!(cache.puts, 3, "the outcome is written");
        let served = run_projects(&files, &read, &Mode::Run(&retry), &opts, &mut cache)
            .unwrap()
            .0;
        assert!(served[1].cached && served[1].status == RunStatus::Ok);
        assert_eq!(*retry.runs.borrow(), 2);
    }

    /// A Maven job with the local image present runs (two fetch runs and the index) and is
    /// cached under the image ID, which is what the project reports as its image.
    #[test]
    fn maven_jobs_are_keyed_by_the_local_image_id() {
        let tmp = tempfile::tempdir().unwrap();
        let files = vec![entry("svc/pom.xml", "1"), entry("svc/src/A.java", "2")];
        let read = |_: &str| Ok(b"class A {}\n".to_vec());
        let fake = Fake::new(index_bytes(), false, true);
        let mut cache = MemCache::default();
        let opts = options(tmp.path(), &fake);
        let cold = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache)
            .unwrap()
            .0;
        assert_eq!(cold.len(), 1);
        assert_eq!(
            (cold[0].indexer, cold[0].status.as_str()),
            ("scip-java", "ok")
        );
        assert_eq!(cold[0].image, JAVA_ID);
        assert_eq!(*fake.runs.borrow(), 3, "go-offline, verify, index");
        let keys: Vec<&(String, String)> = cache.rows.keys().collect();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0].1, JAVA_ID, "the cache key's image part is the ID");
        let warm = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache)
            .unwrap()
            .0;
        assert!(warm[0].cached);
        assert_eq!(*fake.runs.borrow(), 3, "no container ran");
        assert_eq!(warm[0].image, JAVA_ID);
    }

    /// Contract M3b amendment 64 (`source_encoding`): a Maven build declaring ISO-8859-1 has
    /// javac read a non-ASCII file differently from Tree-sitter (the evaluator's `Enc.java`:
    /// valid UTF-8, every later column of the line one unit off per `é`), so that document is
    /// rejected; an ASCII document of the same build, and every document of a UTF-8 build,
    /// is not. `Enc.java` also has an occurrence whose raw column names an `a` under both a
    /// UTF-8 and an ISO-8859-1 reading (contract M3b amendment 69): the pom's signal, read
    /// first, is the reason recorded in the ISO-8859-1 build, `encoding_ambiguous` in the
    /// UTF-8 one.
    #[test]
    fn a_non_utf8_maven_build_rejects_its_non_ascii_documents() {
        use protobuf::Message as _;
        let enc_text = "class Enc { /* \u{e9} */ int aa() { return 0; } }\n";
        let second_a = enc_text.find("aa").unwrap() + 1;
        // Raw column 25 is the second `a` read as UTF-16 units and the first read as bytes.
        assert_eq!(enc_text[..second_a].chars().count(), 25);
        assert_eq!(&enc_text[25..26], "a");
        let index = scip::types::Index {
            metadata: Some(scip::types::Metadata {
                tool_info: Some(scip::types::ToolInfo {
                    name: "scip-java".into(),
                    ..Default::default()
                })
                .into(),
                ..Default::default()
            })
            .into(),
            documents: ["src/Enc.java", "src/Plain.java"]
                .map(|path| scip::types::Document {
                    relative_path: path.into(),
                    occurrences: (path == "src/Enc.java")
                        .then(|| scip::types::Occurrence {
                            symbol: "scip-java maven . . Enc#a().".into(),
                            range: vec![0, 25, 26],
                            ..Default::default()
                        })
                        .into_iter()
                        .collect(),
                    ..Default::default()
                })
                .to_vec(),
            ..Default::default()
        }
        .write_to_bytes()
        .unwrap();
        let files = vec![
            entry("svc/pom.xml", "1"),
            entry("svc/src/Enc.java", "2"),
            entry("svc/src/Plain.java", "3"),
        ];
        let rejected = |encoding: &str| {
            let tmp = tempfile::tempdir().unwrap();
            let pom = format!(
                "<project><properties><project.build.sourceEncoding>{encoding}</project.build.sourceEncoding></properties></project>"
            );
            let sources: HashMap<String, Vec<u8>> = HashMap::from([
                ("1".repeat(40), pom.into_bytes()),
                ("2".repeat(40), enc_text.into()),
                ("3".repeat(40), b"class Plain {}\n".to_vec()),
            ]);
            let read = |blob: &str| Ok(sources[blob].clone());
            let fake = Fake::new(index.clone(), false, true);
            let opts = options(tmp.path(), &fake);
            let projects = run_projects(
                &files,
                &read,
                &Mode::Run(&fake),
                &opts,
                &mut MemCache::default(),
            )
            .unwrap()
            .0;
            projects[0]
                .index
                .as_ref()
                .unwrap()
                .documents
                .iter()
                .map(|d| (d.path.clone(), d.rejected))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            rejected("ISO-8859-1"),
            [
                (
                    "src/Enc.java".to_owned(),
                    Some(rb_scip::Rejection::SourceEncoding)
                ),
                ("src/Plain.java".to_owned(), None),
            ]
        );
        assert_eq!(
            rejected("UTF-8"),
            [
                (
                    "src/Enc.java".to_owned(),
                    Some(rb_scip::Rejection::EncodingAmbiguous)
                ),
                ("src/Plain.java".to_owned(), None),
            ]
        );
    }

    /// Contract M3b amendments 30 and 31: a `docker run` the client or daemon refused (exit 125)
    /// is an `environment` outcome and a phase over its budget a `timeout` naming the phase and
    /// the budget; neither is written to the cache, so the next run tries again. A cached row
    /// with a status no current run writes (`timeout`, from an older binary) is a miss.
    #[test]
    fn runtime_failures_and_timeouts_are_reported_but_not_cached() {
        let tmp = tempfile::tempdir().unwrap();
        let files = vec![
            entry("package.json", "1"),
            entry("tsconfig.json", "2"),
            entry("src/a.ts", "3"),
        ];
        let read = |_: &str| Ok(b"export function f() {}\n".to_vec());
        let outcome = |exit_code: Option<i32>, timed_out: bool, stderr: &str| Outcome {
            exit_code,
            timed_out,
            stderr_tail: stderr.into(),
            elapsed: std::time::Duration::from_millis(3),
        };
        let mut cache = MemCache::default();

        let refused = Fake {
            outcome: Some(outcome(Some(125), false, "shim: docker run refused")),
            ..Fake::new(index_bytes(), false, false)
        };
        let opts = options(tmp.path(), &refused);
        let ran = run_projects(&files, &read, &Mode::Run(&refused), &opts, &mut cache)
            .unwrap()
            .0;
        assert_eq!(
            ran[0].status,
            RunStatus::Environment("shim: docker run refused".into())
        );
        assert_eq!(ran[0].status.as_str(), "environment");
        assert!(!ran[0].cached && ran[0].index.is_none());
        assert_eq!(cache.puts, 0, "an environment outcome is not cached");

        let slow = Fake {
            outcome: Some(outcome(None, true, "")),
            ..Fake::new(index_bytes(), false, false)
        };
        let budget = Budget {
            fetch_timeout: std::time::Duration::from_secs(1),
            ..Budget::default()
        };
        let opts = ScipOptions {
            budget,
            ..options(tmp.path(), &slow)
        };
        let ran = run_projects(&files, &read, &Mode::Run(&slow), &opts, &mut cache)
            .unwrap()
            .0;
        assert_eq!(
            ran[0].status,
            RunStatus::Timeout {
                phase: rb_scip::Phase::Fetch,
                budget_secs: 1
            }
        );
        assert_eq!(
            ran[0].status.reason().as_deref(),
            Some("fetch phase over its 1 s budget")
        );
        assert_eq!(cache.puts, 0, "a timeout is not cached");
        assert_eq!(*slow.runs.borrow(), 1);

        // The next run tries again and, succeeding, is cached.
        let fine = Fake::new(index_bytes(), false, false);
        let opts = options(tmp.path(), &fine);
        let ran = run_projects(&files, &read, &Mode::Run(&fine), &opts, &mut cache)
            .unwrap()
            .0;
        assert_eq!(ran[0].status, RunStatus::Ok);
        assert_eq!(*fine.runs.borrow(), 2, "fetch and index ran");
        assert_eq!(cache.puts, 1);

        // A `timeout` row an older binary wrote is a miss: the job runs again.
        for entry in cache.rows.values_mut() {
            entry.status = "timeout".into();
        }
        let ran = run_projects(&files, &read, &Mode::Run(&fine), &opts, &mut cache)
            .unwrap()
            .0;
        assert!(!ran[0].cached && ran[0].status == RunStatus::Ok);
        assert_eq!(*fine.runs.borrow(), 4, "the stale row was not served");
        assert_eq!(cache.puts, 2, "the row is replaced");
    }

    /// Contract M3b amendment 49: the stored `skipped` reason is fixed text; the Docker error
    /// behind `Unavailable` (which names the host's socket) is not stored.
    #[test]
    fn skip_reasons_name_no_host() {
        let unavailable = ScipMode::Unavailable(
            "docker is not available: Cannot connect to the Docker daemon at unix:///Users/rohit/.colima/default/docker.sock. Is the docker daemon running?".into(),
        );
        let reason = unavailable.skip_reason().unwrap();
        assert_eq!(reason, "docker is not available");
        assert!(!reason.contains("/Users") && !reason.contains("unix://"));
        assert_eq!(
            ScipMode::Off.skip_reason(),
            Some("SCIP disabled (--no-scip)")
        );
        assert_eq!(ScipMode::On.skip_reason(), None);
    }

    #[test]
    fn owner_root_is_the_deepest_containing_root() {
        let roots = ["", "apps", "apps/web"];
        let owner = |p: &str| owner_root(p, roots.iter().copied());
        assert_eq!(owner("apps/web/src/a.ts"), Some("apps/web"));
        assert_eq!(owner("apps/webx/a.ts"), Some("apps"));
        assert_eq!(owner("lib/a.ts"), Some(""));
        assert_eq!(owner_root("lib/a.ts", ["svc"].into_iter()), None);
        assert!(in_root("svc/a", "svc") && !in_root("svc2/a", "svc") && !in_root("svc", "svc"));
    }

    // Contract M6a D7 (S10): what a scip-typescript job exports, and its link targets.

    /// A `package-lock.json` (v3) at `tools/e2e` linking `@ws/core` to `../../packages/core`
    /// (as npm writes a `file:` dependency) and `@ws/lib` to `lib`, inside the root.
    const E2E_LOCK: &str = r#"{"name": "e2e", "lockfileVersion": 3, "packages": {
        "": {"dependencies": {"@ws/core": "file:../../packages/core"}},
        "../../packages/core": {"name": "@ws/core", "version": "1.0.0"},
        "node_modules/@ws/core": {"resolved": "../../packages/core", "link": true},
        "node_modules/@ws/lib": {"resolved": "lib", "link": true},
        "node_modules/left-pad": {"version": "1.3.0", "resolved": "https://registry.npmjs.org/left-pad/-/left-pad-1.3.0.tgz"}
    }}"#;

    fn ts_job(root: &str, lockfile: rb_scip::jobs::NodeLock) -> rb_scip::Job {
        rb_scip::Job {
            root: root.into(),
            indexer: rb_scip::Indexer::ScipTypescript,
            fetch: rb_scip::jobs::Fetch::Npm { lockfile },
            projects: vec![".".into()],
            unpinned: false,
        }
    }

    /// A link target inside the repository and outside the root is exported with its tracked
    /// files (its `package.json` included), never a `node_modules` path; a link inside the
    /// root, a registry package and the sibling `packages/other` add nothing.
    #[test]
    fn a_link_target_inside_the_repository_is_exported() {
        let paths = [
            "tools/e2e/package.json",
            "tools/e2e/package-lock.json",
            "tools/e2e/src/check.ts",
            "tools/e2e/lib/index.ts",
            "packages/core/package.json",
            "packages/core/src/index.ts",
            "packages/core/node_modules/dep/index.js",
            "packages/other/package.json",
            "README.md",
        ];
        let blobs: BTreeMap<&str, &str> = paths.iter().map(|p| (*p, "b")).collect();
        let read = |path: &str| (path == "tools/e2e/package-lock.json").then(|| E2E_LOCK.into());
        let job = ts_job("tools/e2e", rb_scip::jobs::NodeLock::PackageLock);
        let selected = job_files(&job, &blobs, &read).unwrap();
        assert_eq!(selected.link_targets, 1, "packages/core");
        let files: Vec<&str> = selected.files.into_iter().map(|(p, _)| p).collect();
        assert_eq!(
            files,
            [
                "packages/core/package.json",
                "packages/core/src/index.ts",
                "tools/e2e/lib/index.ts",
                "tools/e2e/package-lock.json",
                "tools/e2e/package.json",
                "tools/e2e/src/check.ts",
            ]
        );
        // Without `package-lock.json` (another lockfile, or none) no link is read.
        let yarn = ts_job("tools/e2e", rb_scip::jobs::NodeLock::Yarn);
        assert_eq!(job_files(&yarn, &blobs, &read).unwrap().files.len(), 4);
        // A v1 lockfile has no `packages`: no link is read either.
        let v1 = |path: &str| {
            (path == "tools/e2e/package-lock.json")
                .then(|| br#"{"lockfileVersion": 1, "dependencies": {"@ws/core": {"version": "file:../../packages/core"}}}"#.to_vec())
        };
        assert_eq!(job_files(&job, &blobs, &v1).unwrap().files.len(), 4);
    }

    /// A link that leaves the repository makes the job `unsupported` with a reason that names
    /// no path; nothing is exported, run or cached. Links naming each other are read once.
    #[test]
    fn a_link_target_outside_the_repository_is_unsupported() {
        assert_eq!(
            link_targets(
                "web",
                br#"{"packages": {"node_modules/x": {"resolved": "../../../x", "link": true}}}"#
            ),
            Err("link target outside the repository".to_owned())
        );
        assert_eq!(
            link_targets("web", br#"{"packages": {"../../x": {"version": "1.0.0"}}}"#),
            Err("link target outside the repository".to_owned())
        );
        assert_eq!(
            link_targets(
                "a/b",
                br#"{"packages": {
                    "node_modules/x": {"resolved": "../../c/x", "link": true},
                    "../../c/x": {"name": "x"},
                    "../../c/x/node_modules/y": {"resolved": "../../a/b", "link": true},
                    "node_modules/z": {"resolved": "./../../c/./x/", "link": true}
                }}"#
            ),
            Ok(vec!["c/x".to_owned(), "c/x/node_modules/y".to_owned()])
        );
        let tmp = tempfile::tempdir().unwrap();
        let files = vec![
            entry("web/package.json", "1"),
            entry("web/package-lock.json", "2"),
            entry("web/tsconfig.json", "3"),
        ];
        let read = |blob: &str| {
            Ok(if blob == "2".repeat(40) {
                br#"{"packages": {"node_modules/x": {"resolved": "../../x", "link": true}}}"#
                    .to_vec()
            } else {
                b"{}".to_vec()
            })
        };
        let fake = Fake::new(index_bytes(), false, true);
        let mut cache = MemCache::default();
        let opts = options(tmp.path(), &fake);
        let (ran, _) = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache).unwrap();
        assert_eq!(
            ran[0].status,
            RunStatus::Unsupported("link target outside the repository".into())
        );
        assert_eq!((*fake.runs.borrow(), cache.puts), (0, 0));
        assert!(
            !tmp.path().join("work").exists()
                || !tmp.path().join("work").read_dir().unwrap().any(|_| true)
        );
    }

    /// The cache key covers every exported file: one byte of a link target's file changes it
    /// (the job runs again), a file outside the root and every link target does not.
    #[test]
    fn a_link_targets_files_are_part_of_the_key() {
        let tmp = tempfile::tempdir().unwrap();
        let files = |core: &str, other: &str| {
            vec![
                entry("tools/e2e/package.json", "1"),
                entry("tools/e2e/package-lock.json", "2"),
                entry("tools/e2e/tsconfig.json", "3"),
                entry("packages/core/src/index.ts", core),
                entry("packages/other/a.ts", other),
            ]
        };
        let read = |blob: &str| {
            Ok(if blob == "2".repeat(40) {
                E2E_LOCK.as_bytes().to_vec()
            } else {
                b"{}".to_vec()
            })
        };
        let fake = Fake::new(index_bytes(), false, true);
        let mut cache = MemCache::default();
        let opts = options(tmp.path(), &fake);
        let mut run = |files: Vec<FileEntry>| {
            run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache)
                .unwrap()
                .0
        };
        let first = run(files("4", "5"));
        assert!(!first[0].cached);
        assert_eq!(first[0].link_targets, 1);
        assert!(run(files("4", "6"))[0].cached, "outside every target");
        assert!(
            !run(files("7", "6"))[0].cached,
            "a link target's file changed"
        );
        assert_eq!(
            *fake.runs.borrow(),
            4,
            "two jobs ran, two fetch-and-index each"
        );
    }

    /// Contract M6a S19, invariants 9 and 10: the export selection reads each lockfile entry
    /// once and each target's files as one range of the sorted paths (counted: 4× the files
    /// and links → at most 5× the work).
    fn selection_work_for(n: usize) -> u64 {
        let mut paths = vec![
            "app/package.json".to_owned(),
            "app/package-lock.json".to_owned(),
        ];
        let mut entries = Vec::new();
        for i in 0..n {
            paths.push(format!("app/src/f{i}.ts"));
            paths.push(format!("libs/l{i}/package.json"));
            paths.push(format!("libs/l{i}/index.ts"));
            paths.push(format!("other/o{i}.ts"));
            entries.push(format!(
                r#""node_modules/l{i}": {{"resolved": "../libs/l{i}", "link": true}}"#
            ));
        }
        let lock = format!(r#"{{"packages": {{{}}}}}"#, entries.join(", "));
        let blobs: BTreeMap<&str, &str> = paths.iter().map(|p| (p.as_str(), "b")).collect();
        let read = |path: &str| (path == "app/package-lock.json").then(|| lock.as_bytes().to_vec());
        let job = ts_job("app", rb_scip::jobs::NodeLock::PackageLock);
        rb_frameworks::work::take();
        assert_eq!(
            job_files(&job, &blobs, &read).unwrap().files.len(),
            3 * n + 2
        );
        rb_frameworks::work::take()
    }

    #[test]
    fn export_selection_grows_linearly() {
        let (small, large) = (selection_work_for(100), selection_work_for(400));
        assert!(large <= small * 5, "{small} -> {large}");
    }
}
