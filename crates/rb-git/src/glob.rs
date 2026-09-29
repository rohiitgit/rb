//! Path patterns with GitHub CODEOWNERS semantics (design §4.2). Also used for `rb.toml` excludes,
//! so the Brain has exactly one glob dialect.
//!
//! - A leading `/`, or any `/` before the last character, anchors the pattern at the repo root;
//!   otherwise it matches at any depth (`*.js`, `apps/`).
//! - A trailing `/` (or a trailing `/**`) matches only directories, i.e. everything below them.
//! - A pattern that matches a directory also matches everything below it, **except** when its last
//!   segment is exactly `*` and it has no trailing `/`: `docs/*` owns `docs/a.md` but not
//!   `docs/sub/b.md` (GitHub-documented), while `docs/*/` covers everything below each child dir.
//! - `*` and `?` never cross `/`; a `**` segment matches zero or more directories.
//! - Not supported, and rejected: `!` negation, `[...]` character ranges, `\` escapes.
//!
//! Matching is case-sensitive.

use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    DoubleStar,
    Glob(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    raw: String,
    anchored: bool,
    dir_only: bool,
    segs: Vec<Seg>,
}

impl Pattern {
    /// Parses a pattern; the error explains which unsupported feature was used.
    pub fn parse(raw: &str) -> Result<Self, String> {
        if raw.is_empty() {
            return Err("empty pattern".into());
        }
        if raw.starts_with('!') {
            return Err("negation (`!`) is not supported".into());
        }
        if raw.contains(['[', ']']) {
            return Err("character ranges (`[...]`) are not supported".into());
        }
        if raw.contains('\\') {
            return Err("escaping (`\\`) is not supported".into());
        }

        let mut body = raw;
        let mut dir_only = false;
        if let Some(stripped) = body.strip_suffix("/**") {
            body = stripped;
            dir_only = true;
        } else if let Some(stripped) = body.strip_suffix('/') {
            body = stripped;
            dir_only = true;
        }
        // Anchored if it starts with `/` or has a `/` anywhere before its (stripped) end.
        let anchored = body.starts_with('/') || body.contains('/') || raw.ends_with("/**");
        let body = body.trim_start_matches('/');
        if body.is_empty() {
            // `/` or `/**`: everything in the repository.
            return Ok(Self {
                raw: raw.into(),
                anchored: true,
                dir_only: false,
                segs: vec![Seg::DoubleStar, Seg::Glob("*".into())],
            });
        }
        let segs = body
            .split('/')
            .filter(|s| !s.is_empty())
            .map(|s| {
                if s == "**" {
                    Seg::DoubleStar
                } else {
                    Seg::Glob(s.into())
                }
            })
            .collect();
        Ok(Self {
            raw: raw.into(),
            anchored,
            dir_only,
            segs,
        })
    }

    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Whether a repo-relative *file* path (no leading `/`) is covered by this pattern.
    pub fn matches(&self, path: &str) -> bool {
        let parts: Vec<&str> = path.split('/').collect();
        let last_is_star = matches!(self.segs.last(), Some(Seg::Glob(g)) if g == "*");
        let starts = if self.anchored { 0..1 } else { 0..parts.len() };
        for start in starts {
            for len in match_lengths(&self.segs, &parts[start..]) {
                let end = start + len;
                if len == 0 {
                    continue;
                }
                let is_whole_path = end == parts.len();
                if is_whole_path {
                    if !self.dir_only {
                        return true;
                    }
                } else if !last_is_star || self.dir_only {
                    // Matched a directory: everything below it is covered. Only a plain trailing
                    // `/*` (GitHub's `docs/*`) stops at direct children; `docs/*/` names directories.
                    return true;
                }
            }
        }
        false
    }
}

/// All `k` such that `segs` matches exactly `parts[..k]`.
fn match_lengths(segs: &[Seg], parts: &[&str]) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    match segs.split_first() {
        None => {
            out.insert(0);
        }
        Some((Seg::DoubleStar, rest)) => {
            for skip in 0..=parts.len() {
                out.extend(
                    match_lengths(rest, &parts[skip..])
                        .into_iter()
                        .map(|k| k + skip),
                );
            }
        }
        Some((Seg::Glob(g), rest)) => {
            if let Some((first, tail)) = parts.split_first()
                && segment_matches(g, first)
            {
                out.extend(match_lengths(rest, tail).into_iter().map(|k| k + 1));
            }
        }
    }
    out
}

/// Matches one path segment against a glob with `*` (any run) and `?` (one character).
fn segment_matches(glob: &str, text: &str) -> bool {
    let g: Vec<char> = glob.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut gi, mut ti) = (0, 0);
    let mut backtrack: Option<(usize, usize)> = None;
    while ti < t.len() {
        // The wildcard branch must come first: a literal `*` in the text must not consume it.
        if gi < g.len() && g[gi] == '*' {
            backtrack = Some((gi, ti));
            gi += 1;
        } else if gi < g.len() && (g[gi] == '?' || g[gi] == t[ti]) {
            gi += 1;
            ti += 1;
        } else if let Some((bg, bt)) = backtrack {
            gi = bg + 1;
            ti = bt + 1;
            backtrack = Some((bg, bt + 1));
        } else {
            return false;
        }
    }
    g[gi..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(pattern: &str, yes: &[&str], no: &[&str]) {
        let p = Pattern::parse(pattern).unwrap();
        for path in yes {
            assert!(p.matches(path), "{pattern:?} should match {path:?}");
        }
        for path in no {
            assert!(!p.matches(path), "{pattern:?} should not match {path:?}");
        }
    }

    #[test]
    fn segment_globs() {
        assert!(segment_matches("*.js", "a.js"));
        assert!(segment_matches("*.js", ".js"));
        assert!(!segment_matches("*.js", "a.jsx"));
        assert!(segment_matches("a?c", "abc"));
        assert!(!segment_matches("a?c", "ac"));
        assert!(segment_matches("*a*b*", "xxaxxbxx"));
        assert!(segment_matches("*", ""));
        assert!(segment_matches("ü*", "über"));
    }

    // Each case below is a pattern from GitHub's "About code owners" example file, with the
    // behaviour its accompanying comment documents.
    #[test]
    fn github_documented_examples() {
        check("*", &["README.md", "a/b/c.txt"], &[]);
        check("*.js", &["a.js", "src/deep/a.js"], &["a.jsx", "src/a.ts"]);
        check(
            "/build/logs/",
            &["build/logs/a.log", "build/logs/sub/b.log"],
            &["x/build/logs/a.log", "build/logs"],
        );
        check(
            "docs/*",
            &["docs/getting-started.md"],
            &["docs/build-app/troubleshooting.md", "x/docs/a.md"],
        );
        check(
            "apps/",
            &["apps/a.rb", "x/y/apps/z/a.rb"],
            &["apps", "myapps/a.rb"],
        );
        check("/docs/", &["docs/a.md", "docs/a/b.md"], &["x/docs/a.md"]);
        check(
            "**/logs",
            &[
                "build/logs/a",
                "scripts/logs/a",
                "deeply/nested/logs/a",
                "logs/a",
            ],
            &["logsx/a", "build/logs2/a"],
        );
    }

    #[test]
    fn gitignore_style_extras() {
        check("a/**", &["a/x", "a/x/y"], &["a", "b/a/x"]);
        check("src", &["src", "src/a.rs", "x/src/a.rs"], &["srcs/a.rs"]);
        check("/src", &["src", "src/a.rs"], &["x/src/a.rs"]);
        check("a/**/z.txt", &["a/z.txt", "a/b/c/z.txt"], &["b/a/z.txt"]);
        check("/", &["a", "a/b"], &[]);
        check("docs/*.md", &["docs/a.md"], &["docs/a/b.md", "docs/a.txt"]);
    }

    #[test]
    fn trailing_slash_star_is_recursive_below_each_child_directory() {
        check(
            "docs/*/",
            &["docs/sub/b.md", "docs/sub/x/y.md"],
            &["docs/a.md", "docs"],
        );
        check("*/", &["a/b.txt", "x/y/z"], &["a.txt"]);
    }

    #[test]
    fn literal_star_in_a_file_name_still_backtracks() {
        assert!(segment_matches("a*", "a*b"));
        assert!(segment_matches("*a", "*ba"));
        assert!(segment_matches("*", "*"));
    }

    #[test]
    fn unsupported_syntax_is_rejected() {
        for bad in ["!a", "[ab].js", "\\#a", ""] {
            assert!(Pattern::parse(bad).is_err(), "{bad:?}");
        }
    }
}
