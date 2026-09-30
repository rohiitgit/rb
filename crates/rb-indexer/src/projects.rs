//! Running a commit's SCIP jobs (design §5.2): each build root is hashed, looked up in the index
//! cache, and only on a miss exported and indexed in a container. The indexer never talks to the
//! database; the caller plugs in the cache, as for syntax facts.

use std::collections::BTreeMap;
use std::path::PathBuf;

use rb_model::FileEntry;
use rb_scip::{Budget, RunStatus, Runner, ScipIndex};

/// A recorded indexer run, content-addressed by `(project_input_hash, image)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScipCacheEntry {
    /// `ok`, `failed` or `timeout`.
    pub status: String,
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

/// How SCIP jobs run for one `index_commit`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScipOptions {
    /// `false` records every job as `skipped` without touching Docker.
    pub enabled: bool,
    /// Holds one export directory per job (`<input_hash[..16]>`), removed after the run. Must be a
    /// path the container runtime can bind-mount.
    pub work_dir: PathBuf,
    /// Package-manager caches shared across jobs.
    pub cache_dir: PathBuf,
    pub budget: Budget,
}

impl ScipOptions {
    /// No jobs run; every project stays heuristic.
    pub fn disabled() -> Self {
        ScipOptions {
            enabled: false,
            work_dir: PathBuf::new(),
            cache_dir: PathBuf::new(),
            budget: Budget::default(),
        }
    }
}

/// One build root's outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct ScipProject {
    /// Repo-relative build root (`""` for the repository root).
    pub root: String,
    pub indexer: &'static str,
    /// The image the job runs in, pinned by digest.
    pub image: &'static str,
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
}

/// Whether jobs run at all.
pub(crate) enum Mode<'a> {
    /// Every job is `skipped` with this reason.
    Skip(String),
    Run(&'a dyn Runner),
}

/// Runs (or serves from the cache) every job of `files`, in job order.
pub(crate) fn run_projects(
    files: &[FileEntry],
    read_blob: &dyn Fn(&str) -> anyhow::Result<Vec<u8>>,
    mode: &Mode<'_>,
    options: &ScipOptions,
    cache: &mut dyn ScipCache,
) -> anyhow::Result<Vec<ScipProject>> {
    let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let blobs: BTreeMap<&str, &str> = files
        .iter()
        .map(|f| (f.path.as_str(), f.blob_sha.as_str()))
        .collect();
    let mut out = Vec::new();
    for job in rb_scip::jobs(&paths) {
        let image = rb_scip::image_for(&job);
        let mut project = ScipProject {
            root: job.root.clone(),
            indexer: job.indexer.as_str(),
            image,
            status: RunStatus::Skipped(String::new()),
            index: None,
            elapsed_ms: 0,
            cached: false,
            stderr_tail: String::new(),
            unpinned: job.unpinned,
        };
        let runner = match mode {
            Mode::Skip(reason) => {
                project.status = RunStatus::Skipped(reason.clone());
                out.push(project);
                continue;
            }
            Mode::Run(runner) => *runner,
        };
        let under_root: Vec<(&str, &str)> = blobs
            .iter()
            .filter(|(path, _)| in_root(path, &job.root))
            .map(|(p, b)| (*p, *b))
            .collect();
        let input_hash = rb_scip::project_input_hash(&job.root, under_root.iter().copied());
        let entry = match cache.get(&input_hash, image)? {
            Some(entry) => {
                project.cached = true;
                entry
            }
            None => {
                let exported = under_root
                    .iter()
                    .map(|(path, blob)| Ok((relative(path, &job.root), read_blob(blob)?)))
                    .collect::<anyhow::Result<Vec<_>>>()?;
                let run = rb_scip::run_job(
                    &job,
                    &exported,
                    &options.work_dir.join(&input_hash[..16]),
                    &options.cache_dir,
                    &options.budget,
                    runner,
                )?;
                if let RunStatus::Unsupported(_) = run.status {
                    project.status = run.status;
                    out.push(project);
                    continue;
                }
                let entry = ScipCacheEntry {
                    status: run.status.as_str().to_owned(),
                    index: run.index,
                    stderr_tail: run.stderr_tail,
                    elapsed_ms: u64::try_from(run.elapsed.as_millis()).unwrap_or(u64::MAX),
                };
                cache.put(&input_hash, image, &entry)?;
                entry
            }
        };
        project.status = RunStatus::from_cached(&entry.status);
        project.elapsed_ms = entry.elapsed_ms;
        project.stderr_tail = entry.stderr_tail;
        if let (RunStatus::Ok, Some(bytes)) = (&project.status, &entry.index) {
            let source_of = |doc: &str| {
                let path = join_root(&job.root, doc);
                blobs.get(path.as_str()).and_then(|b| read_blob(b).ok())
            };
            match rb_scip::decode(bytes, &source_of) {
                Ok(index) => project.index = Some(index),
                Err(e) => {
                    project.status = RunStatus::Failed;
                    project.stderr_tail = format!("decoding index.scip: {e:#}");
                }
            }
        }
        out.push(project);
    }
    Ok(out)
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

    struct Fake {
        runs: RefCell<usize>,
        index: Vec<u8>,
        fail: bool,
    }

    impl Runner for Fake {
        fn run(&self, run: &ContainerRun) -> anyhow::Result<Outcome> {
            *self.runs.borrow_mut() += 1;
            if run.args.first().map(String::as_str) == Some("index") && !self.fail {
                std::fs::write(run.workdir.join("index.scip"), &self.index)?;
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

    fn options(tmp: &std::path::Path) -> ScipOptions {
        ScipOptions {
            enabled: true,
            work_dir: tmp.join("work"),
            cache_dir: tmp.join("cache"),
            budget: Budget::default(),
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
        let fake = Fake {
            runs: RefCell::new(0),
            index: index_bytes(),
            fail: false,
        };
        let mut cache = MemCache::default();
        let opts = options(tmp.path());
        let cold = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache).unwrap();
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

        let warm = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache).unwrap();
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
        let fake = Fake {
            runs: RefCell::new(0),
            index: Vec::new(),
            fail: true,
        };
        let mut cache = MemCache::default();
        let opts = options(tmp.path());
        let skipped = run_projects(
            &files,
            &read,
            &Mode::Skip("--no-scip".into()),
            &opts,
            &mut cache,
        )
        .unwrap();
        let statuses: Vec<(&str, &str, Option<&str>)> = skipped
            .iter()
            .map(|p| (p.root.as_str(), p.status.as_str(), p.status.reason()))
            .collect();
        assert_eq!(
            statuses,
            [
                ("svc", "skipped", Some("--no-scip")),
                ("web", "skipped", Some("--no-scip")),
            ]
        );
        assert_eq!(*fake.runs.borrow(), 0);

        let ran = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache).unwrap();
        assert_eq!(ran[0].status.as_str(), "unsupported");
        assert!(ran[0].status.reason().unwrap().contains("scip-java"));
        // The TypeScript job's fetch failed: recorded, cached as such, and served from the cache
        // next time without another attempt.
        assert_eq!(ran[1].status, RunStatus::Failed);
        assert!(ran[1].index.is_none());
        assert_eq!(ran[1].stderr_tail, "boom");
        assert_eq!(cache.puts, 1, "unsupported jobs are not cached");
        assert_eq!(*fake.runs.borrow(), 1);
        let again = run_projects(&files, &read, &Mode::Run(&fake), &opts, &mut cache).unwrap();
        assert!(again[1].cached && again[1].status == RunStatus::Failed);
        assert_eq!(*fake.runs.borrow(), 1);
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
}
