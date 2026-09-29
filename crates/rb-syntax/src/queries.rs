//! Per-language Tree-sitter grammars and tag queries, compiled once.
//!
//! The grammars' own `tags.scm` are used as-is, plus small supplements where they miss definitions
//! the Brain needs. TypeScript's grammar extends JavaScript's and its `tags.scm` only covers the
//! TypeScript additions, so TS/TSX run both.

use std::sync::OnceLock;

use tree_sitter::Query;

use crate::Language;

pub(crate) struct Grammar {
    pub language: tree_sitter::Language,
    pub query: Query,
}

/// Java's `tags.scm` has no enums, records or constructors.
const JAVA_SUPPLEMENT: &str = "
(enum_declaration name: (identifier) @name) @definition.class
(record_declaration name: (identifier) @name) @definition.class
(constructor_declaration name: (identifier) @name) @definition.method
";

pub(crate) fn for_language(language: Language) -> &'static Grammar {
    static TYPESCRIPT: OnceLock<Grammar> = OnceLock::new();
    static TSX: OnceLock<Grammar> = OnceLock::new();
    static JAVASCRIPT: OnceLock<Grammar> = OnceLock::new();
    static JAVA: OnceLock<Grammar> = OnceLock::new();
    static PYTHON: OnceLock<Grammar> = OnceLock::new();
    static GO: OnceLock<Grammar> = OnceLock::new();
    let js_and_ts = [
        tree_sitter_javascript::TAGS_QUERY,
        tree_sitter_typescript::TAGS_QUERY,
    ];
    match language {
        Language::TypeScript => TYPESCRIPT.get_or_init(|| {
            build(
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                &js_and_ts,
            )
        }),
        Language::Tsx => {
            TSX.get_or_init(|| build(tree_sitter_typescript::LANGUAGE_TSX.into(), &js_and_ts))
        }
        Language::JavaScript => JAVASCRIPT.get_or_init(|| {
            build(
                tree_sitter_javascript::LANGUAGE.into(),
                &[tree_sitter_javascript::TAGS_QUERY],
            )
        }),
        Language::Java => JAVA.get_or_init(|| {
            build(
                tree_sitter_java::LANGUAGE.into(),
                &[tree_sitter_java::TAGS_QUERY, JAVA_SUPPLEMENT],
            )
        }),
        Language::Python => PYTHON.get_or_init(|| {
            build(
                tree_sitter_python::LANGUAGE.into(),
                &[tree_sitter_python::TAGS_QUERY],
            )
        }),
        Language::Go => GO.get_or_init(|| {
            build(
                tree_sitter_go::LANGUAGE.into(),
                &[tree_sitter_go::TAGS_QUERY],
            )
        }),
    }
}

fn build(language: tree_sitter::Language, sources: &[&str]) -> Grammar {
    // The queries are bundled constants; a compile failure is a build defect, covered by tests.
    let query = Query::new(&language, &sources.join("\n"))
        .unwrap_or_else(|e| panic!("bundled tag query does not compile: {e}"));
    Grammar { language, query }
}
