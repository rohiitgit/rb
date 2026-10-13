//! Git ingestion (design §4.1): turns a commit into a sorted [`Manifest`] of blobs, and reads blob
//! contents for later stages. CODEOWNERS (§4.2) and service boundaries (§4.3) live in submodules.
//!
//! The Git tree is authoritative; the working tree and `.gitignore` are never consulted.

pub mod boundaries;
pub mod codeowners;
pub mod glob;

use std::path::Path;

use anyhow::Context;
use gix::object::tree::EntryKind;
use rb_model::{FileEntry, Manifest, Skipped};

/// Bytes inspected for binary / generated detection (Git uses the same 8000-byte window for NUL).
const SNIFF_BYTES: usize = 8000;

/// Blobs larger than this are sniffed by streaming their first bytes (`git cat-file`) instead of
/// loading them: gix reads whole objects, which made a 1 GiB blob cost about 2 GiB of memory to
/// sniff 8 KB (contract M7b S5).
const STREAM_SNIFF_ABOVE: u64 = 64 << 20;

/// An open repository.
pub struct Repo {
    inner: gix::Repository,
}

impl Repo {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let inner = gix::open(path)
            .with_context(|| format!("opening git repository {}", path.display()))?;
        Ok(Self { inner })
    }

    /// Resolves `rev` to a commit SHA without reading the tree.
    pub fn resolve_commit(&self, rev: &str) -> anyhow::Result<String> {
        let commit = self
            .inner
            .rev_parse_single(rev)
            .with_context(|| format!("resolving revision {rev:?}"))?
            .object()?
            .peel_to_commit()
            .with_context(|| format!("{rev:?} does not point to a commit"))?;
        Ok(commit.id.to_string())
    }

    /// Reads the tree of `rev` (e.g. `HEAD`, a branch, a SHA).
    pub fn manifest(&self, rev: &str) -> anyhow::Result<Manifest> {
        self.manifest_with(rev, STREAM_SNIFF_ABOVE)
    }

    /// [`Repo::manifest`] with blobs over `stream_above` bytes sniffed by streaming.
    fn manifest_with(&self, rev: &str, stream_above: u64) -> anyhow::Result<Manifest> {
        let commit = self
            .inner
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
            let path = String::from_utf8_lossy(&entry.filepath).into_owned();
            if entry.mode.kind() == EntryKind::Tree {
                continue;
            }
            // Keys are UTF-8 text (as is Postgres `text`); a lossy key could merge distinct files.
            if std::str::from_utf8(&entry.filepath).is_err() {
                skipped.push(Skipped {
                    path,
                    reason: "non_utf8_path",
                });
                continue;
            }
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
                    let size = self
                        .inner
                        .find_header(entry.oid)
                        .with_context(|| format!("reading blob header {} ({path})", entry.oid))?
                        .size();
                    let want = SNIFF_BYTES.min(usize::try_from(size).unwrap_or(usize::MAX));
                    let streamed = (size > stream_above)
                        .then(|| self.blob_head_streamed(entry.oid, want))
                        .flatten()
                        .filter(|head| head.len() == want);
                    let head = match streamed {
                        Some(head) => head,
                        None => {
                            let blob = self
                                .inner
                                .find_blob(entry.oid)
                                .with_context(|| format!("reading blob {} ({path})", entry.oid))?;
                            blob.data[..blob.data.len().min(SNIFF_BYTES)].to_vec()
                        }
                    };
                    let head = head.as_slice();
                    files.push(FileEntry {
                        lang: language_for_path(&path),
                        blob_sha: entry.oid.to_string(),
                        size,
                        executable: kind == EntryKind::BlobExecutable,
                        binary: head.contains(&0),
                        generated: has_generated_marker(head),
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

    /// The first `n` bytes of a blob, streamed through `git cat-file` and cut off there, so a
    /// huge blob is never held in memory (contract M7b S5). `None` when the CLI cannot be run;
    /// a short read is the caller's to reject.
    fn blob_head_streamed(&self, id: gix::ObjectId, n: usize) -> Option<Vec<u8>> {
        use std::io::Read;
        let mut child = std::process::Command::new("git")
            .arg("--git-dir")
            .arg(self.inner.git_dir())
            .args(["cat-file", "blob", &id.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .ok()?;
        let mut head = Vec::with_capacity(n);
        let read = child
            .stdout
            .take()
            .map(|out| out.take(n as u64).read_to_end(&mut head));
        let _ = child.kill();
        let _ = child.wait();
        read?.ok()?;
        Some(head)
    }

    /// Reads a blob's bytes by its hex SHA.
    pub fn blob(&self, sha: &str) -> anyhow::Result<Vec<u8>> {
        let id = gix::ObjectId::from_hex(sha.as_bytes())
            .map_err(|e| anyhow::anyhow!("invalid object id {sha:?}: {e}"))?;
        let mut blob = self
            .inner
            .find_blob(id)
            .with_context(|| format!("reading blob {sha}"))?;
        Ok(blob.take_data())
    }

    /// Reads a blob as UTF-8 text, replacing invalid sequences.
    pub fn blob_text(&self, sha: &str) -> anyhow::Result<String> {
        Ok(String::from_utf8_lossy(&self.blob(sha)?).into_owned())
    }
}

/// Drops a leading UTF-8 byte order mark. Parsers call this; byte spans stay relative to the blob.
pub fn strip_bom(text: &str) -> &str {
    text.strip_prefix('\u{feff}').unwrap_or(text)
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
        // No grammar in rb; named so their files are reported as unanalysed code (M7g S1).
        "php" => "php",
        "scala" | "sc" => "scala",
        "swift" => "swift",
        "dart" => "dart",
        "ex" | "exs" => "elixir",
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

        let m = Repo::open(dir).unwrap().manifest("HEAD").unwrap();
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
        let repo = Repo::open(dir).unwrap();
        assert_eq!(
            repo.blob(&get("src/main.rs").blob_sha).unwrap(),
            b"fn main() {}\n"
        );
        assert!(repo.blob("not-a-sha").is_err());

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

    /// Contract M7b S5 (stress F15): a blob over the streaming threshold is sniffed from its
    /// first bytes through `git cat-file`, never loaded whole; the manifest is the same as when
    /// every blob is loaded (`stream_above: 0` streams them all). The fixture's sniff edges: a
    /// NUL just past the window, a NUL just inside it, a generated marker past 512 bytes.
    #[test]
    fn streamed_sniffing_matches_loading() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git(dir, &["init", "-q", "-b", "main"]);
        let mut late_nul = vec![b'a'; SNIFF_BYTES];
        late_nul.push(0);
        let mut early_nul = vec![b'a'; SNIFF_BYTES - 1];
        early_nul.push(0);
        let mut generated = vec![b' '; 600];
        generated.extend_from_slice(b"// @generated\n");
        write(dir, "late.txt", &late_nul);
        write(dir, "early.bin", &early_nul);
        write(dir, "gen.ts", &generated);
        write(dir, "small.rs", b"fn main() {}\n");
        write(dir, "empty", b"");
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", "init"]);
        let repo = Repo::open(dir).unwrap();
        let loaded = repo.manifest_with("HEAD", u64::MAX).unwrap();
        let streamed = repo.manifest_with("HEAD", 0).unwrap();
        assert_eq!(streamed, loaded);
        let get = |p: &str| loaded.files.iter().find(|f| f.path == p).unwrap();
        assert!(!get("late.txt").binary);
        assert!(get("early.bin").binary);
        assert!(get("gen.ts").generated);
        assert_eq!(get("late.txt").size, SNIFF_BYTES as u64 + 1);
    }

    #[test]
    fn non_utf8_paths_are_skipped_not_merged() {
        use std::os::unix::ffi::OsStrExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        git(dir, &["init", "-q", "-b", "main"]);
        write(dir, "ok.txt", b"x\n");
        git(dir, &["add", "-A"]);
        let blob = git(dir, &["hash-object", "-w", "ok.txt"]);
        for name in [&b"a\xff.txt"[..], &b"a\xfe.txt"[..]] {
            let mut info = format!("100644,{blob},").into_bytes();
            info.extend_from_slice(name);
            let out = Command::new("git")
                .args(["update-index", "--add", "--cacheinfo"])
                .arg(std::ffi::OsStr::from_bytes(&info))
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        git(dir, &["commit", "-q", "-m", "x"]);

        let m = Repo::open(dir).unwrap().manifest("HEAD").unwrap();
        let paths: Vec<&str> = m.files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, ["ok.txt"]);
        let reasons: Vec<&str> = m.skipped.iter().map(|s| s.reason).collect();
        assert_eq!(reasons, ["non_utf8_path", "non_utf8_path"]);
    }

    #[test]
    fn unknown_revision_is_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        git(tmp.path(), &["init", "-q"]);
        assert!(Repo::open(tmp.path()).unwrap().manifest("HEAD").is_err());
    }

    #[test]
    fn language_detection() {
        assert_eq!(language_for_path("a/B.JAVA"), Some("java"));
        assert_eq!(language_for_path("x.tsx"), Some("tsx"));
        assert_eq!(language_for_path("Makefile"), None);
        assert_eq!(language_for_path(".env"), None);
        // Contract M7g S1: languages rb has no grammar for are still named, so a file of one
        // is reported as unanalysed code, not as "other".
        for (path, lang) in [
            ("app/Http/Kernel.php", "php"),
            ("src/Main.scala", "scala"),
            ("Sources/App.swift", "swift"),
            ("lib/main.dart", "dart"),
            ("lib/app.ex", "elixir"),
            ("test/app_test.exs", "elixir"),
        ] {
            assert_eq!(language_for_path(path), Some(lang), "{path}");
        }
    }
}
