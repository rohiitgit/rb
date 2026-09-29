//! Commit manifest (design §3.1 stage 1) and its M0 graph assembly.

use serde_json::{Value, json};

use crate::graph::Attrs;
use crate::{Certainty, EdgeKind, Graph, GraphBuilder, NodeKey, NodeKind, Provenance};

/// Service that owns files outside any detected boundary (design §4.3). In M0 it owns every file.
pub const ROOT_SERVICE: &str = "(root)";

/// One blob in the commit tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Repo-relative path with `/` separators. Non-UTF-8 bytes are replaced (see `non_utf8_path`).
    pub path: String,
    pub blob_sha: String,
    pub size: u64,
    pub executable: bool,
    pub lang: Option<&'static str>,
    pub binary: bool,
    pub generated: bool,
    pub non_utf8_path: bool,
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

/// M0 assembly: one `File` node per blob, a single root `Service`, and `IN_SERVICE` edges.
pub fn graph_from_manifest(manifest: &Manifest) -> anyhow::Result<Graph> {
    let mut b = GraphBuilder::new();
    let root = NodeKey::new(NodeKind::Service, ROOT_SERVICE);
    b.add_node(
        root.clone(),
        ROOT_SERVICE,
        object(json!({"path": "", "detected_by": "default"})),
    )?;

    for f in &manifest.files {
        let key = NodeKey::new(NodeKind::File, f.path.clone());
        let name = f.path.rsplit('/').next().unwrap_or(&f.path);
        b.add_node(key.clone(), name, file_attrs(f))?;
        b.add_edge(
            key,
            EdgeKind::InService,
            root.clone(),
            Provenance::Manifest,
            Certainty::Exact,
            Attrs::new(),
        )?;
    }
    b.finish()
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
        ("non_utf8_path", f.non_utf8_path),
    ] {
        if set {
            a.insert(flag.into(), true.into());
        }
    }
    a
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
            size: 3,
            executable: false,
            lang: Some("rust"),
            binary: false,
            generated: false,
            non_utf8_path: false,
        }
    }

    #[test]
    fn every_file_is_in_the_root_service() {
        let m = Manifest {
            commit_sha: "c".repeat(40),
            files: vec![entry("src/a.rs"), entry("b.rs")],
            skipped: vec![],
        };
        let g = graph_from_manifest(&m).unwrap();
        assert_eq!(g.nodes().len(), 3);
        assert_eq!(g.edges().len(), 2);
        let a = g.nodes().iter().find(|n| n.key == "src/a.rs").unwrap();
        assert_eq!(a.name, "a.rs");
        assert_eq!(a.attrs["lang"], "rust");
        assert!(!a.attrs.contains_key("binary"));
    }
}
