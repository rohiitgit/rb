//! Constants across files for URL and path folding (contract M4a S12), shared by HTTP stitching
//! and the endpoint stage (contract M7c S1): files by number, and a name not bound in a file
//! resolved through M3a — an imported constant, `Type.CONST`, a default import's default
//! export — only when exactly one declaration answers.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rb_frameworks::fold::Lookup;
use rb_model::Span;
use rb_syntax::{FileFacts, Language};

use crate::resolve::Index;

/// One file's top-level non-member bindings (`None`) and type members (`Some(type)`) by name.
type Declared<'a> = HashMap<(Option<usize>, &'a str), Vec<usize>>;

pub(crate) struct Constants<'a, 'i> {
    files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
    index: &'i Index<'a>,
    pub paths: Vec<&'a str>,
    pub ids: HashMap<&'a str, usize>,
    /// Per file.
    declared: Vec<Declared<'a>>,
}

impl<'a, 'i> Constants<'a, 'i> {
    pub fn new(
        files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
        index: &'i Index<'a>,
    ) -> Self {
        let paths: Vec<&'a str> = files.keys().copied().collect();
        let ids = paths.iter().enumerate().map(|(i, p)| (*p, i)).collect();
        let declared = files
            .values()
            .map(|(_, facts)| {
                let mut table: Declared<'a> = HashMap::new();
                for (i, b) in facts.bindings.iter().enumerate() {
                    if b.member == b.scope.is_some() {
                        table.entry((b.scope, b.name.as_str())).or_default().push(i);
                    }
                }
                table
            })
            .collect();
        Constants {
            files,
            index,
            paths,
            ids,
            declared,
        }
    }

    /// `import x from "./m"`: the top-level binding `./m`'s default export names, as
    /// `(file, owner, name)` like [`Index::constant_declarations`] (contract M7c S1; one level,
    /// as for routers and axios instances).
    fn default_imports(&self, path: &'a str, name: &str) -> Vec<(&'a str, Option<usize>, String)> {
        let facts = self.files[path].1;
        facts
            .imports
            .iter()
            .zip(self.index.import_targets(path))
            .filter(|(import, _)| {
                import
                    .names
                    .iter()
                    .any(|n| n.local == name && n.name == "default")
            })
            .flat_map(|(_, targets)| targets.iter().copied())
            .filter_map(|target| {
                let exported = self.files.get(target)?.1.default_export.clone()?;
                Some((target, None, exported))
            })
            .collect()
    }
}

impl Lookup for Constants<'_, '_> {
    fn facts(&self, file: usize) -> &FileFacts {
        self.files[self.paths[file]].1
    }

    fn language(&self, file: usize) -> Language {
        self.files[self.paths[file]].0
    }

    fn external(&self, file: usize, name: &str, at: Span) -> Option<(usize, usize)> {
        let path = self.paths[file];
        let mut declarations = self.index.constant_declarations(path, name, at);
        if !name.contains('.') {
            declarations.extend(self.default_imports(path, name));
        }
        let found: BTreeSet<(usize, usize)> = declarations
            .into_iter()
            .flat_map(|(path, owner, declared)| {
                let id = self.ids[path];
                self.declared[id]
                    .get(&(owner, declared.as_str()))
                    .into_iter()
                    .flatten()
                    .map(move |&b| (id, b))
                    .collect::<Vec<_>>()
            })
            .collect();
        // Several declarations: the constant is not known, so nothing folds.
        match found.len() {
            1 => found.into_iter().next(),
            _ => None,
        }
    }

    /// No configuration here: stitching layers service properties on top.
    fn property(&self, _file: usize, _key: &str) -> Option<String> {
        None
    }
}
