//! Module resolution (design §5.4, level 3): which files of the snapshot an import refers to.
//! Imports that resolve to no file are external packages and give no edges.

use std::collections::BTreeMap;

use rb_syntax::{FileFacts, Import, Language};

/// Indexes of the snapshot's files by the names imports use for them.
pub(crate) struct Modules<'a> {
    files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
    /// Java package → its files.
    java_packages: BTreeMap<&'a str, Vec<&'a str>>,
    /// Every dotted suffix of a Python module name (`a.b.c`, `b.c`, `c`) → its files.
    python_modules: BTreeMap<String, Vec<&'a str>>,
    /// `go.mod` directory → module path.
    go_modules: &'a BTreeMap<String, String>,
    /// Directory → its Go files.
    go_dirs: BTreeMap<&'a str, Vec<&'a str>>,
}

/// Extensions a TS/JS specifier may omit, in the order TypeScript tries them.
const ES_EXTENSIONS: &[&str] = &[".ts", ".tsx", ".d.ts", ".js", ".jsx", ".mjs", ".cjs"];

impl<'a> Modules<'a> {
    pub(crate) fn new(
        files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
        go_modules: &'a BTreeMap<String, String>,
    ) -> Self {
        let mut java_packages: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        let mut python_modules: BTreeMap<String, Vec<&str>> = BTreeMap::new();
        let mut go_dirs: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (&path, (language, facts)) in files {
            match language {
                Language::Java => {
                    let package = facts.package.as_deref().unwrap_or_default();
                    java_packages.entry(package).or_default().push(path);
                }
                Language::Python => {
                    let mut segments: Vec<&str> = path.trim_end_matches(".py").split('/').collect();
                    if segments.last() == Some(&"__init__") {
                        segments.pop();
                    }
                    for k in 1..=segments.len() {
                        python_modules
                            .entry(segments[segments.len() - k..].join("."))
                            .or_default()
                            .push(path);
                    }
                }
                // Test files are never part of an imported package.
                Language::Go if !path.ends_with("_test.go") => {
                    go_dirs.entry(dir(path)).or_default().push(path);
                }
                _ => {}
            }
        }
        Modules {
            files,
            java_packages,
            python_modules,
            go_modules,
            go_dirs,
        }
    }

    /// The files `import` (written in the file at `from`) refers to, sorted. Empty for external
    /// modules.
    pub(crate) fn resolve(&self, from: &str, import: &Import) -> Vec<&'a str> {
        let Some((language, _)) = self.files.get(from) else {
            return Vec::new();
        };
        let mut out = match language {
            Language::TypeScript | Language::Tsx | Language::JavaScript => {
                self.ecmascript(from, &import.module).into_iter().collect()
            }
            Language::Java => self.java(import),
            Language::Python => self.python(from, import),
            Language::Go => self.go(&import.module),
        };
        out.sort_unstable();
        out.dedup();
        out
    }

    fn exists(&self, path: &str) -> Option<&'a str> {
        self.files.get_key_value(path).map(|(k, _)| *k)
    }

    /// Relative specifiers only; bare ones (`express`, `@scope/x`) are packages.
    fn ecmascript(&self, from: &str, spec: &str) -> Option<&'a str> {
        if !spec.starts_with('.') {
            return None;
        }
        let base = join(dir(from), spec)?;
        // `./x.js` may name `x.ts` (TypeScript's ESM convention).
        let stem = [".js", ".jsx", ".mjs", ".cjs"]
            .iter()
            .find_map(|e| base.strip_suffix(e));
        std::iter::once(base.clone())
            .chain(ES_EXTENSIONS.iter().map(|e| format!("{base}{e}")))
            .chain(
                stem.into_iter()
                    .flat_map(|s| [".ts", ".tsx", ".d.ts"].map(|e| format!("{s}{e}"))),
            )
            .chain(ES_EXTENSIONS.iter().map(|e| format!("{base}/index{e}")))
            .find_map(|p| self.exists(&p))
    }

    fn java(&self, import: &Import) -> Vec<&'a str> {
        // `import static a.B.m` / `a.B.*`: the class is the module.
        if import.is_static {
            return self.java_class(&import.module);
        }
        if import.wildcard {
            let package = self.package_files(&import.module);
            // `import a.B.*` may also name a class's nested types.
            return if package.is_empty() {
                self.java_class(&import.module)
            } else {
                package
            };
        }
        import
            .names
            .iter()
            .flat_map(|n| self.java_class(&format!("{}.{}", import.module, n.name)))
            .collect()
    }

    pub(crate) fn package_files(&self, package: &str) -> Vec<&'a str> {
        self.java_packages.get(package).cloned().unwrap_or_default()
    }

    /// Files declaring the top-level class `a.b.C`, or the outer class of a nested one
    /// (`a.b.C.Inner`).
    pub(crate) fn java_class(&self, qualified: &str) -> Vec<&'a str> {
        let mut path = qualified;
        while let Some((package, class)) = path.rsplit_once('.') {
            let found: Vec<&str> = self
                .package_files(package)
                .into_iter()
                .filter(|f| {
                    self.files[f]
                        .1
                        .definitions
                        .iter()
                        .any(|d| d.parent.is_none() && d.name == class)
                })
                .collect();
            if !found.is_empty() {
                return found;
            }
            path = package;
        }
        Vec::new()
    }

    fn python(&self, from: &str, import: &Import) -> Vec<&'a str> {
        let module = import.module.as_str();
        let dots = module.len() - module.trim_start_matches('.').len();
        let rest = &module[dots..];
        let lookup = |dotted: &str| -> Vec<&'a str> {
            if dots > 0 {
                // Relative: from the importing file's package, up `dots - 1` levels.
                let mut base = dir(from);
                for _ in 1..dots {
                    base = dir(base);
                }
                let rel = dotted.replace('.', "/");
                let stem = [base, rel.as_str()]
                    .into_iter()
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
                    .join("/");
                let init = if stem.is_empty() {
                    "__init__.py".to_owned()
                } else {
                    format!("{stem}/__init__.py")
                };
                [format!("{stem}.py"), init]
                    .iter()
                    // `from . import x` at the repo root has no `.py` module of its own.
                    .filter(|p| p.as_str() != ".py")
                    .find_map(|p| self.exists(p))
                    .into_iter()
                    .collect()
            } else {
                self.python_absolute(from, dotted)
            }
        };
        let join_dotted = |a: &str, b: &str| {
            if a.is_empty() {
                b.to_owned()
            } else {
                format!("{a}.{b}")
            }
        };
        let mut out = lookup(rest);
        // `from package import name`: `name` may be a submodule.
        for n in &import.names {
            out.extend(lookup(&join_dotted(rest, &n.name)));
        }
        out
    }

    /// Files whose module name ends with `dotted`; several are narrowed to those sharing the
    /// longest directory prefix with the importing file.
    fn python_absolute(&self, from: &str, dotted: &str) -> Vec<&'a str> {
        let Some(candidates) = self.python_modules.get(dotted) else {
            return Vec::new();
        };
        // Python 3 imports are absolute: `import json` inside the package `pkg` means the
        // top-level `json`, not `pkg/json.py`. A match counts only when the directory above the
        // matched segments is not itself a package (no `__init__.py`), i.e. a source root.
        let depth = dotted.split('.').count();
        let candidates: Vec<&'a str> = candidates
            .iter()
            .copied()
            .filter(|c| {
                let mut segments: Vec<&str> = c.trim_end_matches(".py").split('/').collect();
                if segments.last() == Some(&"__init__") {
                    segments.pop();
                }
                let root = segments[..segments.len().saturating_sub(depth)].join("/");
                let init = if root.is_empty() {
                    "__init__.py".to_owned()
                } else {
                    format!("{root}/__init__.py")
                };
                !self.files.contains_key(init.as_str())
            })
            .collect();
        let best = candidates
            .iter()
            .map(|c| shared_segments(dir(c), dir(from)))
            .max()
            .unwrap_or(0);
        candidates
            .iter()
            .copied()
            .filter(|c| shared_segments(dir(c), dir(from)) == best)
            .collect()
    }

    /// The package directory of a Go import path, under the `go.mod` with the longest matching
    /// module path.
    fn go(&self, path: &str) -> Vec<&'a str> {
        let module = self
            .go_modules
            .iter()
            .filter(|(_, m)| path == m.as_str() || path.starts_with(&format!("{m}/")))
            .max_by_key(|(_, m)| m.len());
        let Some((mod_dir, m)) = module else {
            return Vec::new();
        };
        let sub = path[m.len()..].trim_start_matches('/');
        let target = [mod_dir.as_str(), sub]
            .into_iter()
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        self.go_dirs
            .get(target.as_str())
            .cloned()
            .unwrap_or_default()
    }
}

/// The directory of a repo path (`""` at the root).
pub(crate) fn dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

/// `base` joined with a relative path, `..` and `.` resolved; `None` above the repo root.
fn join(base: &str, rel: &str) -> Option<String> {
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty()).collect();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            _ => parts.push(seg),
        }
    }
    Some(parts.join("/"))
}

/// Leading directory segments two directories share.
fn shared_segments(a: &str, b: &str) -> usize {
    a.split('/')
        .zip(b.split('/'))
        .take_while(|(x, y)| !x.is_empty() && x == y)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolves the imports of `from` against a snapshot of `(path, language, source)` files.
    fn resolved(
        files: &[(&str, Language, &str)],
        go_modules: &[(&str, &str)],
        from: &str,
    ) -> Vec<Vec<String>> {
        let facts: Vec<(String, Language, FileFacts)> = files
            .iter()
            .map(|(p, l, src)| {
                (
                    (*p).to_owned(),
                    *l,
                    rb_syntax::extract(*l, src.as_bytes()).unwrap(),
                )
            })
            .collect();
        let by_path: BTreeMap<&str, (Language, &FileFacts)> = facts
            .iter()
            .map(|(p, l, f)| (p.as_str(), (*l, f)))
            .collect();
        let go: BTreeMap<String, String> = go_modules
            .iter()
            .map(|(d, m)| ((*d).to_owned(), (*m).to_owned()))
            .collect();
        let modules = Modules::new(&by_path, &go);
        by_path[from]
            .1
            .imports
            .iter()
            .map(|i| {
                modules
                    .resolve(from, i)
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            })
            .collect()
    }

    #[test]
    fn typescript_relative_specifiers_probe_extensions_and_index_files() {
        let ts = Language::TypeScript;
        let got = resolved(
            &[
                (
                    "src/app.ts",
                    ts,
                    "import { a } from './auth';\nimport b from '../lib/util.js';\nimport * as m from './models';\nimport x from 'express';\nimport y from './missing';\n",
                ),
                ("src/auth.ts", ts, "export function a() {}\n"),
                ("lib/util.js", Language::JavaScript, "export default 1;\n"),
                ("src/models/index.ts", ts, "export class M {}\n"),
            ],
            &[],
            "src/app.ts",
        );
        assert_eq!(
            got,
            [
                vec!["src/auth.ts"],
                vec!["lib/util.js"],
                vec!["src/models/index.ts"],
                vec![],
                vec![]
            ]
        );
    }

    #[test]
    fn java_classes_wildcards_and_static_imports() {
        let j = Language::Java;
        let got = resolved(
            &[
                (
                    "web/src/Ctl.java",
                    j,
                    "package com.acme.web;\nimport com.acme.model.User;\nimport com.acme.model.*;\nimport static com.acme.util.Strings.trim;\nimport java.util.List;\nclass Ctl {}\n",
                ),
                (
                    "model/src/User.java",
                    j,
                    "package com.acme.model;\npublic class User {}\n",
                ),
                (
                    "model/src/Role.java",
                    j,
                    "package com.acme.model;\npublic enum Role { A }\n",
                ),
                (
                    "util/Strings.java",
                    j,
                    "package com.acme.util;\npublic class Strings { static String trim(String s) { return s; } }\n",
                ),
            ],
            &[],
            "web/src/Ctl.java",
        );
        assert_eq!(
            got,
            [
                vec!["model/src/User.java"],
                vec!["model/src/Role.java", "model/src/User.java"],
                vec!["util/Strings.java"],
                vec![],
            ]
        );
    }

    #[test]
    fn python_absolute_and_relative_modules() {
        let py = Language::Python;
        let got = resolved(
            &[
                (
                    "app/api/views.py",
                    py,
                    "import app.core.db\nfrom app.core import db as d\nfrom . import forms\nfrom ..core.db import Session\nfrom .forms import F\nimport requests\nfrom util import helper\n",
                ),
                ("app/core/db.py", py, "class Session: pass\n"),
                ("app/core/__init__.py", py, ""),
                ("app/api/__init__.py", py, ""),
                ("app/api/forms.py", py, "class F: pass\n"),
                ("app/util.py", py, "def helper(): pass\n"),
                ("tools/util.py", py, "def helper(): pass\n"),
            ],
            &[],
            "app/api/views.py",
        );
        assert_eq!(
            got,
            [
                vec!["app/core/db.py"],
                // `from package import name`: the package, plus the submodule when there is one.
                vec!["app/core/__init__.py", "app/core/db.py"],
                vec!["app/api/__init__.py", "app/api/forms.py"],
                vec!["app/core/db.py"],
                vec!["app/api/forms.py"],
                vec![],
                // Two `util` modules: the one sharing the longest directory prefix wins.
                vec!["app/util.py"],
            ]
        );
    }

    #[test]
    fn go_import_paths_map_to_package_directories() {
        let go = Language::Go;
        let got = resolved(
            &[
                (
                    "svc/cmd/main.go",
                    go,
                    "package main\nimport (\n  \"example.com/svc/store\"\n  s2 \"example.com/svc/store/sql\"\n  \"fmt\"\n)\n",
                ),
                ("svc/store/store.go", go, "package store\n"),
                ("svc/store/cache.go", go, "package store\n"),
                ("svc/store/sql/sql.go", go, "package sql\n"),
            ],
            &[("svc", "example.com/svc")],
            "svc/cmd/main.go",
        );
        assert_eq!(
            got,
            [
                vec!["svc/store/cache.go", "svc/store/store.go"],
                vec!["svc/store/sql/sql.go"],
                vec![],
            ]
        );
    }

    #[test]
    fn python_absolute_imports_do_not_match_modules_inside_packages() {
        let py = Language::Python;
        let got = resolved(
            &[
                ("pkg/app.py", py, "import json\nimport tools\n"),
                ("pkg/json.py", py, ""),
                ("pkg/__init__.py", py, ""),
                // `scripts/` is not a package: its modules are importable top-level names.
                ("scripts/tools.py", py, ""),
            ],
            &[],
            "pkg/app.py",
        );
        assert_eq!(got, [vec![], vec!["scripts/tools.py"]]);
    }
}
