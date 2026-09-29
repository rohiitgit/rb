//! Git ingestion (design §4.1): turns a commit into a sorted [`Manifest`] of blobs.
//!
//! The Git tree is authoritative; the working tree and `.gitignore` are never consulted.

use std::path::Path;

use anyhow::Context;
use gix::object::tree::EntryKind;
use rb_model::{FileEntry, Manifest, Skipped};

/// Bytes inspected for binary / generated detection (Git uses the same 8000-byte window for NUL).
const SNIFF_BYTES: usize = 8000;

/// Reads the tree of `rev` (e.g. `HEAD`, a branch, a SHA) in the repository at `repo_path`.
pub fn read_manifest(repo_path: &Path, rev: &str) -> anyhow::Result<Manifest> {
    let repo = gix::open(repo_path)
        .with_context(|| format!("opening git repository {}", repo_path.display()))?;
    let commit = repo
        .rev_parse_single(rev)
        .with_context(|| format!("resolving revision {rev:?}"))?
        .object()?
        .peel_to_commit()
        .with_context(|| format!("{rev:?} does not point to a commit"))?;
    let tree = commit.tree().context("reading commit tree")?;

    let mut recorder = gix::traverse::tree::Recorder::default();
    tree.traverse()
        .breadthfirst(&mut recorder)
        .context("traversing commit tree")?;

    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for entry in recorder.records {
        let non_utf8_path = std::str::from_utf8(&entry.filepath).is_err();
        let path = String::from_utf8_lossy(&entry.filepath).into_owned();
        match entry.mode.kind() {
            EntryKind::Tree => {}
            EntryKind::Commit => skipped.push(Skipped {
                path,
                reason: "submodule",
            }),
            EntryKind::Link => skipped.push(Skipped {
                path,
                reason: "symlink",
            }),
            kind @ (EntryKind::Blob | EntryKind::BlobExecutable) => {
                let blob = repo
                    .find_blob(entry.oid)
                    .with_context(|| format!("reading blob {} ({path})", entry.oid))?;
                let head = &blob.data[..blob.data.len().min(SNIFF_BYTES)];
                files.push(FileEntry {
                    lang: language_for_path(&path),
                    blob_sha: entry.oid.to_string(),
                    size: blob.data.len() as u64,
                    executable: kind == EntryKind::BlobExecutable,
                    binary: head.contains(&0),
                    generated: has_generated_marker(head),
                    non_utf8_path,
                    path,
                });
            }
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    skipped.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Manifest {
        commit_sha: commit.id.to_string(),
        files,
        skipped,
    })
}

/// Language by file name / extension. Only languages the design plans to parse are named;
/// everything else is `None` and stays a plain `File` node.
pub fn language_for_path(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    Some(match ext.as_str() {
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "py" | "pyi" => "python",
        "go" => "go",
        "rs" => "rust",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" | "hxx" => "cpp",
        "cs" => "csharp",
        "rb" => "ruby",
        "sql" => "sql",
        "proto" => "protobuf",
        "prisma" => "prisma",
        _ => return None,
    })
}

/// Common generated-code headers (design §4.1). `.gitattributes linguist-generated` arrives in M1.
fn has_generated_marker(head: &[u8]) -> bool {
    let first_kb = &head[..head.len().min(1024)];
    let text = String::from_utf8_lossy(first_kb);
    text.contains("@generated") || (text.contains("Code generated") && text.contains("DO NOT EDIT"))
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "rb")
            .env("GIT_AUTHOR_EMAIL", "rb@example.invalid")
            .env("GIT_COMMITTER_NAME", "rb")
            .env("GIT_COMMITTER_EMAIL", "rb@example.invalid")
            .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    #[test]
    fn manifest_matches_git_ls_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git(dir, &["init", "-q", "-b", "main"]);
        write(dir, "src/main.rs", b"fn main() {}\n");
        write(
            dir,
            "src/gen/api.ts",
            b"// @generated by protoc\nexport {}\n",
        );
        write(dir, "img/logo.png", b"\x89PNG\r\n\x1a\n\0\0\0");
        write(dir, "run.sh", b"#!/bin/sh\n");
        write(dir, "README", b"hello\n");
        std::os::unix::fs::symlink("src/main.rs", dir.join("link.rs")).unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["update-index", "--chmod=+x", "run.sh"]);
        // A gitlink (submodule) entry without needing a real submodule.
        let fake = "0123456789abcdef0123456789abcdef01234567";
        git(
            dir,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("160000,{fake},vendor/sub"),
            ],
        );
        git(dir, &["commit", "-q", "-m", "init"]);

        let m = read_manifest(dir, "HEAD").unwrap();
        assert_eq!(m.commit_sha, git(dir, &["rev-parse", "HEAD"]));

        let paths: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            [
                "README",
                "img/logo.png",
                "run.sh",
                "src/gen/api.ts",
                "src/main.rs"
            ]
        );
        for f in &m.files {
            assert_eq!(
                f.blob_sha,
                git(dir, &["rev-parse", &format!("HEAD:{}", f.path)]),
                "{}",
                f.path
            );
        }
        let get = |p: &str| m.files.iter().find(|f| f.path == p).unwrap();
        assert!(get("img/logo.png").binary);
        assert!(!get("src/main.rs").binary);
        assert!(get("run.sh").executable);
        assert!(get("src/gen/api.ts").generated);
        assert_eq!(get("src/main.rs").lang, Some("rust"));
        assert_eq!(get("src/main.rs").size, 13);
        assert_eq!(get("README").lang, None);

        let skipped: Vec<(&str, &str)> = m
            .skipped
            .iter()
            .map(|s| (s.path.as_str(), s.reason))
            .collect();
        assert_eq!(
            skipped,
            [("link.rs", "symlink"), ("vendor/sub", "submodule")]
        );
    }

    #[test]
    fn unknown_revision_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q"]);
        assert!(read_manifest(tmp.path(), "HEAD").is_err());
    }

    #[test]
    fn language_detection() {
        assert_eq!(language_for_path("a/B.JAVA"), Some("java"));
        assert_eq!(language_for_path("x.tsx"), Some("tsx"));
        assert_eq!(language_for_path("Makefile"), None);
        assert_eq!(language_for_path(".env"), None);
    }
}
