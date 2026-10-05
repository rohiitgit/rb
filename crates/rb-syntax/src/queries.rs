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
    /// Anonymous function-like nodes. Definitions inside them are locals, not symbols (§8.2).
    pub anonymous_scopes: &'static [&'static str],
}

const JS_ANONYMOUS: &[&str] = &[
    "arrow_function",
    "function_expression",
    "generator_function",
    "class",
];
const JAVA_ANONYMOUS: &[&str] = &["lambda_expression", "object_creation_expression"];
const PYTHON_ANONYMOUS: &[&str] = &["lambda"];
const GO_ANONYMOUS: &[&str] = &["func_literal"];

/// TypeScript's `tags.scm` matches `module M {}` but not `namespace N {}` (`internal_module`),
/// and neither it nor JavaScript's covers type aliases or enums.
const TYPESCRIPT_SUPPLEMENT: &str = "
(internal_module name: (identifier) @name) @definition.module
(type_alias_declaration name: (type_identifier) @name) @definition.type
(enum_declaration name: (identifier) @name) @definition.class
";

/// Java's `tags.scm` has no enums, records or constructors (a record's compact canonical
/// constructor included).
const JAVA_SUPPLEMENT: &str = "
(enum_declaration name: (identifier) @name) @definition.class
(record_declaration name: (identifier) @name) @definition.class
(constructor_declaration name: (identifier) @name) @definition.method
(compact_constructor_declaration name: (identifier) @name) @definition.method
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
        TYPESCRIPT_SUPPLEMENT,
    ];
    match language {
        Language::TypeScript => TYPESCRIPT.get_or_init(|| {
            build(
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                &js_and_ts,
                JS_ANONYMOUS,
            )
        }),
        Language::Tsx => TSX.get_or_init(|| {
            build(
                tree_sitter_typescript::LANGUAGE_TSX.into(),
                &js_and_ts,
                JS_ANONYMOUS,
            )
        }),
        Language::JavaScript => JAVASCRIPT.get_or_init(|| {
            build(
                tree_sitter_javascript::LANGUAGE.into(),
                &[tree_sitter_javascript::TAGS_QUERY],
                JS_ANONYMOUS,
            )
        }),
        Language::Java => JAVA.get_or_init(|| {
            build(
                tree_sitter_java::LANGUAGE.into(),
                &[tree_sitter_java::TAGS_QUERY, JAVA_SUPPLEMENT],
                JAVA_ANONYMOUS,
            )
        }),
        Language::Python => PYTHON.get_or_init(|| {
            build(
                tree_sitter_python::LANGUAGE.into(),
                &[tree_sitter_python::TAGS_QUERY],
                PYTHON_ANONYMOUS,
            )
        }),
        Language::Go => GO.get_or_init(|| {
            build(
                tree_sitter_go::LANGUAGE.into(),
                &[tree_sitter_go::TAGS_QUERY],
                GO_ANONYMOUS,
            )
        }),
    }
}

/// Every query source, in a fixed order; input to the extractor fingerprint.
pub(crate) fn all_sources() -> String {
    [
        tree_sitter_javascript::TAGS_QUERY,
        tree_sitter_typescript::TAGS_QUERY,
        TYPESCRIPT_SUPPLEMENT,
        tree_sitter_java::TAGS_QUERY,
        JAVA_SUPPLEMENT,
        tree_sitter_python::TAGS_QUERY,
        tree_sitter_go::TAGS_QUERY,
    ]
    .join("\n")
}

fn build(
    language: tree_sitter::Language,
    sources: &[&str],
    anonymous_scopes: &'static [&'static str],
) -> Grammar {
    // The queries are bundled constants; a compile failure is a build defect, covered by tests.
    let query = Query::new(&language, &sources.join("\n"))
        .unwrap_or_else(|e| panic!("bundled tag query does not compile: {e}"));
    Grammar {
        language,
        query,
        anonymous_scopes,
    }
}
