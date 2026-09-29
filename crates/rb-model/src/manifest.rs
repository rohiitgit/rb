//! Commit manifest (design §3.1 stage 1).

/// One blob in the commit tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Repo-relative path with `/` separators (always valid UTF-8; other paths are skipped).
    pub path: String,
    pub blob_sha: String,
    pub size: u64,
    pub executable: bool,
    pub lang: Option<&'static str>,
    pub binary: bool,
    pub generated: bool,
}

/// A tree entry that is deliberately not a `File` node, with the reason (recorded as a diagnostic).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: String,
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub commit_sha: String,
    /// Sorted by `path`.
    pub files: Vec<FileEntry>,
    /// Sorted by `path`.
    pub skipped: Vec<Skipped>,
}
