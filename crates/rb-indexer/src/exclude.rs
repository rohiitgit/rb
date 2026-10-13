//! Which files of a commit are excluded from analysis (contract M7e): vendored and built code
//! keeps its `File` node but is never parsed, scanned, detected as a project or indexed. The
//! decision is a pure function of the tree, made once, from the full tree, so excluding a
//! config file never changes what it decided.

use std::collections::{BTreeMap, BTreeSet};

use rb_git::glob::Pattern;
use rb_model::FileEntry;

/// `.gitattributes` patterns whose attributes mark vendored or generated code, scoped to the
/// directory of the file that declares them.
type AttrRule = (String, Pattern);

/// Every excluded path with its reason, the first matching rule of: `rb_toml` (an `rb.toml`
/// `ignore` glob), `go_vendor` (`D/vendor/**` beside `D/go.mod` and `D/vendor/modules.txt`),
/// `node_modules` (any such path segment), `gitattributes` (`linguist-vendored` or
/// `linguist-generated` set for a pattern of a `.gitattributes`, under its directory),
/// `ts_out_dir` (compiler outputs under a `tsconfig.json` `compilerOptions.outDir` inside that
/// tsconfig's directory). `read` returns a config file's text.
pub(crate) fn decide(
    files: &[FileEntry],
    read: &dyn Fn(&FileEntry) -> anyhow::Result<String>,
) -> anyhow::Result<BTreeMap<String, &'static str>> {
    let paths: BTreeSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let mut ignore = Vec::new();
    let mut attrs: Vec<AttrRule> = Vec::new();
    let mut outs: Vec<String> = Vec::new();
    for f in files.iter().filter(|f| !f.binary) {
        let (dir, name) = split(&f.path);
        match name {
            "rb.toml" if dir.is_empty() => ignore = rb_git::boundaries::rb_toml_ignore(&read(f)?)?,
            ".gitattributes" => attrs.extend(
                linguist_patterns(&read(f)?)
                    .into_iter()
                    .map(|p| (dir.to_owned(), p)),
            ),
            "tsconfig.json" => outs.extend(out_dir(dir, &read(f)?)),
            _ => {}
        }
    }
    let vendors: Vec<String> = paths
        .iter()
        .filter_map(|p| {
            let dir = p.strip_suffix("go.mod")?.trim_end_matches('/');
            let vendor = under(dir, "vendor");
            paths
                .contains(format!("{vendor}/modules.txt").as_str())
                .then(|| format!("{vendor}/"))
        })
        .collect();
    let reason = |path: &str| -> Option<&'static str> {
        if ignore.iter().any(|p| p.matches(path)) {
            Some("rb_toml")
        } else if vendors.iter().any(|v| path.starts_with(v.as_str())) {
            Some("go_vendor")
        } else if path.split('/').any(|s| s == "node_modules") {
            Some("node_modules")
        } else if attrs
            .iter()
            .any(|(dir, p)| relative(path, dir).is_some_and(|r| p.matches(r)))
        {
            Some("gitattributes")
        } else if outs.iter().any(|o| path.starts_with(o.as_str())) && compiler_output(path) {
            Some("ts_out_dir")
        } else {
            None
        }
    };
    Ok(paths
        .iter()
        .filter_map(|p| reason(p).map(|r| ((*p).to_owned(), r)))
        .collect())
}

/// `(directory, file name)`; the directory is `""` at the root.
fn split(path: &str) -> (&str, &str) {
    path.rsplit_once('/').unwrap_or(("", path))
}

/// `dir/name`, or `name` at the root.
fn under(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_owned()
    } else {
        format!("{dir}/{name}")
    }
}

/// `path` relative to `dir` when it lies under it.
fn relative<'p>(path: &'p str, dir: &str) -> Option<&'p str> {
    if dir.is_empty() {
        Some(path)
    } else {
        path.strip_prefix(dir)?.strip_prefix('/')
    }
}

/// The patterns a `.gitattributes` text marks `linguist-vendored` or `linguist-generated`
/// (bare or `=true`). Unset forms (`-attr`, `=false`) and patterns outside the shared dialect
/// mark nothing.
fn linguist_patterns(text: &str) -> Vec<Pattern> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let pattern = words.next().filter(|p| !p.starts_with('#'))?;
            let marked = words.any(|w| {
                let (attr, value) = w.split_once('=').unwrap_or((w, "true"));
                matches!(attr, "linguist-vendored" | "linguist-generated") && value == "true"
            });
            marked.then(|| Pattern::parse(pattern).ok()).flatten()
        })
        .collect()
}

/// A `tsconfig.json`'s output directory as a path prefix (`D/O/`), when its
/// `compilerOptions.outDir` resolves strictly inside the tsconfig's directory `D`.
fn out_dir(dir: &str, text: &str) -> Option<String> {
    let json: serde_json::Value =
        serde_json::from_str(&rb_scip::jobs::strip_json_comments(text)).ok()?;
    let raw = json.pointer("/compilerOptions/outDir")?.as_str()?;
    let mut parts: Vec<&str> = Vec::new();
    for seg in raw.split('/') {
        match seg {
            "" | "." => {}
            ".." => return None,
            s => parts.push(s),
        }
    }
    (!parts.is_empty()).then(|| format!("{}/", under(dir, &parts.join("/"))))
}

/// What `tsc` writes: JavaScript, declarations and source maps, never `.ts`/`.tsx` sources.
fn compiler_output(path: &str) -> bool {
    [".js", ".mjs", ".cjs", ".jsx", ".d.ts", ".map"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> FileEntry {
        FileEntry {
            path: path.to_owned(),
            blob_sha: String::new(),
            size: 0,
            executable: false,
            lang: None,
            binary: false,
            generated: false,
        }
    }

    /// `path reason` per excluded file, over `(path, text)` files.
    fn decided(files: &[(&str, &str)]) -> Vec<String> {
        let texts: BTreeMap<&str, &str> = files.iter().copied().collect();
        let entries: Vec<FileEntry> = files.iter().map(|(p, _)| entry(p)).collect();
        decide(&entries, &|f| Ok(texts[f.path.as_str()].to_owned()))
            .unwrap()
            .into_iter()
            .map(|(p, r)| format!("{p} {r}"))
            .collect()
    }

    #[test]
    fn go_vendor_needs_modules_txt_beside_go_mod() {
        assert_eq!(
            decided(&[
                ("go.mod", "module x\n"),
                ("main.go", ""),
                ("vendor/modules.txt", ""),
                ("vendor/github.com/gin/gin.go", ""),
                ("sub/go.mod", "module y\n"),
                ("sub/vendor/y.go", ""),
                ("vendored/a.go", ""),
            ]),
            [
                "vendor/github.com/gin/gin.go go_vendor",
                "vendor/modules.txt go_vendor",
            ]
        );
    }

    #[test]
    fn node_modules_is_a_whole_segment() {
        assert_eq!(
            decided(&[
                ("web/node_modules/lib/index.js", ""),
                ("node_modules/x.js", ""),
                ("node_modulesx/a.js", ""),
                ("web/src/node_modules.ts", ""),
            ]),
            [
                "node_modules/x.js node_modules",
                "web/node_modules/lib/index.js node_modules",
            ]
        );
    }

    #[test]
    fn gitattributes_linguist_marks_are_scoped_to_their_directory() {
        assert_eq!(
            decided(&[
                (
                    ".gitattributes",
                    "# comment\n*.min.js linguist-vendored\n*.css linguist-generated=true\n*.md -linguist-vendored\nlib/** linguist-vendored=false\ndocs/** linguist-documentation\n",
                ),
                ("app.min.js", ""),
                ("web/style.css", ""),
                ("README.md", ""),
                ("lib/x.js", ""),
                ("docs/a.md", ""),
                ("sub/.gitattributes", "gen/** linguist-generated\n"),
                ("sub/gen/a.ts", ""),
                ("other/gen/a.ts", ""),
            ]),
            [
                "app.min.js gitattributes",
                "sub/gen/a.ts gitattributes",
                "web/style.css gitattributes",
            ]
        );
    }

    #[test]
    fn ts_out_dir_excludes_compiler_outputs_inside_the_tsconfig_directory() {
        assert_eq!(
            decided(&[
                (
                    "web/tsconfig.json",
                    "{\n  // build\n  \"compilerOptions\": { \"outDir\": \"./dist\" }\n}\n",
                ),
                ("web/dist/a.js", ""),
                ("web/dist/a.d.ts", ""),
                ("web/dist/a.js.map", ""),
                ("web/dist/a.ts", ""),
                ("web/src/a.ts", ""),
                (
                    "bad/tsconfig.json",
                    "{\"compilerOptions\": {\"outDir\": \"../x\"}}"
                ),
                ("x/a.js", ""),
                // What an outDir escaping with `..` would wrongly reach if read inside `bad/`.
                ("bad/x/a.js", ""),
                (
                    "self/tsconfig.json",
                    "{\"compilerOptions\": {\"outDir\": \".\"}}"
                ),
                ("self/a.js", ""),
                ("broken/tsconfig.json", "{ not json"),
            ]),
            [
                "web/dist/a.d.ts ts_out_dir",
                "web/dist/a.js ts_out_dir",
                "web/dist/a.js.map ts_out_dir",
            ]
        );
    }

    /// The first matching rule names the reason: `rb.toml` wins over the defaults.
    #[test]
    fn rb_toml_ignore_comes_first() {
        assert_eq!(
            decided(&[
                ("rb.toml", "ignore = [\"generated/**\", \"vendor/**\"]\n"),
                ("generated/a.go", ""),
                ("go.mod", "module x\n"),
                ("vendor/modules.txt", ""),
                ("vendor/x.go", ""),
                ("main.go", ""),
                ("sub/rb.toml", "ignore = [\"main.go\"]\n"),
            ]),
            [
                "generated/a.go rb_toml",
                "vendor/modules.txt rb_toml",
                "vendor/x.go rb_toml",
            ]
        );
    }

    #[test]
    fn an_unreadable_ignore_pattern_fails_the_run() {
        let entries = [entry("rb.toml")];
        assert!(decide(&entries, &|_| Ok("ignore = [\"!x\"]\n".to_owned())).is_err());
    }
}
